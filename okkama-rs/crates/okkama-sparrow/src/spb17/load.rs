//! Executable load gate. Structural verification is always performed first.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

type Check = Result<(), Error>;
type Active = BTreeSet<Location>;

/// The artifact cannot add contracts to this catalog.
pub trait ExternalContracts {
    fn entry(&self, name: &str) -> Option<&Function>;

    /// The checked unresolved-provider adapter for a declaration whose name has no trusted entry
    /// (P2.4 §4.7); `None` rejects it. The adapter is recomputed from the declaration's transport,
    /// never taken from it, and the declaration must then equal it exactly.
    fn adapter(&self, _declared: &Function) -> Option<Function> {
        None
    }
}
#[derive(Debug)]
pub struct NoExternalContracts;
impl ExternalContracts for NoExternalContracts {
    fn entry(&self, _: &str) -> Option<&Function> { None }
}

fn require(condition: bool, message: &str) -> Check {
    if condition { Ok(()) } else { Err(error(message)) }
}

fn linear(f: &Function, location: Location) -> bool {
    matches!(location, Location::Service(id) if f.services[id as usize].linear())
}

/// Operand consumption table. Language values, captures, selections and erased proofs never consume services.
/// context() checks each operand kind before this table is used by the executable gate.
pub(super) fn consumed(f: &Function, op: Opcode, arguments: &[Argument]) -> Vec<Location> {
    use Opcode::*;
    let locations = arguments.iter().filter_map(|arg| match arg {
        Argument::Location(location) => Some(*location), _ => None,
    }).collect::<Vec<_>>();
    let between = arguments.first() == Some(&Argument::Flag(true));
    locations.iter().copied().enumerate().filter_map(|(index, location)| {
        let consume = match op {
            ScopeBegin | OwnEmpty | OwnCreateCommit | DepsOf | DepsEmpty | InvokeObserve | InvokeDispatch
                | ArrayGet | Fail | Evaluate | Catch | Lookup => false,
            // A Future is a language value: awaiting, cancelling or reading it consumes nothing. Its proof
            // produces a capability, which the take consumes.
            Await | FutureCancel | FutureDone | FutureCancelled | UniqueCacheProof => false,
            AwaitTake => index == 1,
            // Every linear operand: fut.new takes the input packet and the owner state; fut.publish the frame
            // and the success packet; frame.save the frame and the saved tokens; await.unwind the state, task and
            // receipt; cleanup.settle the cleanup state and its duty.
            FutureNew | FuturePublish | FrameSave | FrameResume | AwaitUnwind | CleanupDeliverCancel
                | CleanupSettle | CacheRehome | CleanupSettleDynamic | ServiceAbsorb => true,
            // The holder of a materialized entry is a language value; only the packet is consumed.
            ServiceMaterialize => index == 0,
            // The journal packet travels with the buffer; the buffer is a register (check_builders).
            BuilderPush | BuilderAppend | BuilderRange | BuilderFinish | BuilderAbandon => {
                index + 1 == locations.len()
            }
            OwnMove | OwnJoin | OwnAdmit | OwnAccept | OwnDynamic | CleanupBegin | ExitAbort | ExitDispatch => true,
            OwnBind | OwnRebind => index == 0 || index == 2,
            OwnStage => index < 1 + usize::from(between) || index + 1 == locations.len(),
            ScopeTransfer => index < 1 + usize::from(between),
            OwnSplit | OwnSplitChecked | ScopeSplit | DeferRegister | ExitCommit
                | CleanupPlan | CleanupNext | CleanupFinish | CleanupFailure | DestroyDirect | FailureDispatch => {
                    index == 0
                }
            CleanupDestroy | ExitExtend => true,
            ExitPrepare => index + 1 == locations.len(),
            ExitReplace => index == 0 || index + 1 == locations.len(),
            InvokeCallOwn | InvokeDispatchOwn | InvokeDefer | RetOwn | RetDefer => {
                matches!(verify::loc_type(f, location), Ok(ProducedType::Service(ServiceKind::Packet)))
            }
        };
        (consume && linear(f, location)).then_some(location)
    }).collect()
}

