//! Service duties inside packets in SPB 17 (async ABI D25, Champ draft 3 §§6, 15).
//!
//! Two forward analyses over concrete SPB edges, frame saves and resumes:
//! - **May admit** (D25.3): which closes take a scope state that may have admitted a service entry. A packet may
//!   carry entries when it is the result packet of a call or take whose language result can reach a service holder
//!   (D25.1: the result relation names the fresh holder's duty), the packet of an absorb, of an opaque admission
//!   (`own.dynamic`), or of an exit that carries a rehomed duty (C-4), or when such a packet flows into it. A split
//!   never selects an entry (C-4 ruling: the one escape mechanism is `cache.rehome`). Such a close needs exactly one
//!   `cleanup.settle.dynamic` before its first plan (`SPB_SCHEDULE`).
//! - **Holds** (D25.2): which entries a packet surely holds, by the Future value they name. An entry is known from
//!   an absorb, from a rehome into a reserve that an exit commits, and from a call whose callee is proved, by the
//!   same analysis of its returns, to return its Future's entry with its result. `service.materialize` takes only
//!   an entry its packet surely holds (`SPB_MATERIALIZE`, fixture `d25-materialize-absent`).
use super::*;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

type Check = Result<(), Error>;

/// `service.materialize` names an entry its packet surely holds (ABI D25.2, `d25-materialize-absent`).
pub(crate) const MATERIALIZE: &str = "SPB_MATERIALIZE";

/// Analysis steps one function may take before the loader refuses it.
const STEP_BUDGET: usize = 4_000_000;

/// Whether a value of `ty` can reach a service holder (a Future), through object fields, variant cases, error
/// cases, array and map elements, role implementations and any boxes. Unknown declarations can.
pub(crate) fn reaches_holder(p: &Program, ty: Type) -> bool {
    let mut pending = vec![ty];
    let mut seen = std::collections::HashSet::new();
    while let Some(ty) = pending.pop() {
        if !seen.insert(ty) {
            continue;
        }
        match ty {
            Type::Future(_) => return true,
            Type::Object(id) | Type::Successor(id) | Type::Variant(id) => {
                let Some(object) = p.objects.get(id as usize) else { return true };
                pending.extend(&object.layout.storage_fields);
                pending.extend(object.variant_cases.iter().map(|&case| Type::Object(case)));
            }
            Type::Error => {
                pending.extend(p.objects.iter().enumerate().filter(|(_, object)| !object.error_cases.is_empty())
                    .map(|(id, _)| Type::Object(id as u32)));
            }
            Type::Role(id) | Type::RoleSuccessor(id) => {
                if verify::role_decl(p, id).is_err() {
                    return true;
                }
                pending.extend(verify::implementors(p, id).map(|(object, _)| Type::Object(object)));
            }
            Type::Any => {
                pending.extend(p.objects.iter().enumerate().filter(|(_, object)| object.any_box)
                    .map(|(id, _)| Type::Object(id as u32)));
            }
            Type::Array(shape) if shape.depth > 0 => pending.extend(ArrayType { depth: 1, ..shape }.element()),
            Type::Map(id) | Type::MapBuilder(id) | Type::Builder(id) => match p.composites.get(id as usize) {
                Some(Composite::Map { key, value }) => pending.extend([*key, *value]),
                Some(Composite::Builder { array }) => pending.push(*array),
                None => return true,
            },
            _ => {}
        }
    }
    false
}

fn locations(arguments: &[Argument]) -> Vec<Location> {
    arguments.iter().filter_map(|argument| match argument {
        Argument::Location(location) => Some(*location),
        _ => None,
    }).collect()
}

fn kind(f: &Function, location: Location) -> Option<ServiceKind> {
    match location {
        Location::Service(id) => f.services.get(id as usize).copied(),
        Location::Register(_) => None,
    }
}

fn future(f: &Function, location: Location) -> bool {
    matches!(location, Location::Register(id) if matches!(f.registers.get(id as usize), Some(Type::Future(_))))
}

