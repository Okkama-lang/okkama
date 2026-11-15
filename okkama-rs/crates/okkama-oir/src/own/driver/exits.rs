use super::*;

impl Driver<'_, '_> {
    pub(super) fn prepare(
        &mut self,
        mut point: Landing,
        value: Option<ValueId>,
        destination: Option<ScopeId>,
    ) -> Result<Landing, OwnError> {
        let replaces = matches!(self.high.blocks.get(self.current_high).map_err(arena)?.term.kind,
            TermKind::RetDefer { tag: DeferReturnTag::Replace(_), .. })
            || point.env.cleanup.defers.values().flatten().any(|registration| {
                matches!(&self.high.insts.get(*registration).unwrap().op,
                    Operation::Lexical(LexicalOp::DeferRegister { action, .. }) if !action.replacements.is_empty())
            });
        let managed = value.is_some_and(|value| self.managed(self.high.values.get(value).unwrap().ty));
        if (!managed && !replaces) || point.env.scopes.is_empty() {
            return Ok(point);
        }
        let scope = self.body.blocks.get(point.block).map_err(arena)?.lexical_scope;
        let owner = destination.unwrap_or(stack(self.module, scope)?[0]);
        let mut original = None;
        if destination.is_some() {
            for inst in &self.high.blocks.get(self.current_high).map_err(arena)?.insts {
                if let Operation::Exit(ExitOp::Prepare { exit }) = &self.high.insts.get(*inst).map_err(arena)?.op
                    && exit.value == value && self.high.boundaries.get(exit.boundary).map_err(arena)?.scope == owner {
                    original = Some((exit.kind, exit.boundary));
                }
            }
        }
        let (kind, boundary) = if let Some(original) = original {
            original
        } else {
            let boundary = self.body.boundaries.alloc(Boundary { scope: owner, kind: BoundaryKind::Lexical })
                .map_err(arena)?;
            (if destination.is_some() { ExitKind::Fallthrough } else { ExitKind::Return }, boundary)
        };
        point = self.emit(point, Operation::Ownership(OwnershipOp::Empty {}))?;
        let packet = point.result(SlotName::Packet)?;
        let latest = value.map(|value| point.env.value(value)).transpose()?;
        let selection = self.body.selections.alloc(match latest {
            Some(latest) => SelectionExpr::Value(latest.value),
            None => SelectionExpr::Union(vec![]),
        }).map_err(arena)?;
        point = self.emit(point, Operation::Exit(ExitOp::Prepare { exit: PreparedExit {
            kind,
            value: latest.map(|latest| latest.value), packet: Some(packet),
            deps: latest.and_then(|latest| latest.deps), selection, boundary,
        } }))?;
        point.env.cleanup.wanted = value;
        point.env.cleanup.returning = destination.is_none();
        point.env.cleanup.destination = Some(if matches!(kind, ExitKind::Last | ExitKind::Next) {
            ExitDestination::Loop(boundary)
        } else {
            destination.map_or(ExitDestination::Caller, ExitDestination::Scope)
        });
        Ok(point)
    }

    pub(super) fn prepare_edge(
        &mut self,
        mut point: Landing,
        values: &[ValueId],
        destination: ScopeId,
    ) -> Result<Landing, OwnError> {
        if values.len() <= 1 {
            return self.prepare(point, values.first().copied(), Some(destination));
        }
        let mut selections = vec![];
        for original in values {
            let value = point.env.value(*original)?.value;
            selections.push(self.body.selections.alloc(SelectionExpr::Value(value)).map_err(arena)?);
        }
        let selection = self.body.selections.alloc(SelectionExpr::Union(selections)).map_err(arena)?;
        // An empty, statically proved partition materializes the complete union
        // deps: with no owned keys its external remainder is the entire D.
        point = self.emit(point, Operation::Ownership(OwnershipOp::Empty {}))?;
        let packet = point.result(SlotName::Packet)?;
        point = self.emit(point, Operation::Ownership(OwnershipOp::Split { packet, selection }))?;
        let deps = point.result(SlotName::ExternalDeps)?;
        let packets = vec![point.result(SlotName::Rest)?, point.result(SlotName::Selected)?];
        point = self.emit(point, Operation::Ownership(OwnershipOp::Join { packets }))?;
        let packet = point.result(SlotName::Packet)?;
        let boundary = self.body.boundaries.alloc(Boundary {
            scope: destination, kind: BoundaryKind::Lexical,
        }).map_err(arena)?;
        point = self.emit(point, Operation::Exit(ExitOp::Prepare { exit: PreparedExit {
            kind: ExitKind::Fallthrough, value: None, packet: Some(packet), deps: Some(deps), selection, boundary,
        } }))?;
        point.env.cleanup.carried = values.to_vec();
        point.env.cleanup.returning = false;
        Ok(point)
    }

