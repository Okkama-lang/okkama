//! SSA repair after suspension: a value saved at a site is redefined by every resume of that site, the
//! frame token by every save and resume. Uses read the reaching version; merges of different versions
//! become block parameters where the value is still live (pruned SSA).
use super::graph;
use super::liveness::{self, Site};
use super::{Budget, CoroError};
use crate::visit::{Rewrite, VisitorMut};
use crate::*;
use std::collections::BTreeMap;

/// A value whose versions the pass introduced.
pub(super) struct Variable {
    pub original: ValueId,
    pub versions: Vec<ValueId>,
    /// Close sites whose reaching version the caller needs (schedule duties).
    pub implicit: Vec<Site>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reach {
    Unvisited,
    Undefined,
    Value(ValueId),
    Merge(BlockId),
    /// Defined on some paths only.
    Partial,
}

struct Replace {
    from: ValueId,
    to: ValueId,
    count: usize,
}
impl VisitorMut for Replace {
    fn value(&mut self, value: &mut ValueId) {
        if *value == self.from {
            *value = self.to;
            self.count += 1;
        }
    }
}

struct Count {
    value: ValueId,
    count: usize,
}
impl VisitorMut for Count {
    fn value(&mut self, value: &mut ValueId) {
        if *value == self.value {
            self.count += 1;
        }
    }
}

/// Single-value selections of `refs` that name `from`, copied to name `to` instead.
struct Selections(BTreeMap<SelectionId, SelectionId>);
impl VisitorMut for Selections {
    fn selection(&mut self, selection: &mut SelectionId) {
        if let Some(copy) = self.0.get(selection) {
            *selection = *copy;
        }
    }
}

fn value_selections(
    body: &mut Body, refs: Vec<ReferencedOperand>, from: ValueId, to: ValueId,
) -> Result<Selections, CoroError> {
    let mut copies = BTreeMap::new();
    for operand in refs {
        let OperandRef::Selection(selection) = operand.reference else {
            continue;
        };
        if copies.contains_key(&selection) || body.selections.get(selection)? != &SelectionExpr::Value(from) {
            continue;
        }
        let copy = body.selections.alloc(SelectionExpr::Value(to))?;
        copies.insert(selection, copy);
    }
    Ok(Selections(copies))
}

fn direct_uses(op: &mut impl Rewrite, value: ValueId) -> usize {
    let mut count = Count { value, count: 0 };
    op.rewrite(&mut count);
    count.count
}

/// Position of a definition in its block; block parameters come before every instruction.
fn position(body: &Body, positions: &BTreeMap<InstId, (BlockId, usize)>, value: ValueId) -> Option<(BlockId, isize)> {
    match body.values.get(value).ok()?.def {
        Definition::BlockParam { block, .. } => Some((block, -1)),
        Definition::InstResult { inst, .. } => {
            positions.get(&inst).map(|(block, index)| (*block, isize::try_from(*index).unwrap_or(isize::MAX)))
        }
    }
}

/// Repair one variable. Returns the reaching version at each implicit site where it is defined.
pub(super) fn repair(
    module: &Module, function: FuncId, body: &mut Body, variable: &Variable, budget: &mut Budget<'_>,
) -> Result<BTreeMap<Site, ValueId>, CoroError> {
    let unsupported = |message: &str| CoroError::Unsupported { function, message: message.into() };
    let count = body.blocks.len();
    let mut positions = BTreeMap::new();
    for (block, data) in body.blocks.iter() {
        for (index, inst) in data.insts.iter().enumerate() {
            positions.insert(*inst, (block, index));
        }
    }
    // Definitions per block, ordered by position.
    let mut defs: Vec<Vec<(isize, ValueId)>> = vec![Vec::new(); count];
    for &value in std::iter::once(&variable.original).chain(&variable.versions) {
        let (block, at) = position(body, &positions, value).ok_or_else(|| CoroError::InvalidInput {
            function: Some(function), message: "a repaired value has no definition".into(),
        })?;
        defs[block.as_u32() as usize].push((at, value));
    }
    for list in &mut defs {
        list.sort();
    }
    // Uses of the original name per block; the terminator is at position `insts.len()`.
    let mut uses: Vec<Vec<isize>> = vec![Vec::new(); count];
    for (block, data) in body.blocks.iter() {
        budget.step(data.insts.len() + 1)?;
        let index = block.as_u32() as usize;
        for (at, &inst) in data.insts.iter().enumerate() {
            let instruction = body.insts.get(inst)?;
            let used = liveness::operation_uses(&instruction.op, body)?.contains(&variable.original);
            if used || variable.implicit.contains(&Site::Instruction(inst)) {
                uses[index].push(isize::try_from(at).unwrap_or(isize::MAX));
            }
        }
        let used = liveness::terminator_uses(module, block, &data.term, body)?.contains(&variable.original);
        if used || variable.implicit.contains(&Site::Terminator(block)) {
            uses[index].push(isize::try_from(data.insts.len()).unwrap_or(isize::MAX));
        }
        // A block unwind reads its arguments from the start of the block.
        if data.unwind.as_ref().is_some_and(|unwind| unwind.target.args.contains(&variable.original)) {
            uses[index].insert(0, -1);
        }
    }
    let unwind_targets: Vec<BlockId> = body.blocks.iter()
        .filter_map(|(_, data)| data.unwind.as_ref().map(|unwind| unwind.target.block)).collect();
    let predecessors = graph::predecessors(body);
    let successors: Vec<Vec<BlockId>> = body.blocks.iter().map(|(_, data)| graph::successors(&data.term.kind))
        .collect();
    // Liveness of the variable at block entries.
    let used_first: Vec<bool> = (0..count).map(|index| {
        let first_def = defs[index].first().map(|(at, _)| *at);
        uses[index].iter().min().is_some_and(|use_at| first_def.is_none_or(|def_at| *use_at <= def_at))
    }).collect();
    let mut live_in = used_first.clone();
    let mut changed = true;
    while changed {
        changed = false;
        for index in (0..count).rev() {
            budget.step(1)?;
            if live_in[index] || !defs[index].is_empty() {
                continue;
            }
            if successors[index].iter().any(|target| live_in[target.as_u32() as usize]) {
                live_in[index] = true;
                changed = true;
            }
        }
    }
    // Reaching versions at block entries.
    let mut entry = vec![Reach::Unvisited; count];
    let exit = |entry: &Vec<Reach>, index: usize| match defs[index].last() {
        Some((_, value)) => Reach::Value(*value),
        None => entry[index],
    };
    changed = true;
    while changed {
        changed = false;
        for (block, _) in body.blocks.iter() {
            budget.step(1)?;
            let index = block.as_u32() as usize;
            let mut inputs: Vec<Reach> = predecessors[index].iter()
                .map(|source| exit(&entry, source.as_u32() as usize)).collect();
            if block == body.entry {
                inputs.push(Reach::Undefined);
            }
            let reach = join(block, &inputs);
            if reach != entry[index] {
                entry[index] = reach;
                changed = true;
            }
        }
    }
    // Block parameters where different versions meet and the variable is live.
    let template = body.values.get(variable.original)?.clone();
    let mut merges: BTreeMap<BlockId, ValueId> = BTreeMap::new();
    let blocks: Vec<BlockId> = body.blocks.iter().map(|(block, _)| block).collect();
    for &block in &blocks {
        let index = block.as_u32() as usize;
        match entry[index] {
            Reach::Merge(at) if at == block && live_in[index] => {
                // The failure is the last parameter of an unwind target.
                if unwind_targets.contains(&block) {
                    return Err(unsupported("versions of a suspended value merge at a block unwind target"));
                }
                let param_index = u32::try_from(body.blocks.get(block)?.params.len())
                    .map_err(|_| CoroError::LimitExceeded("block parameters"))?;
                let param = body.values.alloc(ValueData {
                    ty: template.ty, def: Definition::BlockParam { block, index: param_index },
                    facts: template.facts.clone(),
                })?;
                body.blocks.get_mut(block)?.params.push(param);
                merges.insert(block, param);
            }
            Reach::Partial if live_in[index] => {
                return Err(unsupported("a value that a suspension redefines is defined on some paths only"));
            }
            _ => {}
        }
    }
    let resolve = |reach: Reach| match reach {
        Reach::Value(value) => Some(value),
        Reach::Merge(block) => merges.get(&block).copied(),
        _ => None,
    };
    for &block in merges.keys() {
        let mut sources = predecessors[block.as_u32() as usize].clone();
        sources.sort();
        sources.dedup();
        for source in sources {
            let incoming = resolve(exit(&entry, source.as_u32() as usize))
                .ok_or_else(|| unsupported("a merged version has no incoming definition"))?;
            graph::push_argument(&mut body.blocks.get_mut(source)?.term.kind, block, incoming);
        }
    }
    // Rewrite the uses with the reaching version.
    let mut versions = BTreeMap::new();
    for block in blocks {
        let index = block.as_u32() as usize;
        let mut current = match defs[index].iter().rev().find(|(at, _)| *at < 0) {
            Some((_, value)) => Some(*value),
            None => merges.get(&block).copied().or_else(|| resolve(entry[index])),
        };
        if current != Some(variable.original)
            && let Some(unwind) = &mut body.blocks.get_mut(block)?.unwind
            && unwind.target.args.contains(&variable.original) {
            let version = current.ok_or_else(|| unsupported("a block unwind names an undefined version"))?;
            let mut replace = Replace { from: variable.original, to: version, count: 0 };
            unwind.target.args.rewrite(&mut replace);
        }
        let insts = body.blocks.get(block)?.insts.clone();
        for inst in insts {
            let site = Site::Instruction(inst);
            let instruction = body.insts.get(inst)?;
            let expanded = liveness::operation_uses(&instruction.op, body)?.iter()
                .filter(|value| **value == variable.original).count();
            let mut op = instruction.op.clone();
            let direct = direct_uses(&mut op, variable.original);
            if expanded > 0 || direct > 0 {
                let Some(version) = current else {
                    return Err(unsupported("a use of a redefined value has no reaching version"));
                };
                if version != variable.original {
                    let mut op = body.insts.get(inst)?.op.clone();
                    let mut selections = value_selections(body, op.operand_refs(), variable.original, version)?;
                    op.rewrite(&mut Replace { from: variable.original, to: version, count: 0 });
                    op.rewrite(&mut selections);
                    if liveness::operation_uses(&op, body)?.contains(&variable.original) {
                        return Err(unsupported("a selection or dependency names a value saved across a suspension"));
                    }
                    body.insts.get_mut(inst)?.op = op;
                }
            }
            if variable.implicit.contains(&site) && let Some(version) = current {
                versions.insert(site, version);
            }
            let results = &body.insts.get(inst)?.results;
            if let Some(defined) = results.iter().find(|value| {
                **value == variable.original || variable.versions.contains(value)
            }) {
                current = Some(*defined);
            }
        }
        let site = Site::Terminator(block);
        let data = body.blocks.get(block)?;
        let unwinding = data.unwind.as_ref().map_or(0, |unwind| {
            unwind.target.args.iter().filter(|value| **value == variable.original).count()
        });
        let expanded = liveness::terminator_uses(module, block, &data.term, body)?.iter()
            .filter(|value| **value == variable.original).count().saturating_sub(unwinding);
        let mut term = data.term.kind.clone();
        let direct = direct_uses(&mut term, variable.original);
        if expanded > 0 || direct > 0 {
            let Some(version) = current else {
                return Err(unsupported("a use of a redefined value has no reaching version"));
            };
            if version != variable.original {
                let mut term = body.blocks.get(block)?.term.clone();
                let refs = match &term.kind {
                    TermKind::InvokeOwn { op, .. } => op.operand_refs(),
                    _ => Vec::new(),
                };
                let mut selections = value_selections(body, refs, variable.original, version)?;
                term.kind.rewrite(&mut Replace { from: variable.original, to: version, count: 0 });
                term.kind.rewrite(&mut selections);
                let remaining = liveness::terminator_uses(module, block, &term, body)?.iter()
                    .filter(|value| **value == variable.original).count();
                if remaining > unwinding {
                    return Err(unsupported("a selection or dependency names a value saved across a suspension"));
                }
                body.blocks.get_mut(block)?.term = term;
            }
        }
        if variable.implicit.contains(&site) && let Some(version) = current {
            versions.insert(site, version);
        }
    }
    Ok(versions)
}

fn join(block: BlockId, inputs: &[Reach]) -> Reach {
    let mut defined: Option<Reach> = None;
    let mut undefined = false;
    let mut merge = false;
    for input in inputs {
        match input {
            Reach::Unvisited => {}
            Reach::Partial => return Reach::Partial,
            Reach::Undefined => undefined = true,
            reach => match defined {
                None => defined = Some(*reach),
                Some(previous) if previous != *reach => merge = true,
                Some(_) => {}
            },
        }
    }
    match (defined, undefined) {
        (Some(_), true) => Reach::Partial,
        (Some(_), false) if merge => Reach::Merge(block),
        (Some(reach), false) => reach,
        (None, true) => Reach::Undefined,
        (None, false) => Reach::Unvisited,
    }
}
