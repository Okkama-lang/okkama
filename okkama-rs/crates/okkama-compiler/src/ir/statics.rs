//! Static resources (#909 step 2b): a resource created, used and ending in one scope of one function is destroyed
//! directly at that scope's exits, with no region operations.
//!
//! The escape question has one answer in the compiler: the ownership census (`ir::ownership`, #553). A creation
//! it places in the scope (`Owner::Scope`) never reaches a result, a field that leaves, a Future, a closure, a
//! worker, an atomic or a failure. On top of that this pass takes a value only when everything else is plain to
//! see:
//!
//! - it is the result of a call (a constructor or a factory) typed as an object with a synchronous destroy of
//!   its own, not builtin, whose fields are all plain (no other identity can hang off it); in an async body
//!   too, unless its region settles (an async destroy elsewhere in the program);
//! - its region operations are `region.track`/`region.adopt` into one region R, and R holds nothing else: no
//!   other member, no transfer, no keep that could reach a resource, R not passed around;
//! - every block reachable from its creation before R's exits is dominated by the creation (no drop flags yet).
//!
//! The rewrite: the creating block is split after the constructor call; `track`/`adopt` become `resource.pin`;
//! every `region.leave R` the value reaches gets a `resource.leave` before it; every edge from the value's code
//! into a handler that runs `region.unwind R` goes through a new block that runs `resource.unwind` first. R is
//! then empty and the `regions` pass removes it. The runtime destroys a pinned resource exactly once (a second
//! close of it does nothing), so a redirected edge that also passes a `resource.leave` cannot destroy twice.
//! Anything unsure stays dynamic.
use std::collections::HashSet;

use super::ownership::{self, Owner};
use super::regions::Plain;
use super::{Block, BlockId, Function, Inst, InstKind, Module, Target, TermKind, Terminator, Type, ValueId, cfg};

/// Make the module's provably local resources static; the number made static.
pub fn make_static(module: &mut Module) -> usize {
    if module.objects.iter().all(|object| object.destroy.is_none()) {
        return 0;
    }
    let report = ownership::analyze(module);
    let plain = Plain::of(module);
    let mut count = 0;
    for found in &report.functions {
        let candidates: Vec<ValueId> = found
            .creations
            .iter()
            .filter(|creation| creation.owner == Owner::Scope && creation.kind == "call.result")
            .map(|creation| ValueId(creation.value))
            .collect();
        for value in candidates {
            let others = match eligible(module, &plain, &module.functions[found.function], value) {
                Ok(others) => others,
                Err(reason) => {
                    okkama_devtrace::count!("lower", format!("statics: not static ({reason})"));
                    continue;
                }
            };
            split_after_definition(&mut module.functions[found.function], value);
            let Some(plan) = plan(&module.functions[found.function], value, &others) else {
                okkama_devtrace::count!("lower", "statics: not static (control flow)");
                // The split alone changes nothing: the value stays dynamic.
                continue;
            };
            apply(&mut module.functions[found.function], value, plan);
            count += 1;
        }
    }
    okkama_devtrace::count!("lower", "statics: resources made static", count as u64);
    count
}

/// Where the value is defined: block and instruction index.
fn definition(function: &Function, value: ValueId) -> Option<(usize, usize)> {
    function.blocks.iter().enumerate().find_map(|(block, body)| {
        body.insts.iter().position(|inst| inst.result == Some(value)).map(|index| (block, index))
    })
}

/// The region a value is adopted into (its binding owns it), when that is its only region use. A value only tracked
/// is a temporary a callee may still adopt (a fresh argument is owned by the action it is passed to): not static.
fn region_of(function: &Function, value: ValueId) -> Option<ValueId> {
    let adopted = function.blocks.iter().flat_map(|block| &block.insts)
        .any(|inst| matches!(&inst.kind, InstKind::RegionAdopt { value: v, .. } if *v == value));
    if !adopted {
        return None;
    }
    let mut region = None;
    for inst in function.blocks.iter().flat_map(|block| &block.insts) {
        match &inst.kind {
            InstKind::RegionTrack { region: r, value: v } | InstKind::RegionAdopt { region: r, value: v } if *v == value => {
                if region.is_some_and(|known| known != *r) {
                    return None;
                }
                region = Some(*r);
            }
            InstKind::RegionTransfer { value: v, .. } if *v == value => return None,
            _ => {}
        }
    }
    region
}

