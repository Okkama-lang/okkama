//! Virtual locations stay SSA in SPB. This checked plan assigns typed frame slots and lowers
//! parallel edges to sequential copies. Post slots are read only on their selected edge.
use std::collections::{BTreeMap, BTreeSet};
use super::*;
#[cfg(any(test, feature = "liveness-reference"))]
#[path = "allocation_reference.rs"]
pub(crate) mod reference;

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

pub(crate) fn definitions(inst: &Instruction) -> Vec<Location> {
    match inst {
        Instruction::Data { output, .. } => output.iter().copied().map(Location::Register).collect(),
        Instruction::Champ { flow: Flow::Next(results), .. } => results.clone(),
        _ => vec![],
    }
}

pub(crate) fn direct_uses(f: &Function, pc: usize) -> Set {
    let mut used = Set::new();
    reads(f, pc, &mut |location| {
        used.insert(location);
    });
    used
}

/// Every location instruction `pc` reads, in operand order and with repeats: what `direct_uses` collects.
fn reads(f: &Function, pc: usize, visit: &mut impl FnMut(Location)) {
    match &f.code[pc] {
        Instruction::Data { inputs, .. } | Instruction::Ret(inputs) => {
            inputs.iter().copied().map(Location::Register).for_each(visit);
        }
        Instruction::Cond { condition, .. } => visit(Location::Register(*condition)),
        Instruction::Raise(value) => visit(Location::Register(*value)),
        Instruction::Champ { arguments, .. } => {
            for arg in arguments {
                match *arg {
                    Argument::Location(location) => visit(location),
                    Argument::Selection(id) => f.proofs.selections[id as usize].locations.iter().copied()
                        .for_each(&mut *visit),
                    Argument::Plan(id) => {
                        let selection = f.proofs.plans[id as usize].selection;
                        f.proofs.selections[selection as usize].locations.iter().copied().for_each(&mut *visit);
                    }
                    // A nonempty schedule is settled duty by duty, each duty an explicit operand (SPB_SCHEDULE);
                    // its list is proof data and keeps no settled duty alive.
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

pub(crate) fn metadata_uses(f: &Function) -> Vec<Set> {
    let mut sites = vec![Set::new(); f.code.len()];
    for root in &f.proofs.roots { sites[root.pc as usize].extend(&root.locations); }
    for state in &f.proofs.states { sites[state.pc as usize].extend(&state.locations); }
    sites
}

pub(crate) fn post_uses(f: &Function) -> Vec<Set> {
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

pub(crate) fn terminal(inst: &Instruction) -> bool {
    !matches!(inst, Instruction::Data { .. } | Instruction::Champ { flow: Flow::Next(_), .. })
        || matches!(inst, Instruction::Champ { op: Opcode::RetOwn | Opcode::RetDefer, .. })
}

/// Dense numbering of a function's locations: registers first, then services, so ascending indices follow
/// `Location`'s order and a bit row lists its locations in the order a `BTreeSet<Location>` does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Index {
    registers: usize,
    count: usize,
}

impl Index {
    fn new(f: &Function) -> Self {
        Self { registers: f.registers.len(), count: f.registers.len().saturating_add(f.services.len()) }
    }

    fn of(self, location: Location) -> Result<usize, Error> {
        match location {
            Location::Register(id) if (id as usize) < self.registers => Ok(id as usize),
            Location::Service(id) if (id as usize) < self.count - self.registers => Ok(self.registers + id as usize),
            _ => Err(error("allocation: invalid location")),
        }
    }

    fn location(self, index: usize) -> Location {
        if index < self.registers {
            Location::Register(index as u32)
        } else {
            Location::Service((index - self.registers) as u32)
        }
    }

    fn width(self) -> usize {
        self.count.div_ceil(64)
    }
}

fn has(words: &[u64], index: usize) -> bool {
    words[index / 64] >> (index % 64) & 1 != 0
}

/// Sets bit `index`; true when it was clear.
fn put(words: &mut [u64], index: usize) -> bool {
    let word = &mut words[index / 64];
    let bit = 1u64 << (index % 64);
    let fresh = *word & bit == 0;
    *word |= bit;
    fresh
}

/// Adds `location` to the visit's candidate set, listing it as touched.
fn mark(candidate: &mut [u64], touched: &mut Vec<u32>, location: u32) {
    if put(candidate, location as usize) {
        touched.push(location);
    }
}

fn take(words: &mut [u64], index: usize) {
    words[index / 64] &= !(1u64 << (index % 64));
}

/// The set bits of `words`, ascending.
fn ones(words: &[u64]) -> impl Iterator<Item = usize> + '_ {
    words.iter().enumerate().flat_map(|(at, &word)| {
        let mut rest = word;
        std::iter::from_fn(move || {
            (rest != 0).then(|| {
                let bit = rest.trailing_zeros() as usize;
                rest &= rest - 1;
                at * 64 + bit
            })
        })
    })
}

/// Lists per PC (or per location) in one buffer: list `at` is `items[starts[at]..starts[at + 1]]`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Lists<T> {
    starts: Vec<usize>,
    items: Vec<T>,
}

impl<T: std::marker::Copy> Lists<T> {
    fn new() -> Self {
        Self { starts: vec![0], items: Vec::new() }
    }

    fn end(&mut self) {
        self.starts.push(self.items.len());
    }

    fn get(&self, at: usize) -> &[T] {
        &self.items[self.starts[at]..self.starts[at + 1]]
    }

    /// `count` lists from `(list, item)` pairs in any order; items of one list keep their relative order.
    fn grouped(count: usize, mut pairs: Vec<(usize, T)>) -> Self {
        pairs.sort_by_key(|&(at, _)| at);
        let mut lists = Self { starts: Vec::with_capacity(count + 1), items: Vec::with_capacity(pairs.len()) };
        lists.starts.push(0);
        let mut pairs = pairs.into_iter().peekable();
        for at in 0..count {
            while let Some((_, item)) = pairs.next_if(|&(list, _)| list == at) {
                lists.items.push(item);
            }
            lists.end();
        }
        lists
    }
}

/// The dependency records of one function as adjacency over location indices: each value with its direct
/// obligations, merged over all records of the value. `close_dependencies` iterates every record until
/// nothing changes; this walks only from the locations that just joined a set and stops at members, which
/// in a closed set have brought their obligations already. Both give the same transitive closure.
#[derive(Clone, Debug)]
pub struct Obligations {
    index: Index,
    /// Bit per location: whether it is the value of some record (empty without records).
    values: Vec<u64>,
    /// The values, ascending, and the span of each one's obligations in `targets`.
    keys: Vec<u32>,
    spans: Vec<usize>,
    targets: Vec<u32>,
}

impl Obligations {
    /// The table of `f.proofs.dependencies`.
    pub fn new(f: &Function) -> Result<Self, Error> {
        Self::of(f, Index::new(f), true)
    }

    fn of(f: &Function, index: Index, included: bool) -> Result<Self, Error> {
        let mut pairs = Vec::new();
        if included {
            for dependency in &f.proofs.dependencies {
                let value = index.of(dependency.value)? as u32;
                for &obligation in &dependency.obligations {
                    pairs.push((value, index.of(obligation)? as u32));
                }
            }
        }
        pairs.sort_unstable();
        pairs.dedup();
        let mut table = Self { index, values: Vec::new(), keys: Vec::new(), spans: vec![0], targets: Vec::new() };
        if pairs.is_empty() {
            return Ok(table);
        }
        table.values = vec![0; index.width()];
        for (value, obligation) in pairs {
            if table.keys.last() != Some(&value) {
                if !table.keys.is_empty() {
                    table.spans.push(table.targets.len());
                }
                table.keys.push(value);
                put(&mut table.values, value as usize);
            }
            table.targets.push(obligation);
        }
        table.spans.push(table.targets.len());
        Ok(table)
    }

    fn of_index(&self, value: usize) -> &[u32] {
        if self.values.is_empty() || !has(&self.values, value) {
            return &[];
        }
        let Ok(at) = self.keys.binary_search(&(value as u32)) else { return &[] };
        &self.targets[self.spans[at]..self.spans[at + 1]]
    }

    /// Closes the bit set `set` over the obligations of `added`, the members that have just joined it (the
    /// others have brought theirs already), appending every location it adds to `added`. Returns the
    /// obligations walked.
    fn close_added(&self, set: &mut [u64], added: &mut Vec<u32>) -> usize {
        if self.values.is_empty() {
            return 0;
        }
        let mut walked = 0;
        let mut next = 0;
        while let Some(&value) = added.get(next) {
            next += 1;
            for &obligation in self.of_index(value as usize) {
                walked += 1;
                if put(set, obligation as usize) {
                    added.push(obligation);
                }
            }
        }
        walked
    }

    /// `close_dependencies` of `f`: adds every obligation of a member of `set`, transitively.
    pub fn close(&self, set: &mut BTreeSet<Location>) {
        if self.values.is_empty() {
            return;
        }
        let mut fresh = set.iter().filter_map(|&location| self.index.of(location).ok())
            .filter(|&value| has(&self.values, value)).collect::<Vec<_>>();
        while let Some(value) = fresh.pop() {
            for &obligation in self.of_index(value) {
                if set.insert(self.index.location(obligation as usize)) {
                    fresh.push(obligation as usize);
                }
            }
        }
    }
}

/// Which proof records keep locations live besides the instructions' own reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Parts {
    pub roots: bool,
    pub states: bool,
    pub post_roots: bool,
    pub dependencies: bool,
}

impl Parts {
    /// Every record: the liveness the allocator, its checker and the JIT's maps use.
    pub const ALL: Parts = Parts { roots: true, states: true, post_roots: true, dependencies: true };
    /// Without the root maps: what the supplied roots must cover (`check_roots`).
    pub const REQUIRED: Parts = Parts { roots: false, states: true, post_roots: false, dependencies: true };
    /// Operand reads only: no proof map or dependency keeps a token alive (suspension checks).
    pub const USES: Parts = Parts { roots: false, states: false, post_roots: false, dependencies: false };
}

/// The locations live before every PC: one ascending list of location indices per PC.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Liveness {
    index: Index,
    parts: Parts,
    rows: Lists<u32>,
}

impl Liveness {
    /// The liveness of `f` with every proof record (`live_locations`).
    pub fn of(f: &Function) -> Result<Self, Error> {
        liveness(f, Parts::ALL)
    }

    /// Number of PCs.
    pub fn len(&self) -> usize {
        self.rows.starts.len() - 1
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn row(&self, pc: usize) -> &[u32] {
        assert!(pc < self.len(), "liveness of PC {pc} beyond {}", self.len());
        self.rows.get(pc)
    }

    /// Location indices live before `pc`, ascending.
    fn indices(&self, pc: usize) -> impl Iterator<Item = usize> + '_ {
        self.row(pc).iter().map(|&at| at as usize)
    }

    /// The locations live before `pc`, in `Location` order.
    pub fn at(&self, pc: usize) -> impl Iterator<Item = Location> + '_ {
        let index = self.index;
        self.indices(pc).map(move |at| index.location(at))
    }

    pub fn contains(&self, pc: usize, location: Location) -> bool {
        pc < self.len() && self.index.of(location).is_ok_and(|at| self.row(pc).binary_search(&(at as u32)).is_ok())
    }

    /// One set per PC, as `live_locations` returns them.
    pub fn sets(&self) -> Vec<Set> {
        (0..self.len()).map(|pc| self.at(pc).collect()).collect()
    }

    /// The liveness with every record, from this one without the root maps (`Parts::REQUIRED`). Roots and
    /// post roots only add reads, so when each location they add is live already where it is added, this
    /// fixed point is also the one with them; only otherwise is it solved again.
    pub(crate) fn with_roots(self, f: &Function) -> Result<Liveness, Error> {
        if self.parts != Parts::REQUIRED || self.index != Index::new(f) || self.len() != f.code.len() {
            return Err(error("allocation: liveness of another function"));
        }
        let covered = f.proofs.roots.iter().all(|root| {
            root.locations.iter().all(|&location| self.contains(root.pc as usize, location))
        }) && f.proofs.post_roots.iter().all(|root| {
            let defined = f.code.get(root.pc as usize).map(definitions).unwrap_or_default();
            root.locations.iter().all(|&location| defined.contains(&location)
                || self.contains(root.pc as usize, location))
        });
        let live = if covered {
            Self { parts: Parts::ALL, ..self }
        } else {
            liveness(f, Parts::ALL)?
        };
        #[cfg(any(test, feature = "liveness-reference"))]
        reference::agree(&Ok(live.clone()), || liveness(f, Parts::ALL), "liveness with roots");
        Ok(live)
    }
}