/// Reconstruct service lifetimes from instructions and parallel edges, never from supplied StateRecords.
pub fn service_states(f: &Function) -> Result<Vec<StateRecord>, Error> {
    if f.code.is_empty() {
        return Ok(vec![]);
    }
    let mut incoming: Vec<Option<Active>> = vec![None; f.code.len()];
    incoming[0] = Some(f.params.iter().copied().filter(|&loc| linear(f, loc)).collect());
    let mut stored = incoming[0].as_ref().map_or(0, Active::len);
    let mut work = vec![0];
    while let Some(pc) = work.pop() {
        let mut active = incoming[pc].as_ref().ok_or_else(|| error("service worklist"))?.clone();
        let inst = &f.code[pc];
        let mut terminal = matches!(inst, Instruction::Ret(_) | Instruction::Raise(_) | Instruction::Unreachable);
        if let Instruction::Champ { op, arguments, flow } = inst {
            for location in consumed(f, *op, arguments) {
                enforce(active.remove(&location), "LINEAR_CONSUME", "linear service consumed twice")?;
            }
            if let Flow::Next(outputs) = flow {
                for &location in outputs.iter().filter(|&&location| linear(f, location)) {
                    require(active.insert(location), "linear service overwritten")?;
                }
            }
            terminal = matches!(op, Opcode::RetOwn | Opcode::RetDefer);
        }
        if terminal {
            require(active.is_empty(), "terminal loses live service state")?;
        }
        let edges = allocation::outgoing(inst);
        let mut successors = Vec::new();
        for edge in &edges {
            let mut next = active.clone();
            let mut destinations = Vec::new();
            let mut consumed = Active::new();
            let mut produced = BTreeSet::new();
            for assignment in &edge.assignments {
                if !linear(f, assignment.destination) {
                    continue;
                }
                match assignment.source {
                    Source::Location(source) => {
                        require(active.contains(&source) && consumed.insert(source),
                            "edge duplicates or forwards a consumed service")?;
                    }
                    Source::ProducedSlot(slot) => {
                        require(produced.insert(slot), "edge duplicates produced service")?;
                    }
                }
                destinations.push(assignment.destination);
            }
            for location in consumed {
                next.remove(&location);
            }
            for destination in destinations {
                require(next.insert(destination), "edge overwrites live service")?;
            }
            successors.push((edge.target_pc as usize, next));
        }
        if edges.is_empty() && !terminal {
            successors.push((pc + 1, active));
        }
        for (target, next) in successors {
            let previous = incoming.get_mut(target).ok_or_else(|| error("service target outside code"))?;
            if let Some(previous) = previous {
                require(*previous == next, "inconsistent live services at CFG join")?;
            } else {
                stored = stored.saturating_add(next.len());
                enforce(stored <= 8 * 1024 * 1024, "LOAD_WORK_BUDGET",
                    "service-state storage exceeds verification work budget")?;
                *previous = Some(next);
                work.push(target);
            }
        }
    }
    Ok(incoming.into_iter().enumerate().filter_map(|(pc, active)| {
        let locations = active?.into_iter().collect::<Vec<_>>();
        (!locations.is_empty()).then_some(StateRecord { pc: pc as u32, locations })
    }).collect())
}

pub fn verify_load(p: &Program, contracts: &dyn ExternalContracts) -> Check {
    for function in &p.functions {
        enforce(function.code.len() <= 65_536
            && function.registers.len().saturating_add(function.services.len()) <= 8 * 1024 * 1024
            && function.blocks.len().saturating_mul(function.blocks.len().div_ceil(64)) <= 8 * 1024 * 1024
            && function.proofs.roots.iter().chain(&function.proofs.post_roots)
                .map(|root| root.locations.len()).sum::<usize>() <= 8 * 1024 * 1024,
            "LOAD_WORK_BUDGET", "function exceeds verification work budget")?;
    }
    verify(p)?;
    for (id, object) in p.objects.iter().enumerate() {
        let entries = p.functions.iter().filter(|function| function.proofs.entry.as_ref()
            .is_some_and(|entry| entry.destroy == Some(id as u32))).collect::<Vec<_>>();
        require(entries.len() == usize::from(object.layout.has_destroy),
            "descriptor destroy entry inventory mismatch")?;
        for entry in entries {
            require(entry.entry_abi == EntryAbi::Observation && entry.results.is_empty()
                && entry.params.len() == 1
                && verify::loc_type(entry, entry.params[0]) == Ok(ProducedType::Value(Type::Object(id as u32))),
                "destroy entry signature mismatch")?;
        }
    }
    // D25.1: which calls return their Future's entry, proved from the callees' returns.
    let fresh = services::fresh_futures(p)?;
    for (index, f) in p.functions.iter().enumerate() {
        if matches!(f.entry_abi, EntryAbi::Owned | EntryAbi::Defer { .. }) {
            require(f.params.iter().filter(|&&location| verify::loc_type(f, location)
                == Ok(ProducedType::Service(ServiceKind::Packet))).count() == 1,
                "owning entry requires one input packet")?;
        }
        enforce(f.params.iter().all(|&location| !matches!(verify::loc_type(f, location),
            Ok(ProducedType::Service(ServiceKind::Pending | ServiceKind::Cleanup | ServiceKind::ExitState
                | ServiceKind::StableKey | ServiceKind::DestroyBorrow | ServiceKind::ScopeState
                | ServiceKind::FailureState | ServiceKind::CacheState | ServiceKind::TaskState
                | ServiceKind::Receipt | ServiceKind::UniqueCache)))),
            "ENTRY_PRIVATE_SERVICE", "private service contents cannot be entry arguments")?;
        if let Some(entry) = &f.proofs.entry
            && let Some(name) = &entry.external
        {
            let adapter;
            let trusted = match contracts.entry(name) {
                Some(trusted) => trusted,
                None => {
                    adapter = contracts.adapter(f).ok_or_else(|| violation("EXTERNAL_UNRESOLVED",
                        format!("external {name} has no trusted entry and no data or handle adapter")))?;
                    &adapter
                }
            };
            enforce(f.entry_abi == trusted.entry_abi && f.registers == trusted.registers
                && f.services == trusted.services && f.params == trusted.params && f.results == trusted.results
                && f.proofs.entry == trusted.proofs.entry,
                "EXTERNAL_SIGNATURE", "external signature/summary mismatch")?;
            continue;
        }
        check_fallible_data(f)?;
        check_creations(p, f)?;
        check_failure_mask(p, f)?;
        check_call_contracts(p, f)?;
        check_service_results(p, f)?;
        suspension::check_flow(p, f)?;
        take::check(p, index as u32, f)?;
        services::check_materialize(p, f, &fresh)?;
        let states = service_states(f)?;
        enforce(f.proofs.states == states, "EVIDENCE_STATES",
            "service maps differ from reconstructed lifetimes")?;
        check_roots(f)?;
        let mut reconstructed = evidence::Evidence::new(p, f);
        let destroy_masks = (0..f.proofs.closes.len()).map(|close| {
            visible_destroy_failures(p, f, close as u32, &mut reconstructed)
        }).collect::<Result<Vec<_>, _>>()?;
        check_plans(p, f, &mut reconstructed, &destroy_masks)?;
        check_fresh(f, &mut reconstructed)?;
        enforce(f.proofs.static_selections == evidence::static_selections_with(p, f, &mut reconstructed)?,
            "EVIDENCE_SELECTIONS", "static selection relocation mismatch")?;
        reconstructed.check_budget()?;
        automaton::check(p, f, &destroy_masks)?;
        let allocation = allocate(f)?;
        check_allocation(f, &allocation)?;
    }
    Ok(())
}

