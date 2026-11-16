//! Own carries reference aliases independently of the successor's must-use permission.
//! Do not manufacture a second permission by forwarding an unused alias after succ.take.
use super::*;

/// The value a successor's succ.take produces: the object of an object successor, the role value of a
/// role successor. `None` for every other type.
fn taken(ty: s::Type) -> Option<s::Type> {
    match ty {
        s::Type::Successor(id) => Some(s::Type::Object(id)),
        s::Type::RoleSuccessor(id) => Some(s::Type::Role(id)),
        _ => None,
    }
}

fn is_successor(ty: s::Type) -> bool {
    taken(ty).is_some()
}

/// Adds to `needed` the source of every edge assignment `(destination, source)` whose destination it holds,
/// transitively (#779): each location is followed once, where passes over every edge took as many passes as the
/// longest chain of forwarded parameters.
fn close_needed(needed: &mut BTreeSet<s::Location>, feeds: Vec<(s::Location, s::Location)>) {
    let mut sources = BTreeMap::<s::Location, Vec<s::Location>>::new();
    for (destination, source) in feeds {
        sources.entry(destination).or_default().push(source);
    }
    let mut pending = needed.iter().copied().collect::<Vec<_>>();
    while let Some(location) = pending.pop() {
        for &source in sources.get(&location).into_iter().flatten() {
            if needed.insert(source) {
                pending.push(source);
            }
        }
    }
}

