//! Deliberately partial driver for commit-2 fixtures. This is not the commit-3
//! function elaborator: unsupported intents and managed escapes are diagnosed.
use super::*;
use super::analyze::analyze_function;
use super::bindings::{block, parameter, plain_or_service, LatestValue, SealedSsa, SsaSlot};
use super::edges::{stack, Graph};
use super::emit::{Landing, LiveEnv, OutcomeEmitter};
use crate::visit::{Rewrite, VisitorMut};
use std::collections::BTreeSet;

pub fn rewrite_subset(mut output: OwnOutput, options: &OwnOptions) -> Result<OwnOutput, OwnError> {
    if output.module.profile != Profile::High {
        return Err(flow_error("partial driver requires High"));
    }
    let functions: Vec<_> = output.module.functions.iter().filter_map(|(id, function)| {
        function.body.is_some().then_some(id)
    }).collect();
    let mut emitter = OutcomeEmitter::new(&output.analysis, options);
    emitter.charge(output.stats.charged_work, output.stats.peak_auxiliary_bytes)?;
    let mut flows = BTreeMap::new();
    for function in &functions {
        let flow = analyze_function(&output.module, *function, &output.analysis, options)?;
        emitter.charge(flow.stats.charged_work, flow.stats.peak_auxiliary_bytes)?;
        flows.insert(*function, flow);
    }
    output.module.profile = Profile::Own;
    for function in functions {
        let high = output.module.functions.get_mut(function).map_err(arena)?.body.take().unwrap();
        let graph = Graph::index(&output.module, &high, emitter.budget_ref())?;
        let root = high.blocks.get(high.entry).map_err(arena)?.lexical_scope;
        let retained: BTreeSet<_> = output.analysis.scopes.iter().filter_map(|(scope, facts)| {
            (!facts.empty_scope_eligible).then_some(*scope)
        }).collect();
        for (scope, declaration) in output.module.scopes.iter() {
            if declaration.function == function && !retained.contains(&scope) {
                output.stats.elided_lexical_scopes += 1;
            }
        }
        let mut driver = Driver {
            module: &mut output.module, high: &high, analysis: &output.analysis, emitter: &mut emitter,
            body: Body {
                high_results: vec![], high_effects: vec![], descriptor_bindings: vec![], service_schedules: vec![],
                entry: BlockId::from_u32(0), values: Arena::new(), blocks: Arena::new(), insts: Arena::new(),
                bindings: high.bindings.clone(), boundaries: high.boundaries.clone(), deps: Arena::new(),
                keys: Arena::new(), selections: Arena::new(),
            },
            retained, blocks: BTreeMap::new(), entries: BTreeMap::new(), ssa: SealedSsa::default(),
            failures: vec![], current_unwind: None, predecessors: BTreeMap::new(), ordinal: 0,
        };
        if !high.high_results.is_empty() || !high.descriptor_bindings.is_empty() || !high.service_schedules.is_empty() {
            return Err(commit3("result, descriptor or service metadata"));
        }
        let liveness = live_in(&high, &graph, driver.emitter.budget_ref())?;
        for original in &graph.rpo {
            let data = high.blocks.get(*original).map_err(arena)?;
            let next = block(&mut driver.body, data.lexical_scope)?;
            driver.blocks.insert(*original, next);
            let mut env = LiveEnv::default();
            let values: BTreeSet<_> = liveness[original].iter().chain(&data.params).copied().collect();
            for value in values {
                let ty = high.values.get(value).map_err(arena)?.ty;
                let managed = driver.managed(ty);
                let latest = driver.ssa.declare(driver.module, &mut driver.body, next,
                    SsaSlot::Value(value), ty, managed)?;
                env.values.insert(value, latest);
            }
            if !matches!(data.term.kind, TermKind::Raise(_))
                && let Some(flow) = flows[&function].entries.get(original) {
                for binding in flow.latest.keys() {
                    let ty = high.bindings.get(*binding).map_err(arena)?.ty;
                    let managed = driver.managed(ty);
                    let latest = driver.ssa.declare(driver.module, &mut driver.body, next,
                        SsaSlot::Binding(*binding), ty, managed)?;
                    env.bindings.insert(*binding, latest);
                }
            }
            for scope in stack(driver.module, data.lexical_scope)? {
                if driver.retained.contains(&scope) {
                    let ty = driver.module.types.intern(Type::Service(ServiceType::Scope(scope))).map_err(arena)?;
                    let state = driver.ssa.declare(driver.module, &mut driver.body, next,
                        SsaSlot::Scope(scope), ty, false)?.value;
                    env.scopes.insert(scope, state);
                }
            }
            driver.entries.insert(*original, env);
        }
        let prologue = block(&mut driver.body, root)?;
        driver.body.entry = prologue;
        let mut env = LiveEnv::default();
        let destroy = driver.module.functions.get(function).map_err(arena)?.attrs.iter()
            .any(|attr| matches!(attr, FunctionAttr::Destroy(_)));
        let mut params = vec![];
        for original in &high.blocks.get(high.entry).map_err(arena)?.params {
            let ty = high.values.get(*original).map_err(arena)?.ty;
            let value = parameter(&mut driver.body, prologue, ty, plain_or_service(driver.module, ty)?)?;
            params.push(ty);
            env.values.insert(*original, LatestValue { value, deps: None });
        }
        for latest in env.values.values_mut() {
            let ty = driver.body.values.get(latest.value).map_err(arena)?.ty;
            if driver.analysis.types.get(&ty).is_some_and(TypeClassification::may_own) {
                if destroy {
                    let selection = driver.body.selections.alloc(SelectionExpr::Value(latest.value)).map_err(arena)?;
                    let deps = driver.body.deps.alloc(DepExpr::Retention {
                        value: latest.value, selection,
                    }).map_err(arena)?;
                    driver.body.values.get_mut(latest.value).map_err(arena)?.facts = ValueFacts::Managed { deps };
                    continue;
                }
                let deps_ty = driver.module.types.intern(Type::Service(ServiceType::Deps)).map_err(arena)?;
                let deps = parameter(&mut driver.body, prologue, deps_ty, ValueFacts::Service)?;
                let expression = driver.body.deps.alloc(DepExpr::FromSsa(deps)).map_err(arena)?;
                driver.body.values.get_mut(latest.value).map_err(arena)?.facts = ValueFacts::Managed {
                    deps: expression,
                };
                latest.deps = Some(deps);
                params.push(deps_ty);
            }
        }
        if destroy {
            let originals: Vec<_> = env.values.iter().filter_map(|(original, latest)| {
                driver.managed(driver.body.values.get(latest.value).unwrap().ty).then_some(*original)
            }).collect();
            for original in originals {
                let value = env.values[&original].value;
                let emitted = driver.emitter.emit(driver.module, &mut driver.body, prologue, env,
                    Operation::Ownership(OwnershipOp::Dependencies { value }))?.normal;
                let deps = emitted.result(SlotName::Value)?;
                env = emitted.env;
                env.values.get_mut(&original).unwrap().deps = Some(deps);
            }
        }
        let explicit = driver.retained.contains(&root);
        let mut signature = driver.module.functions.get(function).map_err(arena)?.signature.clone();
        let mut point = Landing { block: prologue, env, results: vec![], forwarded: BTreeMap::new() };
        if explicit {
            let own = driver.module.types.intern(Type::Service(ServiceType::Own)).map_err(arena)?;
            let input_packet = u16::try_from(params.len()).map_err(|_| flow_error("ABI too large"))?;
            let output_packet = u16::try_from(signature.results.len()).map_err(|_| flow_error("ABI too large"))?;
            let packet = parameter(&mut driver.body, prologue, own, ValueFacts::Service)?;
            params.push(own);
            signature.results.push(own);
            signature.ownership_abi = OwnershipAbi::Explicit { input_packet, output_packet };
            point.env.linear.insert(packet);
            point = driver.emit(point, Operation::Ownership(OwnershipOp::ScopeBegin { scope: root }))?;
            let state = point.env.scopes[&root];
            point = driver.emit(point, Operation::Ownership(OwnershipOp::Accept { state, packet }))?;
        } else {
            signature.ownership_abi = OwnershipAbi::Observation;
            let summary = driver.module.functions.get(function).map_err(arena)?.summary
                .ok_or_else(|| flow_error("partial driver requires inferred summaries"))?;
            let mut specialized = driver.module.summaries.get(summary).map_err(arena)?.clone();
            specialized.entry_kind = EntryKind::ObservationAdmissionStaysCaller;
            let summary = driver.module.summaries.alloc(specialized).map_err(arena)?;
            driver.module.functions.get_mut(function).map_err(arena)?.summary = Some(summary);
        }
        signature.params = params;
        driver.module.functions.get_mut(function).map_err(arena)?.signature = signature;
        driver.connect(point, &Target { block: high.entry, args: high.blocks.get(high.entry)
            .map_err(arena)?.params.clone() }, true, false)?;
        for original in &graph.rpo {
            let data = high.blocks.get(*original).map_err(arena)?;
            driver.current_unwind = data.unwind.as_ref().map(|unwind| unwind.target.clone());
            let mut point = Landing {
                block: driver.blocks[original], env: driver.entries[original].clone(),
                results: vec![], forwarded: BTreeMap::new(),
            };
            for instruction in &data.insts {
                let instruction = high.insts.get(*instruction).map_err(arena)?;
                point = driver.instruction(point, instruction)?;
            }
            match &data.term.kind {
                TermKind::Br(target) => driver.connect(point, target, false, false)?,
                TermKind::Cond { condition, yes, no } => {
                    let condition = point.env.value(*condition)?.value;
                    let left = block(&mut driver.body, data.lexical_scope)?;
                    let right = block(&mut driver.body, data.lexical_scope)?;
                    driver.body.blocks.get_mut(point.block).map_err(arena)?.term.kind = TermKind::Cond {
                        condition, yes: Target { block: left, args: vec![] }, no: Target { block: right, args: vec![] },
                    };
                    let mut left_point = point.clone();
                    left_point.block = left;
                    let mut right_point = point;
                    right_point.block = right;
                    driver.connect(left_point, yes, false, false)?;
                    driver.connect(right_point, no, false, false)?;
                }
                TermKind::Ret { values } => {
                    if values.iter().any(|value| driver.managed(high.values.get(*value).unwrap().ty)) {
                        return Err(commit3("managed return/reservation"));
                    }
                    point = driver.close_all(point, false)?;
                    let mut values = values.iter().map(|value| point.env.value(*value).map(|latest| latest.value))
                        .collect::<Result<Vec<_>, _>>()?;
                    if explicit {
                        point = driver.emit(point, Operation::Ownership(OwnershipOp::Empty {}))?;
                        values.push(point.result(SlotName::Packet)?);
                    }
                    driver.body.blocks.get_mut(point.block).map_err(arena)?.term.kind = TermKind::Ret { values };
                }
                TermKind::Raise(failure) => {
                    point.env.cleanup.failure = Some(point.env.value(*failure)?.value);
                    driver.failures.push(FailureRoute { point, target: None });
                }
                TermKind::Unreachable => {
                    if !point.env.all_linear().is_empty() {
                        return Err(commit3("unreachable with live ownership"));
                    }
                }
                _ => return Err(commit3("terminator")),
            }
        }
        // Iterative cleanup queue: failed closes never recursively emit cleanup.
        driver.current_unwind = None;
        while let Some(FailureRoute { mut point, target }) = driver.failures.pop() {
            // This subset restores only empty constructor input packets. Their
            // admission component is known from construction, not nonemptiness.
            let packets: Vec<_> = point.env.linear.iter().copied().collect();
            for packet in packets {
                let owner = stack(driver.module, driver.body.blocks.get(point.block).map_err(arena)?.lexical_scope)?
                    .into_iter().rev().find(|scope| point.env.scopes.contains_key(scope))
                    .ok_or_else(|| commit3("packet restoration without a retained scope"))?;
                let state = point.env.scopes[&owner];
                point = driver.emit(point, Operation::Ownership(OwnershipOp::Admit { state, packet }))?;
            }
            if let Some(mut target) = target {
                let parameter = *high.blocks.get(target.block).map_err(arena)?.params.last()
                    .ok_or_else(|| flow_error("unwind handler has no failure parameter"))?;
                let failure = point.env.cleanup.failure.ok_or_else(|| flow_error("missing outcome failure"))?;
                point.env.values.insert(parameter, LatestValue { value: failure, deps: None });
                target.args.push(parameter);
                driver.connect(point, &target, false, true)?;
                continue;
            }
            let point = driver.close_all(point, true)?;
            let failure = point.env.cleanup.failure.ok_or_else(|| flow_error("unwind omits failure"))?;
            driver.body.blocks.get_mut(point.block).map_err(arena)?.term.kind = TermKind::Raise(failure);
        }
        for block in driver.blocks.values() {
            driver.ssa.seal(*block, driver.predecessors.get(block).copied().unwrap_or(0))?;
        }
        driver.module.functions.get_mut(function).map_err(arena)?.body = Some(driver.body);
    }
    output.stats.emitted_checked_splits += emitter.stats().emitted_checked_splits;
    output.stats.charged_work = emitter.stats().charged_work;
    output.stats.peak_auxiliary_bytes = emitter.stats().peak_auxiliary_bytes;
    Ok(output)
}

