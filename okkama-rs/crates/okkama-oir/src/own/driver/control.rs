use super::*;

impl Driver<'_, '_> {
    pub(super) fn failure_term(&mut self, point: Landing, term: &TermKind) -> Result<(), OwnError> {
        let scope = self.body.blocks.get(point.block).map_err(arena)?.lexical_scope;
        let entry_schema = OutcomeContract {
            results: vec![], transfers: vec![], postconditions: vec![], retention: vec![],
        };
        let (entry, edge) = self.emitter.landing(self.module, &mut self.body, scope, &point.env,
            &BTreeSet::new(), &entry_schema, Outcome::Normal, &Operation::Ownership(OwnershipOp::Empty {}), &[])?;
        let args = edge.args.into_iter().map(|arg| match arg {
            EdgeArg::Value(value) => Ok(value),
            _ => Err(flow_error("failure entry contains a produced slot")),
        }).collect::<Result<_, _>>()?;
        self.body.blocks.get_mut(point.block).map_err(arena)?.term.kind = TermKind::Br(Target {
            block: entry.block, args,
        });
        let point = entry;
        let schema = OutcomeContract {
            results: vec![ResultSlot::service(SlotName::Failure, ServiceType::Failure)],
            transfers: vec![], postconditions: vec![], retention: vec![],
        };
        let scope = self.body.blocks.get(point.block).map_err(arena)?.lexical_scope;
        let mut term = term.clone();
        let mut missing = false;
        term.rewrite(&mut Rename { env: &point.env, missing: &mut missing });
        if missing {
            return Err(flow_error("failure operand has no reaching definition"));
        }
        // The ordinary unwind ABI appends its implicit failure after all
        // entry-visible arguments, unlike invoke.own's produced-slot ordering.
        let (landing, mut edge) = self.emitter.landing(self.module, &mut self.body, scope, &point.env,
            &BTreeSet::new(), &schema, Outcome::Unwind, &Operation::Ownership(OwnershipOp::Empty {}), &[])?;
        let data = self.body.blocks.get_mut(landing.block).map_err(arena)?;
        let failure = data.params.remove(0);
        data.params.push(failure);
        for (index, parameter) in data.params.iter().enumerate() {
            self.body.values.get_mut(*parameter).map_err(arena)?.def = Definition::BlockParam {
                block: landing.block, index: u32::try_from(index).map_err(|_| flow_error("too many parameters"))?,
            };
        }
        edge.args.remove(0);
        let args = edge.args.into_iter().map(|arg| match arg {
            EdgeArg::Value(value) => Ok(value),
            _ => Err(flow_error("ordinary failure forwards a produced slot")),
        }).collect::<Result<_, _>>()?;
        self.body.blocks.get_mut(point.block).map_err(arena)?.unwind = Some(BlockUnwind {
            target: Target { block: landing.block, args },
        });
        self.body.blocks.get_mut(point.block).map_err(arena)?.term.kind = term;
        self.failures.push(FailureRoute { point: landing, target: self.current_unwind.clone() });
        Ok(())
    }

    pub(super) fn catch(
        &mut self,
        point: Landing,
        failure: ValueId,
        ty: TypeId,
        handler: &InvokeEdge,
        next: &Target,
    ) -> Result<(), OwnError> {
        let scope = self.body.blocks.get(point.block).map_err(arena)?.lexical_scope;
        let schema = OutcomeContract {
            results: vec![ResultSlot::new(SlotName::Value, ResultType::Exact(ty))],
            transfers: vec![], postconditions: vec![], retention: vec![],
        };
        let (mut handled, edge) = self.emitter.landing(self.module, &mut self.body, scope, &point.env,
            &BTreeSet::new(), &schema, Outcome::Normal, &Operation::Ownership(OwnershipOp::Empty {}), &[])?;
        handled.env.cleanup.failure = None;
        let payload = handled.result(SlotName::Value)?;
        let mut latest = LatestValue { value: payload, deps: None };
        if self.managed(ty) {
            handled = self.emit(handled, Operation::Ownership(OwnershipOp::Dependencies { value: payload }))?;
            latest.deps = Some(handled.result(SlotName::Value)?);
        }
        let params = &self.high.blocks.get(handler.block).map_err(arena)?.params;
        let mut args = vec![];
        for (parameter, argument) in params.iter().zip(&handler.args) {
            match argument {
                EdgeArg::Value(value) => args.push(*value),
                EdgeArg::Produced(_) => {
                    handled.env.values.insert(*parameter, latest);
                    args.push(*parameter);
                }
            }
        }
        self.connect(handled, &Target { block: handler.block, args }, false, false, 0)?;
        let miss = block(&mut self.body, scope)?;
        let mut missed = point.clone();
        missed.block = miss;
        self.connect(missed, next, false, false, 1)?;
        self.body.blocks.get_mut(point.block).map_err(arena)?.term.kind = TermKind::Catch {
            failure: point.env.value(failure)?.value, ty, handler: edge,
            next: Target { block: miss, args: vec![] },
        };
        Ok(())
    }
}

