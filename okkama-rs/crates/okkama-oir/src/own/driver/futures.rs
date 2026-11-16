//! Commit 5a, plain async (#601 V2): Future creation, shared await, the final
//! cancellation envelope and the cache-duty schedules of one function.
use super::*;
use crate::own::futures::{is_future, outcome, services_protocol, unsupported_async, CACHE_DUTY};

impl Driver<'_, '_> {
    /// `call @f(args)` of an async entry creates a cold Future (elaboration
    /// D12, §7.2). `fut.new` consumes the input packet and the owner state,
    /// keeps references in `Deferred` without duplicating their obligations,
    /// and adds `ServiceOf{future, Cache}` to the owner state. It returns no
    /// cache-state value. The owner is the innermost retained scope of the call.
    pub(super) fn create_future(
        &mut self,
        mut point: Landing,
        instruction: &Instruction,
        function: FuncId,
    ) -> Result<Landing, OwnError> {
        let [original] = instruction.results.as_slice() else {
            return Err(flow_error("async call has exactly one Future result"));
        };
        let result = outcome(self.module, self.module.functions.get(function).map_err(arena)?)?;
        let owner = self.owner_scope(&point)?;
        if self.created_in_owner_loop(owner)? {
            let message = "fut.new inside a loop of its owner: the duties of its Futures are dynamic (D25), \
                they arrive with R1";
            return Err(unsupported_async(message));
        }
        let Operation::Call(CallOp::Direct { call: original_call, .. }) = &instruction.op else {
            return Err(flow_error("Future creation is a direct call of the async entry"));
        };
        let originals: Vec<_> = original_call.args.iter().map(|argument| argument.value).collect();
        let ty = |original: &ValueId| self.high.values.get(*original).map(|value| value.ty);
        let managed: Vec<_> = originals.iter().copied()
            .filter(|original| ty(original).is_ok_and(|ty| self.managed(ty) && !is_future(self.module, ty))).collect();
        // A Future input borrows its cache: its duty never travels through an offer (OIR-2 §10.2,
        // D12), and no selection names it.
        let future_input = originals.iter().copied()
            .find(|original| ty(original).is_ok_and(|ty| is_future(self.module, ty)));
        // OIR-2 §10.1, #587: the creation retains the fresh part of every managed input in the
        // input packet, which the Future's inputs holder keeps until the first start, and the
        // Future depends on their external deps (the creation summary). Plain inputs offer
        // nothing: their empty deps are those of a plain argument, or the external deps of an
        // empty selection when there is no argument. A Future input alone gives its own deps.
        let deps;
        let mut packet;
        if !managed.is_empty() {
            let (next, offered) = self.offered(point, &originals)?;
            point = next;
            let mut parts = vec![];
            for original in &managed {
                let value = point.env.value(*original)?.value;
                parts.push(self.body.selections.alloc(SelectionExpr::Value(value)).map_err(arena)?);
            }
            let selection = self.body.selections.alloc(SelectionExpr::Union(parts)).map_err(arena)?;
            point = self.emit(point, Operation::Ownership(OwnershipOp::Split { packet: offered, selection }))?;
            let parts = vec![point.result(SlotName::Rest)?, point.result(SlotName::Selected)?];
            deps = point.result(SlotName::ExternalDeps)?;
            point = self.emit(point, Operation::Ownership(OwnershipOp::Join { packets: parts }))?;
            packet = point.result(SlotName::Packet)?;
        } else {
            point = self.emit(point, Operation::Ownership(OwnershipOp::Empty {}))?;
            packet = point.result(SlotName::Packet)?;
            deps = if let Some(argument) = future_input.or_else(|| originals.first().copied()) {
                let value = point.env.value(argument)?.value;
                point = self.emit(point, Operation::Ownership(OwnershipOp::Dependencies { value }))?;
                point.result(SlotName::Value)?
            } else {
                let selection = self.body.selections.alloc(SelectionExpr::Union(vec![])).map_err(arena)?;
                point = self.emit(point, Operation::Ownership(OwnershipOp::Split { packet, selection }))?;
                let parts = vec![point.result(SlotName::Rest)?, point.result(SlotName::Selected)?];
                let deps = point.result(SlotName::ExternalDeps)?;
                point = self.emit(point, Operation::Ownership(OwnershipOp::Join { packets: parts }))?;
                packet = point.result(SlotName::Packet)?;
                deps
            };
        }
        // The arguments as the offer left them, as for an owning call.
        let mut op = instruction.op.clone();
        let mut missing = false;
        op.rewrite(&mut Rename { env: &point.env, missing: &mut missing });
        let Operation::Call(CallOp::Direct { call, .. }) = op else {
            return Err(flow_error("Future creation is a direct call of the async entry"));
        };
        if missing {
            return Err(flow_error("Future creation operand has no reaching SSA definition"));
        }
        let environment = FutureEnvironment::Deferred {
            function,
            arguments: call.args.iter().map(|argument| argument.value).collect(),
            descriptors: call.descriptors.clone(),
        };
        let owner_state = point.env.scopes[&owner];
        point = self.emit(point, Operation::Async(AsyncOp::FutureNew {
            packet, deps, owner_state, environment, result, owner,
        }))?;
        let future = point.result(SlotName::Value)?;
        // A temporary Future has no use but its take: naming its deps would be another use of
        // the cache that the unique-cache evidence excludes (§10.3).
        let deps = if self.temporaries.contains(original) {
            None
        } else {
            point = self.emit(point, Operation::Ownership(OwnershipOp::Dependencies { value: future }))?;
            Some(point.result(SlotName::Value)?)
        };
        point.env.values.insert(*original, LatestValue { value: future, deps });
        self.cache_duties.push((owner, future));
        Ok(point)
    }

