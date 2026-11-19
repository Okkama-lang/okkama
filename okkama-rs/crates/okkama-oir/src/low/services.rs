//! D16, review B4: the service duties that each close settles.
//!
//! A service schedule of an owner scope lists its duties in the creation order of their Futures (lead's
//! ruling on V3). A close settles the duty tokens of its owner that are available there, in that order: one
//! `cleanup.settle` per duty before the plan (OIR-2 §9.3 step 3), named by the close-local schedule of its
//! `cleanup.begin`; Low bodies keep no service schedules of their own (B10.6).
//!
//! A duty is identified by the token that created it: a token resumed from a frame slot is the token that
//! the matching `frame.save` stored, and a block parameter is the duty that every incoming edge passes. So
//! the order does not depend on whether the schedule lists one version per close or one per owner.
//!
//! Duty slots (D26, #856): a `duty.empty` of the owner passes no duty. A slot version is identified by the creation
//! that reaches it on some edges; a version that only empties reach is an empty slot of its scope. A close of a
//! scope with slots settles every slot version there: the full ones at their creation's schedule position, the
//! empty ones at the positions of their kind that no full version takes. A scope without `duty.empty` keeps the
//! form before slots: its close lists the tokens available there, possibly fewer than its schedule.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

/// A close: an instruction, or an `invoke.own` close named by its normal successor, which stays the same
/// block while the expansion splits the block of the close.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum CloseKey {
    Instruction(InstId),
    Edge(BlockId),
}

/// The schedule of one close. `dynamic`: after the static duties, one step settles the entries its owner
/// admitted at run time (D25.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CloseDuties {
    pub duties: Vec<ValueId>,
    pub dictionary: Option<ValueId>,
    pub protocol: ProtocolId,
    pub dynamic: bool,
}

fn invalid(function: FuncId, message: &str) -> LowError {
    LowError::InvalidInput { function: Some(function), message: message.into() }
}

fn closed_scope(module: &Module, body: &Body, op: &Operation) -> Result<Option<ScopeId>, LowError> {
    let Operation::Ownership(OwnershipOp::ScopeEnd { state, .. } | OwnershipOp::ScopeUnwind { state, .. }) = op
    else {
        return Ok(None);
    };
    match module.types.get(body.values.get(*state)?.ty)? {
        Type::Service(ServiceType::Scope(scope)) => Ok(Some(*scope)),
        _ => Ok(None),
    }
}