/// The type and region conditions (no control flow yet).
fn eligible(module: &Module, plain: &Plain, function: &Function, value: ValueId) -> Result<Vec<ValueId>, &'static str> {
    let Some((block, index)) = definition(function, value) else { return Err("no definition") };
    if !matches!(function.blocks[block].insts[index].kind, InstKind::Call { .. }) {
        return Err("not a call");
    }
    // A constructor, or any call the census calls fresh (a factory): `resource.pin` and `resource.leave` act only
    // on a free identity, so a result some other owner keeps is left alone.
    let Type::Object(object) = function.values[value.0] else { return Err("not an object type") };
    let declaration = &module.objects[object.0];
    let Some(destroy) = declaration.destroy else { return Err("no destroy of its own") };
    if declaration.builtin || module.functions[destroy.0].is_async() {
        return Err("builtin or async destroy");
    }
    if !declaration.fields.iter().all(|field| plain.ty(&field.ty)) {
        return Err("a field may hold a resource");
    }
    let Some(region) = region_of(function, value) else { return Err("no single region") };
    // R closes without keeping a resource and is used by nothing but its own operations. Its other members, if
    // any, must be newer than the value (checked with the control flow).
    let mut others = Vec::new();
    for block in &function.blocks {
        for inst in &block.insts {
            match &inst.kind {
                InstKind::RegionEnter => {}
                InstKind::RegionTrack { region: r, value: v } | InstKind::RegionAdopt { region: r, value: v } => {
                    if *r == region && *v != value && !others.contains(v) {
                        others.push(*v);
                    }
                }
                InstKind::RegionTransfer { from, to, .. } => {
                    if *from == region || *to == region {
                        return Err("region transfers");
                    }
                }
                InstKind::RegionLeave { region: r, keep } => {
                    if *r == region && keep.is_some_and(|keep| !plain.ty(&function.values[keep.0])) {
                        return Err("region keeps a value");
                    }
                }
                InstKind::RegionUnwind { .. } => {}
                InstKind::RegionSettle { region: r, .. } | InstKind::RegionSettleUnwind { region: r, .. } => {
                    if *r == region {
                        return Err("region settles");
                    }
                }
                other => {
                    if other.operands().contains(&region) {
                        return Err("region used");
                    }
                }
            }
        }
        let targets = block.term.kind.targets().into_iter().chain(block.unwind.as_ref());
        if targets.flat_map(|target| &target.args).any(|arg| *arg == region)
            || block.term.kind.operands().contains(&region)
        {
            return Err("region passed");
        }
    }
    Ok(others)
}

/// Split the creating block right after the creation, so the creation's own failure keeps the old handler.
fn split_after_definition(function: &mut Function, value: ValueId) {
    let Some((block, index)) = definition(function, value) else { return };
    let rest = function.blocks[block].insts.split_off(index + 1);
    let span = function.blocks[block].term.span;
    let next = BlockId(function.blocks.len());
    let term = std::mem::replace(
        &mut function.blocks[block].term,
        Terminator { kind: TermKind::Br(Target::new(next)), span },
    );
    let unwind = function.blocks[block].unwind.clone();
    function.blocks.push(Block { params: Vec::new(), unwind, insts: rest, term });
}

struct Plan {
    region: ValueId,
    /// R keeps newer members: they close first, then the value.
    shared: bool,
    /// Blocks whose `region.leave R` the value reaches.
    leaves: Vec<usize>,
    /// Blocks whose unwind edge enters a handler of R.
    unwinds: Vec<usize>,
    /// Blocks whose terminator branches into a handler of R.
    branches: Vec<usize>,
}

fn has(block: &Block, region: ValueId, leave: bool) -> bool {
    block.insts.iter().any(|inst| match &inst.kind {
        InstKind::RegionLeave { region: r, .. } => leave && *r == region,
        InstKind::RegionUnwind { region: r, .. } => !leave && *r == region,
        _ => false,
    })
}