    /// D24.1 (slice P1): High's `fut.spawn %f` of the Future its creation just gave. Own forwards the state of the
    /// scope that owns the Future's cache duty, which gains the spawned task's duty `ServiceOf{f, Frame}`; both
    /// duties settle at that scope's close, the task's first (D24.2), or leave together through `cache.rehome`.
    pub(super) fn spawn_future(&mut self, point: Landing, instruction: &Instruction) -> Result<Landing, OwnError> {
        let Operation::Async(AsyncOp::FutureSpawn { future: original, state: None }) = instruction.op else {
            return Err(flow_error("expected High's fut.spawn"));
        };
        let future = point.env.value(original)?.value;
        let owner = self.cache_duties.iter().rev().find(|(_, created)| *created == future).map(|(owner, _)| *owner)
            .ok_or_else(|| unsupported_async("fut.spawn of a Future this function did not just create (D24.1)"))?;
        let state = point.env.state(owner)?;
        self.emit(point, Operation::Async(AsyncOp::FutureSpawn { future, state: Some(state) }))
    }

    /// A shared await (elaboration §13.2): `(value?, deps)` with deps the stable
    /// cache service key and the external outcome deps, and no packet. Its
    /// unwind and cancel edge flows into `await.unwind` (§13.3), which consumes
    /// the failure, the task and the receipt with the owner state before any
    /// other use, so no failure block can drop the task or the receipt.
    pub(super) fn await_future(&mut self, mut point: Landing, instruction: &Instruction) -> Result<Landing, OwnError> {
        let Operation::Async(AsyncOp::Await { future: original, result }) = instruction.op else {
            return Err(flow_error("expected a shared await"));
        };
        let protocol = services_protocol(self.module)?;
        let future = point.env.value(original)?.value;
        if self.temporaries.contains(&original) {
            return self.take_future(point, instruction, future, protocol);
        }
        point.env.unwind_values = Some(self.unwind_needed(&point));
        let emission = self.emitter.emit(self.module, &mut self.body, point.block, point.env,
            Operation::Async(AsyncOp::Await { future, result }))?;
        let failed = emission.unwind.ok_or_else(|| flow_error("await has no unwind outcome"))?;
        let failed = self.await_unwind(failed, protocol)?;
        let target = self.unwind_target(&failed.env);
        self.record_failure(failed, target)?;
        let mut normal = emission.normal;
        normal.env.cleanup.primary_before_failure = None;
        if let (Some(original), Some(ty)) = (instruction.results.first(), result) {
            let value = normal.result(SlotName::Value)?;
            // A managed outcome (a role value of a plain object) carries the await's full deps: the cache
            // service key and the outcome's external deps (`PreserveFullDependencies`).
            let deps = if self.managed(ty) { Some(normal.result(SlotName::Deps)?) } else { None };
            normal.env.values.insert(*original, LatestValue { value, deps });
        }
        Ok(normal)
    }

