//! Typed allocation is checked independently before its physical offsets enter Cranelift maps.
use super::*;
use std::collections::BTreeSet;

type Map = BTreeSet<u32>;

pub(super) fn outgoing(instruction: &s::Instruction) -> Vec<&s::Edge> {
    match instruction {
        s::Instruction::Br(edge) => vec![edge],
        s::Instruction::Cond { yes, no, .. } => vec![yes, no],
        s::Instruction::Champ { flow: s::Flow::Outcomes(outcomes), .. } => {
            outcomes.iter().map(|outcome| &outcome.edge).collect()
        }
        _ => vec![],
    }
}

pub(super) struct Frame {
    pub allocation: s::Allocation,
    pub offsets: Vec<u32>,
    pub words: Vec<u32>,
    pub bytes: usize,
    pub traced: Map,
    before: Vec<Map>,
    after: Vec<Map>,
}

impl Frame {
    pub fn new(source: &s::Function) -> Result<Self, Error> {
        let allocation = s::allocate(source).map_err(|error| Error(error.to_string()))?;
        // Forward symbolic execution checks all outcomes, metadata reads, ownership lifetimes,
        // cycle scratch, parallel edge copies and post roots independently of the live sets.
        s::check_allocation(source, &allocation).map_err(|error| Error(error.to_string()))?;
        let mut frame = Self {
            allocation, offsets: vec![], words: vec![], bytes: 0, traced: Map::new(),
            before: vec![], after: vec![],
        };
        for &ty in &frame.allocation.slots {
            let offset = frame.bytes as u32;
            let (words, references) = shape(ty);
            frame.offsets.push(offset);
            frame.words.push(words);
            frame.traced.extend(references.into_iter().map(|word| offset + word));
            frame.bytes += words as usize * 8;
        }
        frame.bytes = frame.bytes.max(8);
        let live = s::live_locations(source).map_err(|error| Error(error.to_string()))?;
        for (pc, locations) in live.iter().enumerate() {
            frame.before.push(frame.map(locations));
            // After-helper maps describe destinations after Post is copied, never Post itself.
            // In an outcome instruction, edge sources are still in their pre-edge locations.
            let mut after = BTreeSet::new();
            if matches!(&source.code[pc], s::Instruction::Data { .. }
                | s::Instruction::Champ { flow: s::Flow::Next(_), .. })
                && let Some(next) = live.get(pc + 1)
            {
                after.extend(next);
            }
            for edge in outgoing(&source.code[pc]) {
                let mut next = live[edge.target_pc as usize].clone();
                for assignment in &edge.assignments {
                    next.remove(&assignment.destination);
                }
                after.extend(next);
                for assignment in &edge.assignments {
                    if let s::Source::Location(location) = assignment.source {
                        after.insert(location);
                    }
                }
            }
            // Ownership-only post lifetimes are required even without an ordinary next use.
            for root in source.proofs.post_roots.iter().filter(|root| root.pc as usize == pc) {
                after.extend(&root.locations);
            }
            loop {
                let count = after.len();
                for dependency in &source.proofs.dependencies {
                    if after.contains(&dependency.value) {
                        after.extend(&dependency.obligations);
                    }
                }
                if after.len() == count {
                    break;
                }
            }
            frame.after.push(frame.map(&after));
        }
        frame.check()?;
        Ok(frame)
    }

    fn map(&self, locations: &BTreeSet<L>) -> Map {
        locations.iter().flat_map(|location| {
            let slot = self.allocation.locations[location] as usize;
            let (_, references) = shape(self.allocation.slots[slot]);
            references.into_iter().map(move |offset| self.offsets[slot] + offset)
        }).collect()
    }

    pub fn before(&self, source: &s::Function, pc: Option<usize>) -> Map {
        match pc {
            Some(pc) => self.before[pc].clone(),
            // Entry rejection and external adapters precede body PC 0. Only parameters exist.
            None => self.map(&source.params.iter().copied().collect()),
        }
    }

