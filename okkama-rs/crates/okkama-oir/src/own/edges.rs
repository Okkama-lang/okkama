//! Static lexical transitions. H1 is checked before planning any edge.
use super::*;
use std::collections::BTreeSet;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EdgePlan {
    pub close: Vec<ScopeId>,
    pub begin: Vec<ScopeId>,
}

pub(super) fn stack(module: &Module, scope: ScopeId) -> Result<Vec<ScopeId>, OwnError> {
    let mut result = vec![];
    let mut seen = BTreeSet::new();
    let mut next = Some(scope);
    while let Some(scope) = next {
        if !seen.insert(scope) {
            return Err(flow_error("cyclic lexical scope tree"));
        }
        result.push(scope);
        next = module.scopes.get(scope).map_err(arena)?.parent;
    }
    result.reverse();
    Ok(result)
}

impl EdgePlan {
    pub fn between(module: &Module, from: ScopeId, to: Option<ScopeId>) -> Result<Self, OwnError> {
        let source = stack(module, from)?;
        let target = to.map(|scope| stack(module, scope)).transpose()?.unwrap_or_default();
        let common = source.iter().zip(&target).take_while(|(a, b)| a == b).count();
        Ok(Self {
            close: source[common..].iter().rev().copied().collect(),
            begin: target[common..].to_vec(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemanticEdge {
    pub source: BlockId,
    pub target: BlockId,
    /// Distinguishes equal destinations, outcomes and ordinary failure points.
    pub ordinal: usize,
    pub failure_point: Option<InstId>,
    pub args: Vec<ValueId>,
    pub unwind: bool,
    pub plan: EdgePlan,
}

/// D11 is authoritative when present. Legacy lowering copies have preparation
/// intents instead; ordinary SSA forwarding alone never authorizes an escape.
pub(super) fn wanted(body: &Body, edge: &SemanticEdge) -> Result<BTreeSet<ValueId>, OwnError> {
    let ordinal = u16::try_from(edge.ordinal).ok();
    let records: Vec<_> = body.high_results.iter().filter(|record| record.block == edge.source
        && ordinal.is_some_and(|ordinal| record.edge == HighEdgeSelector::Normal(ordinal))).collect();
    if !records.is_empty() {
        return Ok(records.into_iter().filter(|record| record.demand == ResultDemand::Want)
            .map(|record| record.value).collect());
    }
    let mut result = BTreeSet::new();
    for inst in &body.blocks.get(edge.source).map_err(arena)?.insts {
        if let Operation::Exit(ExitOp::Prepare { exit }) = &body.insts.get(*inst).map_err(arena)?.op
            && let Some(value) = exit.value
            && edge.plan.close.contains(&body.boundaries.get(exit.boundary).map_err(arena)?.scope) {
            result.insert(value);
        }
    }
    Ok(result)
}

#[derive(Clone, Debug)]
pub(super) struct Graph {
    pub edges: Vec<SemanticEdge>,
    pub rpo: Vec<BlockId>,
    pub outgoing: Vec<Vec<usize>>,
}
impl Graph {
    pub fn index(module: &Module, body: &Body, budget: &mut Budget<'_>) -> Result<Self, OwnError> {
        let mut edges = vec![];
        for (source, block) in body.blocks.iter() {
            let mut targets = vec![];
            match &block.term.kind {
                TermKind::Br(target) => targets.push((target, false, None)),
                TermKind::Cond { yes, no, .. } => {
                    targets.push((yes, false, None));
                    targets.push((no, false, None));
                }
                TermKind::Switch { cases, default, .. } => {
                    targets.extend(cases.iter().map(|case| (&case.target, false, None)));
                    targets.push((default, false, None));
                }
                TermKind::Catch { handler, next, .. } => {
                    let args = handler.args.iter().filter_map(|arg| match arg {
                        EdgeArg::Value(value) => Some(*value),
                        EdgeArg::Produced(_) => None,
                    }).collect();
                    let destination = body.blocks.get(handler.block).map_err(arena)?.lexical_scope;
                    edges.push(SemanticEdge {
                        source, target: handler.block, ordinal: 0, failure_point: None, args,
                        unwind: false, plan: EdgePlan::between(module, block.lexical_scope, Some(destination))?,
                    });
                    targets.push((next, false, None));
                }
                TermKind::Ret { .. } | TermKind::RetDefer { .. } | TermKind::Raise(_) => {}
                TermKind::Throw(_) | TermKind::Panic(_) | TermKind::Trap { .. } => {
                    if let Some(unwind) = &block.unwind {
                        targets.push((&unwind.target, true, None));
                    }
                }
                TermKind::Unreachable => {}
                _ => return Err(flow_error("High CFG contains an unsupported terminator")),
            }
            if let Some(unwind) = &block.unwind {
                targets.push((&unwind.target, true, None));
                for inst in &block.insts {
                    let operation = &body.insts.get(*inst).map_err(arena)?.op;
                    let schema = ContractRegistry::new(module)
                        .high_site_schema(operation, body, HighOperationSite::Instruction(*inst))
                        .map_err(|error| flow_error(format!("contract: {error}")))?;
                    if schema.unwind.is_some() || matches!(operation, Operation::Call(_)
                        | Operation::Lexical(LexicalOp::Bind { .. } | LexicalOp::Rebind { .. }
                            | LexicalOp::Stage { .. })) {
                        targets.push((&unwind.target, true, Some(*inst)));
                    }
                }
            }
            let offset = usize::from(matches!(block.term.kind, TermKind::Catch { .. }));
            for (ordinal, (target, unwind, failure_point)) in targets.into_iter().enumerate() {
                budget.work(1 + target.args.len())?;
                let destination = body.blocks.get(target.block).map_err(arena)?.lexical_scope;
                let plan = EdgePlan::between(module, block.lexical_scope, Some(destination))?;
                budget.memory(128 + target.args.len() * 4 + (plan.close.len() + plan.begin.len()) * 4)?;
                edges.push(SemanticEdge {
                    source, target: target.block, ordinal: ordinal + offset, failure_point,
                    args: target.args.clone(), unwind, plan,
                });
            }
        }
        let mut adjacency = vec![vec![]; body.blocks.len()];
        let mut outgoing = vec![vec![]; body.blocks.len()];
        for (index, edge) in edges.iter().enumerate() {
            adjacency[edge.source.as_u32() as usize].push(edge.target);
            outgoing[edge.source.as_u32() as usize].push(index);
        }
        // Iterative DFS, including real back edges; no CFG unrolling.
        let mut seen = BTreeSet::new();
        let mut pending = vec![(body.entry, false)];
        let mut rpo = vec![];
        while let Some((block, leaving)) = pending.pop() {
            budget.work(1)?;
            if leaving {
                rpo.push(block);
            } else if seen.insert(block) {
                pending.push((block, true));
                pending.extend(adjacency[block.as_u32() as usize].iter().rev().map(|block| (*block, false)));
            }
        }
        rpo.reverse();
        // Newly entered suffixes identify subtree entries directly. Do not
        // rescan the entire edge set for every declared (possibly unused) scope.
        let mut entries = BTreeMap::new();
        for scope in stack(module, body.blocks.get(body.entry).map_err(arena)?.lexical_scope)? {
            entries.insert(scope, body.entry);
        }
        for edge in edges.iter().filter(|edge| seen.contains(&edge.source)) {
            for scope in &edge.plan.begin {
                budget.work(1)?;
                if entries.insert(*scope, edge.target).is_some_and(|previous| previous != edge.target) {
                    return Err(flow_error("H1: scope subtree has multiple entries"));
                }
            }
        }
        let mut source_stacks = BTreeMap::new();
        for block in &rpo {
            let scopes = stack(module, body.blocks.get(*block).map_err(arena)?.lexical_scope)?;
            budget.work(scopes.len())?;
            budget.memory(scopes.len() * 4)?;
            source_stacks.insert(*block, scopes);
        }
        for edge in edges.iter().filter(|edge| seen.contains(&edge.source)) {
            for scope in &source_stacks[&edge.source] {
                budget.work(1)?;
                if entries.get(scope) == Some(&edge.target) {
                    return Err(flow_error("H1: edge into a scope's own entry block"));
                }
            }
        }
        Ok(Self { edges, rpo, outgoing })
    }
}
