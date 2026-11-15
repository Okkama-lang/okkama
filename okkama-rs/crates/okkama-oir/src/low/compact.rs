//! Drop the instructions that the narrowing took out of their blocks, and their results.
//!
//! Arenas have no removal and the text codec represents only placed instructions and their values, so the
//! body is rebuilt with the placed instructions and the defined values, in their old order.
use super::*;
use crate::visit::{Rewrite, VisitorMut};
use std::collections::{BTreeMap, BTreeSet};

struct Remap {
    values: BTreeMap<ValueId, ValueId>,
    insts: BTreeMap<InstId, InstId>,
    dangling: bool,
}
impl VisitorMut for Remap {
    fn value(&mut self, value: &mut ValueId) {
        match self.values.get(value) {
            Some(new) => *value = *new,
            None => self.dangling = true,
        }
    }
    fn instruction(&mut self, inst: &mut InstId) {
        match self.insts.get(inst) {
            Some(new) => *inst = *new,
            None => self.dangling = true,
        }
    }
}

/// Keep the placed instructions and the values that a block parameter or a placed instruction defines.
pub(super) fn compact(body: &mut Body) -> Result<(), LowError> {
    let placed: BTreeSet<InstId> = body.blocks.iter().flat_map(|(_, block)| block.insts.iter().copied()).collect();
    if placed.len() == body.insts.len() {
        return Ok(());
    }
    let mut defined = BTreeSet::new();
    for (_, block) in body.blocks.iter() {
        defined.extend(block.params.iter().copied());
    }
    for inst in &placed {
        defined.extend(body.insts.get(*inst)?.results.iter().copied());
    }
    let mut remap = Remap { values: BTreeMap::new(), insts: BTreeMap::new(), dangling: false };
    let mut insts = Arena::new();
    for (id, inst) in body.insts.iter() {
        if placed.contains(&id) {
            remap.insts.insert(id, insts.alloc(inst.clone())?);
        }
    }
    let mut values = Arena::new();
    for (id, value) in body.values.iter() {
        if defined.contains(&id) {
            remap.values.insert(id, values.alloc(value.clone())?);
        }
    }
    body.insts = insts;
    body.values = values;
    body.rewrite(&mut remap);
    if remap.dangling {
        return Err(LowError::InvalidOutput("a removed instruction or value is still referenced".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unplaced instructions and their results go; the text round trip needs every value placed.
    #[test]
    fn unplaced_instructions_and_their_values_go() {
        let source = include_str!("../../tests/fixtures/async/low-input/low-suspending-close.oir");
        let mut module = text::parse(source).unwrap();
        let (function, _) = module.functions.iter().find(|(_, function)| function.symbol == "main").unwrap();
        let body = module.functions.get_mut(function).unwrap().body.as_mut().unwrap();
        let before = (body.insts.len(), body.values.len());
        let options = LowOptions::default();
        let mut budget = Budget { options: &options, steps: 0, blocks: 0, roots: 0 };
        narrow::closes(body, &mut budget).unwrap();
        compact(body).unwrap();
        // Two saves and three resumes (the form (a) close resumes in both successors) with four frames.
        assert_eq!((body.insts.len(), body.values.len()), (before.0 - 5, before.1 - 5));
        text::print(&module).unwrap();
    }

    /// A use of a removed value is an invalid output, not a silent renaming.
    #[test]
    fn a_use_of_a_removed_value_is_rejected() {
        let source = include_str!("../../tests/fixtures/async/low-input/low-suspending-close.oir");
        let mut module = text::parse(source).unwrap();
        let (function, _) = module.functions.iter().find(|(_, function)| function.symbol == "main").unwrap();
        let body = module.functions.get_mut(function).unwrap().body.as_mut().unwrap();
        let block = body.entry;
        let first = body.blocks.get(block).unwrap().insts[1];
        let used = body.insts.get(first).unwrap().results[0];
        body.blocks.get_mut(block).unwrap().insts.remove(1);
        assert!(body.insts.iter().any(|(_, inst)| inst.op.operand_refs().iter()
            .any(|operand| operand.reference == OperandRef::Value(used))));
        let error = compact(body).unwrap_err();
        assert!(matches!(error, LowError::InvalidOutput(_)), "{error:?}");
    }
}
