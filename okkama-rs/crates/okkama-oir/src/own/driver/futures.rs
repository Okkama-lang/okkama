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
        let (Operation::Call(CallOp::Direct { call: original_call, .. })
            | Operation::Call(CallOp::Intrinsic { call: original_call, .. })) = &instruction.op else {
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
        // input packet, which the Future's inputs holder keeps until the first start. Its deps
        // operand is `deps.of` each managed input and a bare Future input, joined (#862, Fable,
        // §6.3c): `create_future` never restructures a packet to compute a static fact; the
        // verifier subtracts what the inputs packet owns. Plain inputs offer nothing: their empty
        // deps are those of a plain argument, or `deps.join []` when there is no argument.
        let deps;
        let packet;
        let thunk = self.forward_entry;
        if std::mem::take(&mut self.forward_entry) {
            // Ruling PR-N2 (#860): a creation thunk's `fut.new` takes the thunk's entry packet whole, and depends on
            // the deps of its managed inputs, which the prologue gave its parameters.
            let own = self.module.types.intern(Type::Service(ServiceType::Own)).map_err(arena)?;
            let packets: Vec<_> = point.env.linear.iter().copied()
                .filter(|value| self.body.values.get(*value).is_ok_and(|data| data.ty == own)).collect();
            let [entry] = packets[..] else {
                return Err(flow_error("a creation thunk's fut.new takes its one entry packet (PR-N2)"));
            };
            packet = entry;
            // #863, #864 (PR-N2, OIR-2-async-remaining §6.3b-c): the Future depends on exactly the inputs its creation
            // summary calls managed, which the prologue gave their deps, and on a bare Future input; several are joined
            // and none is the empty `deps.join []`. A thunk never restructures its unaccepted entry packet.
            // The creation summary's managed set is the `may_own` test (`creation_summary`); own's `managed` also
            // counts managed references, which the creation summary does not declare.
            let owning = |original: &&ValueId| ty(original).is_ok_and(|ty| {
                self.analysis.types.get(&ty).is_none_or(TypeClassification::may_own)
            });
            let mut parts = managed.iter().filter(owning).map(|input| point.env.value(*input)?.deps
                .ok_or_else(|| flow_error("a managed thunk input has its deps"))).collect::<Result<Vec<_>, _>>()?;
            if let Some(future) = future_input {
                let value = point.env.value(future)?.value;
                point = self.emit(point, Operation::Ownership(OwnershipOp::Dependencies { value }))?;
                parts.push(point.result(SlotName::Value)?);
            }
            deps = if let [one] = parts[..] {
                one
            } else {
                point = self.emit(point, Operation::Ownership(OwnershipOp::DepsJoin { deps: parts }))?;
                point.result(SlotName::Value)?
            };
        } else if !managed.is_empty() {
            // #862 (OIR-2-async-remaining §6.3c): a bound input (a parameter or a `let`) is never moved into another
            // holder. The Future borrows it and depends on `deps.of` it; it leaves beside the Future through the
            // exit (`extend`). Only the fresh managed inputs are offered, and the offered packet is the Future's
            // inputs packet as it is (no `own.split`: the loader cannot inventory a packet beside a duty that came
            // back from a call). A Future input's cache is borrowed too.
            let bound_values = self.bound_values();
            let (bound, fresh): (Vec<_>, Vec<_>) = managed.iter().copied()
                .partition(|original| bound_values.contains(original));
            let mut parts = vec![];
            if fresh.is_empty() {
                point = self.emit(point, Operation::Ownership(OwnershipOp::Empty {}))?;
                packet = point.result(SlotName::Packet)?;
            } else {
                let (next, offered) = self.offered(point, &fresh)?;
                point = next;
                packet = offered;
            }
            // #864: every managed input, fresh or borrowed, and a Future input add their own deps.
            for original in fresh.iter().chain(bound.iter()).chain(future_input.iter()) {
                let value = point.env.value(*original)?.value;
                point = self.emit(point, Operation::Ownership(OwnershipOp::Dependencies { value }))?;
                parts.push(point.result(SlotName::Value)?);
            }
            deps = if let [one] = parts[..] {
                one
            } else {
                point = self.emit(point, Operation::Ownership(OwnershipOp::DepsJoin { deps: parts }))?;
                point.result(SlotName::Value)?
            };
            if !bound.is_empty() {
                let values = bound.iter().map(|input| point.env.value(*input).map(|latest| latest.value))
                    .collect::<Result<_, _>>()?;
                self.future_bound_inputs.insert(*original, values);
            }
        } else {
            point = self.emit(point, Operation::Ownership(OwnershipOp::Empty {}))?;
            packet = point.result(SlotName::Packet)?;
            deps = if let Some(argument) = future_input.or_else(|| originals.first().copied()) {
                let value = point.env.value(argument)?.value;
                point = self.emit(point, Operation::Ownership(OwnershipOp::Dependencies { value }))?;
                point.result(SlotName::Value)?
            } else {
                point = self.emit(point, Operation::Ownership(OwnershipOp::DepsJoin { deps: vec![] }))?;
                point.result(SlotName::Value)?
            };
        }
        // #866 (Fable, §6.3c for a Future): a bare Future parameter the creation borrows goes back beside the
        // Future it returns, as a bound resource parameter does: the exit names it (its entry, when the caller
        // offered one).
        // A creation thunk forwards its entry into the Future instead (PR-N2).
        if !thunk && let Some(future) = future_input
            && self.high.blocks.get(self.high.entry).is_ok_and(|entry| entry.params.contains(&future)) {
            let value = point.env.value(future)?.value;
            self.future_bound_inputs.entry(*original).or_default().push(value);
        }
        // The arguments as the offer left them, as for an owning call.
        let mut op = instruction.op.clone();
        let mut missing = false;
        op.rewrite(&mut Rename { env: &point.env, missing: &mut missing });
        let (Operation::Call(CallOp::Direct { call, .. }) | Operation::Call(CallOp::Intrinsic { call, .. })) = op else {
            return Err(flow_error("Future creation is a direct call of the async entry"));
        };
        if missing {
            return Err(flow_error("Future creation operand has no reaching SSA definition"));
        }
        let arguments = call.args.iter().map(|argument| argument.value).collect();
        let descriptors = call.descriptors.clone();
        // D22 (#601 Q1, N1): an async intrinsic's or native's Future is run by its runtime driver; own treats the
        // environment as `deferred`, and coro keeps it unchanged.
        let environment = if crate::own::futures::is_async_external(self.module, function) {
            FutureEnvironment::External { function, arguments, descriptors }
        } else {
            FutureEnvironment::Deferred { function, arguments, descriptors }
        };
        let owner_state = point.env.scopes[&owner];
        // PR-B1 (#784): a Future that High binds in its owner scope is bound from its creation.
        // An alias binding in a nested scope (`if c { let g = f; }`) does not displace it.
        let binding = self.future_bindings.get(original).into_iter().flatten().copied()
            .find(|binding| self.high.bindings.get(*binding).is_ok_and(|data| data.scope == owner));
        point = self.emit(point, Operation::Async(AsyncOp::FutureNew {
            packet, deps, owner_state, environment, result, owner, binding, slot: None,
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
    /// Any other Future (a binding, a role slot's or an action value's result, a synchronous call's: #766) gets
    /// `fut.spawn.bound %f` (ruling PR-E): it starts the Future if it is cold and registers it, and changes no duty.
    pub(super) fn spawn_future(&mut self, point: Landing, instruction: &Instruction) -> Result<Landing, OwnError> {
        let Operation::Async(AsyncOp::FutureSpawn { future: original, state: None, task_slot: None }) = instruction.op
        else {
            return Err(flow_error("expected High's fut.spawn"));
        };
        let future = point.env.value(original)?.value;
        let owner = self.cache_duties.iter().rev().find(|(_, created)| *created == future).map(|(owner, _)| *owner);
        let (Some(owner), true) = (owner, self.created_just_before(original)?) else {
            return self.emit(point, Operation::Async(AsyncOp::FutureSpawnBound { future }));
        };
        let state = point.env.state(owner)?;
        self.emit(point, Operation::Async(AsyncOp::FutureSpawn { future, state: Some(state), task_slot: None }))
    }

    /// Whether the High instruction just before the current one, in the same block, is the async call that created
    /// `original` (D24.1's freshness, ruling PR-E).
    fn created_just_before(&self, original: ValueId) -> Result<bool, OwnError> {
        let Some(current) = self.current_instruction else { return Ok(false) };
        let insts = &self.high.blocks.get(self.current_high).map_err(arena)?.insts;
        let Some(position) = insts.iter().position(|&inst| inst == current) else { return Ok(false) };
        let Some(&previous) = position.checked_sub(1).and_then(|index| insts.get(index)) else { return Ok(false) };
        let previous = self.high.insts.get(previous).map_err(arena)?;
        let creates = matches!(previous.op, Operation::Call(CallOp::Direct { function, .. }
            | CallOp::Intrinsic { function, .. }) if super::super::futures::is_async(self.module, function));
        Ok(creates && previous.results.first() == Some(&original))
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

    /// PR-B1 (#784): whether the High value is the Future an async call of this function creates (its `fut.new`).
    pub(super) fn created_by_call(&self, original: ValueId) -> bool {
        let Ok(Definition::InstResult { inst, .. }) = self.high.values.get(original).map(|value| value.def) else {
            return false;
        };
        self.high.insts.get(inst).is_ok_and(|instruction| matches!(instruction.op,
            Operation::Call(CallOp::Direct { function, .. } | CallOp::Intrinsic { function, .. })
                if crate::own::futures::is_async(self.module, function)))
    }

    /// PR-B1 (#784): an admitted Future, whose duty is an entry of its scope's admission journal at run time: the
    /// result of a synchronous call (D25.1) or a value a nested scope's exit rehomed (a block parameter after the
    /// exit). Neither a Future created here nor a parameter of this function is one.
    /// #870: whether the High value is the outcome of a shared await, a view of the outcome its Future's cache keeps
    /// (OIR-2 §10.2). An await of a temporary takes the outcome instead (`take_future`).
    pub(super) fn outcome_view(&self, original: ValueId) -> bool {
        let Ok(Definition::InstResult { inst, .. }) = self.high.values.get(original).map(|value| value.def) else {
            return false;
        };
        matches!(self.high.insts.get(inst).map(|instruction| &instruction.op),
            Ok(Operation::Async(AsyncOp::Await { future, .. })) if !self.temporaries.contains(future))
    }

    pub(super) fn admitted_future(&self, original: ValueId) -> bool {
        match self.high.values.get(original).map(|value| value.def) {
            Ok(Definition::InstResult { .. }) => !self.created_by_call(original),
            Ok(Definition::BlockParam { block, .. }) => block != self.high.entry,
            Err(_) => false,
        }
    }

    /// PR-B1 (#784): High Future → the bindings of its `bind`s and `rebind`s, for every Future
    /// value this function binds: one an async call creates here, or one admitted with a packet. An alias
    /// (`let g = f`) binds the same value again, so every binding is kept, not the last.
    /// #862 (§6.3c, ruling (iii)): the High values a binding names, a `let` or a parameter (the entry block's
    /// parameters, bound or not); a binding is never moved into another holder.
    fn bound_values(&self) -> BTreeSet<ValueId> {
        let parameters = self.high.blocks.get(self.high.entry).map(|block| block.params.clone()).unwrap_or_default();
        self.high.insts.iter().filter_map(|(_, instruction)| match &instruction.op {
            Operation::Lexical(LexicalOp::Bind { value, .. } | LexicalOp::Rebind { value, .. }) => Some(*value),
            _ => None,
        }).chain(parameters).collect()
    }

    pub(super) fn bound_futures(&self) -> BTreeMap<ValueId, Vec<BindingId>> {
        let mut bindings: BTreeMap<ValueId, Vec<BindingId>> = BTreeMap::new();
        for (_, instruction) in self.high.insts.iter() {
            if let Operation::Lexical(LexicalOp::Bind { binding, value } | LexicalOp::Rebind { binding, value, .. })
                = &instruction.op
                && self.high.values.get(*value).is_ok_and(|data| is_future(self.module, data.ty))
            {
                let held = bindings.entry(*value).or_default();
                if !held.contains(binding) {
                    held.push(*binding);
                }
            }
        }
        bindings
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
            // #871 (Fable, fail-closed): a Future a call returned with something beside it (#865) cannot leave this
            // scope yet. What came beside it would be destroyed at this scope's close while the Future still reads it.
            if self.futures_with_beside.get(&future).is_some_and(|owners| owners.contains(&scope)) {
                return Err(unsupported_async("a Future that a call returned with a resource beside it cannot leave \
                    the scope that admitted them yet (#871)"));
            }
            // #867 (Fable): a temporary Future has no use but its take (§10.3). The take moved its outcome out of the
            // cache into the awaiting scope, so an escaping outcome is no view of the cache and needs no rehome; its
            // unique-cache proof says the cache is never published (B92).
            if self.temporaries.contains(&future) {
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

    /// PR-B2 (#782): an escaping Future that reaches no cache duty of the closing scope, so `rehome_caches` takes
    /// nothing for it: a parameter, an element or a field read, borrowed from a holder. Its entry can still be in the
    /// scope when a carrier entry packet brought it, so the exit's selection names it.
    ///
    /// #801: a borrowed alias that a synchronous call returned (a getter of a Future field, a relay of one) is
    /// borrowed even though it depends on the duty of the Future its holder stores: that duty is the holder's,
    /// and the exit that names the alias takes its entry wherever the scope keeps it.
    pub(super) fn borrowed_future(&self, scope: ScopeId, original: ValueId) -> bool {
        if self.borrowed_alias(original) {
            return true;
        }
        let Some(state) = self.flow.before_edges.get(&self.current_high) else {
            return false;
        };
        let Some(value) = state.values.get(original) else {
            return false;
        };
        !state.journals.iter().any(|((owner, atom), quantity)| {
            *owner == scope && value.deps.contains(atom)
                && !matches!(quantity.presence, domain::Presence::Absent | domain::Presence::Bottom)
                && state.provenance.get(&(*owner, *atom)).is_some_and(|mask| mask & CACHE_DUTY != 0)
        })
    }

    /// #801: whether the High value is a result of a synchronous call whose relation makes it a borrowed alias
    /// (`futures::borrowed_result`).
    fn borrowed_alias(&self, original: ValueId) -> bool {
        let Ok(Definition::InstResult { inst, .. }) = self.high.values.get(original).map(|value| value.def) else {
            return false;
        };
        let Ok(instruction) = self.high.insts.get(inst) else {
            return false;
        };
        let Operation::Call(call) = &instruction.op else {
            return false;
        };
        instruction.results.iter().position(|result| *result == original)
            .is_some_and(|index| crate::own::futures::borrowed_result(self.module, call, index))
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