    /// OIR-2 §10.2, §10.3: the awaiting scope takes the outcome of a temporary
    /// Future. The unique-cache proof carries the full evidence: the creation
    /// is this function's `fut.new`, the Future has no alias, frame slot,
    /// storage, other waiter, view or collector retention, and the region is
    /// the block of the take. `await.take` extracts the actual outcome packet,
    /// which the awaiting scope admits as a temporary await result; a borrowed
    /// result stays borrowed (the packet is empty).
    fn take_future(&mut self, mut point: Landing, instruction: &Instruction, future: ValueId, protocol: ProtocolId)
        -> Result<Landing, OwnError> {
        let Operation::Async(AsyncOp::Await { future: original, result }) = instruction.op else {
            return Err(flow_error("expected an await"));
        };
        let Definition::InstResult { inst: creation, .. } = self.body.values.get(future).map_err(arena)?.def else {
            return Err(flow_error("a temporary Future is the result of its fut.new"));
        };
        let scope = self.owner_scope(&point)?;
        let evidence = UniqueCacheEvidence {
            creation: CreationOrigin::Instruction(creation),
            aliases: vec![],
            frame_slots: vec![],
            no_external_storage: true,
            no_other_waiters: true,
            no_live_views: true,
            no_collector_retention: true,
            scheduler_cannot_publish: true,
        };
        let proof = UniqueCacheProof { future, scope, region: vec![point.block], evidence };
        point = self.emit(point, Operation::Async(AsyncOp::UniqueCacheProof { proof }))?;
        let unique = point.result(SlotName::Unique)?;
        let boundary = TakeBoundary::Temporary { awaiting: scope };
        // The take is the Future's last use: no edge carries an alias of the taken cache.
        point.env.values.remove(&original);
        point.env.unwind_values = Some(self.unwind_needed(&point));
        let emission = self.emitter.emit(self.module, &mut self.body, point.block, point.env,
            Operation::Async(AsyncOp::AwaitTake { future, result, unique, boundary }))?;
        let failed = emission.unwind.ok_or_else(|| flow_error("await.take has no unwind outcome"))?;
        let failed = self.await_unwind(failed, protocol)?;
        let target = self.unwind_target(&failed.env);
        self.record_failure(failed, target)?;
        let mut normal = emission.normal;
        normal.env.cleanup.primary_before_failure = None;
        let packet = normal.result(SlotName::Packet)?;
        let deps = normal.result(SlotName::Deps)?;
        let value = result.map(|_| normal.result(SlotName::Value)).transpose()?;
        let state = normal.env.state(scope)?;
        normal = self.emit(normal, Operation::Ownership(OwnershipOp::Admit { state, packet }))?;
        if let (Some(original), Some(value)) = (instruction.results.first(), value) {
            normal.env.values.insert(*original, LatestValue { value, deps: Some(deps) });
        }
        Ok(normal)
    }

    /// The High Futures taken by their awaiting scope (OIR-2 §10.2): created by a call of
    /// an async entry and marked `temp`, with a managed outcome awaited once in the same
    /// block, and no other use. A plain outcome has nothing to take; it stays shared.
    pub(super) fn temporary_futures(&self) -> BTreeSet<ValueId> {
        // Every use of a value: operands, edge arguments, schedules and facts.
        let mut uses: BTreeMap<ValueId, usize> = BTreeMap::new();
        let registry = ContractRegistry::new(self.module);
        let walked = cfg::walk_body(self.high, registry, |event| {
            if let cfg::CfgEvent::Use { operand, .. } = event {
                *uses.entry(operand.value).or_default() += 1;
            }
        });
        if walked.is_err() {
            return BTreeSet::new();
        }
        let mut temporaries = BTreeSet::new();
        for (_, block) in self.high.blocks.iter() {
            let ops: Vec<_> = block.insts.iter().filter_map(|inst| self.high.insts.get(*inst).ok()).collect();
            for (index, instruction) in ops.iter().enumerate() {
                let Operation::Call(CallOp::Direct { function, .. }) = &instruction.op else { continue };
                let Some(&future) = instruction.results.first() else { continue };
                if !crate::own::futures::is_async(self.module, *function) || uses.get(&future) != Some(&2) {
                    continue;
                }
                let later = &ops[index + 1..];
                let temp = later.iter().any(|next| matches!(next.op,
                    Operation::Lexical(LexicalOp::Temp { value, .. }) if value == future));
                let taken = later.iter().any(|next| matches!(next.op,
                    Operation::Async(AsyncOp::Await { future: awaited, result: Some(ty) })
                        if awaited == future && self.managed(ty)));
                if temp && taken {
                    temporaries.insert(future);
                }
            }
        }
        temporaries
    }

