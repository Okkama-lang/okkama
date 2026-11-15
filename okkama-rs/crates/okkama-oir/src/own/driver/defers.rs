use super::*;

impl Driver<'_, '_> {
    pub(super) fn run_defers(&mut self, mut point: Landing, scope: ScopeId) -> Result<Landing, OwnError> {
        while let Some(registration) = point.env.cleanup.defers.get_mut(&scope).and_then(Vec::pop) {
            let Operation::Lexical(LexicalOp::DeferRegister { action, .. }) =
                &self.high.insts.get(registration).map_err(arena)?.op else {
                return Err(flow_error("defer prefix contains a non-registration"));
            };
            let action = action.clone();
            let mut latest = vec![];
            for capture in &action.captures {
                latest.push(point.env.value(*capture)?);
            }
            for binding in &action.bindings {
                latest.push(*point.env.bindings.get(binding)
                    .ok_or_else(|| flow_error("defer capture has no latest binding"))?);
            }
            let values = latest.iter().filter(|latest| {
                self.managed(self.body.values.get(latest.value).unwrap().ty)
            }).map(|latest| latest.value).collect();
            let (next, packet) = self.offered_values(point, values)?;
            point = next;
            for value in &mut latest {
                value.value = point.forwarded.get(&value.value).copied().unwrap_or(value.value);
                value.deps = value.deps.map(|deps| point.forwarded.get(&deps).copied().unwrap_or(deps));
            }
            // All value references dominate the extraction landings. Managed
            // captures retain their complete deps independently of selection.
            let summary = self.module.functions.get(action.function).map_err(arena)?.summary
                .ok_or_else(|| flow_error("defer has no summary"))?;
            let effects = self.module.summaries.get(summary).map_err(arena)?.clone();
            let call = CallSite {
                cleanup: CallCleanupMode::Ordinary,
                args: latest.iter().zip(&effects.parameters).map(|(latest, effect)| CallArgument {
                    value: latest.value,
                    convention: if effect.convention == FreshConvention::Take {
                        OperandConvention::Consume
                    } else {
                        OperandConvention::Borrow
                    },
                }).collect(),
                packet: Some(packet), deps: latest.iter().filter_map(|latest| latest.deps).collect(),
                descriptors: vec![], type_args: vec![], summary: Some(summary),
            };
            let consumed = BTreeSet::from([packet]);
            let op = Operation::Call(CallOp::Direct { function: action.function, call: call.clone() });
            let continue_schema = OutcomeContract {
                results: vec![], transfers: vec![TransferRule::Settle { inputs: vec![packet] }],
                postconditions: vec![Postcondition::CalleeInputsSettled, Postcondition::Shielded], retention: vec![],
            };
            let (continued, continue_edge) = self.emitter.landing(self.module, &mut self.body, scope, &point.env,
                &consumed, &continue_schema, Outcome::DeferContinue, &op, &[])?;
            let mut replacements = vec![];
            let mut continuations = vec![continued];
            for mapping in &action.replacements {
                let labels = self.module.functions.get(action.function).map_err(arena)?.attrs.iter().find_map(|attr| {
                    if let FunctionAttr::DeferEntry { replacements } = attr { Some(replacements) } else { None }
                }).ok_or_else(|| flow_error("defer helper has no tagged entry"))?;
                let index = labels.iter().position(|label| label.label == mapping.label)
                    .ok_or_else(|| flow_error("defer registration names an undeclared replacement label"))?;
                let outcome = Outcome::DeferReplace(u16::try_from(index)
                    .map_err(|_| flow_error("too many defer labels"))?);
                let results = ContractRegistry::new(self.module).defer_results(action.function, outcome)
                    .map_err(|error| flow_error(format!("defer schema: {error}")))?;
                let schema = OutcomeContract { results, ..continue_schema.clone() };
                let (mut replaced, edge) = self.emitter.landing(self.module, &mut self.body, scope, &point.env,
                    &consumed, &schema, outcome, &op, &[])?;
                replacements.push(DeferReplaceEdge { label: mapping.label.clone(), edge });
                let packet = replaced.result(SlotName::Packet)?;
                let state = replaced.env.scopes[&scope];
                if replaced.env.cleanup.failure.is_some() {
                    replaced = self.emit(replaced, Operation::Ownership(OwnershipOp::Accept { state, packet }))?;
                } else {
                    if mapping.kind != ExitKind::Return {
                        return Err(flow_error("D8 non-return replacement needs a producer CFG destination record"));
                    }
                    let boundary = self.high.boundaries.get(mapping.boundary).map_err(arena)?;
                    let root = stack(self.module, scope)?[0];
                    if boundary.scope != root {
                        return Err(flow_error("D8 return replacement must target the enclosing action"));
                    }
                    let function = self.module.scopes.get(root).map_err(arena)?.function;
                    let declaration = self.module.functions.get(function).map_err(arena)?;
                    if declaration.signature.ownership_abi == OwnershipAbi::Observation {
                        return Err(flow_error("D8 return replacement needs an explicit result-packet entry ABI"));
                    }
                    let value = replaced.results.iter().find(|(name, _)| *name == SlotName::Value)
                        .map(|(_, value)| *value);
                    let deps = replaced.result(SlotName::Deps)?;
                    let result = value.map(|value| self.body.values.get(value).map(|data| data.ty))
                        .transpose().map_err(arena)?;
                    let language_result = declaration.signature.results.iter().copied().find(|ty| {
                        !matches!(self.module.types.get(*ty), Ok(Type::Service(ServiceType::Own)))
                    });
                    if result != language_result {
                        return Err(flow_error("D8 replacement type differs from the enclosing return type"));
                    }
                    let selection = if let Some(value) = value {
                        self.body.selections.alloc(SelectionExpr::Value(value)).map_err(arena)?
                    } else {
                        self.body.selections.alloc(SelectionExpr::Union(vec![])).map_err(arena)?
                    };
                    let pending = replaced.env.cleanup.pending
                        .ok_or_else(|| flow_error("defer replacement has no pending exit"))?;
                    replaced = self.emit(replaced, Operation::Exit(ExitOp::Replace {
                        pending, replacement: PreparedExit {
                            kind: mapping.kind, value, packet: Some(packet), deps: Some(deps),
                            selection, boundary: mapping.boundary,
                        },
                    }))?;
                    let packet = replaced.result(SlotName::Rest)?;
                    let state = replaced.env.scopes[&scope];
                    replaced = self.emit(replaced, Operation::Ownership(OwnershipOp::Accept { state, packet }))?;
                    if !replaced.env.cleanup.returning {
                        replaced.env.cleanup.wanted = None;
                        replaced.env.cleanup.carried.clear();
                        replaced.env.cleanup.replacement = value.map(|value| LatestValue {
                            value, deps: result.filter(|ty| self.managed(*ty)).map(|_| deps),
                        });
                        replaced.env.cleanup.returning = true;
                        // The replaced fallthrough continuation is abandoned. Retain
                        // only references needed by remaining defers or an outer catch.
                        let mut needed: BTreeSet<ValueId> = BTreeSet::new();
                        if let Some(target) = &self.current_unwind {
                            needed.extend(&target.args);
                            needed.extend(&self.failure_live[&target.block]);
                        }
                        for prefix in replaced.env.cleanup.defers.values() {
                            for registration in prefix {
                                if let Operation::Lexical(LexicalOp::DeferRegister { action, .. }) =
                                    &self.high.insts.get(*registration).map_err(arena)?.op {
                                    needed.extend(&action.captures);
                                }
                            }
                        }
                        replaced.env.values.retain(|value, _| needed.contains(value));
                        self.replacement_returns.push(ReplacementReturn {
                            point: replaced, target: self.current_unwind.clone(), source: self.current_high, result,
                        });
                        continue;
                    }
                    if let (Some(original), Some(value)) = (replaced.env.cleanup.wanted, value) {
                        let managed = self.managed(self.body.values.get(value).map_err(arena)?.ty);
                        replaced.env.values.insert(original, LatestValue {
                            value, deps: managed.then_some(deps),
                        });
                    }
                    if replaced.env.cleanup.replacement.is_some() {
                        replaced.env.cleanup.replacement = value.map(|value| LatestValue {
                            value, deps: result.filter(|ty| self.managed(*ty)).map(|_| deps),
                        });
                    }
                }
                // Replacement continues the same remaining prefix and close.
                // This path is emitted as a queued cleanup continuation below.
                continuations.push(replaced);
            }
            let unwind = if effects.contract.failures.iter().any(|kind| *kind != FailureKind::Cancelled) {
                let results = ContractRegistry::new(self.module).defer_results(action.function, Outcome::Unwind)
                    .map_err(|error| flow_error(format!("defer unwind schema: {error}")))?;
                let schema = OutcomeContract { results, ..continue_schema };
                let (mut failed, edge) = self.emitter.landing(self.module, &mut self.body, scope, &point.env,
                    &consumed, &schema, Outcome::Unwind, &op, &[])?;
                if let Some(primary) = point.env.cleanup.failure {
                    let primary = failed.forwarded.get(&primary).copied().unwrap_or(primary);
                    let cleanup = failed.result(SlotName::Failure)?;
                    failed = self.emit(failed, Operation::Core(CoreOp::FailSecondary { primary, cleanup }))?;
                    failed.env.cleanup.failure = Some(failed.result(SlotName::Value)?);
                }
                self.failures.push(FailureRoute { point: failed, target: self.current_unwind.clone() });
                Some(edge)
            } else {
                None
            };
            self.body.blocks.get_mut(point.block).map_err(arena)?.term.kind = TermKind::InvokeDefer {
                function: action.function, call, continue_edge, replacements, unwind,
            };
            point = self.merge_cleanup(continuations, scope)?;
        }
        point.env.cleanup.defers.remove(&scope);
        Ok(point)
    }
}

