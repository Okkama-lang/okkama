//! Async ABI D25.2, revision of 2026-11-16 (#771, #758), ruling "How a stored Future leaves".
//!
//! A Future created in a function whose selection enters a call's packet (a constructor's staging or an
//! `array.new`) leaves its owner state with that packet: the `scope.split` before the call names the Future, and its
//! duty becomes a service entry of the packet (coro absorbs its token there). From then on the entry moves with its
//! packet only, through the exit selection of the value that holds it, so own emits no `cache.rehome` for it:
//! `rehome_caches` marks every escaping duty, and this pass drops the rehomes that a store precedes on every path.
//! A rehome that a store precedes on some paths only has no single form and is refused.
//!
//! PR-B1 (#784): only a fresh temporary is stored. A Future that a binding holds (`fut.new … binding`) is aliased by
//! the holder and keeps its rehome; a split that names it is a driver defect.
use super::*;
use crate::coro::graph;
use std::collections::{BTreeMap, BTreeSet};

/// Where a Future's duty enters its owner state: a `fut.new` here, or the start of the normal block of a call that
/// returned the Future (#880: its entry arrives with the call's result packet, D25.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Created(InstId),
    Returned(BlockId),
}

/// The rehomes of Futures created here, or returned to here by a call, that a store precedes on every path are
/// removed: their state and reserve results read the operands they would have continued. Returns how many were
/// removed.
pub(super) fn drop_stored_rehomes(module: &Module, body: &mut Body) -> Result<usize, OwnError> {
    let mut created = BTreeMap::new();
    for (inst, instruction) in body.insts.iter() {
        if let Operation::Async(AsyncOp::FutureNew { owner, binding, .. }) = instruction.op
            && let Some(&future) = instruction.results.first()
        {
            created.insert(future, (owner, Source::Created(inst), binding.is_some()));
        }
    }
    // #880 (Fable): a Future a call returned arrives as an admitted entry of the call's scope, and a store moves it
    // into the holder's packet as it moves a created one. A binding of it is a split and a bind (PR-B1), not a store.
    for (_, block) in body.blocks.iter() {
        let TermKind::InvokeOwn { op, normal, .. } = &block.term.kind else { continue };
        let Operation::Call(call) = op.as_ref() else { continue };
        let target = body.blocks.get(normal.block).map_err(arena)?;
        for (param, argument) in target.params.iter().zip(&normal.args) {
            let EdgeArg::Produced(result) = argument else { continue };
            let ty = body.values.get(*param).map_err(arena)?.ty;
            if super::futures::is_future(module, ty)
                && !super::futures::borrowed_result(module, call, usize::from(result.slot)) {
                created.insert(*param, (target.lexical_scope, Source::Returned(normal.block), false));
            }
        }
    }
    if created.is_empty() {
        return Ok(0);
    }
    let predecessors = graph::predecessors(body);
    // The normal blocks of the splits that store each Future: its duty leaves the state when the split succeeds.
    let mut stores: BTreeMap<ValueId, BTreeSet<BlockId>> = BTreeMap::new();
    for (_, block) in body.blocks.iter() {
        let TermKind::InvokeOwn { op, normal, .. } = &block.term.kind else { continue };
        let Operation::Ownership(OwnershipOp::ScopeSplit { state, selection }) = **op else { continue };
        let owner = scope_of(module, body, state)?;
        for value in named(body, selection)? {
            let Some(future) = origin(body, value, &predecessors, &created)? else { continue };
            if matches!(created.get(&future), Some((_, Source::Returned(_), _)))
                && binds(body, normal.block)? {
                continue;
            }
            if created.get(&future).is_some_and(|(scope, _, _)| *scope == owner) {
                // PR-B1 (#784): a store takes a fresh temporary only. Own never names a bound Future in a split, so
                // such a split is a defect of the driver, not a store.
                if created[&future].2 {
                    return Err(flow_error("a split names a bound Future (PR-B1): a store takes a fresh one only"));
                }
                if predecessors[normal.block.as_u32() as usize].len() != 1 {
                    return Err(flow_error("a split that stores a Future has a shared normal block"));
                }
                stores.entry(future).or_default().insert(normal.block);
            }
        }
    }
    let mut rehomes: Vec<(InstId, ValueId)> = Vec::new();
    for (_, block) in body.blocks.iter() {
        for inst in &block.insts {
            let Operation::Async(AsyncOp::CacheRehome { form: RehomeForm::Scope { state, future, .. }, .. }) =
                body.insts.get(*inst).map_err(arena)?.op
            else {
                continue;
            };
            let owner = scope_of(module, body, state)?;
            if let Some(future) = origin(body, future, &predecessors, &created)?
                && created.get(&future).is_some_and(|(scope, _, _)| *scope == owner)
                && stores.contains_key(&future)
            {
                rehomes.push((*inst, future));
            }
        }
    }
    let mut dropped = Vec::new();
    for &(inst, future) in &rehomes {
        let kills: BTreeSet<InstId> = rehomes.iter().filter(|(other, value)| *value == future && *other != inst)
            .map(|(other, _)| *other).collect();
        let source = created[&future].1;
        match held_before(body, &predecessors, source, &stores[&future], &kills, inst)? {
            (true, _) => {}
            (false, false) => dropped.push(inst),
            (false, true) => {
                return Err(super::futures::unsupported_async(
                    "a Future leaves its owner where it was stored on some paths only (D25.2)",
                ));
            }
        }
    }
    let mut replace = BTreeMap::new();
    for inst in &dropped {
        let instruction = body.insts.get(*inst).map_err(arena)?;
        let Operation::Async(AsyncOp::CacheRehome { form: RehomeForm::Scope { state, pending, .. }, .. }) =
            instruction.op
        else {
            return Err(flow_error("a dropped rehome is the own form"));
        };
        let [moved, reserve] = instruction.results[..] else {
            return Err(flow_error("the own form of cache.rehome gives the state and the reserve"));
        };
        replace.insert(moved, state);
        replace.insert(reserve, pending);
    }
    for (_, block) in body.blocks.iter_mut() {
        block.insts.retain(|inst| !dropped.contains(inst));
    }
    crate::visit::rewrite_uses(body, &mut Rename(replace));
    Ok(dropped.len())
}

