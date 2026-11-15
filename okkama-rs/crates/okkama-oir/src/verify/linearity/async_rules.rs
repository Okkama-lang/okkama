use super::*;

impl Engine<'_, '_> {
    /// The start of a D21 async body (C-2): its frame parameter and the continuation 0 resume that opens
    /// the entry block, with the restored slots and their result values.
    pub(super) fn start_resume(&self) -> Option<(ValueId, &[SavedSlot], &[ValueId])> {
        if !matches!(self.module.profile, Profile::Coro | Profile::Low)
            || !self.declaration.attrs.contains(&FunctionAttr::Async)
            || self.body.frame_layout.is_none() {
            return None;
        }
        let entry = self.body.blocks.get(self.body.entry).ok()?;
        let [frame] = entry.params.as_slice() else {
            return None;
        };
        let inst = self.body.insts.get(*entry.insts.first()?).ok()?;
        match &inst.op {
            Operation::Async(AsyncOp::FrameResume { frame: resumed, continuation: 0, slots })
                if resumed == frame && inst.results.len() == slots.len() + 1 => {
                Some((*frame, slots, &inst.results[1..]))
            }
            _ => None,
        }
    }

    /// An async body of coro/low in the D21 form: it starts from its frame and leaves through `fut.publish`.
    pub(super) fn frame_body(&self) -> bool {
        matches!(self.module.profile, Profile::Coro | Profile::Low)
            && self.declaration.attrs.contains(&FunctionAttr::Async)
            && self.body.frame_layout.is_some()
    }

    /// C-2: `fut.publish` is the exit of a D21 async body. Its success outcome is checked against the body
    /// summary as a return of the value and the packet, its failure as a failure exit.
    fn publish_exit(&mut self, before: &Flow, frame: ValueId, outcome: &PublishOutcome) {
        // C-2: the body publishes after its cleanup, with every lexical scope closed (OIR-2 §10.1).
        if before.scopes.values().any(|status| *status == 1) {
            self.error(R::B41, "fut.publish runs while a lexical scope of the body is still open (C-2)");
        }
        // B9.5, D20: a linear token saved in the frame comes back through its resume; publication consumes the
        // frame, so a linear slot still occupied there would be lost.
        let hidden = before.tokens[frame.as_u32() as usize].frame.and_then(|key| before.frames.get(&key))
            .is_some_and(|layout| layout.values().any(|slot| slot.token.saved_type.is_some_and(|ty| {
                super::super::contracts::linear(self.module, ty)
            })));
        if hidden {
            self.error(R::B95, "fut.publish consumes a frame that still holds a linear token");
        }
        match outcome {
            PublishOutcome::Success { value, packet, .. } => {
                let values: Vec<_> = value.iter().copied().chain([*packet]).collect();
                self.body_summary(before, true, &values);
            }
            PublishOutcome::Failure(failure) => {
                let mut failed = before.clone();
                failed.active_failure = before.failures[failure.as_u32() as usize];
                self.body_summary(&failed, false, &[]);
            }
        }
    }

    /// C-3: the task and the receipt of an await unwind edge reach only `await.unwind`; neither is a service duty
    /// that a close or a settle step may take.
    pub(super) fn await_handle(&self, before: &Flow, value: ValueId) -> bool {
        before.tokens[value.as_u32() as usize].cache.is_some()
            && matches!(super::super::later::value_type(self.module, self.body, value),
                Some(Type::Service(ServiceType::TaskState | ServiceType::Receipt)))
    }

    /// Entry values of the function. The receiver, arguments, descriptors and input packet of a D21 async
    /// body are the results of its start resume (C-2); every other body has them as entry parameters.
    pub(super) fn entry_values(&self) -> Vec<ValueId> {
        if let Some((_, _, results)) = self.start_resume() {
            return results.to_vec();
        }
        self.body.blocks.get(self.body.entry).map(|entry| entry.params.clone()).unwrap_or_default()
    }