/// Assign every close of an owner scope the schedule that lists exactly the duties available there:
/// defined on every path and not yet consumed by an operation or by an earlier close of the owner.
pub(super) fn assign(
    module: &Module, function: FuncId, body: &Body, budget: &mut Budget<'_>,
) -> Result<BTreeMap<CloseKey, CloseDuties>, LowError> {
    // D25.3: an owner whose schedules end with `dynamic` settles its admitted entries at every close.
    let mut dynamic: BTreeMap<ScopeId, (Option<ValueId>, ProtocolId)> = BTreeMap::new();
    for schedule in &body.service_schedules {
        budget.step(schedule.duties.len())?;
        let count = schedule.duties.iter().filter(|duty| **duty == ScheduleDuty::Dynamic).count();
        if count > 1 || count == 1 && schedule.duties.last() != Some(&ScheduleDuty::Dynamic) {
            return Err(invalid(function, "a schedule ends with its one dynamic step (D25.3)"));
        }
        if count == 1 {
            dynamic.insert(schedule.owner, (schedule.dictionary, schedule.protocol));
        }
    }
    for schedule in &body.service_schedules {
        if dynamic.contains_key(&schedule.owner) && schedule.duties.last() != Some(&ScheduleDuty::Dynamic) {
            return Err(invalid(function, "every schedule of an owner with admitted entries ends with dynamic"));
        }
    }
    // C-1: every cache-state token is a version of the duty that one `fut.new` gave its owner scope; D24.1: a
    // spawn forwards that duty and gives its owner the task duty, a task-state token. D26: a version that only
    // empties reach is an empty slot of their scope; one that a creation reaches on some edges is that duty.
    let mut owners: BTreeMap<ValueId, ScopeId> = BTreeMap::new();
    for (value, data) in body.values.iter() {
        budget.step(1)?;
        let task = match module.types.get(data.ty)? {
            Type::Service(ServiceType::CacheState) => false,
            Type::Service(ServiceType::TaskState) => true,
            _ => continue,
        };
        let reach = reaching(body, value, budget)?;
        let created = reach.as_ref().and_then(Reaching::creation);
        let empty = reach.as_ref().and_then(Reaching::empty_owner);
        if task && empty.is_none() && !created.is_some_and(|token| spawned_task(body, token)) {
            // An await's task handle reaches only `await.unwind` (C-3); it is no duty. A spawned task's duty that
            // edges merge from several spawns has no one owner, and the verifier finds it unsettled.
            continue;
        }
        let owner = match (created, empty) {
            (Some(token), _) => duty_owner(body, token, budget).ok().flatten(),
            (None, owner) => owner,
        };
        // D26: the empties a version merges are slots of one scope, its duty's owner when a creation reaches it.
        let scopes: BTreeSet<ScopeId> = reach.iter().flat_map(|reach| reach.empty.iter().map(|&(scope, _)| scope))
            .collect();
        if scopes.len() > 1 || owner.is_some_and(|owner| scopes.iter().any(|&scope| scope != owner)) {
            return Err(invalid(function, "a slot version joins slots of different scopes (D26)"));
        }
        let owner = owner.ok_or_else(|| invalid(function, "a duty token is a duty of one fut.new (C-1, D24.1)"))?;
        owners.insert(value, owner);
    }
    let mut available_at: BTreeMap<CloseKey, (ScopeId, BTreeSet<ValueId>)> = BTreeMap::new();
    if !owners.is_empty() {
        available(module, body, &owners, &mut available_at, budget)?;
    }
    // The closes of the owners with admitted entries, whether or not a token reaches them.
    closes_of(module, body, &dynamic, &mut available_at, budget)?;
    let mut assigned = BTreeMap::new();
    for (key, (scope, duties)) in available_at {
        let admits = dynamic.get(&scope).copied();
        if duties.is_empty() {
            if let Some((dictionary, protocol)) = admits {
                assigned.insert(key, CloseDuties { duties: vec![], dictionary, protocol, dynamic: true });
            }
            continue;
        }
        let mut tokens = BTreeMap::new();
        // D26: the versions that only empties reach, by kind (a task slot or a cache slot), in value order.
        let mut empties: BTreeMap<bool, Vec<ValueId>> = BTreeMap::new();
        for &duty in &duties {
            let reach = reaching(body, duty, budget)?;
            if let Some((_, task)) = reach.as_ref().and_then(Reaching::empty_slot) {
                empties.entry(task).or_default().push(duty);
                continue;
            }
            let creation = reach.as_ref().and_then(Reaching::creation)
                .ok_or_else(|| invalid(function, "a duty token reaches its close from more than one duty"))?;
            if tokens.insert(creation, duty).is_some() {
                return Err(invalid(function, "two tokens of one duty reach a close"));
            }
        }
        let mut chosen = None;
        for schedule in body.service_schedules.iter().filter(|schedule| schedule.owner == scope) {
            let mut order = Vec::new();
            for duty in schedule.duties.iter().filter_map(|duty| duty.value()) {
                order.push(creation(body, duty, budget)?);
            }
            if tokens.keys().all(|creation| order.contains(&Some(*creation))) {
                chosen = Some((schedule, order));
                break;
            }
        }
        let message = "no service schedule of the owner lists the duties that reach its close";
        let (schedule, order) = chosen.ok_or_else(|| invalid(function, message))?;
        let ordered = if slotted(body, scope) {
            settle_every_slot(module, body, schedule, &order, &tokens, empties)
                .ok_or_else(|| invalid(function, "a close does not settle every slot of its scope (D26)"))?
        } else {
            order.into_iter().flatten().filter_map(|creation| tokens.get(&creation).copied()).collect()
        };
        assigned.insert(key, CloseDuties {
            duties: ordered, dictionary: schedule.dictionary, protocol: schedule.protocol, dynamic: admits.is_some(),
        });
    }
    Ok(assigned)
}

/// D26: whether the body has a `duty.empty` of `scope`, so that its closes settle every slot of the scope.
fn slotted(body: &Body, scope: ScopeId) -> bool {
    body.insts.iter().any(|(_, inst)| matches!(inst.op, Operation::Async(AsyncOp::DutyEmpty { owner, .. })
        if owner == scope))
}

