//! A single contract-driven path for consuming instructions and all outcomes.
use super::*;
use super::bindings::{block, parameter, plain_or_service, LatestValue};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CleanupContext {
    pub failure: Option<ValueId>,
    pub primary_before_failure: Option<ValueId>,
    pub pending: Option<ValueId>,
    /// High identity of the current reserved result.
    pub wanted: Option<ValueId>,
    /// Simultaneous language values crossing a CFG edge, selected as one union.
    pub carried: Vec<ValueId>,
    /// A replacement return can abandon a void or differently typed edge.
    pub replacement: Option<LatestValue>,
    pub returning: bool,
    pub destination: Option<ExitDestination>,
    pub defers: BTreeMap<ScopeId, Vec<InstId>>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveEnv {
    pub scopes: BTreeMap<ScopeId, ValueId>,
    pub linear: BTreeSet<ValueId>,
    pub bindings: BTreeMap<BindingId, LatestValue>,
    /// Source names map to their current SSA references and complete deps.
    pub values: BTreeMap<ValueId, LatestValue>,
    pub cleanup: CleanupContext,
    /// High references needed by the original exceptional continuation.
    pub unwind_values: Option<BTreeSet<ValueId>>,
    /// A returned pre-accept construction packet keeps its original admission.
    pub restore_to: BTreeMap<ValueId, ScopeId>,
    /// High builder buffer -> the linear element journal travelling with it.
    /// Journal packets are also members of `linear`.
    pub journals: BTreeMap<ValueId, ValueId>,
}
impl LiveEnv {
    pub fn all_linear(&self) -> BTreeSet<ValueId> {
        self.linear.iter().copied().chain(self.scopes.values().copied()).collect()
    }
    pub fn value(&self, original: ValueId) -> Result<LatestValue, OwnError> {
        self.values.get(&original).copied().ok_or_else(|| flow_error("value has no latest SSA definition"))
    }
}
#[derive(Clone, Debug)]
pub struct Landing {
    pub block: BlockId,
    pub env: LiveEnv,
    pub results: Vec<(SlotName, ValueId)>,
    pub forwarded: BTreeMap<ValueId, ValueId>,
}
impl Landing {
    pub fn result(&self, name: SlotName) -> Result<ValueId, OwnError> {
        self.results.iter().find(|(slot, _)| *slot == name).map(|(_, value)| *value)
            .ok_or_else(|| flow_error(format!("outcome omits {name:?}")))
    }
}
#[derive(Clone, Debug)]
pub struct Emission {
    pub normal: Landing,
    pub unwind: Option<Landing>,
    /// Cancel uses the same physical landing only after positional equality was
    /// checked. Its environment is constructed from its own contract.
    pub cancel: Option<Landing>,
}

#[derive(Debug)]
pub struct OutcomeEmitter<'a> {
    budget: Budget<'a>,
    managed: BTreeSet<TypeId>,
}
impl<'a> OutcomeEmitter<'a> {
    pub fn new(analysis: &OwnAnalysis, options: &'a OwnOptions) -> Self {
        Self {
            budget: Budget::new(options),
            managed: analysis.types.iter().filter_map(|(ty, facts)| {
                (facts.may_own() || analysis.managed_references.contains(ty)).then_some(*ty)
            }).collect(),
        }
    }
    pub(super) fn budget_ref(&mut self) -> &mut Budget<'a> {
        &mut self.budget
    }
    pub fn stats(&self) -> &OwnStats {
        &self.budget.stats
    }
    pub(super) fn charge(&mut self, work: usize, memory: usize) -> Result<(), OwnError> {
        self.budget.work(work)?;
        self.budget.memory(memory)
    }
    pub fn emit(
        &mut self,
        module: &mut Module,
        body: &mut Body,
        source: BlockId,
        env: LiveEnv,
        op: Operation,
    ) -> Result<Emission, OwnError> {
        self.emit_with_result_deps(module, body, source, env, op, &[])
    }
    /// Complete dependency recipes supplied by analysis for results whose
    /// contract has neither a deps result nor a constructor identity formula.
    /// Recipes name full deps, never the split's external-deps remainder.
    #[allow(clippy::too_many_arguments)]
    pub fn emit_with_result_deps(
        &mut self,
        module: &mut Module,
        body: &mut Body,
        source: BlockId,
        env: LiveEnv,
        op: Operation,
        result_deps: &[(SlotName, DepExprId)],
    ) -> Result<Emission, OwnError> {
        if !matches!(body.blocks.get(source).map_err(arena)?.term.kind, TermKind::Unreachable) {
            return Err(flow_error("outcome emission requires an unterminated source block"));
        }
        let lexical = body.blocks.get(source).map_err(arena)?.lexical_scope;
        self.budget.stats.active_function = Some(module.scopes.get(lexical).map_err(arena)?.function);
        let contract = ContractRegistry::new(module).lookup(&op)
            .map_err(|error| flow_error(format!("{} contract: {error}", op.opcode())))?;
        let consumed = self.consumed(module, body, &env, &contract)?;
        for schema in std::iter::once(&contract.normal).chain(contract.unwind.iter())
            .chain(contract.cancellation.iter().map(|cancel| &cancel.outcome)) {
            let accounted: BTreeSet<_> = schema.transfers.iter().flat_map(transfer_inputs).collect();
            if !consumed.is_subset(&accounted) {
                return Err(flow_error("contract does not account for every consumed linear input"));
            }
        }
        if let Some(cancel) = &contract.cancellation
            && contract.unwind.as_ref().is_none_or(|unwind| unwind.results != cancel.outcome.results) {
            return Err(flow_error("cancel/unwind positional schemas require distinct transport"));
        }
        let scope = body.blocks.get(source).map_err(arena)?.lexical_scope;
        if let Some(unwind_schema) = &contract.unwind {
            let (normal, normal_edge) = self.landing(module, body, scope, &env, &consumed,
                &contract.normal, Outcome::Normal, &op, result_deps)?;
            let (unwind, unwind_edge) = self.landing(module, body, scope, &env, &consumed,
                unwind_schema, Outcome::Unwind, &op, result_deps)?;
            let cancel = if let Some(cancel) = &contract.cancellation {
                let mut landing = unwind.clone();
                landing.env = self.outcome_env(module, body, &env, &consumed, &cancel.outcome,
                    &landing.results, &landing.forwarded)?;
                Some(landing)
            } else {
                None
            };
            body.blocks.get_mut(source).map_err(arena)?.term.kind = TermKind::InvokeOwn {
                op: Box::new(op.clone()), normal: normal_edge, unwind: unwind_edge,
            };
            if matches!(op, Operation::Ownership(OwnershipOp::ScopeSplit { .. }
                | OwnershipOp::SplitChecked { .. })) {
                self.budget.stats.emitted_checked_splits += 1;
            }
            Ok(Emission { normal, unwind: Some(unwind), cancel })
        } else {
            let specs = self.specs(module, body, &contract.normal)?;
            self.charge(1 + specs.len(), 128 + specs.len() * 128)?;
            let inst = body.insts.alloc(Instruction {
                results: vec![], op: op.clone(), facts: InstructionFacts::default(), loc: None,
            }).map_err(arena)?;
            let mut results = vec![];
            for (index, (slot, ty)) in contract.normal.results.iter().zip(specs).enumerate() {
                let index = u16::try_from(index).map_err(|_| flow_error("too many outcome slots"))?;
                let value = body.values.alloc(ValueData {
                    ty, facts: plain_or_service(module, ty)?, def: Definition::InstResult { inst, index },
                }).map_err(arena)?;
                results.push((slot.name, value));
            }
            self.result_facts(module, body, &op, &results, result_deps)?;
            body.insts.get_mut(inst).map_err(arena)?.results = results.iter().map(|(_, value)| *value).collect();
            body.blocks.get_mut(source).map_err(arena)?.insts.push(inst);
            let env = self.outcome_env(module, body, &env, &consumed, &contract.normal, &results, &BTreeMap::new())?;
            Ok(Emission {
                normal: Landing { block: source, env, results, forwarded: BTreeMap::new() }, unwind: None, cancel: None,
            })
        }
    }
    fn consumed(
        &mut self,
        module: &Module,
        body: &Body,
        env: &LiveEnv,
        contract: &OpContract,
    ) -> Result<BTreeSet<ValueId>, OwnError> {
        let live = env.all_linear();
        if live.len() != env.linear.len() + env.scopes.len() {
            return Err(flow_error("live environment aliases a linear slot"));
        }
        let mut consumed = BTreeSet::new();
        for operand in &contract.operands {
            let mut pending = vec![operand.reference];
            let mut seen = BTreeSet::new();
            while let Some(reference) = pending.pop() {
                self.charge(1, 0)?;
                if let OperandRef::Value(value) = reference {
                    if linear(module, body, value)? {
                        if !live.contains(&value) {
                            return Err(flow_error("stale or unavailable linear input"));
                        }
                        if operand.mode == OperandMode::Consume && !consumed.insert(value) {
                            return Err(flow_error("duplicate linear consumption"));
                        }
                    }
                } else if seen.insert(reference) {
                    pending.extend(cfg::expression_children(reference, body)
                        .map_err(|error| flow_error(format!("operand expression: {error}")))?);
                }
            }
        }
        Ok(consumed)
    }
    fn specs(
        &mut self,
        module: &mut Module,
        body: &Body,
        schema: &OutcomeContract,
    ) -> Result<Vec<TypeId>, OwnError> {
        schema.results.iter().map(|slot| resolve_result_type(module, body, &slot.ty)
            .map_err(|error| flow_error(format!("outcome type: {error}")))).collect()
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn landing(
        &mut self,
        module: &mut Module,
        body: &mut Body,
        scope: ScopeId,
        env: &LiveEnv,
        consumed: &BTreeSet<ValueId>,
        schema: &OutcomeContract,
        outcome: Outcome,
        op: &Operation,
        result_deps: &[(SlotName, DepExprId)],
    ) -> Result<(Landing, InvokeEdge), OwnError> {
        let filtered;
        let env = if outcome == Outcome::Unwind && env.unwind_values.is_some() {
            let mut next = env.clone();
            // A live buffer keeps its High name: it is abandoned on this path.
            next.values.retain(|value, _| {
                next.unwind_values.as_ref().unwrap().contains(value) || next.journals.contains_key(value)
            });
            if let Operation::Ownership(OwnershipOp::ScopeEnd { state, .. }) = op {
                let ty = body.values.get(*state).map_err(arena)?.ty;
                if let Type::Service(ServiceType::Scope(owner)) = module.types.get(ty).map_err(arena)? {
                    next.bindings.retain(|binding, _| body.bindings.get(*binding)
                        .is_ok_and(|binding| binding.scope != *owner));
                }
                next.cleanup.wanted = None;
                next.cleanup.carried.clear();
                next.cleanup.replacement = None;
            }
            filtered = next;
            &filtered
        } else {
            env
        };
        let target = block(body, scope)?;
        let mut results = vec![];
        let mut args = vec![];
        for (index, (slot, ty)) in schema.results.iter().zip(self.specs(module, body, schema)?).enumerate() {
            let value = parameter(body, target, ty, plain_or_service(module, ty)?)?;
            results.push((slot.name, value));
            let slot = u16::try_from(index).map_err(|_| flow_error("too many outcome slots"))?;
            args.push(EdgeArg::Produced(EdgeResult { outcome, slot }));
        }
        let mut carry: BTreeSet<_> = env.all_linear().difference(consumed).copied().collect();
        for latest in env.values.values().chain(env.bindings.values()).chain(env.cleanup.replacement.iter()) {
            if consumed.contains(&latest.value) {
                return Err(flow_error("consumed linear value used as a copyable binding"));
            }
            carry.insert(latest.value);
            carry.extend(latest.deps);
        }
        carry.extend(env.cleanup.failure);
        carry.extend(env.cleanup.primary_before_failure);
        if let Some(pending) = env.cleanup.pending
            && !consumed.contains(&pending) {
            carry.insert(pending);
        }
        let mut forwarded = BTreeMap::new();
        for value in &carry {
            let data = body.values.get(*value).map_err(arena)?.clone();
            let next = parameter(body, target, data.ty, data.facts)?;
            forwarded.insert(*value, next);
            args.push(EdgeArg::Value(*value));
        }
        for latest in env.values.values().chain(env.bindings.values()).chain(env.cleanup.replacement.iter()) {
            if let Some(deps) = latest.deps {
                let value = forwarded[&latest.value];
                let expression = body.deps.alloc(DepExpr::FromSsa(forwarded[&deps])).map_err(arena)?;
                body.values.get_mut(value).map_err(arena)?.facts = ValueFacts::Managed { deps: expression };
            }
        }
        self.result_facts(module, body, op, &results, result_deps)?;
        let env = self.outcome_env(module, body, env, consumed, schema, &results, &forwarded)?;
        self.charge(1 + args.len(), 256 + args.len() * 192)?;
        Ok((Landing { block: target, env, results, forwarded }, InvokeEdge { block: target, args }))
    }
    fn result_facts(
        &mut self,
        module: &Module,
        body: &mut Body,
        op: &Operation,
        results: &[(SlotName, ValueId)],
        result_deps: &[(SlotName, DepExprId)],
    ) -> Result<(), OwnError> {
        let deps_ssa = results.iter().find(|(slot, _)| *slot == SlotName::Deps).map(|(_, value)| *value);
        for (slot, value) in results {
            let ty = body.values.get(*value).map_err(arena)?.ty;
            if !self.managed.contains(&ty)
                || matches!(module.types.get(ty).map_err(arena)?, Type::Service(_) | Type::Descriptor
                    | Type::Builder(_) | Type::MapBuilder(..) | Type::Frame(_)
                    | Type::Int | Type::Bool | Type::Byte | Type::Float | Type::Rat | Type::Char | Type::Str
                    | Type::Range | Type::Enum(_) | Type::Never) {
                continue;
            }
            let deps = if let Some((_, deps)) = result_deps.iter().find(|(name, _)| name == slot) {
                body.deps.get(*deps).map_err(arena)?;
                *deps
            } else if let Some(deps) = deps_ssa {
                body.deps.alloc(DepExpr::FromSsa(deps)).map_err(arena)?
            } else if *slot == SlotName::Value && matches!(op, Operation::Core(CoreOp::ObjNext { .. })) {
                // The substituted ObjectFields contract supplies exactly the
                // final fields and the new identity, excluding the old self.
                let selection = body.selections.alloc(SelectionExpr::Value(*value)).map_err(arena)?;
                body.deps.alloc(DepExpr::Retention { value: *value, selection }).map_err(arena)?
            } else if *slot == SlotName::Value && matches!(op, Operation::Core(CoreOp::ObjNew { .. })) {
                let site = CreationSiteId::from_u32(value.as_u32());
                let key = body.keys.alloc(KeyExpr::Created { site, instance: *value }).map_err(arena)?;
                let key_deps = body.deps.alloc(DepExpr::Key(key)).map_err(arena)?;
                let mut deps = vec![key_deps];
                let inputs = match op {
                    Operation::Core(CoreOp::ObjNew { fields, .. }) => fields.clone(),
                    _ => unreachable!(),
                };
                for input in inputs {
                    if let ValueFacts::Managed { deps: input } = body.values.get(input).map_err(arena)?.facts {
                        deps.push(input);
                    }
                }
                body.deps.alloc(DepExpr::Union(deps)).map_err(arena)?
            } else if let Operation::Core(CoreOp::FnClosure { captures, .. }) = op {
                let deps = captures.iter().filter_map(|value| match body.values.get(*value).ok()?.facts {
                    ValueFacts::Managed { deps } => Some(deps),
                    _ => None,
                }).collect();
                body.deps.alloc(DepExpr::Union(deps)).map_err(arena)?
            } else if let Operation::Core(CoreOp::FnBind { receiver: input, .. }
                | CoreOp::FnBindRole { receiver: input, .. } | CoreOp::CapMake { owner: input, .. }
                | CoreOp::AnyBox { value: input, .. } | CoreOp::RoleWrap { value: input, .. }
                | CoreOp::RoleNarrow { value: input, .. }) = op {
                match body.values.get(*input).map_err(arena)?.facts {
                    ValueFacts::Managed { deps } => deps,
                    _ => body.deps.alloc(DepExpr::Empty).map_err(arena)?,
                }
            } else if *slot == SlotName::Value && matches!(op, Operation::Ownership(OwnershipOp::Empty {})) {
                // A caught payload (`catch` landing) retains what its failure carried.
                let selection = body.selections.alloc(SelectionExpr::Value(*value)).map_err(arena)?;
                body.deps.alloc(DepExpr::Retention { value: *value, selection }).map_err(arena)?
            } else if matches!(op, Operation::Call(_) | Operation::Collection(_)
                | Operation::Core(CoreOp::ObjGet { .. } | CoreOp::OnceLoad { .. } | CoreOp::Undef { .. }
                    | CoreOp::VariantNew { .. } | CoreOp::VariantOf { .. } | CoreOp::VariantGet { .. }
                    | CoreOp::Retag { .. })) {
                // Collections, variants and projections retain exactly what their
                // contract's retention formula selects from the operands.
                let expression = if matches!(op, Operation::Core(CoreOp::Undef { .. })) {
                    DepExpr::Empty
                } else {
                    let selection = body.selections.alloc(SelectionExpr::Value(*value)).map_err(arena)?;
                    DepExpr::Retention { value: *value, selection }
                };
                body.deps.alloc(expression).map_err(arena)?
            } else {
                return Err(flow_error("managed result without a complete deps recipe"));
            };
            body.values.get_mut(*value).map_err(arena)?.facts = ValueFacts::Managed { deps };
        }
        let _ = module;
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    fn outcome_env(
        &mut self,
        module: &Module,
        body: &Body,
        before: &LiveEnv,
        consumed: &BTreeSet<ValueId>,
        schema: &OutcomeContract,
        results: &[(SlotName, ValueId)],
        forwarded: &BTreeMap<ValueId, ValueId>,
    ) -> Result<LiveEnv, OwnError> {
        let remap = |value: ValueId| forwarded.get(&value).copied().unwrap_or(value);
        let mut env = before.clone();
        env.linear = env.linear.difference(consumed).copied().map(remap).collect();
        env.scopes.retain(|_, value| !consumed.contains(value));
        for state in env.scopes.values_mut() {
            *state = remap(*state);
        }
        for latest in env.values.values_mut().chain(env.bindings.values_mut())
            .chain(env.cleanup.replacement.iter_mut()) {
            latest.value = remap(latest.value);
            latest.deps = latest.deps.map(remap);
        }
        env.cleanup.failure = env.cleanup.failure.map(remap);
        env.cleanup.primary_before_failure = env.cleanup.primary_before_failure.map(remap);
        env.cleanup.pending = env.cleanup.pending.filter(|value| !consumed.contains(value)).map(remap);
        env.restore_to = env.restore_to.into_iter().filter(|(value, _)| !consumed.contains(value))
            .map(|(value, owner)| (remap(value), owner)).collect();
        env.journals = env.journals.into_iter().filter(|(_, packet)| !consumed.contains(packet))
            .map(|(buffer, packet)| (buffer, remap(packet))).collect();
        for (name, value) in results {
            match module.types.get(body.values.get(*value).map_err(arena)?.ty).map_err(arena)? {
                Type::Service(ServiceType::Pending) => {
                    env.linear.insert(*value);
                    env.cleanup.pending = Some(*value);
                }
                Type::Service(ServiceType::Scope(scope)) => {
                    if env.scopes.insert(*scope, *value).is_some() {
                        return Err(flow_error("outcome duplicates a scope state"));
                    }
                }
                _ if linear(module, body, *value)? => {
                    env.linear.insert(*value);
                }
                _ => {}
            }
            if *name == SlotName::Failure {
                if let Some(primary) = env.cleanup.failure {
                    env.cleanup.primary_before_failure = Some(primary);
                }
                env.cleanup.failure = Some(*value);
            }
        }
        for postcondition in &schema.postconditions {
            if let Postcondition::Closed(input) = postcondition
                && env.all_linear().contains(input) {
                return Err(flow_error("closed input remains live"));
            }
        }
        Ok(env)
    }
}

pub(super) fn linear(module: &Module, body: &Body, value: ValueId) -> Result<bool, OwnError> {
    let ty = module.types.get(body.values.get(value).map_err(arena)?.ty).map_err(arena)?;
    Ok(matches!(ty, Type::Builder(_) | Type::MapBuilder(..) | Type::Frame(_))
        || matches!(ty, Type::Service(service) if service.is_linear()))
}

fn transfer_inputs(transfer: &TransferRule) -> Vec<ValueId> {
    match transfer {
        TransferRule::Preserve { input, .. } | TransferRule::Partition { input, .. }
        | TransferRule::Close { input } => vec![*input],
        TransferRule::Move { inputs, .. } | TransferRule::Settle { inputs }
        | TransferRule::Protocol { inputs, .. } => inputs.clone(),
        TransferRule::Admit { state, packet, .. } => vec![*state, *packet],
        TransferRule::ScopeTransfer { states, .. } => states.values(),
        TransferRule::Publish { input, .. } => vec![*input],
        TransferRule::Extract { capability, .. } => vec![*capability],
        TransferRule::Reserve { packet, .. } => vec![*packet],
        TransferRule::CloseReserve { scope, reserve, .. } => vec![*scope, *reserve],
        TransferRule::ExtendReservation { pending, packet } => vec![*pending, *packet],
        TransferRule::ReplaceReservation { pending, packet } => std::iter::once(*pending).chain(*packet).collect(),
        TransferRule::AbortReservation { pending, cleanup, .. } => vec![*pending, *cleanup],
        TransferRule::FrameSave { frame, values } => std::iter::once(*frame).chain(values.iter().copied()).collect(),
        TransferRule::FrameResume { frame, .. } => vec![*frame],
        TransferRule::CompleteIdentity { state, .. } => vec![*state],
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmissionComponent {
    Admission,
    Bound,
    /// A constructor stage can select a mixture of local admission and bound
    /// history. Restoration preserves each part's existing component.
    Preserve,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScopeExtraction {
    pub owner: ScopeId,
    pub selection: SelectionId,
    /// Proven by analysis for this selection, never inferred from packet size.
    pub component: AdmissionComponent,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExtractedPacket {
    pub packet: ValueId,
    pub owner: ScopeId,
    pub component: AdmissionComponent,
}
#[derive(Clone, Debug)]
pub struct ExtractionFailure {
    pub index: usize,
    pub split: Landing,
    pub restored: Landing,
}
#[derive(Clone, Debug)]
pub struct ExtractionBatch {
    pub normal: Landing,
    pub packets: Vec<ExtractedPacket>,
    pub failures: Vec<ExtractionFailure>,
}
impl OutcomeEmitter<'_> {
    /// Build a packet from ordered checked extractions. A later failure restores
    /// every earlier extraction to its original component before lexical unwind.
    pub fn extract_scopes(
        &mut self,
        module: &mut Module,
        body: &mut Body,
        mut point: Landing,
        requests: &[ScopeExtraction],
    ) -> Result<ExtractionBatch, OwnError> {
        let mut packets: Vec<ExtractedPacket> = vec![];
        let mut failures = vec![];
        for (index, request) in requests.iter().enumerate() {
            let state = point.env.scopes.get(&request.owner).copied()
                .ok_or_else(|| flow_error("extraction owner has no live scope state"))?;
            let emitted = self.emit(module, body, point.block, point.env,
                Operation::Ownership(OwnershipOp::ScopeSplit { state, selection: request.selection }))?;
            let split = emitted.unwind.ok_or_else(|| flow_error("checked extraction has no unwind"))?;
            let mut restored = split.clone();
            for earlier in packets.iter().rev() {
                let packet = split.forwarded.get(&earlier.packet).copied()
                    .ok_or_else(|| flow_error("exceptional extraction omits an earlier packet"))?;
                let state = restored.env.scopes[&earlier.owner];
                let op = match earlier.component {
                    AdmissionComponent::Admission | AdmissionComponent::Preserve => {
                        OwnershipOp::Admit { state, packet }
                    }
                    AdmissionComponent::Bound => OwnershipOp::Accept { state, packet },
                };
                restored = self.emit(module, body, restored.block, restored.env,
                    Operation::Ownership(op))?.normal;
            }
            failures.push(ExtractionFailure { index, split, restored });
            point = emitted.normal;
            for earlier in &mut packets {
                earlier.packet = point.forwarded.get(&earlier.packet).copied()
                    .ok_or_else(|| flow_error("normal extraction omits an earlier packet"))?;
            }
            packets.push(ExtractedPacket {
                packet: point.result(SlotName::Selected)?, owner: request.owner, component: request.component,
            });
        }
        Ok(ExtractionBatch { normal: point, packets, failures })
    }
}
