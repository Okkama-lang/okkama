//! Independent finite inventory and creation-origin reconstruction for the loader.
//! Cycles/unknown inventories cannot establish a compiled direct plan.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

type Inventory = BTreeSet<(u32, Option<u32>)>;

/// A call packet contains only obligations of its actual language arguments.
/// The loader checks every call, and the machine entry checks the host boundary.
/// An async body's inputs are the values of its start continuation (D21), not its frame parameter.
pub fn plain_inputs(p: &Program, f: &Function) -> bool {
    let inputs: Vec<Result<ProducedType, Error>> = match p.frame_layout(f) {
        Some(layout) => match layout.saved_types(0) {
            Some(types) => types.into_iter().map(Ok).collect(),
            None => vec![Err(error("an async body without its start continuation"))],
        },
        None => f.params.iter().map(|&location| verify::loc_type(f, location)).collect(),
    };
    inputs.into_iter().all(|ty| match ty {
        Ok(ProducedType::Value(ty @ (Type::Object(_) | Type::Successor(_) | Type::Array(_) | Type::Map(_)
            | Type::Variant(_)))) => {
            !verify::reaches_resource(p, ty)
        }
        Ok(ProducedType::Value(Type::Error)) => p.objects.iter().filter(|object| !object.error_cases.is_empty())
            .all(|object| !object.layout.may_reach_resource),
        Ok(ProducedType::Value(Type::Role(id) | Type::RoleSuccessor(id))) => !verify::role_reaches_resource(p, id),
        Ok(ProducedType::Value(Type::Any)) => !verify::any_reaches_resource(p),
        Ok(_) => true,
        Err(_) => false,
    })
}

/// Where a `frame.resume` output comes from.
enum Resumed {
    /// The value its continuation's `frame.save` stored at this position.
    Saved(Location),
    /// A start value of the body, written by `fut.new`.
    Start,
}

/// The returned packet is empty: no result reaches a resource, nor a Future whose duty's entry it may carry
/// (ABI D25.1).
pub fn plain_results(p: &Program, f: &Function) -> bool {
    f.results.iter().all(|ty| !super::services::reaches_holder(p, *ty) && match ty {
        Type::Object(_) | Type::Successor(_) | Type::Array(_) | Type::Map(_) | Type::Variant(_) => {
            !verify::reaches_resource(p, *ty)
        }
        Type::Error => p.objects.iter().filter(|object| !object.error_cases.is_empty())
            .all(|object| !object.layout.may_reach_resource),
        Type::Role(id) | Type::RoleSuccessor(id) => !verify::role_reaches_resource(p, *id),
        Type::Any => !verify::any_reaches_resource(p),
        _ => true,
    })
}

/// Closed empty-inventory proof, including CFG cycles: no input or producer can introduce a key.
/// Calls may create and destroy their own locals; only their outgoing packets enter this function.
fn resource_free(p: &Program, f: &Function) -> bool {
    plain_inputs(p, f) && f.code.iter().all(|instruction| match instruction {
        Instruction::Champ { op: Opcode::OwnCreateCommit, arguments, .. } => arguments.iter().any(|arg| {
            matches!(arg, Argument::Object(id) if !p.objects[*id as usize].layout.has_destroy)
        }),
        Instruction::Champ { op: Opcode::InvokeCallOwn, arguments, .. } => arguments.iter().any(|arg| {
            matches!(arg, Argument::Entry(id) if plain_results(p, &p.functions[*id as usize]))
        }),
        Instruction::Champ { op: Opcode::InvokeDispatchOwn, arguments, .. } => dispatch_plain_results(p, arguments),
        Instruction::Champ { op: Opcode::InvokeDefer, .. } => false,
        _ => true,
    })
}

/// Every implementation a dispatch can enter returns plain results: its returned packet is empty.
fn dispatch_plain_results(p: &Program, arguments: &[Argument]) -> bool {
    let callees = load::dispatch_callees(p, arguments);
    !callees.is_empty() && callees.iter().all(|callee| plain_results(p, callee))
}

/// PCs where independently reconstructed selected inventories prove the admission filter redundant.
pub fn redundant_admission_filters(p: &Program, f: &Function) -> Result<Vec<usize>, Error> {
    let mut evidence = Evidence::new(p, f);
    let mut result = Vec::new();
    for (pc, instruction) in f.code.iter().enumerate() {
        if let Instruction::Champ { op: Opcode::ScopeSplit, arguments, .. } = instruction
            && arguments.contains(&Argument::Flag(true))
        {
            let mut unfiltered = arguments.clone();
            unfiltered.retain(|argument| !matches!(argument, Argument::Flag(_)));
            if let (Some(filtered), Some(all)) = (
                evidence.partition(arguments, true), evidence.partition(&unfiltered, true),
            ) && filtered == all {
                result.push(pc);
            }
        }
    }
    evidence.check_budget()?;
    Ok(result)
}

#[derive(Clone, Copy)]
enum Definition {
    Instruction(usize, usize),
    Incoming(usize, usize, Source),
}