/// D26: the settle order of a close of a scope with slots. Each position of the schedule takes the full version of
/// its creation, or an empty version of its kind when that creation does not reach the close. `None` unless every
/// version at the close takes exactly one position and every position is taken (R-3).
fn settle_every_slot(
    module: &Module, body: &Body, schedule: &ServiceSchedule, order: &[Option<ValueId>],
    tokens: &BTreeMap<ValueId, ValueId>, mut empties: BTreeMap<bool, Vec<ValueId>>,
) -> Option<Vec<ValueId>> {
    let entries: Vec<ValueId> = schedule.duties.iter().filter_map(|duty| duty.value()).collect();
    let mut ordered = Vec::with_capacity(entries.len());
    let mut placed = BTreeSet::new();
    for (entry, creation) in entries.iter().zip(order) {
        let duty = match creation.as_ref().and_then(|creation| tokens.get(creation)) {
            Some(&duty) => duty,
            None => {
                let task = matches!(module.types.get(body.values.get(*entry).ok()?.ty).ok()?,
                    Type::Service(ServiceType::TaskState));
                let kind = empties.get_mut(&task).filter(|kind| !kind.is_empty())?;
                kind.remove(0)
            }
        };
        if !placed.insert(duty) {
            return None;
        }
        ordered.push(duty);
    }
    let complete = tokens.values().all(|duty| placed.contains(duty)) && empties.values().all(Vec::is_empty);
    complete.then_some(ordered)
}

/// Add every close of an owner in `owners` to `sites`, with no duties if no token reaches it.
fn closes_of(
    module: &Module, body: &Body, owners: &BTreeMap<ScopeId, (Option<ValueId>, ProtocolId)>,
    sites: &mut BTreeMap<CloseKey, (ScopeId, BTreeSet<ValueId>)>, budget: &mut Budget<'_>,
) -> Result<(), LowError> {
    if owners.is_empty() {
        return Ok(());
    }
    for (_, data) in body.blocks.iter() {
        budget.step(data.insts.len() + 1)?;
        for &inst in &data.insts {
            if let Some(scope) = closed_scope(module, body, &body.insts.get(inst)?.op)?
                && owners.contains_key(&scope) {
                sites.entry(CloseKey::Instruction(inst)).or_insert_with(|| (scope, BTreeSet::new()));
            }
        }
        if let TermKind::InvokeOwn { op, normal, .. } = &data.term.kind
            && let Some(scope) = closed_scope(module, body, op)?
            && owners.contains_key(&scope) {
            sites.entry(CloseKey::Edge(normal.block)).or_insert_with(|| (scope, BTreeSet::new()));
        }
    }
    Ok(())
}

/// What reaches a duty version `value` (D26): the tokens that created a duty, and the empty slots, by scope and
/// kind. A resumed version is what its save stored; a block parameter is what every incoming edge passes.
struct Reaching {
    created: BTreeSet<ValueId>,
    empty: BTreeSet<(ScopeId, bool)>,
}
impl Reaching {
    /// The one creation that reaches the version; empties pass no duty.
    fn creation(&self) -> Option<ValueId> {
        let mut created = self.created.iter();
        created.next().copied().filter(|_| created.next().is_none())
    }
    /// The empty slot, scope and kind, of a version that only empties of one slot reach.
    fn empty_slot(&self) -> Option<(ScopeId, bool)> {
        let mut empty = self.empty.iter();
        empty.next().copied().filter(|_| self.created.is_empty() && empty.next().is_none())
    }
    fn empty_owner(&self) -> Option<ScopeId> {
        self.empty_slot().map(|(scope, _)| scope)
    }
}

/// The token that created the duty `value` stands for, or `None` if edges pass different duties or only empties.
fn creation(body: &Body, value: ValueId, budget: &mut Budget<'_>) -> Result<Option<ValueId>, LowError> {
    Ok(reaching(body, value, budget)?.as_ref().and_then(Reaching::creation))
}

