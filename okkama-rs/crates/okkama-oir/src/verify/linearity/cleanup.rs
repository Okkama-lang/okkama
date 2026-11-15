use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum DispatchKind { None, Some, Success, Pending, Failure }
impl DispatchKind {
    fn results(self) -> Vec<ResultSlot> {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("cleanup::results");
        let outcome = match self {
            Self::None => return vec![],
            Self::Some => Outcome::FailureSome,
            Self::Success => Outcome::ExitSuccess,
            Self::Pending => Outcome::ExitPending,
            Self::Failure => Outcome::ExitFailure,
        };
        dispatch_slots(outcome)
    }
}
struct DispatchEdge { kind: DispatchKind, edge: InvokeEdge }


impl Engine<'_, '_> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn cleanup_operation(
        &mut self, block: BlockId, op: &Operation, outcome: Outcome, before: &Flow, flow: &mut Flow,
        schema: &OutcomeContract, slots: &mut [Slot],
    ) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("cleanup::cleanup_operation");
        if let Operation::Ownership(OwnershipOp::ScopeEnd { state, reserve, effects }
            | OwnershipOp::ScopeUnwind { state, reserve, effects, .. }) = op {
            self.scope_effects(block, op, *state, *reserve, effects, before);
        }
        if outcome != Outcome::Normal {
            return;
        }
        if let Operation::Exit(ExitOp::Abort { pending, cleanup }) = op {
            let mut token = before.tokens[cleanup.as_u32() as usize].clone();
            if let Some(fact) = &mut token.cleanup {
                if fact.pending {
                    self.error(R::B101,
                        "abort must preserve the executed prefix and cannot duplicate an owned reserve");
                }
                let incoming = &before.tokens[pending.as_u32() as usize].pieces;
                if !fact.shield || fact.awaiting_failure
                    || incoming.iter().any(|piece| fact.executed.contains(&piece.origin)
                        || token.pieces.iter().any(|old| old.overlaps(piece))) {
                    self.error(R::B101, "abort requires shielded disjoint reserve and a settled executed prefix");
                }
                if before.active_failure == 0 && fact.failures == 0 {
                    self.error(R::B101, "exit.abort requires an actual failure or delivered cancellation");
                }
                if fact.failures == 0 {
                    fact.failures = before.active_failure;
                    fact.failure_origin = before.active_failure_origin;
                }
                token.pieces.extend(incoming.iter().cloned());
                fact.inventory.extend(incoming.iter().cloned());
                fact.phase = 1;
                fact.plan = None;
                fact.aborted = true;
            } else { self.error(R::B101, "exit.abort needs a live cleanup automaton"); }
            if let Some(slot) = slots.first_mut() {
                slot.token = token;
            }
            return;
        }
        let Operation::Cleanup(op) = op else {
            return;
        };
        if let CleanupOp::Begin { state, pending, failure, close, .. } = op {
            let mut token = before.tokens[state.as_u32() as usize].clone();
            let reserve = pending.map(|v| before.tokens[v.as_u32() as usize].pieces.clone()).unwrap_or_default();
            let failures = failure.map_or(0, |v| before.failures[v.as_u32() as usize]);
            let failure_origin = failure.and_then(|value| before.tokens[value.as_u32() as usize].failure_origin);
            if before.active_failure_origin.is_some() && before.active_failure_origin != failure_origin {
                self.error(R::B104, "cleanup.begin dropped or substituted the active primary failure/Error");
            }
            let mut fact = CleanupFact {
                aborted: false,
                // Plan §3.4: in a D21 async body of Low the shield is a property of the close, not an
                // operation, so every close begins shielded, including a final close that delivers cancellation.
                // A body without a frame layout keeps the P1 form: G1 carries D9 shielding in begin, and a
                // suspending or cancelling close needs the explicit shield transition.
                phase: 0,
                shield: self.module.profile == Profile::Low && self.body.cleanup.closes.get(*close)
                    .is_ok_and(|close| self.frame_body() || !close.effects.suspends && !close.effects.cancel),
                pending: pending.is_some(),
                inventory: token.pieces.union(&reserve).cloned().collect(), reserve,
                failures, failure_origin, cancel_delivered: false, cancel_open: false, may_fail: false,
                awaiting_failure: false, awaiting_source: None, awaiting_origin: None, selected: None,
                executed: BTreeSet::new(), services: BTreeSet::new(), had_services: false,
                close: *close, plan: None,
            };
            for (value, _) in self.body.values.iter() {
                if before.availability[value.as_u32() as usize] == AVAILABLE
                    && matches!(super::super::later::value_type(self.module, self.body, value),
                        Some(Type::Frame(_) | Type::Service(ServiceType::TaskState | ServiceType::Pin
                            | ServiceType::Receipt | ServiceType::CacheState)))
                    && let Some(key) = before.tokens[value.as_u32() as usize].service {
                    fact.services.insert(key);
                }
            }
            fact.had_services = !fact.services.is_empty();
            let count = self.body.cleanup.schedules.iter().filter(|(_, schedule)| schedule.close == *close).count();
            if count != 1 {
                self.error(R::B106, "cleanup begin requires one typed D16 schedule for its close");
            }
            if let Some(schedule) = self.body.cleanup.schedules.iter()
                .find_map(|(_, schedule)| (schedule.close == *close).then_some(schedule)) {
                let declared: BTreeSet<_> = schedule.holders.iter().filter_map(|value| {
                    before.tokens[value.as_u32() as usize].service
                }).collect();
                let owner_matches = self.body.cleanup.closes.get(*close).is_ok_and(|close| {
                    matches!(close.source, CleanupSource::Scope(owner) if token.scope == Some(owner))
                });
                // D16: a static inventory of duty tokens names its Services protocol; a runtime dictionary is
                // optional and, when present, must be live.
                if let Some(dictionary) = schedule.dictionary {
                    self.live_use(before, dictionary);
                }
                let service_contract = (schedule.holders.is_empty() || owner_matches)
                    && match schedule.protocol {
                    None => schedule.holders.is_empty() && schedule.dictionary.is_none(),
                    Some(protocol) => self.module.protocols.get(protocol)
                        .is_ok_and(|protocol| protocol.kind == ProtocolKind::Services),
                };
                if !service_contract {
                    self.error(R::B106, "service duties need their owner's checked schedule dictionary/protocol");
                }
                if declared != fact.services || declared.len() != schedule.holders.len() {
                    self.error(R::B106, "service schedule must cover every assigned holder exactly once");
                }
            }
            if failures != 0 {
                token.pieces.extend(fact.reserve.iter().cloned());
                fact.reserve.clear();
                fact.pending = false;
            }
            token.cleanup = Some(fact);
            if let Some(pending) = pending {
                token.selection = before.tokens[pending.as_u32() as usize].selection;
                token.pending_type = before.tokens[pending.as_u32() as usize].pending_type;
                token.pending_kind = before.tokens[pending.as_u32() as usize].pending_kind;
                token.pending_boundary = before.tokens[pending.as_u32() as usize].pending_boundary;
                token.reserve = before.tokens[pending.as_u32() as usize].reserve.clone();
            }
            if let Some(slot) = slots.first_mut() {
                slot.token = token;
                if let Some(pending) = pending {
                    let index = pending.as_u32() as usize;
                    slot.refs = before.refs[index].clone();
                    slot.requirements = before.full_deps[index].clone();
                    slot.aliases = before.aliases[index].clone();
                    slot.identity = before.identities[index];
                }
            }
            return;
        }
        let cleanup = match op {
            CleanupOp::Defer { cleanup, .. } | CleanupOp::Settle { cleanup, .. }
            | CleanupOp::Plan { cleanup, .. }
            | CleanupOp::SettleEmpty { cleanup, .. } | CleanupOp::Destroy { cleanup, .. }
            | CleanupOp::DestroyDirect { cleanup, .. } | CleanupOp::RecordFailure { cleanup, .. }
            | CleanupOp::Shield { cleanup } | CleanupOp::DeliverCancel { cleanup }
            | CleanupOp::Finish { cleanup } => *cleanup,
            _ => return,
        };
        let mut token = before.tokens[cleanup.as_u32() as usize].clone();
        let Some(mut fact) = token.cleanup.take() else {
            self.error(R::B102, "cleanup step has no checked begin/phase state");
            return;
        };
        if !fact.shield && !matches!(op, CleanupOp::Shield { .. }) {
            self.error(R::B105, "cleanup execution, settlement and cancellation require a shield");
        }
        if fact.awaiting_failure && !matches!(op, CleanupOp::RecordFailure { .. }) {
            self.error(R::B104, "destroy failure state must be dispatched before the next cleanup step");
        }
        match op {
            CleanupOp::Defer { action, .. } => {
                let target = self.body.values.get(*action).ok().and_then(|value| match value.def {
                    Definition::InstResult { inst, .. } => self.body.insts.get(inst).ok(),
                    _ => None,
                }).and_then(|inst| match &inst.op {
                    Operation::Core(CoreOp::FnBind { function, .. } | CoreOp::FnClosure { function, .. }) => {
                        self.module.functions.get(*function).ok()
                    }
                    _ => None,
                }).and_then(|function| function.summary).and_then(|id| self.module.summaries.get(id).ok());
                if target.is_none_or(|summary| !summary.contract.failures.is_empty()
                    || summary.contract.suspends || summary.contract.async_destroy) {
                    self.error(R::B104,
                        "effectful or replacing defers require explicit shielded invoke.defer outcomes");
                }
                if fact.phase != 0 {
                    self.error(R::B102, "defer cannot execute after service settlement");
                }
            }
            CleanupOp::Shield { .. } => fact.shield = true,
            CleanupOp::Settle { holder, protocol, .. } => {
                let kind = match super::super::later::value_type(self.module, self.body, *holder) {
                    Some(Type::Service(ServiceType::TaskState)) => HolderKind::Group,
                    Some(Type::Service(ServiceType::Receipt)) => HolderKind::Receipt,
                    Some(Type::Service(ServiceType::Pin)) => HolderKind::Pin,
                    Some(Type::Service(ServiceType::CacheState)) => HolderKind::Cache,
                    Some(Type::Frame(_)) => HolderKind::Frame,
                    _ => HolderKind::Scope,
                };
                if self.module.protocols.get(*protocol).ok().is_none_or(|protocol| !protocol.rules.iter().any(|rule| {
                    rule.transition == ProtocolTransition::Settle && rule.from == kind
                })) {
                    self.error(R::B106, "settlement protocol does not cover this holder kind");
                }
                if fact.phase > 1 {
                    self.error(R::B106, "service settlement must precede the serial plan");
                }
                let key = before.tokens[holder.as_u32() as usize].service;
                if !key.is_some_and(|key| fact.services.remove(&key)) {
                    if key.is_some() && self.scheduled(before, fact.close, key) {
                        self.error(R::LowDutySettledTwice, "a scheduled service duty is settled once per close");
                    } else {
                        self.error(R::B106, "settlement is not one of this cleanup's outstanding service duties");
                    }
                }
                // D24.2: the settle of a cache duty releases its outcome or closes its unstarted inputs; it never
                // fails in 0.6.0. Other holders keep a captured settle failure.
                let cache = matches!(super::super::later::value_type(self.module, self.body, *holder),
                    Some(Type::Service(ServiceType::CacheState)));
                fact.may_fail |= !(cache && self.module.profile == Profile::Low);
                let incoming = &before.tokens[holder.as_u32() as usize];
                if incoming.frame.and_then(|key| before.frames.get(&key)).is_some_and(|layout| {
                    layout.values().any(|slot| slot.token.saved_type.is_some_and(|ty| {
                        super::super::contracts::linear(self.module, ty)
                    }))
                }) {
                    self.error(R::B95, "saved linear frame slots must enter cancellation before service settlement");
                }
                token.pieces.extend(incoming.pieces.iter().cloned());
                fact.inventory.extend(incoming.pieces.iter().cloned());
                fact.phase = 1;
            }
            CleanupOp::SettleEmpty { schedule, .. } => {
                let matches = self.body.cleanup.schedules.get(*schedule).is_ok_and(|entry| {
                    entry.close == fact.close && entry.holders.is_empty()
                });
                if fact.phase > 1 || !fact.services.is_empty() || !matches {
                    self.error(R::B106, "empty settlement requires this owner's checked empty schedule");
                }
                fact.phase = 1;
            }
            CleanupOp::Plan { plan, .. } => {
                if !fact.shield {
                    self.error(R::B105, "cleanup plan requires a shield");
                }
                if fact.phase != 1 || !fact.services.is_empty() {
                    self.error(R::B106, "every assigned service must settle before planning");
                }
                self.check_plan(before, cleanup, &token, &fact, *plan);
                fact.plan = Some(*plan);
                fact.phase = 2;
            }
            CleanupOp::Destroy { key: selected, .. } => {
                if !fact.shield || fact.phase != 2 || fact.selected.is_none()
                    || !before.refs[selected.as_u32() as usize].iter().any(|key| Some(*key) == fact.selected) {
                    self.error(R::B102, "runtime destroy needs the checked serial selector's selected key");
                }
                if let Some(key) = fact.selected.take() {
                    let family = token.pieces.iter().any(|piece| piece.origin == key && piece.family);
                    if !family {
                token.pieces.retain(|piece| piece.origin != key);
                        flow.dead.insert(key);
                    }
                    fact.executed.insert(key);
                }
                fact.awaiting_failure = true;
                fact.awaiting_source = Some(cleanup);
            }
            CleanupOp::DestroyDirect { identity, .. } => {
                let key = self.identity(before, *identity);
                if !fact.shield {
                    self.error(R::B105, "destroy must execute shielded");
                }
                if fact.phase != 2 {
                    self.error(R::B102, "destroy precedes serial planning");
                }
                if fact.executed.contains(&key) || before.dead.contains(&key) {
                    self.error(R::B103, "destroy cannot reenter or destroy an already dead identity");
                }
                let proof = fact.plan.and_then(|id| self.body.cleanup.plans.get(id).ok())
                    .and_then(|plan| plan.direct).and_then(|id| self.body.cleanup.proofs.get(id).ok());
                if proof.is_none_or(|proof| {
                    proof.slots.iter().find(|slot| !fact.executed.contains(&self.identity(before, slot.identity)))
                        .is_none_or(|slot| self.identity(before, slot.identity) != key)
                }) || !token.pieces.iter().any(|piece| piece.origin == key) {
                    self.error(R::B102, "direct destroy needs the next identity of its complete checked order");
                }
                token.pieces.retain(|piece| piece.origin != key);
                fact.executed.insert(key);
                flow.dead.insert(key);
                fact.awaiting_failure = true;
                fact.awaiting_source = Some(cleanup);
            }
            CleanupOp::RecordFailure { failure, .. } => {
                let kind = before.failures[failure.as_u32() as usize];
                if fact.phase != 0 && (!fact.awaiting_failure || fact.awaiting_origin !=
                    before.tokens[failure.as_u32() as usize].failure_origin) {
                    self.error(R::B104, "cleanup.failure needs this step's current FailureState dispatch");
                }
                if fact.awaiting_source.is_some_and(|source| {
                    !before.tokens[failure.as_u32() as usize].sources.contains(&source)
                }) {
                    self.error(R::B104, "cleanup.failure substituted a different failure/Error view");
                }
                if kind == 0 {
                    self.error(R::B104, "cleanup.failure must record an actual failure view");
                }
                if fact.failures == 0 {
                    fact.failures = kind;
                    fact.failure_origin = before.tokens[failure.as_u32() as usize].failure_origin;
                }
                flow.active_failure = fact.failures;
                flow.active_failure_origin = fact.failure_origin;
                fact.awaiting_failure = false;
                fact.awaiting_source = None;
                fact.awaiting_origin = None;
                // The merge plan already includes WholeReserveOnFailure in its unexecuted suffix.
                if !fact.reserve.is_empty() && fact.plan.and_then(|id| self.body.cleanup.plans.get(id).ok())
                    .is_none_or(|plan| plan.direct.is_some()) {
                    fact.phase = 1;
                    fact.plan = None;
                }
                token.pieces.extend(fact.reserve.iter().cloned());
                fact.reserve.clear();
                fact.pending = false;
            }
            CleanupOp::DeliverCancel { .. } => {
                // D18: only the final successful close of an async action delivers a deferred cancellation;
                // a nested or failing close keeps it for the next await or for the final exit.
                let action = token.scope.and_then(|id| self.module.scopes.get(id).ok());
                let close = self.body.cleanup.closes.get(fact.close).ok();
                if !self.declaration.attrs.contains(&FunctionAttr::Async)
                    || action.is_none_or(|scope| scope.kind != ScopeKind::Action || scope.parent.is_some())
                    || close.is_none_or(|close| !close.effects.cancel || close.active_failure) {
                    self.error(R::LowCancelOutsideFinalExit,
                        "cancellation is delivered only by the final successful close of an async action");
                }
                if self.frame_body() {
                    self.conditional_cancel(&token, &mut fact);
                    token.cleanup = Some(fact);
                    if let Some(slot) = slots.first_mut() {
                        slot.token = token;
                        let index = cleanup.as_u32() as usize;
                        slot.refs = before.refs[index].clone();
                        slot.requirements = before.full_deps[index].clone();
                        slot.aliases = before.aliases[index].clone();
                        slot.identity = before.identities[index];
                    }
                    return;
                }
                if fact.failures == 0 {
                    fact.failures = failure_bit(FailureKind::Cancelled);
                    fact.failure_origin = match self.state.location {
                        L::Instruction { instruction, .. } => Some(Origin::ViewInstruction(instruction, u16::MAX)),
                        _ => None,
                    };
                }
                fact.cancel_delivered = true;
                flow.active_failure = fact.failures;
                flow.active_failure_origin = fact.failure_origin;
                fact.phase = 1;
                fact.plan = None;
                fact.aborted = true;
                token.pieces.extend(fact.reserve.iter().cloned());
                fact.reserve.clear();
                fact.pending = false;
            }
            CleanupOp::Finish { .. } => {
                if fact.phase != 2 || !token.pieces.is_empty() || !fact.services.is_empty() {
                    self.error(R::B102, "cleanup.finish skips an unprocessed remainder or service phase");
                }
                if fact.cancel_open {
                    self.error(R::B105, "an open cancellation delivery is decided by the selector of its plan");
                }
                let destroyed: Vec<_> = fact.inventory.difference(&fact.reserve).cloned().collect();
                self.invalidate(before, flow, &destroyed);
                flow.closed_inputs.extend(destroyed.iter().cloned());
                for piece in &destroyed {
                    if !fact.reserve.iter().any(|reserved| reserved.origin == piece.origin) {
                        flow.dead.insert(piece.origin);
                    }
                }
                if let Some(scope) = token.scope {
                    flow.scopes.insert(scope, 2);
                }
                let finish = match self.state.location {
                    L::Instruction { instruction, .. } => Some(Origin::ViewInstruction(instruction, u16::MAX)),
                    _ => None,
                };
                for slot in slots.iter_mut() {
                    slot.token.sources.insert(cleanup);
                    slot.token.finish_origin = finish;
                }
                for (info, slot) in schema.results.iter().zip(slots.iter_mut()) {
                    match info.ty {
                        ResultType::Service(ServiceType::FailureState) => {
                            slot.failures = if fact.failures != 0 { fact.failures }
                                else if fact.may_fail { 15 } else { 0 };
                            slot.token.failure_origin = fact.failure_origin.or({
                                if fact.may_fail {
                                    match self.state.location {
                                        L::Instruction { instruction, .. } => {
                                            Some(Origin::ViewInstruction(instruction, u16::MAX))
                                        }
                                        _ => None,
                                    }
                                } else { None }
                            });
                        }
                        ResultType::Service(ServiceType::ExitState) => {
                            slot.token.pieces = fact.reserve.clone();
                            slot.token.cleanup = Some(fact.clone());
                            slot.token.scope = token.scope;
                            slot.token.selection = token.selection;
                            slot.token.pending_type = token.pending_type;
                            slot.token.pending_kind = token.pending_kind;
                            slot.token.pending_boundary = token.pending_boundary;
                            slot.token.reserve = token.reserve.clone();
                            if let Some(scope) = token.scope {
                                slot.token.reserve.remove(&scope);
                            }
                            if fact.pending {
                                slot.refs = before.refs[cleanup.as_u32() as usize].clone();
                                slot.requirements = before.full_deps[cleanup.as_u32() as usize].clone();
                                slot.aliases = before.aliases[cleanup.as_u32() as usize].clone();
                                slot.identity = before.identities[cleanup.as_u32() as usize];
                            }
                        }
                        _ => {}
                    }
                }
                return;
            }
            _ => {}
        }
        if fact.awaiting_failure
            && let Some(slot) = slots.get_mut(1) {
            slot.token.sources.insert(cleanup);
            slot.token.failure_origin = Some(Origin::ViewInstruction(
                match self.state.location {
                    L::Instruction { instruction, .. } => instruction,
                    _ => InstId::from_u32(u32::MAX),
                }, u16::MAX));
            fact.awaiting_origin = slot.token.failure_origin;
            slot.failures = match op {
                CleanupOp::DestroyDirect { destroy, .. } => self.module.functions.get(*destroy).ok()
                    .and_then(|function| function.summary).and_then(|id| self.module.summaries.get(id).ok())
                    .map_or(15, |summary| summary.contract.failures.iter().filter(|kind| {
                        **kind != FailureKind::Cancelled
                    }).fold(0, |bits, kind| bits | failure_bit(*kind))),
                _ => self.required_effects(&fact.inventory, before).failures.iter()
                    .filter(|kind| **kind != FailureKind::Cancelled)
                    .fold(0, |bits, kind| bits | failure_bit(*kind)),
            };
        }
        let pending = fact.pending;
        token.cleanup = Some(fact);
        if let Some(slot) = slots.first_mut() {
            slot.token = token;
            if pending {
                let index = cleanup.as_u32() as usize;
                slot.refs = before.refs[index].clone();
                slot.requirements = before.full_deps[index].clone();
                slot.aliases = before.aliases[index].clone();
                slot.identity = before.identities[index];
            }
        }
    }

    /// Champ draft 3 §5: in a D21 body `cleanup.deliver_cancel` delivers only a cancellation that was
    /// requested and not yet delivered, and only without a primary failure. It runs once, after the plan has
    /// drained (OIR-2 §9.3 step 6), so the executed prefix stays executed; a second plan then closes the
    /// reserve if the cancellation was delivered. The next selector forks into both outcomes.
    fn conditional_cancel(&mut self, token: &Token, fact: &mut CleanupFact) {
        if fact.phase != 2 || !token.pieces.is_empty() || fact.awaiting_failure || fact.cancel_delivered {
            self.error(R::B105, "cancellation is delivered once, after the plan has drained");
        }
        fact.cancel_delivered = true;
        fact.phase = 1;
        fact.plan = None;
        if fact.failures == 0 {
            // A primary failure keeps priority; without one the delivery is decided at run time.
            fact.cancel_open = true;
            fact.failure_origin = match self.state.location {
                L::Instruction { instruction, .. } => Some(Origin::ViewInstruction(instruction, u16::MAX)),
                _ => None,
            };
        }
    }

    /// The two outcomes of an open cancellation delivery: not delivered, with the reserve still reserved for
    /// the commit; delivered, with Cancelled as the primary failure and the reserve aborted into the plan.
    /// Each outcome is paired with whether it delivered the cancellation.
    fn cancel_outcomes(&self, token: &Token) -> Vec<(Token, bool)> {
        let Some(fact) = token.cleanup.as_ref().filter(|fact| fact.cancel_open) else {
            return vec![(token.clone(), false)];
        };
        let mut kept = token.clone();
        if let Some(fact) = &mut kept.cleanup {
            fact.cancel_open = false;
            fact.failure_origin = None;
        }
        let mut delivered = token.clone();
        let reserve = fact.reserve.clone();
        if let Some(fact) = &mut delivered.cleanup {
            fact.cancel_open = false;
            fact.failures = failure_bit(FailureKind::Cancelled);
            if fact.pending {
                fact.aborted = true;
                fact.pending = false;
                fact.reserve.clear();
            }
        }
        if token.cleanup.as_ref().is_some_and(|fact| fact.pending) {
            delivered.pieces.extend(reserve);
        }
        vec![(kept, false), (delivered, true)]
    }

    /// Whether the D16 schedule of `close` lists a holder whose service key is `key`.
    fn scheduled(&self, before: &Flow, close: CleanupCloseId, key: Option<Origin>) -> bool {
        self.body.cleanup.schedules.iter().filter(|(_, schedule)| schedule.close == close)
            .flat_map(|(_, schedule)| &schedule.holders)
            .any(|value| before.tokens[value.as_u32() as usize].service == key)
    }

    #[allow(clippy::too_many_arguments)]
    fn scope_effects(
        &mut self, block: BlockId, op: &Operation, state: ValueId, reserve: Option<ValueId>,
        envelope: &ScopeEffectEnvelope, before: &Flow,
    ) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("cleanup::scope_effects");
        let pieces = before.tokens[state.as_u32() as usize].pieces.iter()
            .chain(reserve.into_iter().flat_map(|value| &before.tokens[value.as_u32() as usize].pieces))
            .cloned().collect();
        let mut required = self.required_effects(&pieces, before);
        let has_services = before.tokens.iter().enumerate().any(|(index, token)| {
            before.availability[index] == AVAILABLE && token.service.is_some()
        });
        if has_services {
            required.suspends = true;
            required.failures = vec![
                FailureKind::Thrown, FailureKind::Program, FailureKind::Panic, FailureKind::Native,
            ];
        }
        if required.suspends && !envelope.suspends
            || required.failures.iter().any(|kind| !envelope.failures.contains(kind)) {
            self.error(R::B104, "scope effect envelope understates admitted or reserved cleanup effects");
        }
        if required.suspends && !self.declaration.attrs.contains(&FunctionAttr::Async)
            && !self.declaration.attrs.iter().any(|attr| {
                let FunctionAttr::Ctor(owner) = attr else {
                    return false;
                };
                self.module.declarations.get(*owner).is_ok_and(|declaration| {
                    if let DeclarationKind::Object { destroy: Some(destroy), .. } = declaration.kind {
                        self.module.functions.get(destroy).is_ok_and(|function| {
                            function.attrs.contains(&FunctionAttr::Async)
                        })
                    } else {
                        false
                    }
                })
            }) {
            self.error(R::B112, "synchronous owner contains an obligation with asynchronous cleanup");
        }
        if envelope.cancel {
            let scope = scope(self.module, self.body, state).and_then(|id| self.module.scopes.get(id).ok());
            let final_scope = scope.is_some_and(|scope| scope.parent.is_none() && scope.kind == ScopeKind::Action);
            let block_scope = self.body.blocks.get(block).ok().map(|block| block.lexical_scope);
            let active_children = before.scopes.iter().any(|(id, status)| *status == 1
                && Some(*id) != block_scope);
            if !self.declaration.attrs.contains(&FunctionAttr::Async) || !final_scope || active_children
                || before.active_failure != 0 || matches!(op, Operation::Ownership(OwnershipOp::ScopeUnwind { .. })) {
                self.error(R::B105, "cancel envelope is legal only at final successful async action close");
            }
        }
    }

    fn required_effects(&mut self, pieces: &BTreeSet<Piece>, before: &Flow) -> Effects {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("cleanup::required_effects");
        let mut required = Effects::default();
        for piece in pieces {
            let types = self.origin_types(piece.origin, piece, before);
            if types.is_empty() {
                required.suspends = true;
                required.failures = vec![FailureKind::Thrown, FailureKind::Program, FailureKind::Panic,
                    FailureKind::Native];
            }
            for ty in types {
                let effects = super::super::effects::cleanup(self.module, self.body, ty, self.state);
                required.suspends |= effects.suspends;
                for kind in effects.failures {
                    if !required.failures.contains(&kind) {
                        required.failures.push(kind);
                    }
                }
            }
        }
        required
    }

    fn origin_types(&self, origin: Origin, piece: &Piece, flow: &Flow) -> Vec<TypeId> {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("cleanup::origin_types");
        let values: Vec<_> = match origin {
            Origin::Input(value) | Origin::External(value) => vec![value],
            Origin::Instruction(inst, _) | Origin::RetiredInstruction(inst, _) => self.body.insts.get(inst)
                .map(|inst| inst.results.clone()).unwrap_or_default(),
            Origin::Invoke(block, slot) | Origin::RetiredInvoke(block, slot) => {
                if let Ok(block) = self.body.blocks.get(block)
                    && let TermKind::InvokeDefer { function, .. } = &block.term.kind {
                    let index = u32::MAX - slot;
                    if let Some(ty) = self.module.functions.get(*function).ok().and_then(|function| {
                        function.attrs.iter().find_map(|attr| if let FunctionAttr::DeferEntry { replacements } = attr {
                            replacements.get(index as usize).and_then(|label| label.result)
                        } else { None })
                    }) { return vec![ty]; }
                }
                self.body.blocks.get(block).ok()
                .and_then(|block| if let TermKind::InvokeOwn { normal, .. } = &block.term.kind {
                    self.body.blocks.get(normal.block).ok().map(|target| {
                        target.params.iter().zip(&normal.args).filter_map(|(value, arg)| {
                            matches!(arg, EdgeArg::Produced(_)).then_some(*value)
                        }).collect()
                    })
                } else { None }).unwrap_or_default()
            }
            _ => Vec::new(),
        };
        let mut types: Vec<_> = values.iter().filter_map(|value| self.body.values.get(*value).ok().map(|v| v.ty))
            .filter(|ty| !matches!(self.module.types.get(*ty), Ok(Type::Service(_)))).collect();
        if types.is_empty() {
            for filter in piece.predicates.keys() {
                for key in &filter.keys {
                    for (value, data) in self.body.values.iter() {
                        if flow.refs[value.as_u32() as usize].contains(key)
                            && !matches!(self.module.types.get(data.ty), Ok(Type::Service(_))) {
                            types.push(data.ty);
                        }
                    }
                }
            }
        }
        types
    }
}

