//! The temporary take of a Future's cached outcome in SPB 17 (#601 R4). `await.take` moves the outcome packet out
//! of the cache only with the take capability, which only `unique_cache.proof` produces. The capability takes no
//! word: the loader proves it here (Champ draft 3 §§4.2, 15; OIR-2 §10.3; Champ draft 2 §7.3). The Future must be
//! a temporary of this body: created by a `fut.new` of the function, never published, stored, passed or awaited
//! by anyone else (the negative case `box.f` of draft 2), and taken once.
//!
//! "Nothing in between" needs no path search here. A publication anywhere in the function already refuses the
//! take. The capability is linear and only its take consumes it, so it cannot meet a second proof or take of the
//! same cache before its own; and it waits in the frame only as its take's operand (`check_layouts`), so any other
//! suspension between proof and take finds it live outside the frame (`SPB_FRAME_LINEAR`).
use super::*;
use std::collections::{BTreeMap, BTreeSet};

type Check = Result<(), Error>;

/// `await.take` consumes a capability of `unique_cache.proof` on the same Future, which this function created
/// and nobody else can observe; no publication, alias store or await comes between the proof and the take, and a
/// taken cache is never proved or taken again.
pub(crate) const TAKE_CAPABILITY: &str = "SPB_TAKE_CAPABILITY";

/// Where a Future value may come from: a `fut.new` of this function at a pc, or anything else (a parameter,
/// a call result, the start continuation).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Origin {
    Created(u32),
    Foreign,
}

/// Steps of the whole analysis of one function, beyond which the load gate gives up (`LOAD_WORK_BUDGET`).
const WORK_BUDGET: usize = 16 * 1024 * 1024;

struct Analysis<'a> {
    f: &'a Function,
    work: usize,
}

fn locations(arguments: &[Argument]) -> Vec<Location> {
    arguments.iter().filter_map(|argument| match argument {
        Argument::Location(location) => Some(*location),
        _ => None,
    }).collect()
}

fn future(f: &Function, location: Location) -> bool {
    matches!(location, Location::Register(id) if matches!(f.registers.get(id as usize), Some(Type::Future(_))))
}

fn unique(f: &Function, location: Location) -> bool {
    matches!(location, Location::Service(id) if f.services.get(id as usize) == Some(&ServiceKind::UniqueCache))
}

/// The operation of `op` at `pc` with its arguments, if it is one.
fn champ(f: &Function, pc: usize) -> Option<(Opcode, &[Argument], &Flow)> {
    match f.code.get(pc)? {
        Instruction::Champ { op, arguments, flow } => Some((*op, arguments.as_slice(), flow)),
        _ => None,
    }
}

impl<'a> Analysis<'a> {
    fn step(&mut self, amount: usize) -> Check {
        self.work = self.work.saturating_add(amount);
        enforce(self.work <= WORK_BUDGET, "LOAD_WORK_BUDGET", "take capability analysis exceeds work budget")
    }

    /// The control successors of `pc`, as the other load analyses read them.
    fn successors(&self, pc: usize) -> Vec<usize> {
        let instruction = &self.f.code[pc];
        let mut targets = allocation::outgoing(instruction).iter().map(|edge| edge.target_pc as usize)
            .collect::<Vec<_>>();
        if matches!(instruction, Instruction::Data { .. } | Instruction::Champ { flow: Flow::Next(_), .. })
            && !matches!(instruction, Instruction::Champ { op: Opcode::RetOwn | Opcode::RetDefer, .. })
            && pc + 1 < self.f.code.len()
        {
            targets.push(pc + 1);
        }
        targets
    }

    /// The pcs reachable from the successors of `from` along `next`, never expanding past a pc of `stops`.
    fn reach(&mut self, from: &[usize], next: &BTreeMap<usize, Vec<usize>>, stops: &[usize])
        -> Result<BTreeSet<usize>, Error>
    {
        let mut seen = BTreeSet::new();
        let mut work = from.iter().flat_map(|pc| next.get(pc).into_iter().flatten().copied()).collect::<Vec<_>>();
        while let Some(pc) = work.pop() {
            self.step(1)?;
            if !seen.insert(pc) || stops.contains(&pc) {
                continue;
            }
            work.extend(next.get(&pc).into_iter().flatten().copied());
        }
        Ok(seen)
    }

