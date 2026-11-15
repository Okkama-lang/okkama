//! The allocation checker's contract (#701): its verdict is the greatest fixed point of the forward
//! availability analysis, whatever order the worklist takes, so it accepts and rejects exactly what the
//! checker before #701 did. `reference` keeps that checker (a LIFO worklist over one `BTreeSet` per slot)
//! as the oracle of the differential tests; the work tests pin the visiting order's cost.
use std::collections::{BTreeMap, BTreeSet};
use super::*;
use super::allocation::{allocated_roots, close_dependencies, definitions, direct_uses, location_type, metadata_uses,
    outgoing, post_uses, reverse_postorder, terminal};
use super::tests::empty_function;

/// The checker as it was before #701, kept verbatim as the differential oracle.
mod reference {
    use super::*;

    type Set = BTreeSet<Location>;

    pub(super) fn check_allocation(f: &Function, allocation: &Allocation) -> Result<(), Error> {
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
}

/// xorshift64*: deterministic inputs without a dependency.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }

    fn pick(&mut self, values: &[u32]) -> u32 {
        values[self.below(values.len())]
    }
}

fn constant_with(output: u32, inputs: Vec<u32>) -> Instruction {
    Instruction::Data { op: DataOp::Const, inputs, output: Some(output),
        immediate: Immediate::Constant(Constant::Int("0".into())) }
}

fn edge(target_pc: u32, assignments: Vec<(u32, u32)>) -> Edge {
    Edge {
        target_pc,
        assignments: assignments.into_iter().map(|(destination, source)| Assignment {
            destination: Location::Register(destination),
            source: Source::Location(Location::Register(source)),
        }).collect(),
    }
}

/// A well-formed function of int registers: blocks with parameters, straight-line definitions that read
/// what the block may read, and terminators (ret, br, cond) whose edges assign the target's parameters,
/// including back edges, so loops and joins of every shape appear.
fn generated(random: &mut Random) -> Function {
    let mut next_register = 0u32;
    let mut fresh = |count: usize| {
        let registers = (next_register..next_register + count as u32).collect::<Vec<_>>();
        next_register += count as u32;
        registers
    };
    let block_count = 2 + random.below(6);
    let params = (0..block_count).map(|block| fresh(if block == 0 { 1 + random.below(4) } else { random.below(4) }))
        .collect::<Vec<_>>();
    let mut bodies = Vec::new();
    for block in 0..block_count {
        let mut readable = params[0].clone();
        readable.extend(&params[block]);
        let mut body = Vec::new();
        for _ in 0..random.below(5) {
            let inputs = (0..random.below(3)).map(|_| random.pick(&readable)).collect();
            let output = fresh(1)[0];
            body.push(constant_with(output, inputs));
            readable.push(output);
        }
        bodies.push((body, readable));
    }
    // Block PCs are known once every body length is: each block ends with one terminator.
    let mut pcs = Vec::new();
    let mut pc = 0u32;
    for (body, _) in &bodies {
        pcs.push(pc);
        pc += body.len() as u32 + 1;
    }
    let mut code = Vec::new();
    for (body, readable) in bodies {
        code.extend(body);
        let jump = |random: &mut Random| {
            let target = 1 + random.below(block_count - 1);
            let assignments = params[target].iter().map(|&parameter| (parameter, random.pick(&readable))).collect();
            edge(pcs[target], assignments)
        };
        code.push(match random.below(4) {
            0 => Instruction::Ret((0..random.below(3)).map(|_| random.pick(&readable)).collect()),
            1 => Instruction::Br(jump(random)),
            _ => Instruction::Cond { condition: random.pick(&readable), yes: jump(random), no: jump(random) },
        });
    }
    let mut f = empty_function();
    f.registers = vec![Type::Int; next_register as usize];
    f.params = params[0].iter().copied().map(Location::Register).collect();
    f.blocks = pcs.iter().zip(&params).map(|(&pc, params)| Block {
        pc,
        params: params.iter().copied().map(Location::Register).collect(),
    }).collect();
    f.code = code;
    f
}

