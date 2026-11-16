//! Async bodies in SPB 17 (#601 V6, plain await): frame layouts, the entry and exit protocol, suspension
//! brackets and their continuations, and the flow of frame, task, receipt and cache tokens.
//! Champ draft 3 §15 and the OIR-2 async ABI D20/D21 define the rules; the loader checks the evidence a
//! producer supplies and never completes it.
use super::*;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

type Check = Result<(), Error>;

/// Frame-bound operations belong to async bodies; an async body takes exactly its frame, starts by resuming
/// continuation 0 and ends every path with `fut.publish` then `ret`; deferred cancellation only at its final
/// exit.
pub(crate) const ASYNC_ENTRY: &str = "SPB_ASYNC_ENTRY";
/// A linear token is live, saved or an operand at a suspension, never two of these; saved values equal the
/// layout's saved slots, and slot linearity follows the slot type.
pub(crate) const FRAME_LINEAR: &str = "SPB_FRAME_LINEAR";
/// Continuation 0 is the start; every other continuation has one site, and each successor of the site
/// starts with `frame.resume` of that continuation, reloading the frame its save produced.
pub(crate) const RESUME_TABLE: &str = "SPB_RESUME_TABLE";
/// Every suspending operation is bracketed by its continuation's `frame.save`; nothing else is.
pub(crate) const SUSPEND_BRACKET: &str = "SPB_SUSPEND_BRACKET";
/// The task and receipt of an await's unwind reach only one `await.unwind`, together. A spawned task's duty (D24.1)
/// is never one: it is saved across suspensions, settled by its owner's close, rehomed or absorbed, and never
/// reaches `await.unwind`; no location holds both kinds.
pub(crate) const TASK_RECEIPT: &str = "SPB_TASK_RECEIPT";
/// Each scheduled duty is settled once, in schedule order, by the scope that owns it; a spawned Future's task duty
/// before its cache duty (D24.2, ABI §6.3).
pub(crate) const SCHEDULE: &str = "SPB_SCHEDULE";
/// `fut.spawn` takes the cache duty its Future's `fut.new` just gave; between them the Future is only observed
/// (`deps.of`, `fut.done`, `fut.cancelled`, `fut.cancel`) and no edge enters that run (D24.1, `d24-spawn-not-fresh`).
pub(crate) const SPAWN: &str = "SPB_SPAWN";
/// Across a suspension the frame holds every value the body keeps: nothing but the operands of the step
/// lives past it outside the frame, and each slot holds one value of its declared type.
pub(crate) const ROOTS_SUSPEND: &str = "SPB_ROOTS_SUSPEND";

fn locations(arguments: &[Argument]) -> Vec<Location> {
    arguments.iter().filter_map(|argument| match argument {
        Argument::Location(location) => Some(*location),
        _ => None,
    }).collect()
}

pub(crate) fn continuation_of(arguments: &[Argument]) -> Option<u32> {
    arguments.iter().find_map(|argument| match argument {
        Argument::Continuation(id) => Some(*id),
        _ => None,
    })
}

/// The frame operation of `op` at `pc`, with its continuation, location operands and outputs.
fn frame_operation(f: &Function, pc: usize, op: Opcode) -> Option<(u32, Vec<Location>, &[Location])> {
    match f.code.get(pc)? {
        Instruction::Champ { op: found, arguments, flow: Flow::Next(outputs) } if *found == op => {
            Some((continuation_of(arguments)?, locations(arguments), outputs))
        }
        _ => None,
    }
}

fn suspending(p: &Program, f: &Function, pc: usize) -> bool {
    f.code.get(pc).is_some_and(|instruction| suspends(p, instruction))
}

/// A suspension site: an S-class operation, or `invoke.defer` of an async defer entry, which waits for its
/// shielded child (draft 3 §§11, 14.4: ID 29 is S for `AsyncDefer`).
pub(crate) fn suspends(p: &Program, instruction: &Instruction) -> bool {
    match instruction {
        Instruction::Champ { op: Opcode::InvokeDefer, arguments, .. } => arguments.iter().any(|argument| {
            matches!(argument, Argument::Entry(id)
                if matches!(p.functions.get(*id as usize).map(|f| &f.entry_abi), Some(EntryAbi::AsyncDefer { .. })))
        }),
        Instruction::Champ { op, .. } => op.suspends(),
        _ => false,
    }
}

