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

// ---- #755: the worklist liveness, the shared allocation and their reference ----

use super::allocation::{Liveness as Live, Obligations, Parts, liveness, reference as before_755};

const VARIANTS: [Parts; 3] = [Parts::ALL, Parts::REQUIRED, Parts::USES];

/// `f` with services and random proof records: dependencies over registers and services (cycles and repeated
/// values included), root and state maps at any PC, post roots after straight-line instructions (their own
/// definitions among them), and produced sources on some edge assignments.
fn with_proofs(random: &mut Random, mut f: Function) -> Function {
    let kinds = [ServiceKind::Deps, ServiceKind::Packet, ServiceKind::StableKey];
    f.services = (0..random.below(4)).map(|_| kinds[random.below(kinds.len())]).collect();
    let locations = (0..f.registers.len() as u32).map(Location::Register)
        .chain((0..f.services.len() as u32).map(Location::Service)).collect::<Vec<_>>();
    let some = |random: &mut Random, most: usize| {
        (0..random.below(most + 1)).map(|_| locations[random.below(locations.len())]).collect::<Vec<_>>()
    };
    for _ in 0..random.below(7) {
        let value = locations[random.below(locations.len())];
        let obligations = some(random, 3);
        f.proofs.dependencies.push(Dependency { value, obligations });
    }
    for _ in 0..random.below(4) {
        let pc = random.below(f.code.len()) as u32;
        let locations = some(random, 3);
        f.proofs.roots.push(RootRequirement { pc, locations });
    }
    for _ in 0..random.below(4) {
        let pc = random.below(f.code.len()) as u32;
        let locations = some(random, 3);
        f.proofs.states.push(StateRecord { pc, locations });
    }
    for pc in 0..f.code.len() {
        if !terminal(&f.code[pc]) && random.below(3) == 0 {
            let mut locations = some(random, 2);
            if random.below(2) == 0 {
                locations.extend(definitions(&f.code[pc]));
            }
            f.proofs.post_roots.push(RootRequirement { pc: pc as u32, locations });
        }
    }
    for instruction in &mut f.code {
        let edges = match instruction {
            Instruction::Br(edge) => vec![edge],
            Instruction::Cond { yes, no, .. } => vec![yes, no],
            _ => vec![],
        };
        for edge in edges {
            for assignment in &mut edge.assignments {
                if random.below(8) == 0 {
                    assignment.source = Source::ProducedSlot(random.below(2) as u32);
                }
            }
        }
    }
    f
}

/// Every liveness variant, the liveness with roots taken from the one without them, and the allocation equal
/// the algorithms before #755, called directly here (not only through the checks every call makes in tests).
fn agrees_with_the_reference(f: &Function) {
    for parts in VARIANTS {
        let expected = before_755::live_locations(&before_755::restricted(f, parts)).unwrap();
        assert_eq!(liveness(f, parts).unwrap().sets(), expected, "{parts:?} of {f:?}");
    }
    let required = liveness(f, Parts::REQUIRED).unwrap();
    assert_eq!(required.with_roots(f).unwrap(), liveness(f, Parts::ALL).unwrap(), "{f:?}");
    assert_eq!(allocate(f), before_755::allocate(f), "{f:?}");
}

#[test]
fn liveness_and_allocation_of_generated_functions_with_proofs_agree_with_the_reference() {
    let mut random = Random(0x7575_7575_0755_0755);
    let (mut reused, mut solved_again) = (0, 0);
    for _ in 0..3000 {
        let plain = generated(&mut random);
        let f = with_proofs(&mut random, plain);
        agrees_with_the_reference(&f);
        if liveness(&f, Parts::REQUIRED).unwrap().sets() == liveness(&f, Parts::ALL).unwrap().sets() {
            reused += 1;
        } else {
            solved_again += 1;
        }
    }
    // Roots both within and beyond the live sets occur often enough for `with_roots` to be tested both ways.
    assert!(reused > 300 && solved_again > 300, "reused {reused}, solved again {solved_again}");
}