/// The pcs of each continuation's resumes, so that a grown save revisits them.
fn resumes(f: &Function) -> BTreeMap<u32, Vec<usize>> {
    let mut resumes = BTreeMap::<u32, Vec<usize>>::new();
    for (pc, instruction) in f.code.iter().enumerate() {
        if let Instruction::Champ { op: Opcode::FrameResume, arguments, .. } = instruction
            && let Some(k) = suspension::continuation_of(arguments)
        {
            resumes.entry(k).or_default().push(pc);
        }
    }
    resumes
}

/// The produced slots of one outcome of a terminator.
fn schema(p: &Program, f: &Function, op: Opcode, arguments: &[Argument], tag: Tag) -> Result<Vec<ProducedType>, Error> {
    let context = verify::result_context(p, f, op, arguments)?;
    op.results(tag, &context)
}

/// One step of a forward analysis: the successors of `pc` with their states, and pcs to visit again.
type Step<S> = Result<(Vec<(usize, S)>, Vec<usize>), Error>;

/// A forward worklist to a fixed point. `join` merges a state into a successor's and says whether it changed.
fn forward<S: Clone>(f: &Function, start: S, join: impl Fn(&mut S, &S) -> bool,
    mut step: impl FnMut(usize, &S) -> Step<S>) -> Result<Vec<Option<S>>, Error>
{
    let mut incoming: Vec<Option<S>> = vec![None; f.code.len()];
    if f.code.is_empty() {
        return Ok(incoming);
    }
    incoming[0] = Some(start);
    let mut work = VecDeque::from([0]);
    let mut queued = BTreeSet::from([0]);
    let mut steps = 0usize;
    while let Some(pc) = work.pop_front() {
        queued.remove(&pc);
        steps += 1;
        enforce(steps <= STEP_BUDGET, "LOAD_WORK_BUDGET", "service-entry analysis exceeds its work budget")?;
        let state = incoming[pc].clone().ok_or_else(|| error("service-entry worklist"))?;
        let (successors, revisit) = step(pc, &state)?;
        for (target, next) in successors {
            let slot = incoming.get_mut(target).ok_or_else(|| error("service-entry target outside code"))?;
            let changed = match slot {
                Some(previous) => join(previous, &next),
                None => {
                    *slot = Some(next);
                    true
                }
            };
            if changed && queued.insert(target) {
                work.push_back(target);
            }
        }
        for target in revisit {
            if incoming.get(target).is_some_and(Option::is_some) && queued.insert(target) {
                work.push_back(target);
            }
        }
    }
    Ok(incoming)
}

// --- May admit (D25.3) ----------------------------------------------------------------------------------------

/// The locations that may carry service entries: packets, reserves and the cleanup and exit states that continue
/// a reserve; scope states that may have admitted an entry.
type Carriers = BTreeSet<Location>;

fn set(state: &mut Carriers, location: Location, carries: bool) {
    if carries {
        state.insert(location);
    } else {
        state.remove(&location);
    }
}

/// What the outputs of a `Next` instruction carry, from what its operands carry.
fn next_carriers(f: &Function, op: Opcode, arguments: &[Argument], ins: &[bool], outputs: &[Location])
    -> Vec<bool>
{
    use Opcode::*;
    let any = ins.iter().any(|carries| *carries);
    let first = ins.first().copied().unwrap_or(false);
    let last = ins.last().copied().unwrap_or(false);
    let operands = locations(arguments);
    match op {
        ScopeBegin | OwnEmpty | OwnCreateCommit | DepsOf | DepsEmpty | UniqueCacheProof => vec![false; outputs.len()],
        // An admission makes the state carry what the admitted packet carries.
        OwnAdmit | OwnAccept | OwnBind | OwnRebind => vec![first || last],
        OwnStage => outputs.iter().map(|_| any).collect(),
        OwnMove | OwnJoin => vec![any],
        // The selection never takes an entry: the rest keeps them.
        OwnSplit => vec![first, false, false],
        ExitPrepare => vec![last],
        ExitExtend => vec![any],
        // The replacement reserve starts from its own packet; the replaced reserve's content comes back.
        ExitReplace => vec![last, first],
        CacheRehome | ServiceAbsorb => vec![true],
        ServiceMaterialize => vec![first, false],
        // The cleanup state continues the reserve; the scope's own entries settle at the close, never escape.
        CleanupBegin => {
            let reserve = operands.iter().zip(ins).skip(1)
                .any(|(&location, &carries)| carries && kind(f, location) == Some(ServiceKind::Pending));
            vec![reserve]
        }
        CleanupSettle | CleanupSettleDynamic | CleanupPlan | CleanupFailure | CleanupDeliverCancel => vec![first],
        // The aborted reserve's entries settle in this close (ABI C-4 step 3).
        ExitAbort => vec![false],
        DestroyDirect | CleanupDestroy => vec![first, false],
        CleanupFinish => vec![false, false, first],
        ExitCommit => outputs.iter().map(|&location| kind(f, location) == Some(ServiceKind::Packet) && first)
            .collect(),
        // The owner state keeps what it carried; the inputs packet goes into the Future.
        FutureNew => vec![false, false, last],
        AwaitUnwind => vec![first, false],
        _ => outputs.iter().map(|_| any).collect(),
    }
}

