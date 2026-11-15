use super::*;

impl Engine<'_, '_> {
    pub(super) fn partition(&mut self, flow: &Flow, token: &Token, selection: SelectionId) -> (Token, Token) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("packets::partition");
        // A selection never extracts a cache duty or what its cache holds, even when it names the whole
        // Future: the duty leaves its owner only through cache.rehome (C-4, ruling of #601 R1).
        let mut rest = token.clone();
        let mut selected = Token::default();
        for (keys, fresh, labels) in self.selection_terms(flow, selection) {
            let possible = self.possible(flow, &keys);
            let filter = Filter { keys, fresh, possible, labels, stale: false, whole_future: false, state: 0 };
            let (next, part) = self.partition_filter(&rest, filter);
            rest = next;
            selected = self.union([selected, part], true);
        }
        (rest, selected)
    }
    pub(super) fn partition_filter(&mut self, token: &Token, base: Filter) -> (Token, Token) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("packets::partition_filter");
        let Filter { keys, fresh, possible, labels, .. } = &base;
        let mut rest = Token {
            scope: token.scope, sources: token.sources.clone(), ..Token::default()
        };
        let mut selected = Token { destinations: token.destinations.clone(), ..Token::default() };
        rest.destinations = token.destinations.clone();
        for piece in &token.pieces {
            let admits_all = piece.provenance & !FRESH == 0;
            let filter = Filter {
                keys: keys.clone(), fresh: *fresh && !admits_all,
                possible: possible.clone(), labels: labels.clone(), stale: false,
                whole_future: base.whole_future,
                state: if *fresh && !admits_all { piece.provenance } else { 0 },
            };
            let unknown = piece.family || matches!(piece.origin, Origin::Input(_))
                || possible.contains(&piece.origin);
            let provenance = piece.provenance;
            let eligible = !*fresh || provenance & FRESH != 0;
            // An accepted input cannot contain a distinct later creation.
            // Positive partition predicates bound its possible retention graph.
            let input_related = matches!(piece.origin, Origin::Input(_))
                && piece.predicates.iter().filter(|(_, selected)| **selected).all(|(filter, _)| {
                    filter.keys.iter().any(|old| keys.iter().any(|key| {
                        old == key || possible.contains(old) || filter.possible.contains(key)
                            || (matches!(old, Origin::External(_)) && matches!(key, Origin::External(_)))
                    }))
                });
            let related = input_related
                || keys.contains(&piece.origin) || possible.contains(&piece.origin)
                || piece.holder.is_some_and(|holder| keys.contains(&holder));
            if related && eligible && !keys.is_empty() && piece.holder.is_some() && !base.whole_future {
                let rule = if piece.channel { R::B74 } else { R::B91 };
                self.error(rule, "scope/packet selection cannot extract a private protocol holder graph");
                rest.pieces.insert(piece.clone());
                continue;
            }
            if !eligible || keys.is_empty() || !related {
                rest.pieces.insert(piece.clone());
            } else if !unknown {
                selected.pieces.insert(piece.clone());
            } else {
                let mut yes = piece.clone();
                let mut no = piece.clone();
                let recorded = piece.predicate(&filter);
                if recorded != Some(false) {
                    yes.predicates.insert(filter.clone(), true);
                    if *fresh {
                        yes.provenance &= FRESH;
                    }
                    selected.pieces.insert(yes);
                }
                if recorded != Some(true) {
                    no.predicates.insert(filter.clone(), false);
                    rest.pieces.insert(no);
                }
            }
        }
        (rest, selected)
    }
    pub(super) fn union(&mut self, parts: impl IntoIterator<Item = Token>, disjoint: bool) -> Token {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("packets::union");
        let mut result = Token::default();
        for part in parts {
            if !self.state.step(part.pieces.len().saturating_mul(result.pieces.len()).saturating_add(1)) {
                return result;
            }
            if disjoint && part.pieces.iter().any(|piece| result.pieces.iter().any(|old| piece.overlaps(old))) {
                self.error(R::B22, "join inputs have overlapping ownership contents");
            }
            result.scope = result.scope.or(part.scope);
            result.reserve.extend(part.reserve);
            result.destinations.extend(part.destinations);
            result.sources.extend(part.sources);
            result.pieces.extend(part.pieces);
        }
        result
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn operation(
        &mut self, block: BlockId, op: &Operation, schema: &OutcomeContract, outcome: Outcome,
        origin: Origin, flow: &mut Flow,
    ) -> Vec<Slot> {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("packets::operation");
        if !self.charge_flow(flow) {
            return Vec::new();
        }
        self.retire_origin(flow, view_origin(origin, u16::MAX as usize), false);
        for index in 0..schema.results.len() {
            let key = match origin {
                Origin::Instruction(inst, _) => Origin::Instruction(inst, 0x8000_0000 + index as u32),
                Origin::Invoke(block, _) => Origin::Invoke(block, 0x8000_0000 + index as u32),
                _ => origin,
            };
            self.retire_origin(flow, key, false);
        }
        if outcome == Outcome::Normal {
            if super::super::summaries::call(op).is_none() {
                for slot in 0..schema.results.len() {
                    if !self.retire_origin(flow, view_origin(origin, slot), true) {
                        return Vec::new();
                    }
                }
            }
            if matches!(op, Operation::Core(CoreOp::ObjNew { .. } | CoreOp::ObjNext { .. })
                | Operation::Async(AsyncOp::FutureNew { .. } | AsyncOp::FrameAlloc { .. })
                | Operation::Cleanup(CleanupOp::CellNew { .. })) {
                self.retire_origin(flow, origin, false);
            }
            if let Some((call, _, _)) = super::super::summaries::call(op)
                && let Some(summary) = call.summary.and_then(|id| self.module.summaries.get(id).ok()) {
                let sites: BTreeSet<_> = summary.results.iter().flat_map(|relation| &relation.fresh).copied().collect();
                for site in sites {
                    if !self.retire_origin(flow, creation(origin, site), true) {
                        return Vec::new();
                    }
                }
                for (index, _) in summary.results.iter().enumerate() {
                    let view = match origin {
                        Origin::Instruction(inst, _) => Origin::ViewInstruction(inst, index as u16),
                        Origin::Invoke(block, _) => Origin::ViewInvoke(block, index as u16),
                        _ => origin,
                    };
                    if !self.retire_origin(flow, view, true) {
                        return Vec::new();
                    }
                }
            }
        }
        let before = flow.clone();
        if let Operation::Ownership(OwnershipOp::ScopeUnwind { failure, .. }) = op {
            let primary = before.tokens[failure.as_u32() as usize].failure_origin;
            if before.active_failure_origin.is_some() && before.active_failure_origin != primary {
                self.error(R::B104, "scope unwind must preserve the actual primary failure/Error view");
            }
            flow.active_failure = before.failures[failure.as_u32() as usize];
            flow.active_failure_origin = primary;
        }
        if outcome != Outcome::Normal
            && let Ok(contract) = ContractRegistry::new(self.module).construction_schema(op, self.body) {
            flow.latest_failure_origin = Some(view_origin(origin, u16::MAX as usize));
            flow.latest_failure = if outcome == Outcome::Cancel { failure_bit(FailureKind::Cancelled) }
                else { contract.effects.failures.iter().fold(0, |mask, kind| mask | failure_bit(*kind)) };
            if before.active_failure == 0 {
                flow.active_failure = flow.latest_failure;
                flow.active_failure_origin = flow.latest_failure_origin;
            }
        }
        let refs = op.operand_refs();
        let mut metadata: Vec<_> = refs.iter().filter_map(|operand| {
            (!matches!(operand.reference, OperandRef::Value(_))).then_some(operand.reference)
        }).collect();
        let mut seen = BTreeSet::new();
        while let Some(reference) = metadata.pop() {
            if !self.state.step(1) {
                return Vec::new();
            }
            if !seen.insert(reference) {
                continue;
            }
            if let OperandRef::Value(value) = reference {
                self.live_use(&before, value);
            }
            else if let Ok(children) = cfg::expression_children(reference, self.body) { metadata.extend(children); }
        }
        // High has no named exceptional results (H2). Rule::Buffer restores
        // precommit inputs under their entry SSA names; own/low still consume
        // them and name the Preserve slots with invoke.own.
        let high_preserve = self.module.profile == Profile::High
            && outcome == Outcome::Unwind
            && matches!(op.spec().rule, crate::contracts::Rule::Buffer { .. });
        let preserved: BTreeSet<_> = schema.transfers.iter().filter_map(|transfer| {
            match transfer {
                TransferRule::Preserve { input, .. } if high_preserve => Some(*input),
                _ => None,
            }
        }).collect();
        for operand in refs {
            if let OperandRef::Value(value) = operand.reference {
                let rule = if matches!(op, Operation::Ownership(OwnershipOp::ScopeEnd { .. }
                    | OwnershipOp::ScopeUnwind { .. })) { R::B41 } else { R::B21 };
                let mode = if preserved.contains(&value) { OperandMode::Borrow } else { operand.mode };
                self.use_value(flow, value, mode, rule);
            }
        }
        self.scope_operation(block, op, outcome, &before, flow);
        self.call_entry(op, &before, flow);
        let mut slots = vec![Slot::default(); schema.results.len()];
        let slot_index = |name: SlotName| schema.results.iter().position(|slot| slot.name == name);
        for transfer in &schema.transfers {
            if !self.state.step(1) {
                return slots;
            }
            match transfer {
                TransferRule::Preserve { input, output } => {
                    if preserved.contains(input) {
                        continue;
                    }
                    if let Some(i) = slot_index(*output) {
                        slots[i].token = before.tokens[input.as_u32() as usize].clone();
                        slots[i].journal = before.journals[input.as_u32() as usize].clone();
                        slots[i].refs = before.refs[input.as_u32() as usize].clone();
                        slots[i].identity = before.identities[input.as_u32() as usize];
                        slots[i].aliases = before.aliases[input.as_u32() as usize].clone();
                        slots[i].contained = before.contained[input.as_u32() as usize].clone();
                        slots[i].must_use = before.must_use[input.as_u32() as usize];
                        slots[i].requirements = before.full_deps[input.as_u32() as usize].clone();
                        slots[i].lifetime = before.lifetimes[input.as_u32() as usize];
                    }
                }
                TransferRule::Move { inputs, outputs } => {
                    let token = self.union(inputs.iter().map(|v| before.tokens[v.as_u32() as usize].clone()),
                        matches!(op, Operation::Ownership(OwnershipOp::Join { .. })));
                    for output in outputs {
                        if let Some(i) = slot_index(*output) {
                            // Buffer uniqueness and the element journal are independent.
                            if *output != SlotName::Buffer && *output != SlotName::Value {
                                slots[i].token = token.clone();
                            }
                        }
                    }
                    if let Some(i) = slot_index(SlotName::Packet) {
                        slots[i].token = token;
                    }
                }
                TransferRule::Partition { input, selection, rest, selected } => {
                    let (left, right) = self.partition(&before, &before.tokens[input.as_u32() as usize], *selection);
                    if matches!(op, Operation::Ownership(OwnershipOp::Split { .. })) {
                        let original = &before.tokens[input.as_u32() as usize];
                        let proved = left.pieces.iter().chain(&right.pieces).all(|piece| {
                            original.pieces.contains(piece)
                        });
                        if !proved {
                            self.error(R::B22, "unchecked partition requires a static proof; use own.split.checked");
                        }
                    }
                    if let Some(i) = slot_index(*rest) {
                        slots[i].token = left;
                    }
                    if let Some(i) = slot_index(*selected) {
                        slots[i].token = right;
                        slots[i].token.selected_scope = before.tokens[input.as_u32() as usize].scope;
                        slots[i].token.selection = Some(*selection);
                    }
                    if let Some(i) = slot_index(SlotName::ExternalDeps) {
                        let (keys, _) = self.selection(&before, *selection);
                        let owned: BTreeSet<_> = slots.iter().flat_map(|s| s.token.pieces.iter().map(|p| p.origin))
                            .collect();
                        slots[i].refs = keys.difference(&owned).copied().collect();
                    }
                }
                TransferRule::Admit { state, packet, output, provenance } => {
                    let owner = scope(self.module, self.body, *state);
                    if before.tokens[packet.as_u32() as usize].bound_admission
                        && !matches!(op, Operation::Ownership(OwnershipOp::Accept { .. })) {
                        self.error(R::B101, "discarded replacement reserve requires bound own.accept");
                    }
                    let mut incoming = before.tokens[packet.as_u32() as usize].clone();
                    if incoming.destinations.iter().any(|destination| Some(*destination) != owner) {
                        self.error(R::B101, "committed packet must be admitted into its declared destination");
                    }
                    incoming.destinations.clear();
                    incoming.pieces = incoming.pieces.into_iter().map(|mut piece| {
                        let restored = matches!(op, Operation::Ownership(OwnershipOp::Accept { .. }))
                            && incoming.scope == owner
                            && piece.owner.is_some_and(|old| before.scopes.get(&old) == Some(&2))
                            && piece.owner.zip(owner).is_some_and(|(old, target)| {
                                super::super::high::inside(self.module, old, target, self.state)
                            });
                        if piece.owner.is_some() && piece.owner != owner
                            && piece.provenance & !FRESH != 0 && !restored {
                            self.error(R::B43, "admission cannot silently change an existing lexical owner");
                        }
                        piece.owner = owner;
                        piece.journal = true;
                        if let ProvenanceTransition::Set(provenance) = provenance {
                            piece.provenance = provenance_bit(*provenance);
                            if !matches!(piece.origin, Origin::Input(_)) {
                                flow.provenance.insert(piece.origin, piece.provenance);
                            }
                        }
                        piece
                    }).collect();
                    let mut token = self.union([before.tokens[state.as_u32() as usize].clone(), incoming], false);
                    token.scope = owner;
                    if let Some(i) = slot_index(*output) {
                        slots[i].token = token;
                    }
                }
                TransferRule::ScopeTransfer { states, selection } => match states {
                    TransferStates::Same(value) => {
                        if let Some(i) = slot_index(SlotName::State) {
                            slots[i].token = before.tokens[value.as_u32() as usize].clone();
                        }
                    }
                    TransferStates::Between { from, to } => {
                        let (rest, mut part) = self.partition(&before, &before.tokens[from.as_u32() as usize],
                            *selection);
                        let destination = scope(self.module, self.body, *to);
                        part.pieces = part.pieces.into_iter().map(|mut piece| {
                            piece.owner = destination;
                            piece
                        }).collect();
                        let mut target = self.union([before.tokens[to.as_u32() as usize].clone(), part], false);
                        target.scope = destination;
                        if let Some(i) = slot_index(SlotName::From) {
                            slots[i].token = rest;
                        }
                        if let Some(i) = slot_index(SlotName::To) {
                            slots[i].token = target;
                        }
                    }
                },
                TransferRule::Close { input } => {
                    let reserve = match op {
                        Operation::Ownership(OwnershipOp::ScopeEnd { reserve, .. }
                            | OwnershipOp::ScopeUnwind { reserve, .. }) => *reserve,
                        _ => None,
                    };
                    if !matches!(op, Operation::Cleanup(CleanupOp::Finish { .. })) {
                        self.close(&before, flow, *input, reserve);
                    }
                }
                TransferRule::Settle { inputs } => {
                    if matches!(op, Operation::Cleanup(CleanupOp::Settle { .. })
                        | Operation::Async(AsyncOp::FuturePublish { .. } | AsyncOp::FutureSettle { .. })) { continue; }
                    for input in inputs {
                        let pieces: Vec<_> = before.tokens[input.as_u32() as usize].pieces.iter().cloned().collect();
                        self.invalidate(&before, flow, &pieces);
                        for piece in &pieces {
                            flow.dead.insert(piece.origin);
                            if matches!(piece.origin, Origin::Input(_)) {
                                flow.closed_inputs.insert(piece.clone());
                            }
                        }
                    }
                }
                TransferRule::Reserve { packet, boundary } => {
                    let mut token = before.tokens[packet.as_u32() as usize].clone();
                    let current = self.body.blocks.get(block).ok().map(|block| block.lexical_scope);
                    let outer = token.selected_scope.is_some_and(|selected| before.scopes.iter().any(|(id, status)| {
                        *status == 1 && *id != selected
                            && super::super::high::inside(self.module, *id, selected, self.state)
                    }));
                    if !token.pieces.is_empty() && (token.selected_scope != current || outer) {
                        self.error(R::B101, "initial reserve must be selected from the currently closing scope");
                    }
                    for (value, _) in self.body.values.iter() {
                        if before.availability[value.as_u32() as usize] == AVAILABLE
                            && super::super::later::value_type(self.module, self.body, value)
                                == Some(&Type::Service(ServiceType::Pending)) {
                            self.error(R::B101, "an action cannot have two active pending exits");
                        }
                    }
                    token.selection = Some(match op {
                        Operation::Exit(ExitOp::Prepare { exit }) => {
                            token.pending_type = exit.value.and_then(|value| self.body.values.get(value).ok())
                                .map(|value| value.ty);
                            token.pending_kind = Some(exit.kind);
                            token.pending_boundary = Some(exit.boundary);
                            if exit.value.is_none() && !token.pieces.is_empty() {
                                self.error(R::B101, "void exit cannot reserve an unrelated nonempty packet");
                            }
                            exit.selection
                        },
                        _ => unreachable!(),
                    });
                    if let Ok(boundary) = self.body.boundaries.get(*boundary) {
                        token.reserve.insert(boundary.scope);
                        token.pieces = token.pieces.into_iter().map(|mut piece| {
                            piece.owner = piece.owner.or(Some(boundary.scope));
                            piece
                        }).collect();
                    }
                    if let Some(i) = schema.results.iter().position(|slot| {
                        slot.ty == ResultType::Service(ServiceType::Pending)
                    }) { slots[i].token = token; }
                }
                TransferRule::CloseReserve { scope: state, reserve, output } => {
                    let mut token = before.tokens[reserve.as_u32() as usize].clone();
                    if output.is_none() {
                        let merged: Vec<_> = token.pieces.iter().cloned().collect();
                        self.invalidate(&before, flow, &merged);
                        flow.closed_inputs.extend(merged);
                        flow.dead.extend(token.pieces.iter().map(|piece| piece.origin));
                    } else {
                        if let Some(scope) = scope(self.module, self.body, *state) {
                            token.reserve.remove(&scope);
                        }
                        for piece in &token.pieces {
                            flow.dead.remove(&piece.origin);
                        }
                        if let Some(i) = output.and_then(slot_index) {
                            let index = reserve.as_u32() as usize;
                            slots[i].token = token;
                            slots[i].refs = before.refs[index].clone();
                            slots[i].identity = before.identities[index];
                            slots[i].aliases = before.aliases[index].clone();
                            slots[i].contained = before.contained[index].clone();
                            slots[i].must_use = before.must_use[index];
                            slots[i].requirements = before.full_deps[index].clone();
                            slots[i].lifetime = flow.lifetimes[index];
                        }
                    }
                }
                TransferRule::ReplaceReservation { pending, packet } => {
                    if let Operation::Exit(ExitOp::Replace { replacement, .. }) = op {
                        if before.active_failure != 0 {
                            self.error(R::B101, "defer replacement cannot replace an active primary failure");
                        }
                        let (mut rest, kept) = self.partition(&before,
                            &before.tokens[pending.as_u32() as usize], replacement.selection);
                        rest.reserve.clear();
                        rest.bound_admission = true;
                        rest.pieces = rest.pieces.into_iter().map(|mut piece| {
                            piece.provenance = provenance_bit(AdmissionProvenance::Bound);
                            piece
                        }).collect();
                        if slot_index(SlotName::Rest).is_none() && !rest.pieces.is_empty() {
                            self.error(R::B101, "replacement cannot discard its old reserve without a rest slot");
                        }
                        rest.scope = self.body.blocks.get(block).ok().map(|block| block.lexical_scope);
                        if let Some(i) = slot_index(SlotName::Rest) {
                            slots[i].token = rest;
                        }
                        let offered = packet.map(|packet| before.tokens[packet.as_u32() as usize].clone())
                            .unwrap_or_default();
                        let (outside, _) = self.partition(&before, &offered, replacement.selection);
                        if !outside.pieces.is_empty() {
                            self.error(R::B101, "defer replacement packet exceeds the candidate's retention graph");
                        }
                        let mut next = self.union([kept, offered], true);
                        next.reserve.clear();
                        next.selection = Some(replacement.selection);
                        next.pending_type = replacement.value.and_then(|value| self.body.values.get(value).ok())
                            .map(|value| value.ty);
                        next.pending_kind = Some(replacement.kind);
                        next.pending_boundary = Some(replacement.boundary);
                        next.rehome = before.tokens[pending.as_u32() as usize].rehome;
                        if let Ok(boundary) = self.body.boundaries.get(replacement.boundary) {
                            next.reserve.insert(boundary.scope);
                        }
                        if let Some(i) = slot_index(SlotName::State) {
                            slots[i].token = next;
                            slots[i].refs = replacement.value.map(|value| before.refs[value.as_u32() as usize].clone())
                                .unwrap_or_default();
                            slots[i].contained = replacement.value.map(|value| {
                                before.contained[value.as_u32() as usize].clone()
                            }).unwrap_or_default();
                            slots[i].aliases = replacement.value.map(|value| {
                                before.aliases[value.as_u32() as usize].clone()
                            }).unwrap_or_default();
                            slots[i].identity = replacement.value
                                .and_then(|value| before.identities[value.as_u32() as usize]);
                            slots[i].requirements = replacement.deps.map(|deps| before.refs[deps.as_u32() as
                                usize].clone())
                                .unwrap_or_default();
                        }
                    }
                }
                TransferRule::ExtendReservation { pending, packet } => {
                    let part = &before.tokens[packet.as_u32() as usize];
                    let current = self.body.blocks.get(block).ok().map(|block| block.lexical_scope);
                    let pending_token = &before.tokens[pending.as_u32() as usize];
                    if !part.pieces.is_empty() {
                        if part.private || part.selected_scope != current || part.bound_admission {
                            self.error(R::B101, "exit.extend requires the closing scope's selected transferable part");
                        }
                        if let Some(selection) = pending_token.selection {
                            let (rest, _) = self.partition(&before, part, selection);
                            if !rest.pieces.is_empty() {
                                self.error(R::B101, "extension contains obligations outside current pending retention");
                            }
                        }
                    }
                    let mut token = self.union([
                        before.tokens[pending.as_u32() as usize].clone(),
                        before.tokens[packet.as_u32() as usize].clone(),
                    ], true);
                    token.selection = pending_token.selection;
                    token.pending_type = pending_token.pending_type;
                    token.pending_kind = pending_token.pending_kind;
                    token.pending_boundary = pending_token.pending_boundary;
                    token.rehome = pending_token.rehome;
                    token.scope = before.tokens[pending.as_u32() as usize].scope;
                    if let Some(i) = slot_index(SlotName::State) {
                        slots[i].token = token;
                        let index = pending.as_u32() as usize;
                        slots[i].refs = before.refs[index].clone();
                        slots[i].identity = before.identities[index];
                        slots[i].aliases = before.aliases[index].clone();
                        slots[i].contained = before.contained[index].clone();
                        slots[i].must_use = before.must_use[index];
                        slots[i].requirements = before.full_deps[index].clone();
                        slots[i].lifetime = before.lifetimes[index];
                    }
                }
                TransferRule::CompleteIdentity { identity, state, output } => {
                    let mut token = before.tokens[state.as_u32() as usize].clone();
                    let origin = self.identity(&before, *identity);
                    let mut piece = Piece::new(origin, AdmissionProvenance::UnboundResult);
                    piece.owner = token.scope;
                    if token.pieces.iter().any(|p| p.origin == origin) {
                        self.error(R::B23, "identity has already been completed");
                    }
                    token.pieces.insert(piece);
                    flow.provenance.insert(origin, FRESH & 1);
                    if let Some(i) = slot_index(*output) {
                        slots[i].token = token;
                    }
                }
                TransferRule::Protocol { inputs, .. } => {
                    if let Operation::Ownership(OwnershipOp::Dynamic { state, packet, target, reason, .. }) = op {
                        if matches!(reason, DynamicReason::Atomic | DynamicReason::Cell)
                            && !before.tokens[packet.as_u32() as usize].pieces.is_empty() {
                            self.error(R::B73, "Atomic/Cell dynamic publication cannot steal an owned packet");
                        }
                        // The transferred entries join the scope's dynamic part, which its runtime descriptor
                        // releases in serial order: no finite order covers them, so they are a family (#683).
                        let mut transferred = before.tokens[packet.as_u32() as usize].clone();
                        transferred.pieces = transferred.pieces.into_iter().map(|mut piece| {
                            piece.family = true;
                            piece
                        }).collect();
                        let mut token = self.union(inputs.iter().map(|value| if value == packet {
                            transferred.clone()
                        } else {
                            before.tokens[value.as_u32() as usize].clone()
                        }), true);
                        token.scope = Some(*target);
                        token.pieces = token.pieces.into_iter().map(|mut piece| {
                            piece.owner = Some(*target);
                            if *reason == DynamicReason::Channel {
                                piece.holder = Some(origin);
                                piece.channel = true;
                            }
                            piece
                        }).collect();
                        if let Some(i) = slot_index(SlotName::State) { slots[i].token = token; }
                        let _ = state;
                    } else if matches!(op, Operation::Call(_)) && outcome != Outcome::Normal {
                        let mut token = self.union(inputs.iter().map(|value| {
                            before.tokens[value.as_u32() as usize].clone()
                        }), true);
                        token.private = true;
                        token.pieces = token.pieces.into_iter().map(|mut piece| {
                            piece.family = true;
                            piece.holder = Some(origin);
                            piece.provenance = provenance_bit(AdmissionProvenance::StagedOrCarrierOwned);
                            piece
                        }).collect();
                        if let Some(i) = slot_index(SlotName::Receipt).or_else(|| slot_index(SlotName::Task)) {
                            slots[i].token = token;
                        } else if !token.pieces.is_empty() {
                            self.error(R::B91, "exceptional publication needs its actual task or receipt holder");
                        }
                    }
                }
                // These have state-dependent checks below; no claimed annotation
                // is accepted as a substitute for executing their transfer.
                TransferRule::Publish { .. } | TransferRule::Extract { .. }
                | TransferRule::AbortReservation { .. } | TransferRule::FrameSave { .. }
                | TransferRule::FrameResume { .. } | TransferRule::Rehome { .. } => {}
            }
        }
        self.buffer(op, outcome, &before, schema, &mut slots);
        self.stage(op, outcome, &before, flow, schema, &mut slots);
        self.results(op, schema, outcome, origin, &before, flow, &mut slots);
        self.cleanup_operation(block, op, outcome, &before, flow, schema, &mut slots);
        self.async_operation(block, op, outcome, origin, &before, flow, schema, &mut slots);
        self.storage_operation(op, outcome, origin, &before, flow, &mut slots);
        if outcome == Outcome::Normal && self.module.profile != Profile::High {
            self.descriptor_operation(op, origin, &mut slots);
        }
        for (index, (info, slot)) in schema.results.iter().zip(slots.iter_mut()).enumerate() {
            if matches!(info.ty, ResultType::Service(ServiceType::TaskState | ServiceType::Receipt
                | ServiceType::CacheState | ServiceType::Pin) | ResultType::Frame(_))
                && slot.token.service.is_none() {
                slot.token.service = Some(match origin {
                    Origin::Instruction(inst, _) => Origin::Instruction(inst, 0x8000_0000 + index as u32),
                    Origin::Invoke(block, _) => Origin::Invoke(block, 0x8000_0000 + index as u32),
                    _ => origin,
                });
            }
        }
        for postcondition in &schema.postconditions {
            if let Postcondition::DischargeMustUse(value) = postcondition {
                self.discharge_must_use(flow, *value);
            }
        }
        // A conversion renames the single must-use obligation on its normal outcome.
        // Failure keeps the source obligation in the pre-operation state.
        if outcome == Outcome::Normal
            && let Operation::Core(CoreOp::RoleWrap { value, .. } | CoreOp::RoleNarrow { value, .. }) = op
            && successor(self.module, self.body, *value) {
            let aliases = &before.must_use_aliases[value.as_u32() as usize];
            let pending = aliases.iter().any(|alias| before.must_use[alias.as_u32() as usize] == 1);
            if let Some(slot) = slots.first_mut() {
                slot.must_use = if pending { 1 } else { before.must_use[value.as_u32() as usize] };
                slot.must_use_aliases = aliases.clone();
            }
            self.discharge_must_use(flow, *value);
        }
        if matches!(op, Operation::Call(_)) {
            for operand in op.operand_refs() {
                if let OperandRef::Value(value) = operand.reference
                    && successor(self.module, self.body, value) {
                    self.discharge_must_use(flow, value);
                }
            }
        }
        slots
    }
    fn buffer(
        &mut self, op: &Operation, outcome: Outcome, before: &Flow,
        schema: &OutcomeContract, slots: &mut [Slot],
    ) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("packets::buffer");
        let Operation::Collection(op) = op else {
            return;
        };
        let (buffer, packet, inserted) = match op {
            CollectionOp::BuilderPush { buffer, packet, value } => (*buffer, packet, vec![*value]),
            CollectionOp::BuilderAppend { buffer, packet, array } => (*buffer, packet, vec![*array]),
            CollectionOp::BuilderAppendRange { buffer, packet, .. } => (*buffer, packet, vec![]),
            CollectionOp::MapBuilderPut { buffer, packet, key, value } => (*buffer, packet, vec![*key, *value]),
            CollectionOp::MapBuilderAppend { buffer, packet, map } => (*buffer, packet, vec![*map]),
            CollectionOp::BuilderFinish { buffer, packet } | CollectionOp::BuilderAbandon { buffer, packet }
            | CollectionOp::MapBuilderFinish { buffer, packet }
            | CollectionOp::MapBuilderAbandon { buffer, packet } => (*buffer, packet, vec![]),
            _ => return,
        };
        if self.module.profile != Profile::High && packet.is_none() {
            self.error(R::B81, "elaborated builder transition needs its explicit element journal");
        }
        let journal = packet.map(|value| before.tokens[value.as_u32() as usize].clone()).unwrap_or_default();
        if !before.journals[buffer.as_u32() as usize].pieces.is_subset(&journal.pieces) {
            self.error(R::B81, "builder transition loses the element journal of its current buffer version");
        }
        if outcome != Outcome::Normal {
            return;
        }
        let mut deps = before.refs[buffer.as_u32() as usize].clone();
        for value in inserted {
            deps.extend(&before.refs[value.as_u32() as usize]);
        }
        for (slot, info) in schema.results.iter().zip(slots) {
            if matches!(slot.name, SlotName::Buffer | SlotName::Value) {
                info.refs = deps.clone();
            }
            if slot.name == SlotName::Buffer {
                info.journal = journal.clone();
            }
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn results(
        &mut self, op: &Operation, schema: &OutcomeContract, outcome: Outcome, origin: Origin,
        before: &Flow, flow: &mut Flow, slots: &mut [Slot],
    ) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("packets::results");
        let normal = outcome == Outcome::Normal;
        let mut retained = BTreeSet::new();
        for retention in &schema.retention {
            let deps = self.retention(before, &retention.selection);
            if let Some(i) = schema.results.iter().position(|s| s.name == retention.result) {
                slots[i].refs.extend(&deps);
                for value in retention_values(&retention.selection) {
                    slots[i].contained.extend(&before.contained[value.as_u32() as usize]);
                    slots[i].requirements.extend(before.full_deps[value.as_u32() as usize]
                        .difference(&before.refs[value.as_u32() as usize]).copied());
                    slots[i].lifetime |= before.lifetimes[value.as_u32() as usize] & 2;
                }
                for key in &deps {
                    if let Origin::Projection(value, _) = key {
                        flow.views.insert(*key, before.refs[value.as_u32() as usize].clone());
                        slots[i].bases.insert(*value);
                    }
                }
                if let Retention::Value(value) = retention.selection {
                    slots[i].identity = before.identities[value.as_u32() as usize];
                    slots[i].aliases = before.aliases[value.as_u32() as usize].clone();
                    slots[i].contained = before.contained[value.as_u32() as usize].clone();
                    slots[i].requirements = before.full_deps[value.as_u32() as usize].clone();
                }
            }
            retained.extend(deps);
        }
        let call = super::super::summaries::call(op);
        if normal && let Some((call, receiver, _)) = call {
            self.call_results(call, receiver, origin, before, flow, schema, slots);
        }
        // A creation makes a new identity with an obligation only when its value may hold one (OIR-2 §3.2): a
        // value that merely carries deps on borrowed identities, such as a holder of an Atomic capability or of a
        // role whose implementations are all plain, is created without an ownership result (§7.2, P2.4 §4.8).
        let obligation = |ty: TypeId| self.obligations.get(ty.as_u32() as usize).copied().unwrap_or(true);
        let managed_result = schema.results.iter().find(|slot| slot.name == SlotName::Value)
            .is_some_and(|slot| match &slot.ty {
                ResultType::Exact(ty) => obligation(*ty),
                ResultType::Object { decl, .. } => self.module.types.iter().any(|(id, ty)| {
                    matches!(ty, Type::Object { decl: actual, .. } if actual == decl) && obligation(id)
                }),
                ResultType::SuccessorOf(value) => self.is_managed(*value)
                    && self.body.values.get(*value).is_ok_and(|value| obligation(value.ty)),
                _ => true,
            });
        let creates = normal && managed_result && schema.postconditions.iter().any(|p| {
            matches!(p, Postcondition::FreshAdmission(AdmissionProvenance::UnboundResult))
        }) && !matches!(op, Operation::Exit(_) | Operation::Ownership(_)
            | Operation::Core(CoreOp::ObjInit { .. }));
        let incomplete = matches!(op, Operation::Core(CoreOp::ObjNew {
            construction: ConstructionStatus::Incomplete, ..
        }));
        if incomplete && normal && let Some(i) = schema.results.iter().position(|s| s.name == SlotName::Value) {
            slots[i].refs.insert(origin);
            slots[i].identity = Some(origin);
        }
        if creates {
            let piece = Piece::new(origin, AdmissionProvenance::UnboundResult);
            if let Some(i) = schema.results.iter().position(|s| s.name == SlotName::Value) {
                slots[i].refs.insert(origin);
                slots[i].identity = Some(origin);
            }
            if !incomplete {
                if let Some(i) = schema.results.iter().position(|s| s.name == SlotName::Packet) {
                    slots[i].token.pieces.insert(piece);
                } else if self.module.profile != Profile::High {
                    self.error(R::B23, "creation needs an explicit ownership result");
                }
                flow.provenance.insert(origin, 1);
                flow.dead.remove(&origin);
            }
        }
        if normal {
            self.object_fields(op, origin, before, flow);
            if let Operation::Ownership(OwnershipOp::ScopeBegin { scope }) = op
                && let Some(slot) = slots.first_mut() {
                slot.token.scope = Some(*scope);
            }
            if let Operation::Exit(ExitOp::Commit { pending, destination, .. }) = op {
                // C-4: rehomed duties take the exit's destination, the one their rehome named. A loop
                // exit names no scope; the packet then carries the rehome's scope to its admission.
                let rehome = before.tokens[pending.as_u32() as usize].rehome;
                let landing = match (rehome, destination) {
                    (None, _) | (Some(RehomeTarget::Caller), ExitDestination::Caller) => None,
                    (Some(RehomeTarget::Scope(target)), ExitDestination::Scope(scope)) if target == *scope => None,
                    (Some(RehomeTarget::Scope(target)), ExitDestination::Loop(_)) => Some(target),
                    _ => {
                        self.error(R::B94, "rehomed cache duties reach a destination other than their rehome target");
                        None
                    }
                };
                for (schema, slot) in schema.results.iter().zip(slots.iter_mut()) {
                    if schema.name == SlotName::Packet
                        && let ExitDestination::Scope(scope) = destination {
                        slot.token.destinations.insert(*scope);
                    }
                    if schema.name == SlotName::Packet
                        && let Some(scope) = landing {
                        slot.token.destinations.insert(scope);
                    }
                    slot.refs.extend(&before.full_deps[pending.as_u32() as usize]);
                    if schema.name == SlotName::Value {
                        slot.aliases = before.aliases[pending.as_u32() as usize].clone();
                        slot.contained = before.contained[pending.as_u32() as usize].clone();
                        slot.identity = before.identities[pending.as_u32() as usize];
                        slot.requirements = before.full_deps[pending.as_u32() as usize].clone();
                        slot.lifetime = before.lifetimes[pending.as_u32() as usize];
                    }
                    slot.token.pieces = slot.token.pieces.iter().cloned().map(|mut piece| {
                        // A rehomed cache duty and the packets its cache holds stay cache-owned
                        // (C-4, OIR-2 §10.2): the commit hands them over, it admits nothing fresh.
                        if piece.holder.is_none() {
                            piece.provenance = 1;
                        }
                        piece.owner = None;
                        piece.journal = false;
                        if !matches!(piece.origin, Origin::Input(_)) {
                            flow.provenance.insert(piece.origin, 1);
                        }
                        piece
                    }).collect();
                }
            }
            if let Operation::Exit(ExitOp::Prepare { exit }) = op
                && let Some(value) = exit.value
                && let Some(slot) = slots.first_mut() {
                slot.refs = before.refs[value.as_u32() as usize].clone();
                slot.identity = before.identities[value.as_u32() as usize];
                slot.aliases = before.aliases[value.as_u32() as usize].clone();
                slot.contained = before.contained[value.as_u32() as usize].clone();
                if successor(self.module, self.body, value) {
                    self.discharge_must_use(flow, value);
                }
                slot.requirements = before.full_deps[value.as_u32() as usize].clone();
                slot.lifetime = before.lifetimes[value.as_u32() as usize];
            }
        }
        let mut subset = BTreeSet::new();
        for retention in &schema.retention {
            let opaque = match &retention.selection {
                Retention::Difference { removed, .. } => !removed.is_empty(),
                Retention::MapMerge(items) => items.len() > 1,
                Retention::Entries(items) => items.len() > 1,
                _ => false,
            };
            if opaque && let Some(index) = schema.results.iter().position(|slot| slot.name == retention.result) {
                subset.insert(index);
            }
        }
        if matches!(op, Operation::Collection(CollectionOp::MapBuilderPut {
            ..
        }
            | CollectionOp::MapBuilderAppend { .. })) && normal {
            for (index, slot) in schema.results.iter().enumerate() {
                if slot.name == SlotName::Buffer {
                    subset.insert(index);
                }
            }
        }
        for index in subset {
            if slots[index].refs.is_empty() {
                continue;
            }
            let view = view_origin(origin, index);
            flow.views.insert(view, slots[index].refs.clone());
            slots[index].refs = BTreeSet::from([view]);
            slots[index].identity = None;
            slots[index].aliases.clear();
        }
        if super::super::summaries::call(op).is_none() {
            retained = schema.results.iter().zip(slots.iter()).filter(|(slot, _)| {
                matches!(slot.name, SlotName::Value | SlotName::Buffer)
            }).flat_map(|(_, slot)| slot.refs.iter().copied()).collect();
        }
        for (i, slot) in slots.iter_mut().enumerate() {
            if schema.results[i].ty == ResultType::Service(ServiceType::Failure) {
                let primary = match op {
                    Operation::Ownership(OwnershipOp::ScopeUnwind { failure, .. }) => Some(*failure),
                    Operation::Core(CoreOp::FailSecondary { primary, .. }) => Some(*primary),
                    Operation::Core(CoreOp::FailCaused { new, .. }) => Some(*new),
                    _ => None,
                };
                slot.token.failure_origin = primary.and_then(|value| {
                    before.tokens[value.as_u32() as usize].failure_origin
                }).or(if outcome == Outcome::Normal { flow.active_failure_origin }
                    else { flow.latest_failure_origin });
                if matches!(op, Operation::Core(CoreOp::FailSecondary { .. } | CoreOp::FailCaused { .. }))
                    && before.active_failure_origin.is_some()
                    && before.active_failure_origin != slot.token.failure_origin {
                    self.error(R::B104, "cleanup combination cannot change its primary Error identity");
                }
                slot.failures = match op {
                    Operation::Ownership(OwnershipOp::ScopeUnwind { failure, .. }) => {
                        before.failures[failure.as_u32() as usize]
                    }
                    Operation::Core(CoreOp::FailSecondary { primary, .. }) => {
                        before.failures[primary.as_u32() as usize]
                    }
                    Operation::Core(CoreOp::FailCaused { new, .. }) => {
                        before.failures[new.as_u32() as usize]
                    }
                    _ => if outcome == Outcome::Normal { flow.active_failure } else { flow.latest_failure },
                };
            }
            if matches!(schema.results[i].ty, ResultType::SuccessorOf(_)) {
                slot.must_use = 1;
            }
            if slot.refs.is_empty() && schema.results[i].name == SlotName::Deps {
                slot.refs.extend(&retained);
            }
        }
    }
    pub(super) fn facts(
        &mut self, flow: &Flow, op: &Operation, facts: &InstructionFacts, values: &[ValueId],
    ) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("packets::facts");
        for (index, dep) in &facts.result_deps {
            if let Some(value) = values.get(usize::from(*index)) {
                let stated = self.dependencies(flow, *dep);
                if !flow.full_deps[value.as_u32() as usize].is_subset(&stated) {
                    self.error(R::B32, "instruction result_deps omits a required dependency");
                }
            } else {
                self.error(R::B32, "instruction result_deps names an absent result");
            }
        }
        for fact in &facts.provenance {
            let allowed = provenance_bit(fact.state);
            let mut found = false;
            for value in flow.token_values() {
                if !self.scan_work(1) {
                    return;
                }
                if flow.availability[value.as_u32() as usize] != AVAILABLE {
                    continue;
                }
                let (_, selected) = self.partition(flow, &flow.tokens[value.as_u32() as usize], fact.obligations);
                if !selected.pieces.is_empty() {
                    found = true;
                }
                if selected.pieces.iter().any(|piece| piece.provenance != allowed) {
                    self.error(R::B23, "provenance annotation is not justified by an executable transition");
                }
            }
            if !found {
                self.error(R::B23, "provenance annotation has no executable ownership witness");
            }
        }
        if matches!(op, Operation::Ownership(OwnershipOp::Move { .. } | OwnershipOp::Join { .. }))
            && facts.provenance.iter().any(|f| f.state == AdmissionProvenance::UnboundResult) {
            let (keys, _) = self.selection(flow, facts.provenance[0].obligations);
            if keys.is_empty() {
                self.error(R::B23, "a metadata fact cannot create an obligation");
            }
        }
    }
    fn object_fields(&mut self, op: &Operation, origin: Origin, before: &Flow, flow: &mut Flow) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("packets::object_fields");
        let mut entries = Vec::new();
        match op {
            Operation::Core(CoreOp::ObjNew { declaration, fields, .. }) => {
                for (index, value) in fields.iter().enumerate() {
                    entries.push((origin, *declaration, index as u32, *value));
                }
            }
            Operation::Core(CoreOp::ObjInit { object, field, value, .. }) => {
                entries.push((self.identity(before, *object), field.declaration, field.index, *value));
            }
            Operation::Core(CoreOp::ObjNext { object, updates, .. }) => {
                let identity = self.identity(before, *object);
                for ((key, declaration, index), field) in &before.fields {
                    if *key == identity {
                        flow.fields.insert((origin, *declaration, *index), field.clone());
                    }
                }
                for update in updates {
                    entries.push((origin, update.field.declaration, update.field.index, update.value));
                }
            }
            _ => {}
        }
        for (key, declaration, index, value) in entries {
            flow.fields.insert((key, declaration, index),
                (before.refs[value.as_u32() as usize].clone(), before.identities[value.as_u32() as usize]));
        }
    }
    fn retention(&mut self, flow: &Flow, retention: &Retention) -> BTreeSet<Origin> {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("packets::retention");
        if let Retention::FullDeps(value) = retention {
            return flow.full_deps[value.as_u32() as usize].clone();
        }
        let projection = match retention {
            Retention::Field { base, field } => return self.field(flow, *base, *field),
            Retention::Index { base, index } => Some((*base, ProjectionKind::Index(*index))),
            Retention::Slice { base, start, end } => Some((*base, ProjectionKind::Slice(*start, *end))),
            Retention::MapEntry { map, key } => Some((*map, ProjectionKind::Map(*key))),
            Retention::MapPosition { map, index, key } => Some((*map, ProjectionKind::MapIndex(*index, *key))),
            Retention::Keys(value) => Some((*value, ProjectionKind::Keys)),
            Retention::Values(value) => Some((*value, ProjectionKind::Values)),
            Retention::ArrayRemove { array, index } => Some((*array, ProjectionKind::Remove(*index))),
            _ => None,
        };
        if let Some((value, kind)) = projection {
            if flow.refs[value.as_u32() as usize].is_empty() {
                return BTreeSet::new();
            }
            return BTreeSet::from([Origin::Projection(value, kind)]);
        }
        if let Retention::ArrayReplace { array, index, value } = retention {
            let mut refs = flow.refs[value.as_u32() as usize].clone();
            if !flow.refs[array.as_u32() as usize].is_empty() {
                refs.insert(Origin::Projection(*array, ProjectionKind::Remove(*index)));
            }
            return refs;
        }
        let values = retention_values(retention);
        let mut refs: BTreeSet<_> = values.iter().flat_map(|v| flow.refs[v.as_u32() as usize].iter().copied())
            .collect();
        if let Retention::ObjectFields { object, updates } = retention {
            refs.clear();
            let identity = self.identity(flow, *object);
            let declaration = self.body.values.get(*object).ok().and_then(|v| self.module.types.get(v.ty).ok())
                .and_then(|ty| match ty {
                    Type::Object { decl, .. } => Some(*decl),
                    _ => None,
                });
            if let Some(declaration) = declaration
                && let Ok(Declaration { kind: DeclarationKind::Object { fields, .. }, .. }) =
                    self.module.declarations.get(declaration) {
                for index in 0..fields.len() {
                    if let Some(update) = updates.iter().find(|u| u.field.index as usize == index) {
                        refs.extend(&flow.refs[update.value.as_u32() as usize]);
                    } else {
                        refs.extend(self.field(flow, *object, FieldRef { declaration, index: index as u32 }));
                    }
                }
            }
            // The new identity is inserted by the creation transition below.
            refs.remove(&identity);
        }
        refs
    }
}