fn traced(f: &Function, location: Location) -> bool {
    match location {
        Location::Register(id) => !matches!(f.registers[id as usize],
            Type::Bool | Type::Never | Type::Descriptor | Type::Byte | Type::Float | Type::Char),
        Location::Service(id) => !layout::service_layout(f.services[id as usize]).references.is_empty(),
    }
}

fn root_index(roots: &[RootRequirement]) -> BTreeMap<u32, Active> {
    let mut index = BTreeMap::<u32, Active>::new();
    for root in roots { index.entry(root.pc).or_default().extend(&root.locations); }
    index
}

fn check_roots(f: &Function) -> Check {
    let mut bare = f.clone();
    bare.proofs.roots.clear();
    bare.proofs.post_roots.clear();
    // State maps were reconstructed above; dependencies keep zero-word deps leaves alive.
    let live = live_locations(&bare)?;
    let supplied = root_index(&f.proofs.roots);
    let post = root_index(&f.proofs.post_roots);
    // An undef placeholder is the constant zero until a construction stores over it; verify() admits
    // it only as a construction operand or forwarded to a parameter, so it is never a reference.
    let placeholders = f.code.iter().filter_map(|inst| match inst {
        Instruction::Data { op: DataOp::Undef, output: Some(id), .. } => Some(Location::Register(*id)),
        _ => None,
    }).collect::<BTreeSet<_>>();
    let traced = |f: &Function, location: Location| traced(f, location) && !placeholders.contains(&location);
    for (pc, required) in live.iter().enumerate() {
        let roots = supplied.get(&(pc as u32));
        enforce(required.iter().filter(|&&loc| traced(f, loc))
            .all(|loc| roots.is_some_and(|roots| roots.contains(loc))),
            "ROOTS_BEFORE", &format!("missing abstract reference root at pc {pc}"))?;
        // J1/J2 consume both maps. Post is after the helper, before the next instruction/edge copy.
        // Outcome payloads live in the destination block maps; Post records themselves are never roots (§8).
        if matches!(f.code[pc], Instruction::Champ { flow: Flow::Next(_), .. })
            && !matches!(f.code[pc], Instruction::Champ { op: Opcode::RetOwn | Opcode::RetDefer, .. })
        {
            let after = live.get(pc + 1).into_iter().flatten().filter(|&&loc| traced(f, loc));
            enforce(after.clone().all(|loc| post.get(&(pc as u32)).is_some_and(|roots| roots.contains(loc))),
                "ROOTS_AFTER", &format!("missing traced live-out at pc {pc}: {:?}",
                    after.filter(|loc| !post.get(&(pc as u32)).is_some_and(|roots| roots.contains(loc)))
                        .collect::<Vec<_>>()))?;
        }
    }
    Ok(())
}

