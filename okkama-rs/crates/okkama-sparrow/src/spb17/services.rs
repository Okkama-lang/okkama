//! Service duties inside packets in SPB 17 (async ABI D25, Champ draft 3 §§6, 15).
//!
//! Two forward analyses over concrete SPB edges, frame saves and resumes:
//! - **May admit** (D25.3): which closes take a scope state that may have admitted a service entry. A packet may
//!   carry entries when it is the result packet of a call or take whose language result can reach a service holder
//!   (D25.1: the result relation names the fresh holder's duty), the packet of an absorb, or of an exit that
//!   carries a rehomed duty (C-4), or when such a packet flows into it; a packet of unknown retention
//!   (`own.dynamic`) is judged by its source the same way. A dynamic split's selected packet carries what its
//!   source carries when its selection can reach a holder (#771: a stored Future's entry moves with its packet);
//!   a static split never selects an entry. Such a close needs exactly one
//!   `cleanup.settle.dynamic` before its first plan (`SPB_SCHEDULE`).
//! - **Holds** (D25.2): which entries a packet surely holds, by the Future value they name. An entry is known from
//!   an absorb, from a rehome into a reserve that an exit commits, and from a call whose callee is proved, by the
//!   same analysis of its returns, to return its Future's entry with its result. `service.materialize` takes only
//!   an entry its packet surely holds (`SPB_MATERIALIZE`, fixture `d25-materialize-absent`). A dynamic selection
//!   moves the entries of the holders it walks to (#771), so a state keeps only those it surely still holds, and an
//!   exit that returns a value reaching a holder must take that value's selection, or its Future's rehome, into its
//!   reserve (`SPB_EXIT_SELECTION`, ruling "How a stored Future leaves").
use super::*;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

type Check = Result<(), Error>;

/// `service.materialize` names an entry its packet surely holds (ABI D25.2, `d25-materialize-absent`).
pub(crate) const MATERIALIZE: &str = "SPB_MATERIALIZE";

/// Analysis steps one function may take before the loader refuses it.
const STEP_BUDGET: usize = 4_000_000;

/// Whether no value of `ty` reaches an identity (an object with destroy) through the fields the selection walk
/// follows, variant cases, array elements, role implementations and any boxes: what such a value's packet carries
/// is at most the service entries of the Futures it holds (D25.1, #771, #758), so a static selection of it selects
/// no position. Maps and unknown declarations are not known to be free.
pub fn identity_free(objects: &[Object], ty: Type) -> bool {
    let mut pending = vec![ty];
    let mut seen = std::collections::HashSet::new();
    while let Some(ty) = pending.pop() {
        if !seen.insert(ty) {
            continue;
        }
        match ty {
            Type::Object(id) | Type::Successor(id) | Type::Variant(id) => {
                let Some(object) = objects.get(id as usize) else { return false };
                if object.layout.has_destroy {
                    return false;
                }
                let layout = &object.layout;
                for &index in &layout.selection_layout {
                    let Some(&field) = layout.storage_fields.get(index as usize) else { return false };
                    pending.push(field);
                }
                pending.extend(object.variant_cases.iter().map(|&case| Type::Object(case)));
            }
            Type::Role(id) | Type::RoleSuccessor(id) => {
                pending.extend(objects.iter().enumerate()
                    .filter(|(_, object)| object.implements.iter().any(|table| table.role == id))
                    .map(|(object, _)| Type::Object(object as u32)));
            }
            Type::Any => {
                pending.extend(objects.iter().enumerate().filter(|(_, object)| object.any_box)
                    .map(|(object, _)| Type::Object(object as u32)));
            }
            Type::Error => {
                pending.extend(objects.iter().enumerate().filter(|(_, object)| !object.error_cases.is_empty())
                    .map(|(object, _)| Type::Object(object as u32)));
            }
            Type::Array(shape) if shape.depth > 0 => pending.extend(ArrayType { depth: 1, ..shape }.element()),
            Type::Map(_) | Type::MapBuilder(_) | Type::Builder(_) => return false,
            _ => {}
        }
    }
    true
}