fn definitions(f: &Function) -> BTreeMap<Location, Vec<Definition>> {
    let mut definitions = BTreeMap::<Location, Vec<Definition>>::new();
    for (pc, instruction) in f.code.iter().enumerate() {
        match instruction {
            Instruction::Data { output: Some(id), .. } => {
                definitions.entry(Location::Register(*id)).or_default().push(Definition::Instruction(pc, 0));
            }
            Instruction::Champ { flow: Flow::Next(outputs), .. } => {
                for (index, &output) in outputs.iter().enumerate() {
                    definitions.entry(output).or_default().push(Definition::Instruction(pc, index));
                }
            }
            _ => {}
        }
        for (ordinal, edge) in allocation::outgoing(instruction).iter().enumerate() {
            for assignment in &edge.assignments {
                definitions.entry(assignment.destination).or_default()
                    .push(Definition::Incoming(pc, ordinal, assignment.source));
            }
        }
    }
    definitions
}

/// Builder transitions with outcomes: the journal packet is their last operand.
fn journal_transition(op: Opcode) -> bool {
    matches!(op, Opcode::BuilderPush | Opcode::BuilderAppend | Opcode::BuilderRange | Opcode::BuilderFinish)
}

fn arguments(instruction: &Instruction) -> Vec<Location> {
    match instruction {
        Instruction::Champ { arguments, .. } => arguments.iter().filter_map(|argument| match argument {
            Argument::Location(location) => Some(*location),
            _ => None,
        }).collect(),
        _ => vec![],
    }
}

fn callee(p: &Program, instruction: &Instruction) -> Option<u32> {
    let Instruction::Champ { op: Opcode::InvokeCallOwn, arguments, .. } = instruction else { return None };
    arguments.iter().find_map(|argument| match *argument {
        Argument::Entry(id) if p.functions.get(id as usize)?.proofs.entry.as_ref()?.fresh_singleton => Some(id),
        _ => None,
    })
}

/// Resolution steps one function's evidence may take before its queries stay unresolved.
const STEP_BUDGET: usize = 1_000_000;

pub(super) struct Evidence<'a> {
    program: &'a Program,
    function: &'a Function,
    steps: usize,
    exhausted: bool,
    definitions: BTreeMap<Location, Vec<Definition>>,
    cache: BTreeMap<(Location, u8), Option<Inventory>>,
    commits: BTreeMap<Location, u32>,
    evaluating: bool,
    requested: Vec<(Location, u8)>,
    resource_free: bool,
    /// The locations that may hold a key, once computed (#753).
    nonempty: Option<BTreeSet<Location>>,
}
impl<'a> Evidence<'a> {
    pub fn new(program: &'a Program, function: &'a Function) -> Self {
        let commits = function.code.iter().enumerate().filter_map(|(pc, instruction)| match instruction {
            Instruction::Champ { op: Opcode::OwnCreateCommit, arguments, .. } => match arguments.first() {
                Some(Argument::Location(location)) => Some((*location, pc as u32)), _ => None,
            },
            _ => None,
        }).collect();
        Self { program, function, steps: 0, exhausted: false, definitions: definitions(function),
            cache: BTreeMap::new(), commits, evaluating: false, requested: vec![],
            resource_free: resource_free(program, function), nonempty: None }
    }

    /// The source of output `slot` of a `frame.resume` at `pc`; slot 0 is the frame token.
    fn resumed(&self, pc: usize, slot: usize) -> Option<Resumed> {
        let Instruction::Champ { op: Opcode::FrameResume, arguments: args, .. } = &self.function.code[pc] else {
            return None;
        };
        let continuation = suspension::continuation_of(args)?;
        if slot == 0 {
            return None;
        }
        if continuation == 0 {
            return Some(Resumed::Start);
        }
        let layout = self.program.frame_layout(self.function)?;
        let site = layout.continuations.get(continuation as usize)?.site? as usize;
        let save = self.function.code.get(site.checked_sub(1)?)?;
        if !matches!(save, Instruction::Champ { op: Opcode::FrameSave, .. }) {
            return None;
        }
        // The frame operand comes first, then the saved values in slot order, as the resume's outputs.
        arguments(save).get(slot).copied().map(Resumed::Saved)
    }

    /// Spend the whole step budget, so the next unresolved query exhausts it (tests of budget reporting).
    #[cfg(test)]
    pub fn spend_budget(&mut self) {
        self.steps = STEP_BUDGET;
    }

    pub fn check_budget(&self) -> Result<(), Error> {
        enforce(!self.exhausted, "LOAD_WORK_BUDGET", "evidence resolution exceeds step/depth budget")
    }

    pub fn identity(&mut self, location: Location) -> Option<(u32, Option<u32>)> {
        let inventory = self.resolve(location, true)?;
        if inventory.len() == 1 { inventory.first().copied() } else { None }
    }

    /// Whether no obligation can reach `location` (#753): a flow-insensitive closure over every definition of
    /// every location, so cycles (a packet carried around a loop through builder journals) do not defeat it as
    /// they defeat [`Self::inventory`], which needs one finite inventory. A location may hold a key when one of
    /// its definitions introduces one (a fresh call result, a committed resource, an opaque input or an
    /// unrecognised producer) or passes on a location that may hold one. It only proves emptiness: it never
    /// names a key, so it cannot certify a direct plan.
    pub fn provably_empty(&mut self, location: Location) -> bool {
        if self.inventory(location).is_some_and(|keys| keys.is_empty()) {
            return true;
        }
        // A location nothing defines holds whatever the frame left there.
        if !self.definitions.contains_key(&location) && !self.function.params.contains(&location) {
            return false;
        }
        if self.nonempty.is_none() {
            self.nonempty = Some(self.may_hold_keys());
        }
        self.nonempty.as_ref().is_some_and(|nonempty| !nonempty.contains(&location))
    }

