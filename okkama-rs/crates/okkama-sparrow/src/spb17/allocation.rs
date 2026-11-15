//! Virtual locations stay SSA in SPB. This checked plan assigns typed frame slots and lowers
//! parallel edges to sequential copies. Post slots are read only on their selected edge.
use std::collections::{BTreeMap, BTreeSet};
use super::*;

type Set = BTreeSet<Location>;

pub fn location_type(f: &Function, location: Location) -> Result<ProducedType, Error> {
    match location {
        Location::Register(id) => f.registers.get(id as usize).copied().map(ProducedType::Value),
        Location::Service(id) => f.services.get(id as usize).copied().map(ProducedType::Service),
    }.ok_or_else(|| error("allocation: invalid location"))
}

pub fn outgoing(inst: &Instruction) -> Vec<&Edge> {
    match inst {
        Instruction::Br(edge) => vec![edge],
        Instruction::Cond { yes, no, .. } => vec![yes, no],
        Instruction::Champ { flow: Flow::Outcomes(outcomes), .. } => outcomes.iter().map(|o| &o.edge).collect(),
        _ => vec![],
    }
}

fn definitions(inst: &Instruction) -> Vec<Location> {
    match inst {
        Instruction::Data { output, .. } => output.iter().copied().map(Location::Register).collect(),
        Instruction::Champ { flow: Flow::Next(results), .. } => results.clone(),
        _ => vec![],
    }
}

pub(crate) fn direct_uses(f: &Function, pc: usize) -> Set {
    let mut used = Set::new();
    match &f.code[pc] {
        Instruction::Data { inputs, .. } | Instruction::Ret(inputs) => {
            used.extend(inputs.iter().copied().map(Location::Register));
        }
        Instruction::Cond { condition, .. } => { used.insert(Location::Register(*condition)); }
        Instruction::Raise(value) => { used.insert(Location::Register(*value)); }
        Instruction::Champ { arguments, .. } => {
            for arg in arguments {
                match *arg {
                    Argument::Location(location) => { used.insert(location); }
                    Argument::Selection(id) => used.extend(&f.proofs.selections[id as usize].locations),
                    Argument::Plan(id) => {
                        let selection = f.proofs.plans[id as usize].selection;
                        used.extend(&f.proofs.selections[selection as usize].locations);
                    }
                    // A nonempty schedule is settled duty by duty, each duty an explicit operand (SPB_SCHEDULE);
                    // its list is proof data and keeps no settled duty alive.
                    _ => {}
                }
            }
        }
        _ => {}
    }
    used
}

pub(crate) fn metadata_uses(f: &Function) -> Vec<Set> {
    let mut sites = vec![Set::new(); f.code.len()];
    for root in &f.proofs.roots { sites[root.pc as usize].extend(&root.locations); }
    for state in &f.proofs.states { sites[state.pc as usize].extend(&state.locations); }
    sites
}

fn post_uses(f: &Function) -> Vec<Set> {
    let mut sites = vec![Set::new(); f.code.len()];
    for root in &f.proofs.post_roots { sites[root.pc as usize].extend(&root.locations); }
    sites
}

pub(crate) fn close_dependencies(f: &Function, live: &mut Set) {
    loop {
        let previous = live.len();
        for dependency in &f.proofs.dependencies {
            if live.contains(&dependency.value) {
                live.extend(&dependency.obligations);
            }
        }
        if live.len() == previous {
            break;
        }
    }
}

fn terminal(inst: &Instruction) -> bool {
    !matches!(inst, Instruction::Data { .. } | Instruction::Champ { flow: Flow::Next(_), .. })
        || matches!(inst, Instruction::Champ { op: Opcode::RetOwn | Opcode::RetDefer, .. })
}