struct Rename(BTreeMap<ValueId, ValueId>);

impl crate::visit::VisitorMut for Rename {
    fn value(&mut self, value: &mut ValueId) {
        // A dropped rehome's state may feed another one's: follow the chain to an operand that stays.
        while let Some(next) = self.0.get(value) {
            *value = *next;
        }
    }
}

fn scope_of(module: &Module, body: &Body, state: ValueId) -> Result<ScopeId, OwnError> {
    match module.types.get(body.values.get(state).map_err(arena)?.ty).map_err(arena)? {
        Type::Service(ServiceType::Scope(scope)) => Ok(*scope),
        _ => Err(flow_error("a split or a rehome takes a scope state")),
    }
}

/// The values a selection names directly, through unions and admissible restrictions.
fn named(body: &Body, selection: SelectionId) -> Result<Vec<ValueId>, OwnError> {
    let mut values = Vec::new();
    let mut pending = vec![selection];
    while let Some(selection) = pending.pop() {
        match body.selections.get(selection).map_err(arena)? {
            SelectionExpr::Union(items) => pending.extend(items.iter().rev()),
            SelectionExpr::Admissible(inner) => pending.push(*inner),
            SelectionExpr::Value(value) => values.push(*value),
            _ => {}
        }
    }
    Ok(values)
}

/// Whether a split's normal block binds what it selected: the split and bind of an admitted Future's binding (PR-B1).
fn binds(body: &Body, block: BlockId) -> Result<bool, OwnError> {
    Ok(body.blocks.get(block).map_err(arena)?.insts.iter().any(|inst| body.insts.get(*inst).is_ok_and(|instruction| {
        matches!(instruction.op, Operation::Ownership(OwnershipOp::Bind { .. } | OwnershipOp::Rebind { .. }))
    })))
}