/// What the produced slots of one outcome carry.
fn produced_carriers(p: &Program, op: Opcode, tag: Tag, schema: &[ProducedType], ins: &[bool]) -> Vec<bool> {
    use Opcode::*;
    let first = ins.first().copied().unwrap_or(false);
    let any = ins.iter().any(|carries| *carries);
    // D25.1: a result packet carries the entries of the holders its language result can reach.
    let result = schema.iter().any(|slot| matches!(slot, ProducedType::Value(ty) if reaches_holder(p, *ty)));
    let packets = |carries: bool| -> Vec<bool> {
        schema.iter().map(|slot| *slot == ProducedType::Service(ServiceKind::Packet) && carries).collect()
    };
    match (op, tag) {
        (InvokeCallOwn | InvokeDispatchOwn | AwaitTake, Tag::Normal) | (InvokeDefer, Tag::Replace(_)) => {
            packets(result)
        }
        // A selection never takes an entry; a refused check returns the packet unchanged.
        (OwnSplitChecked, Tag::Normal) => vec![first, false, false],
        (OwnSplitChecked, Tag::Unwind) => vec![first, false],
        (ScopeSplit, Tag::Normal) => vec![first, false, false],
        (ScopeSplit, Tag::Unwind) => vec![first, false],
        // An opaque packet (D25.3): its admission may add entries; a refused one comes back.
        (OwnDynamic, Tag::Normal) => vec![true],
        (OwnDynamic, Tag::Unwind) => vec![first, true, false],
        (ScopeTransfer, _) => schema.iter().map(|slot| *slot == ProducedType::Service(ServiceKind::ScopeState)
            && any).collect(),
        (CleanupNext, _) => schema.iter().enumerate().map(|(index, _)| index == 0 && first).collect(),
        (ExitDispatch, Tag::Pending) => vec![false, ins.get(2).copied().unwrap_or(false)],
        (BuilderPush | BuilderAppend | BuilderRange | BuilderFinish | BuilderAbandon, _) => packets(any),
        _ => vec![false; schema.len()],
    }
}