    /// Every location that may hold a key, or every location when its own step budget runs out (the proof then
    /// fails, as before #753, without spending the budget of the inventory queries).
    fn may_hold_keys(&self) -> BTreeSet<Location> {
        let mut steps = 0usize;
        // For each location: whether a definition introduces keys, and the locations its definitions pass on.
        let mut sources = BTreeMap::<Location, Vec<Location>>::new();
        let mut seeds = BTreeSet::new();
        let definitions = &self.definitions;
        for (&location, list) in definitions {
            for &definition in list {
                steps += 1;
                match self.passes_on(definition) {
                    Some(inputs) => sources.entry(location).or_default().extend(inputs),
                    None => {
                        seeds.insert(location);
                    }
                }
            }
        }
        // A parameter without a definition: an opaque packet unless the inputs are plain; anything else unknown.
        for &location in &self.function.params {
            if definitions.contains_key(&location) {
                continue;
            }
            let packet = matches!(location, Location::Service(id)
                if self.function.services[id as usize] == ServiceKind::Packet);
            if !packet || !plain_inputs(self.program, self.function) {
                seeds.insert(location);
            }
        }
        let mut readers = BTreeMap::<Location, Vec<Location>>::new();
        for (&location, inputs) in &sources {
            for &input in inputs {
                readers.entry(input).or_default().push(location);
                // A location read without any definition (not a parameter) is unknown.
                if !definitions.contains_key(&input) && !self.function.params.contains(&input) {
                    seeds.insert(input);
                }
            }
        }
        let mut nonempty = seeds.clone();
        let mut work: Vec<_> = seeds.into_iter().collect();
        while let Some(location) = work.pop() {
            steps += 1;
            if steps > STEP_BUDGET {
                return definitions.keys().copied().chain(self.function.params.iter().copied())
                    .chain(sources.values().flatten().copied()).collect();
            }
            for &reader in readers.get(&location).into_iter().flatten() {
                if nonempty.insert(reader) {
                    work.push(reader);
                }
            }
        }
        nonempty
    }

    /// The locations whose keys one definition of `location` passes on, or `None` when it may introduce keys
    /// of its own. Mirrors the producers [`Self::inventory`] recognises; anything else introduces keys.
    fn passes_on(&self, definition: Definition) -> Option<Vec<Location>> {
        use Opcode::*;
        let code = &self.function.code;
        match definition {
            Definition::Incoming(_, _, Source::Location(source)) => Some(vec![source]),
            Definition::Incoming(pc, ordinal, Source::ProducedSlot(slot)) => {
                let Instruction::Champ { op, arguments: args, flow: Flow::Outcomes(outcomes) } = &code[pc] else {
                    return None;
                };
                let locations = arguments(&code[pc]);
                match (op, outcomes.get(ordinal)?.tag) {
                    (InvokeCallOwn, Tag::Normal) => {
                        let entry = args.iter().find_map(|arg| match arg {
                            Argument::Entry(id) => self.program.functions.get(*id as usize), _ => None,
                        })?;
                        plain_results(self.program, entry).then(Vec::new)
                    }
                    (InvokeDispatchOwn, Tag::Normal) => dispatch_plain_results(self.program, args).then(Vec::new),
                    (op, _) if journal_transition(*op) => Some(vec![*locations.last()?]),
                    (ScopeSplit | OwnSplitChecked, Tag::Normal) => Some(vec![*locations.first()?]),
                    (ScopeSplit | OwnSplitChecked, Tag::Unwind) if slot == 0 => Some(vec![*locations.first()?]),
                    (OwnDynamic, Tag::Normal) => Some(locations.iter().take(2).copied().collect()),
                    _ => None,
                }
            }
            Definition::Instruction(pc, slot) => {
                if self.resumed(pc, slot).is_some() {
                    return None;
                }
                let Instruction::Champ { op, arguments: args, .. } = &code[pc] else {
                    return None;
                };
                let locations = arguments(&code[pc]);
                match op {
                    OwnEmpty | ScopeBegin => Some(vec![]),
                    OwnCreateCommit => {
                        let object = args.iter().find_map(|arg| match arg {
                            Argument::Object(id) => Some(*id), _ => None,
                        })?;
                        (!self.program.objects[object as usize].layout.has_destroy).then(Vec::new)
                    }
                    BuilderAbandon => Some(vec![*locations.last()?]),
                    OwnMove | OwnJoin | OwnAdmit | OwnAccept | ExitExtend => Some(locations),
                    OwnBind | OwnRebind => Some(vec![locations[0], *locations.last()?]),
                    OwnStage if args.first() == Some(&Argument::Flag(false)) => {
                        Some(vec![locations[0], *locations.last()?])
                    }
                    OwnSplit => Some(vec![*locations.first()?]),
                    ExitPrepare => Some(vec![*locations.last()?]),
                    ExitCommit => Some(vec![*locations.first()?]),
                    _ => None,
                }
            }
        }
    }

    pub fn inventory(&mut self, location: Location) -> Option<Inventory> {
        self.resolve(location, false)
    }

    fn resolve(&mut self, location: Location, identity: bool) -> Option<Inventory> {
        self.query(location, u8::from(identity))
    }