struct FailureRoute {
    point: Landing,
    target: Option<Target>,
}

struct Driver<'a, 'o> {
    module: &'a mut Module,
    high: &'a Body,
    analysis: &'a OwnAnalysis,
    emitter: &'a mut OutcomeEmitter<'o>,
    body: Body,
    retained: BTreeSet<ScopeId>,
    blocks: BTreeMap<BlockId, BlockId>,
    entries: BTreeMap<BlockId, LiveEnv>,
    ssa: SealedSsa,
    failures: Vec<FailureRoute>,
    current_unwind: Option<Target>,
    predecessors: BTreeMap<BlockId, usize>,
    ordinal: usize,
}
impl Driver<'_, '_> {
    fn managed(&self, ty: TypeId) -> bool {
        self.analysis.types.get(&ty).is_some_and(TypeClassification::may_own)
    }
    fn emit(&mut self, point: Landing, op: Operation) -> Result<Landing, OwnError> {
        let emission = self.emitter.emit(self.module, &mut self.body, point.block, point.env, op)?;
        if let Some(unwind) = emission.unwind {
            let target = self.current_unwind.clone().filter(|target| {
                let lexical = self.high.blocks.get(target.block).unwrap().lexical_scope;
                !self.retained.contains(&lexical) || unwind.env.scopes.contains_key(&lexical)
            });
            self.failures.push(FailureRoute { point: unwind, target });
        }
        Ok(emission.normal)
    }
    fn instruction(&mut self, mut point: Landing, instruction: &Instruction) -> Result<Landing, OwnError> {
        let mut op = instruction.op.clone();
        let mut missing = false;
        op.rewrite(&mut Rename { env: &point.env, missing: &mut missing });
        if missing {
            return Err(flow_error("input operand has no reaching SSA definition"));
        }
        match op {
            Operation::Lexical(LexicalOp::Bind { binding, value }
                | LexicalOp::Rebind { binding, value, .. }) => {
                let original = match instruction.op {
                    Operation::Lexical(LexicalOp::Bind { value, .. } | LexicalOp::Rebind { value, .. }) => value,
                    _ => unreachable!(),
                };
                let latest = point.env.value(original)?;
                if self.managed(self.body.values.get(value).map_err(arena)?.ty) {
                    let owner = self.high.bindings.get(binding).map_err(arena)?.scope;
                    let current = self.body.blocks.get(point.block).map_err(arena)?.lexical_scope;
                    let state = point.env.scopes[&current];
                    let selection = self.body.selections.alloc(SelectionExpr::Value(value)).map_err(arena)?;
                    let selection = self.body.selections.alloc(SelectionExpr::Admissible(selection)).map_err(arena)?;
                    point = self.emit(point, Operation::Ownership(OwnershipOp::ScopeSplit { state, selection }))?;
                    let resource = point.env.value(original)?.value;
                    let packet = point.result(SlotName::Selected)?;
                    let binding = ElaboratedBinding {
                        binding, resource: ResourceValue { value: resource, packet },
                        state: point.env.scopes[&owner], original_scope: owner,
                    };
                    let op = if matches!(instruction.op, Operation::Lexical(LexicalOp::Rebind { .. })) {
                        OwnershipOp::Rebind { binding }
                    } else {
                        OwnershipOp::Bind { binding }
                    };
                    point = self.emit(point, Operation::Ownership(op))?;
                }
                let latest = point.env.values.get(&original).copied().unwrap_or(latest);
                point.env.bindings.insert(binding, latest);
                return Ok(point);
            }
            Operation::Lexical(LexicalOp::Temp { .. }) => return Ok(point),
            Operation::Core(CoreOp::ObjNew { ref fields, .. }) if !fields.is_empty() => {
                return Err(commit3("constructor fields"));
            }
            Operation::Core(CoreOp::ObjNew { ref mut packet, .. }) => {
                if instruction.results.iter().any(|value| self.managed(self.high.values.get(*value).unwrap().ty)) {
                    point = self.emit(point, Operation::Ownership(OwnershipOp::Empty {}))?;
                    *packet = Some(point.result(SlotName::Packet)?);
                }
            }
            Operation::Const(_) => {}
            Operation::Core(_) if instruction.results.iter().all(|value| {
                !self.managed(self.high.values.get(*value).unwrap().ty)
            }) && op.operand_refs().iter().all(|operand| match operand.reference {
                OperandRef::Value(value) => !self.managed(self.body.values.get(value).unwrap().ty),
                _ => false,
            }) => {}
            _ => return Err(commit3(op.opcode())),
        }
        point = self.emit(point, op)?;
        let results = point.results.clone();
        for (original, (_, value)) in instruction.results.iter().zip(&results) {
            let mut latest = LatestValue { value: *value, deps: None };
            if self.managed(self.body.values.get(*value).map_err(arena)?.ty) {
                let packet = point.result(SlotName::Packet)?;
                let scope = self.body.blocks.get(point.block).map_err(arena)?.lexical_scope;
                let state = point.env.scopes[&scope];
                point = self.emit(point, Operation::Ownership(OwnershipOp::Dependencies { value: *value }))?;
                latest.deps = Some(point.result(SlotName::Value)?);
                point = self.emit(point, Operation::Ownership(OwnershipOp::Admit { state, packet }))?;
            }
            point.env.values.insert(*original, latest);
        }
        Ok(point)
    }
    fn close_all(&mut self, mut point: Landing, unwind: bool) -> Result<Landing, OwnError> {
        let scope = self.body.blocks.get(point.block).map_err(arena)?.lexical_scope;
        let scopes = stack(self.module, scope)?;
        for scope in scopes.into_iter().rev() {
            if point.env.scopes.contains_key(&scope) {
                point = self.close_one(point, scope, unwind)?;
            }
        }
        Ok(point)
    }
    fn close_one(&mut self, point: Landing, scope: ScopeId, unwind: bool) -> Result<Landing, OwnError> {
        let state = point.env.scopes[&scope];
        let effects = self.analysis.scopes[&scope].effects.clone();
        let op = if unwind {
            OwnershipOp::ScopeUnwind {
                state, failure: point.env.cleanup.failure.ok_or_else(|| flow_error("missing active failure"))?,
                reserve: None, effects,
            }
        } else {
            OwnershipOp::ScopeEnd { state, reserve: None, effects }
        };
        let point = self.emit(point, Operation::Ownership(op))?;
        let parent = self.module.scopes.get(scope).map_err(arena)?.parent;
        if let Some(parent) = parent {
            let next = block(&mut self.body, parent)?;
            self.body.blocks.get_mut(point.block).map_err(arena)?.term.kind = TermKind::Br(Target {
                block: next, args: vec![],
            });
            Ok(Landing { block: next, ..point })
        } else {
            Ok(point)
        }
    }
    fn connect(
        &mut self,
        mut point: Landing,
        target: &Target,
        prologue: bool,
        unwind: bool,
    ) -> Result<(), OwnError> {
        let original = self.high.blocks.get(target.block).map_err(arena)?;
        let from = self.body.blocks.get(point.block).map_err(arena)?.lexical_scope;
        let plan = edges::EdgePlan::between(self.module, from, Some(original.lexical_scope))?;
        if !prologue {
            for scope in plan.close {
                if point.env.scopes.contains_key(&scope) {
                    point = self.close_one(point, scope, unwind)?;
                }
            }
            for scope in plan.begin {
                if self.retained.contains(&scope) {
                    point = self.emit(point, Operation::Ownership(OwnershipOp::ScopeBegin { scope }))?;
                }
            }
        }
        if unwind
            && let (Some(parameter), Some(failure)) = (original.params.last(), point.env.cleanup.failure) {
            point.env.values.insert(*parameter, LatestValue { value: failure, deps: None });
        }
        let mut slots: BTreeMap<_, _> = point.env.values.iter().map(|(value, latest)| (SsaSlot::Value(*value), *latest))
            .chain(point.env.bindings.iter().map(|(binding, latest)| (SsaSlot::Binding(*binding), *latest)))
            .chain(point.env.scopes.iter().map(|(scope, value)| (SsaSlot::Scope(*scope), LatestValue {
                value: *value, deps: None,
            }))).collect();
        let arguments: Vec<_> = target.args.iter().map(|value| point.env.value(*value)).collect::<Result<_, _>>()?;
        for (parameter, latest) in original.params.iter().zip(arguments) {
            slots.insert(SsaSlot::Value(*parameter), latest);
        }
        let destination = self.blocks[&target.block];
        let args = self.ssa.incoming(&self.body, destination, point.block, self.ordinal, &slots)?;
        self.ordinal += 1;
        *self.predecessors.entry(destination).or_default() += 1;
        self.body.blocks.get_mut(point.block).map_err(arena)?.term.kind = TermKind::Br(Target {
            block: destination, args,
        });
        Ok(())
    }
}
struct Rename<'a> {
    env: &'a LiveEnv,
    missing: &'a mut bool,
}
impl VisitorMut for Rename<'_> {
    fn value(&mut self, value: &mut ValueId) {
        if let Some(latest) = self.env.values.get(value) {
            *value = latest.value;
        } else {
            *self.missing = true;
        }
    }
}

