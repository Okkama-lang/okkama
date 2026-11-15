//! Async bodies (Champ draft 3 §§3, 13; ABI D20 §2.2; plan decision C4). A body runs in the same stack storage
//! as a synchronous function and takes `(ctx, frame) -> 0 | 1`.
//!
//! SPB liveness ends a saved value at its `frame.save`: the allocation may give its slot to a later value and
//! no stack map of the suspending step keeps it. So `frame.save` moves the saved values into their frame slots
//! and `frame.resume` moves them back, clearing the words; the frame is traced by its map. At PENDING the step's
//! operands and the scope area of the function's scopes join them, RESUME = k, and the body returns 1. Resume
//! dispatch reloads and clears the operands and the scope area, then re-invokes the step, which continues
//! through its own progress record. A suspended frame therefore holds exactly `saved ∪ operands` of its
//! continuation and its scope area.
use super::*;
use cranelift_frontend::Switch;

impl Emit<'_, '_> {
    fn layout(&self) -> &s::FrameLayout {
        let program: &s::Program = self.program;
        program.frame_layout(self.source).expect("an async body has a frame layout")
    }

    fn map(&self) -> &c::FrameMap {
        self.frame_map.expect("an async body has a frame map")
    }

    /// The continuation whose site is the current instruction.
    fn site(&self) -> usize {
        let pc = self.pc.expect("site PC") as u32;
        self.layout().continuations.iter().position(|continuation| continuation.site == Some(pc))
            .expect("verified suspension site")
    }

    /// Move the words of `location` into frame slot `slot`.
    fn save_slot(&mut self, slot: u32, location: L) {
        let offset = self.map().slots[slot as usize].offset as usize;
        for (word, value) in self.read(location).into_iter().enumerate() {
            self.store(self.body, offset + word * 8, value);
        }
    }

    /// Move frame slot `slot` into `location` and clear its words.
    fn reload_slot(&mut self, slot: u32, location: L) {
        let map = self.map();
        let offset = map.slots[slot as usize].offset as usize;
        let words = map.slots[slot as usize].reprs.len();
        let zero = self.iconst(0);
        let values = (0..words).map(|word| self.load(self.body, offset + word * 8)).collect::<Vec<_>>();
        for word in 0..words {
            self.store(self.body, offset + word * 8, zero);
        }
        self.write(location, &values);
    }

    /// Store (or reload and clear) the two words of each lexical scope of the function.
    fn scope_area(&mut self, save: bool) {
        let zero = self.iconst(0);
        for &(scope, offset) in &self.map().scopes.clone() {
            let stack = self.scope_base + scope as usize * 16;
            for word in [0, 8] {
                let (slot, frame) = ((stack + word) as i32, offset as usize + word);
                if save {
                    let value = self.builder.ins().stack_load(types::I64, types::I64, self.storage, slot);
                    self.store(self.body, frame, value);
                } else {
                    let value = self.load(self.body, frame);
                    self.builder.ins().stack_store(types::I64, value, self.storage, slot);
                    self.store(self.body, frame, zero);
                }
            }
        }
    }

    /// Resume dispatch at entry, after the stack check: RESUME 0 starts at pc 0 (which reads continuation 0);
    /// RESUME k reloads k's operands and the scope area and re-invokes the step at k's site.
    pub(super) fn resume_dispatch(&mut self) {
        let resume = self.load(self.body, crate::tasks::RESUME as usize);
        let mut switch = Switch::new();
        switch.set_entry(0, self.blocks[0]);
        let continuations = self.layout().continuations.clone();
        let mut resumes = Vec::new();
        for k in 1..continuations.len() {
            let block = self.builder.create_block();
            self.builder.set_cold_block(block);
            switch.set_entry(k as u128, block);
            resumes.push((k, block));
        }
        let invalid = self.builder.create_block();
        switch.emit(self.builder, resume, invalid);
        for (k, block) in resumes {
            self.builder.switch_to_block(block);
            let site = continuations[k].site.expect("verified continuation site") as usize;
            let s::Instruction::Champ { arguments, .. } = &self.source.code[site] else {
                unreachable!("verified suspension site")
            };
            let operands = arguments.iter().filter_map(|argument| match argument {
                A::Location(location) => Some(*location),
                _ => None,
            }).collect::<Vec<_>>();
            for (&slot, &location) in continuations[k].operands.iter().zip(&operands) {
                self.reload_slot(slot, location);
            }
            self.scope_area(false);
            self.builder.ins().jump(self.blocks[site], &[]);
        }
        self.builder.switch_to_block(invalid);
        self.call("champ_machine_invalid", &[self.ctx]);
        self.builder.ins().jump(self.stop, &[]);
    }