    /// Compare the loader's before AND after requirements with physical slot/byte-offset maps.
    /// Required words come from the checked SPB allocation; emitted words come from liveness.
    pub fn check(&self) -> Result<(), Error> {
        let mut occupied = Map::new();
        for (slot, &ty) in self.allocation.slots.iter().enumerate() {
            let offset = self.offsets[slot];
            let (words, references) = shape(ty);
            if !offset.is_multiple_of(8) || self.words[slot] != words {
                return Err(Error("Champ physical slot layout mismatch".into()));
            }
            for word in 0..words {
                let address = offset + word * 8;
                if address as usize >= self.bytes || !occupied.insert(address) {
                    return Err(Error("Champ overlapping physical slots".into()));
                }
            }
            if references.iter().any(|word| !self.traced.contains(&(offset + word))) {
                return Err(Error("Champ physical trace layout mismatch".into()));
            }
        }
        if self.before.iter().chain(&self.after).any(|map| !map.is_subset(&self.traced)) {
            return Err(Error("Champ stack map includes a raw word".into()));
        }
        for (&(pc, after), required) in &self.allocation.roots {
            let physical = if after { &self.after } else { &self.before };
            let Some(physical) = physical.get(pc as usize) else {
                return Err(Error("Champ physical root PC is missing".into()));
            };
            for root in required {
                let offset = self.offsets[root.slot as usize] + root.offset;
                if !physical.contains(&offset) {
                    return Err(Error(format!("Champ missing physical root at pc {pc}, after={after}: {root:?}")));
                }
            }
        }
        Ok(())
    }
}

fn shape(ty: s::ProducedType) -> (u32, Vec<u32>) {
    match ty {
        s::ProducedType::Service(kind) => {
            let layout = s::layout::service_layout(kind);
            (layout.words, layout.references.to_vec())
        }
        s::ProducedType::Value(s::Type::Bool | s::Type::Descriptor | s::Type::Never) => (1, vec![]),
        s::ProducedType::Value(_) => (1, vec![0]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn champ_j2_missing_physical_words_fail_before_and_after() {
        let text = include_str!("../../../../okkama-compiler/src/codegen/sparrow_oir2/fixtures/local-pair.spb17");
        let program = s::assemble(text).unwrap();
        let function = &program.functions[1];
        for after in [false, true] {
            let mut frame = Frame::new(function).unwrap();
            let (&(pc, _), roots) = frame.allocation.roots.iter()
                .find(|((_, phase), roots)| *phase == after && !roots.is_empty()).unwrap();
            let word = frame.offsets[roots[0].slot as usize] + roots[0].offset;
            let map = if after { &mut frame.after } else { &mut frame.before };
            assert!(map[pc as usize].remove(&word));
            assert!(frame.check().unwrap_err().0.contains("missing physical root"));
        }
    }

    #[test]
    fn champ_j2_key_metadata_is_raw_and_slot_clobber_is_rejected() {
        assert_eq!(shape(s::ProducedType::Service(s::ServiceKind::StableKey)), (2, vec![0]));
        assert_eq!(shape(s::ProducedType::Service(s::ServiceKind::Deps)), (0, vec![]));
        let text = include_str!("../../../../okkama-compiler/src/codegen/sparrow_oir2/fixtures/local-pair.spb17");
        let program = s::assemble(text).unwrap();
        let function = &program.functions[1];
        let mut frame = Frame::new(function).unwrap();
        let copy = frame.allocation.edges.values_mut().find(|copies| !copies.is_empty()).unwrap();
        copy[0].source = s::CopySource::Produced(999);
        assert!(s::check_allocation(function, &frame.allocation).is_err());
        let mut frame = Frame::new(function).unwrap();
        frame.offsets[1] = frame.offsets[0];
        assert!(frame.check().unwrap_err().0.contains("overlapping physical slots"));
    }
}
