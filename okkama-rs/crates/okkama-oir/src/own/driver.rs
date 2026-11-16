//! Synchronous High normalization through the contract-driven outcome emitter.
mod calls;
mod protocols;
mod exits;
mod construction;
mod control;
mod finalize;
mod defers;
mod buffers;
mod futures;
use super::*;
use super::analyze::analyze_function;
use super::bindings::{block, parameter, plain_or_service, LatestValue, SealedSsa, SsaSlot};
use super::edges::{stack, Graph};
use super::emit::{Landing, LiveEnv, OutcomeEmitter};
use crate::visit::{Rewrite, VisitorMut};
use std::collections::BTreeSet;

/// Raw synchronous rewrite. The caller must independently verify the result.
/// This entry point also preserves output for #564 diagnostic witnesses.
pub fn rewrite_subset(mut output: OwnOutput, options: &OwnOptions) -> Result<OwnOutput, OwnError> {
    if output.module.profile != Profile::High {
        return Err(flow_error("synchronous driver requires High"));
    }
    let functions: Vec<_> = output.module.functions.iter().filter_map(|(id, function)| {
        function.body.is_some().then_some(id)
    }).collect();
    let mut helpers: BTreeSet<_> = output.module.functions.iter().filter_map(|(_, function)| function.body.as_ref())
        .flat_map(|body| body.insts.iter().filter_map(|(_, instruction)| match &instruction.op {
            Operation::Lexical(LexicalOp::DeferRegister { action, .. }) => Some(action.function),
            _ => None,
        })).collect();
    helpers.extend(output.module.functions.iter().filter_map(|(id, data)| {
        data.attrs.iter().any(|attr| matches!(attr, FunctionAttr::DeferEntry { .. })).then_some(id)
    }));
    for helper in &helpers {
        let data = output.module.functions.get_mut(*helper).map_err(arena)?;
        if !data.attrs.iter().any(|attr| matches!(attr, FunctionAttr::DeferEntry { .. })) {
            data.attrs.push(FunctionAttr::DeferEntry { replacements: vec![] });
        }
    }
    let mut dictionaries = options.descriptors.clone();
    for (_, function) in output.module.functions.iter() {
        let Some(body) = &function.body else { continue };
        for (_, instruction) in body.insts.iter() {
            if let Operation::Core(CoreOp::TypeOf { ty, descriptor }) = &instruction.op {
                dictionaries.insert(*ty, descriptor.clone());
            }
        }
    }
    let mut emitter = OutcomeEmitter::new(&output.analysis, options);
    emitter.charge(output.stats.charged_work, output.stats.peak_auxiliary_bytes)?;
    let entry_sets = super::entries::entry_sets(&output.module, emitter.budget_ref())?;
    let mut flows = BTreeMap::new();
    for function in &functions {
        let flow = analyze_function(&output.module, *function, &output.analysis, options)?;
        let retained = flow.allocated_bytes() + 128;
        let peak = flow.stats.peak_auxiliary_bytes.max(retained);
        emitter.charge(flow.stats.charged_work, peak)?;
        emitter.budget_ref().release(peak - retained);
        flows.insert(*function, flow);
    }
    // Decide empty entry specializations before rewriting any caller, including
    // recursive calls and functions declared after their callers.
    for function in &functions {
        let data = output.module.functions.get(*function).map_err(arena)?;
        let body = data.body.as_ref().unwrap();
        let root = body.blocks.get(body.entry).map_err(arena)?.lexical_scope;
        let fixed_observation = fixed_root(&output.module, *function)
            || data.attrs.iter().any(|attr| matches!(attr, FunctionAttr::Destroy(_)));
        // The runtime starts an async body with its accepted input packet (async ABI C-2),
        // so only a fixed root (async main, an async test) observes.
        let asynchronous = data.attrs.contains(&FunctionAttr::Async);
        let eligible = output.analysis.scopes.get(&root)
            .ok_or_else(|| flow_error("function root scope has no analysis facts"))?.empty_scope_eligible;
        let empty = eligible && !asynchronous;
        if (empty || fixed_observation) && !helpers.contains(function) {
            let mut summary = output.module.summaries.get(data.summary.unwrap()).map_err(arena)?.clone();
            summary.entry_kind = EntryKind::ObservationAdmissionStaysCaller;
            let summary = output.module.summaries.alloc(summary).map_err(arena)?;
            let data = output.module.functions.get_mut(*function).map_err(arena)?;
            data.summary = Some(summary);
            if asynchronous {
                data.async_body_summary = Some(summary);
            }
        }
    }
    super::entries::dictionary_summaries(&mut output.module, emitter.budget_ref())?;
    let external: Vec<_> = output.module.functions.iter().filter_map(|(id, data)| {
        data.body.is_none().then_some(id)
    }).collect();
    for function in external {
        let declaration = output.module.functions.get(function).map_err(arena)?.clone();
        let summary = output.module.summaries.get(declaration.summary.unwrap()).map_err(arena)?;
        let observation = summary.entry_kind == EntryKind::ObservationAdmissionStaysCaller;
        let mut signature = declaration.signature;
        let deps = output.module.types.intern(Type::Service(ServiceType::Deps)).map_err(arena)?;
        let witness = entry_sets.witnesses.contains(&function);
        let managed = signature.params.iter().enumerate().filter(|(index, ty)| {
            (output.analysis.types.get(ty).is_some_and(TypeClassification::may_own)
                || output.analysis.managed_references.contains(ty) || witness && *index == 0)
                && !matches!(output.module.types.get(**ty), Ok(Type::Service(_) | Type::Descriptor))
        }).count();
        signature.params.extend(std::iter::repeat_n(deps, managed));
        if observation {
            signature.ownership_abi = OwnershipAbi::Observation;
        } else {
            let own = output.module.types.intern(Type::Service(ServiceType::Own)).map_err(arena)?;
            let input_packet = u16::try_from(signature.params.len()).map_err(|_| flow_error("external ABI too large"))?;
            let output_packet = u16::try_from(signature.results.len())
                .map_err(|_| flow_error("external ABI too large"))?;
            signature.params.push(own);
            signature.results.push(own);
            signature.ownership_abi = OwnershipAbi::Explicit { input_packet, output_packet };
        }
        output.module.functions.get_mut(function).map_err(arena)?.signature = signature;
    }
    output.module.profile = Profile::Own;
    for function in functions {
        let high = output.module.functions.get_mut(function).map_err(arena)?.body.take().unwrap();
        let graph = Graph::index(&output.module, &high, emitter.budget_ref())?;
        let root = high.blocks.get(high.entry).map_err(arena)?.lexical_scope;
        let mut retained: BTreeSet<_> = output.analysis.scopes.iter().filter_map(|(scope, facts)| {
            (!facts.empty_scope_eligible).then_some(*scope)
        }).collect();
        let empty_reserves = exits::empty_reserve_scopes(&output.module, &high, &graph, &flows[&function],
            &output.analysis, emitter.budget_ref())?;
        retained.retain(|scope| !empty_reserves.contains(scope));
        // An async action's final successful close delivers cancellation (D18).
        if helpers.contains(&function) || super::futures::is_async(&output.module, function) {
            retained.insert(root);
        }
        for (scope, declaration) in output.module.scopes.iter() {
            if declaration.function == function && !retained.contains(&scope) {
                output.stats.elided_lexical_scopes += 1;
            }
        }
        // Preserve the reviewed synchronous slice. Commit-4 entries additionally
        // exploit snapshot admission facts before introducing checked splits.
        let elide_offers = high.insts.iter().any(|(_, instruction)| matches!(
            routing::handling(&instruction.op), routing::Handling::Snapshot)
            || matches!(instruction.op, Operation::Call(CallOp::Closure { .. } | CallOp::Cap { .. }
                | CallOp::Role { .. } | CallOp::Order { .. })))
            || output.module.functions.get(function).map_err(arena)?.attrs.iter()
                .any(|attr| matches!(attr, FunctionAttr::Lambda { .. }));
        emitter.charge(high.values.len(), high.values.len() * 64)?;
        let origins = origins::callable_origins(&output.module, &high, emitter.budget_ref())?;
        let builder_owners = if arrays::creates_builders(&high) {
            let owners = arrays::builder_owners(&output.module, &high)?;
            emitter.charge(high.insts.len() + high.blocks.len(), owners.len() * 96)?;
            owners
        } else {
            BTreeMap::new()
        };
        let mut driver = Driver {
            module: &mut output.module, high: &high, analysis: &output.analysis, emitter: &mut emitter,
            body: Body {
                high_results: vec![], high_effects: vec![], descriptor_bindings: vec![], service_schedules: vec![],
                frame_layout: None,
                entry: BlockId::from_u32(0), values: Arena::new(), blocks: Arena::new(), insts: Arena::new(),
                bindings: high.bindings.clone(), boundaries: high.boundaries.clone(), deps: Arena::new(),
                keys: Arena::new(), selections: Arena::new(), cleanup: Default::default(),
            },
            retained, blocks: BTreeMap::new(), entries: BTreeMap::new(), ssa: SealedSsa::default(),
            failures: vec![], current_unwind: None, predecessors: BTreeMap::new(), ordinal: 0,
            live_after: BTreeSet::new(), captured: BTreeSet::new(), failure_live: BTreeMap::new(),
            current_high: high.entry, current_instruction: None, escaping: BTreeMap::new(),
            incomplete: BTreeMap::new(),
            replacement_returns: vec![],
            constructor_inputs: flows[&function].constructor_inputs.clone(),
            flow: &flows[&function], elide_offers, definitions: BTreeMap::new(), origins,
            witnesses: &entry_sets.witnesses, builder_owners, cache_duties: vec![], exit_landing: None,
            dynamic_owners: BTreeSet::new(), temporaries: BTreeSet::new(),
        };
        driver.temporaries = driver.temporary_futures();
        let captured: BTreeSet<_> = high.insts.iter().flat_map(|(_, instruction)| match &instruction.op {
            Operation::Lexical(LexicalOp::DeferRegister { action, .. }) => action.bindings.clone(),
            _ => vec![],
        }).collect();
        driver.captured = captured.clone();
        driver.incomplete = super::construction::incomplete_aliases(&high, &graph, driver.emitter.budget_ref())?;
        for edge in graph.edges.iter().filter(|edge| !edge.unwind && !edge.plan.close.is_empty()) {
            let Some(flow) = flows[&function].before_edges.get(&edge.source) else { continue };
            let wanted = edges::wanted(&high, edge)?;
            let mut values = BTreeSet::new();
            for value in &edge.args {
                if !wanted.contains(value) {
                    continue;
                }
                let deps = &flow.values[*value].deps;
                if flow.journals.iter().any(|((scope, atom), quantity)| {
                    edge.plan.close.contains(scope) && deps.contains(atom)
                        && quantity.presence != domain::Presence::Absent
                        && quantity.presence != domain::Presence::Bottom
                }) {
                    values.insert(*value);
                }
            }
            driver.escaping.insert((edge.source, edge.ordinal), values);
        }
        let liveness = live_in(&high, &graph, driver.emitter.budget_ref())?;
        driver.failure_live = liveness.clone();
        let continuations = control::continuations(&high, &graph, &liveness, driver.emitter.budget_ref())?;
        for original in &graph.rpo {
            let data = high.blocks.get(*original).map_err(arena)?;
            let next = block(&mut driver.body, data.lexical_scope)?;
            driver.blocks.insert(*original, next);
            let mut env = LiveEnv::default();
            let mut values: BTreeSet<_> = liveness[original].iter().chain(&data.params).copied().collect();
            if let Some(flow) = flows[&function].entries.get(original) {
                for prefix in flow.defers.values() {
                    for registration in prefix {
                        if let Operation::Lexical(LexicalOp::DeferRegister { action, .. }) =
                            &high.insts.get(*registration).map_err(arena)?.op {
                            values.extend(&action.captures);
                        }
                    }
                }
            }
            for value in values {
                let ty = high.values.get(value).map_err(arena)?.ty;
                let managed = driver.managed(ty);
                let latest = driver.ssa.declare(driver.module, &mut driver.body, next,
                    SsaSlot::Value(value), ty, managed)?;
                if super::emit::linear(driver.module, &driver.body, latest.value)? {
                    env.linear.insert(latest.value);
                    let own = driver.module.types.intern(Type::Service(ServiceType::Own)).map_err(arena)?;
                    let journal = driver.ssa.declare(driver.module, &mut driver.body, next,
                        SsaSlot::Journal(value), own, false)?.value;
                    env.linear.insert(journal);
                    env.journals.insert(value, journal);
                }
                env.values.insert(value, latest);
            }
            if let Some(flow) = flows[&function].entries.get(original) {
                env.cleanup.defers = flow.defers.clone();
                for binding in flow.latest.keys().filter(|binding| captured.contains(binding)) {
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
            for value in &data.params {
                if let Some(latest) = env.values.get(value) {
                    driver.definitions.insert(*value, latest.value);
                }
            }
            driver.entries.insert(*original, env);
        }
        let prologue = block(&mut driver.body, root)?;
        driver.body.entry = prologue;
        let mut env = LiveEnv::default();
        let destroy = driver.module.functions.get(function).map_err(arena)?.attrs.iter()
            .any(|attr| matches!(attr, FunctionAttr::Destroy(_)));
        let witness = driver.witnesses.contains(&function);
        let mut params = vec![];
        for original in &high.blocks.get(high.entry).map_err(arena)?.params {
            let ty = high.values.get(*original).map_err(arena)?.ty;
            let value = parameter(&mut driver.body, prologue, ty, plain_or_service(driver.module, ty)?)?;
            params.push(ty);
            env.values.insert(*original, LatestValue { value, deps: None });
            driver.definitions.insert(*original, value);
        }
        let first = env.values.keys().next().copied();
        for (original, latest) in &mut env.values {
            let ty = driver.body.values.get(latest.value).map_err(arena)?.ty;
            if driver.managed(ty) || witness && Some(*original) == first {
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
        for binding in &high.descriptor_bindings {
            driver.body.descriptor_bindings.push(DescriptorBinding {
                dictionary: env.value(binding.dictionary)?.value,
                contract: binding.contract.clone(),
            });
        }
        let explicit = driver.retained.contains(&root) && !destroy && !fixed_root(driver.module, function);
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
            let state = point.env.state(root)?;
            point = driver.emit(point, Operation::Ownership(OwnershipOp::Accept { state, packet }))?;
        } else {
            signature.ownership_abi = OwnershipAbi::Observation;
            if driver.retained.contains(&root) {
                point = driver.emit(point, Operation::Ownership(OwnershipOp::ScopeBegin { scope: root }))?;
            }
        }
        if let Some(protocol) = driver.module.functions.get(function).map_err(arena)?.summary
            .and_then(|id| driver.module.summaries.get(id).ok()).and_then(|summary| summary.contract.protocol) {
            let reason = protocols::reason(driver.module, protocol)?;
            let packet = if matches!(reason, DynamicReason::Atomic | DynamicReason::Cell) {
                point = driver.emit(point, Operation::Ownership(OwnershipOp::Empty {}))?;
                point.result(SlotName::Packet)?
            } else {
                let values = point.env.values.values().filter_map(|latest| {
                    driver.managed(driver.body.values.get(latest.value).unwrap().ty).then_some(latest.value)
                }).collect();
                let (next, packet) = driver.local_values(point, values)?;
                point = next;
                packet
            };
            let state = point.env.scopes.get(&root).copied()
                .ok_or_else(|| flow_error("checked entry protocol needs its action root"))?;
            point = driver.emit(point, Operation::Ownership(OwnershipOp::Dynamic {
                state, packet, target: root, reason, protocol,
            }))?;
        }
        for (ty, descriptor) in &dictionaries {
            if matches!(driver.module.types.get(*ty), Ok(Type::Parameter(_))) {
                continue;
            }
            let required = driver.analysis.scopes.iter().any(|(scope, facts)| {
                driver.module.scopes.get(*scope).is_ok_and(|scope| scope.function == function)
                    && facts.potential_owned_types.iter().any(|owned| {
                        driver.analysis.types[owned].descriptors.iter().any(|bound| {
                            bound.cleanup == descriptor.cleanup && bound.retention == descriptor.retention
                                && bound.managed == descriptor.managed
                        })
                    })
            });
            if required {
                let mut descriptor = descriptor.clone();
                descriptor.summary = driver.module.functions.get(descriptor.cleanup).map_err(arena)?.summary
                    .ok_or_else(|| flow_error("dictionary cleanup has no inferred summary"))?;
                point = driver.emit(point, Operation::Core(CoreOp::TypeOf { ty: *ty, descriptor }))?;
            }
        }
        signature.params = params;
        driver.module.functions.get_mut(function).map_err(arena)?.signature = signature;
        driver.connect(point, &Target { block: high.entry, args: high.blocks.get(high.entry)
            .map_err(arena)?.params.clone() }, true, false, 0)?;
        for original in &graph.rpo {
            let data = high.blocks.get(*original).map_err(arena)?;
            driver.current_high = *original;
            driver.current_unwind = data.unwind.as_ref().map(|unwind| unwind.target.clone());
            let mut point = Landing {
                block: driver.blocks[original], env: driver.entries[original].clone(),
                results: vec![], forwarded: BTreeMap::new(),
            };
            for instruction in &data.insts {
                driver.current_instruction = Some(*instruction);
                driver.live_after = continuations[instruction].clone();
                let instruction = high.insts.get(*instruction).map_err(arena)?;
                point = driver.instruction(point, instruction).map_err(|error| {
                    at_high_site(error, function, HighOperationSite::Instruction(driver.current_instruction.unwrap()))
                })?;
                for original in &instruction.results {
                    if let Some(latest) = point.env.values.get(original) {
                        driver.definitions.insert(*original, latest.value);
                    }
                }
                driver.prune(&mut point);
            }
            driver.current_instruction = None;
            (|| -> Result<(), OwnError> {
                match &data.term.kind {
                    TermKind::Br(target) => driver.connect(point, target, false, false, 0)?,
                    TermKind::Cond { condition, yes, no } => {
                        let condition = point.env.value(*condition)?.value;
                        let left = block(&mut driver.body, data.lexical_scope)?;
                        let right = block(&mut driver.body, data.lexical_scope)?;
                        driver.body.blocks.get_mut(point.block).map_err(arena)?.term.kind = TermKind::Cond {
                            condition, yes: Target { block: left, args: vec![] },
                            no: Target { block: right, args: vec![] },
                        };
                        let mut left_point = point.clone();
                        left_point.block = left;
                        let mut right_point = point;
                        right_point.block = right;
                        driver.connect(left_point, yes, false, false, 0)?;
                        driver.connect(right_point, no, false, false, 1)?;
                    }
                    TermKind::Switch { value, cases, default } => {
                        driver.switch(point, *value, cases, default)?;
                    }
                    TermKind::Ret { values } | TermKind::RetDefer { values, .. } => {
                        let tag = match &data.term.kind {
                            TermKind::RetDefer { tag, .. } => Some(tag.clone()),
                            _ if helpers.contains(&function) => Some(DeferReturnTag::Continue),
                            _ => None,
                        };
                        driver.exit_landing = None;
                        point = driver.prepare(point, values.first().copied(), None)?;
                        point = driver.close_final(point)?;
                        let committed = if point.env.cleanup.pending.is_some() {
                            let result = values.first().map(|value| high.values.get(*value).unwrap().ty);
                            point = driver.commit(point, ExitDestination::Caller, result)?;
                            let value = point.results.iter().find(|(name, _)| *name == SlotName::Value)
                                .map(|(_, value)| *value);
                            Some((value, point.result(SlotName::Packet)?, point.result(SlotName::Deps)?))
                        } else {
                            None
                        };
                        let mut values = values.iter().map(|value| point.env.value(*value).map(|latest| latest.value))
                            .collect::<Result<Vec<_>, _>>()?;
                        if let Some((value, packet, deps)) = committed {
                            values = value.into_iter().chain([packet]).collect();
                            if matches!(tag, Some(DeferReturnTag::Replace(_))) {
                                values.push(deps);
                            }
                            point.env.linear.remove(&packet);
                        } else if explicit && !helpers.contains(&function) {
                            point = driver.emit(point, Operation::Ownership(OwnershipOp::Empty {}))?;
                            values.push(point.result(SlotName::Packet)?);
                        }
                        driver.body.blocks.get_mut(point.block).map_err(arena)?.term.kind = match tag {
                            Some(DeferReturnTag::Continue) => TermKind::RetDefer {
                                tag: DeferReturnTag::Continue, values: vec![],
                            },
                            Some(tag) => TermKind::RetDefer { tag, values },
                            None => TermKind::Ret { values },
                        };
                    }
                    TermKind::Raise(failure) => {
                        // A raise inside a try follows its block's unwind to the enclosing catch dispatch.
                        point.env.cleanup.failure = Some(point.env.value(*failure)?.value);
                        let target = driver.current_unwind.clone();
                        driver.failures.push(FailureRoute { point, target });
                    }
                    // A tombstone with live linear state (an exhaustive match's
                    // default) becomes an unreachable trap: the verifier treats it
                    // as a program failure, so every live scope must unwind.
                    TermKind::Unreachable if !point.env.all_linear().is_empty() => {
                        let trap = TermKind::Trap { code: TrapCode::Unreachable, message: "unreachable".into() };
                        driver.failure_term(point, &trap)?;
                    }
                    TermKind::Unreachable => {}
                    TermKind::Throw(_) | TermKind::Panic(_) | TermKind::Trap { .. } => {
                        driver.failure_term(point, &data.term.kind)?;
                    }
                    TermKind::Catch { failure, ty, handler, next } => {
                        driver.catch(point, *failure, *ty, handler, next)?;
                    }
                    TermKind::Lookup { kind, collection, key, found, missing } => {
                        driver.lookup(point, *kind, *collection, *key, found, missing)?;
                    }
                    _ => return Err(flow_error("unsupported High terminator")),
                }
                Ok(())
            })().map_err(|error| at_high_site(error, function, HighOperationSite::Terminator(*original)))?;
        }
        // Iterative cleanup queue: failed closes never recursively emit cleanup.
        driver.current_unwind = None;
        while !driver.failures.is_empty() || !driver.replacement_returns.is_empty() {
            if let Some(ReplacementReturn { point, target, source, result }) = driver.replacement_returns.pop() {
                driver.current_high = source;
                driver.current_unwind = target;
                let point = driver.close_all(point, false)?;
                let point = driver.commit(point, ExitDestination::Caller, result)?;
                let mut values = point.results.iter().find(|(name, _)| *name == SlotName::Value)
                    .map(|(_, value)| *value).into_iter().collect::<Vec<_>>();
                values.push(point.result(SlotName::Packet)?);
                driver.body.blocks.get_mut(point.block).map_err(arena)?.term.kind = TermKind::Ret { values };
                continue;
            }
            let FailureRoute { mut point, target } = driver.failures.pop().unwrap();
            let target = driver.consumed_buffer_target(&point.env, target)?;
            let target = target.filter(|target| {
                let data = high.blocks.get(target.block).unwrap();
                !data.insts.is_empty() || !matches!(data.term.kind, TermKind::Raise(value)
                    if data.params.last() == Some(&value) && target.args.is_empty())
            });
            point = driver.abandon_unrouted(point, target.as_ref())?;
            // Pre-accept failure restores fresh parts to their proven original
            // admission. P1 staging proves constructor-root input packets empty.
            let packets: Vec<_> = point.env.linear.iter().copied().collect();
            for packet in packets {
                if Some(packet) == point.env.cleanup.pending {
                    continue;
                }
                // Builder buffers and their journals travel to the High abandon handler by name.
                let ty = driver.body.values.get(packet).map_err(arena)?.ty;
                if !matches!(driver.module.types.get(ty), Ok(Type::Service(ServiceType::Own)))
                    || point.env.journals.values().any(|journal| *journal == packet) {
                    continue;
                }
                let owner = if let Some(owner) = point.env.restore_to.get(&packet) {
                    *owner
                } else {
                    stack(driver.module, driver.body.blocks.get(point.block).map_err(arena)?.lexical_scope)?
                        .into_iter().rev().find(|scope| point.env.scopes.contains_key(scope))
                        .ok_or_else(|| flow_error("packet restoration without a retained scope"))?
                };
                let state = point.env.state(owner)?;
                point = driver.emit(point, Operation::Ownership(OwnershipOp::Admit { state, packet }))?;
            }
            driver.current_unwind = target.clone();
            if let Some(mut target) = target {
                let parameter = *high.blocks.get(target.block).map_err(arena)?.params.last()
                    .ok_or_else(|| flow_error("unwind handler has no failure parameter"))?;
                let failure = point.env.cleanup.failure.ok_or_else(|| flow_error("missing outcome failure"))?;
                point.env.values.insert(parameter, LatestValue { value: failure, deps: None });
                target.args.push(parameter);
                driver.connect(point, &target, false, true, 0)?;
                continue;
            }
            let point = driver.close_all(point, true)?;
            let failure = point.env.cleanup.failure.ok_or_else(|| flow_error("unwind omits failure"))?;
            driver.body.blocks.get_mut(point.block).map_err(arena)?.term.kind = TermKind::Raise(failure);
        }
        for block in driver.blocks.values() {
            driver.ssa.seal(*block, driver.predecessors.get(block).copied().unwrap_or(0))?;
        }
        // D25.2 revision (#771): a stored Future's duty leaves with its packet, never by a rehome.
        super::stored::drop_stored_rehomes(driver.module, &mut driver.body)?;
        for schedule in &high.service_schedules {
            let mut schedule = schedule.clone();
            schedule.dictionary = schedule.dictionary.map(|value| {
                driver.definitions.get(&value).copied()
                    .ok_or_else(|| flow_error("service dictionary has no reaching definition"))
            }).transpose()?;
            driver.body.service_schedules.push(schedule);
        }
        driver.cache_schedules()?;
        driver.emitter.charge(driver.body.values.len() * (1 + driver.body.descriptor_bindings.len()), 0)?;
        for (_, value) in driver.body.values.iter_mut() {
            let Type::Parameter(parameter) = driver.module.types.get(value.ty).map_err(arena)? else { continue };
            if parameter.owner != GenericOwner::Function(function) {
                continue;
            }
            if let Some(binding) = driver.body.descriptor_bindings.iter().find(|binding| {
                binding.contract.managed == DescriptorManaged::Plain && !binding.contract.async_destroy
                    && matches!(binding.contract.retention, SummarySelection::Descriptor(index)
                        if u32::from(index) == parameter.index)
            }) {
                value.facts = ValueFacts::Plain { certificate: PlainCertificate::Descriptor(binding.dictionary) };
            }
        }
        driver.compact()?;
        driver.module.functions.get_mut(function).map_err(arena)?.body = Some(driver.body);
        let flow = flows.remove(&function).unwrap();
        emitter.budget_ref().release(flow.allocated_bytes() + 128);
    }
    super::entries::role_signatures(&mut output.module)?;
    output.stats.emitted_checked_splits += emitter.stats().emitted_checked_splits;
    output.stats.charged_work = emitter.stats().charged_work;
    output.stats.peak_auxiliary_bytes = emitter.stats().peak_auxiliary_bytes;
    output.stats.splits_elided += emitter.stats().splits_elided;
    census::collect(&mut output, emitter.budget_ref())?;
    output.stats.charged_work = emitter.stats().charged_work;
    output.stats.peak_auxiliary_bytes = emitter.stats().peak_auxiliary_bytes;
    Ok(output)
}

struct FailureRoute {
    point: Landing,
    target: Option<Target>,
}

struct ReplacementReturn {
    point: Landing,
    target: Option<Target>,
    source: BlockId,
    result: Option<TypeId>,
}

struct Driver<'a, 'o> {
    module: &'a mut Module,
    high: &'a Body,
    analysis: &'a OwnAnalysis,
    flow: &'a analyze::FunctionAnalysis,
    elide_offers: bool,
    definitions: BTreeMap<ValueId, ValueId>,
    origins: BTreeMap<ValueId, origins::Origins>,
    witnesses: &'a BTreeSet<FuncId>,
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
    live_after: BTreeSet<ValueId>,
    captured: BTreeSet<BindingId>,
    failure_live: BTreeMap<BlockId, BTreeSet<ValueId>>,
    current_high: BlockId,
    current_instruction: Option<InstId>,
    escaping: BTreeMap<(BlockId, usize), BTreeSet<ValueId>>,
    incomplete: BTreeMap<ValueId, BTreeSet<ValueId>>,
    replacement_returns: Vec<ReplacementReturn>,
    constructor_inputs: BTreeMap<InstId, BTreeSet<ScopeId>>,
    /// The lexical scope of the `builder.new` behind every High buffer version.
    builder_owners: BTreeMap<ValueId, ScopeId>,
    /// Cache duties in creation order: the owner scope and the Future (D12, D16).
    cache_duties: Vec<(ScopeId, ValueId)>,
    /// The destination of the exit whose scopes are closing: the landing scope
    /// of an edge, or the caller (`None`) for a return. A `cache.rehome` of an
    /// escaping cache duty names it (C-4).
    exit_landing: Option<ScopeId>,
    /// Scopes that admit service entries at run time: their schedules end with
    /// the dynamic step (D25.3).
    dynamic_owners: BTreeSet<ScopeId>,
    /// High Futures that their awaiting scope takes (OIR-2 §10.2, §10.3).
    temporaries: BTreeSet<ValueId>,
}
impl Driver<'_, '_> {
    /// Whether values of `ty` carry full dependencies. Linear buffers never do:
    /// their element journal travels separately (`LiveEnv::journals`).
    fn managed(&self, ty: TypeId) -> bool {
        (self.analysis.types.get(&ty).is_some_and(TypeClassification::may_own)
            || self.analysis.managed_references.contains(&ty))
            && !matches!(self.module.types.get(ty), Ok(Type::Service(_) | Type::Descriptor | Type::Builder(_)
                | Type::MapBuilder(..) | Type::Frame(_)))
    }
    fn emit(&mut self, point: Landing, op: Operation) -> Result<Landing, OwnError> {
        self.emit_restoring(point, op, None)
    }
    /// `restored` names the High buffer whose checked unwind results (buffer
    /// and element journal) replace it under the same High name.
    fn emit_restoring(
        &mut self,
        mut point: Landing,
        op: Operation,
        restored: Option<ValueId>,
    ) -> Result<Landing, OwnError> {
        point.env.unwind_values = Some(self.unwind_needed(&point));
        let closed_parent = match &op {
            Operation::Ownership(OwnershipOp::ScopeEnd { state, .. }) => {
                match self.module.types.get(self.body.values.get(*state).map_err(arena)?.ty).map_err(arena)? {
                    Type::Service(ServiceType::Scope(scope)) => self.module.scopes.get(*scope).map_err(arena)?.parent,
                    _ => None,
                }
            }
            _ => None,
        };
        let emission = self.emitter.emit(self.module, &mut self.body, point.block, point.env, op)?;
        if let Some(mut unwind) = emission.unwind {
            if let Some(buffer) = restored {
                let value = unwind.result(SlotName::Buffer)?;
                unwind.env.values.insert(buffer, LatestValue { value, deps: None });
                let journal = unwind.result(SlotName::Packet)?;
                unwind.env.journals.insert(buffer, journal);
            }
            // A failed close has already left the child scope before outer defers run.
            if let Some(parent) = closed_parent {
                self.body.blocks.get_mut(unwind.block).map_err(arena)?.lexical_scope = parent;
            }
            let target = self.unwind_target(&unwind.env);
            self.record_failure(unwind, target)?;
        }
        let mut normal = emission.normal;
        normal.env.cleanup.primary_before_failure = None;
        Ok(normal)
    }
    /// High references the exceptional continuation of the current operation needs.
    fn unwind_needed(&self, point: &Landing) -> BTreeSet<ValueId> {
        let mut needed: BTreeSet<ValueId> = BTreeSet::new();
        if let Some(target) = &self.current_unwind {
            needed.extend(&target.args);
            needed.extend(&self.failure_live[&target.block]);
        }
        for prefix in point.env.cleanup.defers.values() {
            for registration in prefix {
                if let Operation::Lexical(LexicalOp::DeferRegister { action, .. }) =
                    &self.high.insts.get(*registration).unwrap().op {
                    needed.extend(&action.captures);
                }
            }
        }
        needed
    }
    /// The High handler of an exceptional landing, unless its scope has already closed.
    fn unwind_target(&self, env: &LiveEnv) -> Option<Target> {
        self.current_unwind.clone().filter(|target| {
            let lexical = self.high.blocks.get(target.block).unwrap().lexical_scope;
            !self.retained.contains(&lexical) || env.scopes.contains_key(&lexical)
        })
    }
    fn instruction(&mut self, mut point: Landing, instruction: &Instruction) -> Result<Landing, OwnError> {
        let _handling = routing::handling(&instruction.op);
        if matches!(instruction.op, Operation::Lexical(LexicalOp::Temp { .. } | LexicalOp::Want { .. }
            | LexicalOp::Discard { .. }) | Operation::Exit(ExitOp::Prepare { .. })) {
            return Ok(point);
        }
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
                let owner = self.high.bindings.get(binding).map_err(arena)?.scope;
                let ty = self.body.values.get(value).map_err(arena)?.ty;
                // A Future's binding names it; its cache duty stays in the owner state (D12).
                if self.managed(ty) && !super::futures::is_future(self.module, ty)
                    && point.env.scopes.contains_key(&owner) {
                    let packet;
                    if point.env.scopes.len() == 1 {
                        let state = *point.env.scopes.values().next().unwrap();
                        let selection = self.body.selections.alloc(SelectionExpr::Value(value)).map_err(arena)?;
                        let selection = self.body.selections.alloc(SelectionExpr::Admissible(selection))
                            .map_err(arena)?;
                        point = self.emit(point, Operation::Ownership(OwnershipOp::ScopeSplit { state, selection }))?;
                        packet = point.result(SlotName::Selected)?;
                    } else {
                        let (next, selected) = self.offered(point, &[original])?;
                        point = next;
                        packet = selected;
                    }
                    let resource = point.env.value(original)?.value;
                    let binding = ElaboratedBinding {
                        binding, resource: ResourceValue { value: resource, packet },
                        state: point.env.state(owner)?, original_scope: owner,
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
            Operation::Lexical(LexicalOp::Temp { .. } | LexicalOp::Want { .. } | LexicalOp::Discard { .. })
            | Operation::Exit(ExitOp::Prepare { .. }) => return Ok(point),
            Operation::Lexical(LexicalOp::DeferRegister { scope, .. }) => {
                let inst = self.current_instruction.ok_or_else(|| flow_error("registration is not in High"))?;
                point.env.cleanup.defers.entry(scope).or_default().push(inst);
                return Ok(point);
            }
            Operation::Call(CallOp::Direct { function, .. } | CallOp::Intrinsic { function, .. })
                if super::futures::is_async(self.module, function) => {
                return self.create_future(point, instruction, function);
            }
            Operation::Async(AsyncOp::Await { .. }) => return self.await_future(point, instruction),
            Operation::Async(AsyncOp::FutureSpawn { state: None, .. }) => {
                return self.spawn_future(point, instruction);
            }
            // Elaboration §13.1, Champ draft 3 §5: a cancellation request and the two queries only observe the
            // Future. Cancelling a cold Future never moves its inputs: they stay with its cache duty in the
            // owner's journal until that duty settles at the owner's close.
            Operation::Async(AsyncOp::FutureCancel { .. } | AsyncOp::FutureDone { .. }
                | AsyncOp::FutureCancelled { .. }) => {}
            Operation::Call(CallOp::Direct { function, .. } | CallOp::Native { function, .. }
                | CallOp::Intrinsic { function, .. }) => return self.call(point, instruction, function),
            Operation::Call(CallOp::Closure { .. } | CallOp::Cap { .. } | CallOp::Role { .. }
                | CallOp::Order { .. }) => {
                return self.dispatch(point, instruction);
            }
            Operation::Core(CoreOp::FnClosure { .. } | CoreOp::FnBind { .. } | CoreOp::FnBindRole { .. }
                | CoreOp::CapMake { .. } | CoreOp::AnyBox { .. } | CoreOp::RoleWrap { .. }
                | CoreOp::RoleNarrow { .. } | CoreOp::OnceLoad { .. } | CoreOp::OnceStore { .. }) => {
                return self.protocol_value(point, instruction);
            }
            Operation::Lexical(LexicalOp::Stage { action, field, .. }) => {
                let Operation::Lexical(LexicalOp::Stage { value, .. }) = instruction.op else { unreachable!() };
                return self.stage(point, action, field, value);
            }
            Operation::Core(CoreOp::ObjNew { .. } | CoreOp::ObjNext { .. } | CoreOp::VariantNew { .. }
                | CoreOp::ObjInit { .. } | CoreOp::ObjGet { .. } | CoreOp::ObjPending { .. }
                | CoreOp::SuccTake { .. }) => {
                return self.construct(point, instruction);
            }
            // High collections never name element packets: inserted obligations stay
            // with their owners and the result retains them through full deps.
            Operation::Collection(ref collection) if arrays::packet(collection).is_none() => {
                if matches!(collection, CollectionOp::BuilderNew { .. } | CollectionOp::MapBuilderNew { .. })
                    || arrays::consumed_buffer(&instruction.op).is_some() {
                    return self.buffer(point, instruction, op);
                }
            }
            Operation::Core(CoreOp::VariantOf { .. } | CoreOp::VariantGet { .. } | CoreOp::Retag { .. }) => {}
            Operation::Const(_) | Operation::Core(CoreOp::Undef { .. }) => {}
            // Plain observations (val.eq, variant.tag, ...) only borrow their operands.
            Operation::Core(_) if routing::handling(&op) == routing::Handling::Scalar
                && instruction.results.iter().all(|value| !self.managed(self.high.values.get(*value).unwrap().ty)) => {}
            Operation::Core(_) if instruction.results.iter().all(|value| {
                !self.managed(self.high.values.get(*value).unwrap().ty)
            }) && op.operand_refs().iter().all(|operand| match operand.reference {
                OperandRef::Value(value) => !self.managed(self.body.values.get(value).unwrap().ty),
                _ => false,
            }) => {}
            _ => return Err(unsupported(&op)),
        }
        point = self.emit(point, op)?;
        let results = point.results.clone();
        for (original, (_, value)) in instruction.results.iter().zip(&results) {
            let mut latest = LatestValue { value: *value, deps: None };
            if self.managed(self.body.values.get(*value).map_err(arena)?.ty) {
                let packet = point.results.iter().find(|(name, _)| *name == SlotName::Packet)
                    .map(|(_, packet)| *packet);
                let scope = self.body.blocks.get(point.block).map_err(arena)?.lexical_scope;
                let state = point.env.scopes.get(&scope).copied();
                point = self.emit(point, Operation::Ownership(OwnershipOp::Dependencies { value: *value }))?;
                latest.deps = Some(point.result(SlotName::Value)?);
                if let (Some(state), Some(packet)) = (state, packet) {
                    point = self.emit(point, Operation::Ownership(OwnershipOp::Admit { state, packet }))?;
                }
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
        self.close_scope(point, scope, unwind, false)
    }
    /// `cancel` adds final deferred-cancellation delivery to the close (D18).
    fn close_scope(&mut self, mut point: Landing, scope: ScopeId, unwind: bool, cancel: bool)
        -> Result<Landing, OwnError> {
        point = self.run_defers(point, scope)?;
        if !unwind && point.env.cleanup.pending.is_some() {
            point = self.rehome_caches(point, scope)?;
            point = self.extend(point, scope)?;
        }
        let state = point.env.state(scope)?;
        let reserve = point.env.cleanup.pending;
        point.env.bindings.retain(|binding, _| self.high.bindings.get(*binding)
            .is_ok_and(|binding| binding.scope != scope));
        if unwind {
            point.env.cleanup.wanted = None;
            point.env.cleanup.carried.clear();
            point.env.cleanup.replacement = None;
            let mut needed: BTreeSet<ValueId> = BTreeSet::new();
            if let Some(target) = &self.current_unwind {
                needed.extend(&target.args);
                needed.extend(&self.failure_live[&target.block]);
            }
            let journals = &point.env.journals;
            point.env.values.retain(|value, _| needed.contains(value) || journals.contains_key(value));
        }
        let mut effects = self.analysis.scopes.get(&scope)
            .ok_or_else(|| flow_error("closed scope has no analysis facts"))?.effects.clone();
        effects.cancel = cancel;
        let op = if unwind {
            OwnershipOp::ScopeUnwind {
                state, failure: point.env.cleanup.failure.ok_or_else(|| flow_error("missing active failure"))?,
                reserve, effects,
            }
        } else {
            OwnershipOp::ScopeEnd { state, reserve, effects }
        };
        let point = self.emit(point, Operation::Ownership(op))?;
        let parent = self.module.scopes.get(scope).map_err(arena)?.parent;
        if let Some(parent) = parent {
            let reached_handler = self.current_unwind.as_ref().is_none_or(|target| {
                self.high.blocks.get(target.block).is_ok_and(|block| block.lexical_scope == parent)
            });
            if reached_handler {
                self.current_unwind = self.high.blocks.iter().find_map(|(_, block)| {
                    (block.lexical_scope == parent).then_some(block.unwind.as_ref())
                        .flatten().map(|unwind| unwind.target.clone())
                });
            }
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
        ordinal: usize,
    ) -> Result<(), OwnError> {
        let original = self.high.blocks.get(target.block).map_err(arena)?;
        let from = self.body.blocks.get(point.block).map_err(arena)?.lexical_scope;
        let plan = edges::EdgePlan::between(self.module, from, Some(original.lexical_scope))?;
        let closing = plan.close.iter().rev().find(|scope| point.env.scopes.contains_key(scope)).copied();
        let escaped: Vec<_> = if !prologue && !unwind && closing.is_some() {
            target.args.iter().filter(|value| self.escaping.get(&(self.current_high, ordinal))
                .is_some_and(|values| values.contains(value))).copied().collect()
        } else {
            vec![]
        };
        self.exit_landing = Some(original.lexical_scope);
        if !prologue && !unwind && let Some(closing) = closing {
            point = self.prepare_edge(point, &escaped, closing)?;
        }
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
        if !unwind && point.env.cleanup.pending.is_some() {
            let result = if escaped.len() == 1 {
                Some(self.high.values.get(escaped[0]).map_err(arena)?.ty)
            } else {
                None
            };
            let destination = match point.env.cleanup.destination {
                Some(ExitDestination::Loop(boundary)) => ExitDestination::Loop(boundary),
                _ => ExitDestination::Scope(original.lexical_scope),
            };
            let rehomed = point.env.cleanup.rehomed;
            point = self.commit(point, destination, result)?;
            if let Some(value) = escaped.first().filter(|_| escaped.len() == 1) {
                point.env.values.insert(*value, LatestValue {
                    value: point.result(SlotName::Value)?, deps: Some(point.result(SlotName::Deps)?),
                });
            }
            let packet = point.result(SlotName::Packet)?;
            let state = point.env.state(original.lexical_scope)?;
            point = self.emit(point, Operation::Ownership(OwnershipOp::Admit { state, packet }))?;
            // Admitted service entries join the landing scope's dynamic inventory (D25.3).
            if rehomed {
                self.dynamic_owners.insert(original.lexical_scope);
            }
            point.env.cleanup.rehomed = false;
            point.env.cleanup.wanted = None;
            point.env.cleanup.carried.clear();
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
        slots.extend(point.env.journals.iter().map(|(buffer, journal)| (SsaSlot::Journal(*buffer), LatestValue {
            value: *journal, deps: None,
        })));
        let arguments: Vec<_> = target.args.iter().map(|value| point.env.value(*value)).collect::<Result<_, _>>()?;
        for ((parameter, latest), argument) in original.params.iter().zip(arguments).zip(&target.args) {
            slots.insert(SsaSlot::Value(*parameter), latest);
            if let Some(journal) = point.env.journals.get(argument) {
                slots.insert(SsaSlot::Journal(*parameter), LatestValue { value: *journal, deps: None });
            }
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
            let mut values = control::term_values(&data.term.kind);
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

/// Rewrite and run the independent Own postcondition. This certifies only the
/// implemented verifier checks; #564's D8/D17/D18 semantic gate remains external.
pub fn rewrite_synchronous(output: OwnOutput, options: &OwnOptions) -> Result<OwnOutput, OwnError> {
    rewrite_synchronous_with(output, options, VerifyContext::default())
}

/// The same semantic gate with explicitly trusted runtime/native contracts.
pub fn rewrite_synchronous_with(
    output: OwnOutput,
    options: &OwnOptions,
    context: VerifyContext<'_>,
) -> Result<OwnOutput, OwnError> {
    let output = rewrite_subset(output, options)?;
    if let Err(mut report) = verify_with(&output.module, &options.verify, context) {
        if report.failure == VerifyFailure::Incomplete {
            return Err(OwnError::IncompleteVerification { report: Box::new(report), stats: Box::new(output.stats) });
        }
        finalize::label_borrowed_rebind(&output.module, &mut report);
        return Err(OwnError::InvalidOutput(Box::new(report)));
    }
    Ok(output)
}

fn at_high_site(mut error: OwnError, function: FuncId, site: HighOperationSite) -> OwnError {
    if let OwnError::Diagnostics { diagnostics, .. } = &mut error {
        for diagnostic in diagnostics {
            diagnostic.function.get_or_insert(function);
            diagnostic.site.get_or_insert(site);
        }
    }
    error
}