impl Driver<'_, '_> {
    fn cleanup_slots(env: &LiveEnv) -> BTreeMap<SsaSlot, LatestValue> {
        let mut slots: BTreeMap<_, _> = env.values.iter().map(|(key, value)| (SsaSlot::Value(*key), *value))
            .chain(env.bindings.iter().map(|(key, value)| (SsaSlot::Binding(*key), *value)))
            .chain(env.scopes.iter().map(|(key, value)| (SsaSlot::Scope(*key), LatestValue {
                value: *value, deps: None,
            })))
            .chain(env.journals.iter().map(|(key, value)| (SsaSlot::Journal(*key), LatestValue {
                value: *value, deps: None,
            }))).collect();
        if let Some(value) = env.cleanup.pending {
            slots.insert(SsaSlot::Linear(0), LatestValue { value, deps: None });
        }
        if let Some(value) = env.cleanup.failure {
            slots.insert(SsaSlot::Linear(1), LatestValue { value, deps: None });
        }
        if let Some(value) = env.cleanup.replacement {
            slots.insert(SsaSlot::ExitValue, value);
        }
        slots
    }

    fn merge_cleanup(&mut self, mut points: Vec<Landing>, scope: ScopeId) -> Result<Landing, OwnError> {
        if points.len() == 1 {
            return Ok(points.pop().unwrap());
        }
        let destination = block(&mut self.body, scope)?;
        let mut ssa = SealedSsa::default();
        let mut env = points[0].env.clone();
        for (slot, latest) in Self::cleanup_slots(&env) {
            let ty = self.body.values.get(latest.value).map_err(arena)?.ty;
            let managed = self.managed(ty);
            let parameter = ssa.declare(self.module, &mut self.body, destination, slot, ty, managed)?;
            if env.linear.remove(&latest.value) {
                env.linear.insert(parameter.value);
            }
            match slot {
                SsaSlot::Value(key) => { env.values.insert(key, parameter); }
                SsaSlot::Journal(key) => { env.journals.insert(key, parameter.value); }
                SsaSlot::Binding(key) => { env.bindings.insert(key, parameter); }
                SsaSlot::Scope(key) => { env.scopes.insert(key, parameter.value); }
                SsaSlot::Linear(0) => {
                    env.linear.remove(&latest.value);
                    env.linear.insert(parameter.value);
                    env.cleanup.pending = Some(parameter.value);
                }
                SsaSlot::Linear(1) => { env.cleanup.failure = Some(parameter.value); }
                SsaSlot::ExitValue => { env.cleanup.replacement = Some(parameter); }
                _ => unreachable!(),
            }
        }
        let count = points.len();
        for (ordinal, point) in points.into_iter().enumerate() {
            let args = ssa.incoming(&self.body, destination, point.block, ordinal,
                &Self::cleanup_slots(&point.env))?;
            self.body.blocks.get_mut(point.block).map_err(arena)?.term.kind = TermKind::Br(Target {
                block: destination, args,
            });
        }
        ssa.seal(destination, count)?;
        Ok(Landing { block: destination, env, results: vec![], forwarded: BTreeMap::new() })
    }
}