/// The instruction result, or the returned Future of `roots`, that `value` stands for through block parameters, if
/// every edge passes the same one.
fn origin<T>(body: &Body, value: ValueId, predecessors: &[Vec<BlockId>], roots: &BTreeMap<ValueId, T>)
    -> Result<Option<ValueId>, OwnError>
{
    let mut pending = vec![value];
    let mut seen = BTreeSet::new();
    let mut found = None;
    while let Some(value) = pending.pop() {
        if !seen.insert(value) {
            continue;
        }
        if roots.contains_key(&value) {
            if found.is_some_and(|previous| previous != value) {
                return Ok(None);
            }
            found = Some(value);
            continue;
        }
        match body.values.get(value).map_err(arena)?.def {
            Definition::BlockParam { block, index } => {
                for &source in &predecessors[block.as_u32() as usize] {
                    let term = &body.blocks.get(source).map_err(arena)?.term.kind;
                    match graph::edge_argument(term, block, index as usize) {
                        Some(EdgeArg::Value(argument)) => pending.push(argument),
                        _ => return Ok(None),
                    }
                }
            }
            Definition::InstResult { .. } => {
                if found.is_some_and(|previous| previous != value) {
                    return Ok(None);
                }
                found = Some(value);
            }
        }
    }
    Ok(found)
}