/// The begin pcs of closes whose scope state may have admitted a service entry on some path (D25.3).
pub(crate) fn admitting_closes(p: &Program, f: &Function) -> Result<BTreeSet<u32>, Error> {
    let resumes = resumes(f);
    let mut saved = BTreeMap::<u32, Vec<bool>>::new();
    let mut closes = BTreeSet::new();
    let join = |previous: &mut Carriers, next: &Carriers| {
        let before = previous.len();
        previous.extend(next.iter().copied());
        previous.len() != before
    };
    forward(f, Carriers::new(), join, |pc, state| {
        let mut out = state.clone();
        let mut successors = Vec::new();
        let mut revisit = Vec::new();
        match &f.code[pc] {
            Instruction::Data { .. } => successors.push((pc + 1, out)),
            Instruction::Br(edge) => successors.push((edge.target_pc as usize, assign(state, edge, &[]))),
            Instruction::Cond { yes, no, .. } => {
                successors.push((yes.target_pc as usize, assign(state, yes, &[])));
                successors.push((no.target_pc as usize, assign(state, no, &[])));
            }
            Instruction::Ret(_) | Instruction::Raise(_) | Instruction::Unreachable => {}
            Instruction::Champ { op, arguments, flow } => {
                let operands = locations(arguments);
                let ins = operands.iter().map(|location| state.contains(location)).collect::<Vec<_>>();
                if *op == Opcode::CleanupBegin && ins.first() == Some(&true)
                    && kind(f, operands[0]) == Some(ServiceKind::ScopeState)
                {
                    closes.insert(pc as u32);
                }
                match flow {
                    Flow::Next(outputs) => {
                        let facts = match op {
                            Opcode::FrameSave => {
                                let k = suspension::continuation_of(arguments).unwrap_or(0);
                                let entry = saved.entry(k).or_insert_with(|| vec![false; ins.len() - 1]);
                                let mut grew = false;
                                for (slot, &carries) in entry.iter_mut().zip(&ins[1..]) {
                                    grew |= carries && !*slot;
                                    *slot |= carries;
                                }
                                if grew {
                                    revisit.extend(resumes.get(&k).into_iter().flatten().copied());
                                }
                                vec![false]
                            }
                            Opcode::FrameResume => {
                                let k = suspension::continuation_of(arguments).unwrap_or(0);
                                let mut facts = vec![false];
                                let stored = saved.get(&k).cloned().unwrap_or_default();
                                facts.extend((1..outputs.len()).map(|index| {
                                    stored.get(index - 1).copied().unwrap_or(false)
                                }));
                                facts
                            }
                            _ => next_carriers(f, *op, arguments, &ins, outputs),
                        };
                        for (&location, carries) in outputs.iter().zip(facts) {
                            set(&mut out, location, carries);
                        }
                        if !matches!(op, Opcode::RetOwn | Opcode::RetDefer) {
                            successors.push((pc + 1, out));
                        }
                    }
                    Flow::Outcomes(outcomes) => {
                        for outcome in outcomes {
                            let slots = schema(p, f, *op, arguments, outcome.tag)?;
                            let produced = produced_carriers(p, *op, outcome.tag, &slots, &ins);
                            successors.push((outcome.edge.target_pc as usize, assign(state, &outcome.edge,
                                &produced)));
                        }
                    }
                }
            }
        }
        Ok((successors, revisit))
    })?;
    Ok(closes)
}

/// The state after an edge's parallel assignments.
fn assign(state: &Carriers, edge: &Edge, produced: &[bool]) -> Carriers {
    let mut out = state.clone();
    for assignment in &edge.assignments {
        let carries = match assignment.source {
            Source::Location(source) => state.contains(&source),
            Source::ProducedSlot(slot) => produced.get(slot as usize).copied().unwrap_or(false),
        };
        set(&mut out, assignment.destination, carries);
    }
    out
}

// --- Holds (D25.2) --------------------------------------------------------------------------------------------

/// A Future value, by its definition: the pc, the outcome ordinal (`u32::MAX` for a next flow) and the slot.
type Val = (u32, u32, u32);

/// What is surely known at a pc.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Held {
    /// Future registers and the value they hold.
    futures: BTreeMap<Location, Val>,
    /// Cache-state tokens and the Future whose duty they are.
    tokens: BTreeMap<Location, Val>,
    /// Packets, reserves and the cleanup and exit states that continue a reserve: the entries they surely hold.
    entries: BTreeMap<Location, BTreeSet<Val>>,
    /// Reserves and their continuations: the Future value the exit returns.
    exits: BTreeMap<Location, Val>,
}

/// One location's facts, as a frame slot or an edge source carries them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Slot {
    future: Option<Val>,
    token: Option<Val>,
    entries: BTreeSet<Val>,
    exit: Option<Val>,
}

impl Held {
    fn read(&self, location: Location) -> Slot {
        Slot {
            future: self.futures.get(&location).copied(),
            token: self.tokens.get(&location).copied(),
            entries: self.entries.get(&location).cloned().unwrap_or_default(),
            exit: self.exits.get(&location).copied(),
        }
    }

    fn write(&mut self, location: Location, slot: Slot) {
        self.kill(location);
        if let Some(value) = slot.future {
            self.futures.insert(location, value);
        }
        if let Some(value) = slot.token {
            self.tokens.insert(location, value);
        }
        if !slot.entries.is_empty() {
            self.entries.insert(location, slot.entries);
        }
        if let Some(value) = slot.exit {
            self.exits.insert(location, value);
        }
    }