impl Driver<'_, '_> {
    pub(super) fn prune(&self, point: &mut Landing) {
        let mut needed = self.live_after.clone();
        needed.extend(point.env.cleanup.wanted);
        needed.extend(&point.env.cleanup.carried);
        for prefix in point.env.cleanup.defers.values() {
            for registration in prefix {
                if let Operation::Lexical(LexicalOp::DeferRegister { action, .. }) =
                    &self.high.insts.get(*registration).unwrap().op {
                    needed.extend(&action.captures);
                }
            }
        }
        point.env.values.retain(|value, _| needed.contains(value));
        point.env.bindings.retain(|binding, _| self.captured.contains(binding));
    }
}

pub(super) fn term_values(term: &TermKind) -> BTreeSet<ValueId> {
    match term {
        TermKind::Cond { condition, .. } | TermKind::Switch { value: condition, .. } => BTreeSet::from([*condition]),
        TermKind::Ret { values } | TermKind::RetDefer { values, .. } => values.iter().copied().collect(),
        TermKind::Raise(value) | TermKind::Throw(value) | TermKind::Panic(value) => BTreeSet::from([*value]),
        TermKind::Catch { failure, .. } => BTreeSet::from([*failure]),
        TermKind::Lookup { collection, key, .. } => BTreeSet::from([*collection, *key]),
        _ => BTreeSet::new(),
    }
}

pub(super) fn continuations(
    body: &Body,
    graph: &Graph,
    live: &BTreeMap<BlockId, BTreeSet<ValueId>>,
    budget: &mut Budget<'_>,
) -> Result<BTreeMap<InstId, BTreeSet<ValueId>>, OwnError> {
    let mut result = BTreeMap::new();
    for original in &graph.rpo {
        let data = body.blocks.get(*original).map_err(arena)?;
        let mut needed = term_values(&data.term.kind);
        for edge in &graph.outgoing[original.as_u32() as usize] {
            let edge = &graph.edges[*edge];
            budget.work(1 + edge.args.len() + live[&edge.target].len())?;
            needed.extend(&edge.args);
            needed.extend(live[&edge.target].iter().filter(|value| {
                !body.blocks.get(edge.target).unwrap().params.contains(value)
            }));
        }
        for inst in data.insts.iter().rev() {
            budget.work(1 + needed.len())?;
            budget.memory(64 + needed.len() * 32)?;
            result.insert(*inst, needed.clone());
            let instruction = body.insts.get(*inst).map_err(arena)?;
            for value in &instruction.results {
                needed.remove(value);
            }
            for operand in cfg::operation_uses(&instruction.op, body)
                .map_err(|error| flow_error(format!("continuation operands: {error}")))? {
                needed.insert(operand.value);
            }
        }
    }
    Ok(result)
}

impl Driver<'_, '_> {
    pub(super) fn record_failure(&mut self, mut point: Landing, target: Option<Target>) -> Result<(), OwnError> {
        let target = self.consumed_buffer_target(&point.env, target)?;
        if let Some(primary) = point.env.cleanup.primary_before_failure.take() {
            let cleanup = point.env.cleanup.failure.ok_or_else(|| flow_error("secondary has no failure"))?;
            point = self.emitter.emit(self.module, &mut self.body, point.block, point.env,
                Operation::Core(CoreOp::FailSecondary { primary, cleanup }))?.normal;
            point.env.cleanup.failure = Some(point.result(SlotName::Value)?);
        }
        self.failures.push(FailureRoute { point, target });
        Ok(())
    }
}

