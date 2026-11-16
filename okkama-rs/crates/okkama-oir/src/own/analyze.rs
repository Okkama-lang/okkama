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
    pub before_instructions: BTreeMap<InstId, FlowState>,
    pub before_edges: BTreeMap<BlockId, FlowState>,
    /// Possible original admission owners of a pre-accept construction packet.
    pub constructor_inputs: BTreeMap<InstId, BTreeSet<ScopeId>>,
    pub outcomes: Vec<(SemanticEdge, FlowState)>,
    pub evaluations: usize,
    /// Upper bound on strict product ascents per program point. Set membership
    /// contributes one; quantity contributes at most four; masks one per bit.
    pub height_bound: usize,
    pub stats: OwnStats,
}

impl FunctionAnalysis {
    /// Retained allocations after graph/worklist/transfer scratch has been dropped.
    pub(super) fn allocated_bytes(&self) -> usize {
        size_of::<Self>() + self.equations.allocated_bytes() + self.value_shapes.len() * 96
            + self.entries.values().chain(self.before_instructions.values()).chain(self.before_edges.values())
                .map(|state| state.allocated_bytes() + 128).sum::<usize>()
            + self.constructor_inputs.values().map(|owners| 128 + owners.len() * 96).sum::<usize>()
            + self.outcomes.capacity() * size_of::<(SemanticEdge, FlowState)>()
            + self.outcomes.iter().map(|(edge, state)| {
                state.allocated_bytes() + edge.args.capacity() * size_of::<ValueId>()
                    + (edge.plan.close.capacity() + edge.plan.begin.capacity()) * size_of::<ScopeId>()
            }).sum::<usize>()
    }
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
    let builder_owners = if arrays::creates_builders(body) {
        budget.work(body.insts.len() + body.blocks.len())?;
        let owners = arrays::builder_owners(module, body)?;
        budget.memory(owners.len() * 96)?;
        owners
    } else {
        BTreeMap::new()
    };
    let mut equations = Equations::default();
    let empty = equation(&mut equations, Shape::Empty, &mut budget)?;
    let mut value_shapes = BTreeMap::new();
    for (value, data) in body.values.iter() {
        budget.work(1)?;
        budget.memory(96)?;
        let shape = if managed(body, value, classification)? {
            let shape = match data.def {
                Definition::BlockParam { block, index } if block == body.entry => Shape::Input { parameter: index },
                Definition::BlockParam { block, index } => Shape::Phi { block, parameter: index },
                Definition::InstResult { inst, .. } => Shape::Fresh { origin: Site::Instruction(inst), current: value },
            };
            equation(&mut equations, shape, &mut budget)?
        } else {
            empty
        };
        value_shapes.insert(value, shape);
    }
    let scopes: Vec<_> = module.scopes.iter().filter_map(|(scope, decl)| {
        (decl.function == function).then_some(scope)
    }).collect();
    budget.memory(scopes.capacity() * size_of::<ScopeId>())?;
    for scope in &scopes {
        for (value, shape) in &value_shapes {
            budget.work(1)?;
            if *shape != empty {
                equation(&mut equations, Shape::Journal { owner: *scope, slot: *value }, &mut budget)?;
            }
        }
    }
    for edge in &graph.edges {
        for (argument, parameter) in edge.args.iter().zip(&body.blocks.get(edge.target).map_err(arena)?.params) {
            if value_shapes[parameter] != empty && value_shapes[argument] != empty
                && equations.connect(value_shapes[parameter], value_shapes[argument]) {
                budget.memory(96)?;
            }
        }
    }
    equations.freeze();
    let origins = equations.nodes.len();
    let values = body.values.len();
    let scopes = scopes.len();
    let height_bound = values.saturating_mul(origins.saturating_mul(2).saturating_add(64))
        .saturating_add(scopes.saturating_mul(origins).saturating_mul(8))
        .saturating_add(body.bindings.len().saturating_mul(values.saturating_add(origins * 2)));
    let height_bound = height_bound.saturating_add(scopes.saturating_mul(origins).saturating_mul(4));
    // Rank, pending and block-state map nodes; no product of possible facts and CFG points.
    budget.memory(body.blocks.len().saturating_mul(256))?;
    // Only values whose type may own carry facts (#707); every one of them has a state at every point.
    let tracked: Vec<_> = value_shapes.iter().filter(|(_, shape)| **shape != empty).map(|(value, _)| *value)
        .collect();
    let initial_bytes = size_of::<FlowState>() + tracked.len() * (size_of::<(ValueId, ValueState)>() + 64);
    budget.memory(initial_bytes)?;
    let mut initial = FlowState::default();
    for value in tracked {
        initial.values.insert(value, ValueState::ABSENT);
    }
    let root = body.blocks.get(body.entry).map_err(arena)?.lexical_scope;
    let declaration = module.functions.get(function).map_err(arena)?;
    let accepts = declaration.signature.ownership_abi != OwnershipAbi::Observation;
    let destroy = declaration.attrs.iter().any(|attr| matches!(attr, FunctionAttr::Destroy(_)));
    let mut initial_reserved = initial_bytes;
    for (index, value) in body.blocks.get(body.entry).map_err(arena)?.params.iter().enumerate() {
        if managed(body, *value, classification)? {
            let state = &mut initial.values[*value];
            state.masks.availability = 2;
            state.masks.lifetime = 1;
            // One dependency plus at most one journal/provenance entry.
            budget.memory(3 * 128)?;
            initial_reserved += 3 * 128;
            let atom = Atom::current(value_shapes[value]);
            initial.values[*value].deps.insert(atom);
            if accepts && !(destroy && index == 0) {
                initial.journals.insert((root, atom), Quantity::ONE);
                initial.provenance.insert((root, atom), 2);
            }
        }
    }
    budget.resize(initial_reserved, initial.allocated_bytes())?;
    let mut entries = BTreeMap::from([(body.entry, initial)]);
    let ranks: BTreeMap<_, _> = graph.rpo.iter().enumerate().map(|(rank, block)| (*block, rank)).collect();
    let mut pending = BTreeSet::from([(ranks[&body.entry], body.entry)]);
    let mut evaluations = 0;
    while let Some((_, block)) = pending.pop_first() {
        budget.work(1)?;
        evaluations += 1;
        let outgoing = transfer(module, body, block, &entries[&block], &graph,
            &value_shapes, &builder_owners, classification, &mut budget, None, None, None)?;
        for (edge, state) in outgoing {
            budget.work(state.stored_facts())?;
            let changed = if let Some(previous) = entries.get_mut(&edge.target) {
                let old = previous.allocated_bytes();
                let scratch = old.saturating_add(state.allocated_bytes().saturating_mul(2));
                budget.memory(scratch)?;
                let changed = previous.join(&state);
                budget.resize(old, previous.allocated_bytes())?;
                budget.release(scratch + state.allocated_bytes());
                changed
            } else {
                entries.insert(edge.target, state);
                true
            };
            budget.release(outcome_bytes(&edge));
            if changed {
                pending.insert((ranks[&edge.target], edge.target));
            }
        }
    }
    // Journal equations point to the existing origin families, never to a new
    // per-iteration expression or a correlated list of alternatives.
    budget.memory(value_shapes.len() * 96)?;
    let origin_values: BTreeMap<_, _> = value_shapes.iter().filter(|(_, shape)| **shape != empty)
        .map(|(value, shape)| (*shape, *value)).collect();
    for flow in entries.values() {
        for (scope, atom) in flow.journals.keys() {
            budget.work(1)?;
            if let Some(value) = origin_values.get(&atom.origin) {
                let journal = equations.intern(Shape::Journal { owner: *scope, slot: *value }).unwrap();
                if equations.connect(journal, atom.origin) {
                    budget.memory(96)?;
                }
            }
        }
    }
    // Replay once at the fixed point. No provisional diagnostics or output.
    let mut outcomes = vec![];
    let mut before_edges = BTreeMap::new();
    let mut constructor_inputs = BTreeMap::new();
    let mut before_instructions = BTreeMap::new();
    for block in &graph.rpo {
        if let Some(entry) = entries.get(block) {
            outcomes.extend(transfer(module, body, *block, entry, &graph,
                &value_shapes, &builder_owners, classification, &mut budget, Some(&mut before_edges),
                Some(&mut constructor_inputs), Some(&mut before_instructions))?);
        }
    }
    for (edge, state) in &outcomes {
        let target = body.blocks.get(edge.target).map_err(arena)?;
        let incoming = &entries[&edge.target].defers;
        budget.work(1 + state.defers.values().map(Vec::len).sum::<usize>()
            + incoming.values().map(Vec::len).sum::<usize>())?;
        let terminal_handler = target.insts.is_empty() && edge.args.is_empty()
            && matches!(target.term.kind, TermKind::Raise(value) if target.params.last() == Some(&value));
        if !terminal_handler && &state.defers != incoming {
            return Err(flow_error(format!(
                "defer prefixes differ at live join {:?}; conditional registration needs CFG specialization",
                edge.target,
            )));
        }
    }
    Ok(FunctionAnalysis {
        equations, value_shapes, entries, before_instructions, before_edges, constructor_inputs, outcomes, evaluations,
        height_bound, stats: budget.stats,
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
    transfer_selected(state, owner, None, deps, true, 2);
}

/// Move the selected journal entries to `owner`; `sources`, when given, limits
/// the scopes they may leave.
fn transfer_selected(
    state: &mut FlowState,
    owner: ScopeId,
    sources: Option<&[ScopeId]>,
    deps: &BTreeSet<Atom>,
    admissible: bool,
    provenance: u8,
) {
    let moving: Vec<_> = state.journals.keys().filter(|(scope, atom)| {
        *scope != owner && sources.is_none_or(|sources| sources.contains(scope)) && deps.contains(atom)
            && state.journals[&(*scope, *atom)].multiplicity != Multiplicity::Bottom
            && (!admissible || state.provenance.get(&(*scope, *atom)).is_some_and(|mask| mask & 1 != 0))
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
            // A cache duty stays in its owner state: binding or staging its Future neither moves nor
            // stages the duty (D12, OIR-2 §10.2), so it can still leave by cache.rehome. PR-B1 (#784):
            // a binding adds the bound bit and keeps the duty bit, which `rehome_caches` selects by.
            if *mask & futures::CACHE_DUTY != 0 {
                if provenance == 2 {
                    *mask |= 2;
                }
                continue;
            }
            if deps.len() == 1 && atom.incarnation == Incarnation::Current && *quantity == Quantity::ONE {
                *mask = provenance;
            } else {
                *mask |= provenance;
            }
        }
    }
}

/// A builder transition's `held(...)` transfers (#697): every scope inside the builder's owner gives the owner what
/// it holds of the inserted values, admitted or bound. Scopes outside `sources` keep theirs.
fn rehome_held(state: &mut FlowState, owner: ScopeId, sources: &[ScopeId], inserted: &BTreeSet<Atom>) {
    transfer_selected(state, owner, Some(sources), inserted, false, 1);
}

#[allow(clippy::too_many_arguments)]
fn transfer(
    module: &Module,
    body: &Body,
    block: BlockId,
    entry: &FlowState,
    graph: &Graph,
    shapes: &BTreeMap<ValueId, ShapeId>,
    builder_owners: &BTreeMap<ValueId, ScopeId>,
    classification: &OwnAnalysis,
    budget: &mut Budget<'_>,
    snapshots: Option<&mut BTreeMap<BlockId, FlowState>>,
    mut constructor_inputs: Option<&mut BTreeMap<InstId, BTreeSet<ScopeId>>>,
    mut before_instructions: Option<&mut BTreeMap<InstId, FlowState>>,
) -> Result<Vec<(SemanticEdge, FlowState)>, OwnError> {
    budget.memory(entry.allocated_bytes())?;
    let mut state = entry.clone();
    budget.resize(entry.allocated_bytes(), state.allocated_bytes())?;
    let data = body.blocks.get(block).map_err(arena)?;
    let edge_refs = graph.outgoing[block.as_u32() as usize].len() * size_of::<&SemanticEdge>();
    budget.memory(edge_refs)?;
    let edges: Vec<_> = graph.outgoing[block.as_u32() as usize].iter().map(|index| &graph.edges[*index]).collect();
    let mut outcomes = vec![];
    for inst in &data.insts {
        budget.work(state.stored_facts() + 1)?;
        if let Some(snapshots) = before_instructions.as_deref_mut() {
            budget.memory(state.allocated_bytes() + 128)?;
            let snapshot = state.clone();
            budget.resize(state.allocated_bytes(), snapshot.allocated_bytes())?;
            snapshots.insert(*inst, snapshot);
        }
        for edge in edges.iter().filter(|edge| edge.failure_point == Some(*inst)) {
            outcomes.push(outcome(module, body, edge, &state, budget)?);
        }
        let instruction = body.insts.get(*inst).map_err(arena)?;
        let old = state.allocated_bytes();
        // Transfers clone dependency sets, retire maps and operand vectors. Reserve scratch
        // before allocation, then retain only the resulting sparse state's actual capacity.
        let scratch = state.sparse_bytes().saturating_mul(2 + instruction.results.len())
            .saturating_add(instruction.op.operand_refs().len().saturating_mul(128)).saturating_add(1024);
        budget.memory(scratch)?;
        if let Some(inputs) = constructor_inputs.as_deref_mut() {
            let fields = match &instruction.op {
                Operation::Core(CoreOp::ObjNew { fields, construction: ConstructionStatus::Complete, .. }) => {
                    Some(fields.clone())
                }
                Operation::Core(CoreOp::ObjNext { updates, .. }) => {
                    Some(updates.iter().map(|field| field.value).collect())
                }
                _ => None,
            };
            if let Some(fields) = fields {
                let deps: BTreeSet<_> = fields.iter().flat_map(|value| {
                    state.values[*value].deps.iter().copied()
                }).collect();
                let owners: BTreeSet<_> = state.journals.iter().filter(|((scope, atom), quantity)| {
                    deps.contains(atom) && !matches!(quantity.presence, Presence::Absent | Presence::Bottom)
                        && state.provenance.get(&(*scope, *atom)).is_some_and(|mask| mask & 1 != 0)
                }).map(|((scope, _), _)| *scope).collect();
                budget.work(1 + deps.len() + state.journals.len())?;
                budget.memory(32 + owners.len() * 16)?;
                inputs.insert(*inst, owners);
            }
        }
        match &instruction.op {
            Operation::Collection(CollectionOp::BuilderNew { .. } | CollectionOp::MapBuilderNew { .. }) => {}
            // A transition moves the held parts of inserted values, admitted or bound (#697),
            // from the scopes inside the builder's owner into the owner's journal.
            Operation::Collection(collection) if arrays::consumed_buffer(&instruction.op).is_some() => {
                let buffer = arrays::consumed_buffer(&instruction.op).unwrap();
                let mut deps = state.values[buffer].deps.clone();
                let mut inserted = BTreeSet::new();
                for value in arrays::inserted(collection) {
                    inserted.extend(state.values[value].deps.iter().copied());
                }
                // Every transition, finish and abandon included, lies inside the builder's owner.
                let owner = *builder_owners.get(&buffer)
                    .ok_or_else(|| flow_error("builder transition has no builder.new owner"))?;
                let sources = arrays::inner_scopes(module, data.lexical_scope, owner)?;
                if !inserted.is_empty() {
                    rehome_held(&mut state, owner, &sources, &inserted);
                    deps.extend(inserted);
                }
                for result in &instruction.results {
                    if managed(body, *result, classification)? {
                        state.values[*result].deps = deps.clone();
                    }
                }
            }
            // Derived collections, variants and projections create no identity and
            // take no packet: inserted obligations stay with their owners and the
            // result depends on everything its operands depend on.
            Operation::Collection(_)
            | Operation::Core(CoreOp::VariantOf { .. } | CoreOp::VariantGet { .. } | CoreOp::Retag { .. }) => {
                let mut deps = BTreeSet::new();
                for operand in cfg::operation_uses(&instruction.op, body)
                    .map_err(|error| flow_error(format!("operands: {error}")))? {
                    deps.extend(&state.values[operand.value].deps);
                }
                for result in &instruction.results {
                    if managed(body, *result, classification)? {
                        state.values[*result].deps = deps.clone();
                    }
                }
            }
            // Plain observations borrow their operands and produce no dependency.
            Operation::Core(_) if routing::handling(&instruction.op) == routing::Handling::Scalar
                && instruction.results.iter().all(|value| !classification.types
                    .get(&body.values.get(*value).unwrap().ty).is_some_and(TypeClassification::may_own)) => {}
            Operation::Const(_) => {}
            Operation::Core(CoreOp::RangeNew { .. } | CoreOp::Undef { .. }) => {}
            Operation::Core(CoreOp::ObjNew { construction: ConstructionStatus::Complete, .. }
                | CoreOp::VariantNew { .. }) => {
                let mut retained = BTreeSet::new();
                for operand in crate::cfg::operation_uses(&instruction.op, body)
                    .map_err(|error| flow_error(format!("operands: {error}")))? {
                    retained.extend(&state.values[operand.value].deps);
                }
                for result in &instruction.results {
                    if !managed(body, *result, classification)? {
                        continue;
                    }
                    let origin = shapes[result];
                    state.retire(origin);
                    let atom = Atom::current(origin);
                    let value = &mut state.values[*result];
                    value.deps = retained.clone();
                    value.deps.insert(atom);
                    state.journals.insert((data.lexical_scope, atom), Quantity::ONE);
                    state.provenance.insert((data.lexical_scope, atom), 1);
                }
            }
            Operation::Core(CoreOp::FnClosure { .. } | CoreOp::FnBind { .. } | CoreOp::FnBindRole { .. }
                | CoreOp::CapMake { .. } | CoreOp::AnyBox { .. } | CoreOp::RoleWrap { .. }
                | CoreOp::RoleNarrow { .. } | CoreOp::OnceLoad { .. }) => {
                // Wrapping snapshots references and their full dependencies. It
                // neither creates an identity nor promotes borrowed admission.
                let mut deps = BTreeSet::new();
                for operand in cfg::operation_uses(&instruction.op, body)
                    .map_err(|error| flow_error(format!("operands: {error}")))? {
                    deps.extend(&state.values[operand.value].deps);
                }
                for result in &instruction.results {
                    if managed(body, *result, classification)? {
                        state.values[*result].deps = deps.clone();
                    }
                }
            }
            Operation::Core(CoreOp::OnceStore { value, .. }) => {
                let ty = body.values.get(*value).map_err(arena)?.ty;
                if !classification.types[&ty].once_plain {
                    return Err(flow_error("once.store requires a value that cannot retain a resource"));
                }
            }
            Operation::Lexical(LexicalOp::Bind { binding, value }
                | LexicalOp::Rebind { binding, value, .. }) => {
                let owner = body.bindings.get(*binding).map_err(arena)?.scope;
                let deps = state.values[*value].deps.clone();
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
            Operation::Lexical(LexicalOp::Temp { .. } | LexicalOp::Want { .. }
                | LexicalOp::Discard { .. })
            | Operation::Exit(ExitOp::Prepare { .. }) => {}
            Operation::Lexical(LexicalOp::DeferRegister { scope, .. }) => {
                let prefix = state.defers.entry(*scope).or_default();
                if !prefix.contains(inst) {
                    prefix.push(*inst);
                }
            }
            Operation::Lexical(LexicalOp::Stage { action, value, .. }) => {
                let deps = state.values[*value].deps.clone();
                transfer_selected(&mut state, *action, None, &deps, false, 4);
            }
            Operation::Core(CoreOp::ObjNew { construction: ConstructionStatus::Incomplete, .. }) => {
                for result in &instruction.results {
                    if managed(body, *result, classification)? {
                        state.values[*result].deps.insert(Atom::current(shapes[result]));
                    }
                }
            }
            Operation::Core(CoreOp::ObjInit { object, field, value, .. }) => {
                let incoming = state.values[*value].deps.clone();
                state.values[*object].deps.extend(incoming);
                if module.declarations.get(field.declaration).is_ok_and(|declaration| {
                    matches!(&declaration.kind, DeclarationKind::Object { fields, .. }
                        if fields[field.index as usize].name == "$pending")
                }) {
                    let owner = super::edges::stack(module, data.lexical_scope)?[0];
                    let atom = Atom::current(shapes[object]);
                    state.journals.insert((owner, atom), Quantity::ONE);
                    state.provenance.insert((owner, atom), 1);
                }
            }
            Operation::Call(_) | Operation::Core(CoreOp::ObjNext { .. }
                | CoreOp::ObjGet { .. } | CoreOp::ObjPending { .. } | CoreOp::SuccTake { .. }) => {
                let mut deps = BTreeSet::new();
                for operand in cfg::operation_uses(&instruction.op, body)
                    .map_err(|error| flow_error(format!("operands: {error}")))? {
                    deps.extend(&state.values[operand.value].deps);
                }
                // An async call creates a Future whose cache duty joins its creation
                // scope (D12). The duty is never offered, bound or staged.
                let provenance = match &instruction.op {
                    Operation::Call(CallOp::Direct { function, .. }) if futures::is_async(module, *function) => {
                        futures::CACHE_DUTY
                    }
                    // A returned Future brings its cache duty in the result packet (D25.1): the same
                    // duty, never offered, and leaving a scope only by cache.rehome.
                    Operation::Call(_) if instruction.results.iter().any(|result| {
                        body.values.get(*result).is_ok_and(|value| futures::is_future(module, value.ty))
                    }) => futures::CACHE_DUTY,
                    _ => 1,
                };
                for result in &instruction.results {
                    if managed(body, *result, classification)? {
                        let origin = shapes[result];
                        state.retire(origin);
                        let atom = Atom::current(origin);
                        state.values[*result].deps = deps.clone();
                        state.values[*result].deps.insert(atom);
                        state.journals.insert((data.lexical_scope, atom), Quantity::ONE);
                        state.provenance.insert((data.lexical_scope, atom), provenance);
                    }
                }
            }
            // A shared await borrows the cached outcome: the value depends on the
            // stable cache service key and the outcome's external deps (§13.2).
            Operation::Async(AsyncOp::Await { future, .. }) => {
                let deps = state.values[*future].deps.clone();
                for result in &instruction.results {
                    if managed(body, *result, classification)? {
                        state.values[*result].deps = deps.clone();
                    }
                }
            }
            // A cancellation request and the Future queries observe the Future; their results are scalars. A
            // spawn (D24.1) has no result: the task duty it adds travels with the Future's cache duty.
            Operation::Async(AsyncOp::FutureCancel { .. } | AsyncOp::FutureDone { .. }
                | AsyncOp::FutureCancelled { .. } | AsyncOp::FutureSpawn { state: None, .. }) => {}
            Operation::Core(_) if instruction.results.iter().all(|value| {
                !classification.types.get(&body.values.get(*value).unwrap().ty)
                    .is_some_and(TypeClassification::may_own)
            }) && instruction.op.operand_refs().iter().all(|operand| match operand.reference {
                OperandRef::Value(value) => !classification.types.get(&body.values.get(value).unwrap().ty)
                    .is_some_and(TypeClassification::may_own),
                _ => false,
            }) => {}
            _ => return Err(unsupported(&instruction.op)),
        }
        budget.resize(old, state.allocated_bytes())?;
        budget.release(scratch);
        for value in &instruction.results {
            if !managed(body, *value, classification)? {
                continue;
            }
            let masks = &mut state.values[*value].masks;
            masks.availability = 2;
            masks.lifetime = 1;
            if matches!(module.types.get(body.values.get(*value).map_err(arena)?.ty), Ok(Type::Successor(_))) {
                masks.must_use = 2;
            }
        }
    }
    if let Some(snapshots) = snapshots {
        budget.work(state.stored_facts())?;
        budget.memory(state.allocated_bytes() + 128)?;
        let snapshot = state.clone();
        budget.resize(state.allocated_bytes(), snapshot.allocated_bytes())?;
        snapshots.insert(block, snapshot);
    }
    for edge in edges.iter().filter(|edge| edge.failure_point.is_none()) {
        outcomes.push(outcome(module, body, edge, &state, budget)?);
    }
    budget.release(state.allocated_bytes() + edge_refs);
    Ok(outcomes)
}

fn equation(equations: &mut Equations, shape: Shape, budget: &mut Budget<'_>) -> Result<ShapeId, OwnError> {
    // Node, interning-map node, and spare vector capacity. Shapes indexed here have no heap children.
    budget.memory(256)?;
    Ok(equations.intern(shape).expect("index before freeze"))
}

fn outcome_bytes(edge: &SemanticEdge) -> usize {
    // Copied edge payload, outcome vector capacity and map-node allowance when retained.
    2 * size_of::<(SemanticEdge, FlowState)>() + 128
        + edge.args.len() * size_of::<ValueId>()
        + (edge.plan.close.len() + edge.plan.begin.len()) * size_of::<ScopeId>()
}

fn outcome(
    module: &Module,
    body: &Body,
    edge: &SemanticEdge,
    state: &FlowState,
    budget: &mut Budget<'_>,
) -> Result<(SemanticEdge, FlowState), OwnError> {
    let old = state.allocated_bytes();
    let scratch = state.sparse_bytes().saturating_mul(2)
        .saturating_add(edge.args.len().saturating_mul(size_of::<ValueState>())).saturating_add(1024);
    budget.memory(old.saturating_add(scratch).saturating_add(outcome_bytes(edge)))?;
    let result = edge_state(module, body, edge, state.clone())?;
    budget.resize(old, result.allocated_bytes())?;
    budget.release(scratch);
    Ok((edge.clone(), result))
}

fn edge_state(
    module: &Module,
    body: &Body,
    edge: &SemanticEdge,
    mut state: FlowState,
) -> Result<FlowState, OwnError> {
    // Read all outgoing references first, then bind every phi simultaneously.
    let arguments: Vec<_> = edge.args.iter().map(|value| state.values.get(*value).cloned()).collect();
    if !edge.unwind && !edge.plan.close.is_empty() {
        let wanted = super::edges::wanted(body, edge)?;
        let deps: BTreeSet<_> = edge.args.iter().zip(&arguments).filter(|(value, _)| wanted.contains(value))
            .flat_map(|(_, value)| value.iter().flat_map(|value| value.deps.iter().copied())).collect();
        let destination = body.blocks.get(edge.target).map_err(arena)?.lexical_scope;
        let moving: Vec<_> = state.journals.iter().filter(|((scope, atom), _)| {
            edge.plan.close.contains(scope) && deps.contains(atom)
        }).map(|(key, quantity)| (*key, *quantity)).collect();
        for ((source, atom), quantity) in moving {
            let target = state.journals.entry((destination, atom)).or_default();
            *target = target.append(quantity);
            // A cache duty stays a cache duty in the landing scope: cache.rehome moves it,
            // and it never becomes admissible for an offer (C-4, OIR-2 §10.2).
            let duty = state.provenance.get(&(source, atom)).is_some_and(|mask| mask & futures::CACHE_DUTY != 0);
            state.provenance.insert((destination, atom), if duty { futures::CACHE_DUTY } else { 1 });
        }
    }
    for scope in &edge.plan.close {
        state.defers.remove(scope);
        state.journals.retain(|(owner, _), _| owner != scope);
        state.provenance.retain(|(owner, _), _| owner != scope);
        state.latest.retain(|binding, _| body.bindings.get(*binding).is_ok_and(|decl| decl.scope != *scope));
        state.binding_deps.retain(|binding, _| body.bindings.get(*binding).is_ok_and(|decl| decl.scope != *scope));
    }
    // A parameter has the type, and so the tracking, of its argument.
    for (parameter, value) in body.blocks.get(edge.target).map_err(arena)?.params.iter().zip(arguments) {
        match value {
            Some(value) => state.values.insert(*parameter, value),
            None => {
                state.values.remove(*parameter);
            }
        }
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

    /// #697: a builder element may borrow what a scope inside the builder's owner binds (a cursor step bound in
    /// the iteration scope). The held rehome moves the admitted and the bound parts to the owner; a scope outside
    /// the transition's sources keeps its part.
    #[test]
    fn builder_rehome_moves_admitted_and_bound_parts_of_inner_scopes_only() {
        let owner = ScopeId::from_u32(0);
        let iteration = ScopeId::from_u32(1);
        let elsewhere = ScopeId::from_u32(2);
        let admitted = Atom::current(ShapeId(0));
        let bound = Atom::current(ShapeId(1));
        let outside = Atom::current(ShapeId(2));
        let mut state = FlowState::default();
        for (scope, atom, mask) in [(iteration, admitted, 1), (iteration, bound, 2), (elsewhere, outside, 2)] {
            state.journals.insert((scope, atom), Quantity::ONE);
            state.provenance.insert((scope, atom), mask);
        }
        rehome_held(&mut state, owner, &[iteration], &BTreeSet::from([admitted, bound, outside]));
        // Several inserted atoms make each move possible, not definite (§4.3: no correlated state).
        for atom in [admitted, bound] {
            assert_eq!(state.journals[&(iteration, atom)], Quantity::new(Presence::Maybe, Multiplicity::One));
            assert_eq!(state.journals[&(owner, atom)], Quantity::new(Presence::Maybe, Multiplicity::One));
        }
        assert_eq!(state.journals[&(elsewhere, outside)], Quantity::ONE);
        assert!(!state.journals.contains_key(&(owner, outside)));
    }
}