    fn query(&mut self, location: Location, mode: u8) -> Option<Inventory> {
        let query = (location, mode);
        if let Some(result) = self.cache.get(&query) { return result.clone(); }
        if self.evaluating {
            self.requested.push(query);
            return None;
        }
        // Explicit DFS frames and memoization: a hostile move/phi chain never uses the process stack.
        let mut stack = vec![query];
        let mut active = BTreeSet::from([query]);
        while let Some(&(location, mode)) = stack.last() {
            self.steps += 1;
            if self.steps > STEP_BUDGET || stack.len() > 65_536 {
                self.exhausted = true;
                self.cache.insert(query, None);
                return None;
            }
            self.evaluating = true;
            self.requested.clear();
            let result = self.resolve_inner(location, mode);
            self.evaluating = false;
            if let Some(child) = self.requested.first().copied() {
                if active.insert(child) {
                    stack.push(child);
                } else {
                    self.cache.insert(child, None);
                }
            } else {
                self.cache.insert((location, mode), result);
                stack.pop();
                active.remove(&(location, mode));
            }
        }
        self.cache.get(&query).cloned().flatten()
    }

    pub fn flat_packet(&mut self, location: Location) -> bool {
        self.query(location, 3).is_some()
    }

    fn flat(&mut self, location: Location) -> Option<Inventory> {
        // An empty packet has no journals to classify, wherever it came from (loops included).
        if self.inventory(location).is_some_and(|keys| keys.is_empty()) {
            return Some(Inventory::new());
        }
        let definitions = self.definitions.get(&location).cloned().unwrap_or_default();
        if definitions.is_empty() {
            return self.inventory(location).filter(Inventory::is_empty);
        }
        for definition in definitions {
            match definition {
                Definition::Incoming(_, _, Source::Location(source)) => { self.query(source, 3)?; }
                Definition::Incoming(pc, ordinal, Source::ProducedSlot(_)) => {
                    let Instruction::Champ { op, flow: Flow::Outcomes(outcomes), .. }
                        = &self.function.code[pc] else { return None };
                    match (op, outcomes[ordinal].tag) {
                        (Opcode::InvokeCallOwn | Opcode::InvokeDispatchOwn, Tag::Normal) => {}
                        (Opcode::OwnSplitChecked | Opcode::ScopeSplit, Tag::Normal | Tag::Unwind) => {
                            let input = arguments(&self.function.code[pc])[0];
                            self.query(input, 3)?;
                        }
                        // The state and the transferred packet, joined or returned unaccepted.
                        (Opcode::OwnDynamic, Tag::Normal | Tag::Unwind) => {
                            for input in arguments(&self.function.code[pc]) { self.query(input, 3)?; }
                        }
                        (op, _) if journal_transition(*op) => {
                            let input = *arguments(&self.function.code[pc]).last()?;
                            self.query(input, 3)?;
                        }
                        _ => return None,
                    }
                }
                Definition::Instruction(pc, slot) => {
                    let Instruction::Champ { op, .. } = &self.function.code[pc] else { return None };
                    match self.resumed(pc, slot) {
                        Some(Resumed::Saved(source)) => {
                            self.query(source, 3)?;
                            continue;
                        }
                        Some(Resumed::Start) => return self.inventory(location).filter(Inventory::is_empty),
                        None => {}
                    }
                    match op {
                        Opcode::ExitCommit | Opcode::OwnCreateCommit | Opcode::OwnEmpty
                            | Opcode::InvokeCallOwn | Opcode::InvokeDispatchOwn => {}
                        Opcode::OwnMove | Opcode::OwnJoin | Opcode::OwnSplit => {
                            for input in arguments(&self.function.code[pc]) { self.query(input, 3)?; }
                        }
                        Opcode::BuilderAbandon => {
                            self.query(*arguments(&self.function.code[pc]).last()?, 3)?;
                        }
                        _ => return self.inventory(location).filter(Inventory::is_empty),
                    }
                }
            }
        }
        Some(Inventory::new())
    }

    fn resolve_inner(&mut self, location: Location, mode: u8) -> Option<Inventory> {
        if mode == 3 { return self.flat(location); }
        let identity = mode == 1;
        if !identity && matches!(verify::loc_type(self.function, location),
            Ok(ProducedType::Service(ServiceKind::Packet | ServiceKind::ScopeState | ServiceKind::Pending)))
            && self.resource_free
        {
            return Some(BTreeSet::new());
        }
        if mode == 2 {
            return self.admission(location);
        }
        let definitions = self.definitions.get(&location).cloned().unwrap_or_default();
        if definitions.is_empty() {
            if !identity && self.function.params.contains(&location)
                && let Location::Service(id) = location
                && self.function.services[id as usize] == ServiceKind::Packet
            {
                if plain_inputs(self.program, self.function) {
                    return Some(BTreeSet::new());
                }
                // Opaque input runs stay in the inventory. They can never certify a direct slot.
                return Some(BTreeSet::from([(u32::MAX, Some(id))]));
            }
            return None;
        }
        let mut result = None;
        for definition in definitions {
            let value = match definition {
                Definition::Incoming(_, _, Source::Location(source)) => self.resolve(source, identity)?,
                Definition::Incoming(pc, ordinal, Source::ProducedSlot(slot)) => {
                    self.produced(pc, ordinal, slot as usize, identity)?
                }
                Definition::Instruction(pc, slot) => self.instruction(pc, slot, location, identity)?,
            };
            if let Some(previous) = &result {
                if previous != &value {
                    return None;
                }
            } else {
                result = Some(value);
            }
        }
        result
    }

