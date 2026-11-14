//! Independent finite inventory and creation-origin reconstruction for the loader.
//! Cycles/unknown inventories cannot establish a compiled direct plan.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

type Inventory = BTreeSet<(u32, Option<u32>)>;

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

pub(super) struct Evidence<'a> {
    program: &'a Program,
    function: &'a Function,
    steps: usize,
    exhausted: bool,
    definitions: BTreeMap<Location, Vec<Definition>>,
    cache: BTreeMap<(Location, bool), Option<Inventory>>,
    commits: BTreeMap<Location, u32>,
    evaluating: bool,
    requested: Vec<(Location, bool)>,
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
            cache: BTreeMap::new(), commits, evaluating: false, requested: vec![] }
    }

    pub fn check_budget(&self) -> Result<(), Error> {
        enforce(!self.exhausted, "LOAD_WORK_BUDGET", "evidence resolution exceeds step/depth budget")
    }

    pub fn identity(&mut self, location: Location) -> Option<(u32, Option<u32>)> {
        let inventory = self.resolve(location, true)?;
        if inventory.len() == 1 { inventory.first().copied() } else { None }
    }

    pub fn inventory(&mut self, location: Location) -> Option<Inventory> {
        self.resolve(location, false)
    }

    fn resolve(&mut self, location: Location, identity: bool) -> Option<Inventory> {
        let query = (location, identity);
        if let Some(result) = self.cache.get(&query) { return result.clone(); }
        if self.evaluating {
            self.requested.push(query);
            return None;
        }
        // Explicit DFS frames and memoization: a hostile move/phi chain never uses the process stack.
        let mut stack = vec![query];
        let mut active = BTreeSet::from([query]);
        while let Some(&(location, identity)) = stack.last() {
            self.steps += 1;
            if self.steps > 1_000_000 || stack.len() > 65_536 {
                self.exhausted = true;
                self.cache.insert(query, None);
                return None;
            }
            self.evaluating = true;
            self.requested.clear();
            let result = self.resolve_inner(location, identity);
            self.evaluating = false;
            if let Some(child) = self.requested.first().copied() {
                if active.insert(child) {
                    stack.push(child);
                } else {
                    self.cache.insert(child, None);
                }
            } else {
                self.cache.insert((location, identity), result);
                stack.pop();
                active.remove(&(location, identity));
            }
        }
        self.cache.get(&query).cloned().flatten()
    }

    fn resolve_inner(&mut self, location: Location, identity: bool) -> Option<Inventory> {
        let definitions = self.definitions.get(&location).cloned().unwrap_or_default();
        if definitions.is_empty() {
            if !identity && self.function.params.contains(&location)
                && let Location::Service(id) = location
                && self.function.services[id as usize] == ServiceKind::Packet
            {
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
        let instruction = &self.function.code[pc];
        if identity {
            if matches!(instruction, Instruction::Data { op: DataOp::ObjNew, .. }) {
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
            OwnEmpty | ScopeBegin => Some(BTreeSet::new()),
            OwnCreateCommit => Some(BTreeSet::from([(pc as u32, None)])),
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
            OwnSplit => self.partition(args, slot == 1),
            ExitPrepare => self.resolve(if identity { *locations.first()? } else { *locations.last()? }, identity),
            ExitExtend if identity => self.resolve(locations[0], true),
            ExitCommit => self.resolve(locations[0], identity),
            _ => None,
        }
    }

    fn partition(&mut self, args: &[Argument], selected: bool) -> Option<Inventory> {
        let input = args.iter().find_map(|arg| match *arg {
            Argument::Location(location) => Some(location), _ => None,
        })?;
        let selection = args.iter().find_map(|arg| match *arg {
            Argument::Selection(id) => self.function.proofs.selections.get(id as usize), _ => None,
        })?;
        let input = self.resolve(input, false)?;
        let mut keys = Inventory::new();
        for &location in &selection.locations {
            keys.insert(self.identity(location)?);
        }
        if !keys.is_subset(&input) {
            return None;
        }
        Some(if selected { keys } else { input.difference(&keys).copied().collect() })
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
            let callee = callee(self.program, &self.function.code[pc])?;
            return Some(BTreeSet::from([(pc as u32, Some(callee))]));
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
        match (op, tag) {
            (Opcode::ScopeSplit | Opcode::OwnSplitChecked, Tag::Normal) => self.partition(args, slot == 1),
            (Opcode::ScopeSplit | Opcode::OwnSplitChecked, Tag::Unwind) if slot == 0 => {
                self.resolve(arguments(&self.function.code[pc])[0], false)
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

pub(super) fn static_selections_with(p: &Program, f: &Function, evidence: &mut Evidence<'_>)
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
        let mut keys = Inventory::new();
        for &location in &f.proofs.selections[selection as usize].locations {
            if let Ok(ProducedType::Value(Type::Object(id))) = verify::loc_type(f, location) {
                if !p.objects[id as usize].layout.may_reach_resource { continue }
                keys.insert(evidence.identity(location).ok_or_else(|| error("unknown static selection identity"))?);
            } else if matches!(verify::loc_type(f, location),
                Ok(ProducedType::Value(Type::Bool | Type::Int | Type::Str | Type::Descriptor)))
            {
                continue;
            } else {
                return Err(error("runtime traversal cannot be used as a static selection"));
            }
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