    /// An S-class helper at a suspension site: STOP stops, and PENDING stores the step's operands and the scope
    /// area, writes RESUME = k and returns 1. Post is never read on PENDING.
    fn suspending(&mut self, name: &str, args: &[Value], operands: &[L]) -> Value {
        let ordinal = self.checked(name, args);
        let pending = self.builder.ins().icmp_imm_s(IntCC::Equal, ordinal, c::OUTCOME_PENDING as i64);
        let suspend = self.builder.create_block();
        let resumed = self.builder.create_block();
        self.builder.set_cold_block(suspend);
        self.builder.ins().brif(pending, suspend, &[], resumed, &[]);
        self.builder.switch_to_block(suspend);
        let k = self.site();
        let slots = self.layout().continuations[k].operands.clone();
        for (&slot, &location) in slots.iter().zip(operands) {
            self.save_slot(slot, location);
        }
        self.scope_area(true);
        let continuation = self.iconst(k as u64);
        self.store(self.body, crate::tasks::RESUME as usize, continuation);
        let one = self.iconst(1);
        self.builder.ins().return_(&[one]);
        self.builder.switch_to_block(resumed);
        ordinal
    }

    fn continuation(arguments: &[A]) -> usize {
        arguments.iter().find_map(|argument| match argument {
            A::Continuation(k) => Some(*k as usize),
            _ => None,
        }).expect("verified continuation")
    }

    fn kind(&self, location: L) -> Option<s::ServiceKind> {
        match location {
            L::Service(id) => Some(self.source.services[id as usize]),
            L::Register(_) => None,
        }
    }