/// Exact CFG fixed point with simultaneous edge renames. Every outcome, all service results,
/// dependency leaves, ownership states and root requirements participate, including zero-word deps.
pub fn live_locations(f: &Function) -> Result<Vec<Set>, Error> {
    Liveness::of(f).map(|live| live.sets())
}

/// What one PC does to the locations live after it, as location indices.
struct Transfer {
    /// Read locations and the proof maps' locations (live before the PC whatever follows).
    reads: Lists<u32>,
    definitions: Lists<u32>,
    /// Post-root locations: live after the helper, before the next instruction or edge copy.
    post: Lists<u32>,
    /// Outgoing edges: target PC and the span of their assignments.
    edges: Lists<(usize, usize, usize)>,
    /// (destination, source location or `NO_SOURCE`) per assignment.
    assignments: Vec<(u32, u32)>,
    falls_through: Vec<bool>,
    predecessors: Lists<u32>,
}

const NO_SOURCE: u32 = u32::MAX;

impl Transfer {
    fn new(f: &Function, index: Index, parts: Parts) -> Result<Self, Error> {
        let pcs = f.code.len();
        let site = |pc: u32| {
            if (pc as usize) < pcs { Ok(pc as usize) } else { Err(error("allocation: invalid proof PC")) }
        };
        let mut maps = Vec::new();
        let mut post = Vec::new();
        for (included, records, into) in [(parts.roots, &f.proofs.roots, &mut maps),
            (parts.post_roots, &f.proofs.post_roots, &mut post)]
        {
            for root in records.iter().filter(|_| included) {
                for &location in &root.locations {
                    into.push((site(root.pc)?, index.of(location)? as u32));
                }
            }
        }
        for state in f.proofs.states.iter().filter(|_| parts.states) {
            for &location in &state.locations {
                maps.push((site(state.pc)?, index.of(location)? as u32));
            }
        }
        let maps = Lists::grouped(pcs, maps);
        let mut transfer = Self {
            reads: Lists::new(), definitions: Lists::new(), post: Lists::grouped(pcs, post), edges: Lists::new(),
            assignments: Vec::new(), falls_through: Vec::with_capacity(pcs), predecessors: Lists::new(),
        };
        let mut invalid = None;
        let mut arcs = Vec::new();
        for (pc, inst) in f.code.iter().enumerate() {
            reads(f, pc, &mut |location| match index.of(location) {
                Ok(at) => transfer.reads.items.push(at as u32),
                Err(failure) => invalid = Some(failure),
            });
            if let Some(failure) = invalid.take() {
                return Err(failure);
            }
            transfer.reads.items.extend_from_slice(maps.get(pc));
            transfer.reads.end();
            for definition in definitions(inst) {
                transfer.definitions.items.push(index.of(definition)? as u32);
            }
            transfer.definitions.end();
            for edge in outgoing(inst) {
                let target = edge.target_pc as usize;
                if target >= pcs {
                    return Err(error("allocation: invalid edge PC"));
                }
                let from = transfer.assignments.len();
                for assignment in &edge.assignments {
                    let source = match assignment.source {
                        Source::Location(location) => index.of(location)? as u32,
                        Source::ProducedSlot(_) => NO_SOURCE,
                    };
                    transfer.assignments.push((index.of(assignment.destination)? as u32, source));
                }
                transfer.edges.items.push((target, from, transfer.assignments.len()));
                arcs.push((target, pc as u32));
            }
            transfer.edges.end();
            let falls = !terminal(inst) && pc + 1 < pcs;
            if falls {
                arcs.push((pc + 1, pc as u32));
            }
            transfer.falls_through.push(falls);
        }
        transfer.predecessors = Lists::grouped(pcs, arcs);
        Ok(transfer)
    }