fn retention_values(retention: &Retention) -> Vec<ValueId> {
    match retention {
        Retention::FullDeps(v) | Retention::Value(v) | Retention::OwnerGraph(v)
        | Retention::ViewOwner(v) | Retention::Cache(v) | Retention::Keys(v) | Retention::Values(v) => vec![*v],
        Retention::Field { base, .. } | Retention::Index { base, .. } | Retention::Slice { base, .. }
        | Retention::Difference { base, .. } => vec![*base],
        Retention::MapEntry { map, .. } | Retention::MapPosition { map, .. } => vec![*map],
        Retention::ArrayReplace { array, value, .. } => vec![*array, *value],
        Retention::ArrayRemove { array, .. } => vec![*array],
        Retention::MapMerge(items) | Retention::Union(items) | Retention::CarrierGraph(items) => items.clone(),
        Retention::ObjectFields { object, updates } => {
            std::iter::once(*object).chain(updates.iter().map(|f| f.value)).collect()
        }
        Retention::Entries(items) => items.iter().flat_map(|(k, v)| [*k, *v]).collect(),
        Retention::Descriptor { value, .. } => vec![*value],
        Retention::TakenOutcome { future, .. } | Retention::AwaitFailure { future, .. } => vec![*future],
        Retention::Global(_) => vec![],
    }
}