    fn instruction(&mut self, pc: usize, slot: usize, location: Location, identity: bool) -> Option<Inventory> {
        match self.resumed(pc, slot) {
            Some(Resumed::Saved(source)) => return self.resolve(source, identity),
            // A start value is an entry input: its packet is plain or opaque, as an owned entry's.
            Some(Resumed::Start) => {
                let Location::Service(id) = location else { return None };
                if identity || self.function.services[id as usize] != ServiceKind::Packet {
                    return None;
                }
                if plain_inputs(self.program, self.function) {
                    return Some(BTreeSet::new());
                }
                return Some(BTreeSet::from([(u32::MAX, Some(id))]));
            }
            None => {}
        }
        let instruction = &self.function.code[pc];
        if identity {
            if matches!(instruction, Instruction::Data { op: DataOp::ObjNew | DataOp::ObjNext, .. }) {
                let commit = *self.commits.get(&location)?;
                return Some(BTreeSet::from([(commit, None)]));
            }
            if let Instruction::Data { op: DataOp::Move, inputs, .. } = instruction {
                return self.resolve(Location::Register(inputs[0]), true);
            }
        }
        if let Some(callee) = callee(self.program, instruction) {
            return Some(BTreeSet::from([(pc as u32, Some(callee))]));
        }
        let Instruction::Champ { op, arguments: args, .. } = instruction else { return None };
        let locations = arguments(instruction);
        use Opcode::*;
        match op {
            InvokeCallOwn if !identity => {
                let entry = args.iter().find_map(|arg| match arg {
                    Argument::Entry(id) => self.program.functions.get(*id as usize), _ => None,
                })?;
                if plain_results(self.program, entry) { Some(BTreeSet::new()) } else { None }
            }
            InvokeDispatchOwn if !identity => dispatch_plain_results(self.program, args).then(BTreeSet::new),
            OwnEmpty | ScopeBegin => Some(BTreeSet::new()),
            BuilderAbandon if !identity => self.resolve(*locations.last()?, false),
            OwnCreateCommit => {
                let object = args.iter().find_map(|arg| match arg { Argument::Object(id) => Some(*id), _ => None })?;
                Some(if self.program.objects[object as usize].layout.has_destroy {
                    BTreeSet::from([(pc as u32, None)])
                } else { BTreeSet::new() })
            }
            OwnMove | OwnJoin | OwnAdmit | OwnAccept | OwnBind | OwnRebind | ExitExtend if !identity => {
                let operands = match op {
                    OwnBind | OwnRebind => vec![locations[0], *locations.last()?],
                    _ => locations,
                };
                let mut result = BTreeSet::new();
                for operand in operands {
                    let inventory = self.resolve(operand, false)?;
                    if inventory.iter().any(|key| result.contains(key)) {
                        return None;
                    }
                    result.extend(inventory);
                }
                Some(result)
            }
            OwnStage if !identity && args.first() == Some(&Argument::Flag(false)) => {
                let mut result = self.resolve(locations[0], false)?;
                let packet = self.resolve(*locations.last()?, false)?;
                if !result.is_disjoint(&packet) { return None; }
                result.extend(packet);
                Some(result)
            }
            OwnSplit => self.partition(args, slot == 1),
            ExitPrepare => self.resolve(if identity { *locations.first()? } else { *locations.last()? }, identity),
            ExitReplace if !identity => {
                let selected = self.partition(args, true)?;
                let mut replacement = self.resolve(*locations.last()?, false)?;
                if slot == 0 {
                    replacement.extend(selected);
                    Some(replacement)
                } else {
                    self.partition(args, false)
                }
            }
            ExitExtend if identity => self.resolve(locations[0], true),
            ExitCommit => self.resolve(locations[0], identity),
            // The owner state keeps its identities: the input packet moves into the Future, and the cache
            // duty is a service token of its own.
            FutureNew if !identity && slot == 2 => self.resolve(*locations.last()?, false),
            AwaitUnwind if !identity && slot == 0 => self.resolve(locations[0], false),
            _ => None,
        }
    }