impl Engine<'_, '_> {
    fn check_plan(&mut self, flow: &Flow, cleanup: ValueId, token: &Token, fact: &CleanupFact, id: CleanupPlanId) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("cleanup::check_plan");
        let Ok(plan) = self.body.cleanup.plans.get(id) else {
            return;
        };
        let Ok(close) = self.body.cleanup.closes.get(plan.close) else {
            return;
        };
        if plan.close != fact.close {
            self.error(R::B102, "cleanup plan belongs to a different close");
        }
        // G1 rev 2 §§6.3–6.6: an explicit abort contributes the entire reserve just like begin's reserve.
        let mut sources = vec![match close.source {
            CleanupSource::Scope(_) => MergeSource::ScopeInventory,
            CleanupSource::Packet(_) => MergeSource::PacketInventory,
        }];
        if close.reserve || fact.aborted { sources.push(MergeSource::WholeReserveOnFailure); }
        if plan.sources != sources || fact.aborted && plan.direct.is_some() {
            self.error(R::B101, "plan must cover the whole explicitly aborted reserve through merge");
        }
        let envelope = &close.effects;
        if envelope.cancel {
            let root = token.scope.and_then(|scope| self.module.scopes.get(scope).ok());
            if !self.declaration.attrs.contains(&FunctionAttr::Async)
                || root.is_none_or(|scope| scope.kind != ScopeKind::Action || scope.parent.is_some()) {
                self.error(R::B105, "lowered cancellation envelope requires the final async action boundary");
            }
        }
        if fact.cancel_delivered && !envelope.cancel {
            self.error(R::B105, "lowered cancellation delivery is missing from its close envelope");
        }
        let mut effects = self.required_effects(&fact.inventory, flow);
        // D20 §2.6: in Low a close suspends or fails only through its steps. No service settle waits or fails in
        // 0.6.0 (async ABI D24.2); the outcome identities a settle releases are in the inventory with their types.
        let services = fact.had_services && self.module.profile != Profile::Low;
        effects.suspends |= services;
        if services {
            for kind in [FailureKind::Thrown, FailureKind::Program, FailureKind::Panic, FailureKind::Native] {
                if !effects.failures.contains(&kind) {
                    effects.failures.push(kind);
                }
            }
        }
        if effects.suspends && !envelope.suspends
            || effects.failures.iter().any(|kind| !envelope.failures.contains(kind)) {
            self.error(R::B104, "lowered plan needs a sound close effect envelope including reserve and services");
        }
        if effects.suspends && !self.declaration.attrs.contains(&FunctionAttr::Async) {
            self.error(R::B112, "lowered synchronous cleanup owns an asynchronous or opaque source");
        }
        self.cleanup_roots(flow, cleanup);
        let Some(proof) = plan.direct.and_then(|id| self.body.cleanup.proofs.get(id).ok()) else {
            return;
        };
        let keys: BTreeSet<_> = proof.slots.iter().map(|slot| self.identity(flow, slot.identity)).collect();
        let actual: BTreeSet<_> = token.pieces.iter().map(|piece| piece.origin).collect();
        if keys != actual || keys.len() != proof.slots.len()
            || token.pieces.iter().any(|piece| piece.family || !piece.predicates.is_empty())
            || proof.slots.iter().any(|slot| flow.availability[slot.identity.as_u32() as usize] != AVAILABLE) {
            self.error(R::B102, "static plan must cover every available singleton identity exactly once");
        }
        if proof.reserve_empty && !fact.reserve.is_empty()
            || (!envelope.failures.is_empty() || fact.failures != 0 || fact.may_fail)
                && !fact.reserve.is_empty() {
            self.error(R::B102, "direct cleanup violates B2 or forges an empty reserve witness");
        }
        let mut positions = Vec::new();
        let mut matching = !proof.slots.is_empty() && proof.suffixes.len() == proof.slots.len()
            && self.proof_links(proof);
        for (index, slot) in proof.slots.iter().enumerate() {
            matching &= slot.active_at == close.after_defers;
            if matches!(slot.origin, CreationWitness::Singleton) {
                matching &= proof.slots.len() == 1;
            } else if let Some((key, block, position)) = self.witness_completion(flow, &slot.origin, slot.identity) {
                matching &= key == self.identity(flow, slot.identity);
                positions.push((block, position));
            } else {
                matching = false;
            }
            if let Some(suffix) = proof.suffixes.get(index) {
                let remaining: Vec<_> = proof.slots[index + 1..].iter().map(|slot| slot.identity).collect();
                let roots: Vec<_> = proof.slots[index + 1..].iter()
                    .flat_map(|slot| [slot.identity, slot.descriptor]).collect();
                matching &= suffix.completed as usize == index + 1 && suffix.remaining == remaining
                    && suffix.roots == roots;
            }
        }
        if let Some(graph) = super::super::graph::Dominators::new(self.body, self.state) {
            for &(block, position) in &positions {
                if block == close.after_defers {
                    let at = self.body.blocks.get(block).ok().and_then(|block| block.insts.iter().position(|id| {
                        self.body.insts.get(*id).is_ok_and(|inst| matches!(inst.op,
                            Operation::Cleanup(CleanupOp::Begin { close, .. }) if close == plan.close))
                    }));
                    matching &= at.is_some_and(|at| position < at);
                } else {
                    matching &= graph.dominates(super::super::graph::Dominators::entry(block) + 1,
                        super::super::graph::Dominators::entry(close.after_defers), self.state);
                }
            }
            for pair in positions.windows(2) {
                let later = pair[0];
                let earlier = pair[1];
                if later.0 == earlier.0 {
                    matching &= later.1 > earlier.1;
                } else {
                    matching &= graph.dominates(super::super::graph::Dominators::entry(earlier.0) + 1,
                        super::super::graph::Dominators::entry(later.0), self.state);
                }
            }
        } else {
            matching = false;
        }
        if !matching {
            self.error(R::B102, "direct order needs active completion witnesses, descending order and exact suffixes");
        }
    }

    fn proof_links(&mut self, proof: &CleanupOrderProof) -> bool {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("cleanup::proof_links");
        let edges = super::super::graph::edges(self.body, self.state);
        let mut valid = true;
        for witness in &proof.preserves {
            if !self.state.step(1) {
                return false;
            }
            let Some(data) = self.body.values.get(witness.to).ok() else {
                return false;
            };
            let consumes = |op: &Operation| {
                op.operand_refs().iter().any(|operand| {
                    operand.reference == OperandRef::Value(witness.from) && operand.mode == OperandMode::Consume
                })
            };
            valid &= match data.def {
                Definition::InstResult { inst, .. } => {
                    self.body.insts.get(inst).is_ok_and(|inst| consumes(&inst.op))
                }
                Definition::BlockParam { block, index } => edges.iter().any(|edge| {
                    if edge.target != block {
                        return false;
                    }
                    match edge.args.get(index as usize) {
                        Some(EdgeArg::Value(from)) => *from == witness.from,
                        Some(EdgeArg::Produced(EdgeResult { outcome: Outcome::Normal, .. })) => {
                            self.body.blocks.get(edge.source).is_ok_and(|source| {
                                matches!(&source.term.kind, TermKind::InvokeOwn { op, .. } if consumes(op))
                            })
                        }
                        _ => false,
                    }
                }),
            };
        }
        for witness in &proof.joins {
            if !self.state.step(edges.len() + 1) {
                return false;
            }
            let Some(Definition::BlockParam { block, index }) = self.body.values.get(witness.state)
                .ok().map(|data| data.def) else {
                return false;
            };
            let predecessors: Vec<_> = edges.iter().filter(|edge| edge.target == block).filter_map(|edge| {
                match edge.args.get(index as usize) {
                    Some(EdgeArg::Value(value)) => Some(*value),
                    _ => None,
                }
            }).collect();
            valid &= predecessors.len() > 1 && witness.predecessors == predecessors;
        }
        valid
    }

    fn cleanup_roots(&mut self, flow: &Flow, cleanup: ValueId) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("cleanup::cleanup_roots");
        let L::Instruction { instruction, .. } = self.state.location else {
            return;
        };
        let maps: Vec<_> = self.body.cleanup.roots.iter().filter_map(|(_, map)| {
            (map.boundary == RootBoundary::BeforeInstruction(instruction)).then_some(map)
        }).collect();
        if maps.len() != 1 || !maps[0].bindings.iter().any(|binding| {
            binding.value == cleanup && binding.trace == RootTrace::CleanupActivation
        }) {
            self.error(R::B105, "cleanup root map omits its activation, self, remainder, schedule or reserve");
        }
        for map in maps {
            let suspends = flow.tokens[cleanup.as_u32() as usize].cleanup.as_ref()
                .and_then(|fact| self.body.cleanup.closes.get(fact.close).ok())
                .is_some_and(|close| close.effects.suspends);
            if suspends && !map.through_suspend {
                self.error(R::B105, "cleanup root bindings must survive the declared suspension");
            }
            let mut seen = BTreeSet::new();
            for binding in &map.bindings {
                let value = binding.value;
                if !seen.insert(value) || flow.availability[value.as_u32() as usize] != AVAILABLE
                    || flow.lifetimes[value.as_u32() as usize] != 1
                    || flow.dead.contains(&self.identity(flow, value)) {
                    self.error(R::B105, "cleanup root map refers to an unavailable or duplicate SSA holder");
                }
            }
        }
    }

    fn witness_completion(
        &self, flow: &Flow, witness: &CreationWitness, identity: ValueId,
    ) -> Option<(Origin, BlockId, usize)> {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("cleanup::witness_completion");
        if matches!(witness, CreationWitness::Singleton) {
            return None;
        }
        let site = match witness {
            CreationWitness::Singleton => return None,
            CreationWitness::Create { site } | CreationWitness::FreshResult { site, .. } => site,
        };
        let origin_site = match site {
            HighOperationSite::Instruction(inst) => CreationOrigin::Instruction(*inst),
            HighOperationSite::Terminator(block) => {
                CreationOrigin::Invoke { block: *block, outcome: Outcome::Normal, slot: 0 }
            }
        };
        if matches!(witness, CreationWitness::Create { .. }) {
            return self.completion(flow, origin_site);
        }
        let (op, block, position) = match site {
            HighOperationSite::Instruction(inst) => {
                let &(block, position) = self.state.positions.get(&(self.function, *inst))?;
                (&self.body.insts.get(*inst).ok()?.op, block, position)
            }
            HighOperationSite::Terminator(block) => {
                let TermKind::InvokeOwn { op, .. } = &self.body.blocks.get(*block).ok()?.term.kind else {
                    return None;
                };
                (&**op, *block, usize::MAX)
            }
        };
        let CreationWitness::FreshResult { callee, summary, .. } = witness else {
            return None;
        };
        let Operation::Call(CallOp::Direct { function, call }) = op else {
            return None;
        };
        let declaration = self.module.functions.get(*callee).ok()?;
        let contract = self.module.summaries.get(*summary).ok()?;
        let [relation] = contract.results.as_slice() else {
            return None;
        };
        if *function != *callee || call.summary != Some(*summary) || declaration.summary != Some(*summary)
            || relation.fresh.len() != 1 || !relation.aliases.is_empty() || relation.opaque
            || relation.provenance != AdmissionProvenance::UnboundResult || declaration.body.is_none() {
            return None;
        }
        let origin = match site {
            HighOperationSite::Instruction(inst) => Origin::Instruction(*inst, 0),
            HighOperationSite::Terminator(block) => Origin::Invoke(*block, 0),
        };
        let expected = creation(origin, relation.fresh[0]);
        (self.identity(flow, identity) == expected).then_some((expected, block, position))
    }

    fn completion(&self, flow: &Flow, creation: CreationOrigin) -> Option<(Origin, BlockId, usize)> {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("cleanup::completion");
        let (op, key, block, position) = match creation {
            CreationOrigin::Instruction(inst) => {
                let data = self.body.insts.get(inst).ok()?;
                let &(block, position) = self.state.positions.get(&(self.function, inst))?;
                (&data.op, Origin::Instruction(inst, 0), block, position)
            }
            CreationOrigin::Invoke { block, outcome: Outcome::Normal, slot: 0 } => {
                let TermKind::InvokeOwn { op, .. } = &self.body.blocks.get(block).ok()?.term.kind else {
                    return None;
                };
                (&**op, Origin::Invoke(block, 0), block, usize::MAX)
            }
            _ => return None,
        };
        match op {
            Operation::Core(CoreOp::ObjNew { construction: ConstructionStatus::Complete, .. }) => {
                Some((key, block, position))
            }
            Operation::Core(CoreOp::ObjInit { object, completion: Some(_), .. }) => {
                Some((self.identity(flow, *object), block, position))
            }
            _ => None,
        }
    }

    pub(super) fn cleanup_next(
        &mut self, flow: &mut Flow, cleanup: ValueId, selected: &InvokeEdge, done: &InvokeEdge,
        outgoing: &mut Vec<(BlockId, Flow)>,
    ) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("cleanup::cleanup_next");
        let before = flow.clone();
        self.use_value(flow, cleanup, OperandMode::Consume, R::B102);
        let mut token = before.tokens[cleanup.as_u32() as usize].clone();
        let Some(fact) = &mut token.cleanup else {
            self.error(R::B102, "cleanup.next needs a live cleanup automaton");
            return;
        };
        if !fact.shield || fact.phase != 2 || fact.awaiting_failure || fact.plan.is_none() {
            self.error(R::B102, "cleanup.next requires a shielded settled plan and a dispatched failure");
        }
        if fact.plan.and_then(|id| self.body.cleanup.plans.get(id).ok()).is_some_and(|plan| plan.direct.is_some()) {
            self.error(R::B102, "runtime selector cannot replace a direct plan's finite order");
        }
        let index = cleanup.as_u32() as usize;
        for (token, delivered) in self.cancel_outcomes(&token) {
            let mut branch = flow.clone();
            if let Some(fact) = token.cleanup.as_ref().filter(|_| delivered) {
                branch.active_failure = fact.failures;
                branch.active_failure_origin = fact.failure_origin;
            }
            let slot = Slot {
                token: token.clone(), refs: before.refs[index].clone(), requirements: before.full_deps[index].clone(),
                aliases: before.aliases[index].clone(), identity: before.identities[index], ..Slot::default()
            };
            if let Some(piece) = token.pieces.iter().next() {
                let mut selected_slot = slot.clone();
                selected_slot.token.cleanup.as_mut().unwrap().selected = Some(piece.origin);
                let mut key = Slot::default();
                key.refs.insert(piece.origin);
                key.token.sources.insert(cleanup);
                outgoing.push(self.edge(&branch, selected.block, &selected.args, &[selected_slot, key], R::B102));
            }
            if token.pieces.is_empty() || token.pieces.iter().all(|piece| piece.family) {
                let mut drained = slot;
                drained.token.pieces.clear();
                drained.token.cleanup.as_mut().unwrap().selected = None;
                outgoing.push(self.edge(&branch, done.block, &done.args, &[drained], R::B102));
            }
        }
    }

    pub(super) fn cleanup_dispatch(
        &mut self, flow: &mut Flow, term: &TermKind, outgoing: &mut Vec<(BlockId, Flow)>,
    ) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("cleanup::cleanup_dispatch");
        let (closed, failure, exit, arms) = match term {
            TermKind::ExitDispatch { closed, failure, exit, success, pending, failed, close } => {
                if flow.tokens[exit.as_u32() as usize].cleanup.as_ref().is_none_or(|fact| fact.close != *close) {
                    self.error(R::B101, "exit dispatch has the wrong close descriptor");
                }
                let arms = [(DispatchKind::Success, success), (DispatchKind::Pending, pending),
                    (DispatchKind::Failure, failed)].into_iter().filter_map(|(kind, edge)| {
                    edge.as_ref().map(|edge| DispatchEdge { kind, edge: edge.clone() })
                }).collect::<Vec<_>>();
                (Some(*closed), *failure, Some(*exit), arms)
            }
            TermKind::FailureDispatch { failure, none, some } => {
                let none = InvokeEdge { block: none.block,
                    args: none.args.iter().copied().map(EdgeArg::Value).collect() };
                (None, *failure, None, vec![DispatchEdge { kind: DispatchKind::None, edge: none },
                    DispatchEdge { kind: DispatchKind::Some, edge: some.clone() }])
            }
            _ => return,
        };
        self.dispatch(flow, closed, failure, exit, &arms, outgoing);
    }

    fn dispatch(
        &mut self, flow: &mut Flow, closed: Option<ValueId>, failure: ValueId, exit: Option<ValueId>,
        arms: &[DispatchEdge], outgoing: &mut Vec<(BlockId, Flow)>,
    ) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("cleanup::dispatch");
        let before = flow.clone();
        self.live_use(&before, failure);
        if let Some(exit) = exit {
            self.use_value(flow, exit, OperandMode::Consume, R::B101);
        }
        let fact = exit.and_then(|exit| before.tokens[exit.as_u32() as usize].cleanup.clone());
        if let (Some(exit), Some(closed)) = (exit, closed) {
            let sources = &before.tokens[exit.as_u32() as usize].sources;
            let generation = before.tokens[exit.as_u32() as usize].finish_origin;
            if sources.is_empty() || generation.is_none()
                || generation != before.tokens[closed.as_u32() as usize].finish_origin
                || generation != before.tokens[failure.as_u32() as usize].finish_origin {
                self.error(R::B104, "closed/failure/exit states must come from the same finish operation");
            }
        }
        if exit.is_some() && fact.is_none() {
            self.error(R::B101, "exit.dispatch requires the states produced together by cleanup.finish");
        }
        let mut expected = Vec::new();
        if let Some(fact) = &fact {
            if fact.failures != 0 {
                expected.push(DispatchKind::Failure);
            }
            else {
                if fact.pending {
                    expected.push(DispatchKind::Pending);
                }
                else { expected.push(DispatchKind::Success); }
                if fact.may_fail {
                    expected.push(DispatchKind::Failure);
                }
            }
        } else {
            expected.push(DispatchKind::None);
            if before.failures[failure.as_u32() as usize] != 0 {
                expected.push(DispatchKind::Some);
            }
        }
        if expected.iter().any(|kind| !arms.iter().any(|arm| arm.kind == *kind)) {
            self.error(R::B104, "typed dispatch omits a possible cleanup outcome");
        }
        for arm in arms {
            if exit.is_some() && !expected.contains(&arm.kind) {
                continue;
            }
            if exit.is_none() && arm.kind == DispatchKind::Some
                && before.failures[failure.as_u32() as usize] == 0 { continue; }
            let mut branch = flow.clone();
            if exit.is_some() && matches!(arm.kind, DispatchKind::Success | DispatchKind::Pending) {
                let generation = before.tokens[failure.as_u32() as usize].finish_origin;
                self.clear_finish_generation(&mut branch, generation);
            }
            if arm.kind == DispatchKind::Failure && let Some(fact) = &fact {
                let reserve: Vec<_> = fact.reserve.iter().cloned().collect();
                self.invalidate(&before, &mut branch, &reserve);
                branch.closed_inputs.extend(reserve.iter().cloned());
                branch.dead.extend(reserve.iter().map(|piece| piece.origin));
            }
            let mut slots = vec![Slot::default(); arm.kind.results().len()];
            if matches!(arm.kind, DispatchKind::Some | DispatchKind::Failure) {
                if branch.active_failure == 0 || exit.is_some() {
                    branch.active_failure = before.failures[failure.as_u32() as usize];
                    branch.active_failure_origin = before.tokens[failure.as_u32() as usize].failure_origin;
                }
                if let Some(slot) = slots.last_mut() {
                    slot.failures = before.failures[failure.as_u32() as usize];
                    slot.token = before.tokens[failure.as_u32() as usize].clone();
                }
            }
            if let Some(fact) = &fact {
                if arm.kind == DispatchKind::Pending
                && let (Some(slot), Some(exit)) = (slots.last_mut(), exit) {
                    slot.token = before.tokens[exit.as_u32() as usize].clone();
                    slot.token.cleanup = None;
                    slot.token.pieces = fact.reserve.clone();
                    let index = exit.as_u32() as usize;
                    slot.refs = before.refs[index].clone();
                    slot.requirements = before.full_deps[index].clone();
                    slot.aliases = before.aliases[index].clone();
                    slot.identity = before.identities[index];
                }
            } else {
                // Copyable FailureState is not a linear permission. Its association
                // with the current cleanup token prevents skipping a failed step.
                for token in branch.tokens.stored_mut() {
                    if let Some(fact) = &mut token.cleanup
                        && fact.awaiting_failure && arm.kind == DispatchKind::None
                            && fact.awaiting_source.is_some_and(|source| {
                                before.tokens[failure.as_u32() as usize].sources.contains(&source)
                            }) && fact.awaiting_origin == before.tokens[failure.as_u32() as usize].failure_origin {
                        fact.awaiting_failure = false;
                        fact.awaiting_source = None;
                        fact.awaiting_origin = None;
                    }
                }
            }
            let _ = closed;
            outgoing.push(self.edge(&branch, arm.edge.block, &arm.edge.args, &slots, R::B101));
        }
    }
}