/// The SPB 17 fixtures of the compiler and of this crate, every function in every liveness variant.
#[test]
fn liveness_and_allocation_of_every_fixture_function_agree_with_the_reference() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut functions = 0;
    for directory in ["../okkama-compiler/src/codegen/sparrow_oir2/fixtures", "src/spb17/fixtures"] {
        let mut paths = std::fs::read_dir(root.join(directory)).unwrap().map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "spb17")).collect::<Vec<_>>();
        paths.sort();
        for path in paths {
            let program = assemble(&std::fs::read_to_string(&path).unwrap()).unwrap();
            for f in &program.functions {
                agrees_with_the_reference(f);
                functions += 1;
            }
        }
    }
    assert!(functions > 40, "{functions} fixture functions");
}

#[test]
fn obligations_close_sets_as_close_dependencies_does() {
    let mut random = Random(0x0b11_9a71_0b11_9a71);
    for _ in 0..2000 {
        let plain = generated(&mut random);
        let f = with_proofs(&mut random, plain);
        let obligations = Obligations::new(&f).unwrap();
        let locations = (0..f.registers.len() as u32).map(Location::Register)
            .chain((0..f.services.len() as u32).map(Location::Service)).collect::<Vec<_>>();
        let mut set = (0..random.below(4)).map(|_| locations[random.below(locations.len())]).collect::<BTreeSet<_>>();
        let mut expected = set.clone();
        close_dependencies(&f, &mut expected);
        obligations.close(&mut set);
        assert_eq!(set, expected, "{f:?}");
    }
}

/// A chain of obligations through a cycle: a dependency reached only through another one is kept.
#[test]
fn obligations_are_transitive_through_cycles() {
    let mut f = empty_function();
    f.registers = vec![Type::Int; 4];
    let dependency = |value: u32, obligations: &[u32]| Dependency {
        value: Location::Register(value), obligations: obligations.iter().copied().map(Location::Register).collect(),
    };
    f.proofs.dependencies = vec![dependency(0, &[1]), dependency(1, &[2]), dependency(2, &[0]), dependency(0, &[])];
    let obligations = Obligations::new(&f).unwrap();
    let mut set = BTreeSet::from([Location::Register(1)]);
    obligations.close(&mut set);
    assert_eq!(set, (0..3).map(Location::Register).collect());
    let mut set = BTreeSet::from([Location::Register(3)]);
    obligations.close(&mut set);
    assert_eq!(set, BTreeSet::from([Location::Register(3)]));
}

#[test]
fn obligations_reject_a_location_outside_the_function() {
    let mut f = empty_function();
    f.registers = vec![Type::Int];
    f.proofs.dependencies = vec![Dependency { value: Location::Register(0), obligations: vec![Location::Service(0)] }];
    assert!(Obligations::new(&f).unwrap_err().message.contains("invalid location"));
}

/// r1 is read at the ret; r0 is a register no instruction reads.
fn one_read() -> Function {
    let mut f = empty_function();
    f.registers = vec![Type::Int; 2];
    f.code = vec![constant_with(1, vec![]), Instruction::Ret(vec![1])];
    f
}

#[test]
fn liveness_lists_each_pcs_locations_in_location_order() {
    let mut f = one_read();
    f.services = vec![ServiceKind::Deps];
    f.proofs.states = vec![StateRecord { pc: 1, locations: vec![Location::Service(0)] }];
    let live = liveness(&f, Parts::ALL).unwrap();
    assert_eq!(live.len(), 2);
    // The state record keeps s0 live before PC 1 and so before PC 0, which does not define it.
    assert!(live.at(0).eq([Location::Service(0)]));
    assert_eq!(live.at(1).collect::<Vec<_>>(), vec![Location::Register(1), Location::Service(0)]);
    assert!(live.contains(1, Location::Service(0)) && !live.contains(1, Location::Register(0)));
    assert!(!live.contains(2, Location::Register(1)) && !live.contains(1, Location::Service(5)));
    assert!(liveness(&f, Parts::USES).unwrap().at(1).eq([Location::Register(1)]));
}