fn kind(f: &Function, location: Location) -> Option<ServiceKind> {
    match location {
        Location::Service(id) => f.services.get(id as usize).copied(),
        Location::Register(_) => None,
    }
}

/// A fixed root (#667): the program's entry or one of its tests. Nothing calls it, so nothing offers it inputs.
pub(crate) fn fixed_root(p: &Program, function: u32) -> bool {
    p.entry == Some(function) || p.tests.iter().flatten().any(|test| test.function == function)
}

/// verify(): every frame layout is well formed and belongs to exactly one async body.
pub(crate) fn check_layouts(p: &Program) -> Check {
    let mut owners = vec![Vec::new(); p.tables.frames.len()];
    for (index, f) in p.functions.iter().enumerate() {
        if let EntryAbi::AsyncBody { layout } | EntryAbi::AsyncDefer { layout } = f.entry_abi {
            let owner = owners.get_mut(layout as usize).ok_or_else(|| error("frame layout index out of range"))?;
            owner.push(index as u32);
        }
    }
    enforce(owners.iter().all(|owners| owners.len() == 1), ASYNC_ENTRY,
        "a frame layout belongs to exactly one async body")?;
    for (layout, owners) in p.tables.frames.iter().zip(&owners) {
        for slot in &layout.slots {
            match slot.ty {
                ProducedType::Value(ty) => {
                    verify::ty_valid(ty, p)?;
                    enforce(!slot.linear, FRAME_LINEAR, "a value slot is not linear")?;
                }
                ProducedType::Service(kind) => {
                    enforce(kind != ServiceKind::Frame, FRAME_LINEAR, "a frame is never stored in a frame slot")?;
                    enforce(slot.linear == kind.linear(), FRAME_LINEAR, "slot linearity differs from its type")?;
                }
            }
        }
        let start = layout.continuations.first()
            .ok_or_else(|| violation(RESUME_TABLE, "a frame layout needs its start continuation"))?;
        enforce(start.site.is_none() && start.operands.is_empty(), RESUME_TABLE,
            "continuation 0 is the start: no site and no operands")?;
        let mut sites = BTreeSet::new();
        for continuation in &layout.continuations[1..] {
            let site = continuation.site.ok_or_else(|| violation(RESUME_TABLE, "a continuation has no site"))?;
            enforce(sites.insert(site), RESUME_TABLE, "two continuations share one site")?;
        }
        for continuation in &layout.continuations {
            let mut slots = BTreeSet::new();
            for &slot in continuation.saved.iter().chain(&continuation.operands) {
                require_index(&layout.slots, slot)?;
                enforce(slots.insert(slot), ROOTS_SUSPEND, "one frame slot holds two values of a suspension")?;
            }
            // A take capability waits in the frame only as an operand of its own take (draft 2 §7.3).
            enforce(continuation.saved.iter().all(|&slot| {
                layout.slots[slot as usize].ty != ProducedType::Service(ServiceKind::UniqueCache)
            }), take::TAKE_CAPABILITY, "a take capability crosses a suspension other than its take")?;
        }
        let inputs = layout.saved_types(0).ok_or_else(|| error("invalid start slots"))?;
        if fixed_root(p, owners[0]) {
            // The root driver writes nothing into continuation 0 (draft 3 §3).
            enforce(inputs.is_empty(), ASYNC_ENTRY, "a fixed root starts without inputs and without a packet")?;
            continue;
        }
        // The deps of borrowed inputs (a Future argument's, for one) are zero-word proof inputs after them.
        let values = inputs.iter().take_while(|ty| matches!(ty, ProducedType::Value(_))).count();
        let deps = inputs[values..].iter().take_while(|ty| **ty == ProducedType::Service(ServiceKind::Deps)).count();
        let packet = inputs.last() == Some(&ProducedType::Service(ServiceKind::Packet));
        enforce(packet && values + deps + 1 == inputs.len(), ASYNC_ENTRY,
            "the start holds the language inputs, their deps, then exactly one input packet")?;
    }
    Ok(())
}