/// The definitions that reach `value`, or `None` when one cannot be traced (an outcome slot, a block without
/// edges, a resume without its save).
fn reaching(body: &Body, value: ValueId, budget: &mut Budget<'_>) -> Result<Option<Reaching>, LowError> {
    let mut pending = vec![value];
    let mut seen = BTreeSet::new();
    let mut found = Reaching { created: BTreeSet::new(), empty: BTreeSet::new() };
    while let Some(value) = pending.pop() {
        budget.step(1)?;
        if !seen.insert(value) {
            continue;
        }
        match body.values.get(value)?.def {
            Definition::InstResult { inst, index } if index > 0 => {
                let Operation::Async(AsyncOp::FrameResume { frame, slots, .. }) = &body.insts.get(inst)?.op else {
                    found.created.insert(value);
                    continue;
                };
                let slot = slots.get(usize::from(index) - 1).map(|slot| slot.slot);
                let saved = match body.values.get(*frame)?.def {
                    Definition::InstResult { inst, .. } => match &body.insts.get(inst)?.op {
                        Operation::Async(AsyncOp::FrameSave { slots, .. }) => {
                            slots.iter().find(|item| Some(item.slot) == slot).map(|item| item.value)
                        }
                        _ => None,
                    },
                    Definition::BlockParam { .. } => None,
                };
                let Some(saved) = saved else {
                    return Ok(None);
                };
                pending.push(saved);
            }
            Definition::BlockParam { block, index } => {
                let incoming = plans::incoming(body, block, index as usize);
                if incoming.is_empty() {
                    return Ok(None);
                }
                for (_, arg) in incoming {
                    let EdgeArg::Value(source) = arg else {
                        return Ok(None);
                    };
                    pending.push(source);
                }
            }
            Definition::InstResult { inst, .. } => match body.insts.get(inst)?.op {
                Operation::Async(AsyncOp::DutyEmpty { owner, task }) => {
                    found.empty.insert((owner, task));
                }
                _ => {
                    found.created.insert(value);
                }
            },
        }
    }
    Ok(Some(found))
}

/// Whether `token` is the task duty that a `fut.spawn` gave (D24.1).
fn spawned_task(body: &Body, token: ValueId) -> bool {
    matches!(body.values.get(token).map(|data| data.def), Ok(Definition::InstResult { inst, index: 1 })
        if body.insts.get(inst).is_ok_and(|inst| matches!(inst.op, Operation::Async(AsyncOp::FutureSpawn { .. }))))
}

/// The owner scope of the duty `token` created: the owner of a `fut.new`'s cache duty, which a `fut.spawn` forwards
/// (result 0) and whose Future it gives a task duty (result 1).
fn duty_owner(body: &Body, token: ValueId, budget: &mut Budget<'_>) -> Result<Option<ScopeId>, LowError> {
    let Definition::InstResult { inst, index } = body.values.get(token)?.def else {
        return Ok(None);
    };
    match &body.insts.get(inst)?.op {
        Operation::Async(AsyncOp::FutureNew { owner, .. }) if index == 1 => Ok(Some(*owner)),
        Operation::Async(AsyncOp::FutureSpawn { state: Some(cache), .. }) if index <= 1 => {
            match creation(body, *cache, budget)? {
                Some(created) => duty_owner(body, created, budget),
                None => Ok(None),
            }
        }
        _ => Ok(None),
    }
}

/// Forward must-availability of duty tokens, recording the available duties of the owner at each close.
fn available(
    module: &Module, body: &Body, owners: &BTreeMap<ValueId, ScopeId>,
    sites: &mut BTreeMap<CloseKey, (ScopeId, BTreeSet<ValueId>)>, budget: &mut Budget<'_>,
) -> Result<(), LowError> {
    let count = body.blocks.len();
    let mut input: Vec<Option<BTreeSet<ValueId>>> = vec![None; count];
    input[body.entry.as_u32() as usize] = Some(BTreeSet::new());
    for report in [false, true] {
        let mut changed = true;
        while changed {
            changed = false;
            for (block, data) in body.blocks.iter() {
                budget.step(data.insts.len() + 1)?;
                let Some(mut live) = input[block.as_u32() as usize].clone() else {
                    continue;
                };
                live.extend(data.params.iter().filter(|value| owners.contains_key(value)));
                for &inst in &data.insts {
                    let instruction = body.insts.get(inst)?;
                    let key = CloseKey::Instruction(inst);
                    step(module, body, key, &instruction.op, owners, &mut live, report.then_some(&mut *sites))?;
                    live.extend(instruction.results.iter().filter(|value| owners.contains_key(value)));
                }
                if let TermKind::InvokeOwn { op, normal, .. } = &data.term.kind {
                    let key = CloseKey::Edge(normal.block);
                    step(module, body, key, op, owners, &mut live, report.then_some(&mut *sites))?;
                }
                if report {
                    continue;
                }
                for (target, moved) in edges(body, block)? {
                    // A token passed as an edge argument moves into the target's parameter (D26: slot versions
                    // merge this way), so its old name is not available there.
                    let live: BTreeSet<ValueId> = live.difference(&moved).copied().collect();
                    let slot = &mut input[target.as_u32() as usize];
                    let merged = match slot {
                        None => live.clone(),
                        Some(previous) => previous.intersection(&live).copied().collect(),
                    };
                    if slot.as_ref() != Some(&merged) {
                        *slot = Some(merged);
                        changed = true;
                    }
                }
            }
            if report {
                break;
            }
        }
    }
    Ok(())
}