fn live_in(
    body: &Body,
    graph: &Graph,
    budget: &mut Budget<'_>,
) -> Result<BTreeMap<BlockId, BTreeSet<ValueId>>, OwnError> {
    let mut live: BTreeMap<_, BTreeSet<_>> = graph.rpo.iter().map(|block| (*block, BTreeSet::new())).collect();
    loop {
        let mut changed = false;
        for block in graph.rpo.iter().rev() {
            let data = body.blocks.get(*block).map_err(arena)?;
            let mut values = BTreeSet::new();
            match &data.term.kind {
                TermKind::Cond { condition, .. } => { values.insert(*condition); }
                TermKind::Ret { values: results } => values.extend(results),
                TermKind::Raise(value) => { values.insert(*value); }
                _ => {}
            }
            for edge in graph.edges.iter().filter(|edge| edge.source == *block) {
                values.extend(&edge.args);
                values.extend(live[&edge.target].iter().filter(|value| {
                    !body.blocks.get(edge.target).unwrap().params.contains(value)
                }));
            }
            for instruction in data.insts.iter().rev() {
                let instruction = body.insts.get(*instruction).map_err(arena)?;
                for result in &instruction.results {
                    values.remove(result);
                }
                for operand in cfg::operation_uses(&instruction.op, body)
                    .map_err(|error| flow_error(format!("liveness: {error}")))? {
                    values.insert(operand.value);
                }
            }
            for parameter in &data.params {
                values.remove(parameter);
            }
            budget.work(1 + values.len())?;
            if values != live[block] {
                live.insert(*block, values);
                changed = true;
            }
        }
        if !changed {
            return Ok(live);
        }
    }
}
