use super::*;

#[derive(Clone)]
pub(super) struct ReturnWitness {
    pub tag: DeferReturnTag,
    pub values: Vec<ValueId>,
    pub flow: Flow,
}

impl Engine<'_, '_> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn defer_invoke(
        &mut self, block: BlockId, function: FuncId, call: &CallSite, continued: &InvokeEdge,
        replacements: &[DeferReplaceEdge], unwind: Option<&InvokeEdge>, before: &Flow,
        outgoing: &mut Vec<(BlockId, Flow)>,
    ) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("defer::defer_invoke");
        if !replacements.is_empty() && !self.defer_cache.contains_key(&function) {
            if self.defer_stack.contains(&function)
                || self.defer_stack.len() >= self.state.options.limits.expression_depth {
                // A cyclic/over-budget result query is incomplete, never evidence
                // that the recursive arm returns an empty packet.
                self.state.incomplete = true;
                return;
            }
            let Some(declaration) = self.module.functions.get(function).ok() else {
                return;
            };
            let Some(body) = &declaration.body else {
                self.error(R::B51, "defer replacement requires a checked body");
                return;
            };
            let mut stack = self.defer_stack.clone();
            stack.push(function);
            let mut helper = Engine {
                module: self.module, function, declaration, body, state: self.state,
                emit: false, reported: BTreeSet::new(), managed: self.managed, obligations: self.obligations,
                flow_memory: 0, materialized_scopes: None,
                diagnose: false, collecting: false, capture_returns: true, contract_invalid: false,
                witnesses: witnesses(self.module, body),
                defer_stack: stack, defer_outputs: vec![],
                defer_cache: Default::default(),
            };
            helper.solve();
            let invalid = helper.contract_invalid;
            let outputs = helper.defer_outputs;
            let loc = self.body.blocks.get(block).ok().and_then(|block| block.term.loc);
            self.state.at(L::Terminator { function: self.function, block }, loc);
            if invalid {
                self.error(R::B51, "defer result contract is not established by its checked body");
                return;
            }
            self.defer_cache.insert(function, outputs);
        }
        let mut shielded = call.clone();
        shielded.cleanup = CallCleanupMode::Shielded;
        let op = Operation::Call(CallOp::Direct { function, call: shielded });
        let registry = ContractRegistry::new(self.module);
        let Ok(contract) = registry.construction_schema(&op, self.body) else {
            return;
        };
        let arms = std::iter::once((Outcome::DeferContinue, continued))
            .chain(replacements.iter().enumerate().map(|(index, edge)| {
                (Outcome::DeferReplace(index as u16), &edge.edge)
            }))
            .chain(unwind.map(|edge| (Outcome::Unwind, edge)));
        for (outcome, edge) in arms {
            let Ok(results) = registry.defer_results(function, outcome) else {
                continue;
            };
            let mut schema = if outcome == Outcome::Unwind {
                contract.unwind.clone().unwrap_or_else(|| contract.normal.clone())
            } else { contract.normal.clone() };
            schema.results = results;
            if outcome == Outcome::DeferContinue {
                schema.transfers = vec![TransferRule::Settle { inputs: call.packet.into_iter().collect() }];
            }
            let mut branch = before.clone();
            let opaque = if let Outcome::DeferReplace(index) = outcome {
                Some(Origin::Invoke(block, u32::MAX - u32::from(index)))
            } else { None };
            if let Some(origin) = opaque {
                self.retire_origin(&mut branch, origin, false);
            }
            let normal = if outcome == Outcome::Unwind { Outcome::Unwind } else { Outcome::Normal };
            let mut slots = self.operation(block, &op, &schema, normal, Origin::Invoke(block, 0), &mut branch);
            if let Some(key) = opaque {
                let Outcome::DeferReplace(index) = outcome else {
                    unreachable!()
                };
                let label = &replacements[usize::from(index)].label;
                self.defer_replacement(function, label, &schema, key, call, before, &mut slots);
            }
            if before.active_failure != 0 {
                branch.active_failure = before.active_failure;
                branch.active_failure_origin = before.active_failure_origin;
            }
            outgoing.push(self.edge(&branch, edge.block, &edge.args, &slots, R::B101));
        }
    }
}