    /// C-1, C-2: the runtime writes the start slots into the frame before the body runs. The running
    /// frame is the body's own activation, never a service duty of its owner, so closes do not settle it.
    pub(super) fn start_frame(&self, flow: &mut Flow) {
        let Some((frame, slots, results)) = self.start_resume() else {
            return;
        };
        let key = Origin::Input(frame);
        let index = frame.as_u32() as usize;
        flow.availability[index] = AVAILABLE;
        flow.aliases[index].insert(frame);
        flow.contained[index].insert(frame);
        let mut token = Token { frame: Some(key), ..Token::default() };
        token.sources.insert(frame);
        flow.tokens[index] = token;
        let mut layout = std::collections::BTreeMap::new();
        for (slot, &value) in slots.iter().zip(results) {
            let index = value.as_u32() as usize;
            let mut token = flow.tokens[index].clone();
            token.saved_type = Some(slot.ty);
            layout.insert(slot.slot, Slot {
                token, refs: flow.refs[index].clone(), identity: flow.identities[index],
                aliases: flow.aliases[index].clone(), contained: flow.contained[index].clone(),
                requirements: flow.full_deps[index].clone(), lifetime: flow.lifetimes[index],
                must_use: flow.must_use[index], failures: flow.failures[index], ..Slot::default()
            });
            // The start resume defines the value from its slot.
            flow.availability[index] = ABSENT;
        }
        flow.frames.insert(key, layout);
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn async_operation(
        &mut self, block: BlockId, op: &Operation, outcome: Outcome, origin: Origin, before: &Flow,
        flow: &mut Flow, schema: &OutcomeContract, slots: &mut [Slot],
    ) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("async_rules::async_operation");
        if let Operation::Ownership(OwnershipOp::DynamicCheck { key, snapshot }) = op {
            let keys = &before.refs[key.as_u32() as usize];
            if before.tokens[snapshot.as_u32() as usize].snapshot.as_ref()
                .is_none_or(|(saved, stale)| *stale || saved != keys || saved.is_empty()) {
                self.error(R::B72, "dynamic.check requires a current snapshot for this exact stable key");
            }
        }
        if let Operation::Async(AsyncOp::Await { future, .. } | AsyncOp::AwaitShielded { future, .. }
            | AsyncOp::AwaitTake { future, .. }) = op
            && before.taken_caches.contains(&self.identity(before, *future)) {
            self.error(R::B91, "a taken cache cannot publish another view or ownership packet");
        }
        if outcome != Outcome::Normal {
            if let Operation::Async(AsyncOp::Await { future, .. } | AsyncOp::AwaitShielded { future, .. }
                | AsyncOp::AwaitTake { future, .. }) = op {
                let key = self.identity(before, *future);
                for (info, slot) in schema.results.iter().zip(slots.iter_mut()) {
                    if matches!(info.name, SlotName::Task | SlotName::Receipt) {
                        slot.token.cache = Some(key);
                        slot.token.private = true;
                    }
                }
            }
            return;
        }
        if let Operation::Ownership(OwnershipOp::DynamicRefresh { key, access }) = op {
            let keys = before.refs[key.as_u32() as usize].clone();
            if keys.is_empty() || keys.iter().any(|key| before.dead.contains(key))
                || before.availability[access.owner.as_u32() as usize] != AVAILABLE
                || before.availability[access.holder.as_u32() as usize] != AVAILABLE {
                self.error(R::B72, "snapshot refresh needs live access to the current owner and holder");
            }
            if let Some(slot) = slots.first_mut() {
                slot.token.snapshot = Some((keys, false));
            }
        }
        if let Operation::Ownership(OwnershipOp::ScopeTransfer {
            states: TransferStates::Between { from, to }, ..
        }) = op
            && before.tokens[from.as_u32() as usize].scope != before.tokens[to.as_u32() as usize].scope {
                let moved: BTreeSet<_> = schema.results.iter().zip(slots.iter()).filter(|(info, _)| {
                    info.name == SlotName::To
                }).flat_map(|(_, slot)| slot.token.pieces.iter().filter(|piece| {
                    before.tokens[from.as_u32() as usize].pieces.iter().any(|old| {
                        old.origin == piece.origin && old.owner != piece.owner
                    })
                }).map(|piece| piece.origin)).collect();
                for token in flow.tokens.stored_mut() {
                    if let Some((keys, stale)) = &mut token.snapshot {
                        *stale |= !keys.is_disjoint(&moved);
                    }
                }
        }
        let Operation::Async(op) = op else {
            return;
        };
        if before.frame_conflict && matches!(op, AsyncOp::FrameSave { .. } | AsyncOp::FrameResume { .. }
            | AsyncOp::FutureNew { .. }) {
            self.error(R::B95, "occupied frame families need producer layout/continuation metadata");
        }
        match op {
            AsyncOp::Pin { deps, owner, .. } => {
                for (info, slot) in schema.results.iter().zip(slots.iter_mut()) {
                    if info.name == SlotName::State {
                        slot.token = before.tokens[owner.as_u32() as usize].clone();
                    }
                    if info.name == SlotName::Pin {
                        slot.token.scope = before.tokens[owner.as_u32() as usize].scope;
                        slot.token.pin_deps = before.refs[deps.as_u32() as usize].clone();
                    }
                }
            }
            AsyncOp::PinTransfer { pin, owner, .. } | AsyncOp::Unpin { pin, owner, .. } => {
                for (info, slot) in schema.results.iter().zip(slots.iter_mut()) {
                    if info.name == SlotName::To {
                        slot.token = before.tokens[owner.as_u32() as usize].clone();
                    }
                    if info.name == SlotName::State {
                        slot.token = before.tokens[pin.as_u32() as usize].clone();
                        slot.token.scope = before.tokens[owner.as_u32() as usize].scope;
                    }
                }
            }
            AsyncOp::CacheRehome { form: RehomeForm::State(state), target, .. } => {
                let mut token = before.tokens[state.as_u32() as usize].clone();
                token.scope = Some(*target);
                let changed = before.tokens[state.as_u32() as usize].scope != Some(*target);
                if changed && before.scopes.get(target) != Some(&1) {
                    self.error(R::B94, "cache rehome requires a live sufficient target boundary");
                }
                if let Some(key) = token.cache.filter(|_| changed) {
                    for (index, holder) in flow.tokens.indexed_mut() {
                        if flow.availability[index] != AVAILABLE {
                            continue;
                        }
                        let moved: Vec<_> = holder.pieces.iter().filter(|piece| piece.holder == Some(key))
                            .cloned().collect();
                        for mut piece in moved {
                            holder.pieces.remove(&piece);
                            piece.owner = Some(*target);
                            token.pieces.insert(piece);
                        }
                        if let Some((keys, stale)) = &mut holder.snapshot
                            && keys.contains(&key) {
                            *stale = true;
                        }
                    }
                }
                if let Some(slot) = slots.first_mut() {
                    slot.token = token;
                }
            }
            AsyncOp::FutureSettle { task, .. } => {
                let mut token = before.tokens[task.as_u32() as usize].clone();
                if token.settled {
                    self.error(R::B91, "waiter task cannot be settled twice");
                }
                token.settled = true;
                if let Some(slot) = slots.first_mut() {
                    slot.token = token;
                }
            }
            AsyncOp::UniqueCacheProof { proof } => {
                self.unique_proof(block, proof, before);
                if let Some(slot) = slots.first_mut() {
                    slot.token.unique = Some(self.identity(before, proof.future));
                    slot.token.unique_region.extend(&proof.region);
                }
            }
            AsyncOp::AwaitTake { future, unique, boundary, .. } => {
                let key = self.identity(before, *future);
                let capability = &before.tokens[unique.as_u32() as usize];
                if capability.unique != Some(key) || !capability.unique_region.contains(&block) {
                    self.error(R::B92, "await.take capability must prove this exact Future/cache and region");
                }
                let mut extracted = Token::default();
                for (index, token) in flow.tokens.indexed_mut() {
                    if flow.availability[index] != AVAILABLE {
                        continue;
                    }
                    let owned: Vec<_> = token.pieces.iter().filter(|piece| {
                        piece.holder == Some(key) && piece.origin != key
                    }).cloned().collect();
                    for piece in owned {
                        token.pieces.remove(&piece);
                        let mut piece = piece;
                        piece.holder = None;
                        piece.provenance = provenance_bit(match boundary {
                            TakeBoundary::Temporary { .. } => AdmissionProvenance::TemporaryAwaitResult,
                            _ => AdmissionProvenance::ExtractedAtExistingBoundary,
                        });
                        extracted.pieces.insert(piece);
                    }
                }
                flow.taken_caches.insert(key);
                for (info, slot) in schema.results.iter().zip(slots.iter_mut()) {
                    if info.name == SlotName::Packet {
                        slot.token = extracted.clone();
                    }
                }
                if matches!(boundary, TakeBoundary::Temporary { .. })
                    && before.provenance.get(&key).is_none_or(|provenance| provenance & !FRESH != 0) {
                    self.error(R::B93, "temporary take cannot change a bound Future's cleanup boundary");
                }
                if let TakeBoundary::Existing { owner } = boundary {
                    let key = self.identity(before, *future);
                    let actual = before.tokens.iter().flat_map(|token| &token.pieces)
                        .find(|piece| piece.origin == key).and_then(|piece| piece.owner);
                    if actual != Some(*owner) {
                        self.error(R::B93, "bound extraction must preserve its original owner boundary");
                    }
                }
            }
            AsyncOp::FrameAlloc { .. } => {
                flow.frames.insert(origin, std::collections::BTreeMap::new());
                if let Some(slot) = slots.first_mut() {
                    slot.token.frame = Some(origin);
                }
            }
            AsyncOp::FrameLoad { frame, slot, ty } => {
                let content = before.tokens[frame.as_u32() as usize].frame
                    .and_then(|key| before.frames.get(&key)).and_then(|layout| layout.get(slot));
                if let Some(content) = content {
                    if content.token.saved_type != Some(*ty)
                        || super::super::contracts::linear(self.module, *ty) {
                        self.error(R::B95, "raw frame load cannot reinterpret or copy a saved linear slot");
                    }
                    if let Some(result) = slots.first_mut() {
                        *result = content.clone();
                    }
                }
            }
            AsyncOp::FrameStore { frame, slot, value } => {
                let frame_key = before.tokens[frame.as_u32() as usize].frame;
                let mut layout = frame_key.and_then(|key| before.frames.get(&key).cloned()).unwrap_or_default();
                if layout.get(slot).is_some_and(|content| content.token.saved_type.is_some_and(|ty| {
                    super::super::contracts::linear(self.module, ty)
                })) { self.error(R::B95, "raw frame store cannot overwrite a saved linear slot"); }
                let index = value.as_u32() as usize;
                let mut token = before.tokens[index].clone();
                token.saved_type = self.body.values.get(*value).ok().map(|value| value.ty);
                layout.insert(*slot, Slot {
                    token, refs: before.refs[index].clone(), requirements: before.full_deps[index].clone(),
                    identity: before.identities[index], ..Slot::default()
                });
                if let Some(key) = frame_key {
                    flow.frames.insert(key, layout);
                }
            }
            AsyncOp::FrameSave { frame, slots: saved, .. } => {
                let frame_key = before.tokens[frame.as_u32() as usize].frame;
                if frame_key.is_none() {
                    self.error(R::B95, "save needs an allocated or forwarded frame");
                }
                let mut layout = frame_key.and_then(|key| before.frames.get(&key).cloned()).unwrap_or_default();
                let mut seen = BTreeSet::new();
                for slot in saved {
                    let value = slot.value.as_u32() as usize;
                    if layout.get(&slot.slot).is_some_and(|old| old.token.saved_type.is_some_and(|ty| {
                        super::super::contracts::linear(self.module, ty)
                    })) {
                        self.error(R::B95, "save overwrites an occupied linear slot");
                    }
                    let mut token = before.tokens[value].clone();
                    token.saved_type = self.body.values.get(slot.value).ok().map(|data| data.ty);
                    layout.insert(slot.slot, Slot {
                        token, refs: before.refs[value].clone(),
                        identity: before.identities[value], aliases: before.aliases[value].clone(),
                        requirements: before.full_deps[value].clone(), lifetime: before.lifetimes[value],
                        ..Slot::default()
                    });
                    if !seen.insert(slot.slot) {
                        self.error(R::B95, "frame.save has duplicate slots");
                    }
                    if linear(self.module, self.body, slot.value) && slot.mode != OperandMode::Consume {
                        self.error(R::B95, "linear execution state must move into its frame slot");
                    }
                }
                if let Some(key) = frame_key {
                    flow.frames.insert(key, layout);
                }
                if let Some(slot) = slots.first_mut() {
                    slot.token = before.tokens[frame.as_u32() as usize].clone();
                }
            }
            AsyncOp::FrameResume { frame, slots: saved, .. } => {
                let frame_key = before.tokens[frame.as_u32() as usize].frame;
                let mut layout = frame_key.and_then(|key| before.frames.get(&key).cloned()).unwrap_or_default();
                if frame_key.is_none() {
                    self.error(R::B95, "resume needs the matching saved frame");
                }
                let mut seen = BTreeSet::new();
                for (index, slot) in saved.iter().enumerate() {
                    let actual = self.module.types.get(slot.ty).is_ok_and(|ty| {
                        matches!(ty, Type::Frame(_) | Type::Builder(_) | Type::MapBuilder(..))
                            || matches!(ty, Type::Service(service) if service.is_linear())
                    });
                    if let Some(content) = layout.get(&slot.slot).cloned() {
                        if content.token.saved_type != Some(slot.ty) {
                            self.error(R::B95, "resume slot type disagrees with the actual saved value");
                        }
                        if let Some(result) = slots.get_mut(index + 1) {
                            *result = content;
                        }
                        if actual {
                            layout.remove(&slot.slot);
                        }
                    } else { self.error(R::B95, "resume reads an empty/already moved frame slot"); }
                    if actual != slot.linear || !seen.insert(slot.slot) {
                        self.error(R::B95, "resume slot type/linearity or multiplicity is invalid");
                    }
                }
                if let Some(key) = frame_key {
                    flow.frames.insert(key, layout);
                }
                if let Some(slot) = slots.first_mut() {
                    slot.token = before.tokens[frame.as_u32() as usize].clone();
                }
            }
            AsyncOp::FutureNew { packet, owner_state, environment, .. } => {
                let packet = &before.tokens[packet.as_u32() as usize];
                let owner = &before.tokens[owner_state.as_u32() as usize];
                let mut input = packet.clone();
                input.pieces = input.pieces.into_iter().map(|mut piece| {
                    piece.holder = Some(origin);
                    piece
                }).collect();
                // B10.6, D16/D25: one `fut.new` site names one static duty of its owner. Reached again while the
                // owner still holds that duty (a loop inside the owner), its Futures need dynamic entries (R1).
                let retired = match origin {
                    Origin::Instruction(inst, slot) => Origin::RetiredInstruction(inst, slot),
                    other => other,
                };
                if owner.pieces.iter().any(|piece| piece.origin == origin || piece.origin == retired) {
                    self.error(R::B106, "fut.new reaches its owner again while the owner holds its earlier duty (D25)");
                }
                let mut token = self.union([input, owner.clone()], true);
                token.scope = owner.scope;
                let mut duty = Piece::new(origin, AdmissionProvenance::CacheOwned);
                duty.owner = owner.scope;
                duty.holder = Some(origin);
                token.pieces.insert(duty);
                flow.cache_keys.insert(origin);
                flow.provenance.insert(origin, provenance_bit(AdmissionProvenance::UnboundResult));
                for (result, slot) in schema.results.iter().zip(slots.iter_mut()) {
                    match result.name {
                        SlotName::To => slot.token = token.clone(),
                        SlotName::State => {
                            slot.token = Token {
                                private: true, cache: Some(origin), scope: owner.scope, ..Token::default()
                            };
                            if let FutureEnvironment::Frame(frame) = environment {
                                slot.token.frame = before.tokens[frame.as_u32() as usize].frame;
                            }
                        }
                        SlotName::Value => {
                            slot.refs.insert(origin);
                            slot.identity = Some(origin);
                        }
                        _ => {}
                    }
                }
            }
            // `fut.publish` (C-2) has no result slot; in a D21 body it is the exit checked by the summary.
            AsyncOp::FuturePublish { frame, outcome } if self.frame_body() => {
                self.publish_exit(before, *frame, outcome);
            }
            _ => {}
        }
    }

