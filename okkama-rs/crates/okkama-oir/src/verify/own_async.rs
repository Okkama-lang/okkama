//! Own-profile rules of plain async (commit 5a, #601 V2), checked on the
//! structure of the IR. They hold before coro introduces frame and service
//! tokens, so they bind the own profile only. The failure rule of
//! `await.unwind` (K1) binds every profile with await unwind adapters.
use super::{EntityLocation as L, RuleId as R, State};
use crate::*;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn check(module: &Module, function: FuncId, body: &Body, state: &mut State<'_>) {
    if matches!(module.profile, Profile::Own | Profile::Coro | Profile::Low) {
        superseded_await_failures(module, function, body, state);
    }
    if module.profile != Profile::Own {
        return;
    }
    loose_cache_states(module, function, body, state);
    await_unwind_consumers(module, function, body, state);
    cache_schedules(module, function, body, state);
}

/// B9.1, elaboration D12: own returns no cache-state value. The owner state
/// holds the duty `ServiceOf{future, Cache}`; a cache-state token would be a
/// second holder of that duty.
fn loose_cache_states(module: &Module, function: FuncId, body: &Body, state: &mut State<'_>) {
    for (value, data) in body.values.iter() {
        if !state.step(1) {
            return;
        }
        if module.types.get(data.ty) == Ok(&Type::Service(ServiceType::CacheState)) {
            state.at(L::Value { function, value }, None);
            state.error(R::B91, "own holds a cache duty in its owner state, never as a cache-state value (D12)");
        }
    }
}

/// B9.1, elaboration §13.3 and async ABI C-3: every unwind or cancel edge of
/// an await flows its task and receipt into exactly one `await.unwind`, which
/// consumes both before any other use. No failure block may drop either one,
/// or settle it by another consumer such as a service schedule.
fn await_unwind_consumers(module: &Module, function: FuncId, body: &Body, state: &mut State<'_>) {
    let registry = ContractRegistry::new(module);
    // (await block, task, receipt, landing block) for every await with an unwind edge.
    let mut awaits = vec![];
    for (block_id, block) in body.blocks.iter() {
        if !state.step(1) {
            return;
        }
        let TermKind::InvokeOwn { op, unwind, .. } = &block.term.kind else {
            continue;
        };
        if !matches!(**op, Operation::Async(AsyncOp::Await { .. } | AsyncOp::AwaitShielded { .. }
            | AsyncOp::AwaitTake { .. })) {
            continue;
        }
        let Some(schema) = registry.lookup(op).ok().and_then(|contract| contract.unwind) else {
            continue;
        };
        let Ok(target) = body.blocks.get(unwind.block) else {
            continue;
        };
        let parameter = |name: SlotName| {
            let slot = schema.results.iter().position(|slot| slot.name == name)?;
            let index = unwind.args.iter().position(|argument| matches!(argument,
                EdgeArg::Produced(EdgeResult { outcome: Outcome::Unwind, slot: produced })
                    if usize::from(*produced) == slot))?;
            target.params.get(index).copied()
        };
        awaits.push((block_id, parameter(SlotName::Task), parameter(SlotName::Receipt), unwind.block));
    }
    if awaits.is_empty() {
        return;
    }
    // Every reference to a task or receipt, through memoized expression leaves.
    let watched: Vec<_> = awaits.iter().flat_map(|(_, task, receipt, _)| [*task, *receipt]).flatten().collect();
    let mut uses: BTreeMap<ValueId, Vec<Option<InstId>>> = BTreeMap::new();
    let mut record = |values: Vec<ValueId>, site: Option<InstId>| {
        for value in values.into_iter().filter(|value| watched.contains(value)) {
            uses.entry(value).or_default().push(site);
        }
    };
    for (_, block) in body.blocks.iter() {
        for &inst in &block.insts {
            let Ok(instruction) = body.insts.get(inst) else {
                continue;
            };
            record(super::dominance::leaves(&instruction.op, body, state), Some(inst));
            record(super::dominance::leaves(&instruction.facts, body, state), None);
        }
        record(super::dominance::leaves(&block.term, body, state), None);
        record(super::dominance::leaves(&block.unwind, body, state), None);
    }
    record(super::dominance::leaves(&body.service_schedules, body, state), None);
    for (block_id, task, receipt, landing) in awaits {
        let single = |value: Option<ValueId>| match value.and_then(|value| uses.get(&value)).map(Vec::as_slice) {
            Some([Some(inst)]) => Some(*inst),
            _ => None,
        };
        let consumed = match (single(task), single(receipt)) {
            (Some(inst), Some(other)) if inst == other => body.blocks.get(landing)
                .is_ok_and(|landing| landing.insts.contains(&inst))
                && body.insts.get(inst).is_ok_and(|inst| matches!(inst.op,
                    Operation::Async(AsyncOp::AwaitUnwind { task: t, receipt: r, .. })
                        if Some(t) == task && Some(r) == receipt)),
            _ => false,
        };
        if !consumed {
            let loc = body.blocks.get(block_id).ok().and_then(|block| block.term.loc);
            state.at(L::Terminator { function, block: block_id }, loc);
            state.error(R::B91, "await unwind must consume its task and receipt in one await.unwind first (§13.3)");
        }
    }
}