/// The control-flow conditions, after the split: the blocks to rewrite, or `None`.
fn plan(function: &Function, value: ValueId, others: &[ValueId]) -> Option<Plan> {
    let region = region_of(function, value)?;
    let (start, _) = definition(function, value)?;
    let edges: Vec<Vec<usize>> = (0..function.blocks.len())
        .map(|block| cfg::all_successors(function, BlockId(block)).into_iter().map(|target| target.0).collect())
        .collect();
    let (idom, _) = cfg::dominators(&edges);
    let dominates = |by: usize, mut block: usize| loop {
        if block == by {
            return true;
        }
        match idom[block] {
            Some(parent) if parent != block => block = parent,
            _ => return false,
        }
    };
    let handler = |block: usize| has(&function.blocks[block], region, false);
    // A handler closes R first: whatever it closed before would have to die before the value.
    let closes_first = |block: usize| matches!(function.blocks[block].insts.first().map(|inst| &inst.kind),
        Some(InstKind::RegionUnwind { region: r, .. }) if *r == region);
    // The creating block's own successor is the split-off rest; the walk starts there.
    let TermKind::Br(first) = &function.blocks[start].term.kind else { return None };
    let shared = !others.is_empty();
    // Every other member is made after the value (so it dies first): defined by an instruction the value's
    // creation dominates.
    for &other in others {
        let (block, _) = definition(function, other)?;
        if block == start || !dominates(start, block) {
            return None;
        }
    }
    // With other members, a handler is copied with the value's close after R's: only the simple shape.
    let simple = |block: usize| {
        let body = &function.blocks[block];
        let Some(Inst { result: Some(closed), kind: InstKind::RegionUnwind { region: r, .. }, .. }) = body.insts.first()
        else {
            return false;
        };
        *r == region && body.insts.len() == 1 && match &body.term.kind {
            TermKind::Raise(raised) => raised == closed,
            TermKind::Br(target) => target.args.last() == Some(closed),
            _ => false,
        }
    };
    let closes_first = |block: usize| closes_first(block) && (!shared || simple(block));
    let mut plan = Plan { region, shared, leaves: Vec::new(), unwinds: Vec::new(), branches: Vec::new() };
    let mut seen = HashSet::new();
    let mut stack = vec![first.block.0];
    while let Some(block) = stack.pop() {
        if !seen.insert(block) {
            continue;
        }
        if !dominates(start, block) || handler(block) {
            return None;
        }
        let body = &function.blocks[block];
        if has(body, region, true) {
            // Nothing before `region.leave R` may fail into a handler that does not close R.
            let first_leave = body.insts.iter().position(|inst| matches!(&inst.kind,
                InstKind::RegionLeave { region: r, .. } if *r == region)).expect("leave");
            if first_leave > 0 && (shared || !body.unwind.as_ref().is_some_and(|unwind| handler(unwind.block.0))) {
                return None;
            }
            // The value is dead after `resource.leave`: what fails later in the block is not its concern.
            if first_leave > 0 {
                if !closes_first(body.unwind.as_ref().expect("checked").block.0) {
                    return None;
                }
                plan.unwinds.push(block);
            }
            plan.leaves.push(block);
            continue;
        }
        if let Some(unwind) = &body.unwind {
            if handler(unwind.block.0) {
                if !closes_first(unwind.block.0) {
                    return None;
                }
                plan.unwinds.push(block);
            } else {
                stack.push(unwind.block.0);
            }
        }
        let targets = body.term.kind.targets();
        if targets.iter().any(|target| handler(target.block.0)) {
            if targets.iter().any(|target| handler(target.block.0) && !closes_first(target.block.0)) {
                return None;
            }
            plan.branches.push(block);
        }
        stack.extend(targets.iter().filter(|target| !handler(target.block.0)).map(|target| target.block.0));
    }
    Some(plan)
}

fn apply(function: &mut Function, value: ValueId, plan: Plan) {
    let region = plan.region;
    // `track`/`adopt` into R become one pin where the first of them was.
    let mut pinned = false;
    for block in &mut function.blocks {
        let mut kept = Vec::with_capacity(block.insts.len());
        for inst in block.insts.drain(..) {
            match &inst.kind {
                InstKind::RegionTrack { region: r, value: v } | InstKind::RegionAdopt { region: r, value: v }
                    if *r == region && *v == value =>
                {
                    if !pinned {
                        pinned = true;
                        kept.push(Inst { result: None, kind: InstKind::ResourcePin { value }, span: inst.span });
                    }
                }
                _ => kept.push(inst),
            }
        }
        block.insts = kept;
    }
    for &block in &plan.leaves {
        if plan.shared {
            leave_after(function, block, region, value);
            continue;
        }
        let insts = &mut function.blocks[block].insts;
        let at = insts.iter().position(|inst| matches!(&inst.kind,
            InstKind::RegionLeave { region: r, .. } if *r == region)).expect("leave");
        let span = insts[at].span;
        insts.insert(at, Inst { result: None, kind: InstKind::ResourceLeave { value }, span });
    }
    // One forwarding block per handler: close the value, then the handler as before.
    let mut forwarders: Vec<(usize, usize)> = Vec::new();
    let mut forwarder = |function: &mut Function, handler: usize| -> usize {
        if let Some(&(_, made)) = forwarders.iter().find(|(of, _)| *of == handler) {
            return made;
        }
        if plan.shared {
            let made = handler_after(function, handler, value);
            forwarders.push((handler, made));
            return made;
        }
        let params: Vec<ValueId> = function.blocks[handler].params.iter().map(|param| {
            let ty = function.values[param.0].clone();
            function.values.push(ty);
            ValueId(function.values.len() - 1)
        }).collect();
        let (failure, rest) = params.split_last().expect("a handler takes its failure");
        function.values.push(Type::Failure);
        let closed = ValueId(function.values.len() - 1);
        let span = function.blocks[handler].term.span;
        let mut args = rest.to_vec();
        args.push(closed);
        function.blocks.push(Block {
            params: params.clone(),
            unwind: None,
            insts: vec![Inst { result: Some(closed), kind: InstKind::ResourceUnwind { value, failure: *failure }, span }],
            term: Terminator { kind: TermKind::Br(Target { block: BlockId(handler), args }), span },
        });
        let made = function.blocks.len() - 1;
        forwarders.push((handler, made));
        made
    };
    for &block in &plan.unwinds {
        let handler = function.blocks[block].unwind.as_ref().expect("unwind").block.0;
        let made = forwarder(function, handler);
        function.blocks[block].unwind.as_mut().expect("unwind").block = BlockId(made);
    }
    for &block in &plan.branches {
        let handlers: Vec<usize> = function.blocks[block].term.kind.targets().iter()
            .map(|target| target.block.0)
            .filter(|&target| has(&function.blocks[target], region, false))
            .collect();
        for handler in handlers {
            let made = forwarder(function, handler);
            for target in function.blocks[block].term.kind.targets_mut() {
                if target.block.0 == handler {
                    target.block = BlockId(made);
                }
            }
        }
    }
}