/// The edges out of `block`, each with the values it passes as arguments. An edge the terminator does not name
/// with arguments (a block's unwind edge, the other terminators) passes none.
fn edges(body: &Body, block: BlockId) -> Result<Vec<(BlockId, BTreeSet<ValueId>)>, LowError> {
    let data = body.blocks.get(block)?;
    let plain = |target: &Target| (target.block, target.args.iter().copied().collect());
    let mut edges: Vec<(BlockId, BTreeSet<ValueId>)> = match &data.term.kind {
        TermKind::Br(target) => vec![plain(target)],
        TermKind::Cond { yes, no, .. } => vec![plain(yes), plain(no)],
        TermKind::Switch { cases, default, .. } => cases.iter().map(|case| &case.target).chain([default])
            .map(plain).collect(),
        TermKind::InvokeOwn { normal, unwind, .. } => [normal, unwind].into_iter().map(|edge| {
            (edge.block, edge.args.iter().filter_map(|arg| match arg {
                EdgeArg::Value(value) => Some(*value),
                _ => None,
            }).collect())
        }).collect(),
        _ => vec![],
    };
    if let Some(unwind) = &data.unwind {
        edges.push((unwind.target.block, BTreeSet::new()));
    }
    for target in graph::successors(body, block)? {
        if !edges.iter().any(|(known, _)| *known == target) {
            edges.push((target, BTreeSet::new()));
        }
    }
    Ok(edges)
}