pub(super) fn prune_dead_aliases(function: &mut s::Function) {
    let mut needed = function.params.iter().copied().collect::<BTreeSet<_>>();
    for instruction in &function.code {
        match instruction {
            s::Instruction::Data { inputs, .. } | s::Instruction::Ret(inputs) => {
                needed.extend(inputs.iter().copied().map(s::Location::Register));
            }
            s::Instruction::Cond { condition, .. } | s::Instruction::Raise(condition) => {
                needed.insert(s::Location::Register(*condition));
            }
            s::Instruction::Champ { arguments, .. } => {
                for argument in arguments {
                    match argument {
                        s::Argument::Location(location) => { needed.insert(*location); }
                        s::Argument::Selection(id) => {
                            needed.extend(&function.proofs.selections[*id as usize].locations);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    let mut feeds = Vec::new();
    for instruction in &function.code {
        for edge in edges(instruction) {
            for assignment in &edge.assignments {
                if let s::Source::Location(source) = assignment.source {
                    feeds.push((assignment.destination, source));
                }
            }
        }
    }
    close_needed(&mut needed, feeds);
    let mut removed = BTreeSet::new();
    for block in &mut function.blocks {
        block.params.retain(|location| {
            let dead = matches!(location, s::Location::Register(id)
                if is_successor(function.registers[*id as usize]))
                && !needed.contains(location);
            if dead { removed.insert(*location); }
            !dead
        });
    }
    for instruction in &mut function.code {
        let edges = match instruction {
            s::Instruction::Br(edge) => vec![edge],
            s::Instruction::Cond { yes, no, .. } => vec![yes, no],
            s::Instruction::Champ { flow: s::Flow::Outcomes(outcomes), .. } => {
                outcomes.iter_mut().map(|outcome| &mut outcome.edge).collect()
            }
            _ => vec![],
        };
        for edge in edges {
            edge.assignments.retain(|assignment| !removed.contains(&assignment.destination));
        }
    }
    for root in function.proofs.roots.iter_mut().chain(&mut function.proofs.post_roots) {
        root.locations.retain(|location| !removed.contains(location));
    }
    function.proofs.dependencies.retain(|dependency| !removed.contains(&dependency.value));
    for dependency in &mut function.proofs.dependencies {
        dependency.obligations.retain(|location| !removed.contains(location));
    }
}

fn edges(instruction: &s::Instruction) -> Vec<&s::Edge> {
    match instruction {
        s::Instruction::Br(edge) => vec![edge],
        s::Instruction::Cond { yes, no, .. } => vec![yes, no],
        s::Instruction::Champ { flow: s::Flow::Outcomes(outcomes), .. } => {
            outcomes.iter().map(|outcome| &outcome.edge).collect()
        }
        _ => vec![],
    }
}

/// Successor registers whose must-use permission an instruction takes or moves away (a Data `move`, the
/// returned values of ret.own/ret.defer, the value of exit.prepare/exit.replace, which moves into the
/// pending exit), and the successor registers it creates a permission in (Data and `Flow::Next` outputs).
/// Mirrors the SPB 17 loader's `check_successors`.
pub(super) fn permission_effects(function: &s::Function, instruction: &s::Instruction)
    -> (Vec<s::Location>, Vec<s::Location>)
{
    let successor = |location: s::Location| matches!(location, s::Location::Register(id)
        if is_successor(function.registers[id as usize]));
    match instruction {
        s::Instruction::Data { op, inputs, output, .. } => {
            let taken = inputs.first().copied().map(s::Location::Register)
                .filter(|&location| *op == s::DataOp::Move && successor(location));
            let created = output.map(s::Location::Register).filter(|&location| successor(location));
            (taken.into_iter().collect(), created.into_iter().collect())
        }
        s::Instruction::Champ { op, arguments, flow } => {
            let moves = matches!(op, s::Opcode::RetOwn | s::Opcode::RetDefer | s::Opcode::ExitPrepare
                | s::Opcode::ExitReplace);
            let taken = arguments.iter().filter_map(|argument| match argument {
                s::Argument::Location(location) if moves && successor(*location) => Some(*location),
                _ => None,
            }).collect();
            let created = match flow {
                s::Flow::Next(outputs) => outputs.iter().copied().filter(|&location| successor(location)).collect(),
                s::Flow::Outcomes(_) => vec![],
            };
            (taken, created)
        }
        _ => (vec![], vec![]),
    }
}

/// Unwind abandons unused successor results. Discharge their must-use permission explicitly;
/// cleanup still owns the same identity packet. No destroy or ownership operation is introduced.
pub(super) fn close_unwind_permissions(function: &mut s::Function) -> Result<(), GenerateError> {
    if !function.registers.iter().any(|&ty| is_successor(ty)) || function.code.is_empty() {
        return Ok(());
    }
    let successor = |location| matches!(location, s::Location::Register(id)
        if is_successor(function.registers[id as usize]));
    let mut bare = function.clone();
    bare.proofs.roots.clear();
    bare.proofs.post_roots.clear();
    bare.proofs.states.clear();
    bare.proofs.dependencies.clear();
    let live = s::live_locations(&bare)?;
    let blocks = function.blocks.iter().enumerate().map(|(index, block)| (block.pc, index))
        .collect::<BTreeMap<_, _>>();
    let mut incoming = vec![BTreeSet::new(); function.blocks.len()];
    incoming[0].extend(function.params.iter().copied().filter(|&location| successor(location)));
    let mut work = vec![0];
    let mut seen = BTreeSet::new();
    let mut drops = BTreeMap::new();
    while let Some(index) = work.pop() {
        seen.insert(index);
        let mut active = incoming[index].clone();
        let block = &function.blocks[index];
        let end = function.blocks.get(index + 1).map_or(function.code.len(), |block| block.pc as usize);
        for pc in block.pc as usize..end {
            let instruction = &function.code[pc];
            let (taken, created) = permission_effects(function, instruction);
            for location in taken {
                active.remove(&location);
            }
            active.extend(created);
            for (ordinal, edge) in edges(instruction).into_iter().enumerate() {
                let mut next = active.clone();
                let mut destinations = BTreeSet::new();
                for assignment in &edge.assignments {
                    if successor(assignment.destination) {
                        if let s::Source::Location(source) = assignment.source { next.remove(&source); }
                        destinations.insert(assignment.destination);
                    }
                }
                next.extend(&destinations);
                let exceptional = matches!(instruction,
                    s::Instruction::Champ { flow: s::Flow::Outcomes(outcomes), .. }
                        if matches!(outcomes[ordinal].tag, s::Tag::Unwind | s::Tag::Failure));
                if exceptional {
                    let abandoned = next.iter().copied().filter(|location| {
                        !live[edge.target_pc as usize].contains(location) && !destinations.contains(location)
                    }).collect::<BTreeSet<_>>();
                    for location in &abandoned { next.remove(location); }
                    if !abandoned.is_empty() { drops.insert((pc, ordinal), abandoned); }
                }
                let target = blocks[&edge.target_pc];
                let previous = incoming[target].len();
                incoming[target].extend(next);
                if incoming[target].len() != previous || !seen.contains(&target) { work.push(target); }
            }
        }
    }
    let original_len = function.code.len();
    let mut extended = BTreeSet::new();
    for ((pc, ordinal), abandoned) in drops {
        let edge = edges(&function.code[pc])[ordinal].clone();
        let bridge_pc = function.code.len() as u32;
        let mut entry = edge.clone();
        let mut continuation = edge;
        entry.target_pc = bridge_pc;
        let mut params = Vec::new();
        for (input, output) in entry.assignments.iter_mut().zip(&mut continuation.assignments) {
            let location = match input.destination {
                s::Location::Register(id) => {
                    s::Location::Register(fresh(function, function.registers[id as usize])?)
                }
                s::Location::Service(id) => {
                    let location = s::Location::Service(function.services.len() as u32);
                    function.services.push(function.services[id as usize]);
                    location
                }
            };
            params.push(location);
            input.destination = location;
            output.source = s::Source::Location(location);
        }
        function.blocks.push(s::Block { pc: bridge_pc, params });
        for location in abandoned {
            extended.insert(location);
            let s::Location::Register(input) = location else { unreachable!() };
            let Some(object) = taken(function.registers[input as usize]) else { unreachable!() };
            let output = fresh(function, object)?;
            function.code.push(data(s::DataOp::Move, vec![input], output, s::Immediate::None));
        }
        function.code.push(s::Instruction::Br(continuation));
        match &mut function.code[pc] {
            s::Instruction::Champ { flow: s::Flow::Outcomes(outcomes), .. } => outcomes[ordinal].edge = entry,
            _ => unreachable!("exceptional edge"),
        }
    }
    if function.code.len() != original_len {
        let states = s::service_states(function)?;
        function.proofs.states.extend(states.into_iter().filter(|state| state.pc as usize >= original_len));
        let live = s::live_locations(function)?;
        for (pc, locations) in live.iter().enumerate() {
            let roots = if pc >= original_len { locations.clone() }
                else { locations.intersection(&extended).copied().collect() };
            if !roots.is_empty() {
                function.proofs.roots.push(s::RootRequirement {
                    pc: pc as u32, locations: roots.into_iter().collect(),
                });
            }
            if let s::Instruction::Champ { flow: s::Flow::Next(_), op, .. } = function.code[pc]
                && !matches!(op, s::Opcode::RetOwn | s::Opcode::RetDefer)
            {
                let roots = live.get(pc + 1).into_iter().flatten().copied()
                    .filter(|location| extended.contains(location)).collect::<Vec<_>>();
                if !roots.is_empty() {
                    function.proofs.post_roots.push(s::RootRequirement { pc: pc as u32, locations: roots });
                }
            }
        }
    }
    Ok(())
}

/// A successor's must-use state on one path: live permissions and, for each taken successor, the
/// object reference its succ.take produced.
#[derive(Clone, PartialEq, Eq)]
struct Permissions {
    active: BTreeSet<s::Location>,
    taken: BTreeMap<s::Location, s::Location>,
}

/// OIR-2 discharges must-use at least once and keeps the reference readable after succ.take (§8), so
/// Own may forward a taken successor alias and take it again. SPB 17 keeps exactly one permission per
/// path. Forward the object reference the take produced instead: the destination becomes a plain object
/// parameter, and a later succ.take of it is a plain move. No permission is created or dropped.
/// The forwarded objects now live longer: they become roots wherever they are live, as Low roots the
/// successor alias they replace.
pub(super) fn forward_taken(function: &mut s::Function) -> Result<(), GenerateError> {
    let mut forwarded = BTreeSet::new();
    while let Some((sources, params, retakes)) = taken_forwards(function) {
        // OIR-2 §8: a successor is used at least once and its value may be read again, so Own may take it
        // twice on one path. The second take moves the object the first one produced: one permission per path.
        for (pc, object) in retakes {
            forwarded.insert(object);
            let s::Location::Register(object) = object else { unreachable!("taken object register") };
            if let s::Instruction::Data { inputs, .. } = &mut function.code[pc] {
                inputs[0] = object;
            }
        }
        for (pc, ordinal, index, object) in sources {
            forwarded.insert(object);
            let edge = match &mut function.code[pc] {
                s::Instruction::Br(edge) => edge,
                s::Instruction::Cond { yes, no, .. } => if ordinal == 0 { yes } else { no },
                s::Instruction::Champ { flow: s::Flow::Outcomes(outcomes), .. } => &mut outcomes[ordinal].edge,
                _ => unreachable!("edge instruction"),
            };
            edge.assignments[index].source = s::Source::Location(object);
        }
        for param in params {
            if let Some(object) = taken(function.registers[param as usize]) {
                function.registers[param as usize] = object;
            }
        }
    }
    if forwarded.is_empty() {
        return Ok(());
    }
    let mut bare = function.clone();
    bare.proofs.roots.clear();
    bare.proofs.post_roots.clear();
    bare.proofs.dependencies.clear();
    let live = s::live_locations(&bare)?;
    for (pc, locations) in live.iter().enumerate() {
        let roots = locations.intersection(&forwarded).copied().collect::<Vec<_>>();
        if !roots.is_empty() {
            function.proofs.roots.push(s::RootRequirement { pc: pc as u32, locations: roots });
        }
        if let s::Instruction::Champ { flow: s::Flow::Next(_), op, .. } = function.code[pc]
            && !matches!(op, s::Opcode::RetOwn | s::Opcode::RetDefer)
        {
            let roots = live.get(pc + 1).into_iter().flatten().copied()
                .filter(|location| forwarded.contains(location)).collect::<Vec<_>>();
            if !roots.is_empty() {
                function.proofs.post_roots.push(s::RootRequirement { pc: pc as u32, locations: roots });
            }
        }
    }
    Ok(())
}

type Forward = (usize, usize, usize, s::Location);
type Round = (Vec<Forward>, BTreeSet<u32>, Vec<(usize, s::Location)>);

/// One round of `forward_taken`: the edge sources to replace, the parameters to retype and the repeated
/// takes to redirect, or `None` when nothing can be forwarded. Inconsistent states are left for the
/// verifier to reject.
fn taken_forwards(function: &s::Function) -> Option<Round> {
    let successor = |location: s::Location| matches!(location, s::Location::Register(id)
        if is_successor(function.registers[id as usize]));
    if function.code.is_empty() || !function.registers.iter().any(|&ty| is_successor(ty)) {
        return None;
    }
    let blocks = function.blocks.iter().enumerate().map(|(index, block)| (block.pc, index))
        .collect::<BTreeMap<_, _>>();
    let start = Permissions {
        active: function.params.iter().copied().filter(|&location| successor(location)).collect(),
        taken: BTreeMap::new(),
    };
    let mut incoming = vec![None; function.blocks.len()];
    incoming[0] = Some(start);
    let mut work = vec![0];
    // Per (pc, ordinal, assignment): the object to forward, or None where the source holds no object.
    let mut forwards = BTreeMap::<(usize, usize, usize), Option<s::Location>>::new();
    // Per pc of a succ.take whose permission is already spent: the object its first take produced, or None
    // where the paths into it disagree.
    let mut retakes = BTreeMap::<usize, Option<s::Location>>::new();
    while let Some(index) = work.pop() {
        let mut state: Permissions = incoming[index].clone()?;
        let block = &function.blocks[index];
        let end = function.blocks.get(index + 1).map_or(function.code.len(), |block| block.pc as usize);
        for pc in block.pc as usize..end {
            let instruction = &function.code[pc];
            let (taken, created) = permission_effects(function, instruction);
            for location in taken {
                // Only succ.take (a Data move) leaves an object reference to forward in place of the alias.
                let take = match instruction {
                    s::Instruction::Data { op: s::DataOp::Move, output: Some(output), .. } => Some(*output),
                    _ => None,
                };
                if state.active.remove(&location) {
                    if let Some(output) = take {
                        state.taken.insert(location, s::Location::Register(output));
                    }
                } else if take.is_some() {
                    // OIR-2 §8: a second succ.take of a taken successor moves the first take's object.
                    let object = state.taken.get(&location).copied();
                    let entry = retakes.entry(pc).or_insert(object);
                    if *entry != object {
                        *entry = None;
                    }
                }
            }
            state.active.extend(created);
            for (ordinal, edge) in edges(instruction).into_iter().enumerate() {
                let mut next = state.clone();
                for (position, assignment) in edge.assignments.iter().enumerate() {
                    if !successor(assignment.destination) {
                        continue;
                    }
                    let s::Source::Location(source) = assignment.source else {
                        next.active.insert(assignment.destination);
                        continue;
                    };
                    if state.active.contains(&source) {
                        next.active.remove(&source);
                        next.active.insert(assignment.destination);
                    } else {
                        // A parameter already retyped by an earlier round forwards its own object.
                        let object = if successor(source) {
                            state.taken.get(&source).copied()
                        } else {
                            Some(source)
                        };
                        let entry = forwards.entry((pc, ordinal, position)).or_insert(object);
                        if *entry != object {
                            *entry = None;
                        }
                    }
                }
                let target = blocks[&edge.target_pc];
                match &mut incoming[target] {
                    Some(previous) => {
                        if previous.active != next.active {
                            return None;
                        }
                        let before = previous.taken.len();
                        previous.taken.retain(|location, object| next.taken.get(location) == Some(object));
                        if previous.taken.len() != before {
                            work.push(target);
                        }
                    }
                    slot => {
                        *slot = Some(next);
                        work.push(target);
                    }
                }
            }
        }
    }
    // A parameter becomes an object only when every edge into it forwards a taken successor's object.
    let mut candidates = BTreeMap::<u32, bool>::new();
    for (pc, instruction) in function.code.iter().enumerate() {
        for (ordinal, edge) in edges(instruction).into_iter().enumerate() {
            for (position, assignment) in edge.assignments.iter().enumerate() {
                let s::Location::Register(destination) = assignment.destination else { continue };
                if !successor(assignment.destination) {
                    continue;
                }
                let forwarded = forwards.get(&(pc, ordinal, position)).is_some_and(Option::is_some);
                *candidates.entry(destination).or_insert(true) &= forwarded;
            }
        }
    }
    let params = candidates.into_iter().filter_map(|(param, all)| all.then_some(param)).collect::<BTreeSet<_>>();
    let retakes = retakes.into_iter().filter_map(|(pc, object)| Some((pc, object?))).collect::<Vec<_>>();
    if params.is_empty() && retakes.is_empty() {
        return None;
    }
    let sources = forwards.into_iter().filter_map(|((pc, ordinal, position), object)| {
        let edge = edges(&function.code[pc])[ordinal];
        let s::Location::Register(destination) = edge.assignments[position].destination else { return None };
        params.contains(&destination).then_some((pc, ordinal, position, object?))
    }).collect();
    Some((sources, params, retakes))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `close_needed` reaches what repeated passes over the assignments reached before #779. Random chains,
    /// cycles and repeated assignments over a few locations.
    #[test]
    fn needed_sources_close_as_the_passes_did() {
        let mut random = 0x0779_a11a_0779_a11au64;
        let mut next = |bound: u32| {
            random ^= random << 13;
            random ^= random >> 7;
            random ^= random << 17;
            (random % u64::from(bound)) as u32
        };
        let mut added = 0;
        for _ in 0..2000 {
            let count = 1 + next(30);
            let location = |id: u32| {
                if id.is_multiple_of(2) { s::Location::Register(id) } else { s::Location::Service(id) }
            };
            let feeds: Vec<_> = (0..next(50)).map(|_| (location(next(count)), location(next(count)))).collect();
            let initial: BTreeSet<_> = (0..count).filter(|_| next(5) == 0).map(location).collect();
            let mut expected = initial.clone();
            loop {
                let before = expected.len();
                for &(destination, source) in &feeds {
                    if expected.contains(&destination) {
                        expected.insert(source);
                    }
                }
                if expected.len() == before { break; }
            }
            let mut actual = initial.clone();
            close_needed(&mut actual, feeds.clone());
            assert_eq!(actual, expected, "{initial:?} {feeds:?}");
            added += actual.len() - initial.len();
        }
        assert!(added > 1000, "{added}");
    }
}
