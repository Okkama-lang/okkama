//! Unbound construction fields never travel along edges in SPB 17 (#677).
//!
//! The producer keeps a constructor's unbound fields as `undef` locals, so a branch before the
//! fields are bound passes them as block parameters. SPB 17 admits an undef register only as a
//! construction operand, never as an edge source. A parameter that receives undef on every
//! incoming edge is therefore defined by its own `undef` at the start of its block instead.
use crate::oir2 as o;
use std::collections::BTreeSet;

use super::GenerateError;

enum Edge<'a> {
    Branch(&'a mut o::Target),
    Invoke(&'a mut o::InvokeEdge),
}

fn edges(block: &mut o::Block) -> Vec<Edge<'_>> {
    let mut edges = vec![];
    if let Some(unwind) = &mut block.unwind {
        edges.push(Edge::Branch(&mut unwind.target));
    }
    match &mut block.term.kind {
        o::TermKind::CleanupNext { selected, done, .. } => edges.extend([Edge::Invoke(selected), Edge::Invoke(done)]),
        o::TermKind::ExitDispatch { success, pending, failed, .. } => {
            edges.extend([success, pending, failed].into_iter().flatten().map(Edge::Invoke));
        }
        o::TermKind::FailureDispatch { none, some, .. } => edges.extend([Edge::Branch(none), Edge::Invoke(some)]),
        o::TermKind::Br(target) => edges.push(Edge::Branch(target)),
        o::TermKind::Cond { yes, no, .. } => edges.extend([Edge::Branch(yes), Edge::Branch(no)]),
        o::TermKind::Switch { cases, default, .. } => {
            edges.extend(cases.iter_mut().map(|case| Edge::Branch(&mut case.target)));
            edges.push(Edge::Branch(default));
        }
        o::TermKind::InvokeOwn { normal, unwind, .. } => edges.extend([Edge::Invoke(normal), Edge::Invoke(unwind)]),
        o::TermKind::InvokeDefer { continue_edge, replacements, unwind, .. } => {
            edges.push(Edge::Invoke(continue_edge));
            edges.extend(replacements.iter_mut().map(|replacement| Edge::Invoke(&mut replacement.edge)));
            edges.extend(unwind.iter_mut().map(Edge::Invoke));
        }
        o::TermKind::Catch { handler, next, .. } => edges.extend([Edge::Invoke(handler), Edge::Branch(next)]),
        o::TermKind::Lookup { found, missing, .. } => edges.extend([Edge::Invoke(found), Edge::Branch(missing)]),
        o::TermKind::RetDefer { .. } | o::TermKind::Ret { .. } | o::TermKind::Throw(_) | o::TermKind::Raise(_)
            | o::TermKind::Panic(_) | o::TermKind::Trap { .. } | o::TermKind::Unreachable => {}
    }
    edges
}

impl Edge<'_> {
    fn block(&self) -> o::BlockId {
        match self {
            Self::Branch(target) => target.block,
            Self::Invoke(edge) => edge.block,
        }
    }

    /// The value passed at parameter `index`; `None` for a produced slot or the implicit failure.
    fn value(&self, index: usize) -> Option<o::ValueId> {
        match self {
            Self::Branch(target) => target.args.get(index).copied(),
            Self::Invoke(edge) => match edge.args.get(index) {
                Some(o::EdgeArg::Value(value)) => Some(*value),
                _ => None,
            },
        }
    }

    fn remove(&mut self, indices: &BTreeSet<usize>) {
        let keep = |index: &usize| !indices.contains(index);
        match self {
            Self::Branch(target) => target.args = std::mem::take(&mut target.args).into_iter().enumerate()
                .filter(|(index, _)| keep(index)).map(|(_, value)| value).collect(),
            Self::Invoke(edge) => edge.args = std::mem::take(&mut edge.args).into_iter().enumerate()
                .filter(|(index, _)| keep(index)).map(|(_, value)| value).collect(),
        }
    }
}