fn require_index<T>(items: &[T], index: u32) -> Check {
    if (index as usize) < items.len() { Ok(()) } else { Err(error(format!("invalid frame slot index {index}"))) }
}

/// verify_function(), before anything that returns early: the entry shape of async bodies, and no frame
/// token anywhere else.
pub(crate) fn check_entry(f: &Function) -> Check {
    let asynchronous = f.entry_abi.asynchronous();
    if !asynchronous {
        return enforce(!f.services.contains(&ServiceKind::Frame), ASYNC_ENTRY,
            "a frame token outside an async body");
    }
    enforce(!f.proofs.entry.as_ref().is_some_and(|entry| entry.external.is_some()), ASYNC_ENTRY,
        "an async body has code; drivers are AsyncExternal")?;
    enforce(matches!(f.params.as_slice(), [location] if kind(f, *location) == Some(ServiceKind::Frame)),
        ASYNC_ENTRY, "an async body takes exactly its frame")
}

/// verify_function(), after SSA and outcome checks: the exit protocol, the brackets and the resume table.
pub(crate) fn check_structure(p: &Program, f: &Function) -> Check {
    let Some(layout) = p.frame_layout(f) else {
        // An async defer waits for its child, so only an async body runs it.
        return enforce(!f.code.iter().any(|instruction| suspends(p, instruction)), ASYNC_ENTRY,
            "an async defer entry runs only in an async body");
    };
    let starts = f.blocks.iter().map(|block| block.pc as usize).collect::<BTreeSet<_>>();
    let mut incoming = BTreeMap::<usize, usize>::new();
    for instruction in &f.code {
        for edge in allocation::outgoing(instruction) {
            *incoming.entry(edge.target_pc as usize).or_default() += 1;
        }
    }
    // The start: pc 0 resumes continuation 0 from the entry frame, and nothing else resumes it.
    let start = frame_operation(f, 0, Opcode::FrameResume);
    enforce(matches!(&start, Some((0, operands, _)) if operands.as_slice() == f.params.as_slice()),
        RESUME_TABLE, "an async body starts by resuming continuation 0 from its entry frame")?;
    let mut expected = BTreeMap::<usize, u32>::from([(0, 0)]);
    for (pc, instruction) in f.code.iter().enumerate() {
        let Instruction::Champ { op, arguments, flow } = instruction else {
            enforce(!matches!(instruction, Instruction::Raise(_)), ASYNC_ENTRY,
                "a failure leaves an async body through fut.publish, never raise")?;
            if matches!(instruction, Instruction::Ret(_)) {
                let published = pc > 0 && !starts.contains(&pc)
                    && matches!(f.code[pc - 1], Instruction::Champ { op: Opcode::FuturePublish, .. });
                enforce(published, ASYNC_ENTRY, "every path of an async body ends in fut.publish then ret")?;
            }
            continue;
        };
        if *op == Opcode::FuturePublish {
            enforce(matches!(f.code.get(pc + 1), Some(Instruction::Ret(_))), ASYNC_ENTRY,
                "fut.publish is followed by ret")?;
        }
        if *op == Opcode::FrameSave {
            let continuation = continuation_of(arguments).unwrap_or(0);
            let site = layout.continuations.get(continuation as usize).and_then(|continuation| continuation.site);
            enforce(site == Some(pc as u32 + 1) && suspending(p, f, pc + 1), SUSPEND_BRACKET,
                "frame.save brackets exactly the suspending site of its continuation")?;
        }
        if suspends(p, instruction) {
            let save = (pc > 0 && !starts.contains(&pc)).then(|| frame_operation(f, pc - 1, Opcode::FrameSave))
                .flatten();
            let Some((continuation, _, outputs)) = save else {
                return Err(violation(SUSPEND_BRACKET, format!("{} at pc {pc} has no frame.save", op.name())));
            };
            enforce(layout.continuations.get(continuation as usize)
                .is_some_and(|continuation| continuation.site == Some(pc as u32)), SUSPEND_BRACKET,
                "a suspending operation is bracketed by its own continuation")?;
            let operands = locations(arguments);
            let slots = &layout.continuations[continuation as usize].operands;
            let types = slots.iter().map(|&slot| layout.slots[slot as usize].ty).collect::<Vec<_>>();
            let actual = operands.iter().map(|&location| verify::loc_type(f, location))
                .collect::<Result<Vec<_>, _>>()?;
            enforce(types == actual, ROOTS_SUSPEND, "operand slots differ from the suspending operation's operands")?;
            let frame = outputs.first().copied();
            // Form (a): every outcome enters a block that only this site enters and that resumes first.
            // Form (b): the next instruction resumes.
            let resumes = match flow {
                Flow::Outcomes(outcomes) => outcomes.iter().map(|outcome| outcome.edge.target_pc as usize).collect(),
                Flow::Next(_) => vec![pc + 1],
            };
            for target in resumes {
                let alone = matches!(flow, Flow::Next(_)) || incoming.get(&target) == Some(&1);
                let resume = frame_operation(f, target, Opcode::FrameResume);
                let reloads = matches!(&resume, Some((k, operands, _))
                    if *k == continuation && operands.first().copied() == frame);
                enforce(alone && reloads, RESUME_TABLE,
                    "each successor of a suspension resumes its continuation from the saved frame first")?;
                expected.insert(target, continuation);
            }
        }
    }
    for (pc, instruction) in f.code.iter().enumerate() {
        if let Instruction::Champ { op: Opcode::FrameResume, arguments, .. } = instruction {
            enforce(expected.get(&pc).copied() == continuation_of(arguments), RESUME_TABLE,
                "frame.resume outside its continuation's successors")?;
        }
    }
    for continuation in &layout.continuations[1..] {
        let site = continuation.site.unwrap_or(u32::MAX) as usize;
        enforce(suspending(p, f, site), SUSPEND_BRACKET, "a continuation's site is not a suspending operation")?;
    }
    Ok(())
}