    /// The `ordinal`th successor of `pc`: edge targets, then the fall-through PC.
    fn successor(&self, pc: usize, ordinal: usize) -> Option<usize> {
        let edges = self.edges.get(pc);
        match edges.get(ordinal) {
            Some(&(target, _, _)) => Some(target),
            None => (ordinal == edges.len() && self.falls_through[pc]).then_some(pc + 1),
        }
    }

    /// PCs in postorder of a depth-first forest: from PC 0, then from every PC not reached yet, ascending.
    /// A backward analysis taken in this order sees a PC's successors before the PC, except across back
    /// edges, so a loop-free function needs one visit per PC.
    fn postorder(&self, pcs: usize) -> Vec<usize> {
        let mut seen = vec![false; pcs];
        let mut order = Vec::with_capacity(pcs);
        let mut stack = Vec::new();
        for root in 0..pcs {
            if std::mem::replace(&mut seen[root], true) {
                continue;
            }
            stack.push((root, 0usize));
            while let Some((pc, next)) = stack.last_mut() {
                let pc = *pc;
                if let Some(target) = self.successor(pc, *next) {
                    *next += 1;
                    if !std::mem::replace(&mut seen[target], true) {
                        stack.push((target, 0));
                    }
                } else {
                    order.push(pc);
                    stack.pop();
                }
            }
        }
        order
    }
}