fn step(
    module: &Module, body: &Body, key: CloseKey, op: &Operation, owners: &BTreeMap<ValueId, ScopeId>,
    live: &mut BTreeSet<ValueId>, sites: Option<&mut BTreeMap<CloseKey, (ScopeId, BTreeSet<ValueId>)>>,
) -> Result<(), LowError> {
    if let Some(scope) = closed_scope(module, body, op)? {
        let settled: BTreeSet<ValueId> = live.iter().copied().filter(|duty| owners.get(duty) == Some(&scope))
            .collect();
        for duty in &settled {
            live.remove(duty);
        }
        if let Some(sites) = sites {
            sites.insert(key, (scope, settled));
        }
    }
    for operand in op.operand_refs() {
        if let (OperandRef::Value(value), OperandMode::Consume) = (operand.reference, operand.mode) {
            live.remove(&value);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(source: &str) -> Module {
        text::parse(source).unwrap_or_else(|error| panic!("{error}"))
    }

    fn narrowed(source: &str) -> (Module, FuncId) {
        let mut module = parsed(source);
        let (function, _) = module.functions.iter().find(|(_, function)| function.symbol == "main").unwrap();
        let options = LowOptions::default();
        let mut budget = Budget { options: &options, steps: 0, blocks: 0, roots: 0 };
        let body = module.functions.get_mut(function).unwrap().body.as_mut().unwrap();
        narrow::closes(body, &mut budget).unwrap();
        (module, function)
    }

    fn duties(module: &Module, function: FuncId) -> Result<Vec<Vec<String>>, LowError> {
        let body = module.functions.get(function).unwrap().body.as_ref().unwrap();
        let options = LowOptions::default();
        let mut budget = Budget { options: &options, steps: 0, blocks: 0, roots: 0 };
        let assigned = assign(module, function, body, &mut budget)?;
        Ok(assigned.values().map(|close| close.duties.iter().map(|value| {
            let ty = body.values.get(*value).unwrap().ty;
            format!("{:?}", module.types.get(ty).unwrap())
        }).collect()).collect())
    }

    /// async-never-awaited with a loop after the creation whose header carries the cache duty as a parameter: the
    /// edge into the loop passes the token, the back edge passes the parameter (#805's shape, without the await).
    fn loop_carried(header_args: &str) -> String {
        let source = include_str!("../../tests/fixtures/async/coro/async-never-awaited.oir");
        let close = "    %8: int = const 0\n    %9: closed = scope.end %6,";
        assert!(source.contains(close));
        source.replace("duties [%5]", "duties [%14]").replace(close, &format!("    br ^b2({header_args})\n    \
            ^b2(%13: scope<S1>, %14: cache-state) in S1:\n    %15: bool = const false\n    \
            cond %15, ^b2(%13, %14), ^b3\n    \
            ^b3 in S1:\n    %8: int = const 0\n    %9: closed = scope.end %13,"))
    }

    /// #805: a duty carried round a loop as a block parameter reaches the owner's close once. The token an edge
    /// passes moves into the parameter, so the token's old name does not also reach the close.
    #[test]
    fn a_duty_carried_round_a_loop_reaches_its_close_once() {
        let (module, function) = narrowed(&loop_carried("%6, %5"));
        assert_eq!(duties(&module, function).unwrap(), vec![vec!["Service(CacheState)".to_string()]]);
    }

    /// Two versions of one duty that both reach a close are refused: one token passed into two parameters.
    #[test]
    fn two_versions_of_one_duty_at_a_close_are_refused() {
        let source = loop_carried("%6, %5").replace("^b2(%13: scope<S1>, %14: cache-state)",
            "^b2(%13: scope<S1>, %14: cache-state, %16: cache-state)").replace("br ^b2(%6, %5)", "br ^b2(%6, %5, %5)")
            .replace("cond %15, ^b2(%13, %14)", "cond %15, ^b2(%13, %14, %16)");
        let (module, function) = narrowed(&source);
        let error = duties(&module, function).unwrap_err();
        assert!(error.to_string().contains("two tokens of one duty reach a close"), "{error}");
    }

    /// Each close of main settles the duty version that reaches it: two closes, one cache duty each.
    #[test]
    fn each_close_gets_the_schedule_of_its_reaching_duties() {
        let (module, function) = narrowed(include_str!("../../tests/fixtures/async/coro/async-plain-await.oir"));
        let assigned = duties(&module, function).unwrap();
        assert_eq!(assigned.len(), 2);
        assert!(assigned.iter().all(|close| close.len() == 1), "{assigned:?}");
    }

    /// Two duties settle in the creation order of their Futures, the order of their schedule.
    #[test]
    fn duties_keep_the_creation_order_of_the_schedule() {
        let source = include_str!("../../tests/fixtures/async/coro/coro-schedule-duty-live-across-await.oir");
        let (module, function) = narrowed(source);
        let body = module.functions.get(function).unwrap().body.as_ref().unwrap();
        let options = LowOptions::default();
        let mut budget = Budget { options: &options, steps: 0, blocks: 0, roots: 0 };
        let assigned = assign(&module, function, body, &mut budget).unwrap();
        assert_eq!(assigned.len(), 2);
        for close in assigned.values() {
            let schedule = body.service_schedules.iter().find(|schedule| {
                schedule.duties.iter().filter_map(|duty| duty.value()).collect::<Vec<_>>() == close.duties
            });
            assert!(schedule.is_some(), "the duties of a close follow one schedule in order");
        }
    }

    /// A close whose reaching duties no schedule of the owner lists is invalid input.
    #[test]
    fn a_close_without_its_schedule_is_invalid() {
        let source = include_str!("../../tests/fixtures/async/coro/async-never-awaited.oir");
        let (mut module, function) = narrowed(source);
        let body = module.functions.get_mut(function).unwrap().body.as_mut().unwrap();
        body.service_schedules[0].duties.clear();
        let error = duties(&module, function).unwrap_err();
        assert!(matches!(error, LowError::InvalidInput { .. }), "{error:?}");
    }

    /// One schedule per owner scope, listing the creating tokens (the own form of D16), gives each close the
    /// versions that reach it, in creation order.
    #[test]
    fn one_schedule_per_owner_names_the_creating_tokens() {
        let source = include_str!("../../tests/fixtures/async/coro/coro-schedule-duty-live-across-await.oir");
        let (mut module, function) = narrowed(source);
        let body = module.functions.get_mut(function).unwrap().body.as_mut().unwrap();
        let created: Vec<ValueId> = body.blocks.iter().flat_map(|(_, block)| block.insts.clone())
            .filter_map(|inst| match body.insts.get(inst).unwrap().op {
                Operation::Async(AsyncOp::FutureNew { .. }) => Some(body.insts.get(inst).unwrap().results[1]),
                _ => None,
            }).collect();
        assert_eq!(created.len(), 2);
        let mut schedule = body.service_schedules[0].clone();
        schedule.duties = created.iter().copied().map(ScheduleDuty::Value).collect();
        body.service_schedules = vec![schedule];
        let body = module.functions.get(function).unwrap().body.as_ref().unwrap();
        let options = LowOptions::default();
        let mut budget = Budget { options: &options, steps: 0, blocks: 0, roots: 0 };
        let assigned = assign(&module, function, body, &mut budget).unwrap();
        assert_eq!(assigned.len(), 2);
        for close in assigned.values() {
            let order: Vec<_> = close.duties.iter()
                .map(|duty| creation(body, *duty, &mut budget).unwrap().unwrap()).collect();
            assert_eq!(order, created, "duties follow the creation order");
        }
    }

    fn dynamic_closes(module: &Module, function: FuncId) -> Result<Vec<(usize, bool)>, LowError> {
        let body = module.functions.get(function).unwrap().body.as_ref().unwrap();
        let options = LowOptions::default();
        let mut budget = Budget { options: &options, steps: 0, blocks: 0, roots: 0 };
        let assigned = assign(module, function, body, &mut budget)?;
        Ok(assigned.values().map(|close| (close.duties.len(), close.dynamic)).collect())
    }

    /// D25.3: an owner whose schedule ends with `dynamic` settles its admitted entries at every close, after the
    /// static duties, including a close that no token reaches.
    #[test]
    fn a_dynamic_entry_gives_every_close_of_its_owner_the_dynamic_step() {
        let source = include_str!("../../tests/fixtures/async/coro/async-never-awaited.oir");
        let (module, function) = narrowed(source);
        let before = dynamic_closes(&module, function).unwrap();
        assert!(before.iter().all(|(_, dynamic)| !dynamic), "{before:?}");
        let (mut module, function) = narrowed(source);
        let body = module.functions.get_mut(function).unwrap().body.as_mut().unwrap();
        let owner = body.service_schedules[0].owner;
        body.service_schedules[0].duties.push(ScheduleDuty::Dynamic);
        let closes = dynamic_closes(&module, function).unwrap();
        assert!(closes.iter().all(|(_, dynamic)| *dynamic), "{closes:?}");
        // The owner's closes that no token reaches settle their entries too.
        let body = module.functions.get(function).unwrap().body.as_ref().unwrap();
        let sites = body.blocks.iter().flat_map(|(_, block)| block.insts.iter().copied()).filter(|&inst| {
            closed_scope(&module, body, &body.insts.get(inst).unwrap().op).unwrap() == Some(owner)
        }).count() + body.blocks.iter().filter(|(_, block)| matches!(&block.term.kind,
            TermKind::InvokeOwn { op, .. } if closed_scope(&module, body, op).unwrap() == Some(owner))).count();
        assert_eq!(closes.len(), sites);
    }

    /// A schedule ends with its one dynamic step, and every schedule of such an owner has it.
    #[test]
    fn a_dynamic_step_ends_every_schedule_of_its_owner_once() {
        let source = include_str!("../../tests/fixtures/async/coro/async-never-awaited.oir");
        let (mut module, function) = narrowed(source);
        let body = module.functions.get_mut(function).unwrap().body.as_mut().unwrap();
        body.service_schedules[0].duties.insert(0, ScheduleDuty::Dynamic);
        assert!(matches!(dynamic_closes(&module, function), Err(LowError::InvalidInput { .. })));
        let (mut module, function) = narrowed(source);
        let body = module.functions.get_mut(function).unwrap().body.as_mut().unwrap();
        body.service_schedules[0].duties.extend([ScheduleDuty::Dynamic, ScheduleDuty::Dynamic]);
        assert!(matches!(dynamic_closes(&module, function), Err(LowError::InvalidInput { .. })));
        let (mut module, function) = narrowed(source);
        let body = module.functions.get_mut(function).unwrap().body.as_mut().unwrap();
        let mut second = body.service_schedules[0].clone();
        second.name = "Other".into();
        second.duties = vec![];
        body.service_schedules[0].duties.push(ScheduleDuty::Dynamic);
        body.service_schedules.push(second);
        assert!(matches!(dynamic_closes(&module, function), Err(LowError::InvalidInput { .. })));
    }
}
