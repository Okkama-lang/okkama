//! Forward worklist over the original cyclic CFG. Emit/diagnose only after fixpoint.
use super::*;
use super::domain::*;
use super::edges::{Graph, SemanticEdge};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionAnalysis {
    pub equations: Equations,
    pub value_shapes: BTreeMap<ValueId, ShapeId>,
    pub entries: BTreeMap<BlockId, FlowState>,
    pub outcomes: Vec<(SemanticEdge, FlowState)>,
    pub evaluations: usize,
    /// Upper bound on strict product ascents per program point. Set membership
    /// contributes one; quantity contributes at most four; masks one per bit.
    pub height_bound: usize,
    pub stats: OwnStats,
}

pub fn analyze_function(
    module: &Module,
    function: FuncId,
    classification: &OwnAnalysis,
    options: &OwnOptions,
) -> Result<FunctionAnalysis, OwnError> {
    let mut budget = Budget::new(options);
    budget.stats.active_function = Some(function);
    let body = module.functions.get(function).map_err(arena)?.body.as_ref()
        .ok_or_else(|| flow_error("analysis requires a function body"))?;
    let graph = Graph::index(module, body, &mut budget)?;
    let mut equations = Equations::default();
    equations.intern(Shape::Empty).unwrap();
    let mut value_shapes = BTreeMap::new();
    for (value, data) in body.values.iter() {
        budget.work(1)?;
        let shape = match data.def {
            Definition::BlockParam { block, index } if block == body.entry => Shape::Input { parameter: index },
            Definition::BlockParam { block, index } => Shape::Phi { block, parameter: index },
            Definition::InstResult { inst, .. } => Shape::Fresh { origin: Site::Instruction(inst), current: value },
        };
        value_shapes.insert(value, equations.intern(shape).unwrap());
    }
    for (_, block) in body.blocks.iter() {
        for inst in &block.insts {
            let site = Site::Instruction(*inst);
            equations.intern(Shape::Rest { site }).unwrap();
            equations.intern(Shape::Selected { site }).unwrap();
        }
    }
    for (scope, decl) in module.scopes.iter() {
        if decl.function == function {
            for (value, _) in body.values.iter() {
                budget.work(1)?;
                equations.intern(Shape::Journal { owner: scope, slot: value }).unwrap();
            }
        }
    }
    for edge in &graph.edges {
        for (argument, parameter) in edge.args.iter().zip(&body.blocks.get(edge.target).map_err(arena)?.params) {
            equations.connect(value_shapes[parameter], value_shapes[argument]);
        }
    }
    equations.freeze();
    let origins = equations.nodes.len();
    let values = body.values.len();
    let scopes = module.scopes.len();
    let height_bound = values.saturating_mul(origins.saturating_mul(2).saturating_add(64))
        .saturating_add(scopes.saturating_mul(origins).saturating_mul(8))
        .saturating_add(body.bindings.len().saturating_mul(values.saturating_add(origins * 2)));
    let height_bound = height_bound.saturating_add(scopes.saturating_mul(origins).saturating_mul(4));
    let facts = values.saturating_mul(origins.saturating_mul(2).saturating_add(8))
        .saturating_add(scopes.saturating_mul(origins).saturating_mul(4))
        .saturating_add(body.bindings.len().saturating_mul(values.saturating_add(origins * 2)));
    budget.memory(origins.saturating_mul(128))?;
    budget.memory((body.blocks.len() + graph.edges.len()).saturating_mul(facts).saturating_mul(64))?;
    let mut initial = FlowState { values: vec![ValueState::default(); values], ..FlowState::default() };
    let root = body.blocks.get(body.entry).map_err(arena)?.lexical_scope;
    let declaration = module.functions.get(function).map_err(arena)?;
    let accepts = declaration.signature.ownership_abi != OwnershipAbi::Observation;
    let destroy = declaration.attrs.iter().any(|attr| matches!(attr, FunctionAttr::Destroy(_)));
    for (index, value) in body.blocks.get(body.entry).map_err(arena)?.params.iter().enumerate() {
        let state = &mut initial.values[value.as_u32() as usize];
        state.masks.availability = 2;
        state.masks.lifetime = 1;
        if managed(body, *value, classification)? {
            let atom = Atom::current(value_shapes[value]);
            state.deps.insert(atom);
            if accepts && !(destroy && index == 0) {
                initial.journals.insert((root, atom), Quantity::ONE);
                initial.provenance.insert((root, atom), 2);
            }
        }
    }
    let mut entries = BTreeMap::from([(body.entry, initial)]);
    let ranks: BTreeMap<_, _> = graph.rpo.iter().enumerate().map(|(rank, block)| (*block, rank)).collect();
    let mut pending = BTreeSet::from([(ranks[&body.entry], body.entry)]);
    let mut evaluations = 0;
    while let Some((_, block)) = pending.pop_first() {
        budget.work(1)?;
        evaluations += 1;
        let outgoing = transfer(module, body, block, entries[&block].clone(), &graph,
            &value_shapes, classification, &mut budget)?;
        for (edge, state) in outgoing {
            budget.work(state.stored_facts())?;
            let changed = if let Some(previous) = entries.get_mut(&edge.target) {
                previous.join(&state)
            } else {
                entries.insert(edge.target, state);
                true
            };
            if changed {
                pending.insert((ranks[&edge.target], edge.target));
            }
        }
    }
    // Journal equations point to the existing origin families, never to a new
    // per-iteration expression or a correlated list of alternatives.
    let origin_values: BTreeMap<_, _> = value_shapes.iter().map(|(value, shape)| (*shape, *value)).collect();
    for flow in entries.values() {
        for (scope, atom) in flow.journals.keys() {
            budget.work(1)?;
            if let Some(value) = origin_values.get(&atom.origin) {
                let journal = equations.intern(Shape::Journal { owner: *scope, slot: *value }).unwrap();
                equations.connect(journal, atom.origin);
            }
        }
    }
    // Replay once at the fixed point. No provisional diagnostics or output.
    let mut outcomes = vec![];
    for block in &graph.rpo {
        if let Some(entry) = entries.get(block) {
            outcomes.extend(transfer(module, body, *block, entry.clone(), &graph,
                &value_shapes, classification, &mut budget)?);
        }
    }
    Ok(FunctionAnalysis {
        equations, value_shapes, entries, outcomes, evaluations, height_bound, stats: budget.stats,
    })
}