/// The least fixed point of `live(pc) = close(reads ∪ ((out ∪ post) \ definitions))`, where `out` joins the
/// fall-through PC's set and every edge target's set with the edge's destinations replaced by its sources.
///
/// A worklist over predecessors, taken in postorder, revisits a PC only when a successor's set grew. Sets only
/// grow, so a visit adds to the PC's list just the locations that are new in it, closes the dependency
/// obligations from those alone, and queues the predecessors when anything was added. Bit sets over the
/// location indices serve one visit at a time and are cleared through the locations it touched.
///
/// Budgets, with their codes and meanings kept from the sweeping solver before #755: storage counts the live
/// entries over all PCs (8 M, what `live_locations` returns); work counts the entries a visit reads from its
/// successors and its own list, every listed location it applies and every obligation it walks (50 M). The
/// sweeping solver counted only the size of every recomputed set, but recomputed every PC on every sweep.
pub(crate) fn liveness(f: &Function, parts: Parts) -> Result<Liveness, Error> {
    let live = solve(f, parts);
    #[cfg(any(test, feature = "liveness-reference"))]
    reference::agree(&live.as_ref().map(Liveness::sets).map_err(Clone::clone),
        || reference::live_locations(&reference::restricted(f, parts)), "liveness");
    live
}

fn solve(f: &Function, parts: Parts) -> Result<Liveness, Error> {
    let index = Index::new(f);
    if index.count.saturating_add(f.code.len()) > 8 * 1024 * 1024 {
        return Err(error("allocation liveness budget exceeded"));
    }
    let pcs = f.code.len();
    let transfer = Transfer::new(f, index, parts)?;
    let obligations = Obligations::of(f, index, parts.dependencies)?;
    let order = transfer.postorder(pcs);
    let mut position = vec![0usize; pcs];
    for (at, &pc) in order.iter().enumerate() {
        position[pc] = at;
    }
    let mut rows = vec![Vec::<u32>::new(); pcs];
    let mut queued = vec![true; pcs];
    let mut work = std::collections::BinaryHeap::from((0..pcs).map(std::cmp::Reverse).collect::<Vec<_>>());
    // The visit's set before closing (`touched` lists its candidates), the destinations of one edge, and the
    // PC's members, old and new.
    let width = index.width();
    let (mut candidate, mut renamed, mut members) = (vec![0u64; width], vec![0u64; width], vec![0u64; width]);
    let (mut touched, mut added) = (Vec::new(), Vec::new());
    let (mut entries, mut spent) = (0usize, 0usize);
    while let Some(std::cmp::Reverse(at)) = work.pop() {
        let pc = order[at];
        queued[pc] = false;
        for &(target, from, to) in transfer.edges.get(pc) {
            let assignments = &transfer.assignments[from..to];
            for &(destination, _) in assignments {
                put(&mut renamed, destination as usize);
            }
            for &location in &rows[target] {
                let (word, bit) = (location as usize / 64, 1u64 << (location % 64));
                if (renamed[word] | candidate[word]) & bit == 0 {
                    candidate[word] |= bit;
                    touched.push(location);
                }
            }
            for &(destination, source) in assignments {
                take(&mut renamed, destination as usize);
                if source != NO_SOURCE {
                    mark(&mut candidate, &mut touched, source);
                }
            }
            spent = spent.saturating_add(rows[target].len() + 2 * assignments.len() + 1);
        }
        if transfer.falls_through[pc] {
            for &location in &rows[pc + 1] {
                let (word, bit) = (location as usize / 64, 1u64 << (location % 64));
                if candidate[word] & bit == 0 {
                    candidate[word] |= bit;
                    touched.push(location);
                }
            }
            spent = spent.saturating_add(rows[pc + 1].len());
        }
        for &location in transfer.post.get(pc) {
            mark(&mut candidate, &mut touched, location);
        }
        for &location in transfer.definitions.get(pc) {
            take(&mut candidate, location as usize);
        }
        for &location in transfer.reads.get(pc) {
            mark(&mut candidate, &mut touched, location);
        }
        for &location in &rows[pc] {
            put(&mut members, location as usize);
        }
        added.clear();
        for &location in &touched {
            if has(&candidate, location as usize) && put(&mut members, location as usize) {
                added.push(location);
            }
            take(&mut candidate, location as usize);
        }
        touched.clear();
        let walked = obligations.close_added(&mut members, &mut added);
        for &location in rows[pc].iter().chain(&added) {
            take(&mut members, location as usize);
        }
        spent = spent.saturating_add(rows[pc].len() + walked + 1 + transfer.post.get(pc).len()
            + transfer.definitions.get(pc).len() + transfer.reads.get(pc).len());
        enforce(spent <= 50_000_000, "ALLOCATION_WORK", "allocation liveness work budget exceeded")?;
        if !added.is_empty() {
            entries = entries.saturating_add(added.len());
            enforce(entries <= 8 * 1024 * 1024, "ALLOCATION_MEMORY", "allocation liveness storage exceeded")?;
            added.sort_unstable();
            let old = std::mem::take(&mut rows[pc]);
            let mut merged = Vec::with_capacity(old.len() + added.len());
            let (mut left, mut right) = (old.iter().peekable(), added.iter().peekable());
            while let (Some(&&a), Some(&&b)) = (left.peek(), right.peek()) {
                merged.push(if a < b { left.next(); a } else { right.next(); b });
            }
            merged.extend(left);
            merged.extend(right);
            rows[pc] = merged;
            for &predecessor in transfer.predecessors.get(pc) {
                let predecessor = predecessor as usize;
                if !std::mem::replace(&mut queued[predecessor], true) {
                    work.push(std::cmp::Reverse(position[predecessor]));
                }
            }
        }
    }
    let mut lists = Lists::new();
    for row in rows {
        lists.items.extend(row);
        lists.end();
    }
    Ok(Liveness { index, parts, rows: lists })
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

/// Functions with at most this many locations keep their interference graph as a bit matrix (8 MiB at most).
const DENSE_INTERFERENCE: usize = 8 * 1024;

/// The interference graph over location indices, built from cliques in order. A clique within the last one
/// adds no edge, and its size is within the last one's, so it is skipped; the budgets see the same edges in
/// the same order as with every clique applied.
struct Interference {
    width: usize,
    /// Bit matrix, row per location, when the function is small enough; otherwise sparse rows.
    dense: Vec<u64>,
    sparse: BTreeMap<u32, BTreeSet<u32>>,
    edges: usize,
    last: Vec<u32>,
}

impl Interference {
    fn new(index: Index) -> Self {
        let width = index.width();
        let dense = if index.count <= DENSE_INTERFERENCE { vec![0; index.count * width] } else { Vec::new() };
        Self { width, dense, sparse: BTreeMap::new(), edges: 0, last: Vec::new() }
    }

    /// Every two members interfere. `members` holds location indices in any order, repeats allowed.
    fn clique(&mut self, members: &mut Vec<u32>) -> Result<(), Error> {
        members.sort_unstable();
        members.dedup();
        enforce(members.len().saturating_mul(members.len().saturating_sub(1)) <= 8 * 1024 * 1024,
            "ALLOCATION_MEMORY", "interference clique exceeds storage budget")?;
        let mut last = self.last.iter().peekable();
        if members.iter().all(|member| last.find(|&&other| other >= *member) == Some(member)) {
            return Ok(());
        }
        for &location in members.iter() {
            for &other in members.iter().filter(|&&other| other != location) {
                let added = if self.dense.is_empty() {
                    self.sparse.entry(location).or_default().insert(other)
                } else {
                    put(&mut self.dense[location as usize * self.width..(location as usize + 1) * self.width],
                        other as usize)
                };
                self.edges += usize::from(added);
            }
            enforce(self.edges <= 8 * 1024 * 1024, "ALLOCATION_MEMORY", "interference graph exceeds storage budget")?;
        }
        self.last.clone_from(members);
        Ok(())
    }

    /// Calls `visit` with each neighbour of `location` below it.
    fn earlier(&self, location: usize, visit: &mut impl FnMut(usize)) {
        if self.dense.is_empty() {
            let neighbours = self.sparse.get(&(location as u32)).into_iter()
                .flat_map(|neighbours| neighbours.range(..location as u32));
            neighbours.for_each(|&other| visit(other as usize));
        } else {
            let row = &self.dense[location * self.width..location * self.width + location / 64 + 1];
            ones(row).take_while(|&other| other < location).for_each(visit);
        }
    }
}

pub fn allocate(f: &Function) -> Result<Allocation, Error> {
    allocate_with(f, &Liveness::of(f)?)
}

/// `allocate` over the function's liveness with every proof record (`Liveness::of`).
pub fn allocate_with(f: &Function, live: &Liveness) -> Result<Allocation, Error> {
    if live.parts != Parts::ALL || live.index != Index::new(f) || live.len() != f.code.len() {
        return Err(error("allocation: liveness of another function"));
    }
    let allocation = allocate_live(f, live);
    #[cfg(any(test, feature = "liveness-reference"))]
    reference::agree(&allocation, || reference::allocate(f), "allocation");
    allocation
}

fn allocate_live(f: &Function, live: &Liveness) -> Result<Allocation, Error> {
    let index = Index::new(f);
    let pcs = f.code.len();
    let mut post = Vec::new();
    for root in &f.proofs.post_roots {
        enforce((root.pc as usize) < pcs, "SPB_STRUCTURE", "allocation: invalid proof PC")?;
        for &location in &root.locations {
            post.push((root.pc as usize, index.of(location)? as u32));
        }
    }
    let post = Lists::grouped(pcs, post);
    let mut graph = Interference::new(index);
    let mut members = Vec::new();
    // Entry adapters store every parameter before their first collecting call, including externals.
    for &param in &f.params {
        members.push(index.of(param)? as u32);
    }
    graph.clique(&mut members)?;
    for pc in 0..pcs {
        members.clear();
        members.extend(live.indices(pc).map(|at| at as u32));
        graph.clique(&mut members)?;
    }
    for block in &f.blocks {
        members.clear();
        for &param in &block.params {
            members.push(index.of(param)? as u32);
        }
        enforce((block.pc as usize) < pcs, "SPB_STRUCTURE", "allocation: invalid block PC")?;
        members.extend(live.indices(block.pc as usize).map(|at| at as u32));
        graph.clique(&mut members)?;
    }
    for (pc, inst) in f.code.iter().enumerate() {
        if !terminal(inst) {
            // The definitions with the live-out alone form a subset of this clique: no edge of their own.
            members.clear();
            for definition in definitions(inst) {
                members.push(index.of(definition)? as u32);
            }
            members.extend_from_slice(post.get(pc));
            if pc + 1 < pcs {
                members.extend(live.indices(pc + 1).map(|at| at as u32));
            }
            graph.clique(&mut members)?;
        }
    }
    let mut slots = Vec::new();
    let mut slot_of = Vec::with_capacity(index.count);
    // `busy[slot] == stamp` marks the slots of the current location's earlier neighbours.
    let mut busy = Vec::new();
    for at in 0..index.count {
        let ty = location_type(f, index.location(at))?;
        let stamp = at + 1;
        busy.resize(slots.len(), 0);
        graph.earlier(at, &mut |other| busy[slot_of[other] as usize] = stamp);
        let slot = slots.iter().zip(&busy).position(|(&kind, &mark)| kind == ty && mark != stamp)
            .unwrap_or(slots.len());
        if slot == slots.len() {
            slots.push(ty);
        }
        slot_of.push(slot as u32);
    }
    let mut allocation = Allocation {
        locations: slot_of.iter().enumerate().map(|(at, &slot)| (index.location(at), slot)).collect(),
        slots, edges: BTreeMap::new(), roots: BTreeMap::new(),
    };
    let slot = |location: Location| index.of(location).map(|at| slot_of[at]);
    for (pc, inst) in f.code.iter().enumerate() {
        for (ordinal, edge) in outgoing(inst).into_iter().enumerate() {
            let mut pending = Vec::with_capacity(edge.assignments.len());
            for assignment in &edge.assignments {
                let copy = Copy {
                    destination: slot(assignment.destination)?,
                    source: match assignment.source {
                        Source::Location(location) => CopySource::Slot(slot(location)?),
                        Source::ProducedSlot(slot) => CopySource::Produced(slot),
                    },
                };
                if copy.source != CopySource::Slot(copy.destination) {
                    pending.push(copy);
                }
            }
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
    allocation.roots = allocated_roots_with(f, &allocation, &Obligations::of(f, index, true)?)?;
    Ok(allocation)
}

/// Independent forward symbolic execution: no interference graph or allocator liveness is trusted.
/// Joins intersect known contents; loops run to a fixed point. Copies execute sequentially and are
/// compared against the original parallel edge, so stale, clobbered and wrong-outcome values fail.
///
/// A state holds, for every frame slot, the sorted locations whose value the slot is known to hold.
/// The fixed point is the greatest one whatever the visiting order (every transfer is monotone and joins
/// intersect), so the worklist visits in reverse postorder, each queued PC once (#701).
pub fn check_allocation(f: &Function, allocation: &Allocation) -> Result<(), Error> {
    let index = Index::new(f);
    let obligations = Obligations::of(f, index, true)?;
    if allocation.roots != allocated_roots_with(f, allocation, &obligations)? {
        return Err(error("allocation clobbered concrete roots"));
    }
    let mut slots = Vec::with_capacity(index.count);
    for at in 0..index.count {
        let location = index.location(at);
        let slot = *allocation.locations.get(&location).ok_or_else(|| error("allocation missing location"))?;
        if allocation.slots.get(slot as usize) != Some(&location_type(f, location)?) {
            return Err(error("allocation slot type mismatch"));
        }
        slots.push(slot as usize);
    }
    // (slot, index) of a location of the function; every location has a checked slot.
    let slot = |location: Location| -> Result<(usize, u32), Error> {
        let at = index.of(location).map_err(|_| error("allocation missing location"))?;
        Ok((slots[at], at as u32))
    };
    let mut entry = vec![Vec::new(); allocation.slots.len()];
    for &param in &f.params {
        let (held, at) = slot(param)?;
        if !entry[held].is_empty() {
            return Err(error("allocation clobbers entry parameters"));
        }
        entry[held].push(at);
    }
    let entry = State::from_lists(entry);
    let mut incoming: Vec<Option<State>> = vec![None; f.code.len()];
    if incoming.is_empty() {
        if f.proofs.entry.as_ref().is_some_and(|entry| entry.external.is_some()) {
            return Ok(());
        }
        return Err(error("allocation requires code"));
    }
    let mut stored = entry.size();
    let mut visits = 0usize;
    incoming[0] = Some(entry);
    let metadata = metadata_uses(f);
    let post = post_uses(f);
    let order = reverse_postorder(f);
    let mut work = Worklist::new(&order);
    work.push(0);
    while let Some(pc) = work.pop() {
        let mut state = incoming[pc].as_ref().ok_or_else(|| error("allocation worklist state"))?.clone();
        visits = visits.saturating_add(state.size());
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
        obligations.close(&mut used);
        for location in used {
            let (held, at) = slot(location)?;
            if !state.holds(held, at) {
                return Err(error(format!("allocation clobbered {location:?} at pc {pc}")));
            }
        }
        for definition in definitions(&f.code[pc]) {
            let (held, at) = slot(definition)?;
            state.define(held, at);
        }
        for &location in &post[pc] {
            let (held, at) = slot(location)?;
            if !state.holds(held, at) {
                return Err(error(format!("allocation clobbered post root at pc {pc}")));
            }
        }
        let mut successors = Vec::new();
        for (ordinal, edge) in outgoing(&f.code[pc]).into_iter().enumerate() {
            visits = visits.saturating_add(state.size());
            enforce(visits <= 50_000_000, "ALLOCATION_WORK", "allocation edge work budget exceeded")?;
            successors.push((edge.target_pc as usize, follow_edge(&state, allocation, pc, ordinal, edge, &slot)?));
        }
        if !terminal(&f.code[pc]) && pc + 1 < f.code.len() {
            successors.push((pc + 1, state));
        }
        for (target, next) in successors {
            let previous = incoming.get_mut(target).ok_or_else(|| error("allocation invalid target"))?;
            if let Some(previous) = previous {
                if previous.intersect(&next) {
                    work.push(target);
                }
            } else {
                stored = stored.saturating_add(next.size());
                enforce(stored <= 8 * 1024 * 1024, "ALLOCATION_MEMORY", "allocation checker storage exceeded")?;
                *previous = Some(next);
                work.push(target);
            }
        }
    }
    Ok(())
}

/// Contents of every frame slot at one PC: slot `s` is known to hold the locations (as ascending indices,
/// which is `Location` order) `items[spans[s]..spans[s + 1]]`. One buffer per state, whatever its slots.
#[derive(Clone, Debug)]
struct State {
    spans: Vec<usize>,
    items: Vec<u32>,
}

impl State {
    fn from_lists(lists: Vec<Vec<u32>>) -> Self {
        let mut state = Self { spans: Vec::with_capacity(lists.len() + 1), items: Vec::new() };
        state.spans.push(0);
        for list in lists {
            state.items.extend(list);
            state.spans.push(state.items.len());
        }
        state
    }

    fn slots(&self) -> usize {
        self.spans.len() - 1
    }

    fn slot(&self, slot: usize) -> &[u32] {
        &self.items[self.spans[slot]..self.spans[slot + 1]]
    }

    /// Stored entries: one per slot plus one per known location (the budget's unit).
    fn size(&self) -> usize {
        self.slots() + self.items.len()
    }

    fn holds(&self, slot: usize, location: u32) -> bool {
        self.slot(slot).binary_search(&location).is_ok()
    }

    /// A definition: no slot holds the location's old value any more, and `slot` holds only the new one.
    fn define(&mut self, slot: usize, location: u32) {
        self.retain(|item| item != location);
        self.replace(slot, &[location]);
    }

    /// Keeps the locations `keep` accepts, in every slot.
    fn retain(&mut self, mut keep: impl FnMut(u32) -> bool) {
        let mut write = 0;
        let mut start = 0;
        for at in 0..self.slots() {
            let end = self.spans[at + 1];
            self.spans[at] = write;
            for read in start..end {
                let item = self.items[read];
                if keep(item) {
                    self.items[write] = item;
                    write += 1;
                }
            }
            start = end;
        }
        let slots = self.slots();
        self.spans[slots] = write;
        self.items.truncate(write);
    }

    /// Slot `slot` holds exactly `contents` (ascending).
    fn replace(&mut self, slot: usize, contents: &[u32]) {
        let (start, end) = (self.spans[slot], self.spans[slot + 1]);
        self.items.splice(start..end, contents.iter().copied());
        let length = contents.len();
        for span in &mut self.spans[slot + 1..] {
            *span = *span - (end - start) + length;
        }
    }

    /// Keeps in every slot only the locations `other` holds there too; true when anything was dropped.
    fn intersect(&mut self, other: &State) -> bool {
        if self.items == other.items && self.spans == other.spans {
            return false;
        }
        let before = self.items.len();
        let mut write = 0;
        let mut start = 0;
        for at in 0..self.slots() {
            let end = self.spans[at + 1];
            self.spans[at] = write;
            let theirs = other.slot(at);
            let mut cursor = 0;
            for read in start..end {
                let item = self.items[read];
                while theirs.get(cursor).is_some_and(|&candidate| candidate < item) {
                    cursor += 1;
                }
                if theirs.get(cursor) == Some(&item) {
                    self.items[write] = item;
                    write += 1;
                }
            }
            start = end;
        }
        let slots = self.slots();
        self.spans[slots] = write;
        self.items.truncate(write);
        write != before
    }
}

/// What a slot holds while an edge's copies run: the pre-edge contents of a slot, or a produced value.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Held {
    Slot(usize),
    Produced(u32),
}

/// Run the edge's sequential copies over `state`, check every parallel assignment against the pre-edge
/// value, then rename the SSA parameters simultaneously. A copy moves a reference to what its source slot
/// holds, so it costs O(1) whatever the slot holds.
fn follow_edge(state: &State, allocation: &Allocation, pc: usize, ordinal: usize, edge: &Edge,
    slot: &dyn Fn(Location) -> Result<(usize, u32), Error>) -> Result<State, Error>
{
    let mut held = (0..state.slots()).map(Held::Slot).collect::<Vec<_>>();
    let copies = allocation.edges.get(&(pc as u32, ordinal as u32))
        .ok_or_else(|| error("allocation missing edge copies"))?;
    for copy in copies {
        let value = match copy.source {
            CopySource::Slot(index) => *held.get(index as usize)
                .ok_or_else(|| error("allocation invalid copy source"))?,
            CopySource::Produced(index) => Held::Produced(index),
        };
        let destination = held.get_mut(copy.destination as usize)
            .ok_or_else(|| error("allocation invalid copy destination"))?;
        *destination = value;
    }
    // Check every destination against the pre-edge value; no assignment-order assumptions.
    let mut renamed = Vec::with_capacity(edge.assignments.len());
    for assignment in &edge.assignments {
        let (destination, at) = slot(assignment.destination)?;
        let present = match (held[destination], assignment.source) {
            (Held::Slot(index), Source::Location(location)) => slot(location)
                .is_ok_and(|(_, source)| state.holds(index, source)),
            (Held::Produced(index), Source::ProducedSlot(expected)) => index == expected,
            _ => false,
        };
        if !present {
            return Err(error(format!("allocation clobbered parallel assignment at pc {pc}")));
        }
        renamed.push((destination, at));
    }
    // SSA parameters are renamed simultaneously after checking all pre-edge sources: no slot keeps a
    // destination's old value, and each destination's slot gains its new one.
    renamed.sort_unstable();
    renamed.dedup();
    let mut dropped = renamed.iter().map(|&(_, location)| location).collect::<Vec<_>>();
    dropped.sort_unstable();
    // Slots the copies reached hold what their source held before the edge, or nothing for a produced value.
    let mut next = state.clone();
    for (at, value) in held.into_iter().enumerate() {
        match value {
            Held::Slot(index) if index == at => {}
            Held::Slot(index) => next.replace(at, state.slot(index)),
            Held::Produced(_) => next.replace(at, &[]),
        }
    }
    if !dropped.is_empty() {
        next.retain(|item| dropped.binary_search(&item).is_err());
    }
    for &(destination, location) in &renamed {
        let contents = next.slot(destination);
        let mut gained = Vec::with_capacity(contents.len() + 1);
        gained.extend_from_slice(contents);
        if let Err(position) = gained.binary_search(&location) {
            gained.insert(position, location);
        }
        next.replace(destination, &gained);
    }
    Ok(next)
}

/// Rank of every PC reachable from PC 0 in a reverse postorder of the instruction graph (outcome edges and
/// fall-through); `usize::MAX` for unreachable PCs. Edges to invalid PCs are ignored here: the checker
/// rejects them when it follows them.
pub(crate) fn reverse_postorder(f: &Function) -> Vec<usize> {
    let successors = |pc: usize| {
        let inst = &f.code[pc];
        let mut next = outgoing(inst).into_iter().map(|edge| edge.target_pc as usize).collect::<Vec<_>>();
        if !terminal(inst) {
            next.push(pc + 1);
        }
        next.retain(|&target| target < f.code.len());
        next
    };
    let mut rank = vec![usize::MAX; f.code.len()];
    if f.code.is_empty() {
        return rank;
    }
    let mut seen = vec![false; f.code.len()];
    let mut postorder = Vec::with_capacity(f.code.len());
    // Iterative depth-first search: (PC, its successors, index of the next successor to follow).
    let mut stack = vec![(0usize, successors(0), 0usize)];
    seen[0] = true;
    while let Some((pc, next, index)) = stack.last_mut() {
        if let Some(&target) = next.get(*index) {
            *index += 1;
            if !seen[target] {
                seen[target] = true;
                stack.push((target, successors(target), 0));
            }
        } else {
            postorder.push(*pc);
            stack.pop();
        }
    }
    for (position, pc) in postorder.into_iter().rev().enumerate() {
        rank[pc] = position;
    }
    rank
}

/// PCs waiting for a visit, taken lowest rank first; a PC already waiting is not queued twice.
struct Worklist<'a> {
    rank: &'a [usize],
    queued: BTreeSet<(usize, usize)>,
}

impl<'a> Worklist<'a> {
    fn new(rank: &'a [usize]) -> Self {
        Self { rank, queued: BTreeSet::new() }
    }

    fn push(&mut self, pc: usize) {
        let rank = self.rank.get(pc).copied().unwrap_or(usize::MAX);
        self.queued.insert((rank, pc));
    }

    fn pop(&mut self) -> Option<usize> {
        self.queued.pop_first().map(|(_, pc)| pc)
    }
}

type RootMaps = BTreeMap<(u32, bool), Vec<AllocatedRoot>>;

#[cfg(test)]
pub(crate) fn allocated_roots(f: &Function, allocation: &Allocation) -> Result<RootMaps, Error> {
    allocated_roots_with(f, allocation, &Obligations::new(f)?)
}

fn allocated_roots_with(f: &Function, allocation: &Allocation, obligations: &Obligations)
    -> Result<RootMaps, Error>
{
    let maps = root_maps(f, allocation, obligations);
    #[cfg(any(test, feature = "liveness-reference"))]
    reference::agree(&maps, || reference::allocated_roots(f, allocation), "allocated roots");
    maps
}

fn root_maps(f: &Function, allocation: &Allocation, obligations: &Obligations) -> Result<RootMaps, Error> {
    let index = obligations.index;
    let mut slots = vec![None; index.count];
    for (&location, &slot) in &allocation.locations {
        if let Ok(at) = index.of(location) {
            slots[at] = Some(slot);
        }
    }
    let mut maps = RootMaps::new();
    let mut members = vec![0u64; index.width()];
    let mut closed = Vec::new();
    for (after, requirements) in [(false, &f.proofs.roots), (true, &f.proofs.post_roots)] {
        for root in requirements {
            closed.clear();
            for &location in &root.locations {
                let at = index.of(location)?;
                if put(&mut members, at) {
                    closed.push(at as u32);
                }
            }
            obligations.close_added(&mut members, &mut closed);
            let map = maps.entry((root.pc, after)).or_default();
            for &at in &closed {
                take(&mut members, at as usize);
            }
            for &at in &closed {
                let slot = slots[at as usize].ok_or_else(|| error("missing root location"))?;
                match location_type(f, index.location(at as usize))? {
                    ProducedType::Service(kind) => {
                        for &offset in layout::service_layout(kind).references {
                            map.push(AllocatedRoot { slot, offset, tagged_int: false });
                        }
                    }
                    ProducedType::Value(Type::Bool | Type::Never | Type::Descriptor | Type::Byte | Type::Float
                        | Type::Char) => {}
                    ProducedType::Value(ty) => {
                        map.push(AllocatedRoot { slot, offset: 0, tagged_int: ty == Type::Int });
                    }
                }
            }
        }
    }
    for roots in maps.values_mut() {
        roots.sort_unstable();
        roots.dedup();
    }
    Ok(maps)
}
