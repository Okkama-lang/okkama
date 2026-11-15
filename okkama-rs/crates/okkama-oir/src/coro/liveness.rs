//! Backward liveness of SSA values, with the implicit uses of schedule duties at close sites.
use super::{graph, CoroError};
use crate::*;
use std::collections::{BTreeMap, BTreeSet};

/// A suspension or close site: an instruction, or the terminator of a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Site {
    Instruction(InstId),
    Terminator(BlockId),
}

/// Values used by an instruction operation, including the leaves of selection and dependency operands.
pub(super) fn operation_uses(op: &Operation, body: &Body) -> Result<Vec<ValueId>, CoroError> {
    let uses = cfg::operation_uses(op, body).map_err(traversal)?;
    Ok(uses.into_iter().map(|operand| operand.value).collect())
}

/// Values used by a terminator: its operands and the values its edges pass.
pub(super) fn terminator_uses(
    module: &Module, block: BlockId, term: &Terminator, body: &Body,
) -> Result<Vec<ValueId>, CoroError> {
    let mut uses = Vec::new();
    cfg::walk_terminator(block, term, body, ContractRegistry::new(module), &mut |event| {
        if let cfg::CfgEvent::Use { operand, .. } = event {
            uses.push(operand.value);
        }
    }).map_err(traversal)?;
    // A block unwind may leave from any point of the block; its arguments live through the block.
    if let Some(unwind) = body.blocks.get(block).ok().and_then(|data| data.unwind.as_ref()) {
        uses.extend(unwind.target.args.iter().copied());
    }
    Ok(uses)
}

fn traversal(error: cfg::TraversalError) -> CoroError {
    CoroError::InvalidInput { function: None, message: error.to_string() }
}

pub(super) struct Liveness {
    pub live_in: Vec<BTreeSet<ValueId>>,
    pub live_out: Vec<BTreeSet<ValueId>>,
    /// Uses of each instruction and terminator, with the implicit ones, after `ignored` is removed.
    instruction_uses: BTreeMap<InstId, Vec<ValueId>>,
    terminator_uses: Vec<Vec<ValueId>>,
}

impl Liveness {
    /// `implicit` adds uses at sites; values for which `ignored` holds never become live.
    pub(super) fn compute(
        module: &Module, body: &Body, implicit: &BTreeMap<Site, Vec<ValueId>>, ignored: impl Fn(ValueId) -> bool,
        budget: &mut super::Budget<'_>,
    ) -> Result<Self, CoroError> {
        let count = body.blocks.len();
        let mut instruction_uses = BTreeMap::new();
        let mut terminators = vec![Vec::new(); count];
        let mut generated = vec![BTreeSet::new(); count];
        let mut kill = vec![BTreeSet::new(); count];
        for (block, data) in body.blocks.iter() {
            budget.step(data.insts.len() + 1)?;
            let index = block.as_u32() as usize;
            let mut uses = terminator_uses(module, block, &data.term, body)?;
            uses.extend(implicit.get(&Site::Terminator(block)).into_iter().flatten().copied());
            uses.retain(|value| !ignored(*value));
            let mut live: BTreeSet<ValueId> = uses.iter().copied().collect();
            terminators[index] = uses;
            for &inst in data.insts.iter().rev() {
                let instruction = body.insts.get(inst)?;
                for result in &instruction.results {
                    live.remove(result);
                    kill[index].insert(*result);
                }
                let mut uses = operation_uses(&instruction.op, body)?;
                uses.extend(implicit.get(&Site::Instruction(inst)).into_iter().flatten().copied());
                uses.retain(|value| !ignored(*value));
                live.extend(uses.iter().copied());
                instruction_uses.insert(inst, uses);
            }
            for param in &data.params {
                live.remove(param);
                kill[index].insert(*param);
            }
            generated[index] = live;
        }
        let successors: Vec<Vec<BlockId>> = body.blocks.iter().map(|(_, data)| {
            let mut targets = graph::successors(&data.term.kind);
            targets.extend(data.unwind.iter().map(|unwind| unwind.target.block));
            targets
        }).collect();
        let mut live_in = generated.clone();
        let mut live_out = vec![BTreeSet::new(); count];
        let mut changed = true;
        while changed {
            changed = false;
            for index in (0..count).rev() {
                budget.step(1)?;
                let mut out = BTreeSet::new();
                for target in &successors[index] {
                    out.extend(live_in[target.as_u32() as usize].iter().copied());
                }
                let mut input = generated[index].clone();
                input.extend(out.difference(&kill[index]).copied());
                if input != live_in[index] || out != live_out[index] {
                    live_in[index] = input;
                    live_out[index] = out;
                    changed = true;
                }
            }
        }
        Ok(Self { live_in, live_out, instruction_uses, terminator_uses: terminators })
    }

    /// Values live right after the instruction at `position` of `block`.
    pub(super) fn after(
        &self, body: &Body, block: BlockId, position: usize,
    ) -> Result<BTreeSet<ValueId>, CoroError> {
        let index = block.as_u32() as usize;
        let data = body.blocks.get(block)?;
        let mut live = self.live_out[index].clone();
        live.extend(self.terminator_uses[index].iter().copied());
        for &inst in data.insts.iter().skip(position + 1).rev() {
            for result in &body.insts.get(inst)?.results {
                live.remove(result);
            }
            live.extend(self.instruction_uses.get(&inst).into_iter().flatten().copied());
        }
        Ok(live)
    }
}