    fn admission(&mut self, location: Location) -> Option<Inventory> {
        let definitions = self.definitions.get(&location).cloned().unwrap_or_default();
        if definitions.is_empty() {
            // Call-entry consumes fresh admission; accepted inputs are bound before parameter binding.
            return self.function.params.contains(&location).then(Inventory::new);
        }
        let mut result = None;
        for definition in definitions {
            let value = match definition {
                Definition::Incoming(_, _, Source::Location(source)) => self.query(source, 2)?,
                Definition::Incoming(pc, ordinal, Source::ProducedSlot(slot)) => {
                    let Instruction::Champ { op, arguments: args, flow: Flow::Outcomes(outcomes) }
                        = &self.function.code[pc] else { return None };
                    let locations = arguments(&self.function.code[pc]);
                    match (op, outcomes[ordinal].tag) {
                        (Opcode::ScopeSplit | Opcode::OwnSplitChecked, Tag::Normal) => {
                            let admission = self.query(locations[0], 2)?;
                            let picked = self.partition(args, true)?;
                            if slot == 1 { admission.intersection(&picked).copied().collect() }
                            else { admission.difference(&picked).copied().collect() }
                        }
                        (Opcode::ScopeSplit | Opcode::OwnSplitChecked, Tag::Unwind) if slot == 0 => {
                            self.query(locations[0], 2)?
                        }
                        (Opcode::OwnDynamic, Tag::Normal) => {
                            let mut joined = self.query(locations[0], 2)?;
                            joined.extend(self.query(locations[1], 2)?);
                            joined
                        }
                        (Opcode::OwnDynamic, Tag::Unwind) => self.query(*locations.get(slot as usize)?, 2)?,
                        (Opcode::InvokeCallOwn | Opcode::InvokeDispatchOwn, Tag::Normal) => {
                            self.produced(pc, ordinal, slot as usize, false)?
                        }
                        (Opcode::ExitDispatch, Tag::Pending) => self.produced(pc, ordinal, slot as usize, false)?,
                        (op, _) if journal_transition(*op) => self.query(*locations.last()?, 2)?,
                        _ => return None,
                    }
                }
                Definition::Instruction(pc, slot) => {
                    let Instruction::Champ { op, arguments: args, .. } = &self.function.code[pc] else {
                        return None;
                    };
                    let locations = arguments(&self.function.code[pc]);
                    match (op, self.resumed(pc, slot)) {
                        (_, Some(Resumed::Saved(source))) => return self.query(source, 2),
                        // Call-entry consumes fresh admission, as for an owned entry's packet parameter.
                        (_, Some(Resumed::Start)) => return Some(Inventory::new()),
                        (Opcode::FutureNew, _) if slot == 2 => return self.query(*locations.last()?, 2),
                        (Opcode::AwaitUnwind, _) if slot == 0 => return self.query(locations[0], 2),
                        _ => {}
                    }
                    match op {
                        Opcode::OwnEmpty | Opcode::ScopeBegin => Inventory::new(),
                        Opcode::OwnCreateCommit | Opcode::InvokeCallOwn | Opcode::InvokeDispatchOwn => {
                            self.instruction(pc, slot, location, false)?
                        }
                        Opcode::OwnMove | Opcode::OwnJoin | Opcode::OwnAdmit | Opcode::ExitExtend => {
                            let mut joined = Inventory::new();
                            for input in locations { joined.extend(self.query(input, 2)?); }
                            joined
                        }
                        Opcode::OwnAccept => self.query(locations[0], 2)?,
                        Opcode::BuilderAbandon => self.query(*locations.last()?, 2)?,
                        Opcode::OwnBind | Opcode::OwnRebind | Opcode::OwnStage
                            if *op != Opcode::OwnStage || args.first() == Some(&Argument::Flag(false)) => {
                            let mut admission = self.query(locations[0], 2)?;
                            // The runtime binds every identity the value reaches: its own, and the elements of
                            // a managed collection. Without a static inventory of them there is no admission.
                            let ProducedType::Value(ty) = verify::loc_type(self.function, locations[1]).ok()? else {
                                return None;
                            };
                            if verify::reaches_resource(self.program, ty) {
                                for key in self.selected_identities(locations[1])? {
                                    admission.remove(&key);
                                }
                            }
                            admission
                        }
                        Opcode::OwnSplit => {
                            let admission = self.query(locations[0], 2)?;
                            let picked = self.partition(args, true)?;
                            if slot == 1 { admission.intersection(&picked).copied().collect() }
                            else { admission.difference(&picked).copied().collect() }
                        }
                        Opcode::ExitPrepare => self.query(*locations.last()?, 2)?,
                        Opcode::ExitCommit => self.resolve(locations[0], false)?,
                        _ => return None,
                    }
                }
            };
            if result.as_ref().is_some_and(|previous| previous != &value) { return None; }
            result = Some(value);
        }
        result
    }

    fn partition(&mut self, args: &[Argument], selected: bool) -> Option<Inventory> {
        let input = args.iter().find_map(|arg| match *arg {
            Argument::Location(location) => Some(location), _ => None,
        })?;
        let selection = args.iter().find_map(|arg| match *arg {
            Argument::Selection(id) => self.function.proofs.selections.get(id as usize), _ => None,
        })?;
        // Selecting through an external borrow cannot introduce obligations into an empty holder.
        if self.resolve(input, false).is_some_and(|keys| keys.is_empty()) {
            return Some(BTreeSet::new());
        }
        let mut keys = Inventory::new();
        for &location in &selection.locations {
            keys.extend(self.selected_identities(location)?);
        }
        if selected && keys.is_empty() {
            return Some(keys);
        }
        let all = self.resolve(input, false)?;
        let eligible = if args.contains(&Argument::Flag(true)) { self.query(input, 2)? } else { all.clone() };
        if args.contains(&Argument::Flag(true)) {
            keys = keys.intersection(&eligible).copied().collect();
        }
        let input = all;
        if !keys.is_subset(&input) {
            return None;
        }
        Some(if selected { keys } else { input.difference(&keys).copied().collect() })
    }