/// The `fut.new` that produced each cache token, through edges and through each continuation's save to its
/// resumes. `None` marks a token that two creations can reach. A `fut.spawn` passes its cache duty's origin to
/// both its outputs: the cache duty it gives back and the task duty of the same Future (D24.1), so a task-state
/// location has an origin exactly when a spawn can reach it.
pub(crate) fn cache_origins(p: &Program, f: &Function) -> BTreeMap<Location, Option<u32>> {
    let tracked = |location: Location| matches!(kind(f, location),
        Some(ServiceKind::CacheState | ServiceKind::TaskState));
    let mut flows = BTreeMap::<Location, Vec<Location>>::new();
    let mut seeds = Vec::new();
    let layout = p.frame_layout(f);
    for (pc, instruction) in f.code.iter().enumerate() {
        for edge in allocation::outgoing(instruction) {
            for assignment in &edge.assignments {
                if let Source::Location(source) = assignment.source
                    && tracked(assignment.destination)
                {
                    flows.entry(source).or_default().push(assignment.destination);
                }
            }
        }
        let Instruction::Champ { op, arguments, flow: Flow::Next(outputs) } = instruction else { continue };
        if *op == Opcode::FutureNew && let Some(&cache) = outputs.get(1) {
            seeds.push((cache, pc as u32));
        }
        if *op == Opcode::FutureSpawn && let Some(&cache) = locations(arguments).get(1) {
            flows.entry(cache).or_default().extend(outputs.iter().copied().filter(|&output| tracked(output)));
        }
        if *op == Opcode::FrameResume
            && let Some(continuation) = continuation_of(arguments)
            && let Some(site) = layout.and_then(|layout| layout.continuations.get(continuation as usize))
                .and_then(|continuation| continuation.site)
            && let Some((_, saved, _)) = (site as usize).checked_sub(1)
                .and_then(|save| frame_operation(f, save, Opcode::FrameSave))
        {
            for (&source, &destination) in saved.iter().skip(1).zip(outputs.iter().skip(1)) {
                if tracked(destination) {
                    flows.entry(source).or_default().push(destination);
                }
            }
        }
    }
    let mut origin = BTreeMap::<Location, Option<u32>>::new();
    let mut work = VecDeque::new();
    for (location, pc) in seeds {
        if merge(&mut origin, location, Some(pc)) {
            work.push_back(location);
        }
    }
    // Each location changes at most twice (unknown, one producer, two producers), so the work is linear.
    while let Some(location) = work.pop_front() {
        let value = origin[&location];
        for &destination in flows.get(&location).into_iter().flatten() {
            if merge(&mut origin, destination, value) {
                work.push_back(destination);
            }
        }
    }
    origin
}

