//! Cleanup phase and exactly-once checks over concrete SPB edges.
use super::*;
use std::collections::BTreeMap;

type Check = Result<(), Error>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase { Begun, Settled, Ready, Selected, Awaiting, Failed, Done }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Cleanup {
    close: u32,
    plan: Option<u32>,
    index: u32,
    phase: Phase,
    pending: bool,
    aborted: bool,
    failure: bool,
    payload: Option<PendingType>,
    step: Option<u32>,
    /// Duties of `schedule` settled so far, in schedule order.
    settled: u32,
    schedule: Option<u32>,
    /// A settled cache may release obligations (an outcome or unstarted inputs) into this close's plan.
    released: bool,
    /// `cleanup.deliver_cancel` ran in this close.
    delivered: bool,
    /// Plans this close has begun: the plan, then for a delivered cancellation the aborted reserve's.
    plans: u32,
    /// The close's `cleanup.settle.dynamic` ran (ABI D25.3).
    dynamic: bool,
    /// The closing scope may have admitted a service entry, so the dynamic step is required (D25.3).
    admits: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingType { value: Option<Type>, kind: ExitKind, rehome: Option<Rehome> }
/// The cache duties a pending reserve carries since `cache.rehome` (C-4): the scope that owned them, the
/// destination the rehome named, and whether the owner's close has taken the reserve yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rehome { owner: u32, target: ExitDestination, closed: bool }

/// A Future escapes its owner S only through S's exit: `cache.rehome` puts S's cache duty into the pending
/// reserve of that exit, the next close to take the reserve is S's, and the exit commits at the destination the
/// rehome named (C-4 and Claude's ruling of 2026-11-15 on #601 R1).
pub(crate) const REHOME: &str = "SPB_REHOME";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fact {
    Cleanup(Cleanup), Step(u32, u32), Captured(u32, u32), Failure(u32), Exit(u32, Option<PendingType>),
    Key(u32), Pending(PendingType),
}
type State = BTreeMap<Location, Fact>;

fn require(condition: bool, message: &str) -> Check {
    if condition { Ok(()) } else { Err(error(message)) }
}

pub(super) fn check(p: &Program, f: &Function, destroy_masks: &[u32]) -> Check {
    let inferred = infer(p, f, destroy_masks)?;
    for evidence in &f.proofs.cleanup_plans {
        if let Some(reserve) = inferred[evidence.plan as usize] {
            enforce(evidence.reserve == reserve, "EVIDENCE_RESERVE", "runtime plan reserve relocation mismatch")?;
        } else {
            let close = &f.proofs.closes[evidence.close as usize];
            enforce(evidence.reserve == close.reserve, "EVIDENCE_RESERVE",
                "unused plan must match its close reserve contract")?;
        }
    }
    Ok(())
}

pub fn infer_plan_reserves(p: &Program, f: &Function) -> Result<Vec<Option<bool>>, Error> {
    let mut evidence = evidence::Evidence::new(p, f);
    let masks = (0..f.proofs.closes.len()).map(|close| {
        load::visible_destroy_failures(p, f, close as u32, &mut evidence)
    }).collect::<Result<Vec<_>, _>>()?;
    infer(p, f, &masks)
}

/// Whether the close's current plan has run: the executor reached done, or the direct chain its last slot.
fn plan_complete(f: &Function, cleanup: &Cleanup) -> Result<bool, Error> {
    let plan = cleanup.plan.ok_or_else(|| error("no current plan"))?;
    if f.proofs.plans[plan as usize].merge {
        return Ok(cleanup.phase == Phase::Done);
    }
    let evidence = f.proofs.cleanup_plans.iter().find(|evidence| evidence.plan == plan)
        .ok_or_else(|| error("missing suffix evidence"))?;
    Ok(cleanup.phase == Phase::Ready && cleanup.index as usize == evidence.slots.len())
}

/// One step of a nonempty schedule: the next duty in order, owned by the closing scope.
fn settle_duty(p: &Program, f: &Function, origins: &BTreeMap<Location, Option<u32>>, cleanup: &mut Cleanup,
    schedule: u32, duty: Location) -> Check
{
    let duties = &f.proofs.schedules[schedule as usize].services;
    enforce(cleanup.phase == Phase::Begun && cleanup.schedule.is_none_or(|current| current == schedule),
        suspension::SCHEDULE, "a close settles the duties of one schedule, before its plan")?;
    enforce(duties.get(cleanup.settled as usize) == Some(&duty), suspension::SCHEDULE,
        "each scheduled duty is settled once, in schedule order")?;
    let origin = origins.get(&duty).copied().flatten();
    let creation = origin.map(|pc| &f.code[pc as usize]);
    let Some(Instruction::Champ { op: Opcode::FutureNew, arguments, .. }) = creation else {
        return Err(violation(suspension::SCHEDULE, "a settled duty has no single fut.new"));
    };
    // D24.2, ABI §6.3: a spawned Future's task duty settles before its cache duty, which may then detach with it.
    let task = |location: Location| verify::loc_type(f, location) == Ok(ProducedType::Service(ServiceKind::TaskState));
    let task_later = duties.iter().skip(cleanup.settled as usize + 1)
        .any(|&other| task(other) && origins.get(&other).copied().flatten() == origin);
    enforce(task(duty) || !task_later, suspension::SCHEDULE,
        "a spawned Future's task duty is settled before its cache duty (D24.2)")?;
    let owner = arguments.iter().find_map(|arg| match arg { Argument::Scope(id) => Some(*id), _ => None });
    let close = &f.proofs.closes[cleanup.close as usize];
    let closing = p.tables.boundaries.get(close.boundary as usize).map(|boundary| boundary.scope);
    enforce(owner.is_some() && owner == closing, suspension::SCHEDULE,
        "a duty is settled by a scope that does not own it")?;
    let callee = arguments.iter().find_map(|arg| match arg {
        Argument::Entry(id) => p.functions.get(*id as usize),
        _ => None,
    }).ok_or_else(|| error("fut.new has no entry"))?;
    cleanup.released |= !plain_inputs(p, callee) || !plain_results(p, callee);
    cleanup.schedule = Some(schedule);
    cleanup.settled += 1;
    if cleanup.settled as usize == duties.len() {
        cleanup.phase = Phase::Settled;
    }
    Ok(())
}

fn infer(p: &Program, f: &Function, destroy_masks: &[u32]) -> Result<Vec<Option<bool>>, Error> {
    let mut reserves = vec![None; f.proofs.plans.len()];
    if f.code.is_empty() { return Ok(reserves) }
    let origins = suspension::cache_origins(p, f);
    let admitting = services::admitting_closes(p, f)?;
    let rehome_scopes = services::rehome_scopes(p, f)?;
    let fixed_root = p.functions.iter().position(|function| std::ptr::eq(function, f))
        .is_some_and(|index| suspension::fixed_root(p, index as u32));
    // Facts of the values each continuation's frame.save stored, for its resumes.
    let mut saved = BTreeMap::<u32, Vec<Option<Fact>>>::new();
    let mut incoming = vec![None; f.code.len()];
    incoming[0] = Some(State::new());
    let mut work = vec![0];
    while let Some(pc) = work.pop() {
        let mut state = incoming[pc].as_ref().ok_or_else(|| error("cleanup worklist"))?.clone();
        let inst = &f.code[pc];
        let mut produced = Vec::new();
        let mut dispatch = None;
        if let Instruction::Champ { op, arguments, flow } = inst {
            let locations = arguments.iter().filter_map(|arg| match arg {
                Argument::Location(location) => Some(*location), _ => None,
            }).collect::<Vec<_>>();
            let operands = locations.iter().map(|loc| state.get(loc).copied()).collect::<Vec<_>>();
            for location in load::consumed(f, *op, arguments) {
                state.remove(&location);
            }
            let cleanup = || match operands.first().copied().flatten() {
                Some(Fact::Cleanup(cleanup)) => Ok(cleanup),
                _ => Err(error("operation has no current cleanup activation")),
            };
            let unary = |cleanup| vec![Some(Fact::Cleanup(cleanup))];
            use Opcode::*;
            produced = match op {
                ExitPrepare | ExitReplace => {
                    let offset = usize::from(*op == ExitReplace);
                    let value = if locations.len() == offset + 2 {
                        let ProducedType::Value(ty) = verify::loc_type(f, locations[offset])? else {
                            return Err(violation("OPERAND_VALUE", "pending requires a language value"));
                        };
                        Some(ty)
                    } else { None };
                    let kind = arguments.iter().find_map(|arg| match arg {
                        Argument::Kind(kind) => Some(*kind), _ => None,
                    }).ok_or_else(|| error("missing exit kind"))?;
                    if *op == ExitReplace {
                        // Which reserve the rehomed duties would join is not decided for a replaced exit.
                        enforce(!matches!(operands[0], Some(Fact::Pending(PendingType { rehome: Some(_), .. }))),
                            REHOME, "exit.replace of a reserve that holds rehomed cache duties")?;
                    }
                    let mut facts = vec![Some(Fact::Pending(PendingType { value, kind, rehome: None }))];
                    if *op == ExitReplace { facts.push(None); }
                    facts
                }
                CacheRehome => {
                    let Some(Fact::Pending(mut payload)) = operands[0] else {
                        return Err(violation(REHOME, "cache.rehome outside an exit's preparation"));
                    };
                    let owner = if services::rehome_state_form(f, &locations) {
                        // The state form moves an entry the owner admitted: the owner is the state's scope.
                        rehome_scopes.get(&pc).copied().flatten()
                            .ok_or_else(|| violation(REHOME, "a rehomed entry leaves the state of one scope"))?
                    } else {
                        let origin = locations.get(1).and_then(|duty| origins.get(duty)).copied().flatten();
                        let creation = origin.map(|pc| &f.code[pc as usize]);
                        let Some(Instruction::Champ { op: FutureNew, arguments: created, .. }) = creation else {
                            return Err(violation(REHOME, "a rehomed duty has no single fut.new"));
                        };
                        // #765, C-4 item 1: a spawned Future's task and cache duties leave together, and the task
                        // is the one of the same Future.
                        if let Some(task) = locations.get(2) {
                            enforce(origins.get(task).copied().flatten() == origin, REHOME,
                                "a rehome's task duty belongs to the Future of its cache duty")?;
                        } else {
                            let spawned = f.code.iter().any(|instruction| matches!(instruction,
                                Instruction::Champ { op: FutureSpawn, arguments, .. }
                                    if services::spawned_cache(arguments)
                                        .is_some_and(|cache| origins.get(&cache).copied().flatten() == origin)));
                            enforce(!spawned, REHOME, "a spawned Future's cache duty leaves without its task duty")?;
                        }
                        created.iter().find_map(|arg| match arg { Argument::Scope(id) => Some(*id), _ => None })
                            .ok_or_else(|| error("fut.new has no owner scope"))?
                    };
                    let target = arguments.iter().find_map(|arg| match arg {
                        Argument::Destination(destination) => Some(*destination),
                        _ => None,
                    }).ok_or_else(|| error("cache.rehome has no target"))?;
                    let rehome = Rehome { owner, target, closed: false };
                    enforce(payload.rehome.is_none_or(|previous| previous == rehome), REHOME,
                        "one reserve takes the duties of one owner, for one destination")?;
                    payload.rehome = Some(rehome);
                    // The state form gives the state back first.
                    if services::rehome_state_form(f, &locations) {
                        vec![None, Some(Fact::Pending(payload))]
                    } else {
                        vec![Some(Fact::Pending(payload))]
                    }
                }
                ExitExtend => vec![operands[0]],
                ExitCommit => {
                    let Some(Fact::Pending(payload)) = operands[0] else {
                        return Err(violation("PENDING_COMMIT_TYPE", "commit has no typed pending"));
                    };
                    let context = verify::result_context(p, f, *op, arguments)?;
                    enforce(payload.value == context.value
                        && (payload.kind != ExitKind::Return || payload.value == f.results.first().copied()),
                        if payload.kind == ExitKind::Return { "PENDING_RETURN_TYPE" } else { "PENDING_COMMIT_TYPE" },
                        "commit result differs from recorded pending value")?;
                    if let Some(rehome) = payload.rehome {
                        enforce(rehome.closed, REHOME, "rehomed duties commit without their owner's close")?;
                        let destination = arguments.iter().find_map(|arg| match arg {
                            Argument::Destination(destination) => Some(*destination),
                            _ => None,
                        });
                        enforce(destination == Some(rehome.target), REHOME,
                            "a rehome names the destination of the exit that carries it")?;
                    }
                    vec![]
                }
                CleanupBegin => {
                    let (close, contract) = f.proofs.closes.iter().enumerate()
                        .find(|(_, close)| close.begin_pc == pc as u32)
                        .ok_or_else(|| error("cleanup.begin has no close descriptor"))?;
                    let expected = 1 + usize::from(contract.reserve) + usize::from(contract.active_failure);
                    enforce(locations.len() == expected, "CLOSE_TAIL", "close operand count mismatch")?;
                    let mut tail = Vec::new();
                    if contract.reserve { tail.push(ProducedType::Service(ServiceKind::Pending)); }
                    if contract.active_failure { tail.push(ProducedType::Value(Type::Failure)); }
                    let actual = locations[1..].iter().map(|&loc| verify::loc_type(f, loc))
                        .collect::<Result<Vec<_>, _>>()?;
                    enforce(actual == tail, "CLOSE_TAIL", "close tail must match reserve/active_failure")?;
                    let payload = if contract.reserve {
                        let Some(Fact::Pending(mut payload)) = operands[1] else {
                            return Err(violation("PENDING_COMMIT_TYPE", "close has no typed pending"));
                        };
                        if let Some(rehome) = payload.rehome.as_mut().filter(|rehome| !rehome.closed) {
                            let closing = p.tables.boundaries.get(contract.boundary as usize)
                                .map(|boundary| boundary.scope);
                            enforce(closing == Some(rehome.owner), REHOME,
                                "rehomed duties leave through the exit of the scope that owns them")?;
                            rehome.closed = true;
                        }
                        Some(payload)
                    } else { None };
                    require(arguments.contains(&Argument::Envelope(contract.envelope))
                        && arguments.contains(&Argument::Boundary(contract.boundary)), "close descriptor mismatch")?;
                    unary(Cleanup { close: close as u32, plan: None, index: 0, phase: Phase::Begun,
                        pending: contract.reserve, aborted: false, failure: contract.active_failure,
                        payload, step: None, settled: 0, schedule: None, released: false, delivered: false,
                        plans: 0, dynamic: false, admits: admitting.contains(&(pc as u32)) })
                }
                CleanupSettle => {
                    let mut cleanup = cleanup()?;
                    let schedule = arguments.iter().find_map(|arg| match arg {
                        Argument::Schedule(id) => Some(*id),
                        _ => None,
                    }).ok_or_else(|| error("missing schedule"))?;
                    let duties = &f.proofs.schedules[schedule as usize].services;
                    if let Some(&duty) = locations.get(1) {
                        settle_duty(p, f, &origins, &mut cleanup, schedule, duty)?;
                    } else {
                        enforce(duties.is_empty(), suspension::SCHEDULE,
                            "a nonempty schedule is settled duty by duty")?;
                        require(cleanup.phase == Phase::Begun, "settle outside begin phase")?;
                        cleanup.phase = Phase::Settled;
                    }
                    unary(cleanup)
                }
                // D25.3: after the static duties, once, before the plan. Its entries may release obligations.
                CleanupSettleDynamic => {
                    let mut cleanup = cleanup()?;
                    enforce(cleanup.phase == Phase::Settled && cleanup.plans == 0, suspension::SCHEDULE,
                        "cleanup.settle.dynamic follows the static duties and precedes the plan")?;
                    enforce(!cleanup.dynamic, suspension::SCHEDULE, "a close settles its dynamic entries once")?;
                    cleanup.dynamic = true;
                    cleanup.released = true;
                    unary(cleanup)
                }
                // D18 at the final exit: settle, the plan, then the delivery, then the aborted reserve's plan.
                CleanupDeliverCancel => {
                    let mut cleanup = cleanup()?;
                    let close = &f.proofs.closes[cleanup.close as usize];
                    let envelope = f.proofs.envelopes[close.envelope as usize].failures;
                    enforce(envelope & ENVELOPE_CANCEL != 0, suspension::ASYNC_ENTRY,
                        "a close delivers a deferred cancellation exactly when its envelope declares one: none here")?;
                    let root = p.tables.boundaries.get(close.boundary as usize)
                        .and_then(|boundary| p.tables.scopes.get(boundary.scope as usize))
                        .is_some_and(|scope| scope.parent.is_none());
                    // The successful final exit: a body's returns; a fixed root has no caller and closes without a
                    // reserve (#667).
                    let final_exit = match cleanup.payload {
                        Some(payload) => payload.kind == ExitKind::Return,
                        None => !cleanup.pending && fixed_root,
                    };
                    enforce(root && final_exit && !close.active_failure, suspension::ASYNC_ENTRY,
                        "a deferred cancellation is delivered only at the final exit of the body")?;
                    enforce(!cleanup.delivered, suspension::ASYNC_ENTRY, "a deferred cancellation is delivered once")?;
                    enforce(cleanup.plans == 1 && plan_complete(f, &cleanup)?, suspension::ASYNC_ENTRY,
                        "a deferred cancellation is delivered after the settle phase and the first plan")?;
                    cleanup.delivered = true;
                    unary(cleanup)
                }
                FrameSave => {
                    let continuation = suspension::continuation_of(arguments)
                        .ok_or_else(|| error("no continuation"))?;
                    let facts = operands[1..].to_vec();
                    if let Some(previous) = saved.insert(continuation, facts.clone()) {
                        require(previous == facts, "one continuation saves different cleanup facts")?;
                    }
                    vec![None]
                }
                FrameResume => {
                    let continuation = suspension::continuation_of(arguments)
                        .ok_or_else(|| error("no continuation"))?;
                    let mut facts = vec![None];
                    if continuation != 0 {
                        facts.extend(saved.get(&continuation).ok_or_else(|| error("resume before its save"))?);
                    }
                    facts
                }
                CleanupPlan => {
                    let mut cleanup = cleanup()?;
                    let id = arguments.iter().find_map(|arg| match arg { Argument::Plan(id) => Some(*id), _ => None })
                        .ok_or_else(|| error("missing plan id"))?;
                    if cleanup.plans == 0 {
                        require(cleanup.phase == Phase::Settled, "plan before complete settle")?;
                        enforce(cleanup.dynamic || !cleanup.admits, suspension::SCHEDULE,
                            "a close of a scope that may admit service entries settles them dynamically first")?;
                    } else {
                        // The second plan executes the reserve that the delivered cancellation aborts.
                        enforce(cleanup.plans == 1 && cleanup.delivered, suspension::ASYNC_ENTRY,
                            "a second plan follows a delivered cancellation; a close has at most two plans")?;
                        let merge = f.proofs.plans.get(id as usize).is_some_and(|plan| plan.merge);
                        enforce(cleanup.plan != Some(id) && merge, suspension::ASYNC_ENTRY,
                            "the aborted reserve runs its own executor plan")?;
                        cleanup.index = 0;
                    }
                    cleanup.plans += 1;
                    let evidence = f.proofs.cleanup_plans.iter().find(|plan| plan.plan == id)
                        .ok_or_else(|| error("plan has no evidence"))?;
                    require(evidence.close == cleanup.close, "plan belongs to another close")?;
                    enforce(!cleanup.aborted || f.proofs.plans[id as usize].merge,
                        "ABORT_MERGE", "aborted reserve requires a merge plan")?;
                    let reserve = cleanup.pending || cleanup.aborted;
                    if let Some(previous) = reserves[id as usize] {
                        require(previous == reserve, "one runtime plan has inconsistent reserve inventory")?;
                    }
                    reserves[id as usize] = Some(reserve);
                    enforce(!cleanup.released || f.proofs.plans[id as usize].merge, suspension::SCHEDULE,
                        "a settled cache releases obligations into the common plan, which needs the executor")?;
                    cleanup.plan = Some(id);
                    cleanup.phase = Phase::Ready;
                    unary(cleanup)
                }
                CleanupNext => {
                    let cleanup = cleanup()?;
                    require(cleanup.phase == Phase::Ready, "next before capture/finish of current")?;
                    require(cleanup.plan.is_some_and(|id| f.proofs.plans[id as usize].merge),
                        "cleanup.next requires executor plan")?;
                    dispatch = Some(Fact::Cleanup(cleanup));
                    vec![]
                }
                DestroyDirect | CleanupDestroy => {
                    let mut cleanup = cleanup()?;
                    let plan = cleanup.plan.ok_or_else(|| error("destroy before plan"))?;
                    if *op == DestroyDirect {
                        require(cleanup.phase == Phase::Ready && !f.proofs.plans[plan as usize].merge,
                            "direct destroy outside direct ready phase")?;
                        let evidence = f.proofs.cleanup_plans.iter().find(|evidence| evidence.plan == plan)
                            .ok_or_else(|| error("missing direct evidence"))?;
                        let slot = evidence.slots.get(cleanup.index as usize)
                            .ok_or_else(|| error("repeated direct current or exhausted suffix"))?;
                        require(locations.get(1) == Some(&slot.identity)
                            && arguments.contains(&Argument::Entry(slot.destroy)), "direct chain skips current")?;
                        let failures = p.functions[slot.destroy as usize].proofs.entry.as_ref()
                            .map_or(15, |entry| entry.failures);
                        let close = &f.proofs.closes[cleanup.close as usize];
                        require(failures & !f.proofs.envelopes[close.envelope as usize].failures == 0,
                            "understated D18 destroy envelope")?;
                        cleanup.index += 1;
                    } else {
                        require(cleanup.phase == Phase::Selected
                            && operands.get(1) == Some(&Some(Fact::Key(cleanup.close))),
                            "destroy does not use the selected key")?;
                    }
                    let close = &f.proofs.closes[cleanup.close as usize];
                    if *op == CleanupDestroy {
                        let failures = destroy_masks[cleanup.close as usize];
                        enforce(failures & !f.proofs.envelopes[close.envelope as usize].failures == 0,
                            "D18_EXECUTOR", "executor envelope omits visible destroy failures")?;
                    }
                    cleanup.phase = Phase::Awaiting;
                    cleanup.step = Some(pc as u32);
                    vec![Some(Fact::Cleanup(cleanup)), Some(Fact::Step(cleanup.close, pc as u32))]
                }
                FailureDispatch => {
                    let Some(fact @ Fact::Step(_, _)) = operands.first().copied().flatten() else {
                        return Err(error("failure.dispatch has no current destroy result"));
                    };
                    dispatch = Some(fact);
                    vec![]
                }
                CleanupFailure => {
                    let mut cleanup = cleanup()?;
                    require(matches!(cleanup.phase, Phase::Begun | Phase::Failed),
                        "cleanup.failure without captured failure or begin phase")?;
                    if cleanup.phase == Phase::Failed {
                        enforce(operands.get(1) == Some(&Some(Fact::Captured(cleanup.close,
                            cleanup.step.ok_or_else(|| error("missing destroy step"))?))),
                            "CLEANUP_FAILURE_SLOT", "failure belongs to a different dispatch slot")?;
                        cleanup.phase = Phase::Ready;
                        cleanup.step = None;
                    } else {
                        let close = &f.proofs.closes[cleanup.close as usize];
                        enforce(close.active_failure || f.proofs.envelopes[close.envelope as usize].failures != 0,
                            "CLEANUP_FAILURE_BEGIN", "failure injection exceeds close contract")?;
                    }
                    if let Some(location) = locations.get(1) { state.remove(location); }
                    cleanup.failure = true;
                    unary(cleanup)
                }
                CleanupFinish => {
                    let cleanup = cleanup()?;
                    let plan = cleanup.plan.ok_or_else(|| error("finish without plan"))?;
                    let complete = if f.proofs.plans[plan as usize].merge {
                        cleanup.phase == Phase::Done
                    } else {
                        let evidence = f.proofs.cleanup_plans.iter().find(|evidence| evidence.plan == plan)
                            .ok_or_else(|| error("missing suffix evidence"))?;
                        cleanup.phase == Phase::Ready && cleanup.index as usize == evidence.slots.len()
                    };
                    require(complete, "cleanup.finish loses current or suffix")?;
                    let envelope = &f.proofs.envelopes[f.proofs.closes[cleanup.close as usize].envelope as usize];
                    let declared = envelope.failures & ENVELOPE_CANCEL != 0;
                    enforce(cleanup.delivered || !declared, suspension::ASYNC_ENTRY,
                        "a close delivers a deferred cancellation exactly when its envelope declares one: never here")?;
                    enforce(!cleanup.delivered || cleanup.plans == 2, suspension::ASYNC_ENTRY,
                        "a delivered cancellation runs the aborted reserve's plan before the close finishes")?;
                    vec![None, Some(Fact::Failure(cleanup.close)), Some(Fact::Exit(cleanup.close, cleanup.payload))]
                }
                ExitDispatch => {
                    let [_, Some(Fact::Failure(close)), Some(Fact::Exit(exit, payload))] = operands.as_slice() else {
                        return Err(error("exit.dispatch has no matching completed close"));
                    };
                    require(close == exit, "exit/failure belong to different closes")?;
                    let contract = &f.proofs.closes[*close as usize];
                    let flags = arguments.iter().filter_map(|arg| match arg {
                        Argument::Flag(flag) => Some(*flag), _ => None,
                    }).collect::<Vec<_>>();
                    require(flags == [contract.reserve, contract.active_failure,
                        f.proofs.envelopes[contract.envelope as usize].failures != 0],
                        "exit.dispatch understates close contract")?;
                    dispatch = Some(Fact::Exit(*close, *payload));
                    vec![]
                }
                ExitAbort => {
                    let Some(Fact::Cleanup(mut cleanup)) = operands.get(1).copied().flatten() else {
                        return Err(error("abort without cleanup"));
                    };
                    require(!cleanup.pending && !cleanup.aborted, "pending consumed twice")?;
                    require(matches!(cleanup.phase, Phase::Begun | Phase::Settled | Phase::Ready),
                        "abort interrupts current before finish/dead")?;
                    require(cleanup.failure,
                        "abort requires an actual failure")?;
                    enforce(cleanup.plan.is_none_or(|id| f.proofs.plans[id as usize].merge),
                        "ABORT_MERGE", "reserve insertion requires executor (B2)")?;
                    cleanup.aborted = true;
                    unary(cleanup)
                }
                _ => vec![],
            };
            if let Flow::Next(outputs) = flow {
                for (&location, fact) in outputs.iter().zip(&produced) {
                    if let Some(fact) = fact { state.insert(location, *fact); }
                }
            }
        }
        let mut successors = Vec::new();
        for (ordinal, edge) in allocation::outgoing(inst).iter().enumerate() {
            let mut next = state.clone();
            let mut produced = produced.clone();
            if let Some(fact) = dispatch {
                match fact {
                    Fact::Cleanup(mut cleanup) => {
                        cleanup.phase = if ordinal == 0 { Phase::Selected } else { Phase::Done };
                        produced = vec![Some(Fact::Cleanup(cleanup)), Some(Fact::Key(cleanup.close))];
                    }
                    Fact::Exit(_, payload) => {
                        if let Instruction::Champ { flow: Flow::Outcomes(outcomes), .. } = inst
                            && outcomes[ordinal].tag == Tag::Pending
                        {
                            produced = vec![None, payload.map(Fact::Pending)];
                        }
                    }
                    Fact::Step(close, step) => {
                        if ordinal == 1 { produced = vec![Some(Fact::Captured(close, step))]; }
                        for fact in next.values_mut() {
                            if let Fact::Cleanup(cleanup) = fact
                                && cleanup.close == close && cleanup.phase == Phase::Awaiting
                            {
                                cleanup.phase = if ordinal == 0 { Phase::Ready } else { Phase::Failed };
                                if ordinal == 0 { cleanup.step = None; }
                            }
                        }
                    }
                    _ => {}
                }
            }
            let before = next.clone();
            for assignment in &edge.assignments {
                if let Source::Location(source) = assignment.source { next.remove(&source); }
            }
            for assignment in &edge.assignments {
                let fact = match assignment.source {
                    Source::Location(source) => before.get(&source).copied(),
                    Source::ProducedSlot(slot) => produced.get(slot as usize).copied().flatten(),
                };
                if let Some(fact) = fact { next.insert(assignment.destination, fact); }
            }
            successors.push((edge.target_pc as usize, next));
        }
        if allocation::outgoing(inst).is_empty()
            && matches!(inst, Instruction::Data { .. } | Instruction::Champ { flow: Flow::Next(_), .. })
            && !matches!(inst, Instruction::Champ { op: Opcode::RetOwn | Opcode::RetDefer, .. })
        {
            successors.push((pc + 1, state));
        }
        for (target, next) in successors {
            let previous = incoming.get_mut(target).ok_or_else(|| error("cleanup CFG target"))?;
            if let Some(previous) = previous {
                require(previous.keys().eq(next.keys()), "cleanup activations disagree at join")?;
                let mut changed = false;
                for (location, old) in previous.iter_mut() {
                    let new = next[location];
                    if let (Fact::Cleanup(old_cleanup), Fact::Cleanup(mut new_cleanup)) = (&mut *old, new) {
                        let failure = old_cleanup.failure && new_cleanup.failure;
                        changed |= old_cleanup.failure != failure;
                        old_cleanup.failure = failure;
                        new_cleanup.failure = failure;
                        require(*old_cleanup == new_cleanup, "cleanup phases disagree at join")?;
                    } else {
                        require(*old == new, "cleanup results disagree at join")?;
                    }
                }
                if changed { work.push(target); }
            } else {
                *previous = Some(next);
                work.push(target);
            }
        }
    }
    Ok(reserves)
}