/// A proved empty inventory sees no descriptor. Opaque packets/journals conservatively see all
/// registered destroys, including descriptors reachable through fields and an aborted reserve.
pub(super) fn visible_destroy_failures(p: &Program, f: &Function, close: u32,
    evidence: &mut evidence::Evidence<'_>) -> Result<u32, Error>
{
    let contract = &f.proofs.closes[close as usize];
    let Instruction::Champ { op: Opcode::CleanupBegin, arguments, .. } = &f.code[contract.begin_pc as usize]
        else { return Err(violation("CLOSE_TAIL", "close is not cleanup.begin")); };
    let locations = arguments.iter().filter_map(|arg| match arg {
        Argument::Location(location) => Some(*location), _ => None,
    }).collect::<Vec<_>>();
    let reserve = f.proofs.cleanup_plans.iter().any(|plan| plan.close == close && plan.reserve);
    let empty_source = locations.first()
        .is_some_and(|&loc| evidence.inventory(loc).is_some_and(|keys| keys.is_empty()));
    let empty_reserve = !reserve || contract.reserve && locations.get(1)
        .is_some_and(|&loc| evidence.inventory(loc).is_some_and(|keys| keys.is_empty()));
    evidence.check_budget()?;
    // A settled cache releases its Future's outcome or unstarted inputs into the plan, outside the inventory.
    let releases = f.code.iter().any(|instruction| match instruction {
        Instruction::Champ { op: Opcode::FutureNew, arguments, .. } => arguments.iter().any(|arg| matches!(arg,
            Argument::Entry(id) if !plain_inputs(p, &p.functions[*id as usize])
                || !plain_results(p, &p.functions[*id as usize]))),
        _ => false,
    });
    if empty_source && empty_reserve && !releases { return Ok(0); }
    // A scope can only acquire input obligations or packets produced by this activation's calls.
    // Reconstruct their descriptor closure; unrelated module destroys cannot enter this inventory.
    let mut functions = vec![f];
    let mut seen = BTreeSet::new();
    let mut descriptors = BTreeSet::new();
    let mut types = Vec::new();
    let mut work = 0usize;
    while let Some(function) = functions.pop() {
        if !seen.insert(std::ptr::from_ref(function)) { continue; }
        work = work.saturating_add(function.code.len() + function.params.len());
        enforce(work <= 1_000_000, "LOAD_WORK_BUDGET", "destroy descriptor closure exceeds work budget")?;
        if function.entry_abi != EntryAbi::Observation {
            types.extend(function.params.iter().filter_map(|&location| match verify::loc_type(function, location) {
                Ok(ProducedType::Value(ty)) => Some(ty), _ => None,
            }));
        }
        // An async body's inputs arrive through its start continuation.
        if let Some(start) = p.frame_layout(function).and_then(|layout| layout.saved_types(0)) {
            types.extend(start.into_iter().filter_map(|ty| match ty {
                ProducedType::Value(ty) => Some(ty), _ => None,
            }));
        }
        for instruction in &function.code {
            let Instruction::Champ { op, arguments, .. } = instruction else { continue };
            if *op == Opcode::OwnCreateCommit {
                types.extend(arguments.iter().filter_map(|arg| match arg {
                    Argument::Object(id) => Some(Type::Object(*id)), _ => None,
                }));
            }
            if matches!(op, Opcode::InvokeCallOwn | Opcode::InvokeDefer | Opcode::FutureNew | Opcode::InvokeDispatchOwn) {
                for callee in dispatch_callees(p, arguments).into_iter().chain(arguments.iter().filter_map(|arg| {
                    match arg { Argument::Entry(id) => Some(&p.functions[*id as usize]), _ => None }
                })) {
                    // A settled cache releases a Future's outcome and its unstarted inputs into the plan.
                    let released = *op == Opcode::FutureNew && !plain_inputs(p, callee);
                    if !plain_results(p, callee) || *op == Opcode::InvokeDefer || released {
                        if callee.proofs.entry.as_ref().is_some_and(|entry| entry.external.is_some()) {
                            return Ok(p.functions.iter().filter_map(|f| f.proofs.entry.as_ref())
                                .filter(|entry| entry.destroy.is_some()).fold(0, |mask, entry| mask | entry.failures));
                        }
                        functions.push(callee);
                    }
                }
            }
        }
    }
    let mut errors = false;
    while let Some(ty) = types.pop() {
        work = work.saturating_add(1);
        enforce(work <= 1_000_000, "LOAD_WORK_BUDGET", "destroy type closure exceeds work budget")?;
        if ty == Type::Error && !errors {
            errors = true;
            types.extend(p.objects.iter().enumerate().filter_map(|(id, object)| {
                (!object.error_cases.is_empty()).then_some(Type::Object(id as u32))
            }));
        }
        if let Type::Object(id) | Type::Successor(id) = ty
            && descriptors.insert(id)
        {
            types.extend(&p.objects[id as usize].layout.storage_fields);
        }
    }
    Ok(p.functions.iter().filter_map(|function| function.proofs.entry.as_ref())
        .filter(|entry| entry.destroy.is_some_and(|id| descriptors.contains(&id)))
        .fold(0, |mask, entry| mask | entry.failures))
}

pub(super) fn check_plans(p: &Program, f: &Function, reconstructed: &mut evidence::Evidence<'_>, destroy_masks: &[u32])
    -> Check
{
    for (id, _) in f.proofs.plans.iter().enumerate() {
        require(f.proofs.cleanup_plans.iter().filter(|evidence| evidence.plan == id as u32).count() == 1,
            "missing or duplicate cleanup plan evidence")?;
    }
    for evidence in &f.proofs.cleanup_plans {
        let plan = f.proofs.plans.get(evidence.plan as usize).ok_or_else(|| error("invalid plan relocation"))?;
        let close = f.proofs.closes.get(evidence.close as usize).ok_or_else(|| error("invalid close relocation"))?;
        let envelope = f.proofs.envelopes.get(close.envelope as usize).ok_or_else(|| error("invalid D18"))?;
        let mut sources = vec![if matches!(&f.code[close.begin_pc as usize],
            Instruction::Champ { arguments, .. } if matches!(arguments.first(),
                Some(Argument::Location(Location::Service(id))) if f.services[*id as usize] == ServiceKind::Packet))
        { MergeSource::Packet } else { MergeSource::Scope }];
        if evidence.reserve {
            sources.push(MergeSource::Reserve);
        }
        enforce(evidence.sources == sources, "EVIDENCE_SOURCES", "plan omits inventory or reserve")?;

        let Instruction::Champ { op: Opcode::CleanupBegin, arguments: begin, .. }
            = &f.code[close.begin_pc as usize] else { return Err(error("close is not cleanup.begin")) };
        let source = begin.iter().filter_map(|argument| match argument {
            Argument::Location(location) => Some(*location), _ => None,
        }).collect::<Vec<_>>();
        let reserve_empty = !evidence.reserve || close.reserve && source.get(1)
            .is_some_and(|&pending| reconstructed.inventory(pending).is_some_and(|keys| keys.is_empty()));
        enforce(!evidence.reserve || close.reserve || plan.merge, "ABORT_MERGE",
            "an explicitly aborted reserve requires merge")?;
        if plan.merge {
            enforce(evidence.slots.is_empty() && evidence.suffixes.is_empty()
                && f.proofs.selections[plan.selection as usize].locations.is_empty()
                && f.proofs.order_witnesses[plan.order_witness as usize].creation_pcs.is_empty()
                && !evidence.reserve_empty, "MERGE_EVIDENCE", "merge carries direct evidence")?;
            let failures = destroy_masks[evidence.close as usize];
            enforce(failures & !envelope.failures == 0, "D18_EXECUTOR",
                &format!("{} close {}: merge envelope {} omits destroy failures {}",
                    f.symbol, evidence.close, envelope.failures, failures))?;
            continue;
        }
        require(evidence.reserve_empty == reserve_empty, "forged empty-reserve proof (B2)")?;
        require(envelope.failures == 0 || reserve_empty, "direct plan violates B2")?;
        require(!close.active_failure || reserve_empty, "active failure requires whole-reserve executor")?;
        require(evidence.suffixes.len() == evidence.slots.len(), "incomplete direct suffixes")?;
        let mut seen = Active::new();
        let mut creation_pcs = Vec::new();
        for (index, slot) in evidence.slots.iter().enumerate() {
            require(seen.insert(slot.identity), "duplicate direct identity")?;
            require(f.blocks.iter().any(|block| block.pc == slot.active_pc)
                && (slot.active_pc == close.begin_pc || evidence::precedes(f, slot.active_pc, close.begin_pc)),
                "direct active-slot witness does not reach close")?;
            let ty = verify::loc_type(f, slot.identity)?;
            let ProducedType::Value(Type::Object(object)) = ty else { return Err(error("direct identity type")) };
            require(matches!(slot.descriptor, Location::Register(id) if f.code.iter().any(|instruction| {
                matches!(instruction, Instruction::Data { op: DataOp::Const, output: Some(output),
                    immediate: Immediate::Constant(Constant::Descriptor(descriptor)), .. }
                    if *output == id && *descriptor == object)
            })), "direct descriptor witness does not match identity")?;
            require(p.functions.get(slot.destroy as usize).and_then(|f| f.proofs.entry.as_ref())
                .is_some_and(|entry| entry.destroy == Some(object)), "direct destroy descriptor/entry mismatch")?;
            let Some(actual) = reconstructed.identity(slot.identity) else {
                return evidence_rule(reconstructed, false, "SPB_STRUCTURE", "unknown direct identity origin");
            };
            match slot.origin {
                CreationOrigin::Singleton => require(evidence.slots.len() <= 1, "singleton proves no serial order")?,
                CreationOrigin::Create(pc) => {
                    require(matches!(f.code.get(pc as usize), Some(Instruction::Champ {
                        op: Opcode::OwnCreateCommit, .. })), "forged local creation witness")?;
                    require(actual == (pc, None), "creation witness belongs to another identity")?;
                    creation_pcs.push(pc);
                }
                CreationOrigin::FreshResult { pc, callee } => {
                    require(matches!(f.code.get(pc as usize), Some(Instruction::Champ {
                        op: Opcode::InvokeCallOwn, arguments, .. }) if arguments.contains(&Argument::Entry(callee))),
                        "fresh witness is not its owning call")?;
                    require(p.functions.get(callee as usize).and_then(|f| f.proofs.entry.as_ref())
                        .is_some_and(|entry| entry.fresh_singleton), "forged fresh-result witness")?;
                    require(actual == (pc, Some(callee)), "fresh-result witness is not on its successful value")?;
                    creation_pcs.push(pc);
                }
            }
            let suffix = &evidence.suffixes[index];
            let remaining = evidence.slots[index + 1..].iter().map(|slot| slot.identity).collect::<Vec<_>>();
            require(suffix.completed == index as u32 + 1 && suffix.remaining == remaining,
                "direct suffix repeats current or loses remainder")?;
            let roots = evidence.slots[index + 1..].iter().flat_map(|slot| [slot.identity, slot.descriptor])
                .collect::<Vec<_>>();
            require(suffix.roots == roots, "missing direct suffix roots")?;
        }
        let expected = evidence.slots.iter().map(|slot| reconstructed.identity(slot.identity))
            .collect::<Option<BTreeSet<_>>>();
        let Some(expected) = expected else {
            return evidence_rule(reconstructed, false, "SPB_STRUCTURE", "unknown direct slots");
        };
        let kept = reconstructed.inventory(source[0]) == Some(expected);
        evidence_rule(reconstructed, kept, "SPB_STRUCTURE", "direct plan loses owner inventory")?;
        require(creation_pcs.windows(2).all(|pcs| evidence::precedes(f, pcs[1], pcs[0])),
            "direct order is not descending creation on every CFG path")?;
        require(f.proofs.order_witnesses[plan.order_witness as usize].creation_pcs == creation_pcs,
            "order witness relocation mismatch")?;
        require(f.proofs.selections[plan.selection as usize].locations
            == evidence.slots.iter().map(|slot| slot.identity).collect::<Vec<_>>(), "direct inventory mismatch")?;
    }
    Ok(())
}

pub(super) fn check_fresh(f: &Function, evidence: &mut evidence::Evidence<'_>) -> Check {
    if !f.proofs.entry.as_ref().is_some_and(|entry| entry.fresh_singleton) {
        return Ok(());
    }
    let mut returns = 0;
    let reachable = reachable(f);
    for (pc, instruction) in f.code.iter().enumerate().filter(|(pc, _)| reachable.contains(pc)) {
        let _ = pc;
        let Instruction::Champ { op: Opcode::RetOwn, arguments, .. } = instruction else { continue };
        returns += 1;
        let locations = arguments.iter().filter_map(|argument| match *argument {
            Argument::Location(location) => Some(location), _ => None,
        }).collect::<Vec<_>>();
        require(locations.len() >= 2, "fresh singleton needs value and packet")?;
        let Some(key) = evidence.identity(locations[0]) else {
            return evidence_rule(evidence, false, "FRESH_ORIGIN",
                &format!("{}: fresh result has no successful creation", f.symbol));
        };
        let supported = evidence.inventory(locations[1]) == Some(BTreeSet::from([key]));
        evidence_rule(evidence, supported, "FRESH_PACKET",
            &format!("{}: fresh summary not supported by every reachable return packet", f.symbol))?;
    }
    enforce(returns != 0, "FRESH_REACHABLE_RETURN", "fresh summary has no reachable return evidence")
}

fn check_service_results(p: &Program, f: &Function) -> Check {
    for instruction in &f.code {
        for (edge, schema) in verify::checked_edges(p, f, instruction)? {
            for (index, ty) in schema.iter().enumerate() {
                if matches!(ty, ProducedType::Service(kind)
                    if !matches!(kind, ServiceKind::Closed | ServiceKind::Deps))
                {
                    require(edge.assignments.iter().filter(|assignment| {
                        assignment.source == Source::ProducedSlot(index as u32)
                    }).count() == 1, "outcome loses or duplicates linear result schema slot")?;
                }
            }
        }
    }
    Ok(())
}

/// A rule decided from evidence: when it does not hold because the evidence ran out of budget, the
/// budget is the reason reported, not the rule.
fn evidence_rule(evidence: &evidence::Evidence, holds: bool, rule: &'static str, message: &str) -> Check {
    if !holds {
        evidence.check_budget()?;
    }
    enforce(holds, rule, message)
}

fn check_call_contracts(p: &Program, f: &Function) -> Check {
    let mut reconstructed = evidence::Evidence::new(p, f);
    for instruction in &f.code {
        if let Instruction::Champ { op: Opcode::FutureNew, arguments, .. } = instruction {
            let callee = arguments.iter().find_map(|arg| match *arg {
                Argument::Entry(id) => p.functions.get(id as usize), _ => None,
            }).ok_or_else(|| error("fut.new has no entry"))?;
            if plain_inputs(p, callee) {
                let packet = arguments.iter().find_map(|arg| match arg {
                    Argument::Location(location) if verify::loc_type(f, *location)
                        == Ok(ProducedType::Service(ServiceKind::Packet)) => Some(*location),
                    _ => None,
                }).ok_or_else(|| violation("CALL_PLAIN_PACKET", "fut.new omits its input packet"))?;
                let empty = reconstructed.inventory(packet).is_some_and(|keys| keys.is_empty());
                evidence_rule(&reconstructed, empty, "CALL_PLAIN_PACKET",
                    &format!("{} -> {}: plain Future inputs cannot carry resource obligations", f.symbol,
                        callee.symbol))?;
            }
        }
        if let Instruction::Champ { op: Opcode::FuturePublish, arguments, .. } = instruction
            && let Some(packet) = arguments.iter().find_map(|arg| match arg {
                Argument::Location(location) if verify::loc_type(f, *location)
                    == Ok(ProducedType::Service(ServiceKind::Packet)) => Some(*location),
                _ => None,
            })
        {
            // A published success packet goes to the cache as a returned packet goes to the caller.
            if plain_results(p, f) {
                let empty = reconstructed.inventory(packet).is_some_and(|keys| keys.is_empty());
                evidence_rule(&reconstructed, empty, "RETURN_PLAIN_PACKET",
                    "a plain outcome cannot carry resource obligations")?;
            }
            let flat = reconstructed.flat_packet(packet);
            evidence_rule(&reconstructed, flat, "RETURN_PACKET_NOT_FLAT",
                "a published packet must have identical admission and full inventories")?;
        }
        if let Instruction::Champ { op: Opcode::RetOwn, arguments, .. } = instruction {
            let packet = arguments.iter().find_map(|arg| match arg {
                Argument::Location(location) if verify::loc_type(f, *location)
                    == Ok(ProducedType::Service(ServiceKind::Packet)) => Some(*location),
                _ => None,
            }).ok_or_else(|| violation("RETURN_PLAIN_PACKET", "plain return omits packet"))?;
            if plain_results(p, f) {
                let empty = reconstructed.inventory(packet).is_some_and(|keys| keys.is_empty());
                evidence_rule(&reconstructed, empty, "RETURN_PLAIN_PACKET",
                    "plain return cannot carry resource obligations")?;
            }
            let flat = reconstructed.flat_packet(packet);
            evidence_rule(&reconstructed, flat, "RETURN_PACKET_NOT_FLAT",
                "returned packet must have identical admission and full inventories")?;
        }
        if let Instruction::Champ { op: Opcode::InvokeDispatch | Opcode::InvokeDispatchOwn, arguments, .. }
            = instruction
        {
            check_dispatch_contract(p, f, arguments, &mut reconstructed)?;
            continue;
        }
        let Instruction::Champ {
            op: Opcode::InvokeCallOwn | Opcode::InvokeObserve | Opcode::InvokeDefer, arguments, ..
        } = instruction
            else { continue };
        let summary = arguments.iter().find_map(|arg| match *arg {
            Argument::Summary(id) => p.tables.summaries.get(id as usize), _ => None,
        }).ok_or_else(|| error("call has no summary"))?;
        let contract = p.functions[summary.entry as usize].proofs.entry.as_ref()
            .ok_or_else(|| error("callee has no verified entry contract"))?;
        require(summary.may_fail == (contract.failures != 0) && summary.retained_params == contract.retained_params,
            "call summary disagrees with verified entry contract")?;
        if p.functions[summary.entry as usize].entry_abi != EntryAbi::Observation
            && plain_inputs(p, &p.functions[summary.entry as usize])
        {
            let packet = arguments.iter().find_map(|arg| match arg {
                Argument::Location(location) if verify::loc_type(f, *location)
                    == Ok(ProducedType::Service(ServiceKind::Packet)) => Some(*location),
                _ => None,
            }).ok_or_else(|| violation("CALL_PLAIN_PACKET", "plain call omits packet"))?;
            let empty = reconstructed.inventory(packet).is_some_and(|keys| keys.is_empty());
            evidence_rule(&reconstructed, empty, "CALL_PLAIN_PACKET",
                &format!("{} -> {}: plain arguments cannot carry resource obligations",
                    f.symbol, p.functions[summary.entry as usize].symbol))?;
        }
    }
    reconstructed.check_budget()?;
    Ok(())
}

/// The implementation functions a dispatch can enter, in target order. Empty for every other operation.
pub(super) fn dispatch_callees<'a>(p: &'a Program, arguments: &[Argument]) -> Vec<&'a Function> {
    let Some(Argument::Dispatch(dispatch)) = arguments.first() else { return vec![] };
    dispatch.targets.iter().filter_map(|&target| {
        verify::implementation(p, target, dispatch.role, dispatch.slot).and_then(|id| p.functions.get(id as usize))
    }).collect()
}