    /// Champ draft 3 §14.4: the Future operations, the suspension brackets, the take with its proof, the rehome, the
    /// nonempty `cleanup.settle`, the dynamic settle and service materialize/absorb (ABI D25).
    pub(super) fn future_operation(&mut self, op: O, arguments: &[A], locations: &[L], flow: &s::Flow)
        -> Result<(), Error>
    {
        let ctx = self.ctx;
        let body = self.body;
        let zero = self.iconst(0);
        let post = self.post();
        let (helper, ordinal) = match op {
            O::FrameSave => {
                let k = Self::continuation(arguments);
                let saved = self.layout().continuations[k].saved.clone();
                for (&slot, &location) in saved.iter().zip(&locations[1..]) {
                    self.save_slot(slot, location);
                }
                self.next();
                return Ok(());
            }
            O::FrameResume => {
                let k = Self::continuation(arguments);
                let saved = self.layout().continuations[k].saved.clone();
                let s::Flow::Next(outputs) = flow else { unreachable!("verified frame.resume") };
                for (&slot, &output) in saved.iter().zip(&outputs[1..]) {
                    self.reload_slot(slot, output);
                }
                self.next();
                return Ok(());
            }
            O::FutureDone => {
                // Inline, as Tomato's `fut done`: a success or a failure.
                let future = self.word(locations[0]);
                let state = self.load(future, crate::tasks::STATE as usize);
                let done = self.builder.ins().icmp_imm_s(IntCC::Equal, state, crate::tasks::DONE as i64);
                let failed = self.builder.ins().icmp_imm_s(IntCC::Equal, state, crate::tasks::FAILURE_STATE as i64);
                let settled = self.builder.ins().bor(done, failed);
                let settled = self.builder.ins().uextend(types::I64, settled);
                let s::Flow::Next(outputs) = flow else { unreachable!("verified fut.done") };
                self.write(outputs[0], &[settled]);
                self.next();
                return Ok(());
            }
            O::FutureNew => {
                let entry = arguments.iter().find_map(|argument| match argument {
                    A::Entry(id) => Some(u64::from(*id)),
                    _ => None,
                }).expect("verified entry");
                let args = locations.iter().filter(|location| matches!(location, L::Register(_)))
                    .map(|&location| self.word(location)).collect::<Vec<_>>();
                let argv = self.record(&args);
                let packet = *locations.iter().find(|&&location| self.kind(location) == Some(s::ServiceKind::Packet))
                    .expect("verified inputs");
                let owner = *locations.iter()
                    .find(|&&location| self.kind(location) == Some(s::ServiceKind::ScopeState))
                    .expect("verified owner state");
                let entry = self.iconst(entry);
                let argc = self.iconst(args.len() as u64);
                let packet = self.pointer(packet);
                let owner = self.pointer(owner);
                let inputs = self.record(&[entry, argv, argc, packet, owner]);
                ("champ_future_new", self.checked("champ_future_new", &[ctx, inputs, post]))
            }
            O::Await => {
                let future = self.word(locations[0]);
                ("champ_await_view", self.suspending("champ_await_view", &[ctx, body, future, post], locations))
            }
            O::AwaitTake => {
                // The Future, its zero-word capability and the boundary scope, whose owner takes the packet.
                let future = self.word(locations[0]);
                let scope = arguments.iter().find_map(|argument| match argument {
                    A::Scope(id) => Some(*id),
                    _ => None,
                }).expect("verified take boundary");
                let boundary = self.scope_word(scope, 8);
                let args = [ctx, body, future, boundary, post];
                ("champ_await_take", self.suspending("champ_await_take", &args, locations))
            }
            O::UniqueCacheProof => {
                // A proof the loader checks (SPB_TAKE_CAPABILITY): zero words, no helper.
                self.next();
                return Ok(());
            }
            O::CacheRehome => {
                // The token form (ABI C-4): the exit's target is a scope's owner or the caller.
                let destination = arguments.iter().find_map(|argument| match argument {
                    A::Destination(destination) => Some(*destination),
                    _ => None,
                }).expect("verified rehome target");
                let scope = match destination {
                    s::ExitDestination::Caller => None,
                    s::ExitDestination::Loop(id) => Some(self.program.tables.boundaries[id as usize].scope),
                    s::ExitDestination::Scope(id) => Some(id),
                };
                let target = match scope {
                    Some(scope) => self.scope_word(scope, 8),
                    None => self.iconst(c::TARGET_CALLER),
                };
                let pending = self.pointer(locations[0]);
                let args = if let [_, state, future] = locations {
                    // The state form takes the entry the scope admitted (D25.3).
                    let state = self.pointer(*state);
                    let future = self.word(*future);
                    [ctx, pending, state, zero, zero, future, target, post]
                } else {
                    let cache = self.pointer(locations[1]);
                    let future = self.word(locations[1]);
                    [ctx, pending, zero, cache, zero, future, target, post]
                };
                ("champ_cache_rehome", self.checked("champ_cache_rehome", &args))
            }
            // `invoke.defer` of an async defer entry (ABI C-6, draft 3 §11), class S: the helper runs the entry as a
            // child of this body and waits for it; Continue or Unwind follows. Its operands are the entry's start
            // values (arguments, then captures) and the input packet, which the helper empties and finally consumes.
            O::InvokeDefer => {
                let target = arguments.iter().find_map(|argument| match argument {
                    A::Entry(id) => Some(*id),
                    _ => None,
                }).expect("verified defer entry");
                let program: &s::Program = self.program;
                let callee = &program.functions[target as usize];
                let count = program.frame_layout(callee).and_then(|layout| layout.saved_types(0))
                    .map_or(0, |types| types.iter().filter(|ty| matches!(ty, s::ProducedType::Value(_))).count());
                let values = locations[..count].iter().map(|&location| self.word(location)).collect::<Vec<_>>();
                let argv = self.record(&values);
                let argc = self.iconst(count as u64);
                let packet = self.word(locations[count]);
                let entry = self.iconst(u64::from(target));
                let io = self.record(&[packet, zero, post, zero, zero]);
                let registration = self.record(&[zero]);
                let args = [ctx, body, entry, argv, argc, io, registration, post];
                let ordinal = self.suspending("champ_invoke_defer_async", &args, locations);
                // The helper consumed the input packet on its outcome.
                self.write(locations[count], &[zero]);
                ("champ_invoke_defer_async", ordinal)
            }
            // ABI D25.3: the closing scope's service list, after the static duties. `this` is the body's frame, or
            // zero in a synchronous owner.
            O::CleanupSettleDynamic => {
                let cleanup = self.pointer(locations[0]);
                let args = [ctx, body, cleanup, post];
                ("champ_cleanup_settle_dynamic", self.checked("champ_cleanup_settle_dynamic", &args))
            }
            // ABI D25.2: a cache duty's entry out of a packet as its token, or a duty token into a packet.
            O::ServiceMaterialize => {
                let packet = self.pointer(locations[0]);
                let holder = self.word(locations[1]);
                let kind = self.iconst(c::SERVICE_CACHE);
                let args = [ctx, packet, holder, kind, post];
                ("champ_service_materialize", self.checked("champ_service_materialize", &args))
            }
            O::ServiceAbsorb => {
                let packet = self.pointer(locations[0]);
                let token = self.word(locations[1]);
                let kind = match self.kind(locations[1]) {
                    Some(s::ServiceKind::TaskState) => c::SERVICE_TASK,
                    _ => c::SERVICE_CACHE,
                };
                let kind = self.iconst(kind);
                let args = [ctx, packet, token, kind, post];
                ("champ_service_absorb", self.checked("champ_service_absorb", &args))
            }
            O::AwaitUnwind => {
                let state = self.pointer(locations[0]);
                let failure = self.word(locations[1]);
                let task = self.pointer(locations[2]);
                let receipt = self.pointer(locations[3]);
                let args = [ctx, body, state, failure, task, receipt, post];
                ("champ_await_unwind", self.suspending("champ_await_unwind", &args, locations))
            }
            O::FuturePublish => {
                let success = arguments.contains(&A::Flag(true));
                let register = locations[1..].iter().copied().find(|location| matches!(location, L::Register(_)));
                let word = register.map_or(zero, |location| self.word(location));
                let packet = locations[1..].iter().copied()
                    .find(|&location| self.kind(location) == Some(s::ServiceKind::Packet));
                let packet = match packet {
                    Some(location) => self.pointer(location),
                    None => self.record(&[zero]),
                };
                let tag = self.iconst(u64::from(!success));
                let (value, failure) = if success { (word, zero) } else { (zero, word) };
                let inputs = self.record(&[tag, value, packet, failure]);
                ("champ_future_publish", self.checked("champ_future_publish", &[ctx, body, inputs]))
            }
            O::FutureCancel => {
                let future = self.word(locations[0]);
                ("champ_future_cancel", self.checked("champ_future_cancel", &[ctx, future]))
            }
            O::FutureCancelled => {
                let future = self.word(locations[0]);
                ("champ_future_cancelled", self.checked("champ_future_cancelled", &[ctx, future, post]))
            }
            O::CleanupDeliverCancel => {
                let cleanup = self.pointer(locations[0]);
                let args = [ctx, body, cleanup, post];
                ("champ_cleanup_deliver_cancel", self.checked("champ_cleanup_deliver_cancel", &args))
            }
            O::CleanupSettle => {
                let local = arguments.iter().find_map(|argument| match argument {
                    A::Schedule(id) => Some(*id as usize),
                    _ => None,
                }).expect("verified schedule");
                let duties = &self.source.proofs.schedules[local].services;
                let index = duties.iter().position(|&duty| duty == locations[1]).expect("scheduled duty");
                let cleanup = self.pointer(locations[0]);
                let schedule = self.iconst(self.schedules[local]);
                let index = self.iconst(index as u64);
                let duty = self.word(locations[1]);
                let args = [ctx, body, cleanup, schedule, index, duty, post];
                ("champ_cleanup_settle", self.checked("champ_cleanup_settle", &args))
            }
            _ => unreachable!("not a Future operation"),
        };
        self.flow(op, flow, arguments, ordinal, helper)
    }
}