    /// The origins of every Future register: through edges, moves and each continuation's save to its resumes.
    fn future_origins(&mut self) -> Result<BTreeMap<Location, BTreeSet<Origin>>, Error> {
        let f = self.f;
        let mut flows = BTreeMap::<Location, Vec<Location>>::new();
        let mut seeds = Vec::new();
        let mut saves = BTreeMap::<u32, &[Argument]>::new();
        for instruction in &f.code {
            if let Instruction::Champ { op: Opcode::FrameSave, arguments, .. } = instruction
                && let Some(continuation) = suspension::continuation_of(arguments)
            {
                saves.insert(continuation, arguments);
            }
        }
        for &param in &f.params {
            if future(f, param) {
                seeds.push((param, Origin::Foreign));
            }
        }
        for (pc, instruction) in f.code.iter().enumerate() {
            for edge in allocation::outgoing(instruction) {
                for assignment in edge.assignments.iter().filter(|assignment| future(f, assignment.destination)) {
                    match assignment.source {
                        Source::Location(source) => flows.entry(source).or_default().push(assignment.destination),
                        Source::ProducedSlot(_) => seeds.push((assignment.destination, Origin::Foreign)),
                    }
                }
            }
            match instruction {
                Instruction::Data { op: DataOp::Move, inputs, output: Some(output), .. } => {
                    if let [input] = inputs[..] {
                        flows.entry(Location::Register(input)).or_default().push(Location::Register(*output));
                    }
                }
                Instruction::Data { output: Some(output), .. } if future(f, Location::Register(*output)) => {
                    seeds.push((Location::Register(*output), Origin::Foreign));
                }
                Instruction::Champ { op, arguments, flow: Flow::Next(outputs) } => {
                    let resumed = (*op == Opcode::FrameResume).then(|| suspension::continuation_of(arguments))
                        .flatten().filter(|&continuation| continuation != 0)
                        .and_then(|continuation| saves.get(&continuation));
                    for (index, &output) in outputs.iter().enumerate().filter(|(_, output)| future(f, **output)) {
                        match (op, resumed) {
                            (Opcode::FutureNew, _) if index == 0 => seeds.push((output, Origin::Created(pc as u32))),
                            // A save lists the frame, then its values; a resume the frame, then the same values.
                            (Opcode::FrameResume, Some(saved)) => {
                                let source = locations(saved).get(index).copied()
                                    .ok_or_else(|| error("frame.resume outside its continuation"))?;
                                flows.entry(source).or_default().push(output);
                            }
                            _ => seeds.push((output, Origin::Foreign)),
                        }
                    }
                }
                _ => {}
            }
        }
        let mut origins = BTreeMap::<Location, BTreeSet<Origin>>::new();
        let mut work = Vec::new();
        for (location, origin) in seeds {
            if origins.entry(location).or_default().insert(origin) {
                work.push(location);
            }
        }
        while let Some(location) = work.pop() {
            let known = origins[&location].clone();
            for &destination in flows.get(&location).into_iter().flatten() {
                self.step(known.len())?;
                let entry = origins.entry(destination).or_default();
                let before = entry.len();
                entry.extend(&known);
                if entry.len() != before {
                    work.push(destination);
                }
            }
        }
        Ok(origins)
    }

    /// The proofs whose capability each location may hold, through edges.
    fn capability_origins(&mut self) -> Result<BTreeMap<Location, BTreeSet<usize>>, Error> {
        let f = self.f;
        let mut origins = BTreeMap::<Location, BTreeSet<usize>>::new();
        let mut flows = BTreeMap::<Location, Vec<Location>>::new();
        let mut work = Vec::new();
        for (pc, instruction) in f.code.iter().enumerate() {
            for edge in allocation::outgoing(instruction) {
                for assignment in edge.assignments.iter().filter(|assignment| unique(f, assignment.destination)) {
                    if let Source::Location(source) = assignment.source {
                        flows.entry(source).or_default().push(assignment.destination);
                    }
                }
            }
            if let Instruction::Champ { op: Opcode::UniqueCacheProof, flow: Flow::Next(outputs), .. } = instruction
                && let Some(&output) = outputs.first()
            {
                origins.entry(output).or_default().insert(pc);
                work.push(output);
            }
        }
        while let Some(location) = work.pop() {
            let known = origins[&location].clone();
            for &destination in flows.get(&location).into_iter().flatten() {
                self.step(known.len())?;
                let entry = origins.entry(destination).or_default();
                let before = entry.len();
                entry.extend(&known);
                if entry.len() != before {
                    work.push(destination);
                }
            }
        }
        Ok(origins)
    }

