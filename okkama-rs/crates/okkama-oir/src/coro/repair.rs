//! SSA repair after suspension: a value saved at a site is redefined by every resume of that site, the
//! frame token by every save and resume. Uses read the reaching version; merges of different versions
//! become block parameters where the value is still live (pruned SSA).
use super::graph;
use super::liveness::{self, Site};
use super::{Budget, CoroError};
use crate::visit::{Rewrite, VisitorMut};
use crate::cfg;
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

pub(super) struct Replace {
    from: ValueId,
    to: ValueId,
    count: usize,
}
impl Replace {
    /// Every use of `from` names `to` instead.
    pub(super) fn single(from: ValueId, to: ValueId) -> Self {
        Self { from, to, count: 0 }
    }
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

/// Every value a visitor sees (#889): the direct operands of an operation or a terminator.
struct Collect(Vec<ValueId>);
impl VisitorMut for Collect {
    fn value(&mut self, value: &mut ValueId) {
        self.0.push(*value);
    }
}

/// The uses of every value in a body (#889), built once before a series of repairs. A repair adds block parameters
/// and edge arguments (uses of its own versions) and rewrites only its own variable's uses; it inserts no
/// instruction, so positions and the uses of the other variables stay valid. `live` follows the liveness of
/// `operation_uses` and `terminator_uses` (an instruction at its index, the terminator at `insts.len()`, a block
/// unwind's argument at -1); `touched` is every block whose operations or terminator name a value at all.
pub(super) struct Uses {
    positions: BTreeMap<InstId, (BlockId, usize)>,
    live: BTreeMap<ValueId, Vec<(BlockId, isize)>>,
    touched: BTreeMap<ValueId, std::collections::BTreeSet<BlockId>>,
}
impl Uses {
    pub(super) fn of(module: &Module, body: &Body, budget: &mut Budget<'_>) -> Result<Self, CoroError> {
        let mut uses = Uses { positions: BTreeMap::new(), live: BTreeMap::new(), touched: BTreeMap::new() };
        for (block, data) in body.blocks.iter() {
            budget.step(data.insts.len() + 1)?;
            for (at, &inst) in data.insts.iter().enumerate() {
                uses.positions.insert(inst, (block, at));
                let op = &body.insts.get(inst)?.op;
                let at = isize::try_from(at).unwrap_or(isize::MAX);
                for value in liveness::operation_uses(op, body)? {
                    uses.live.entry(value).or_default().push((block, at));
                    uses.touched.entry(value).or_default().insert(block);
                }
                let mut direct = Collect(Vec::new());
                op.clone().rewrite(&mut direct);
                for value in direct.0 {
                    uses.touched.entry(value).or_default().insert(block);
                }
            }
            let end = isize::try_from(data.insts.len()).unwrap_or(isize::MAX);
            for value in liveness::terminator_uses(module, block, &data.term, body)? {
                uses.live.entry(value).or_default().push((block, end));
                uses.touched.entry(value).or_default().insert(block);
            }
            let mut direct = Collect(Vec::new());
            data.term.kind.clone().rewrite(&mut direct);
            for value in direct.0 {
                uses.touched.entry(value).or_default().insert(block);
            }
            if let Some(unwind) = &data.unwind {
                for &value in &unwind.target.args {
                    uses.live.entry(value).or_default().push((block, -1));
                    uses.touched.entry(value).or_default().insert(block);
                }
            }
        }
        Ok(uses)
    }
}

/// Copies of the selection and dependency expressions that name `from`, naming `to` instead (#734). An
/// expression nests (the admissible part of a union of single values, the retention of a value's selection),
/// so a copy is made wherever `from` occurs below it; key expressions are identities and are never copied.
struct Renamed {
    from: ValueId,
    to: ValueId,
    selections: BTreeMap<SelectionId, SelectionId>,
    deps: BTreeMap<DepExprId, DepExprId>,
}
impl VisitorMut for Renamed {
    fn value(&mut self, value: &mut ValueId) {
        if *value == self.from {
            *value = self.to;
        }
    }
    fn selection(&mut self, selection: &mut SelectionId) {
        if let Some(copy) = self.selections.get(selection) {
            *selection = *copy;
        }
    }
    fn dependency(&mut self, dependency: &mut DepExprId) {
        if let Some(copy) = self.deps.get(dependency) {
            *dependency = *copy;
        }
    }
}

fn renamed(
    body: &mut Body, refs: Vec<ReferencedOperand>, from: ValueId, to: ValueId,
) -> Result<Renamed, CoroError> {
    let traversal = |error: cfg::TraversalError| CoroError::InvalidInput { function: None, message: error.to_string() };
    let mut renamed = Renamed { from, to, selections: BTreeMap::new(), deps: BTreeMap::new() };
    let mut seen = std::collections::BTreeSet::new();
    for operand in refs {
        // Post order: an expression is copied after its children, so the copy names their copies.
        let mut pending = vec![(operand.reference, false)];
        while let Some((reference, leaving)) = pending.pop() {
            if !leaving {
                if matches!(reference, OperandRef::Selection(_) | OperandRef::Dependency(_)) && seen.insert(reference) {
                    pending.push((reference, true));
                    let children = cfg::expression_children(reference, body).map_err(traversal)?;
                    pending.extend(children.into_iter().map(|child| (child, false)));
                }
                continue;
            }
            match reference {
                OperandRef::Selection(id) => {
                    let original = body.selections.get(id)?.clone();
                    let mut expr = original.clone();
                    expr.rewrite(&mut renamed);
                    if expr != original {
                        let copy = body.selections.alloc(expr)?;
                        renamed.selections.insert(id, copy);
                    }
                }
                OperandRef::Dependency(id) => {
                    let original = body.deps.get(id)?.clone();
                    let mut expr = original.clone();
                    expr.rewrite(&mut renamed);
                    if expr != original {
                        let copy = body.deps.alloc(expr)?;
                        renamed.deps.insert(id, copy);
                    }
                }
                _ => {}
            }
        }
    }
    Ok(renamed)
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
    module: &Module, function: FuncId, body: &mut Body, variable: &Variable, index: &Uses, budget: &mut Budget<'_>,
) -> Result<BTreeMap<Site, ValueId>, CoroError> {
    let unsupported = |message: &str| CoroError::Unsupported { function, message: message.into() };
    let count = body.blocks.len();
    let positions = &index.positions;
    // Definitions per block, ordered by position.
    let mut defs: Vec<Vec<(isize, ValueId)>> = vec![Vec::new(); count];
    for &value in std::iter::once(&variable.original).chain(&variable.versions) {
        let (block, at) = position(body, positions, value).ok_or_else(|| CoroError::InvalidInput {
            function: Some(function), message: "a repaired value has no definition".into(),
        })?;
        defs[block.as_u32() as usize].push((at, value));
    }
    for list in &mut defs {
        list.sort();
    }
    // Uses of the original name per block, from the index and the implicit sites; the terminator is at position
    // `insts.len()`, a block unwind reads its arguments from the start of the block (-1). Only the minimum is read.
    let mut uses: Vec<Vec<isize>> = vec![Vec::new(); count];
    // The blocks whose operations, terminator or unwind name the original, or hold an implicit site: the only ones
    // the rewrite below changes or reads a version in.
    let mut rewritten: std::collections::BTreeSet<BlockId> =
        index.touched.get(&variable.original).cloned().unwrap_or_default();
    for &(block, at) in index.live.get(&variable.original).into_iter().flatten() {
        budget.step(1)?;
        uses[block.as_u32() as usize].push(at);
    }
    for site in &variable.implicit {
        budget.step(1)?;
        let (block, at) = match *site {
            Site::Instruction(inst) => match positions.get(&inst) {
                Some(&(block, at)) => (block, isize::try_from(at).unwrap_or(isize::MAX)),
                None => continue,
            },
            Site::Terminator(block) => (block, isize::try_from(body.blocks.get(block)?.insts.len())
                .unwrap_or(isize::MAX)),
        };
        uses[block.as_u32() as usize].push(at);
        rewritten.insert(block);
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
    // #889: a worklist from the blocks live at entry back through the blocks that do not define the variable,
    // along terminator edges only (a block unwind's arguments are uses at its start, -1).
    let mut sources: Vec<Vec<usize>> = vec![Vec::new(); count];
    for (index, targets) in successors.iter().enumerate() {
        for target in targets {
            if let Some(list) = sources.get_mut(target.as_u32() as usize) {
                list.push(index);
            }
        }
    }
    let mut live_in = used_first.clone();
    let mut pending: Vec<usize> = (0..count).filter(|&index| live_in[index]).collect();
    while let Some(index) = pending.pop() {
        for &source in &sources[index] {
            budget.step(1)?;
            if !live_in[source] && defs[source].is_empty() {
                live_in[source] = true;
                pending.push(source);
            }
        }
    }
    // Reaching versions at block entries.
    let mut entry = vec![Reach::Unvisited; count];
    let exit = |entry: &Vec<Reach>, index: usize| match defs[index].last() {
        Some((_, value)) => Reach::Value(*value),
        None => entry[index],
    };
    // #806: `join` is not monotone (an entry can move from `Merge(b)` to another block's merge and back), so the
    // fixed point could oscillate around several loops that each suspend. A block that has once merged keeps its
    // merge, which bounds every entry's changes; the merges that end up trivial (every input the same version)
    // are removed afterwards, so the result again satisfies `join` at every block.
    let inputs = |entry: &Vec<Reach>, block: BlockId| {
        let index = block.as_u32() as usize;
        let mut inputs: Vec<Reach> = predecessors[index].iter()
            .map(|source| exit(entry, source.as_u32() as usize)).collect();
        if block == body.entry {
            inputs.push(Reach::Undefined);
        }
        inputs
    };
    // #889: a worklist in block order; a block whose entry changes queues its successors (its exit is its entry
    // unless it defines the variable, and a block that defines it has a constant exit).
    let order: Vec<BlockId> = body.blocks.iter().map(|(block, _)| block).collect();
    let mut queued = vec![true; count];
    let mut pending: std::collections::VecDeque<BlockId> = order.iter().copied().collect();
    while let Some(block) = pending.pop_front() {
        budget.step(1)?;
        let index = block.as_u32() as usize;
        queued[index] = false;
        if entry[index] == Reach::Merge(block) {
            continue;
        }
        let reach = join(block, &inputs(&entry, block));
        if reach != entry[index] {
            entry[index] = reach;
            if defs[index].is_empty() {
                // The targets whose entries read this exit: the terminator's and the block unwind's.
                let unwind = body.blocks.get(block)?.unwind.as_ref().map(|unwind| unwind.target.block);
                for target in successors[index].iter().chain(&unwind) {
                    let target_index = target.as_u32() as usize;
                    if !queued[target_index] {
                        queued[target_index] = true;
                        pending.push_back(*target);
                    }
                }
            }
        }
    }
    loop {
        // The trivial merges of this round, each replaced by its single input; one rewrite of every entry
        // follows the chain (a merge whose input is another removed merge).
        let mut removed: BTreeMap<BlockId, Reach> = BTreeMap::new();
        for (block, _) in body.blocks.iter() {
            budget.step(1)?;
            let index = block.as_u32() as usize;
            if entry[index] != Reach::Merge(block) {
                continue;
            }
            let reach = join(block, &inputs(&entry, block));
            if reach != Reach::Merge(block) {
                removed.insert(block, reach);
            }
        }
        if removed.is_empty() {
            break;
        }
        for other in entry.iter_mut() {
            budget.step(1)?;
            let mut seen = 0;
            while let Reach::Merge(at) = *other
                && let Some(&reach) = removed.get(&at) {
                *other = reach;
                seen += 1;
                if seen > removed.len() {
                    return Err(CoroError::InvalidInput {
                        function: Some(function), message: "trivial merges replace each other in a cycle".into(),
                    });
                }
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
    // Rewrite the uses with the reaching version, in the blocks that name the original or hold an implicit site.
    let mut versions = BTreeMap::new();
    for block in blocks.into_iter().filter(|block| rewritten.contains(block)) {
        budget.step(1)?;
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
                    let mut renamed = renamed(body, op.operand_refs(), variable.original, version)?;
                    op.rewrite(&mut Replace { from: variable.original, to: version, count: 0 });
                    op.rewrite(&mut renamed);
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
                let mut renamed = renamed(body, refs, variable.original, version)?;
                term.kind.rewrite(&mut Replace { from: variable.original, to: version, count: 0 });
                term.kind.rewrite(&mut renamed);
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
