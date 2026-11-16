//! The liveness and the allocator as they were before #755, kept verbatim as the oracle the new ones are
//! checked against: in this crate's tests on every call, and wherever the `liveness-reference` feature is on
//! (the compiler's tests turn it on for its fixtures and goldens). Never compiled into a runtime.
use std::collections::{BTreeMap, BTreeSet};
use super::*;

type Set = BTreeSet<Location>;

/// Exact CFG fixed point with simultaneous edge renames: full reverse sweeps until nothing changes.
pub(crate) fn live_locations(f: &Function) -> Result<Vec<Set>, Error> {
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

pub(crate) fn allocate(f: &Function) -> Result<Allocation, Error> {
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

pub(crate) fn allocated_roots(f: &Function, allocation: &Allocation)
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

/// `f` without the proof records that `parts` leaves out, as the callers before #755 cleared them.
pub(crate) fn restricted(f: &Function, parts: Parts) -> Function {
    let mut bare = f.clone();
    if !parts.roots {
        bare.proofs.roots.clear();
    }
    if !parts.states {
        bare.proofs.states.clear();
    }
    if !parts.post_roots {
        bare.proofs.post_roots.clear();
    }
    if !parts.dependencies {
        bare.proofs.dependencies.clear();
    }
    bare
}

/// Only a budget may tell the two apart: the new work unit differs, so either side may stop where the other
/// does not. Otherwise both reject alike, or both accept with the same result. The reference is not run
/// when the new side stopped at a budget, since anything it gives then agrees.
pub(crate) fn agree<T: PartialEq + std::fmt::Debug>(new: &Result<T, Error>, old: impl FnOnce() -> Result<T, Error>,
    what: &str)
{
    let budget = |result: &Result<T, Error>| result.as_ref()
        .is_err_and(|error| matches!(error.rule, "ALLOCATION_WORK" | "ALLOCATION_MEMORY"));
    if budget(new) {
        return;
    }
    let old = old();
    match (new, &old) {
        (Ok(new), Ok(old)) => assert_eq!(new, old, "{what} differs from the reference before #755"),
        (Err(new), Err(old)) => assert_eq!(new.rule, old.rule, "{what}: {new} against the reference's {old}"),
        _ if budget(&old) => {}
        _ => panic!("{what}: {new:?} against the reference's {old:?}"),
    }
}
