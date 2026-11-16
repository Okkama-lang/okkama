//! Async ABI D25.2, revision of 2026-11-16 (#771, #758), ruling "How a stored Future leaves".
//!
//! A Future created in a function whose selection enters a call's packet (a constructor's staging or an
//! `array.new`) leaves its owner state with that packet: the `scope.split` before the call names the Future, and its
//! duty becomes a service entry of the packet (coro absorbs its token there). From then on the entry moves with its
//! packet only, through the exit selection of the value that holds it, so own emits no `cache.rehome` for it:
//! `rehome_caches` marks every escaping duty, and this pass drops the rehomes that a store precedes on every path.
//! A rehome that a store precedes on some paths only has no single form and is refused.
use super::*;
use crate::coro::graph;
use std::collections::{BTreeMap, BTreeSet};

/// The rehomes of Futures created here that a store precedes on every path are removed: their state and reserve
/// results read the operands they would have continued. Returns how many were removed.
pub(super) fn drop_stored_rehomes(module: &Module, body: &mut Body) -> Result<usize, OwnError> {
    let mut created = BTreeMap::new();
    for (inst, instruction) in body.insts.iter() {
        if let Operation::Async(AsyncOp::FutureNew { owner, .. }) = instruction.op
            && let Some(&future) = instruction.results.first()
        {
            created.insert(future, (owner, inst));
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
            let Some(future) = origin(body, value, &predecessors)? else { continue };
            if created.get(&future).is_some_and(|(scope, _)| *scope == owner) {
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
            if let Some(future) = origin(body, future, &predecessors)?
                && created.get(&future).is_some_and(|(scope, _)| *scope == owner)
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
        let definition = created[&future].1;
        match held_before(body, &predecessors, definition, &stores[&future], &kills, inst)? {
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

/// The instruction result `value` stands for through block parameters, if every edge passes the same one.
fn origin(body: &Body, value: ValueId, predecessors: &[Vec<BlockId>]) -> Result<Option<ValueId>, OwnError> {
    let mut pending = vec![value];
    let mut seen = BTreeSet::new();
    let mut found = None;
    while let Some(value) = pending.pop() {
        if !seen.insert(value) {
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

/// Whether the duty created by `definition` is still in its owner state just before `at`, on every path and on
/// some path. A store kills it at the start of its split's normal block; an instruction of `kills` kills it there.
fn held_before(body: &Body, predecessors: &[Vec<BlockId>], definition: InstId, stores: &BTreeSet<BlockId>,
    kills: &BTreeSet<InstId>, at: InstId) -> Result<(bool, bool), OwnError>
{
    let blocks: Vec<_> = body.blocks.iter().map(|(id, block)| (id, block.insts.clone())).collect();
    // A block's effect on the duty: `Some(true)` when its last event creates it, `Some(false)` when that event
    // takes it, `None` when the entry state passes through.
    let effect = |id: BlockId, insts: &[InstId], until: Option<InstId>| -> Option<bool> {
        let mut effect = stores.contains(&id).then_some(false);
        for inst in insts {
            if Some(*inst) == until {
                break;
            }
            if *inst == definition {
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
