use super::*;

impl Engine<'_, '_> {
    pub(super) fn scope_operation(
        &mut self, block: BlockId, op: &Operation, outcome: Outcome, before: &Flow, flow: &mut Flow,
    ) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("scopes::scope_operation");
        if outcome != Outcome::Normal {
            return;
        }
        let closing = self.has_closing_token(before);
        if closing && !matches!(op, Operation::Cleanup(_) | Operation::Exit(ExitOp::Abort { .. })
            | Operation::Const(_) | Operation::Core(CoreOp::FailSecondary { .. } | CoreOp::FailCaused { .. })) {
            let resource = op.operand_refs().iter().any(|operand| match operand.reference {
                OperandRef::Value(value) => self.is_managed(value) || linear(self.module, self.body, value),
                _ => true,
            });
            if resource {
                self.error(R::B103, "ordinary resource access/adoption cannot reenter a running cleanup plan");
            }
        }
        if !before.entry_accepted {
            if let Operation::Ownership(OwnershipOp::Accept { state, packet }) = op {
                let input = match self.declaration.signature.ownership_abi {
                    OwnershipAbi::Explicit { input_packet, .. } => self.body.blocks.get(self.body.entry).ok()
                        .and_then(|b| b.params.get(usize::from(input_packet))).copied(),
                    // C-2: a D21 async body accepts the input packet of its start continuation.
                    _ => self.entry_values().into_iter()
                        .find(|v| self.body.values.get(*v).ok().and_then(|v| self.module.types.get(v.ty).ok())
                            == Some(&Type::Service(ServiceType::Own))),
                };
                let accepted = input.is_some_and(|input| before.tokens[packet.as_u32() as usize]
                    .sources.contains(&input));
                let root = scope(self.module, self.body, *state).and_then(|s| self.module.scopes.get(s).ok());
                if !accepted || root.is_none_or(|s| s.kind != ScopeKind::Action || s.parent.is_some()) {
                    self.error(R::B51, "call inputs must be accepted into the callee action root");
                } else { flow.entry_accepted = true; }
            } else if !matches!(op, Operation::Ownership(OwnershipOp::ScopeBegin { .. }))
                // C-2: the start resume of a D21 body restores the entry values that the acceptance admits.
                && !(self.frame_body()
                    && matches!(op, Operation::Async(AsyncOp::FrameResume { continuation: 0, .. }))) {
                self.error(R::B51, "callee must accept fresh inputs before evaluating its body");
            }
        }
        if self.module.profile != Profile::High {
            let prepared = match op {
                Operation::Exit(ExitOp::Prepare { exit }) => Some(exit),
                Operation::Exit(ExitOp::Replace { replacement, .. }) => Some(replacement),
                _ => None,
            };
            if let Some(exit) = prepared
            && let Some(value) = exit.value {
                let (keys, _) = self.selection(before, exit.selection);
                if keys != before.refs[value.as_u32() as usize] {
                    self.error(R::B101, "pending selection must describe the current value's exact retention");
                }
            }
        }
        if let Operation::Exit(ExitOp::Commit { pending, result, destination }) = op {
            let token = &before.tokens[pending.as_u32() as usize];
            if token.pending_kind.is_none() || token.pending_type != *result {
                self.error(R::B101, "exit.commit payload type must match its checked prepare/replacement");
            }
            let destination_ok = match (token.pending_kind, destination) {
                (Some(ExitKind::Return), ExitDestination::Caller) => true,
                (Some(ExitKind::Last | ExitKind::Next), ExitDestination::Loop(boundary)) => {
                    token.pending_boundary == Some(*boundary)
                }
                (Some(ExitKind::Fallthrough), ExitDestination::Scope(scope)) => {
                    let mut parent = token.pending_boundary
                        .and_then(|id| self.body.boundaries.get(id).ok())
                        .and_then(|boundary| self.module.scopes.get(boundary.scope).ok())
                        .and_then(|scope| scope.parent);
                    let mut nearest = None;
                    while let Some(ancestor) = parent {
                        let Some(materialized) = self.materialized_scope(ancestor) else {
                            return;
                        };
                        if materialized {
                            nearest = Some(ancestor);
                            break;
                        }
                        parent = self.module.scopes.get(ancestor).ok().and_then(|scope| scope.parent);
                    }
                    nearest == Some(*scope) && before.scopes.get(scope) == Some(&1)
                }
                _ => false,
            };
            if !destination_ok {
                self.error(R::B101, "exit.commit destination differs from its pending exit kind/boundary");
            }
            if before.active_failure != 0 {
                self.error(R::B101, "exit.commit cannot publish a reserve while failure/cancel is active");
            }
            let reservation = &before.tokens[pending.as_u32() as usize];
            if !reservation.reserve.is_empty() {
                self.error(R::B101, "exit.commit precedes completion of its lexical cleanup boundary");
            }
            self.live_use(before, *pending);
        }
        if let Operation::Ownership(OwnershipOp::ScopeBegin { scope }) = op {
            if flow.scopes.get(scope) == Some(&1) {
                self.error(R::B41, "scope is already open");
            }
            let mut parent = self.module.scopes.get(*scope).ok().and_then(|scope| scope.parent);
            while let Some(ancestor) = parent {
                let Some(materialized) = self.materialized_scope(ancestor) else {
                    return;
                };
                if materialized && flow.scopes.get(&ancestor) != Some(&1) {
                    self.error(R::B41, "nested scope opened without its materialized lexical ancestor");
                }
                parent = self.module.scopes.get(ancestor).ok().and_then(|scope| scope.parent);
            }
            flow.scopes.insert(*scope, 1);
            flow.ended.remove(scope);
            flow.bindings.retain(|binding| {
                self.body.bindings.get(*binding).is_ok_and(|binding| binding.scope != *scope)
            });
        }
        if let Operation::Ownership(OwnershipOp::Bind { binding } | OwnershipOp::Rebind { binding }) = op {
            let declared = self.body.bindings.get(binding.binding).ok();
            if declared.is_none_or(|decl| decl.scope != binding.original_scope)
                || scope(self.module, self.body, binding.state) != Some(binding.original_scope) {
                self.error(R::B43, "binding history must remain in its original owner's journal");
            }
            let rebind = matches!(op, Operation::Ownership(OwnershipOp::Rebind { .. }));
            if rebind != before.bindings.contains(&binding.binding) {
                self.error(R::B43, "bind/rebind does not match the binding's lexical history");
            }
            flow.bindings.insert(binding.binding);
        }
        if let Operation::Ownership(OwnershipOp::End { packet, boundary }) = op
            && let Ok(boundary) = self.body.boundaries.get(*boundary) {
            if boundary.kind == BoundaryKind::Lexical {
                flow.ended.insert(boundary.scope);
            }
            for piece in &before.tokens[packet.as_u32() as usize].pieces {
                if piece.owner.is_some_and(|owner| owner != boundary.scope)
                    || (piece.owner.is_some() && boundary.kind == BoundaryKind::Call) {
                    self.error(R::B42, "end cannot shorten a lexical owner's cleanup boundary");
                }
            }
        }
        if let Some(lexical) = self.body.blocks.get(block).ok().map(|b| b.lexical_scope) {
            let closed = before.scopes.iter().filter(|(_, status)| **status == 2).map(|(scope, _)| scope)
                .chain(&before.ended)
                .any(|scope| super::super::high::inside(self.module, lexical, *scope, self.state));
            let empty_transport = matches!(op, Operation::Ownership(OwnershipOp::Move { .. }
                | OwnershipOp::Join { .. } | OwnershipOp::Split { .. }))
                && op.operand_refs().iter().all(|operand| match operand.reference {
                    OperandRef::Value(value) if linear(self.module, self.body, value) => {
                        before.tokens[value.as_u32() as usize].pieces.is_empty()
                    }
                    _ => true,
                });
            if closed && !empty_transport && !matches!(op,
                Operation::Ownership(OwnershipOp::ScopeEnd { .. } | OwnershipOp::ScopeUnwind { .. }
                    | OwnershipOp::Empty {}) | Operation::Exit(_) | Operation::Core(CoreOp::FailSecondary { .. }
                    | CoreOp::Undef { .. }))
                // D20, C-2: resuming after a suspending close and publishing the outcome end a D21 body.
                && !(self.frame_body()
                    && matches!(op, Operation::Async(AsyncOp::FrameResume { .. } | AsyncOp::FuturePublish { .. }))) {
                self.error(R::B42, "ordinary execution continues after its lexical cleanup boundary");
            }
        }
    }
    pub(super) fn close(
        &mut self, before: &Flow, flow: &mut Flow, input: ValueId, reserve: Option<ValueId>,
    ) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("scopes::close");
        let token = &before.tokens[input.as_u32() as usize];
        self.invalidate(before, flow, &token.pieces.iter().cloned().collect::<Vec<_>>());
        for piece in &token.pieces {
            flow.dead.insert(piece.origin);
            flow.closed_inputs.insert(piece.clone());
        }
        if let Some(scope) = scope(self.module, self.body, input) {
            for value in before.token_values() {
                if !self.scan_work(1) {
                    return;
                }
                if value == input || Some(value) == reserve
                    || before.availability[value.as_u32() as usize] & AVAILABLE == 0 { continue; }
                let held = &before.tokens[value.as_u32() as usize];
                if held.reserve.contains(&scope) || (held.scope == Some(scope) && !held.pin_deps.is_empty())
                    || held.pieces.iter().any(|piece| piece.owner == Some(scope)) {
                    self.error(R::B101, "closing scope has an unreserved part in another linear carrier");
                }
            }
            for slot in before.frames.values().flat_map(|layout| layout.values()) {
                if slot.token.reserve.contains(&scope)
                    || slot.token.pieces.iter().any(|piece| piece.owner == Some(scope)) {
                    self.error(R::B101, "closing scope still has an unreserved owned part saved in a frame");
                }
            }
            if before.scopes.get(&scope) != Some(&1) {
                self.error(R::B41, "scope closes more than once");
            }
            if before.scopes.iter().any(|(child, status)| *status == 1 && *child != scope
                && super::super::high::inside(self.module, *child, scope, self.state)) {
                self.error(R::B41, "outer scope closes while a nested lexical scope is open");
            }
            // #684: the duties of enclosing scopes pass through a nested close untouched.
            let available: BTreeSet<_> = before.token_values().filter(|value| {
                let token = &before.tokens[value.as_u32() as usize];
                before.availability[value.as_u32() as usize] == AVAILABLE && token.service.is_some()
                    && !self.enclosing_duty(token, scope)
            }).collect();
            if !available.is_empty() {
                // A `dynamic` entry covers runtime-admitted entries, never a static token (D25.3).
                let schedule = self.body.service_schedules.iter().find(|schedule| {
                    let duties: Vec<_> = schedule.duties.iter().filter_map(|duty| duty.value()).collect();
                    schedule.owner == scope && duties.iter().copied().collect::<BTreeSet<_>>() == available
                        && duties.len() == available.len()
                        && self.module.protocols.get(schedule.protocol)
                            .is_ok_and(|protocol| protocol.kind == ProtocolKind::Services)
                });
                if schedule.is_none() {
                    self.error(R::B106, "abstract close needs complete, unique service schedule coverage");
                }
                for value in available {
                    if self.await_handle(before, value) {
                        self.error(R::B91, "a close settles an await task or receipt instead of await.unwind (C-3)");
                    }
                    self.use_value(flow, value, OperandMode::Consume, R::B106);
                    let pieces: Vec<_> = before.tokens[value.as_u32() as usize].pieces.iter().cloned().collect();
                    self.invalidate(before, flow, &pieces);
                    flow.dead.extend(pieces.iter().map(|piece| piece.origin));
                    flow.closed_inputs.extend(pieces);
                    if let Some(cache) = before.tokens[value.as_u32() as usize].cache {
                        flow.dead.insert(cache);
                    }
                }
            }
            flow.scopes.insert(scope, 2);

        }
    }
    pub(super) fn stage(
        &mut self, op: &Operation, outcome: Outcome, before: &Flow, flow: &mut Flow,
        schema: &OutcomeContract, slots: &mut [Slot],
    ) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("scopes::stage");
        let Operation::Ownership(OwnershipOp::Stage { stage }) = op else {
            return;
        };
        if outcome != Outcome::Normal {
            return;
        }
        let target = match stage.states { TransferStates::Same(v) => v, TransferStates::Between { to, .. } => to };
        if scope(self.module, self.body, target) != Some(stage.action)
            || !self.module.scopes.get(stage.action).is_ok_and(|s| s.kind == ScopeKind::Action) {
            self.error(R::B43, "staging history must belong to the action owner");
        }
        for (i, slot) in slots.iter_mut().enumerate() {
            slot.token = match stage.states {
                TransferStates::Same(v) => before.tokens[v.as_u32() as usize].clone(),
                TransferStates::Between { from, to } => {
                    let v = if schema.results[i].name == SlotName::From { from } else { to };
                    before.tokens[v.as_u32() as usize].clone()
                }
            };
            if slot.token.scope == Some(stage.action) {
                for piece in &before.tokens[stage.resource.packet.as_u32() as usize].pieces {
                    let mut piece = piece.clone();
                    piece.owner = Some(stage.action);
                    piece.journal = true;
                    piece.provenance = provenance_bit(AdmissionProvenance::StagedOrCarrierOwned);
                    flow.provenance.insert(piece.origin, piece.provenance);
                    slot.token.pieces.insert(piece);
                }
            }
        }
    }
}