    fn kill(&mut self, location: Location) {
        self.futures.remove(&location);
        self.tokens.remove(&location);
        self.entries.remove(&location);
        self.exits.remove(&location);
    }

    /// Keep what both states know; whether `self` changed.
    fn meet(&mut self, other: &Held) -> bool {
        let before = self.clone();
        self.futures.retain(|location, value| other.futures.get(location) == Some(value));
        self.tokens.retain(|location, value| other.tokens.get(location) == Some(value));
        self.exits.retain(|location, value| other.exits.get(location) == Some(value));
        self.entries.retain(|location, entries| {
            let Some(theirs) = other.entries.get(location) else { return false };
            entries.retain(|value| theirs.contains(value));
            !entries.is_empty()
        });
        *self != before
    }
}

fn meet_slot(slot: &mut Slot, other: &Slot) -> bool {
    let before = slot.clone();
    if slot.future != other.future {
        slot.future = None;
    }
    if slot.token != other.token {
        slot.token = None;
    }
    if slot.exit != other.exit {
        slot.exit = None;
    }
    slot.entries.retain(|value| other.entries.contains(value));
    *slot != before
}

/// The facts of the outputs of a `Next` instruction at `pc`. When `check`, `Err` names a materialize whose entry is
/// not held.
fn next_held(f: &Function, pc: usize, op: Opcode, arguments: &[Argument], state: &Held, outputs: &[Location],
    check: bool) -> Result<Vec<Slot>, Error>
{
    use Opcode::*;
    let operands = locations(arguments);
    let read = |index: usize| operands.get(index).map(|&location| state.read(location)).unwrap_or_default();
    let mut facts = vec![Slot::default(); outputs.len()];
    let union = |indices: &mut dyn Iterator<Item = usize>| -> BTreeSet<Val> {
        indices.flat_map(|index| read(index).entries).collect()
    };
    match op {
        FutureNew => {
            let value = (pc as u32, u32::MAX, 0);
            facts[0].future = Some(value);
            facts[1].token = Some(value);
        }
        OwnMove => facts[0].entries = read(0).entries,
        OwnJoin => facts[0].entries = union(&mut (0..operands.len())),
        OwnSplit => facts[0].entries = read(0).entries,
        ServiceAbsorb => {
            let mut entries = read(0).entries;
            entries.extend(read(1).token);
            facts[0].entries = entries;
        }
        ServiceMaterialize => {
            let holder = read(1).future;
            let mut entries = read(0).entries;
            let held = holder.is_some_and(|value| entries.remove(&value));
            enforce(held || !check, MATERIALIZE, &format!(
                "service.materialize at pc {pc} names an entry its packet does not surely hold"))?;
            facts[0].entries = entries;
            facts[1].token = holder;
        }
        ExitPrepare => {
            let packet = operands.len() - 1;
            facts[0].entries = read(packet).entries;
            facts[0].exit = (operands.len() == 2).then(|| read(0).future).flatten();
        }
        ExitExtend => {
            facts[0].entries = union(&mut (0..2));
            facts[0].exit = read(0).exit;
        }
        CacheRehome => {
            let mut entries = read(0).entries;
            entries.extend(read(1).token);
            facts[0].entries = entries;
            facts[0].exit = read(0).exit;
        }
        CleanupBegin => {
            if let Some(index) = operands.iter().position(|&location| kind(f, location) == Some(ServiceKind::Pending)) {
                facts[0].entries = read(index).entries;
                facts[0].exit = read(index).exit;
            }
        }
        CleanupSettle | CleanupSettleDynamic | CleanupPlan | CleanupFailure | CleanupDeliverCancel
            | DestroyDirect | CleanupDestroy => {
            facts[0].entries = read(0).entries;
            facts[0].exit = read(0).exit;
        }
        CleanupFinish => {
            facts[2].entries = read(0).entries;
            facts[2].exit = read(0).exit;
        }
        ExitCommit => {
            let pending = read(0);
            for (index, &location) in outputs.iter().enumerate() {
                if kind(f, location) == Some(ServiceKind::Packet) {
                    facts[index].entries = pending.entries.clone();
                } else if future(f, location) {
                    facts[index].future = pending.exit;
                }
            }
        }
        _ => {}
    }
    Ok(facts)
}

