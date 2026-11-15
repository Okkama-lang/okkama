//! Own carries reference aliases independently of the successor's must-use permission.
//! Do not manufacture a second permission by forwarding an unused alias after succ.take.
use super::*;

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
    loop {
        let before = needed.len();
        for instruction in &function.code {
            for edge in edges(instruction) {
                for assignment in &edge.assignments {
                    if needed.contains(&assignment.destination)
                        && let s::Source::Location(source) = assignment.source
                    {
                        needed.insert(source);
                    }
                }
            }
        }
        if needed.len() == before { break; }
    }
    let mut removed = BTreeSet::new();
    for block in &mut function.blocks {
        block.params.retain(|location| {
            let dead = matches!(location, s::Location::Register(id)
                if matches!(function.registers[*id as usize], s::Type::Successor(_)))
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

/// Unwind abandons unused successor results. Discharge their must-use permission explicitly;
/// cleanup still owns the same identity packet. No destroy or ownership operation is introduced.
pub(super) fn close_unwind_permissions(function: &mut s::Function) -> Result<(), GenerateError> {
    if !function.registers.iter().any(|ty| matches!(ty, s::Type::Successor(_))) || function.code.is_empty() {
        return Ok(());
    }
    let successor = |location| matches!(location, s::Location::Register(id)
        if matches!(function.registers[id as usize], s::Type::Successor(_)));
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
            if let s::Instruction::Data { op, inputs, output, .. } = instruction {
                if *op == s::DataOp::Move && inputs.first().is_some_and(|&id| successor(s::Location::Register(id))) {
                    active.remove(&s::Location::Register(inputs[0]));
                }
                if let Some(output) = output && successor(s::Location::Register(*output)) {
                    active.insert(s::Location::Register(*output));
                }
            }
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
            let s::Type::Successor(id) = function.registers[input as usize] else { unreachable!() };
            let output = fresh(function, s::Type::Object(id))?;
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
