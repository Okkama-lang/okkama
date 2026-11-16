//! Control-flow edges of a terminator, in edge order.
use crate::*;

/// Successor blocks, one entry per edge.
pub(crate) fn successors(term: &TermKind) -> Vec<BlockId> {
    let mut blocks = Vec::new();
    match term {
        TermKind::CleanupNext { selected, done, .. } => blocks.extend([selected.block, done.block]),
        TermKind::ExitDispatch { success, pending, failed, .. } => {
            blocks.extend([success, pending, failed].into_iter().flatten().map(|edge| edge.block));
        }
        TermKind::FailureDispatch { none, some, .. } => blocks.extend([none.block, some.block]),
        TermKind::Br(target) => blocks.push(target.block),
        TermKind::Cond { yes, no, .. } => blocks.extend([yes.block, no.block]),
        TermKind::Switch { cases, default, .. } => {
            blocks.extend(cases.iter().map(|case| case.target.block));
            blocks.push(default.block);
        }
        TermKind::InvokeOwn { normal, unwind, .. } => blocks.extend([normal.block, unwind.block]),
        TermKind::InvokeDefer { continue_edge, replacements, unwind, .. } => {
            blocks.push(continue_edge.block);
            blocks.extend(replacements.iter().map(|replacement| replacement.edge.block));
            blocks.extend(unwind.iter().map(|edge| edge.block));
        }
        TermKind::Catch { handler, next, .. } => blocks.extend([handler.block, next.block]),
        TermKind::Lookup { found, missing, .. } => blocks.extend([found.block, missing.block]),
        TermKind::RetDefer { .. } | TermKind::Ret { .. } | TermKind::Throw(_) | TermKind::Raise(_)
        | TermKind::Panic(_) | TermKind::Trap { .. } | TermKind::Unreachable => {}
    }
    blocks
}

/// Predecessor edges of every block: one entry per edge, so a block reached twice from one terminator
/// counts twice.
pub(crate) fn predecessors(body: &Body) -> Vec<Vec<BlockId>> {
    let mut predecessors = vec![Vec::new(); body.blocks.len()];
    for (block, data) in body.blocks.iter() {
        for target in successors(&data.term.kind) {
            if let Some(list) = predecessors.get_mut(target.as_u32() as usize) {
                list.push(block);
            }
        }
        if let Some(unwind) = &data.unwind
            && let Some(list) = predecessors.get_mut(unwind.target.block.as_u32() as usize) {
            list.push(block);
        }
    }
    predecessors
}

/// Append `value` to every edge of `term` that enters `target`.
pub(crate) fn push_argument(term: &mut TermKind, target: BlockId, value: ValueId) {
    let target_edge = |edge: &mut Target| {
        if edge.block == target {
            edge.args.push(value);
        }
    };
    let invoke_edge = |edge: &mut InvokeEdge| {
        if edge.block == target {
            edge.args.push(EdgeArg::Value(value));
        }
    };
    apply(term, target_edge, invoke_edge);
}

/// The argument at `index` of the first edge of `term` that enters `target`.
pub(crate) fn edge_argument(term: &TermKind, target: BlockId, index: usize) -> Option<EdgeArg> {
    let found = std::cell::Cell::new(None);
    let mut term = term.clone();
    apply(&mut term, |edge| {
        if edge.block == target && found.get().is_none() {
            found.set(Some(edge.args.get(index).copied().map(EdgeArg::Value)));
        }
    }, |edge| {
        if edge.block == target && found.get().is_none() {
            found.set(Some(edge.args.get(index).copied()));
        }
    });
    found.get().flatten()
}

fn apply(term: &mut TermKind, mut target: impl FnMut(&mut Target), mut invoke: impl FnMut(&mut InvokeEdge)) {
    match term {
        TermKind::CleanupNext { selected, done, .. } => {
            invoke(selected);
            invoke(done);
        }
        TermKind::ExitDispatch { success, pending, failed, .. } => {
            for edge in [success, pending, failed].into_iter().flatten() {
                invoke(edge);
            }
        }
        TermKind::FailureDispatch { none, some, .. } => {
            target(none);
            invoke(some);
        }
        TermKind::Br(edge) => target(edge),
        TermKind::Cond { yes, no, .. } => {
            target(yes);
            target(no);
        }
        TermKind::Switch { cases, default, .. } => {
            for case in cases {
                target(&mut case.target);
            }
            target(default);
        }
        TermKind::InvokeOwn { normal, unwind, .. } => {
            invoke(normal);
            invoke(unwind);
        }
        TermKind::InvokeDefer { continue_edge, replacements, unwind, .. } => {
            invoke(continue_edge);
            for replacement in replacements {
                invoke(&mut replacement.edge);
            }
            if let Some(edge) = unwind {
                invoke(edge);
            }
        }
        TermKind::Catch { handler, next, .. } => {
            invoke(handler);
            target(next);
        }
        TermKind::Lookup { found, missing, .. } => {
            invoke(found);
            target(missing);
        }
        TermKind::RetDefer { .. } | TermKind::Ret { .. } | TermKind::Throw(_) | TermKind::Raise(_)
        | TermKind::Panic(_) | TermKind::Trap { .. } | TermKind::Unreachable => {}
    }
}

/// The invoke edges of a suspending terminator, mutably and in edge order.
pub(crate) fn invoke_edges(term: &mut TermKind) -> Vec<&mut InvokeEdge> {
    match term {
        TermKind::InvokeOwn { normal, unwind, .. } => vec![normal, unwind],
        TermKind::InvokeDefer { continue_edge, replacements, unwind, .. } => {
            let mut edges = vec![continue_edge];
            edges.extend(replacements.iter_mut().map(|replacement| &mut replacement.edge));
            edges.extend(unwind.as_mut());
            edges
        }
        _ => Vec::new(),
    }
}