/// B10.6, elaboration D16: each scope that owns cache duties has a Services
/// schedule covering each duty once. In own a duty is `ServiceOf{f, Cache}`,
/// assigned by `fut.new ... owner S` and named in the schedule by its Future.
fn cache_schedules(module: &Module, function: FuncId, body: &Body, state: &mut State<'_>) {
    let mut duties = BTreeMap::new();
    for (_, block) in body.blocks.iter() {
        for &inst in &block.insts {
            if !state.step(1) {
                return;
            }
            if let Ok(Instruction { op: Operation::Async(AsyncOp::FutureNew { owner, .. }), results, .. })
                = body.insts.get(inst)
                && let Some(future) = results.first() {
                duties.insert(*future, *owner);
            }
        }
    }
    let mut covered: BTreeMap<ValueId, usize> = BTreeMap::new();
    for schedule in &body.service_schedules {
        let services = module.protocols.get(schedule.protocol)
            .is_ok_and(|protocol| protocol.kind == ProtocolKind::Services);
        for duty in &schedule.duties {
            if !state.step(1) {
                return;
            }
            let ScheduleDuty::Value(value) = duty else {
                continue;
            };
            if services && duties.get(value) == Some(&schedule.owner) {
                *covered.entry(*value).or_default() += 1;
            } else {
                state.at(L::Value { function, value: *value }, None);
                state.error(R::B106, "own schedule entry names no cache duty of its owner scope (D16)");
            }
        }
    }
    for future in duties.keys() {
        if covered.get(future) != Some(&1) {
            state.at(L::Value { function, value: *future }, None);
            state.error(R::B106, "a cache duty needs exactly one entry in a Services schedule of its owner (D16)");
        }
    }
}

/// The SSA edges of a block: each target with the values its arguments forward
/// (`None` for an edge result).
fn forwarding_edges(block: &Block) -> Vec<(BlockId, Vec<Option<ValueId>>)> {
    let target = |target: &Target| (target.block, target.args.iter().copied().map(Some).collect());
    let invoke = |edge: &InvokeEdge| (edge.block, edge.args.iter().map(|argument| match argument {
        EdgeArg::Value(value) => Some(*value),
        EdgeArg::Produced(_) => None,
    }).collect());
    let mut edges: Vec<_> = block.unwind.iter().map(|unwind| target(&unwind.target)).collect();
    match &block.term.kind {
        TermKind::Br(next) => edges.push(target(next)),
        TermKind::Cond { yes, no, .. } => edges.extend([target(yes), target(no)]),
        TermKind::Switch { cases, default, .. } => {
            edges.extend(cases.iter().map(|case| target(&case.target)).chain([target(default)]));
        }
        TermKind::InvokeOwn { normal, unwind, .. } => edges.extend([invoke(normal), invoke(unwind)]),
        TermKind::InvokeDefer { continue_edge, replacements, unwind, .. } => {
            edges.push(invoke(continue_edge));
            edges.extend(replacements.iter().map(|replacement| invoke(&replacement.edge)));
            edges.extend(unwind.iter().map(invoke));
        }
        TermKind::FailureDispatch { none, some, .. } => edges.extend([target(none), invoke(some)]),
        TermKind::CleanupNext { .. } | TermKind::ExitDispatch { .. } => {
            edges.extend(dispatch_edges(&block.term.kind).into_iter().map(|(_, edge)| invoke(edge)));
        }
        TermKind::Catch { handler, next, .. } => edges.extend([invoke(handler), target(next)]),
        TermKind::Lookup { found, missing, .. } => edges.extend([invoke(found), target(missing)]),
        TermKind::RetDefer { .. } | TermKind::Ret { .. } | TermKind::Throw(_) | TermKind::Raise(_)
        | TermKind::Panic(_) | TermKind::Trap { .. } | TermKind::Unreachable => {}
    }
    edges
}