/// The facts of the produced slots of one outcome. `fresh` says which functions return their Future's entry.
#[allow(clippy::too_many_arguments)]
fn produced_held(p: &Program, pc: usize, ordinal: usize, op: Opcode, arguments: &[Argument], tag: Tag,
    schema: &[ProducedType], state: &Held, fresh: &[bool]) -> Vec<Slot>
{
    use Opcode::*;
    let operands = locations(arguments);
    let read = |index: usize| operands.get(index).map(|&location| state.read(location)).unwrap_or_default();
    let mut facts = vec![Slot::default(); schema.len()];
    let returns_entry = match op {
        InvokeCallOwn => arguments.iter().find_map(|argument| match argument {
            Argument::Entry(id) => Some(fresh.get(*id as usize).copied().unwrap_or(false)),
            _ => None,
        }).unwrap_or(false),
        InvokeDispatchOwn => {
            let callees = load::dispatch_callees(p, arguments);
            !callees.is_empty() && callees.iter().all(|callee| {
                p.functions.iter().position(|function| std::ptr::eq(function, *callee))
                    .is_some_and(|index| fresh.get(index).copied().unwrap_or(false))
            })
        }
        _ => false,
    };
    match (op, tag) {
        (InvokeCallOwn | InvokeDispatchOwn, Tag::Normal) if returns_entry => {
            if let (Some(ProducedType::Value(Type::Future(_))), Some(packet)) = (schema.first(),
                schema.iter().position(|slot| *slot == ProducedType::Service(ServiceKind::Packet)))
            {
                let value = (pc as u32, ordinal as u32, 0);
                facts[0].future = Some(value);
                facts[packet].entries = BTreeSet::from([value]);
            }
        }
        (OwnSplitChecked, Tag::Normal | Tag::Unwind) => facts[0].entries = read(0).entries,
        (CleanupNext, _) => {
            facts[0].entries = read(0).entries;
            facts[0].exit = read(0).exit;
        }
        (ExitDispatch, Tag::Pending) => {
            facts[1].entries = read(2).entries;
            facts[1].exit = read(2).exit;
        }
        _ => {}
    }
    facts
}

fn assign_held(state: &Held, edge: &Edge, produced: &[Slot]) -> Held {
    let mut out = state.clone();
    let sources = edge.assignments.iter().map(|assignment| match assignment.source {
        Source::Location(source) => state.read(source),
        Source::ProducedSlot(slot) => produced.get(slot as usize).cloned().unwrap_or_default(),
    }).collect::<Vec<_>>();
    for (assignment, slot) in edge.assignments.iter().zip(sources) {
        out.write(assignment.destination, slot);
    }
    out
}

