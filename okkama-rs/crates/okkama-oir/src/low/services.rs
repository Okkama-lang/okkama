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
    // spawn forwards that duty and gives its owner the task duty, a task-state token.
    let mut owners: BTreeMap<ValueId, ScopeId> = BTreeMap::new();
    for (value, data) in body.values.iter() {
        budget.step(1)?;
        let task = match module.types.get(data.ty)? {
            Type::Service(ServiceType::CacheState) => false,
            Type::Service(ServiceType::TaskState) => true,
            _ => continue,
        };
        let created = creation(body, value, budget)?;
        if task && !created.is_some_and(|token| spawned_task(body, token)) {
            // An await's task handle reaches only `await.unwind` (C-3); it is no duty. A spawned task's duty that
            // edges merge from several spawns has no one owner, and the verifier finds it unsettled.
            continue;
        }
        let owner = created.and_then(|token| duty_owner(body, token, budget).ok().flatten())
            .ok_or_else(|| invalid(function, "a duty token is a duty of one fut.new (C-1, D24.1)"))?;
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
        for &duty in &duties {
            let creation = creation(body, duty, budget)?
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
        let ordered = order.into_iter().flatten().filter_map(|creation| tokens.get(&creation).copied()).collect();
        assigned.insert(key, CloseDuties {
            duties: ordered, dictionary: schedule.dictionary, protocol: schedule.protocol, dynamic: admits.is_some(),
        });
    }
    Ok(assigned)
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

/// The token that created the duty `value` stands for, or `None` if edges pass different duties.
fn creation(body: &Body, value: ValueId, budget: &mut Budget<'_>) -> Result<Option<ValueId>, LowError> {
    let mut pending = vec![value];
    let mut seen = BTreeSet::new();
    let mut found = None;
    while let Some(value) = pending.pop() {
        budget.step(1)?;
        if !seen.insert(value) {
            continue;
        }
        match body.values.get(value)?.def {
            Definition::InstResult { inst, index } if index > 0 => {
                let Operation::Async(AsyncOp::FrameResume { frame, slots, .. }) = &body.insts.get(inst)?.op else {
                    found = merge(found, value);
                    if found.is_none() {
                        return Ok(None);
                    }
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
            Definition::InstResult { .. } => {
                found = merge(found, value);
                if found.is_none() {
                    return Ok(None);
                }
            }
        }
    }
    Ok(found)
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

fn merge(found: Option<ValueId>, value: ValueId) -> Option<ValueId> {
    match found {
        Some(previous) if previous != value => None,
        _ => Some(value),
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
                for target in graph::successors(body, block)? {
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