    pub(super) fn extend(&mut self, mut point: Landing, scope: ScopeId) -> Result<Landing, OwnError> {
        let mut values = vec![];
        if let Some(replacement) = point.env.cleanup.replacement {
            values.push(replacement.value);
        } else {
            for original in point.env.cleanup.wanted.iter().chain(&point.env.cleanup.carried) {
                values.push(point.env.value(*original)?.value);
            }
        }
        let packet = if !values.is_empty() {
            let mut selections = vec![];
            for value in values {
                selections.push(self.body.selections.alloc(SelectionExpr::Value(value)).map_err(arena)?);
            }
            let selection = if selections.len() == 1 {
                selections[0]
            } else {
                self.body.selections.alloc(SelectionExpr::Union(selections)).map_err(arena)?
            };
            let state = point.env.state(scope)?;
            point = self.emit(point, Operation::Ownership(OwnershipOp::ScopeSplit { state, selection }))?;
            point.result(SlotName::Selected)?
        } else {
            point = self.emit(point, Operation::Ownership(OwnershipOp::Empty {}))?;
            point.result(SlotName::Packet)?
        };
        let pending = point.env.cleanup.pending.ok_or_else(|| flow_error("selection lost reserve"))?;
        self.emit(point, Operation::Exit(ExitOp::Extend { pending, packet }))
    }

    pub(super) fn commit(
        &mut self,
        point: Landing,
        destination: ExitDestination,
        result: Option<TypeId>,
    ) -> Result<Landing, OwnError> {
        let pending = point.env.cleanup.pending.ok_or_else(|| flow_error("commit has no reserve"))?;
        let latest = point.env.cleanup.replacement.or(point.env.cleanup.wanted
            .map(|original| point.env.value(original)).transpose()?);
        let recipes = if let Some(latest) = latest {
            if let Some(deps) = latest.deps {
                let recipe = self.body.deps.alloc(DepExpr::FromSsa(deps)).map_err(arena)?;
                vec![(SlotName::Value, recipe)]
            } else {
                vec![]
            }
        } else {
            vec![]
        };
        let emitted = self.emitter.emit_with_result_deps(self.module, &mut self.body, point.block, point.env,
            Operation::Exit(ExitOp::Commit { pending, destination, result }), &recipes)?;
        Ok(emitted.normal)
    }
}

fn present(quantity: &domain::Quantity) -> bool {
    !matches!(quantity.presence, domain::Presence::Bottom | domain::Presence::Absent)
}

fn crosses(
    module: &Module,
    flow: &domain::FlowState,
    scope: ScopeId,
    values: &[ValueId],
    budget: &mut Budget<'_>,
) -> Result<bool, OwnError> {
    let deps: BTreeSet<_> = values.iter().flat_map(|value| {
        flow.values[value.as_u32() as usize].deps.iter().copied()
    }).collect();
    budget.work(1 + deps.len() + flow.journals.len())?;
    for ((owner, atom), quantity) in &flow.journals {
        if present(quantity) && deps.contains(atom) {
            let owners = stack(module, *owner)?;
            budget.work(owners.len())?;
            if owners.contains(&scope) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// A source preparation marker is not an executable reserve. Remove its sole
/// elision blocker only after the finite domain proves both the local journal
/// and every reserve crossing this boundary empty. Closed-inner reserves count.
pub(super) fn empty_reserve_scopes(
    module: &Module,
    high: &Body,
    graph: &Graph,
    flow: &analyze::FunctionAnalysis,
    analysis: &OwnAnalysis,
    budget: &mut Budget<'_>,
) -> Result<BTreeSet<ScopeId>, OwnError> {
    let function = module.scopes.get(high.blocks.get(high.entry).map_err(arena)?.lexical_scope)
        .map_err(arena)?.function;
    let mut elided = BTreeSet::new();
    for (scope, facts) in &analysis.scopes {
        if module.scopes.get(*scope).map_err(arena)?.function != function
            || facts.blockers != BTreeSet::from([ElisionBlocker::Reserve]) {
            continue;
        }
        let owned = flow.entries.values().chain(flow.before_edges.values()).any(|state| {
            state.journals.iter().any(|((owner, _), quantity)| *owner == *scope && present(quantity))
        });
        let facts = flow.entries.values().chain(flow.before_edges.values())
            .map(|state| 1 + state.journals.len()).sum::<usize>();
        budget.work(1 + facts)?;
        if owned {
            continue;
        }
        let mut reserved = false;
        for edge in graph.edges.iter().filter(|edge| !edge.unwind && edge.plan.close.contains(scope)) {
            if let Some(state) = flow.before_edges.get(&edge.source)
                && crosses(module, state, *scope, &edge.args, budget)? {
                reserved = true;
                break;
            }
        }
        if !reserved {
            for (source, state) in &flow.before_edges {
                let data = high.blocks.get(*source).map_err(arena)?;
                if let TermKind::Ret { values } | TermKind::RetDefer { values, .. } = &data.term.kind
                    && stack(module, data.lexical_scope)?.contains(scope)
                    && crosses(module, state, *scope, values, budget)? {
                    reserved = true;
                    break;
                }
            }
        }
        if !reserved {
            elided.insert(*scope);
        }
    }
    Ok(elided)
}