    /// The creations whose Futures something publishes: every use of a Future register except a copy (edge,
    /// move, frame save), its proof, its take and the observations `fut.done` and `fut.cancelled`. An await is a
    /// second waiter with an outcome view; a call, a creation, an exit or a publication hands the Future to another
    /// holder (the `box.f` of draft 2).
    fn published(&mut self, origins: &BTreeMap<Location, BTreeSet<Origin>>) -> Result<BTreeSet<Origin>, Error> {
        let f = self.f;
        let mut published = BTreeSet::new();
        for (pc, instruction) in f.code.iter().enumerate() {
            let uses = allocation::direct_uses(f, pc);
            self.step(uses.len())?;
            let copies = match instruction {
                Instruction::Data { op: DataOp::Move, .. } => uses.clone(),
                Instruction::Champ { op, arguments, .. } => {
                    let operands = locations(arguments);
                    match op {
                        Opcode::FrameSave => operands.into_iter().skip(1).collect(),
                        Opcode::UniqueCacheProof | Opcode::AwaitTake | Opcode::FutureDone | Opcode::FutureCancelled => {
                            operands.into_iter().take(1).collect()
                        }
                        _ => BTreeSet::new(),
                    }
                }
                _ => BTreeSet::new(),
            };
            for location in uses.iter().filter(|location| future(f, **location) && !copies.contains(location)) {
                published.extend(origins.get(location).into_iter().flatten());
            }
        }
        Ok(published)
    }
}

/// The creation a take or proof names: its Future operand's single origin.
fn creation(origins: &BTreeMap<Location, BTreeSet<Origin>>, location: Option<Location>) -> Option<u32> {
    match origins.get(&location?)?.iter().collect::<Vec<_>>()[..] {
        [Origin::Created(pc)] => Some(*pc),
        _ => None,
    }
}

/// verify_load(): every `await.take` of function `index` holds its capability (`SPB_TAKE_CAPABILITY`).
pub(crate) fn check(p: &Program, index: u32, f: &Function) -> Check {
    let takes = (0..f.code.len()).filter(|&pc| matches!(champ(f, pc), Some((Opcode::AwaitTake, _, _))))
        .collect::<Vec<_>>();
    if takes.is_empty() {
        return Ok(());
    }
    let mut analysis = Analysis { f, work: 0 };
    let origins = analysis.future_origins()?;
    let capabilities = analysis.capability_origins()?;
    let published = analysis.published(&origins)?;
    let mut successors = BTreeMap::<usize, Vec<usize>>::new();
    for pc in 0..f.code.len() {
        successors.insert(pc, analysis.successors(pc));
    }
    // The proof and the take of each creation, which a taken cache never meets again.
    let mut sites = BTreeMap::<u32, Vec<usize>>::new();
    for pc in 0..f.code.len() {
        if let Some((Opcode::UniqueCacheProof | Opcode::AwaitTake, arguments, _)) = champ(f, pc)
            && let Some(created) = creation(&origins, locations(arguments).first().copied())
        {
            sites.entry(created).or_default().push(pc);
        }
    }
    for &take in &takes {
        let Some((_, arguments, _)) = champ(f, take) else { continue };
        let operands = locations(arguments);
        let boundary = arguments.iter().find_map(|argument| match argument {
            Argument::Scope(id) => Some(*id),
            _ => None,
        });
        let own = boundary.and_then(|id| p.tables.scopes.get(id as usize)).is_some_and(|scope| scope.function == index);
        enforce(own, TAKE_CAPABILITY, "the boundary of a temporary take is a scope of the awaiting body")?;
        // Every proof whose capability can reach the take names the Future the take awaits.
        let proofs = operands.get(1).and_then(|capability| capabilities.get(capability)).cloned().unwrap_or_default();
        let created = creation(&origins, operands.first().copied());
        let proved = |proof: &usize| {
            let future = champ(f, *proof).and_then(|(_, arguments, _)| locations(arguments).first().copied());
            creation(&origins, future)
        };
        enforce(created.is_some() && !proofs.is_empty() && proofs.iter().all(|proof| proved(proof) == created),
            TAKE_CAPABILITY, "await.take takes the Future its proof names, a temporary that this body created")?;
        let created = created.unwrap_or_default();
        enforce(!published.contains(&Origin::Created(created)), TAKE_CAPABILITY,
            "a Future another holder can observe has no take capability")?;
        // After the take, until the Future is created again: no proof or take of the consumed cache.
        let later = analysis.reach(&[take], &successors, &[created as usize])?;
        enforce(sites[&created].iter().all(|site| !later.contains(site)), TAKE_CAPABILITY,
            "a taken cache is proved or taken again")?;
    }
    Ok(())
}