/// B9.1, elaboration §13.3 and async ABI C-3 (K1): `await.unwind` observes the
/// failure of its await unwind edge and returns the failure that the lexical
/// unwind continues with, after the structural await protocol has finished and
/// has accumulated its own failures into it. The failure of the edge reaches only
/// that adapter: edges may forward it there, but no other operation reads it,
/// before or after the adapter. Nothing suspends between the landing and the
/// adapter, so no frame slot holds it either.
fn superseded_await_failures(module: &Module, function: FuncId, body: &Body, state: &mut State<'_>) {
    let registry = ContractRegistry::new(module);
    let mut watched = BTreeSet::new();
    for (_, block) in body.blocks.iter() {
        if !state.step(1) {
            return;
        }
        let TermKind::InvokeOwn { op, unwind, .. } = &block.term.kind else {
            continue;
        };
        if !matches!(**op, Operation::Async(AsyncOp::Await { .. } | AsyncOp::AwaitShielded { .. }
            | AsyncOp::AwaitTake { .. })) {
            continue;
        }
        let Some(schema) = registry.lookup(op).ok().and_then(|contract| contract.unwind) else {
            continue;
        };
        // High awaits have no adapter: their unwind edge carries the failure alone.
        if !schema.results.iter().any(|slot| slot.name == SlotName::Task) {
            continue;
        }
        let Some(slot) = schema.results.iter().position(|slot| slot.name == SlotName::Failure) else {
            continue;
        };
        let index = unwind.args.iter().position(|argument| matches!(argument,
            EdgeArg::Produced(EdgeResult { outcome: Outcome::Unwind, slot: produced })
                if usize::from(*produced) == slot));
        if let Some(parameter) = index.and_then(|index| body.blocks.get(unwind.block).ok()?.params.get(index)) {
            watched.insert(*parameter);
        }
    }
    if watched.is_empty() {
        return;
    }
    // The block parameters that receive a watched failure through an edge are watched as well.
    loop {
        let mut grown = false;
        for (_, block) in body.blocks.iter() {
            if !state.step(1) {
                return;
            }
            for (target, arguments) in forwarding_edges(block) {
                let Ok(target) = body.blocks.get(target) else {
                    continue;
                };
                for (argument, parameter) in arguments.iter().zip(&target.params) {
                    if argument.is_some_and(|argument| watched.contains(&argument)) {
                        grown |= watched.insert(*parameter);
                    }
                }
            }
        }
        if !grown {
            break;
        }
    }
    for (block_id, block) in body.blocks.iter() {
        for &inst in &block.insts {
            let Ok(instruction) = body.insts.get(inst) else {
                continue;
            };
            let allowed = match &instruction.op {
                Operation::Async(AsyncOp::AwaitUnwind { failure, .. }) => Some(*failure),
                _ => None,
            };
            let used = super::dominance::leaves(&instruction.op, body, state);
            if used.iter().any(|value| watched.contains(value) && Some(*value) != allowed) {
                state.at(L::Instruction { function, instruction: inst }, instruction.loc);
                state.error(R::B91, "an await unwind continues with the failure of its await.unwind, \
                    never the failure the adapter observed (§13.3)");
            }
        }
        let operands: Vec<ValueId> = match &block.term.kind {
            TermKind::Ret { values } | TermKind::RetDefer { values, .. } => values.clone(),
            TermKind::Throw(value) | TermKind::Raise(value) | TermKind::Panic(value) => vec![*value],
            TermKind::Catch { failure, .. } | TermKind::FailureDispatch { failure, .. } => vec![*failure],
            TermKind::ExitDispatch { closed, failure, exit, .. } => vec![*closed, *failure, *exit],
            TermKind::CleanupNext { cleanup, .. } => vec![*cleanup],
            TermKind::Cond { condition, .. } => vec![*condition],
            TermKind::Switch { value, .. } => vec![*value],
            TermKind::Lookup { collection, key, .. } => vec![*collection, *key],
            TermKind::InvokeOwn { op, .. } => super::dominance::leaves(&**op, body, state),
            TermKind::InvokeDefer { call, .. } => super::dominance::leaves(call, body, state),
            TermKind::Br(_) | TermKind::Trap { .. } | TermKind::Unreachable => vec![],
        };
        if operands.iter().any(|value| watched.contains(value)) {
            state.at(L::Terminator { function, block: block_id }, block.term.loc);
            state.error(R::B91, "an await unwind continues with the failure of its await.unwind, \
                never the failure the adapter observed (§13.3)");
        }
    }
}