    fn unique_proof(&mut self, block: BlockId, proof: &UniqueCacheProof, before: &Flow) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("async_rules::unique_proof");
        let evidence = &proof.evidence;
        if !proof.region.contains(&block) || !evidence.no_external_storage || !evidence.no_other_waiters
            || !evidence.no_live_views || !evidence.no_collector_retention || !evidence.scheduler_cannot_publish {
            self.error(R::B92, "unique-cache evidence is incomplete");
        }
        let creation = match evidence.creation {
            CreationOrigin::Instruction(inst) => self.body.insts.get(inst).ok().map(|inst| &inst.op),
            CreationOrigin::Invoke { block, outcome: Outcome::Normal, .. } => self.body.blocks.get(block).ok()
                .and_then(|block| if let TermKind::InvokeOwn { op, .. } = &block.term.kind { Some(&**op) }
                    else { None }),
            _ => None,
        };
        let expected_origin = match evidence.creation {
            CreationOrigin::Instruction(inst) => Origin::Instruction(inst, 0),
            CreationOrigin::Invoke { block, slot: 0, .. } => Origin::Invoke(block, 0),
            _ => Origin::External(proof.future),
        };
        if self.identity(before, proof.future) != expected_origin {
            self.error(R::B92, "proof creation does not produce the proved Future");
        }
        if !matches!(creation, Some(Operation::Async(AsyncOp::FutureNew { .. }))) {
            self.error(R::B92, "proof must refer to an actual Future creation");
        }
        let aliases = &before.aliases[proof.future.as_u32() as usize];
        // R2: a name that a suspension saved is the frame slot's retention; the evidence accounts for it by the
        // slot, and coro forbids reading it after the save (B9.5).
        let in_slot = |alias: ValueId| self.body.insts.iter().any(|(_, inst)| matches!(&inst.op,
            Operation::Async(AsyncOp::FrameSave { slots, .. })
                if slots.iter().any(|slot| slot.value == alias && evidence.frame_slots.contains(&slot.slot))));
        if aliases.iter().any(|alias| *alias != proof.future && !evidence.aliases.contains(alias) && !in_slot(*alias)) {
            self.error(R::B92, "unique-cache evidence omits an existing alias");
        }
        for (value, _) in self.body.values.iter() {
            if before.availability[value.as_u32() as usize] == AVAILABLE
                && before.tokens[value.as_u32() as usize].unique == Some(self.identity(before, proof.future)) {
                self.error(R::B92, "cache has two simultaneous unique capabilities");
            }
        }
        for (_, candidate) in self.body.blocks.iter() {
            for &id in &candidate.insts {
                if !self.state.step(1) {
                    return;
                }
                let Ok(inst) = self.body.insts.get(id) else {
                    continue;
                };
                let uses = inst.op.operand_refs().iter().any(|operand| {
                    matches!(operand.reference, OperandRef::Value(value) if aliases.contains(&value))
                });
                // A frame save retains the cache in a slot; the evidence lists the slot (below).
                if uses && !matches!(inst.op, Operation::Async(AsyncOp::UniqueCacheProof { .. }
                    | AsyncOp::AwaitTake { .. } | AsyncOp::FutureDone { .. } | AsyncOp::FutureCancelled { .. }
                    | AsyncOp::FrameSave { .. })) {
                    self.error(R::B92, "cache is published, retained, shared-awaited or stored in the function");
                }
                if let Operation::Async(AsyncOp::FrameSave { slots, .. }) = &inst.op {
                    for slot in slots {
                        if aliases.contains(&slot.value) && !evidence.frame_slots.contains(&slot.slot) {
                            self.error(R::B92, "coro proof omits a retained frame slot");
                        }
                    }
                }
            }
            if let TermKind::InvokeOwn { op, .. } = &candidate.term.kind {
                let uses = op.operand_refs().iter().any(|operand| {
                    matches!(operand.reference, OperandRef::Value(value) if aliases.contains(&value))
                });
                if uses && !matches!(**op, Operation::Async(AsyncOp::AwaitTake { .. })) {
                    self.error(R::B92, "invoke can publish another observer of the unique cache");
                }
            }
        }
    }
}