fn is_undef(body: &o::Body, value: o::ValueId) -> Result<bool, GenerateError> {
    Ok(match body.values.get(value)?.def {
        o::Definition::InstResult { inst, .. } => {
            matches!(body.insts.get(inst)?.op, o::Operation::Core(o::CoreOp::Undef { .. }))
        }
        o::Definition::BlockParam { .. } => false,
    })
}

/// The parameters (block, index) that receive undef on every incoming edge, to a fixed point.
fn undef_params(body: &mut o::Body) -> Result<BTreeSet<(o::BlockId, usize)>, GenerateError> {
    let ids: Vec<_> = body.blocks.iter().map(|(id, _)| id).collect();
    let mut undef = BTreeSet::new();
    for (value, _) in body.values.iter() {
        if is_undef(body, value)? {
            undef.insert(value);
        }
    }
    let mut incoming = std::collections::BTreeMap::<(o::BlockId, usize), Vec<Option<o::ValueId>>>::new();
    for &id in &ids {
        let mut block = body.blocks.get(id)?.clone();
        for edge in edges(&mut block) {
            let target = edge.block();
            for index in 0..body.blocks.get(target)?.params.len() {
                incoming.entry((target, index)).or_default().push(edge.value(index));
            }
        }
    }
    loop {
        let mut changed = false;
        for (&(block, index), sources) in &incoming {
            let param = body.blocks.get(block)?.params[index];
            if !undef.contains(&param) && sources.iter().all(|source| source.is_some_and(|v| undef.contains(&v))) {
                undef.insert(param);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut params = BTreeSet::new();
    for &(block, index) in incoming.keys() {
        if undef.contains(&body.blocks.get(block)?.params[index]) {
            params.insert((block, index));
        }
    }
    Ok(params)
}

/// Rewrites every parameter that only ever receives undef into a block-local `undef`.
pub(super) fn sink_undef_params(body: &mut o::Body) -> Result<(), GenerateError> {
    let params = undef_params(body)?;
    if params.is_empty() {
        return Ok(());
    }
    let ids: Vec<_> = body.blocks.iter().map(|(id, _)| id).collect();
    for &id in &ids {
        let removed: BTreeSet<usize> = params.iter().filter(|(block, _)| *block == id).map(|(_, index)| *index)
            .collect();
        if removed.is_empty() {
            continue;
        }
        let old = body.blocks.get(id)?.params.clone();
        let mut locals = vec![];
        for &index in removed.iter().rev() {
            let value = old[index];
            let ty = body.values.get(value)?.ty;
            let inst = body.insts.alloc(o::Instruction {
                results: vec![value],
                op: o::Operation::Core(o::CoreOp::Undef { ty }),
                facts: o::InstructionFacts::default(),
                loc: None,
            })?;
            body.values.get_mut(value)?.def = o::Definition::InstResult { inst, index: 0 };
            locals.push(inst);
        }
        let kept: Vec<_> = old.iter().enumerate().filter(|(index, _)| !removed.contains(index))
            .map(|(_, &value)| value).collect();
        for (index, &value) in kept.iter().enumerate() {
            let index = u32::try_from(index).map_err(|_| GenerateError("too many block parameters".into()))?;
            body.values.get_mut(value)?.def = o::Definition::BlockParam { block: id, index };
        }
        // The new instructions see the roots of their block's entry, which they precede.
        let entry_roots: Vec<_> = body.cleanup.roots.iter()
            .filter(|(_, root)| root.boundary == o::RootBoundary::Entry(id))
            .map(|(_, root)| root.clone()).collect();
        for root in entry_roots {
            for &inst in &locals {
                body.cleanup.roots.alloc(o::RootRequirement {
                    boundary: o::RootBoundary::BeforeInstruction(inst), ..root.clone()
                })?;
            }
        }
        let block = body.blocks.get_mut(id)?;
        block.params = kept;
        locals.reverse();
        locals.append(&mut block.insts);
        block.insts = locals;
        for source in &ids {
            let mut edges_of = edges(body.blocks.get_mut(*source)?);
            for edge in edges_of.iter_mut().filter(|edge| edge.block() == id) {
                edge.remove(&removed);
            }
        }
    }
    Ok(())
}