    /// `@oir.await.unwind` on an await's exceptional landing (async ABI C-3).
    /// The adapter's failure continues the unwind; it is not a cleanup secondary.
    fn await_unwind(&mut self, landing: Landing, protocol: ProtocolId) -> Result<Landing, OwnError> {
        let failure = landing.result(SlotName::Failure)?;
        let task = landing.result(SlotName::Task)?;
        let receipt = landing.result(SlotName::Receipt)?;
        let state = landing.env.scopes[&self.owner_scope(&landing)?];
        let primary = landing.env.cleanup.primary_before_failure;
        let mut next = self.emitter.emit(self.module, &mut self.body, landing.block, landing.env,
            Operation::Async(AsyncOp::AwaitUnwind { state, failure, task, receipt, protocol }))?.normal;
        next.env.cleanup.primary_before_failure = primary;
        Ok(next)
    }

    /// Whether the current function is an async body with the explicit entry ABI:
    /// its successful exits publish through an exit transaction (async ABI C-2).
    pub(super) fn async_exit(&self, point: &Landing) -> Result<bool, OwnError> {
        let lexical = self.body.blocks.get(point.block).map_err(arena)?.lexical_scope;
        let root = stack(self.module, lexical)?[0];
        let function = self.module.scopes.get(root).map_err(arena)?.function;
        Ok(crate::own::futures::is_async(self.module, function) && !fixed_root(self.module, function))
    }

    /// D25 (OIR-2 B10.6): one `fut.new` site names one static duty of its owner.
    /// A site that a loop inside the owner reaches again finds the owner still
    /// holding the duty of the earlier iteration, so it needs dynamic duty
    /// entries. A loop that opens the owner on every iteration leaves the owner
    /// on its way back, and its site keeps one static duty.
    fn created_in_owner_loop(&self, owner: ScopeId) -> Result<bool, OwnError> {
        let registry = ContractRegistry::new(self.module);
        let mut seen = BTreeSet::new();
        let mut work = vec![self.current_high];
        while let Some(block) = work.pop() {
            let successors = crate::cfg::successors(block, self.high, registry)
                .map_err(|error| flow_error(format!("Future creation site has no CFG: {error:?}")))?;
            for next in successors {
                if next == self.current_high {
                    return Ok(true);
                }
                let lexical = self.high.blocks.get(next).map_err(arena)?.lexical_scope;
                if stack(self.module, lexical)?.contains(&owner) && seen.insert(next) {
                    work.push(next);
                }
            }
        }
        Ok(false)
    }

    /// The innermost retained scope at a program point: the owner of a created
    /// Future's cache duty and the state an await unwind updates.
    fn owner_scope(&self, point: &Landing) -> Result<ScopeId, OwnError> {
        let lexical = self.body.blocks.get(point.block).map_err(arena)?.lexical_scope;
        stack(self.module, lexical)?.into_iter().rev().find(|scope| point.env.scopes.contains_key(scope))
            .ok_or_else(|| flow_error("async operation has no retained owner scope"))
    }

    /// Close every open scope at a successful return. The root close of an
    /// async action delivers deferred cancellation; nested closes never do (D18).
    pub(super) fn close_final(&mut self, mut point: Landing) -> Result<Landing, OwnError> {
        let lexical = self.body.blocks.get(point.block).map_err(arena)?.lexical_scope;
        let scopes = stack(self.module, lexical)?;
        let function = self.module.scopes.get(scopes[0]).map_err(arena)?.function;
        let asynchronous = crate::own::futures::is_async(self.module, function);
        for scope in scopes.into_iter().rev() {
            if point.env.scopes.contains_key(&scope) {
                let root = self.module.scopes.get(scope).map_err(arena)?.parent.is_none();
                point = self.close_scope(point, scope, false, asynchronous && root)?;
            }
        }
        Ok(point)
    }