fn fresh(function: &mut Function, ty: Type) -> ValueId {
    function.values.push(ty);
    ValueId(function.values.len() - 1)
}

/// With newer members in R: the value closes after `region.leave R`, in a block of its own. A failing leave goes
/// through a block that closes the value (its failure secondary), then wherever the leave's failure went.
fn leave_after(function: &mut Function, block: usize, region: ValueId, value: ValueId) {
    let at = function.blocks[block].insts.iter().position(|inst| matches!(&inst.kind,
        InstKind::RegionLeave { region: r, .. } if *r == region)).expect("leave");
    let span = function.blocks[block].insts[at].span;
    let mut rest = function.blocks[block].insts.split_off(at + 1);
    rest.insert(0, Inst { result: None, kind: InstKind::ResourceLeave { value }, span });
    let outer = function.blocks[block].unwind.clone();
    let failure = fresh(function, Type::Failure);
    let closed = fresh(function, Type::Failure);
    let term = match &outer {
        Some(target) => {
            let mut args = target.args.clone();
            args.push(closed);
            TermKind::Br(Target { block: target.block, args })
        }
        None => TermKind::Raise(closed),
    };
    let close = BlockId(function.blocks.len());
    function.blocks.push(Block {
        params: vec![failure],
        unwind: None,
        insts: vec![Inst { result: Some(closed), kind: InstKind::ResourceUnwind { value, failure }, span }],
        term: Terminator { kind: term, span },
    });
    let after = BlockId(function.blocks.len());
    let term = std::mem::replace(
        &mut function.blocks[block].term,
        Terminator { kind: TermKind::Br(Target::new(after)), span },
    );
    function.blocks[block].unwind = Some(Target::new(close));
    function.blocks.push(Block { params: Vec::new(), unwind: outer, insts: rest, term });
}

/// With newer members in R: a copy of the simple handler `[region.unwind R; raise | br]` that closes the value
/// right after R, so the value dies after what R held.
fn handler_after(function: &mut Function, handler: usize, value: ValueId) -> usize {
    let source = function.blocks[handler].clone();
    let params: Vec<ValueId> = source.params.iter().map(|param| {
        let ty = function.values[param.0].clone();
        fresh(function, ty)
    }).collect();
    let rename = |v: ValueId| source.params.iter().position(|p| *p == v).map_or(v, |i| params[i]);
    let Inst { result: Some(old), kind: InstKind::RegionUnwind { region, failure }, span } = source.insts[0].clone()
    else {
        unreachable!("checked simple");
    };
    let closed = fresh(function, Type::Failure);
    let both = fresh(function, Type::Failure);
    let swap = |v: ValueId| if v == old { both } else { rename(v) };
    let term = match &source.term.kind {
        TermKind::Raise(_) => TermKind::Raise(both),
        TermKind::Br(target) => TermKind::Br(Target { block: target.block, args: target.args.iter().map(|&a| swap(a)).collect() }),
        _ => unreachable!("checked simple"),
    };
    let unwind = source.unwind.as_ref().map(|target| Target { block: target.block, args: target.args.iter().map(|&a| rename(a)).collect() });
    let insts = vec![
        Inst { result: Some(closed), kind: InstKind::RegionUnwind { region, failure: rename(failure) }, span },
        Inst { result: Some(both), kind: InstKind::ResourceUnwind { value, failure: closed }, span },
    ];
    function.blocks.push(Block {
        params: params.clone(),
        unwind,
        insts,
        term: Terminator { kind: term, span: source.term.span },
    });
    function.blocks.len() - 1
}