/// Whether a value of `ty` can reach a service holder (a Future, or a FutureGroup with its own entry, #810), through
/// object fields, variant cases, error cases, array and map elements, role implementations and any boxes. Unknown
/// declarations can.
pub fn reaches_holder(p: &Program, ty: Type) -> bool {
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
                if super::drivers::group_specialization(&object.name) {
                    return true;
                }
                // PR-A: an Atomic over Futures borrows them; its value word holds no entry (ATOMIC_BORROW), as the
                // ownership verifier's `holds_futures` reads a builtin Atomic.
                let borrowing = object.name.starts_with("async.atomic.Atomic_t_")
                    && object.layout.storage_fields.first().is_some_and(|value| super::p1::borrowed_future(*value));
                pending.extend(object.layout.storage_fields.iter().skip(usize::from(borrowing)));
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

/// The cache-state operand of a `fut.spawn` (#765): the Future's cache duty, which the spawn forwards.
pub(crate) fn spawned_cache(arguments: &[Argument]) -> Option<Location> {
    locations(arguments).get(1).copied()
}

/// Whether `cache.rehome` with these location operands is the state form (reserve, scope state, Future): the token
/// form takes the reserve, the cache duty and, for a spawned Future, its task duty (ABI C-4, D24.1).
pub(crate) fn rehome_state_form(f: &Function, operands: &[Location]) -> bool {
    operands.len() == 3 && kind(f, operands[1]) == Some(ServiceKind::ScopeState)
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
        // The state form gives the state back with what else it admitted, then the reserve.
        CacheRehome if outputs.len() == 2 => vec![ins.get(1) == Some(&true), true],
        CacheRehome => vec![true],
        ServiceAbsorb => vec![true],
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
fn produced_carriers(p: &Program, f: &Function, op: Opcode, arguments: &[Argument], tag: Tag, schema: &[ProducedType],
    ins: &[bool]) -> Vec<bool>
{
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
        // A selection takes the entries of the holders it walks to (#771); a refused check returns the packet
        // unchanged.
        (OwnSplitChecked | ScopeSplit, Tag::Normal) => {
            let walks = selection_roots(f, arguments).iter().any(|&root| holder_value(p, f, root));
            vec![first, first && walks, false]
        }
        (OwnSplitChecked, Tag::Unwind) => vec![first, false],
        (ScopeSplit, Tag::Unwind) => vec![first, false],
        // A packet of unknown retention (D25.3): which obligations it holds is unknown, but its entries can only
        // name holders its source's language result reaches, which the packet's own fact already judges.
        (OwnDynamic, Tag::Normal) => vec![any],
        (OwnDynamic, Tag::Unwind) => vec![first, ins.get(1).copied().unwrap_or(false), false],
        (ScopeTransfer, _) => schema.iter().map(|slot| *slot == ProducedType::Service(ServiceKind::ScopeState)
            && any).collect(),
        (CleanupNext, _) => schema.iter().enumerate().map(|(index, _)| index == 0 && first).collect(),
        (ExitDispatch, Tag::Pending) => vec![false, ins.get(2).copied().unwrap_or(false)],
        (BuilderPush | BuilderAppend | BuilderRange | BuilderFinish | BuilderAbandon, _) => packets(any),
        _ => vec![false; schema.len()],
    }
}

/// `fut.new` gives the callee an inputs packet that carries service entries only when some start value of the callee
/// can bring one (`entry_carries`; PR-B2, #782): a bare Future input is borrowed and arrives without its entry.
pub(crate) const FUTURE_INPUT: &str = "SPB_FUTURE_INPUT";

/// The language inputs of an entry: the start values of an async body (its continuation 0, D21), or the
/// parameters of a synchronous function; `None` for an async body without a start continuation.
fn language_inputs(p: &Program, f: &Function) -> Option<Vec<ProducedType>> {
    match p.frame_layout(f) {
        Some(layout) => layout.saved_types(0),
        None => Some(f.params.iter().filter_map(|&location| verify::loc_type(f, location).ok()).collect()),
    }
}

/// PR-B2 (#782): whether the entry packet of `f` may carry service entries, so that its scope may hold them
/// (D25.3). `own.accept` puts them in the scope's bound journal (`admit(.., bound = true)`), not its admission
/// journal. A language input that can reach a holder brings its entries in the packet: a parameter of a synchronous
/// function (a constructor or a synchronous owner stores it, D25.2), or an aggregate start value of an async body.
/// A bare Future start value does not: own never offers a Future input to `fut.new` (OIR-2 §10.2), so the body
/// borrows it.
pub(crate) fn entry_carries(p: &Program, f: &Function) -> bool {
    let asynchronous = p.frame_layout(f).is_some();
    language_inputs(p, f).is_none_or(|inputs| inputs.into_iter().any(|ty| match ty {
        ProducedType::Value(Type::Future(_)) if asynchronous => false,
        ProducedType::Value(ty) => reaches_holder(p, ty),
        _ => false,
    }))
}

/// The begin pcs of closes whose scope state may have admitted a service entry on some path (D25.3). An entry
/// packet that `entry_carries` is a carrier from the start (#782): the packet parameter of a synchronous entry, the
/// start packet that an async body's continuation 0 resumes with.
pub(crate) fn admitting_closes(p: &Program, f: &Function) -> Result<BTreeSet<u32>, Error> {
    let resumes = resumes(f);
    let mut saved = BTreeMap::<u32, Vec<bool>>::new();
    let mut closes = BTreeSet::new();
    let join = |previous: &mut Carriers, next: &Carriers| {
        let before = previous.len();
        previous.extend(next.iter().copied());
        previous.len() != before
    };
    let carries = entry_carries(p, f);
    let start: Carriers = if carries && p.frame_layout(f).is_none() {
        f.params.iter().copied().filter(|&location| kind(f, location) == Some(ServiceKind::Packet)).collect()
    } else {
        Carriers::new()
    };
    forward(f, start, join, |pc, state| {
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
                                        || (k == 0 && carries && kind(f, outputs[index]) == Some(ServiceKind::Packet))
                                }));
                                facts
                            }
                            Opcode::FutureNew => {
                                future_input(p, arguments, &ins)?;
                                next_carriers(f, *op, arguments, &ins, outputs)
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
                            let produced = produced_carriers(p, f, *op, arguments, outcome.tag, &slots, &ins);
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

/// `SPB_FUTURE_INPUT` (#782): the inputs packet of `fut.new`, which follows the callee's start values, carries
/// entries only when the callee's entry packet may (`entry_carries`).
fn future_input(p: &Program, arguments: &[Argument], ins: &[bool]) -> Check {
    let Some(callee) = arguments.iter().find_map(|argument| match argument {
        Argument::Entry(id) => p.functions.get(*id as usize),
        _ => None,
    }) else {
        return Ok(());
    };
    let Some(inputs) = language_inputs(p, callee) else {
        return Ok(());
    };
    let values = inputs.iter().filter(|ty| matches!(ty, ProducedType::Value(_))).count();
    let carried = ins.get(values).copied().unwrap_or(false);
    enforce(!carried || entry_carries(p, callee), FUTURE_INPUT,
        &format!("fut.new of {} gives an inputs packet that may carry service entries to a body whose inputs cannot \
            bring them", callee.symbol))
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
    /// Registers whose type can reach a service holder, and the value they hold (a Future register's is its
    /// Future's): what an exit returns and what a dynamic selection names (#771).
    values: BTreeMap<Location, Val>,
    /// Packets and the values whose holders a dynamic selection surely walked into them, or whose call returned
    /// them with their entries (#771).
    selected: BTreeMap<Location, BTreeSet<Val>>,
    /// Reserves and their continuations whose exit returns a value that can reach a holder: that value, and
    /// whether a selection or a rehome of it surely joined the reserve (`SPB_EXIT_SELECTION`).
    needs: BTreeMap<Location, (Val, bool)>,
    /// Cache-state tokens and the Future whose duty they are.
    tokens: BTreeMap<Location, Val>,
    /// Packets, scope states, reserves and the cleanup and exit states that continue a reserve: the entries they
    /// surely hold.
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
    value: Option<Val>,
    selected: BTreeSet<Val>,
    need: Option<(Val, bool)>,
}

impl Held {
    fn read(&self, location: Location) -> Slot {
        Slot {
            future: self.futures.get(&location).copied(),
            token: self.tokens.get(&location).copied(),
            entries: self.entries.get(&location).cloned().unwrap_or_default(),
            exit: self.exits.get(&location).copied(),
            value: self.values.get(&location).copied(),
            selected: self.selected.get(&location).cloned().unwrap_or_default(),
            need: self.needs.get(&location).copied(),
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
        if let Some(value) = slot.value {
            self.values.insert(location, value);
        }
        if !slot.selected.is_empty() {
            self.selected.insert(location, slot.selected);
        }
        if let Some(need) = slot.need {
            self.needs.insert(location, need);
        }
    }

    fn kill(&mut self, location: Location) {
        self.futures.remove(&location);
        self.tokens.remove(&location);
        self.entries.remove(&location);
        self.exits.remove(&location);
        self.values.remove(&location);
        self.selected.remove(&location);
        self.needs.remove(&location);
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
        self.values.retain(|location, value| other.values.get(location) == Some(value));
        self.selected.retain(|location, values| {
            let Some(theirs) = other.selected.get(location) else { return false };
            values.retain(|value| theirs.contains(value));
            !values.is_empty()
        });
        // A requirement on some path stays; it holds only where every path met it.
        for (location, (value, covered)) in &other.needs {
            let need = self.needs.entry(*location).or_insert((*value, *covered));
            need.1 &= *covered && need.0 == *value;
        }
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
    if slot.value != other.value {
        slot.value = None;
    }
    slot.selected.retain(|value| other.selected.contains(value));
    match (&mut slot.need, other.need) {
        (Some(need), Some(theirs)) => need.1 &= theirs.1 && need.0 == theirs.0,
        (need @ None, theirs) => *need = theirs,
        (Some(_), None) => {}
    }
    *slot != before
}

/// `exit.commit` returns a value that can reach a service holder only when its exit selection (or the rehome of
/// the returned Future) surely joined the reserve, so the holders' entries travel with the value (ruling "How a
/// stored Future leaves", #771).
pub(crate) const EXIT_SELECTION: &str = "SPB_EXIT_SELECTION";

/// The roots of a dynamic selection argument.
fn selection_roots<'f>(f: &'f Function, arguments: &[Argument]) -> &'f [Location] {
    arguments.iter().find_map(|argument| match argument {
        Argument::Selection(id) => {
            f.proofs.selections.get(*id as usize).map(|selection| selection.locations.as_slice())
        }
        _ => None,
    }).unwrap_or(&[])
}

/// Whether `location` holds a value whose type can reach a service holder.
fn holder_value(p: &Program, f: &Function, location: Location) -> bool {
    matches!(verify::loc_type(f, location), Ok(ProducedType::Value(ty)) if reaches_holder(p, ty))
}

/// The values whose holders a dynamic selection may walk to: `Some` with the known ones, `None` when a root that can
/// reach a holder has no known value.
fn walked(p: &Program, f: &Function, arguments: &[Argument], state: &Held) -> Option<BTreeSet<Val>> {
    let mut values = BTreeSet::new();
    for &root in selection_roots(f, arguments) {
        if !holder_value(p, f, root) {
            continue;
        }
        values.insert(*state.values.get(&root)?);
    }
    Some(values)
}

/// A selection moves the entries of the holders it walks to (ABI D25.1, #771): `kept` loses them, and the part
/// gains those `kept` surely held. A root of unknown value may reach any holder, so `kept` keeps nothing sure.
fn take_walked(kept: &mut BTreeSet<Val>, walked: &Option<BTreeSet<Val>>) -> BTreeSet<Val> {
    match walked {
        Some(values) => {
            let moved = kept.intersection(values).copied().collect();
            kept.retain(|value| !values.contains(value));
            moved
        }
        None => {
            kept.clear();
            BTreeSet::new()
        }
    }
}

/// The facts of the outputs of a `Next` instruction at `pc`. When `check`, `Err` names a materialize whose entry is
/// not held, a state-form rehome whose entry is not held, or a commit whose exit selection is not met.
#[allow(clippy::too_many_arguments)]
fn next_held(p: &Program, f: &Function, pc: usize, op: Opcode, arguments: &[Argument], state: &Held,
    outputs: &[Location], check: bool) -> Result<Vec<Slot>, Error>
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
            facts[2].entries = read(operands.len() - 1).entries;
        }
        // An admission adds the packet's entries to the state's (D25.3).
        OwnAdmit | OwnAccept | OwnBind | OwnRebind => {
            facts[0].entries = union(&mut [0, operands.len() - 1].into_iter());
        }
        AwaitUnwind | DeferRegister => facts[0].entries = read(0).entries,
        OwnMove => {
            facts[0].entries = read(0).entries;
            facts[0].selected = read(0).selected;
        }
        OwnJoin => {
            facts[0].entries = union(&mut (0..operands.len()));
            facts[0].selected = (0..operands.len()).flat_map(|index| read(index).selected).collect();
        }
        OwnSplit => facts[0].entries = read(0).entries,
        ServiceAbsorb => {
            let mut entries = read(0).entries;
            entries.extend(read(1).token);
            facts[0].entries = entries;
            facts[0].selected = read(0).selected;
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
            facts[0].need = (operands.len() == 2 && holder_value(p, f, operands[0])).then(|| {
                read(0).value.map(|value| (value, read(packet).selected.contains(&value)))
            }).flatten();
        }
        ExitExtend => {
            facts[0].entries = union(&mut (0..2));
            facts[0].exit = read(0).exit;
            facts[0].need = read(0).need.map(|(value, covered)| (value, covered || read(1).selected.contains(&value)));
        }
        // The replacement reserve returns its own value, if any, from its own packet; the replaced reserve's content
        // comes back as a packet.
        ExitReplace => {
            let packet = operands.len() - 1;
            facts[0].entries = read(packet).entries;
            facts[0].need = (operands.len() == 3 && holder_value(p, f, operands[1])).then(|| {
                read(1).value.map(|value| (value, read(packet).selected.contains(&value)))
            }).flatten();
            facts[1].entries = read(0).entries;
        }
        // D24.1: the cache duty comes back and the task duty is new; both name the spawned Future.
        FutureSpawn => {
            facts[0].token = read(1).token;
            facts[1].token = read(1).token;
        }
        CacheRehome if rehome_state_form(f, &operands) => {
            // The state form takes the entry of its Future out of a state that surely holds it.
            let holder = read(2).future;
            let mut held = read(1).entries;
            let found = holder.is_some_and(|value| held.remove(&value));
            enforce(found || !check, automaton::REHOME, &format!(
                "cache.rehome at pc {pc} takes an entry its state does not surely hold"))?;
            let mut entries = read(0).entries;
            entries.extend(holder);
            facts[0].entries = held;
            facts[1].entries = entries;
            facts[1].exit = read(0).exit;
            // Rehoming the returned Future moves its entry with the reserve.
            facts[1].need = read(0).need.map(|(value, covered)| (value, covered || holder == Some(value)));
        }
        CacheRehome => {
            let mut entries = read(0).entries;
            entries.extend(read(1).token);
            facts[0].entries = entries;
            facts[0].exit = read(0).exit;
            facts[0].need = read(0).need.map(|(value, covered)| (value, covered || read(1).token == Some(value)));
        }
        CleanupBegin => {
            if let Some(index) = operands.iter().position(|&location| kind(f, location) == Some(ServiceKind::Pending)) {
                facts[0].entries = read(index).entries;
                facts[0].exit = read(index).exit;
                facts[0].need = read(index).need;
            }
        }
        CleanupSettle | CleanupSettleDynamic | CleanupPlan | CleanupFailure | CleanupDeliverCancel
            | DestroyDirect | CleanupDestroy => {
            facts[0].entries = read(0).entries;
            facts[0].exit = read(0).exit;
            facts[0].need = read(0).need;
        }
        CleanupFinish => {
            facts[2].entries = read(0).entries;
            facts[2].exit = read(0).exit;
            facts[2].need = read(0).need;
        }
        ExitCommit => {
            let pending = read(0);
            enforce(pending.need.is_none_or(|(_, covered)| covered) || !check, EXIT_SELECTION, &format!(
                "{}: exit.commit at pc {pc} returns a value whose holders' entries its exit selection does not \
                surely take", f.symbol))?;
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
fn produced_held(p: &Program, f: &Function, pc: usize, ordinal: usize, op: Opcode, arguments: &[Argument],
    tag: Tag, schema: &[ProducedType], state: &Held, fresh: &[bool]) -> Vec<Slot>
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
    // Every produced value that can reach a holder is a new value, named by its slot.
    for (slot, produced) in schema.iter().enumerate() {
        if matches!(produced, ProducedType::Value(ty) if reaches_holder(p, *ty)) {
            facts[slot].value = Some((pc as u32, ordinal as u32, slot as u32));
        }
    }
    let packet = schema.iter().position(|slot| *slot == ProducedType::Service(ServiceKind::Packet));
    match (op, tag) {
        (InvokeCallOwn | InvokeDispatchOwn, Tag::Normal) if returns_entry => {
            if let (Some(ProducedType::Value(Type::Future(_))), Some(packet)) = (schema.first(), packet) {
                let value = (pc as u32, ordinal as u32, 0);
                facts[0].future = Some(value);
                facts[packet].entries = BTreeSet::from([value]);
            }
        }
        _ => {}
    }
    match (op, tag) {
        // A callee's result packet carries what its own exit selection took for the result (#771).
        (InvokeCallOwn | InvokeDispatchOwn | AwaitTake, Tag::Normal) | (InvokeDefer, Tag::Replace(_)) => {
            if let (Some(value), Some(packet)) = (facts.first().and_then(|slot| slot.value), packet) {
                facts[packet].selected.insert(value);
            }
        }
        // A dynamic selection moves the entries of the holders it walks to (ABI D25.1, #771); a refused check
        // returns its input unchanged.
        (OwnSplitChecked | ScopeSplit, Tag::Normal) => {
            let walked = walked(p, f, arguments, state);
            let mut kept = read(0).entries;
            facts[1].entries = take_walked(&mut kept, &walked);
            facts[1].selected = walked.unwrap_or_default();
            facts[0].entries = kept;
        }
        (ScopeTransfer, Tag::Normal) => {
            let walked = walked(p, f, arguments, state);
            let mut kept = read(0).entries;
            let moved = take_walked(&mut kept, &walked);
            facts[0].entries = kept;
            if schema.get(1) == Some(&ProducedType::Service(ServiceKind::ScopeState)) {
                let mut target = read(1).entries;
                target.extend(moved);
                facts[1].entries = target;
            }
        }
        (OwnSplitChecked | ScopeSplit | ScopeTransfer | OwnDynamic, _) => facts[0].entries = read(0).entries,
        (CleanupNext, _) => {
            facts[0].entries = read(0).entries;
            facts[0].exit = read(0).exit;
            facts[0].need = read(0).need;
        }
        (ExitDispatch, Tag::Pending) => {
            facts[1].entries = read(2).entries;
            facts[1].exit = read(2).exit;
            facts[1].need = read(2).need;
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
///
/// `admitting` names the closes whose state may hold admitted entries (D25.3): an exit's selection requirement
/// stands only through such a close; elsewhere the returned value's holders have no entry in the closing state
/// (a borrowed view, an element of a borrowed array).
fn held(p: &Program, f: &Function, fresh: &[bool], check: bool, admitting: Option<&BTreeSet<u32>>)
    -> Result<Vec<Option<Held>>, Error>
{
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
                    let mut copied = match (op, inputs.as_slice()) {
                        (DataOp::Move, [input]) => state.read(Location::Register(*input)),
                        _ => Slot::default(),
                    };
                    if copied.value.is_none() && holder_value(p, f, location) {
                        copied.value = Some((pc as u32, u32::MAX, 0));
                    }
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
                        Opcode::ExitPrepare => {
                            // A returned value joined from several definitions has no known value yet: it is the
                            // one the prepare names, as the exit's selection names it.
                            let mut named = state.clone();
                            let operands = locations(arguments);
                            if operands.len() == 2 && holder_value(p, f, operands[0])
                                && !named.values.contains_key(&operands[0])
                            {
                                named.values.insert(operands[0], (pc as u32, u32::MAX - 1, 0));
                                out.values.insert(operands[0], (pc as u32, u32::MAX - 1, 0));
                            }
                            next_held(p, f, pc, *op, arguments, &named, outputs, check)?
                        }
                        Opcode::CleanupBegin => {
                            let mut facts = next_held(p, f, pc, *op, arguments, state, outputs, check)?;
                            if admitting.is_some_and(|closes| !closes.contains(&(pc as u32))) {
                                facts[0].need = None;
                            }
                            facts
                        }
                        _ => next_held(p, f, pc, *op, arguments, state, outputs, check)?,
                    };
                    for (index, (&location, mut slot)) in outputs.iter().zip(facts).enumerate() {
                        if slot.value.is_none() && holder_value(p, f, location) {
                            slot.value = Some(slot.future.unwrap_or((pc as u32, u32::MAX, index as u32)));
                        }
                        out.write(location, slot);
                    }
                    if !matches!(op, Opcode::RetOwn | Opcode::RetDefer) {
                        successors.push((pc + 1, out));
                    }
                }
                Flow::Outcomes(outcomes) => {
                    for (ordinal, outcome) in outcomes.iter().enumerate() {
                        let slots = schema(p, f, *op, arguments, outcome.tag)?;
                        let produced = produced_held(p, f, pc, ordinal, *op, arguments, outcome.tag, &slots,
                            state, fresh);
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
    let states = held(p, f, fresh, false, None)?;
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

/// `SPB_MATERIALIZE`: every materialize of `f` takes an entry its packet surely holds; `SPB_REHOME`: every
/// state-form rehome takes an entry its state surely holds; `SPB_EXIT_SELECTION`: every exit that returns a value
/// reaching a holder takes that value's selection (or its Future's rehome) into its reserve (#771).
pub(crate) fn check_materialize(p: &Program, f: &Function, fresh: &[bool]) -> Check {
    if !f.code.iter().any(|instruction| match instruction {
        Instruction::Champ { op: Opcode::ServiceMaterialize, .. } => true,
        Instruction::Champ { op: Opcode::CacheRehome, arguments, .. } => rehome_state_form(f, &locations(arguments)),
        Instruction::Champ { op: Opcode::ExitPrepare | Opcode::ExitReplace, arguments, .. } => {
            locations(arguments).iter().any(|&location| holder_value(p, f, location))
        }
        _ => false,
    }) {
        return Ok(());
    }
    let admitting = admitting_closes(p, f)?;
    held(p, f, fresh, true, Some(&admitting)).map(|_| ())
}

// --- Scopes of states (C-4, state form) ----------------------------------------------------------------------

/// The scopes each scope-state location may belong to.
type Scopes = BTreeMap<Location, BTreeSet<u32>>;

/// The scopes of the outputs (or produced slots) of an operation, from the scopes of its state operands: the
/// i-th state result continues the i-th state operand, and a lone state result continues all of them.
fn state_results(slots: &[bool], states: &[BTreeSet<u32>]) -> Vec<BTreeSet<u32>> {
    let count = slots.iter().filter(|state| **state).count();
    let mut index = 0;
    slots.iter().map(|&state| {
        if !state {
            return BTreeSet::new();
        }
        let scopes = if count == states.len() {
            states[index].clone()
        } else {
            states.iter().flatten().copied().collect()
        };
        index += 1;
        scopes
    }).collect()
}

/// For each state-form `cache.rehome`, the scope of its state when only one reaches it (C-4: the duties leave
/// the scope that owns them).
pub(crate) fn rehome_scopes(p: &Program, f: &Function) -> Result<BTreeMap<usize, Option<u32>>, Error> {
    // Only a function with a state-form rehome has anything to report; the others skip the analysis (#750).
    let rehomes_state = f.code.iter().any(|instruction| matches!(instruction,
        Instruction::Champ { op: Opcode::CacheRehome, arguments, .. } if rehome_state_form(f, &locations(arguments))));
    if !rehomes_state {
        return Ok(BTreeMap::new());
    }
    let resumes = resumes(f);
    let mut saved = BTreeMap::<u32, Vec<BTreeSet<u32>>>::new();
    let mut rehomes = BTreeMap::new();
    let state = |location: Location| kind(f, location) == Some(ServiceKind::ScopeState);
    let join = |previous: &mut Scopes, next: &Scopes| {
        let before = previous.clone();
        for (location, scopes) in next {
            previous.entry(*location).or_default().extend(scopes.iter().copied());
        }
        *previous != before
    };
    let assign = |scopes: &Scopes, edge: &Edge, produced: &[BTreeSet<u32>]| {
        let mut out = scopes.clone();
        let sources = edge.assignments.iter().map(|assignment| match assignment.source {
            Source::Location(source) => scopes.get(&source).cloned().unwrap_or_default(),
            Source::ProducedSlot(slot) => produced.get(slot as usize).cloned().unwrap_or_default(),
        }).collect::<Vec<_>>();
        // Sparse: a location without scopes is absent, which every read takes as the empty set (#750).
        for (assignment, value) in edge.assignments.iter().zip(sources) {
            if value.is_empty() {
                out.remove(&assignment.destination);
            } else {
                out.insert(assignment.destination, value);
            }
        }
        out
    };
    forward(f, Scopes::new(), join, |pc, scopes| {
        let mut out = scopes.clone();
        let mut successors = Vec::new();
        let mut revisit = Vec::new();
        match &f.code[pc] {
            Instruction::Data { .. } => successors.push((pc + 1, out)),
            Instruction::Br(edge) => successors.push((edge.target_pc as usize, assign(scopes, edge, &[]))),
            Instruction::Cond { yes, no, .. } => {
                successors.push((yes.target_pc as usize, assign(scopes, yes, &[])));
                successors.push((no.target_pc as usize, assign(scopes, no, &[])));
            }
            Instruction::Ret(_) | Instruction::Raise(_) | Instruction::Unreachable => {}
            Instruction::Champ { op, arguments, flow } => {
                let operands = locations(arguments);
                let read = |location: &Location| scopes.get(location).cloned().unwrap_or_default();
                let states = operands.iter().filter(|location| state(**location)).map(read).collect::<Vec<_>>();
                if *op == Opcode::CacheRehome && rehome_state_form(f, &operands) {
                    let scope = read(&operands[1]);
                    let single = (scope.len() == 1).then(|| scope.first().copied()).flatten();
                    let entry = rehomes.entry(pc).or_insert(single);
                    if *entry != single {
                        *entry = None;
                    }
                }
                match flow {
                    Flow::Next(outputs) => {
                        let facts = match op {
                            Opcode::ScopeBegin => {
                                let scope = arguments.iter().find_map(|argument| match argument {
                                    Argument::Scope(id) => Some(*id),
                                    _ => None,
                                });
                                vec![scope.into_iter().collect()]
                            }
                            Opcode::FrameSave => {
                                let k = suspension::continuation_of(arguments).unwrap_or(0);
                                let values = operands[1..].iter().map(read).collect::<Vec<_>>();
                                let entry = saved.entry(k).or_insert_with(|| vec![BTreeSet::new(); values.len()]);
                                let mut grew = false;
                                for (slot, value) in entry.iter_mut().zip(values) {
                                    let before = slot.len();
                                    slot.extend(value);
                                    grew |= slot.len() != before;
                                }
                                if grew {
                                    revisit.extend(resumes.get(&k).into_iter().flatten().copied());
                                }
                                vec![BTreeSet::new()]
                            }
                            Opcode::FrameResume => {
                                let k = suspension::continuation_of(arguments).unwrap_or(0);
                                let stored = saved.get(&k).cloned().unwrap_or_default();
                                let mut facts = vec![BTreeSet::new()];
                                facts.extend((1..outputs.len()).map(|index| {
                                    stored.get(index - 1).cloned().unwrap_or_default()
                                }));
                                facts
                            }
                            _ => {
                                let slots = outputs.iter().map(|location| state(*location)).collect::<Vec<_>>();
                                state_results(&slots, &states)
                            }
                        };
                        for (&location, value) in outputs.iter().zip(facts) {
                            if value.is_empty() {
                                out.remove(&location);
                            } else {
                                out.insert(location, value);
                            }
                        }
                        if !matches!(op, Opcode::RetOwn | Opcode::RetDefer) {
                            successors.push((pc + 1, out));
                        }
                    }
                    Flow::Outcomes(outcomes) => {
                        for outcome in outcomes {
                            let slots = schema(p, f, *op, arguments, outcome.tag)?.iter()
                                .map(|slot| *slot == ProducedType::Service(ServiceKind::ScopeState))
                                .collect::<Vec<_>>();
                            let produced = state_results(&slots, &states);
                            successors.push((outcome.edge.target_pc as usize, assign(scopes, &outcome.edge,
                                &produced)));
                        }
                    }
                }
            }
        }
        Ok((successors, revisit))
    })?;
    Ok(rehomes)
}