#[test]
fn liveness_with_roots_adds_a_root_beyond_the_required_sets() {
    let mut f = one_read();
    f.proofs.roots = vec![RootRequirement { pc: 1, locations: vec![Location::Register(0)] }];
    let required = liveness(&f, Parts::REQUIRED).unwrap();
    assert!(!required.contains(1, Location::Register(0)));
    let all = required.with_roots(&f).unwrap();
    assert!(all.contains(1, Location::Register(0)) && all.contains(0, Location::Register(0)));
    assert_eq!(all, liveness(&f, Parts::ALL).unwrap());
}

#[test]
fn liveness_with_roots_keeps_a_post_root_the_pc_defines_out_of_the_pc() {
    // Post roots are live after the PC: r1, defined at PC 0, is not live before it.
    let mut f = one_read();
    f.proofs.post_roots = vec![RootRequirement { pc: 0, locations: vec![Location::Register(1)] }];
    let all = liveness(&f, Parts::REQUIRED).unwrap().with_roots(&f).unwrap();
    assert!(!all.contains(0, Location::Register(1)));
    assert_eq!(all, liveness(&f, Parts::ALL).unwrap());
}

#[test]
fn liveness_with_roots_takes_only_the_liveness_without_roots() {
    let f = one_read();
    let all = liveness(&f, Parts::ALL).unwrap();
    assert!(all.with_roots(&f).unwrap_err().message.contains("liveness of another function"));
}

#[test]
fn allocation_takes_only_the_liveness_of_its_own_function() {
    let f = one_read();
    let mut other = one_read();
    other.registers.push(Type::Int);
    assert!(allocate_with(&f, &Live::of(&other).unwrap()).unwrap_err().message.contains("another function"));
    let required = liveness(&f, Parts::REQUIRED).unwrap();
    assert!(allocate_with(&f, &required).unwrap_err().message.contains("another function"));
    assert_eq!(allocate_with(&f, &Live::of(&f).unwrap()).unwrap(), allocate(&f).unwrap());
}

#[test]
fn liveness_rejects_an_edge_to_a_missing_pc() {
    let mut f = empty_function();
    f.code = vec![Instruction::Br(edge(7, vec![]))];
    assert!(liveness(&f, Parts::ALL).unwrap_err().message.contains("invalid edge PC"));
}

/// Unreachable PCs are solved too: the chain 1100 → … → 1 leads into the ret at PC 0 that nothing reaches.
#[test]
fn liveness_covers_pcs_unreachable_from_the_entry() {
    let mut f = empty_function();
    f.registers = vec![Type::Int; 3];
    f.code = vec![Instruction::Ret(vec![0, 2])];
    f.code.extend((1..=50u32).map(|pc| Instruction::Br(edge(pc - 1, vec![]))));
    let live = liveness(&f, Parts::ALL).unwrap();
    assert!((0..live.len()).all(|pc| live.at(pc).eq([Location::Register(0), Location::Register(2)])));
}

/// The checker over flat states and the obligation table (#755) accepts and rejects what the checker before
/// #701 does, on functions with dependencies, roots, states and post roots, and on faulty allocations of them.
#[test]
fn checker_agrees_with_the_reference_on_generated_allocations_with_proofs() {
    let mut random = Random(0x0755_c4ec_0755_c4ec);
    let (mut accepted, mut rejected) = (0, 0);
    for _ in 0..2000 {
        let plain = generated(&mut random);
        let f = with_proofs(&mut random, plain);
        let Ok(allocation) = allocate(&f) else { continue };
        let candidates = std::iter::once(allocation.clone())
            .chain((0..4).map(|_| mutated(&mut random, &allocation)));
        for candidate in candidates {
            let expected = verdict(reference::check_allocation(&f, &candidate));
            assert_eq!(verdict(check_allocation(&f, &candidate)), expected, "{f:?}\n{candidate:?}");
            if expected.is_ok() { accepted += 1 } else { rejected += 1 }
        }
    }
    assert!(accepted > 1000 && rejected > 1000, "accepted {accepted}, rejected {rejected}");
}