/// One mutation a faulty allocator could make: a location moved to another slot of its type, or an edge's
/// copies reordered, dropped or redirected.
fn mutated(random: &mut Random, allocation: &Allocation) -> Allocation {
    let mut result = allocation.clone();
    let edges = result.edges.keys().copied().collect::<Vec<_>>();
    match random.below(4) {
        0 if !result.locations.is_empty() => {
            let locations = result.locations.keys().copied().collect::<Vec<_>>();
            let location = locations[random.below(locations.len())];
            let slot = random.below(result.slots.len()) as u32;
            result.locations.insert(location, slot);
        }
        1 if !edges.is_empty() => {
            let copies = result.edges.get_mut(&edges[random.below(edges.len())]).unwrap();
            copies.reverse();
        }
        2 if !edges.is_empty() => {
            let copies = result.edges.get_mut(&edges[random.below(edges.len())]).unwrap();
            if !copies.is_empty() {
                let index = random.below(copies.len());
                copies.remove(index);
            }
        }
        _ if !edges.is_empty() => {
            let slots = result.slots.len() as u32;
            let copies = result.edges.get_mut(&edges[random.below(edges.len())]).unwrap();
            let source = CopySource::Slot(random.below(slots as usize) as u32);
            copies.push(Copy { destination: random.below(slots as usize) as u32, source });
        }
        _ => {}
    }
    result
}

fn verdict(result: Result<(), Error>) -> Result<(), &'static str> {
    result.map_err(|failure| failure.rule)
}

#[test]
fn checker_agrees_with_the_reference_on_generated_allocations() {
    let mut random = Random(0x9e37_79b9_7f4a_7c15);
    let (mut accepted, mut rejected) = (0, 0);
    for _ in 0..3000 {
        let f = generated(&mut random);
        let Ok(allocation) = allocate(&f) else { continue };
        let candidates = std::iter::once(allocation.clone())
            .chain((0..4).map(|_| mutated(&mut random, &allocation)));
        for candidate in candidates {
            let expected = verdict(reference::check_allocation(&f, &candidate));
            assert_eq!(verdict(check_allocation(&f, &candidate)), expected, "{f:?}\n{candidate:?}");
            if expected.is_ok() { accepted += 1 } else { rejected += 1 }
        }
    }
    // Both verdicts occur often enough for the agreement to mean something.
    assert!(accepted > 3000 && rejected > 1000, "accepted {accepted}, rejected {rejected}");
}

#[test]
fn allocator_output_of_generated_functions_is_accepted() {
    let mut random = Random(0x0123_4567_89ab_cdef);
    for _ in 0..1000 {
        let f = generated(&mut random);
        let allocation = allocate(&f).unwrap();
        check_allocation(&f, &allocation).unwrap();
    }
}

fn successors(f: &Function, pc: usize) -> Vec<usize> {
    let mut next = outgoing(&f.code[pc]).into_iter().map(|edge| edge.target_pc as usize).collect::<Vec<_>>();
    if !terminal(&f.code[pc]) && pc + 1 < f.code.len() {
        next.push(pc + 1);
    }
    next
}

#[test]
fn reverse_postorder_ranks_reachable_pcs_before_their_forward_successors() {
    let mut random = Random(0x5151_5151_5151_5151);
    for _ in 0..500 {
        let f = generated(&mut random);
        let rank = reverse_postorder(&f);
        assert_eq!(rank[0], 0);
        // Reachability by a plain search; every reachable PC has a distinct rank, the others none.
        let mut reachable = vec![false; f.code.len()];
        let mut stack = vec![0];
        while let Some(pc) = stack.pop() {
            if !std::mem::replace(&mut reachable[pc], true) {
                stack.extend(successors(&f, pc));
            }
        }
        let ranks = (0..f.code.len()).filter(|&pc| reachable[pc]).map(|pc| rank[pc]).collect::<BTreeSet<_>>();
        assert_eq!(ranks.len(), reachable.iter().filter(|&&r| r).count());
        assert!(ranks.iter().all(|&r| r < f.code.len()));
        assert!((0..f.code.len()).all(|pc| reachable[pc] || rank[pc] == usize::MAX));
        // An edge to a lower or equal rank closes a cycle: its target reaches its source.
        for pc in (0..f.code.len()).filter(|&pc| reachable[pc]) {
            for target in successors(&f, pc) {
                if rank[target] <= rank[pc] {
                    let mut seen = vec![false; f.code.len()];
                    let mut stack = vec![target];
                    let mut closes = false;
                    while let Some(at) = stack.pop() {
                        closes |= at == pc;
                        if !std::mem::replace(&mut seen[at], true) {
                            stack.extend(successors(&f, at));
                        }
                    }
                    assert!(closes, "edge {pc} -> {target} is not a back edge");
                }
            }
        }
    }
}