/// The surely held entries at every pc of `f`. When `check`, a materialize whose entry is not held is refused.
fn held(p: &Program, f: &Function, fresh: &[bool], check: bool) -> Result<Vec<Option<Held>>, Error> {
    let resumes = resumes(f);
    let mut saved = BTreeMap::<u32, Vec<Slot>>::new();
    forward(f, Held::default(), Held::meet, |pc, state| {
        let mut out = state.clone();
        let mut successors = Vec::new();
        let mut revisit = Vec::new();
        match &f.code[pc] {
            Instruction::Data { op, inputs, output, .. } => {
                if let Some(id) = output {
                    let location = Location::Register(*id);
                    let copied = match (op, inputs.as_slice()) {
                        (DataOp::Move, [input]) => state.read(Location::Register(*input)),
                        _ => Slot::default(),
                    };
                    out.write(location, copied);
                }
                successors.push((pc + 1, out));
            }
            Instruction::Br(edge) => successors.push((edge.target_pc as usize, assign_held(state, edge, &[]))),
            Instruction::Cond { yes, no, .. } => {
                successors.push((yes.target_pc as usize, assign_held(state, yes, &[])));
                successors.push((no.target_pc as usize, assign_held(state, no, &[])));
            }
            Instruction::Ret(_) | Instruction::Raise(_) | Instruction::Unreachable => {}
            Instruction::Champ { op, arguments, flow } => match flow {
                Flow::Next(outputs) => {
                    let facts = match op {
                        Opcode::FrameSave => {
                            let k = suspension::continuation_of(arguments).unwrap_or(0);
                            let values = locations(arguments)[1..].iter().map(|&location| state.read(location))
                                .collect::<Vec<_>>();
                            match saved.get_mut(&k) {
                                Some(previous) => {
                                    let mut changed = false;
                                    for (slot, value) in previous.iter_mut().zip(&values) {
                                        changed |= meet_slot(slot, value);
                                    }
                                    if changed {
                                        revisit.extend(resumes.get(&k).into_iter().flatten().copied());
                                    }
                                }
                                None => {
                                    saved.insert(k, values);
                                    revisit.extend(resumes.get(&k).into_iter().flatten().copied());
                                }
                            }
                            vec![Slot::default()]
                        }
                        Opcode::FrameResume => {
                            let k = suspension::continuation_of(arguments).unwrap_or(0);
                            let stored = saved.get(&k).cloned().unwrap_or_default();
                            let mut facts = vec![Slot::default()];
                            facts.extend((1..outputs.len()).map(|index| {
                                stored.get(index - 1).cloned().unwrap_or_default()
                            }));
                            facts
                        }
                        _ => next_held(f, pc, *op, arguments, state, outputs, check)?,
                    };
                    for (&location, slot) in outputs.iter().zip(facts) {
                        out.write(location, slot);
                    }
                    if !matches!(op, Opcode::RetOwn | Opcode::RetDefer) {
                        successors.push((pc + 1, out));
                    }
                }
                Flow::Outcomes(outcomes) => {
                    for (ordinal, outcome) in outcomes.iter().enumerate() {
                        let slots = schema(p, f, *op, arguments, outcome.tag)?;
                        let produced = produced_held(p, pc, ordinal, *op, arguments, outcome.tag, &slots, state,
                            fresh);
                        successors.push((outcome.edge.target_pc as usize, assign_held(state, &outcome.edge,
                            &produced)));
                    }
                }
            },
        }
        Ok((successors, revisit))
    })
}

/// Whether every reachable return of `f` returns its Future with that Future's entry in its packet (D25.1).
fn returns_entry(p: &Program, f: &Function, fresh: &[bool]) -> Result<bool, Error> {
    if f.entry_abi != EntryAbi::Owned || !matches!(f.results.as_slice(), [Type::Future(_)]) {
        return Ok(false);
    }
    let states = held(p, f, fresh, false)?;
    let mut returns = 0;
    for (pc, instruction) in f.code.iter().enumerate() {
        let (Instruction::Champ { op: Opcode::RetOwn, arguments, .. }, Some(state)) = (instruction, &states[pc])
        else {
            continue;
        };
        returns += 1;
        let operands = locations(arguments);
        let value = operands.iter().find(|&&location| future(f, location)).and_then(|&location| {
            state.futures.get(&location).copied()
        });
        let packet = operands.iter().find(|&&location| kind(f, location) == Some(ServiceKind::Packet));
        let holds = value.zip(packet).is_some_and(|(value, packet)| {
            state.entries.get(packet).is_some_and(|entries| entries.contains(&value))
        });
        if !holds {
            return Ok(false);
        }
    }
    Ok(returns != 0)
}

/// For each function, whether it returns its Future's entry: the least fixed point over calls, so a recursion
/// proves nothing it does not show.
pub(crate) fn fresh_futures(p: &Program) -> Result<Vec<bool>, Error> {
    let mut fresh = vec![false; p.functions.len()];
    loop {
        let mut changed = false;
        for (index, f) in p.functions.iter().enumerate() {
            if !fresh[index] && returns_entry(p, f, &fresh)? {
                fresh[index] = true;
                changed = true;
            }
        }
        if !changed {
            return Ok(fresh);
        }
    }
}

/// `SPB_MATERIALIZE`: every materialize of `f` takes an entry its packet surely holds.
pub(crate) fn check_materialize(p: &Program, f: &Function, fresh: &[bool]) -> Check {
    if !f.code.iter().any(|instruction| matches!(instruction,
        Instruction::Champ { op: Opcode::ServiceMaterialize, .. }))
    {
        return Ok(());
    }
    held(p, f, fresh, true).map(|_| ())
}