    /// OIR-2 §10.2, async ABI C-4: a Future, or a view of its cached outcome,
    /// leaving the scope that owns its cache duty takes the duty along. Before
    /// the scope's close, `cache.rehome` moves the duty (and the packets its
    /// cache holds) from the scope state into the pending reserve of the exit,
    /// targeted at the exit's destination; `exit.commit` hands it over there.
    /// The escaping values are the reserved result or the carried edge values;
    /// a duty escapes with them when their deps reach its analysis atom.
    pub(super) fn rehome_caches(&mut self, mut point: Landing, scope: ScopeId) -> Result<Landing, OwnError> {
        let escaping: Vec<_> = point.env.cleanup.wanted.iter().chain(&point.env.cleanup.carried).copied().collect();
        let Some(state) = self.flow.before_edges.get(&self.current_high) else {
            return Ok(point);
        };
        let atoms: BTreeSet<_> = escaping.iter()
            .flat_map(|value| state.values[*value].deps.iter().copied()).collect();
        let duties: BTreeSet<_> = state.journals.iter().filter(|((owner, atom), quantity)| {
            *owner == scope && atoms.contains(atom)
                && !matches!(quantity.presence, domain::Presence::Absent | domain::Presence::Bottom)
                && state.provenance.get(&(*owner, *atom)).is_some_and(|mask| mask & CACHE_DUTY != 0)
        }).map(|((_, atom), _)| atom.origin).collect();
        if duties.is_empty() {
            return Ok(point);
        }
        let target = self.exit_landing.map_or(RehomeTarget::Caller, RehomeTarget::Scope);
        let protocol = services_protocol(self.module)?;
        for future in self.high_futures() {
            if !self.flow.value_shapes.get(&future).is_some_and(|shape| duties.contains(shape)) {
                continue;
            }
            let value = match point.env.values.get(&future) {
                Some(latest) => latest.value,
                None => *self.definitions.get(&future)
                    .ok_or_else(|| flow_error("an escaping cache duty's Future has no SSA definition"))?,
            };
            let state = point.env.state(scope)?;
            let pending = point.env.cleanup.pending.ok_or_else(|| flow_error("cache.rehome has no reserve"))?;
            let form = RehomeForm::Scope { state, pending, future: value };
            point = self.emit(point, Operation::Async(AsyncOp::CacheRehome { form, target, protocol }))?;
            point.env.cleanup.rehomed = true;
        }
        Ok(point)
    }

    /// The High Futures whose cache duty a call gives the calling scope, in body order: Future creations
    /// (calls of async entries, D12) and Futures returned with their duty in the result packet (D25.1).
    fn high_futures(&self) -> Vec<ValueId> {
        let mut futures = vec![];
        for (_, instruction) in self.high.insts.iter() {
            if !matches!(instruction.op, Operation::Call(_)) {
                continue;
            }
            for result in &instruction.results {
                if self.high.values.get(*result).is_ok_and(|value| is_future(self.module, value.ty)) {
                    futures.push(*result);
                }
            }
        }
        futures
    }

    /// D16 schedules for cache duties: each scope that owns duties gets one
    /// schedule covering each duty once. A High schedule of that scope is the
    /// admissible order input: its duties name the Futures and must agree with
    /// the duties own assigns. Without one, own declares the schedule in
    /// creation order. A scope that admits service entries at run time ends
    /// its schedule with the one dynamic step (D25.3).
    pub(super) fn cache_schedules(&mut self) -> Result<(), OwnError> {
        for schedule in &mut self.body.service_schedules {
            for duty in &mut schedule.duties {
                if let ScheduleDuty::Value(value) = duty {
                    *value = self.definitions.get(value).copied()
                        .ok_or_else(|| flow_error("service schedule duty has no reaching definition"))?;
                }
            }
        }
        let mut owners: Vec<ScopeId> = vec![];
        for owner in self.cache_duties.iter().map(|(owner, _)| owner).chain(&self.dynamic_owners) {
            if !owners.contains(owner) {
                owners.push(*owner);
            }
        }
        for owner in owners {
            let mut duties: Vec<_> = self.cache_duties.iter().filter(|(scope, _)| *scope == owner)
                .map(|(_, future)| ScheduleDuty::Value(*future)).collect();
            let dynamic = self.dynamic_owners.contains(&owner);
            if let Some(schedule) = self.body.service_schedules.iter_mut().find(|schedule| schedule.owner == owner) {
                let declared: Vec<_> = schedule.duties.iter().copied().filter(|duty| *duty != ScheduleDuty::Dynamic)
                    .collect();
                if declared.is_empty() {
                    schedule.duties = duties;
                } else {
                    let unique: BTreeSet<_> = declared.iter().copied().collect();
                    if unique.len() != declared.len() || unique != duties.iter().copied().collect() {
                        return Err(phase_error(OwnDiagnosticKind::InvalidEquation, format!(
                            "service schedule {} differs from the cache duties of its scope", schedule.name,
                        )));
                    }
                    schedule.duties = declared;
                }
                if dynamic {
                    schedule.duties.push(ScheduleDuty::Dynamic);
                }
                continue;
            }
            if dynamic {
                duties.push(ScheduleDuty::Dynamic);
            }
            let protocol = services_protocol(self.module)?;
            let mut index = 0usize;
            let name = loop {
                let name = format!("Schedule{index}");
                if self.body.service_schedules.iter().all(|schedule| schedule.name != name) {
                    break name;
                }
                index += 1;
            };
            self.body.service_schedules.push(ServiceSchedule { dictionary: None, duties, name, owner, protocol });
        }
        Ok(())
    }
}