    /// The identities a selected value names: none for plain data, its own for a resource object, and
    /// for a managed array or map those of its elements. A collection whose elements are not proved by
    /// the `array.new` or `map.new` that built it has no static inventory, so a selection never loses them.
    pub fn selected_identities(&mut self, location: Location) -> Option<Inventory> {
        let ty = match verify::loc_type(self.function, location).ok()? {
            ProducedType::Value(ty) => ty,
            ProducedType::Service(_) => return None,
        };
        match ty {
            // A case object refers to resources without being an identity: no static inventory.
            Type::Variant(_) if verify::reaches_resource(self.program, ty) => None,
            Type::Object(_) | Type::Successor(_) | Type::Array(_) | Type::Map(_)
                if verify::reaches_resource(self.program, ty) =>
            {
                if !matches!(ty, Type::Array(_) | Type::Map(_)) {
                    return Some(BTreeSet::from([self.identity(location)?]));
                }
                let mut keys = Inventory::new();
                for element in self.collection_elements(location)? {
                    keys.extend(self.selected_identities(element)?);
                }
                Some(keys)
            }
            // A role value is its object: a managed one is selected by its identity.
            Type::Role(id) | Type::RoleSuccessor(id) if verify::role_reaches_resource(self.program, id) => {
                Some(BTreeSet::from([self.identity(location)?]))
            }
            // An `any` value is its box: a managed one is selected by its identity.
            Type::Any if verify::any_reaches_resource(self.program) => Some(BTreeSet::from([self.identity(location)?])),
            Type::Object(_) | Type::Successor(_) | Type::Int | Type::Bool | Type::Str | Type::Bytes | Type::Array(_)
                | Type::Range | Type::Descriptor | Type::Byte | Type::Float | Type::Char | Type::Rat
                | Type::Variant(_) | Type::Handle(_) | Type::Map(_) | Type::Role(_) | Type::RoleSuccessor(_)
                | Type::Any => {
                Some(Inventory::new())
            }
            _ => None,
        }
    }

    /// The element locations of the single `array.new` (or the values of the single `map.new`), through
    /// moves, that defines `location`.
    fn collection_elements(&mut self, location: Location) -> Option<Vec<Location>> {
        let mut current = location;
        for _ in 0..65_536 {
            self.steps += 1;
            if self.steps > 1_000_000 {
                self.exhausted = true;
                return None;
            }
            let [Definition::Instruction(pc, _)] = self.definitions.get(&current)?.as_slice() else { return None };
            match &self.function.code[*pc] {
                Instruction::Data { op: DataOp::ArrayNew, inputs, .. } => {
                    return Some(inputs.iter().map(|&id| Location::Register(id)).collect());
                }
                Instruction::Data { op: DataOp::MapNew, inputs, .. } => {
                    // A repeated key keeps only its last value: the values are the map's elements only when
                    // the keys are pairwise distinct constants.
                    let mut keys = BTreeSet::new();
                    for &key in inputs.iter().step_by(2) {
                        self.steps += 1;
                        let key = self.key_constant(Location::Register(key))?;
                        if !keys.insert(key) {
                            return None;
                        }
                    }
                    return Some(inputs.iter().skip(1).step_by(2).map(|&id| Location::Register(id)).collect());
                }
                Instruction::Data { op: DataOp::Move, inputs, .. } => current = Location::Register(inputs[0]),
                _ => return None,
            }
        }
        self.exhausted = true;
        None
    }

    /// The value of a map key defined by a single constant, in a form where equal keys are equal: constants
    /// are canonical, except that the two float zeros are one key.
    fn key_constant(&self, location: Location) -> Option<String> {
        let [Definition::Instruction(pc, _)] = self.definitions.get(&location)?.as_slice() else { return None };
        let instruction = &self.function.code[*pc];
        let Instruction::Data { op: DataOp::Const, immediate: Immediate::Constant(value), .. } = instruction else {
            return None;
        };
        if let Constant::Float(bits) = value
            && bits.parse::<u64>().ok().map(f64::from_bits) == Some(0.0)
        {
            return Some(format!("{:?}", Constant::Float("0".into())));
        }
        Some(format!("{value:?}"))
    }

    fn cleanup_begins(&mut self, location: Location) -> Option<BTreeSet<usize>> {
        let mut work = vec![location];
        let mut seen = BTreeSet::new();
        let mut begins = BTreeSet::new();
        let mut steps = 0;
        while let Some(location) = work.pop() {
            steps += 1;
            if steps > 1_000_000 {
                self.exhausted = true;
                return None;
            }
            if !seen.insert(location) { continue; }
            for definition in self.definitions.get(&location).into_iter().flatten() {
                match *definition {
                    Definition::Incoming(_, _, Source::Location(source)) => work.push(source),
                    Definition::Instruction(pc, _) | Definition::Incoming(pc, _, Source::ProducedSlot(_)) => {
                        let instruction = &self.function.code[pc];
                        if matches!(instruction, Instruction::Champ { op: Opcode::CleanupBegin, .. }) {
                            begins.insert(pc);
                        } else {
                            work.extend(arguments(instruction).into_iter().filter(|&source| {
                                matches!(verify::loc_type(self.function, source),
                                    Ok(ProducedType::Service(ServiceKind::Cleanup)))
                            }));
                        }
                    }
                }
            }
        }
        Some(begins)
    }

