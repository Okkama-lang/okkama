//! Conservative unique-cache proofs, independent of optional OIR passes (#581).
//!
//! Async calls allocate frames; a plain action may also return a fresh frame.
//! A single SSA use alone is insufficient: require fresh provenance, reject
//! every publication/alias, and keep bound extraction at its original owner.
use super::ctx::Ctx;
use crate::ir::{Function, Inst, InstKind, TermKind, Type};

/// Re-entering an await without allocating again would consume twice, even
/// with only one syntactic use. Cutting the definition block must break every
/// cycle through the await (including unwind edges).
fn once_per_definition(function: &Function, definition: usize, awaiting: usize) -> bool {
    if definition == awaiting { return true; }
    let mut seen = vec![false; function.blocks.len()];
    let mut pending = crate::ir::cfg::all_successors(function, crate::ir::BlockId(awaiting));
    while let Some(block) = pending.pop() {
        if block.0 == definition { continue; }
        if block.0 == awaiting { return false; }
        if !std::mem::replace(&mut seen[block.0], true) {
            pending.extend(crate::ir::cfg::all_successors(function, block));
        }
    }
    true
}

fn bookkeeping(kind: &InstKind) -> bool {
    matches!(kind, InstKind::RegionTrack { .. } | InstKind::RegionAdopt { .. }
        | InstKind::RegionTransfer { .. } | InstKind::RegionLeave { .. }
        | InstKind::RegionSettle { .. } | InstKind::RegionUnwind { .. }
        | InstKind::RegionSettleUnwind { .. })
}

fn uses(function: &Function) -> Vec<usize> {
    let mut uses = vec![0; function.values.len()];
    for block in &function.blocks {
        for inst in &block.insts {
            if !bookkeeping(&inst.kind) {
                for value in inst.kind.operands() { uses[value.0] += 1; }
            }
        }
        for value in block.term.kind.operands() { uses[value.0] += 1; }
        for target in block.term.kind.targets().into_iter().chain(block.unwind.as_ref()) {
            for value in &target.args { uses[value.0] += 1; }
        }
    }
    uses
}

/// A definition is fresh only if its producer cannot return a stored alias.
/// Spawn preserves the proof only when it is the input's sole real use.
fn fresh_values(function: &Function, fresh_calls: &[bool], fresh_results: &[bool], async_calls: &[bool], uses: &[usize]) -> Vec<Option<usize>> {
    let mut fresh = vec![None; function.values.len()];
    let mut fresh_outcome = vec![false; function.values.len()];
    for (block_index, block) in function.blocks.iter().enumerate() {
        for inst in &block.insts {
            let Some(value) = inst.result else { continue };
            if !matches!(function.values[value.0], Type::Future(_)) { continue; }
            let unique = match inst.kind {
                InstKind::Call { func, .. } => {
                    fresh_outcome[value.0] = async_calls[func.0] && fresh_results[func.0];
                    fresh_calls[func.0]
                }
                InstKind::FutSpawn(input) => {
                    fresh_outcome[value.0] = fresh_outcome[input.0];
                    fresh[input.0] == Some(block_index) && uses[input.0] == 1
                }
                // Only extraction exposes an exclusive outcome. Ordinary
                // await of even a fresh-producing action can have other views.
                InstKind::AwaitTake(input) => fresh_outcome[input.0],
                _ => false,
            };
            if unique { fresh[value.0] = Some(block_index); }
        }
    }
    fresh
}

pub(super) fn take_unique(ctx: &mut Ctx<'_, '_>, functions: &mut [Function]) {
    let all_uses: Vec<_> = functions.iter().map(uses).collect();
    let async_calls: Vec<_> = functions.iter().map(Function::is_async).collect();
    let mut fresh_calls = async_calls.clone();
    let mut fresh_results = vec![false; functions.len()];
    loop {
        let mut changed = false;
        for (index, function) in functions.iter().enumerate() {
            if fresh_results[index] || !matches!(function.result, crate::ir::ResultType::Value(Type::Future(_))) { continue; }
            let fresh = fresh_values(function, &fresh_calls, &fresh_results, &async_calls, &all_uses[index]);
            let mut returned = false;
            let proven = function.blocks.iter().all(|body| {
                if let TermKind::Ret(Some(value)) = body.term.kind {
                    returned = true;
                    fresh[value.0].is_some() && all_uses[index][value.0] == 1
                } else { true }
            });
            if returned && proven {
                fresh_results[index] = true;
                fresh_calls[index] = true;
                changed = true;
            }
        }
        if !changed { break; }
    }
    for (index, function) in functions.iter_mut().enumerate() {
        if !function.blocks.iter().flat_map(|block| &block.insts).any(|inst| matches!(inst.kind, InstKind::Await(_))) {
            continue;
        }
        let fresh = fresh_values(function, &fresh_calls, &fresh_results, &async_calls, &all_uses[index]);
        let eligible: Vec<_> = function.blocks.iter().enumerate().map(|(block_index, block)| {
            block.insts.iter().filter_map(|inst| match inst.kind {
                InstKind::Await(future) if all_uses[index][future.0] == 1
                    && fresh[future.0].is_some_and(|definition| once_per_definition(function, definition, block_index)) => Some(future),
                _ => None,
            }).collect::<std::collections::HashSet<_>>()
        }).collect();
        for (block_index, block) in function.blocks.iter_mut().enumerate() {
            let mut insts = Vec::with_capacity(block.insts.len());
            let mut pending = std::mem::take(&mut block.insts).into_iter().peekable();
            while let Some(mut inst) = pending.next() {
                let extraction = if let InstKind::Await(future) = inst.kind
                    && eligible[block_index].contains(&future)
                {
                    ctx.await_regions.get(index).and_then(|sites| sites.get(&future)).copied().map(|(region, bound)| {
                        inst.kind = InstKind::AwaitTake(future);
                        (region, bound)
                    })
                } else { None };
                let result = inst.result;
                let span = inst.span;
                insts.push(inst);
                if let (Some((region, bound)), Some(value)) = (extraction, result) {
                    let ty = &function.values[value.0];
                    if !ctx.is_plain(ty) {
                        // Bound results never gain fresh call admission: adopt
                        // them immediately at the binding's cleanup boundary.
                        let kind = if bound { InstKind::RegionAdopt { region, value } }
                            else { InstKind::RegionTrack { region, value } };
                        // The binding's own lowering already adopts it right after (#999): one adopt, not two.
                        let adopted_next = bound && pending.peek().is_some_and(|next| {
                            matches!(next.kind, InstKind::RegionAdopt { region: r, value: v } if r == region && v == value)
                        });
                        if !adopted_next {
                            insts.push(Inst { result: None, kind, span });
                        }
                        if let Some(Some(types)) = ctx.settle_types.get_mut(index)
                            && !types.contains(ty) { types.push(ty.clone()); }
                    }
                }
            }
            block.insts = insts;
        }
    }
}