/// The task-state locations an await's unwind outcome can reach through edges: the waiter's handles (C-3).
fn await_handles(f: &Function) -> BTreeSet<Location> {
    let task = |location: Location| kind(f, location) == Some(ServiceKind::TaskState);
    let mut flows = BTreeMap::<Location, Vec<Location>>::new();
    let mut handles = BTreeSet::new();
    for instruction in &f.code {
        let produced = matches!(instruction, Instruction::Champ { op: Opcode::Await | Opcode::AwaitTake, .. });
        for edge in allocation::outgoing(instruction) {
            for assignment in edge.assignments.iter().filter(|assignment| task(assignment.destination)) {
                match assignment.source {
                    Source::ProducedSlot(_) if produced => {
                        handles.insert(assignment.destination);
                    }
                    Source::Location(source) => flows.entry(source).or_default().push(assignment.destination),
                    Source::ProducedSlot(_) => {}
                }
            }
        }
    }
    let mut work: Vec<_> = handles.iter().copied().collect();
    while let Some(location) = work.pop() {
        for &destination in flows.get(&location).into_iter().flatten() {
            if handles.insert(destination) {
                work.push(destination);
            }
        }
    }
    handles
}

/// `SPB_SPAWN` (D24.1): each `fut.spawn` takes the Future and the cache duty that a `fut.new` earlier in the same
/// run of straight-line code produced. Between them the Future is only observed, and no edge enters the run, so
/// the Future is not awaited, stored or offered before its task starts.
fn check_spawns(f: &Function) -> Check {
    let targets: BTreeSet<usize> = f.code.iter().flat_map(allocation::outgoing)
        .map(|edge| edge.target_pc as usize).collect();
    for (pc, instruction) in f.code.iter().enumerate() {
        let Instruction::Champ { op: Opcode::FutureSpawn, arguments, .. } = instruction else { continue };
        let operands = locations(arguments);
        let (Some(&future), Some(&cache)) = (operands.first(), operands.get(1)) else {
            return Err(violation(SPAWN, "fut.spawn takes a Future and its cache duty"));
        };
        let mut fresh = false;
        for earlier in (0..pc).rev() {
            // An edge into the run between the creation and the spawn.
            if targets.contains(&(earlier + 1)) {
                break;
            }
            let instruction = &f.code[earlier];
            if let Instruction::Champ { op: Opcode::FutureNew, flow: Flow::Next(outputs), .. } = instruction
                && outputs.first() == Some(&future)
            {
                fresh = outputs.get(1) == Some(&cache);
                break;
            }
            let straight = matches!(instruction,
                Instruction::Data { .. } | Instruction::Champ { flow: Flow::Next(_), .. });
            let observes = matches!(instruction, Instruction::Champ { op: Opcode::DepsOf | Opcode::FutureDone
                | Opcode::FutureCancelled | Opcode::FutureCancel, .. });
            let reads = allocation::direct_uses(f, earlier).contains(&future) && !observes;
            let defines = allocation::definitions(instruction).iter()
                .any(|&location| location == future || location == cache);
            if !straight || reads || defines {
                break;
            }
        }
        enforce(fresh, SPAWN, "fut.spawn takes the Future and the cache duty its fut.new just gave (D24.1)")?;
    }
    Ok(())
}