/// Exact CFG fixed point with simultaneous edge renames. Every outcome, all service results,
/// dependency leaves, ownership states and root requirements participate, including zero-word deps.
pub fn live_locations(f: &Function) -> Result<Vec<Set>, Error> {
    let count = f.registers.len().saturating_add(f.services.len());
    if count.saturating_add(f.code.len()) > 8 * 1024 * 1024 {
        return Err(error("allocation liveness budget exceeded"));
    }
    let metadata = metadata_uses(f);
    let post = post_uses(f);
    let mut before = vec![Set::new(); f.code.len()];
    let mut entries = 0usize;
    let mut work = 0usize;
    loop {
        let mut changed = false;
        for pc in (0..f.code.len()).rev() {
            let inst = &f.code[pc];
            let mut live = Set::new();
            for edge in outgoing(inst) {
                let mut next = before.get(edge.target_pc as usize)
                    .ok_or_else(|| error("allocation: invalid edge PC"))?.clone();
                for assignment in &edge.assignments {
                    next.remove(&assignment.destination);
                }
                for assignment in &edge.assignments {
                    if let Source::Location(location) = assignment.source {
                        next.insert(location);
                    }
                }
                live.extend(next);
            }
            if !terminal(inst) && let Some(next) = before.get(pc + 1) {
                live.extend(next);
            }
            for definition in definitions(inst) {
                live.remove(&definition);
            }
            live.extend(&post[pc]);
            for definition in definitions(inst) {
                live.remove(&definition);
            }
            live.extend(direct_uses(f, pc));
            live.extend(&metadata[pc]);
            close_dependencies(f, &mut live);
            work = work.saturating_add(live.len().max(1));
            enforce(work <= 50_000_000, "ALLOCATION_WORK", "allocation liveness work budget exceeded")?;
            if before[pc] != live {
                entries = entries - before[pc].len() + live.len();
                enforce(entries <= 8 * 1024 * 1024, "ALLOCATION_MEMORY", "allocation liveness storage exceeded")?;
                before[pc] = live;
                changed = true;
            }
        }
        if !changed {
            return Ok(before);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopySource {
    Slot(u32),
    Produced(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Copy {
    pub destination: u32,
    pub source: CopySource,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Allocation {
    pub locations: BTreeMap<Location, u32>,
    /// Typed frame storage. Deps entries describe proof positions and occupy zero machine words.
    pub slots: Vec<ProducedType>,
    /// (instruction PC, outgoing edge ordinal) → sequential copies, including typed cycle scratch.
    pub edges: BTreeMap<(u32, u32), Vec<Copy>>,
    /// (PC, after instruction) → traced words in typed frame slots. No Post belongs to a call map.
    pub roots: BTreeMap<(u32, bool), Vec<AllocatedRoot>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct AllocatedRoot {
    pub slot: u32,
    pub offset: u32,
    pub tagged_int: bool,
}

fn clique(graph: &mut BTreeMap<Location, Set>, locations: impl IntoIterator<Item = Location>,
    edges: &mut usize) -> Result<(), Error>
{
    let locations = locations.into_iter().collect::<Set>();
    enforce(locations.len().saturating_mul(locations.len().saturating_sub(1)) <= 8 * 1024 * 1024,
        "ALLOCATION_MEMORY", "interference clique exceeds storage budget")?;
    for &location in &locations {
        let neighbours = graph.entry(location).or_default();
        let before = neighbours.len();
        neighbours.extend(locations.iter().copied().filter(|&other| other != location));
        *edges = edges.saturating_add(neighbours.len() - before);
        enforce(*edges <= 8 * 1024 * 1024, "ALLOCATION_MEMORY", "interference graph exceeds storage budget")?;
    }
    Ok(())
}

pub fn allocate(f: &Function) -> Result<Allocation, Error> {
    let live = live_locations(f)?;
    let post = post_uses(f);
    let mut graph = BTreeMap::new();
    let mut edges = 0;
    // Entry adapters store every parameter before their first collecting call, including externals.
    clique(&mut graph, f.params.iter().copied(), &mut edges)?;
    for set in &live {
        clique(&mut graph, set.iter().copied(), &mut edges)?;
    }
    for block in &f.blocks {
        clique(&mut graph, block.params.iter().copied().chain(live[block.pc as usize].iter().copied()), &mut edges)?;
    }
    for (pc, inst) in f.code.iter().enumerate() {
        if !terminal(inst) {
            clique(&mut graph, definitions(inst).into_iter().chain(post[pc].iter().copied())
                .chain(live.get(pc + 1).into_iter().flatten().copied()), &mut edges)?;
            clique(&mut graph, definitions(inst).into_iter()
                .chain(live.get(pc + 1).into_iter().flatten().copied()), &mut edges)?;
        }
    }
    let mut allocation = Allocation {
        locations: BTreeMap::new(), slots: vec![], edges: BTreeMap::new(), roots: BTreeMap::new(),
    };
    let locations = (0..f.registers.len() as u32).map(Location::Register)
        .chain((0..f.services.len() as u32).map(Location::Service));
    for location in locations {
        let ty = location_type(f, location)?;
        let busy = graph.get(&location).into_iter().flatten()
            .filter_map(|other| allocation.locations.get(other).copied()).collect::<BTreeSet<_>>();
        let slot = allocation.slots.iter().enumerate().find_map(|(index, &kind)| {
            (kind == ty && !busy.contains(&(index as u32))).then_some(index as u32)
        }).unwrap_or(allocation.slots.len() as u32);
        if slot as usize == allocation.slots.len() {
            allocation.slots.push(ty);
        }
        allocation.locations.insert(location, slot);
    }
    for (pc, inst) in f.code.iter().enumerate() {
        for (ordinal, edge) in outgoing(inst).into_iter().enumerate() {
            let mut pending = edge.assignments.iter().map(|assignment| Copy {
                destination: allocation.locations[&assignment.destination],
                source: match assignment.source {
                    Source::Location(location) => CopySource::Slot(allocation.locations[&location]),
                    Source::ProducedSlot(slot) => CopySource::Produced(slot),
                },
            }).filter(|copy| copy.source != CopySource::Slot(copy.destination)).collect::<Vec<_>>();
            let mut copies = Vec::new();
            while !pending.is_empty() {
                let ready = pending.iter().position(|copy| !pending.iter()
                    .any(|other| other.source == CopySource::Slot(copy.destination)));
                if let Some(index) = ready {
                    copies.push(pending.remove(index));
                } else {
                    let source = pending[0].destination;
                    let scratch = allocation.slots.len() as u32;
                    allocation.slots.push(allocation.slots[source as usize]);
                    copies.push(Copy { destination: scratch, source: CopySource::Slot(source) });
                    for copy in &mut pending {
                        if copy.source == CopySource::Slot(source) {
                            copy.source = CopySource::Slot(scratch);
                        }
                    }
                }
            }
            allocation.edges.insert((pc as u32, ordinal as u32), copies);
        }
    }
    allocation.roots = allocated_roots(f, &allocation)?;
    Ok(allocation)
}

/// Independent forward symbolic execution: no interference graph or allocator liveness is trusted.
/// Joins intersect known contents; loops run to a fixed point. Copies execute sequentially and are
/// compared against the original parallel edge, so stale, clobbered and wrong-outcome values fail.
pub fn check_allocation(f: &Function, allocation: &Allocation) -> Result<(), Error> {
    if allocation.roots != allocated_roots(f, allocation)? {
        return Err(error("allocation clobbered concrete roots"));
    }
    type State = Vec<BTreeSet<Location>>;
    let slot = |location: Location| -> Result<usize, Error> {
        let index = *allocation.locations.get(&location).ok_or_else(|| error("allocation missing location"))?;
        if allocation.slots.get(index as usize) != Some(&location_type(f, location)?) {
            return Err(error("allocation slot type mismatch"));
        }
        Ok(index as usize)
    };
    for location in (0..f.registers.len() as u32).map(Location::Register)
        .chain((0..f.services.len() as u32).map(Location::Service))
    {
        slot(location)?;
    }
    let empty = || vec![Set::new(); allocation.slots.len()];
    let mut entry = empty();
    for &param in &f.params {
        let index = slot(param)?;
        if !entry[index].is_empty() {
            return Err(error("allocation clobbers entry parameters"));
        }
        entry[index].insert(param);
    }
    let mut incoming: Vec<Option<State>> = vec![None; f.code.len()];
    if incoming.is_empty() {
        if f.proofs.entry.as_ref().is_some_and(|entry| entry.external.is_some()) {
            return Ok(());
        }
        return Err(error("allocation requires code"));
    }
    let size = |state: &State| state.len() + state.iter().map(Set::len).sum::<usize>();
    let mut stored = size(&entry);
    let mut visits = 0usize;
    incoming[0] = Some(entry);
    let metadata = metadata_uses(f);
    let post = post_uses(f);
    let mut work = vec![0];
    while let Some(pc) = work.pop() {
        let mut state = incoming[pc].as_ref().ok_or_else(|| error("allocation worklist state"))?.clone();
        visits = visits.saturating_add(size(&state));
        enforce(visits <= 50_000_000, "ALLOCATION_WORK", "allocation checker work budget exceeded")?;
        let mut used = direct_uses(f, pc);
        used.extend(&metadata[pc]);
        // This is a forward availability check, independent of the backward live set computation.
        for edge in outgoing(&f.code[pc]) {
            for assignment in &edge.assignments {
                if let Source::Location(location) = assignment.source {
                    used.insert(location);
                }
            }
        }
        close_dependencies(f, &mut used);
        for location in used {
            if !state[slot(location)?].contains(&location) {
                return Err(error(format!("allocation clobbered {location:?} at pc {pc}")));
            }
        }
        for definition in definitions(&f.code[pc]) {
            for contents in &mut state {
                contents.remove(&definition);
            }
            state[slot(definition)?] = Set::from([definition]);
        }
        for &location in &post[pc] {
            if !state[slot(location)?].contains(&location) {
                return Err(error(format!("allocation clobbered post root at pc {pc}")));
            }
        }
        let mut successors = Vec::new();
        for (ordinal, edge) in outgoing(&f.code[pc]).into_iter().enumerate() {
            visits = visits.saturating_add(size(&state));
            enforce(visits <= 50_000_000, "ALLOCATION_WORK", "allocation edge work budget exceeded")?;
            let mut values = state.iter().map(|locations| locations.iter().copied()
                .map(Source::Location).collect::<Vec<_>>()).collect::<Vec<_>>();
            let copies = allocation.edges.get(&(pc as u32, ordinal as u32))
                .ok_or_else(|| error("allocation missing edge copies"))?;
            for copy in copies {
                let value = match copy.source {
                    CopySource::Slot(index) => values.get(index as usize).cloned()
                        .ok_or_else(|| error("allocation invalid copy source"))?,
                    CopySource::Produced(index) => vec![Source::ProducedSlot(index)],
                };
                let destination = values.get_mut(copy.destination as usize)
                    .ok_or_else(|| error("allocation invalid copy destination"))?;
                *destination = value;
            }
            // Check every destination against the pre-edge value; no assignment-order assumptions.
            for assignment in &edge.assignments {
                if !values[slot(assignment.destination)?].contains(&assignment.source) {
                    return Err(error(format!("allocation clobbered parallel assignment at pc {pc}")));
                }
            }
            let mut next = empty();
            for (index, contents) in values.into_iter().enumerate() {
                for source in contents {
                    if let Source::Location(location) = source {
                        next[index].insert(location);
                    }
                }
            }
            // SSA parameters are renamed simultaneously after checking all pre-edge sources.
            for assignment in &edge.assignments {
                for contents in &mut next {
                    contents.remove(&assignment.destination);
                }
            }
            for assignment in &edge.assignments {
                next[slot(assignment.destination)?].insert(assignment.destination);
            }
            successors.push((edge.target_pc as usize, next));
        }
        if !terminal(&f.code[pc]) && pc + 1 < f.code.len() {
            successors.push((pc + 1, state));
        }
        for (target, next) in successors {
            let previous = incoming.get_mut(target).ok_or_else(|| error("allocation invalid target"))?;
            if let Some(previous) = previous {
                let mut changed = false;
                for (contents, other) in previous.iter_mut().zip(next) {
                    let intersection = contents.intersection(&other).copied().collect::<Set>();
                    changed |= *contents != intersection;
                    *contents = intersection;
                }
                if changed {
                    work.push(target);
                }
            } else {
                stored = stored.saturating_add(size(&next));
                enforce(stored <= 8 * 1024 * 1024, "ALLOCATION_MEMORY", "allocation checker storage exceeded")?;
                *previous = Some(next);
                work.push(target);
            }
        }
    }
    Ok(())
}

fn allocated_roots(f: &Function, allocation: &Allocation)
    -> Result<BTreeMap<(u32, bool), Vec<AllocatedRoot>>, Error>
{
    let mut maps: BTreeMap<_, BTreeSet<_>> = BTreeMap::new();
    for (after, requirements) in [(false, &f.proofs.roots), (true, &f.proofs.post_roots)] {
        for root in requirements {
            let mut locations = root.locations.iter().copied().collect();
            close_dependencies(f, &mut locations);
            let map = maps.entry((root.pc, after)).or_default();
            for location in locations {
                let slot = *allocation.locations.get(&location).ok_or_else(|| error("missing root location"))?;
                match location_type(f, location)? {
                    ProducedType::Service(kind) => {
                        for &offset in layout::service_layout(kind).references {
                            map.insert(AllocatedRoot { slot, offset, tagged_int: false });
                        }
                    }
                    ProducedType::Value(Type::Bool | Type::Never | Type::Descriptor | Type::Byte | Type::Float
                        | Type::Char) => {}
                    ProducedType::Value(ty) => {
                        map.insert(AllocatedRoot { slot, offset: 0, tagged_int: ty == Type::Int });
                    }
                }
            }
        }
    }
    Ok(maps.into_iter().map(|(site, roots)| (site, roots.into_iter().collect())).collect())
}