/// #932 (owner, 2026-11-21): a temporary Future taken at once by `await.take` belongs to the awaiting frame while it
/// is awaited, as an awaited call's state does in Rust, Kotlin or Swift: its `region.track` leaves the hot path for
/// the await's unwind edge, where a landing block admits it to the region before the handler closes it. Delivered,
/// it is consumed (`take_outcome`) and never reaches a region; not delivered (the callee failed, the awaiter was
/// cancelled before, during or after the callee ran), the existing close releases its inputs, failure cache or
/// running body exactly as before.
///
/// Qualifies: `call %f = F(...)`, then only `region.track R, %f`, then `await.take %f`, in one block with an unwind
/// edge, where F is async. A runtime driver keeps fresh inputs past its settlement (#587, #845: a collector's
/// items, a send's message), so for a driver the track follows the take on the normal path: the scope still closes
/// those inputs, and a Future with nothing left to close is not kept (`region_add`).
pub(super) fn own_awaited_temporaries(functions: &mut [Function]) {
    let drivers: Vec<bool> = functions.iter().map(|function| function.attrs.iter().any(|attr| {
        matches!(attr, crate::ir::Attr::Extern(_) | crate::ir::Attr::Intrinsic { .. })
    })).collect();
    let async_calls: Vec<bool> = functions.iter().map(Function::is_async).collect();
    for function in functions.iter_mut() {
        let mut block = 0;
        while block < function.blocks.len() {
            if let Some((at, driver)) = awaited_temporary(function, block, &async_calls, &drivers) {
                split_awaited_temporary(function, block, at, driver);
            }
            block += 1;
        }
    }
}

/// The index of the call of a qualifying `call; region.track; await.take` triple in `block`, and whether the
/// callee is a runtime driver.
fn awaited_temporary(function: &Function, block: usize, async_calls: &[bool], drivers: &[bool]) -> Option<(usize, bool)> {
    let body = &function.blocks[block];
    body.unwind.as_ref()?;
    body.insts.windows(3).enumerate().find_map(|(index, triple)| {
        let (Some(future), InstKind::Call { func, .. }) = (triple[0].result, &triple[0].kind) else { return None };
        (async_calls[func.0]
            && matches!(triple[1].kind, InstKind::RegionTrack { value, .. } if value == future)
            && matches!(triple[2].kind, InstKind::AwaitTake(input) if input == future))
            .then_some((index, drivers[func.0]))
    })
}

/// `[.. call %f; region.track R, %f; await.take %f; rest]` (unwind U) becomes `[.. call %f] -> [await.take %f]`
/// (unwind L) `-> [rest]` (unwind U), with `L(failure): region.track R, %f; br U(args.., failure)`; for a driver
/// `rest` starts with `region.track R, %f`.
fn split_awaited_temporary(function: &mut Function, block: usize, at: usize, driver: bool) {
    use crate::ir::{Block, BlockId, Target, Terminator};
    let InstKind::RegionTrack { region, value: future } = function.blocks[block].insts[at + 1].kind else {
        unreachable!("checked by awaited_temporary")
    };
    let span = function.blocks[block].insts[at + 1].span;
    let unwind = function.blocks[block].unwind.clone().expect("checked by awaited_temporary");
    let mut rest = function.blocks[block].insts.split_off(at + 1);
    let track = rest.remove(0);
    let take = rest.remove(0);
    let term = std::mem::replace(&mut function.blocks[block].term, Terminator { kind: TermKind::Br(Target::new(BlockId(0))), span });
    let awaiting = BlockId(function.blocks.len());
    let tail = BlockId(awaiting.0 + 1);
    let landing = BlockId(awaiting.0 + 2);
    function.blocks[block].term = Terminator { kind: TermKind::Br(Target::new(awaiting)), span };
    function.blocks.push(Block {
        params: Vec::new(),
        unwind: Some(Target::new(landing)),
        insts: vec![take],
        term: Terminator { kind: TermKind::Br(Target::new(tail)), span },
    });
    if driver {
        rest.insert(0, track.clone());
    }
    function.blocks.push(Block { params: Vec::new(), unwind: Some(unwind.clone()), insts: rest, term });
    let failure = crate::ir::ValueId(function.values.len());
    function.values.push(Type::Failure);
    let mut args = unwind.args.clone();
    args.push(failure);
    function.blocks.push(Block {
        params: vec![failure],
        unwind: None,
        insts: vec![Inst { result: None, kind: InstKind::RegionTrack { region, value: future }, span: track.span }],
        term: Terminator { kind: TermKind::Br(Target { block: unwind.block, args }), span },
    });
}