impl Engine<'_, '_> {
    pub(super) fn defer_return(&mut self, tag: &DeferReturnTag, values: &[ValueId], flow: &Flow) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("defer::defer_return");
        if matches!(tag, DeferReturnTag::Continue) {
            return;
        }
        let language: Vec<_> = values.iter().copied().filter(|value| !matches!(
            super::super::later::value_type(self.module, self.body, *value), Some(Type::Service(_)))).collect();
        let keys = language.iter().flat_map(|value| flow.refs[value.as_u32() as usize].iter().copied()).collect();
        let labels = language.iter().flat_map(|value| flow.contained[value.as_u32() as usize].iter().copied())
            .collect();
        for &value in values {
            if super::super::later::value_type(self.module, self.body, value)
                != Some(&Type::Service(ServiceType::Own)) { continue; }
            for piece in &flow.tokens[value.as_u32() as usize].pieces {
                if !self.subset(piece, &keys, &labels) {
                    self.error(R::B101, "defer may export only its actual replacement's retained packet");
                }
            }
        }
    }

    fn defer_projection_bound(
        &mut self, witness: &Flow, base: ValueId, actual: &impl Fn(ValueId) -> Option<ValueId>, flow: &Flow,
    ) -> Option<BTreeSet<Origin>> {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("defer::defer_projection_bound");
        let mut pending = vec![base];
        let mut seen = BTreeSet::new();
        let mut bound = BTreeSet::new();
        let mut resolved = false;
        while let Some(value) = pending.pop() {
            if !self.state.step(1) {
                return None;
            }
            if !seen.insert(value) {
                continue;
            }
            if let Some(value) = actual(value) {
                resolved = true;
                bound.extend(&flow.refs[value.as_u32() as usize]);
                bound.extend(&flow.full_deps[value.as_u32() as usize]);
                continue;
            }
            let refs = &witness.refs[value.as_u32() as usize];
            if refs.is_empty() {
                return None;
            }
            for origin in refs {
                match origin {
                    Origin::External(parameter) => {
                        let value = actual(*parameter)?;
                        resolved = true;
                        bound.extend(&flow.refs[value.as_u32() as usize]);
                        bound.extend(&flow.full_deps[value.as_u32() as usize]);
                    }
                    Origin::Projection(base, _) => pending.push(*base),
                    _ => return None,
                }
            }
        }
        resolved.then_some(bound)
    }

    #[allow(clippy::too_many_arguments)]
    fn defer_replacement(
        &mut self, function: FuncId, label: &str, schema: &OutcomeContract, key: Origin,
        call: &CallSite, flow: &Flow, slots: &mut [Slot],
    ) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("defer::defer_replacement");
        // D8 exports the actual packet. Derive each tagged return independently
        // with the verifier engine; a borrowed replacement never creates ownership.
        let declaration = self.module.functions.get(function).unwrap();
        let body = declaration.body.as_ref().unwrap();
        let parameters: Vec<_> = body.blocks.get(body.entry).unwrap().params.iter().copied().filter(|value| {
            body.values.get(*value).ok().and_then(|data| self.module.types.get(data.ty).ok())
                .is_some_and(|ty| !matches!(ty, Type::Service(_) | Type::Descriptor))
        }).collect();
        let actual = |value: ValueId| parameters.iter().position(|parameter| *parameter == value)
            .and_then(|index| call.args.get(index)).map(|arg| arg.value);
        // Unknown callee-local origins still carry every caller dependency.
        // D8 changes ownership only for the actual packet (design §10.4).
        let external: BTreeSet<_> = call.args.iter().flat_map(|arg| {
            flow.full_deps[arg.value.as_u32() as usize].iter().copied()
        }).collect();
        let witnesses = self.defer_cache.get(&function).cloned().unwrap_or_default();
        let mut refs = BTreeSet::new();
        let mut deps = BTreeSet::new();
        let mut contained = SharedSet::new();
        let mut packet = Token::default();
        for witness in witnesses.iter().filter(|item| {
            matches!(&item.tag, DeferReturnTag::Replace(returned) if returned == label)
        }) {
            let origins: BTreeSet<_> = witness.flow.refs.iter().chain(&witness.flow.full_deps)
                .flat_map(|refs| refs.iter().copied()).chain(witness.flow.tokens.iter().flat_map(|token| {
                    token.pieces.iter().flat_map(|piece| piece.predicates.keys().flat_map(|filter| {
                        filter.keys.iter().copied()
                    }))
                })).collect();
            let translated: std::collections::BTreeMap<_, _> = origins.into_iter().map(|origin| {
                let bound = match origin {
                    Origin::External(value) => actual(value).map(|value| {
                        flow.full_deps[value.as_u32() as usize].clone()
                    }),
                    Origin::Projection(value, kind) => {
                        let identity = witness.flow.identities[value.as_u32() as usize];
                        let base = match identity {
                            Some(Origin::External(base)) => base,
                            _ => value,
                        };
                        if let Some(value) = actual(base) {
                            Some(match kind {
                                ProjectionKind::Field(declaration, index) => {
                                    self.field(flow, value, FieldRef { declaration, index })
                                }
                                _ => flow.refs[value.as_u32() as usize].union(
                                    &flow.full_deps[value.as_u32() as usize]).copied().collect(),
                            })
                        } else {
                            // Nested projections may use an intermediate SSA base.
                            // Follow its graph back to formals; unknown roots use
                            // the full call-argument bound below.
                            self.defer_projection_bound(&witness.flow, value, &actual, flow)
                        }
                    }
                    _ => None,
                };
                let mut refs = bound.unwrap_or_else(|| {
                    let mut refs = external.clone();
                    refs.insert(key);
                    refs
                });
                if let Origin::External(parameter) = origin
                    && let Some(value) = actual(parameter) {
                    refs.extend(&flow.refs[value.as_u32() as usize]);
                }
                (origin, refs)
            }).collect();
            let translate = |origin: &Origin| translated.get(origin).cloned().unwrap_or_else(|| {
                let mut refs = external.clone();
                refs.insert(key);
                refs
            });
            for value in &witness.values {
                let index = value.as_u32() as usize;
                let ty = body.values.get(*value).unwrap().ty;
                if self.module.types.get(ty) == Ok(&Type::Service(ServiceType::Own)) {
                    let mut local = false;
                    for piece in &witness.flow.tokens[index].pieces {
                        if matches!(piece.origin, Origin::Input(_)) {
                            if let Some(offered) = call.packet {
                                let mut token = flow.tokens[offered.as_u32() as usize].clone();
                                for (filter, selected) in &piece.predicates {
                                    let keys = filter.keys.iter().flat_map(&translate).collect();
                                    let labels = call.args.iter().flat_map(|arg| {
                                        flow.contained[arg.value.as_u32() as usize].iter().copied()
                                    }).collect();
                                    let filter = self.graph_filter(flow, keys, labels, filter.fresh);
                                    let (rest, part) = self.partition_filter(&token, filter);
                                    token = if *selected { part } else { rest };
                                }
                                packet.pieces.extend(token.pieces);
                            }
                        } else {
                            local = true;
                        }
                    }
                    if local {
                        let mut piece = Piece::new(key, AdmissionProvenance::Bound);
                        piece.family = true;
                        piece.predicates.insert(Filter {
                            keys: BTreeSet::from([key]), fresh: false, possible: BTreeSet::new(),
                            labels: SharedSet::new(), stale: false, whole_future: false, state: 0,
                        }, true);
                        packet.pieces.insert(piece);
                    }
                } else if self.module.types.get(ty) == Ok(&Type::Service(ServiceType::Deps)) {
                    deps.extend(witness.flow.refs[index].iter().flat_map(&translate));
                } else {
                    refs.extend(witness.flow.refs[index].iter().flat_map(&translate));
                    deps.extend(witness.flow.full_deps[index].iter().flat_map(&translate));
                    for origin in &witness.flow.refs[index] {
                        if let Origin::External(parameter) = origin && let Some(value) = actual(*parameter) {
                            contained.extend(&flow.contained[value.as_u32() as usize]);
                        }
                    }
                }
            }
        }
        for (info, slot) in schema.results.iter().zip(slots) {
            match info.name {
                SlotName::Value => {
                    slot.refs = refs.clone();
                    slot.requirements = deps.clone();
                    slot.contained = contained.clone();
                    slot.identity = if refs.len() == 1 { refs.first().copied() } else { None };
                }
                SlotName::Packet => slot.token = packet.clone(),
                SlotName::Deps => slot.refs = deps.clone(),
                _ => {}
            }
        }
    }
}