/// Every implementation a dispatch can enter refines the site contract: it fails, and retains its
/// language parameters (receiver first), within the dispatch's bounds (`DISPATCH_TARGET`). A plain
/// call packet is required where every target has plain inputs, or where a plain target is external:
/// a target with code checks its packet on entry, an external one cannot.
fn check_dispatch_contract(p: &Program, f: &Function, arguments: &[Argument],
    reconstructed: &mut evidence::Evidence<'_>) -> Check
{
    let Some(Argument::Dispatch(dispatch)) = arguments.first() else {
        return Err(violation("DISPATCH_SITE", "missing dispatch"));
    };
    let callees = dispatch_callees(p, arguments);
    enforce(callees.len() == dispatch.targets.len(), "DISPATCH_TARGET", "dispatch target has no implementation")?;
    for callee in &callees {
        let contract = callee.proofs.entry.as_ref()
            .ok_or_else(|| violation("DISPATCH_TARGET", "implementation has no verified entry contract"))?;
        let language = callee.params.iter().enumerate().filter(|(_, location)| {
            matches!(verify::loc_type(callee, **location), Ok(ProducedType::Value(_)))
        }).map(|(index, _)| index as u32).collect::<Vec<_>>();
        let retained = contract.retained_params.iter().all(|param| language.iter().position(|index| index == param)
            .is_some_and(|position| dispatch.retained_params.contains(&(position as u32))));
        enforce(contract.failures & !dispatch.failures == 0 && retained, "DISPATCH_TARGET",
            &format!("{} -> {}: implementation fails or retains beyond the dispatch contract",
                f.symbol, callee.symbol))?;
    }
    let packet = arguments.iter().find_map(|arg| match arg {
        Argument::Location(location) if verify::loc_type(f, *location)
            == Ok(ProducedType::Service(ServiceKind::Packet)) => Some(*location),
        _ => None,
    });
    let plain = callees.iter().all(|callee| plain_inputs(p, callee))
        || callees.iter().any(|callee| callee.code.is_empty() && plain_inputs(p, callee));
    if let Some(packet) = packet
        && plain
    {
        enforce(reconstructed.inventory(packet).is_some_and(|keys| keys.is_empty()),
            "CALL_PLAIN_PACKET", &format!("{}: plain dispatch arguments cannot carry resource obligations", f.symbol))?;
    }
    Ok(())
}

pub(super) fn reachable(f: &Function) -> BTreeSet<usize> {
    let mut result = BTreeSet::new();
    let mut work = if f.code.is_empty() { vec![] } else { vec![0] };
    while let Some(pc) = work.pop() {
        if !result.insert(pc) { continue; }
        let inst = &f.code[pc];
        work.extend(allocation::outgoing(inst).iter().map(|edge| edge.target_pc as usize));
        if matches!(inst, Instruction::Data { .. } | Instruction::Champ { flow: Flow::Next(_), .. })
            && !matches!(inst, Instruction::Champ { op: Opcode::RetOwn | Opcode::RetDefer, .. })
            && pc + 1 < f.code.len()
        {
            work.push(pc + 1);
        }
    }
    result
}

/// A created object becomes an obligation only at its commit; until then no cleanup can destroy it.
/// So the creation of a resource-reaching object (has_destroy implies may_reach_resource) must reach
/// its own commit in straight-line code, without any other use of the value: nothing returns, stores,
/// passes or packs it, and no path leaves its scope, before the commit.
fn check_creations(p: &Program, f: &Function) -> Check {
    check_creations_with(p, f, &mut evidence::Evidence::new(p, f))
}

pub(super) fn check_creations_with(p: &Program, f: &Function, evidence: &mut evidence::Evidence<'_>) -> Check {
    for (pc, instruction) in f.code.iter().enumerate() {
        let Instruction::Data { op: DataOp::ObjNew | DataOp::ObjNext, output: Some(output), .. } = instruction
            else { continue };
        let (Type::Object(id) | Type::Successor(id)) = f.registers[*output as usize] else { continue };
        if !p.objects[id as usize].layout.may_reach_resource {
            continue;
        }
        let created = Location::Register(*output);
        let commit = (pc + 1..f.code.len()).find(|&next| matches!(&f.code[next],
            Instruction::Champ { op: Opcode::OwnCreateCommit, arguments, .. }
                if arguments.first() == Some(&Argument::Location(created))));
        let straight = commit.is_some_and(|commit| (pc + 1..commit).all(|between| {
            let falls_through = matches!(f.code[between], Instruction::Data { .. }
                | Instruction::Champ { flow: Flow::Next(_), .. })
                && !matches!(f.code[between], Instruction::Champ { op: Opcode::RetOwn | Opcode::RetDefer, .. });
            falls_through && !allocation::direct_uses(f, between).contains(&created)
        }));
        let committed = commit.map(|commit| (commit as u32, None));
        let message = format!("{}: object created at pc {pc} can escape before its own commit", f.symbol);
        enforce(straight, "CREATION_UNCOMMITTED", &message)?;
        let own = evidence.identity(created) == committed;
        evidence_rule(evidence, own, "CREATION_UNCOMMITTED", &message)?;
    }
    evidence.check_budget()
}

/// A fallible data operation has no outcome edge: after its failure the machine can only leave the
/// function, which would lose every live service. With service locations it must be an `evaluate`.
fn check_fallible_data(f: &Function) -> Check {
    if f.services.is_empty() {
        return Ok(());
    }
    let fallible = f.code.iter().any(|instruction| f.data_fallible(instruction));
    enforce(!fallible, "FALLIBLE_DATA_OUTCOMES",
        "a fallible data operation in a function with service locations needs explicit outcomes")
}

/// Failure views follow SSA, including handled Unwind edges. Only escaping raise or implicit scalar
/// failure contributes to the entry mask. Catching an Unwind does not itself add a failure.
fn check_failure_mask(p: &Program, f: &Function) -> Check {
    let reachable = reachable(f);
    let mut masks = BTreeMap::<Location, u32>::new();
    for &param in &f.params {
        if verify::loc_type(f, param)? == ProducedType::Value(Type::Failure) {
            masks.insert(param, 15);
        }
    }
    // D18 is checked against reconstructed inventories below. Cleanup failure states are bounded
    // by this function's verified close envelopes, including when unrelated module destroys fail.
    let destroy_mask = f.proofs.envelopes.iter().fold(0, |mask, envelope| mask | envelope.failures);
    loop {
        let mut changed = false;
        for &pc in &reachable {
            let inst = &f.code[pc];
            if let Instruction::Data { op: DataOp::Move | DataOp::FailSecondary | DataOp::FailCaused, inputs,
                output: Some(output), .. } = inst
            {
                let mask = inputs.iter().fold(0, |mask, &id| mask | masks.get(&Location::Register(id)).copied()
                    .unwrap_or(0));
                let entry = masks.entry(Location::Register(*output)).or_default();
                changed |= *entry | mask != *entry;
                *entry |= mask;
            }
            for (ordinal, edge) in allocation::outgoing(inst).iter().enumerate() {
                let produced = if let Instruction::Champ { op, arguments, flow: Flow::Outcomes(outcomes) } = inst {
                    match (op, outcomes[ordinal].tag) {
                        (Opcode::Fail, Tag::Unwind) => arguments.iter().find_map(|arg| match arg {
                            Argument::Label(kind) => Some(1 << kind), _ => None,
                        }).unwrap_or(15),
                        (Opcode::InvokeDispatch | Opcode::InvokeDispatchOwn, Tag::Unwind) => {
                            arguments.iter().find_map(|arg| match arg {
                                Argument::Dispatch(dispatch) => Some(dispatch.failures), _ => None,
                            }).unwrap_or(15)
                        }
                        (Opcode::InvokeCallOwn | Opcode::InvokeObserve | Opcode::InvokeDefer, Tag::Unwind) => {
                            arguments.iter()
                            .find_map(|arg| match arg {
                                Argument::Entry(id) => Some(p.functions[*id as usize].proofs.entry.as_ref()
                                    .map_or(15, |entry| entry.failures)), _ => None,
                            }).unwrap_or(15)
                        }
                        (Opcode::ExitDispatch, Tag::Failure) => f.proofs.envelopes.iter()
                            .fold(0, |mask, envelope| mask | envelope.failures)
                            | masks.values().fold(0, |mask, value| mask | value),
                        (Opcode::FailureDispatch, Tag::Some) => destroy_mask,
                        // An awaited Future fails with whatever its body published.
                        (Opcode::Await, Tag::Unwind) => 15,
                        (_, Tag::Unwind) => 2,
                        _ => 0,
                    }
                } else { 0 };
                for assignment in &edge.assignments {
                    if verify::loc_type(f, assignment.destination)? != ProducedType::Value(Type::Failure) {
                        continue;
                    }
                    let mask = match assignment.source {
                        Source::Location(source) => masks.get(&source).copied().unwrap_or(0),
                        Source::ProducedSlot(_) => produced,
                    };
                    let entry = masks.entry(assignment.destination).or_default();
                    changed |= *entry | mask != *entry;
                    *entry |= mask;
                }
            }
        }
        if !changed { break; }
    }
    let mut computed = 0;
    for pc in reachable {
        match &f.code[pc] {
            Instruction::Raise(id) => computed |= masks.get(&Location::Register(*id)).copied().unwrap_or(15),
            instruction if f.data_fallible(instruction) => computed |= 2,
            _ => {}
        }
    }
    let declared = f.proofs.entry.as_ref().map_or(0, |entry| entry.failures);
    enforce(computed & !declared == 0, "ENTRY_FAILURE_MASK",
        &format!("{}: body failure mask {computed} exceeds declared mask {declared}", f.symbol))
}