/// A constant passed down a chain of `count` blocks, each with one parameter: the allocator gives every
/// parameter the constant's slot and elides every copy, so without pruning that slot keeps each earlier
/// parameter as a dead alias and the stored states grow with `count`² / 2 (#781).
fn alias_chain(count: u32) -> Function {
    let mut f = empty_function();
    f.registers = vec![Type::Int; count as usize + 1];
    f.code = vec![constant_with(0, vec![])];
    f.code.extend((1..=count).map(|pc| Instruction::Br(edge(pc + 1, vec![(pc, pc - 1)]))));
    f.code.push(Instruction::Ret(vec![count]));
    f.blocks.extend((1..=count).map(|parameter| Block {
        pc: parameter + 1, params: vec![Location::Register(parameter)],
    }));
    f
}

/// With the liveness the states keep only live locations (#781): 4200 blocks would store about 8.8 M dead
/// aliases, beyond the checker's 8 Mi storage bound; pruned, each state holds the one live parameter.
#[test]
fn checker_states_forget_dead_aliases() {
    let f = alias_chain(4200);
    let live = Liveness::of(&f).unwrap();
    let allocation = allocate_with(&f, &live).unwrap();
    assert_eq!(allocation.slots.len(), 1, "one slot for the whole chain");
    check_allocation_with(&f, &allocation, &live).unwrap();
}

/// The pruned checker accepts and rejects what the checker before #701 does, as the unpruned one does (#781).
#[test]
fn pruned_checker_agrees_with_the_reference_on_generated_allocations_with_proofs() {
    let mut random = Random(0x0781_c4ec_0781_c4ec);
    let (mut accepted, mut rejected) = (0, 0);
    for _ in 0..2000 {
        let plain = generated(&mut random);
        let f = with_proofs(&mut random, plain);
        let Ok(live) = Liveness::of(&f) else { continue };
        let Ok(allocation) = allocate_with(&f, &live) else { continue };
        let candidates = std::iter::once(allocation.clone())
            .chain((0..4).map(|_| mutated(&mut random, &allocation)));
        for candidate in candidates {
            let expected = verdict(reference::check_allocation(&f, &candidate));
            assert_eq!(verdict(check_allocation_with(&f, &candidate, &live)), expected, "{f:?}\n{candidate:?}");
            if expected.is_ok() { accepted += 1 } else { rejected += 1 }
        }
    }
    assert!(accepted > 1000 && rejected > 1000, "accepted {accepted}, rejected {rejected}");
}

/// The liveness only prunes: one that misses a location the function reads later makes the check reject the
/// correct allocation, never accept a wrong one. Here the liveness of the same shape that returns `r1` instead
/// of `r0` drops `r0` before the ret.
#[test]
fn checker_rejects_with_a_liveness_that_misses_a_use() {
    let shape = |returned: u32| {
        let mut f = empty_function();
        f.registers = vec![Type::Int; 2];
        f.code = vec![constant_with(0, vec![]), constant_with(1, vec![]), Instruction::Ret(vec![returned])];
        f
    };
    let (f, other) = (shape(0), shape(1));
    let live = Liveness::of(&f).unwrap();
    let allocation = allocate_with(&f, &live).unwrap();
    check_allocation_with(&f, &allocation, &live).unwrap();
    let failure = check_allocation_with(&f, &allocation, &Liveness::of(&other).unwrap()).unwrap_err();
    assert!(failure.message.contains("clobbered"), "{failure}");
}

/// A liveness of another function, or one without every proof record, is refused before any state is pruned.
#[test]
fn checker_refuses_a_liveness_of_another_function() {
    let f = alias_chain(3);
    let allocation = allocate(&f).unwrap();
    for live in [Liveness::of(&alias_chain(4)).unwrap(), liveness(&f, Parts::REQUIRED).unwrap()] {
        let failure = check_allocation_with(&f, &allocation, &live).unwrap_err();
        assert!(failure.message.contains("liveness of another function"), "{failure}");
    }
}