impl Driver<'_, '_> {
    /// `m[k] ?? fallback`: the found edge receives the element as a projection
    /// of the collection; the missing edge continues without it.
    pub(super) fn lookup(
        &mut self,
        point: Landing,
        kind: LookupKind,
        collection: ValueId,
        key: ValueId,
        found: &InvokeEdge,
        missing: &Target,
    ) -> Result<(), OwnError> {
        let scope = self.body.blocks.get(point.block).map_err(arena)?.lexical_scope;
        let source = point.env.value(collection)?.value;
        let index = point.env.value(key)?.value;
        let (formula, projection) = match kind {
            LookupKind::Array => (ResultType::Element(source),
                Operation::Collection(CollectionOp::ArrayGet { array: source, index })),
            LookupKind::Map => (ResultType::MapValue(source),
                Operation::Collection(CollectionOp::MapGet { map: source, key: index })),
        };
        let schema = OutcomeContract {
            results: vec![ResultSlot::new(SlotName::Value, formula)],
            transfers: vec![], postconditions: vec![], retention: vec![],
        };
        let (mut handled, edge) = self.emitter.landing(self.module, &mut self.body, scope, &point.env,
            &BTreeSet::new(), &schema, Outcome::Normal, &projection, &[])?;
        let value = handled.result(SlotName::Value)?;
        let mut latest = LatestValue { value, deps: None };
        if self.managed(self.body.values.get(value).map_err(arena)?.ty) {
            handled = self.emit(handled, Operation::Ownership(OwnershipOp::Dependencies { value }))?;
            latest.deps = Some(handled.result(SlotName::Value)?);
        }
        let params = &self.high.blocks.get(found.block).map_err(arena)?.params;
        let mut args = vec![];
        for (parameter, argument) in params.iter().zip(&found.args) {
            match argument {
                EdgeArg::Value(value) => args.push(*value),
                EdgeArg::Produced(_) => {
                    handled.env.values.insert(*parameter, latest);
                    args.push(*parameter);
                }
            }
        }
        self.connect(handled, &Target { block: found.block, args }, false, false, 0)?;
        let miss = block(&mut self.body, scope)?;
        let mut missed = point.clone();
        missed.block = miss;
        self.connect(missed, missing, false, false, 1)?;
        self.body.blocks.get_mut(point.block).map_err(arena)?.term.kind = TermKind::Lookup {
            kind, collection: source, key: index, found: edge, missing: Target { block: miss, args: vec![] },
        };
        Ok(())
    }

    /// High routes every failing site of a block to one handler that abandons
    /// the block's live buffers and re-raises. A failure Own introduces after a
    /// buffer was finished in that block has no buffer left to abandon, and a
    /// failure with a pending exit must unwind with its reserve: both continue
    /// at the handler's own unwind target. Skipped buffers are abandoned on the
    /// failure path itself (`abandon_unrouted`).
    pub(super) fn consumed_buffer_target(
        &self,
        env: &LiveEnv,
        mut target: Option<Target>,
    ) -> Result<Option<Target>, OwnError> {
        while let Some(current) = &target {
            let consumed = current.args.iter().any(|value| !env.values.contains_key(value)
                && self.high.values.get(*value).is_ok_and(|value| {
                    matches!(self.module.types.get(value.ty), Ok(Type::Builder(_) | Type::MapBuilder(..)))
                }));
            let handler = self.high.blocks.get(current.block).map_err(arena)?;
            let abandon = |inst: &InstId| self.high.insts.get(*inst).is_ok_and(|inst| matches!(inst.op,
                Operation::Collection(CollectionOp::BuilderAbandon { .. } | CollectionOp::MapBuilderAbandon { .. })));
            let abandons = !handler.insts.is_empty() && handler.insts.iter().all(abandon)
                && matches!(handler.term.kind, TermKind::Raise(failure) if handler.params.last() == Some(&failure));
            if !consumed && !(abandons && env.cleanup.pending.is_some()) {
                break;
            }
            let Some(next) = &handler.unwind else {
                return Ok(None);
            };
            let args = next.target.args.iter().map(|value| {
                handler.params.iter().position(|parameter| parameter == value)
                    .and_then(|index| current.args.get(index).copied()).unwrap_or(*value)
            }).collect();
            target = Some(Target { block: next.target.block, args });
        }
        Ok(target)
    }

    pub(super) fn switch(
        &mut self,
        point: Landing,
        value: ValueId,
        cases: &[SwitchCase],
        default: &Target,
    ) -> Result<(), OwnError> {
        let scope = self.body.blocks.get(point.block).map_err(arena)?.lexical_scope;
        let mut rewritten = vec![];
        for (ordinal, case) in cases.iter().enumerate() {
            let landing = block(&mut self.body, scope)?;
            let mut branch = point.clone();
            branch.block = landing;
            self.connect(branch, &case.target, false, false, ordinal)?;
            rewritten.push(SwitchCase { value: case.value.clone(), target: Target { block: landing, args: vec![] } });
        }
        let landing = block(&mut self.body, scope)?;
        let mut branch = point.clone();
        branch.block = landing;
        self.connect(branch, default, false, false, cases.len())?;
        self.body.blocks.get_mut(point.block).map_err(arena)?.term.kind = TermKind::Switch {
            value: point.env.value(value)?.value, cases: rewritten, default: Target { block: landing, args: vec![] },
        };
        Ok(())
    }
}