#[test]
fn reverse_postorder_ignores_invalid_targets_and_the_checker_rejects_them() {
    let mut f = empty_function();
    f.code = vec![Instruction::Br(edge(7, vec![])), Instruction::Ret(vec![])];
    assert_eq!(reverse_postorder(&f), vec![0, usize::MAX]);
    let allocation = Allocation {
        locations: BTreeMap::new(),
        slots: vec![],
        edges: BTreeMap::from([((0, 0), vec![])]),
        roots: BTreeMap::new(),
    };
    assert!(check_allocation(&f, &allocation).unwrap_err().message.contains("invalid target"));
}

/// `count` diamonds in a row over `padding` unused slots. Diamond k reads `c` and either redefines `r_k` in
/// the slot of the dead parameter `p_k` or keeps `p_k`; the join learns that slot holds neither. The
/// allocation is valid. Every join therefore shrinks when its second branch arrives, which made the
/// LIFO worklist before #701 walk the rest of the chain again from every join: quadratic in `count`.
fn diamonds(count: u32, padding: usize) -> (Function, Allocation) {
    let condition = 2 * count;
    let mut f = empty_function();
    f.registers = vec![Type::Int; 2 * count as usize + 1];
    f.params = (0..count).chain([condition]).map(Location::Register).collect();
    f.blocks = vec![Block { pc: 0, params: f.params.clone() }];
    f.code = Vec::new();
    for k in 0..count {
        let base = 3 * k;
        f.code.push(Instruction::Cond { condition, yes: edge(base + 1, vec![]), no: edge(base + 3, vec![]) });
        f.code.push(constant_with(count + k, vec![]));
        f.code.push(Instruction::Br(edge(base + 3, vec![])));
    }
    f.code.push(Instruction::Ret(vec![condition]));
    let mut locations = BTreeMap::new();
    for k in 0..count {
        locations.insert(Location::Register(k), k);
        locations.insert(Location::Register(count + k), k);
    }
    locations.insert(Location::Register(condition), count);
    let mut edges = BTreeMap::new();
    for k in 0..count {
        edges.insert((3 * k, 0), vec![]);
        edges.insert((3 * k, 1), vec![]);
        edges.insert((3 * k + 2, 0), vec![]);
    }
    let allocation = Allocation {
        locations,
        slots: vec![ProducedType::Value(Type::Int); count as usize + 1 + padding],
        edges,
        roots: BTreeMap::new(),
    };
    (f, allocation)
}

#[test]
fn diamond_chain_is_checked_within_the_work_bound() {
    // The reference needs about 3 * 150^2 / 2 visits of 2.3 K entries each: past the 50 M bound.
    let (f, allocation) = diamonds(150, 2000);
    check_allocation(&f, &allocation).unwrap();
}

#[test]
fn diamond_chain_still_rejects_a_clobbered_condition() {
    // r_0 shares the condition's slot: the first diamond's definition clobbers the condition.
    let (f, mut allocation) = diamonds(150, 2000);
    allocation.locations.insert(Location::Register(150), 150);
    let failure = check_allocation(&f, &allocation).unwrap_err();
    assert!(failure.message.contains("clobbered Register(300)"), "{failure}");
}

#[test]
fn diamond_chain_agrees_with_the_reference_where_the_reference_fits() {
    let (f, allocation) = diamonds(40, 20);
    assert_eq!(verdict(check_allocation(&f, &allocation)), Ok(()));
    assert_eq!(verdict(reference::check_allocation(&f, &allocation)), Ok(()));
}