    fn produced(&mut self, pc: usize, ordinal: usize, slot: usize, identity: bool) -> Option<Inventory> {
        let Instruction::Champ { op, arguments: args, flow: Flow::Outcomes(outcomes) } = &self.function.code[pc]
            else { return None };
        let tag = outcomes.get(ordinal)?.tag;
        if *op == Opcode::InvokeCallOwn && tag == Tag::Normal {
            let entry = args.iter().find_map(|arg| match arg {
                Argument::Entry(id) => self.program.functions.get(*id as usize), _ => None,
            })?;
            if !identity && plain_results(self.program, entry) {
                return Some(BTreeSet::new());
            }
            let callee = callee(self.program, &self.function.code[pc])?;
            return Some(BTreeSet::from([(pc as u32, Some(callee))]));
        }
        if *op == Opcode::InvokeDispatchOwn && tag == Tag::Normal {
            // No implementation's fresh result certifies a direct slot: the inventory is opaque unless plain.
            return (!identity && dispatch_plain_results(self.program, args)).then(BTreeSet::new);
        }
        if *op == Opcode::ExitDispatch && tag == Tag::Pending && slot == 1 {
            let exit = arguments(&self.function.code[pc])[2];
            let begins = self.cleanup_begins(exit)?;
            if begins.len() != 1 { return None }
            let begin = *begins.first()?;
            let locations = arguments(&self.function.code[begin]);
            return self.resolve(*locations.get(1)?, identity);
        }
        if identity {
            return None;
        }
        if journal_transition(*op) {
            // The journal packet leaves every outcome of a builder transition as it entered.
            let input = *arguments(&self.function.code[pc]).last()?;
            return self.resolve(input, false);
        }
        match (op, tag) {
            (Opcode::ScopeSplit | Opcode::OwnSplitChecked, Tag::Normal) => self.partition(args, slot == 1),
            (Opcode::ScopeSplit | Opcode::OwnSplitChecked, Tag::Unwind) if slot == 0 => {
                self.resolve(arguments(&self.function.code[pc])[0], false)
            }
            // The transferred packet joins the scope inventory; a failed check returns both unchanged.
            (Opcode::OwnDynamic, Tag::Normal) => {
                let locations = arguments(&self.function.code[pc]);
                let mut joined = self.resolve(locations[0], false)?;
                let packet = self.resolve(locations[1], false)?;
                if !joined.is_disjoint(&packet) { return None; }
                joined.extend(packet);
                Some(joined)
            }
            (Opcode::OwnDynamic, Tag::Unwind) if slot < 2 => {
                self.resolve(arguments(&self.function.code[pc])[slot], false)
            }
            _ => None,
        }
    }
}

/// Uses the actual CFG, not PC ordering or SSA numbering. A path to later that avoids earlier disproves dominance.
pub(super) fn precedes(f: &Function, earlier: u32, later: u32) -> bool {
    if earlier == later {
        return false;
    }
    let mut work = vec![0usize];
    let mut seen = BTreeSet::new();
    while let Some(pc) = work.pop() {
        if pc == earlier as usize || !seen.insert(pc) {
            continue;
        }
        if pc == later as usize {
            return false;
        }
        let Some(inst) = f.code.get(pc) else { return false };
        let edges = allocation::outgoing(inst);
        work.extend(edges.iter().map(|edge| edge.target_pc as usize));
        if matches!(inst, Instruction::Data { .. } | Instruction::Champ { flow: Flow::Next(_), .. })
            && !matches!(inst, Instruction::Champ { op: Opcode::RetOwn | Opcode::RetDefer, .. })
        {
            work.push(pc + 1);
        }
    }
    true
}

/// P1 static helpers take packet positions, not runtime traversal roots.
/// Empty selections and proved singleton packets need no storage-order inference.
pub fn static_selections(p: &Program, f: &Function) -> Result<Vec<StaticSelection>, Error> {
    let mut evidence = Evidence::new(p, f);
    let result = static_selections_with(p, f, &mut evidence)?;
    evidence.check_budget()?;
    Ok(result)
}

pub(super) fn static_selections_with(_p: &Program, f: &Function, evidence: &mut Evidence<'_>)
    -> Result<Vec<StaticSelection>, Error>
{
    let mut result = Vec::new();
    for (pc, instruction) in f.code.iter().enumerate() {
        let Instruction::Champ { op: Opcode::OwnSplit | Opcode::ExitReplace, arguments: args, .. } = instruction
            else { continue };
        let locations = arguments(instruction);
        let selection = args.iter().find_map(|arg| match *arg {
            Argument::Selection(id) => Some(id), _ => None,
        }).ok_or_else(|| error("missing static selection"))?;
        if evidence.inventory(locations[0]).is_some_and(|keys| keys.is_empty()) {
            result.push(StaticSelection { pc: pc as u32, selection, input: locations[0], indices: vec![] });
            continue;
        }
        let mut keys = Inventory::new();
        for &location in &f.proofs.selections[selection as usize].locations {
            let static_value = matches!(verify::loc_type(f, location), Ok(ProducedType::Value(
                Type::Object(_) | Type::Bool | Type::Int | Type::Str | Type::Descriptor | Type::Bytes | Type::Range
                | Type::Array(_) | Type::Byte | Type::Float | Type::Char | Type::Rat | Type::Variant(_)
                | Type::Handle(_) | Type::Role(_) | Type::RoleSuccessor(_) | Type::Any)));
            if !static_value {
                return Err(error("runtime traversal cannot be used as a static selection"));
            }
            keys.extend(evidence.selected_identities(location)
                .ok_or_else(|| error("unknown static selection identity"))?);
        }
        let indices = if keys.is_empty() {
            vec![]
        } else {
            let input = evidence.inventory(locations[0]).ok_or_else(|| error("unknown static packet inventory"))?;
            if input != keys || input.iter().any(|key| key.0 == u32::MAX) {
                return Err(error("static partial selection needs a proved packet-position layout"));
            }
            // Whole finite inventories are independent of the runtime's admission/bound/staging run layout.
            (0..input.len() as u32).collect()
        };
        result.push(StaticSelection { pc: pc as u32, selection, input: locations[0], indices });
    }
    Ok(result)
}

#[cfg(test)]
#[path = "evidence_tests.rs"]
mod tests;