/// Join a producer into a token's lineage; whether the lineage changed.
fn merge(origin: &mut BTreeMap<Location, Option<u32>>, location: Location, value: Option<u32>) -> bool {
    let next = match origin.get(&location) {
        None => value,
        Some(previous) if *previous == value => return false,
        Some(_) => None,
    };
    let changed = origin.get(&location) != Some(&next);
    origin.insert(location, next);
    changed
}

/// verify_load(): where tasks and receipts go, and what lives across each suspension.
pub(crate) fn check_flow(p: &Program, f: &Function) -> Check {
    // Only await.unwind consumes them; edges may forward them. Nothing else may read, save or settle them.
    // Since no token lives across a suspension outside the frame, the pair it takes on any path belongs to
    // the one await of that path. A spawned task's duty is a task-state too, told apart by its lineage (D24.1).
    let origins = cache_origins(p, f);
    let handles = await_handles(f);
    for location in &handles {
        enforce(!origins.contains_key(location), TASK_RECEIPT,
            "one location holds an await's task and a spawned task's duty")?;
    }
    for instruction in &f.code {
        let Instruction::Champ { op, arguments, .. } = instruction else { continue };
        for (index, location) in locations(arguments).into_iter().enumerate() {
            let spawned = kind(f, location) == Some(ServiceKind::TaskState) && origins.contains_key(&location);
            if spawned {
                // The duty is saved across suspensions, settled by its owner's close, or leaves with its cache
                // duty through a rehome or an absorb (D24.2, C-4, D25.2).
                let duty = match op {
                    Opcode::FrameSave | Opcode::ServiceAbsorb => index > 0,
                    Opcode::CleanupSettle => index == 1,
                    Opcode::CacheRehome => index == 2,
                    _ => false,
                };
                enforce(duty, TASK_RECEIPT, &format!("a spawned task's duty reaches {}", op.name()))?;
            } else if matches!(kind(f, location), Some(ServiceKind::TaskState | ServiceKind::Receipt)) {
                enforce(*op == Opcode::AwaitUnwind && matches!(index, 2 | 3), TASK_RECEIPT,
                    &format!("an await's task or receipt reaches {}", op.name()))?;
            }
        }
    }
    check_spawns(f)?;
    if let Some(layout) = p.frame_layout(f) {
        // Uses only: proof maps and dependency edges never keep a token alive past its frame.save.
        let mut bare = f.clone();
        bare.proofs.roots.clear();
        bare.proofs.post_roots.clear();
        bare.proofs.states.clear();
        bare.proofs.dependencies.clear();
        let live = live_locations(&bare)?;
        for continuation in layout.continuations.iter().skip(1) {
            let site = continuation.site.unwrap_or(0) as usize;
            let save = site.checked_sub(1).and_then(|save| frame_operation(f, save, Opcode::FrameSave));
            let Some((_, saved, frame)) = save else {
                return Err(violation(SUSPEND_BRACKET, "a continuation without its frame.save"));
            };
            let Some(Instruction::Champ { arguments, .. }) = f.code.get(site) else {
                return Err(violation(SUSPEND_BRACKET, "a continuation's site is not a suspending operation"));
            };
            let operands = locations(arguments);
            let values = &saved[1..];
            let mut seen = BTreeSet::new();
            for value in values {
                enforce(seen.insert(*value) && !operands.contains(value), FRAME_LINEAR,
                    "a token is saved twice, or saved and an operand of the step")?;
            }
            for location in &live[site] {
                if frame.contains(location) || operands.contains(location) {
                    continue;
                }
                if kind(f, *location).is_some_and(ServiceKind::linear) {
                    return Err(violation(FRAME_LINEAR, format!(
                        "linear {location:?} lives across the suspension at pc {site} outside the frame")));
                }
                return Err(violation(ROOTS_SUSPEND, format!(
                    "{location:?} lives across the suspension at pc {site} outside the frame")));
            }
        }
    }
    Ok(())
}