/// Whether the duty that enters at `source` is still in its owner state just before `at`, on every path and on some
/// path. A returned Future's duty enters at the start of its call's normal block; a store kills it at the start of
/// its split's normal block; an instruction of `kills` kills it there.
fn held_before(body: &Body, predecessors: &[Vec<BlockId>], source: Source, stores: &BTreeSet<BlockId>,
    kills: &BTreeSet<InstId>, at: InstId) -> Result<(bool, bool), OwnError>
{
    let blocks: Vec<_> = body.blocks.iter().map(|(id, block)| (id, block.insts.clone())).collect();
    // A block's effect on the duty: `Some(true)` when its last event creates it, `Some(false)` when that event
    // takes it, `None` when the entry state passes through.
    let effect = |id: BlockId, insts: &[InstId], until: Option<InstId>| -> Option<bool> {
        let mut effect = if stores.contains(&id) {
            Some(false)
        } else {
            (source == Source::Returned(id)).then_some(true)
        };
        for inst in insts {
            if Some(*inst) == until {
                break;
            }
            if source == Source::Created(*inst) {
                effect = Some(true);
            } else if kills.contains(inst) {
                effect = Some(false);
            }
        }
        effect
    };
    let effects: Vec<Option<bool>> = blocks.iter().map(|(id, insts)| effect(*id, insts, None)).collect();
    let count = blocks.len();
    let entry = body.entry.as_u32() as usize;
    let mut must = vec![true; count];
    let mut may = vec![false; count];
    must[entry] = false;
    let mut changed = true;
    while changed {
        changed = false;
        for index in 0..count {
            let inputs = &predecessors[index];
            let all = index != entry && inputs.iter().all(|source| {
                let source = source.as_u32() as usize;
                effects[source].unwrap_or(must[source])
            });
            let any = inputs.iter().any(|source| {
                let source = source.as_u32() as usize;
                effects[source].unwrap_or(may[source])
            });
            if all != must[index] || any != may[index] {
                must[index] = all;
                may[index] = any;
                changed = true;
            }
        }
    }
    let (id, insts) = blocks.iter().find(|(_, insts)| insts.contains(&at))
        .ok_or_else(|| flow_error("a rehome outside every block"))?;
    let index = id.as_u32() as usize;
    Ok(match effect(*id, insts, Some(at)) {
        Some(held) => (held, held),
        None => (must[index], may[index]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWICE: &str = include_str!("../../tests/fixtures/async/own/d25-stored-twice.oir");
    const FIRST_STORE: &str = "    selection s0: selection = admissible(selection(%19))\n";
    const NAMES_C: &str = concat!("    selection s9: selection = union([selection(%19), selection(%22)])\n",
        "    selection s0: selection = admissible(s9)\n");

    /// `make` of d25-stored-twice after `edit`, and the result of `drop_stored_rehomes` on it.
    fn dropped(edit: impl Fn(String) -> String) -> (Result<usize, OwnError>, usize) {
        let module = crate::text::parse(&edit(TWICE.to_string())).unwrap();
        let (_, make) = module.functions.iter().find(|(_, function)| function.symbol == "app.Maker.make#0").unwrap();
        let mut body = make.body.clone().unwrap();
        let result = drop_stored_rehomes(&module, &mut body);
        let left = body.blocks.iter().flat_map(|(_, block)| block.insts.iter())
            .filter(|inst| matches!(body.insts.get(**inst).unwrap().op, Operation::Async(AsyncOp::CacheRehome { .. })))
            .count();
        (result, left)
    }

    /// PR-B1 (#784): a bound Future is aliased by its holders, so the rehome of `c` that the returned Holder needs
    /// stays.
    #[test]
    fn a_bound_future_keeps_its_rehome() {
        let (result, left) = dropped(|text| text);
        assert_eq!((result.unwrap(), left), (0, 1));
    }

    /// A store of a fresh Future (no `binding`) moves its duty into the holder's packet: the rehome a store precedes
    /// on every path is dropped (D25.2 revision, "How a stored Future leaves").
    #[test]
    fn a_stored_fresh_future_loses_its_rehome() {
        let (result, left) = dropped(|text| text.replacen(", owner S2, binding b1", ", owner S2", 1)
            .replacen(FIRST_STORE, NAMES_C, 1));
        assert_eq!((result.unwrap(), left), (1, 0));
    }

    /// `wrap` of a d880 fixture, and the result of `drop_stored_rehomes` on it.
    fn dropped_in_wrap(text: &str) -> (Result<usize, OwnError>, usize) {
        let module = crate::text::parse(text).unwrap();
        let (_, wrap) = module.functions.iter().find(|(_, function)| function.symbol == "app.Mk.wrap#0").unwrap();
        let mut body = wrap.body.clone().unwrap();
        let result = drop_stored_rehomes(&module, &mut body);
        let left = body.blocks.iter().flat_map(|(_, block)| block.insts.iter())
            .filter(|inst| matches!(body.insts.get(**inst).unwrap().op, Operation::Async(AsyncOp::CacheRehome { .. })))
            .count();
        (result, left)
    }

    /// #880 (Fable): a Future a call returned arrives as an admitted entry; a store moves it into the holder's packet
    /// as it moves a created one, so the rehome the store precedes is dropped (D25.2 revision).
    #[test]
    fn a_stored_returned_future_loses_its_rehome() {
        let (result, left) = dropped_in_wrap(include_str!("../../tests/fixtures/async/own/d880-stored-returned.oir"));
        assert_eq!((result.unwrap(), left), (1, 0));
    }

    /// #880: the binding of a returned Future is a split and a bind (PR-B1), not a store; the Box borrows the bound
    /// Future, which leaves by its rehome.
    #[test]
    fn a_bound_returned_future_keeps_its_rehome() {
        let (result, left) = dropped_in_wrap(include_str!("../../tests/fixtures/async/own/d880-bound-returned.oir"));
        assert_eq!((result.unwrap(), left), (0, 1));
    }

    /// PR-B1: own never names a bound Future in a split. A split that does is a defect of the driver, never a store:
    /// counting it as one would drop the rehome of `c` that the returned Holder needs.
    #[test]
    fn a_split_that_names_a_bound_future_is_an_internal_error() {
        let (result, left) = dropped(|text| text.replacen(FIRST_STORE, NAMES_C, 1));
        let Err(OwnError::Diagnostics { diagnostics, .. }) = result else { panic!("{result:?}") };
        assert!(diagnostics[0].message.contains("names a bound Future"), "{diagnostics:?}");
        assert_eq!(left, 1);
    }
}