fn managed(body: &Body, value: ValueId, classification: &OwnAnalysis) -> Result<bool, OwnError> {
    let ty = body.values.get(value).map_err(arena)?.ty;
    Ok(classification.types.get(&ty).is_some_and(TypeClassification::may_own))
}

/// A may-dependency set is not a list of actual obligations. Only a definite
/// singleton current admission permits a strong source update. Otherwise both
/// possible locations retain Maybe facts; runtime scope.split chooses contents.
fn bind_admission(state: &mut FlowState, owner: ScopeId, deps: &BTreeSet<Atom>) {
    let moving: Vec<_> = state.journals.keys().filter(|(scope, atom)| {
        *scope != owner && deps.contains(atom)
            && state.journals[&(*scope, *atom)].multiplicity != Multiplicity::Bottom
            && state.provenance.get(&(*scope, *atom)).is_some_and(|mask| mask & 1 != 0)
    }).copied().collect();
    for key in moving {
        let quantity = state.journals[&key];
        let definite = deps.len() == 1 && key.1.incarnation == Incarnation::Current
            && quantity == Quantity::ONE && state.provenance.get(&key) == Some(&1);
        let selected = if definite {
            state.journals.remove(&key);
            state.provenance.remove(&key);
            quantity
        } else {
            let possible = Quantity::new(Presence::Maybe, quantity.multiplicity);
            state.journals.insert(key, possible);
            possible
        };
        let target = state.journals.entry((owner, key.1)).or_default();
        *target = target.append(selected);
    }
    for atom in deps {
        if let Some(quantity) = state.journals.get(&(owner, *atom)) {
            let mask = state.provenance.entry((owner, *atom)).or_default();
            if deps.len() == 1 && atom.incarnation == Incarnation::Current && *quantity == Quantity::ONE {
                *mask = 2;
            } else {
                *mask |= 2;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn transfer(
    module: &Module,
    body: &Body,
    block: BlockId,
    mut state: FlowState,
    graph: &Graph,
    shapes: &BTreeMap<ValueId, ShapeId>,
    classification: &OwnAnalysis,
    budget: &mut Budget<'_>,
) -> Result<Vec<(SemanticEdge, FlowState)>, OwnError> {
    let data = body.blocks.get(block).map_err(arena)?;
    let edges: Vec<_> = graph.outgoing[block.as_u32() as usize].iter().map(|index| &graph.edges[*index]).collect();
    let mut outcomes = vec![];
    for inst in &data.insts {
        budget.work(state.stored_facts() + 1)?;
        for edge in edges.iter().filter(|edge| edge.failure_point == Some(*inst)) {
            outcomes.push(((*edge).clone(), edge_state(module, body, edge, state.clone())?));
        }
        let instruction = body.insts.get(*inst).map_err(arena)?;
        match &instruction.op {
            Operation::Const(_) => {}
            Operation::Core(CoreOp::RangeNew { .. } | CoreOp::Undef { .. }) => {}
            Operation::Core(CoreOp::ObjNew { construction: ConstructionStatus::Complete, .. }) => {
                let mut retained = BTreeSet::new();
                for operand in crate::cfg::operation_uses(&instruction.op, body)
                    .map_err(|error| flow_error(format!("operands: {error}")))? {
                    retained.extend(&state.values[operand.value.as_u32() as usize].deps);
                }
                for result in &instruction.results {
                    if !managed(body, *result, classification)? {
                        continue;
                    }
                    let origin = shapes[result];
                    state.retire(origin);
                    let atom = Atom::current(origin);
                    let value = &mut state.values[result.as_u32() as usize];
                    value.deps = retained.clone();
                    value.deps.insert(atom);
                    state.journals.insert((data.lexical_scope, atom), Quantity::ONE);
                    state.provenance.insert((data.lexical_scope, atom), 1);
                }
            }
            Operation::Lexical(LexicalOp::Bind { binding, value }
                | LexicalOp::Rebind { binding, value, .. }) => {
                let owner = body.bindings.get(*binding).map_err(arena)?.scope;
                let deps = state.values[value.as_u32() as usize].deps.clone();
                if matches!(instruction.op, Operation::Lexical(LexicalOp::Rebind { .. })) {
                    let previous = state.binding_deps.get(binding).cloned().unwrap_or_default();
                    for atom in previous.difference(&deps) {
                        if atom.incarnation == Incarnation::Current {
                            state.retire(atom.origin);
                        }
                    }
                }
                state.binding_deps.insert(*binding, deps.clone());
                bind_admission(&mut state, owner, &deps);
                state.latest.insert(*binding, BTreeSet::from([*value]));
            }
            Operation::Lexical(LexicalOp::Temp { .. }) => {}
            Operation::Core(_) if instruction.results.iter().all(|value| {
                !classification.types.get(&body.values.get(*value).unwrap().ty)
                    .is_some_and(TypeClassification::may_own)
            }) && instruction.op.operand_refs().iter().all(|operand| match operand.reference {
                OperandRef::Value(value) => !classification.types.get(&body.values.get(value).unwrap().ty)
                    .is_some_and(TypeClassification::may_own),
                _ => false,
            }) => {}
            _ => return Err(commit3(instruction.op.opcode())),
        }
        for value in &instruction.results {
            let masks = &mut state.values[value.as_u32() as usize].masks;
            masks.availability = 2;
            masks.lifetime = 1;
            if matches!(module.types.get(body.values.get(*value).map_err(arena)?.ty), Ok(Type::Successor(_))) {
                masks.must_use = 2;
            }
        }
    }
    for edge in edges.iter().filter(|edge| edge.failure_point.is_none()) {
        outcomes.push(((*edge).clone(), edge_state(module, body, edge, state.clone())?));
    }
    Ok(outcomes)
}

fn edge_state(
    module: &Module,
    body: &Body,
    edge: &SemanticEdge,
    mut state: FlowState,
) -> Result<FlowState, OwnError> {
    // Read all outgoing references first, then bind every phi simultaneously.
    let arguments: Vec<_> = edge.args.iter().map(|value| state.values[value.as_u32() as usize].clone()).collect();
    for scope in &edge.plan.close {
        state.journals.retain(|(owner, _), _| owner != scope);
        state.provenance.retain(|(owner, _), _| owner != scope);
        state.latest.retain(|binding, _| body.bindings.get(*binding).is_ok_and(|decl| decl.scope != *scope));
        state.binding_deps.retain(|binding, _| body.bindings.get(*binding).is_ok_and(|decl| decl.scope != *scope));
    }
    for (parameter, value) in body.blocks.get(edge.target).map_err(arena)?.params.iter().zip(arguments) {
        state.values[parameter.as_u32() as usize] = value;
    }
    // Lexical ownership is keyed by ScopeId, independent of incoming SSA names.
    let _ = module;
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phi_selection_weakly_moves_possible_admissions_and_never_takes_bound_parts() {
        let source = ScopeId::from_u32(1);
        let target = ScopeId::from_u32(0);
        let a = Atom::current(ShapeId(0));
        let b = Atom::current(ShapeId(1));
        let bound = Atom::current(ShapeId(2));
        let mut state = FlowState::default();
        for atom in [a, b, bound] {
            state.journals.insert((source, atom), Quantity::ONE);
            state.provenance.insert((source, atom), if atom == bound { 2 } else { 1 });
        }
        bind_admission(&mut state, target, &BTreeSet::from([a, b, bound]));
        for atom in [a, b] {
            assert_eq!(state.journals[&(source, atom)], Quantity::new(Presence::Maybe, Multiplicity::One));
            assert_eq!(state.journals[&(target, atom)], Quantity::new(Presence::Maybe, Multiplicity::One));
        }
        assert_eq!(state.journals[&(source, bound)], Quantity::ONE);
        assert!(!state.journals.contains_key(&(target, bound)));
    }
}
