//! Commit 5a, plain async (#601 V2): Future creation, shared await, the final
//! cancellation envelope and the cache-duty schedules of one function.
use super::*;
use crate::own::futures::{is_future, outcome, services_protocol, unsupported_async};

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
        let mut op = instruction.op.clone();
        let mut missing = false;
        op.rewrite(&mut Rename { env: &point.env, missing: &mut missing });
        let Operation::Call(CallOp::Direct { call, .. }) = op else {
            return Err(flow_error("Future creation is a direct call of the async entry"));
        };
        if missing {
            return Err(flow_error("Future creation operand has no reaching SSA definition"));
        }
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
        // Plain async (V2): every language argument is plain, so the offered input
        // packet is empty and the Future has no external deps. R1 brings fresh cold
        // inputs. The empty deps are those of a plain argument, or the external deps
        // of an empty selection when there is no argument.
        point = self.emit(point, Operation::Ownership(OwnershipOp::Empty {}))?;
        let mut packet = point.result(SlotName::Packet)?;
        let deps = if let Some(argument) = call.args.first() {
            point = self.emit(point, Operation::Ownership(OwnershipOp::Dependencies { value: argument.value }))?;
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
        point = self.emit(point, Operation::Ownership(OwnershipOp::Dependencies { value: future }))?;
        let deps = point.result(SlotName::Value)?;
        point.env.values.insert(*original, LatestValue { value: future, deps: Some(deps) });
        self.cache_duties.push((owner, future));
        Ok(point)
    }

    /// A shared await (elaboration §13.2): `(value?, deps)` with deps the stable
    /// cache service key and the external outcome deps, and no packet. Its
    /// unwind and cancel edge flows into `await.unwind` (§13.3), which consumes
    /// the failure, the task and the receipt with the owner state before any
    /// other use, so no failure block can drop the task or the receipt.
    pub(super) fn await_future(&mut self, mut point: Landing, instruction: &Instruction) -> Result<Landing, OwnError> {
        let Operation::Async(AsyncOp::Await { future, result }) = instruction.op else {
            return Err(flow_error("expected a shared await"));
        };
        if let Some(ty) = result && self.analysis.types.get(&ty).is_none_or(TypeClassification::may_own) {
            return Err(unsupported_async("resource-bearing await outcome: cache-owned results arrive with R1"));
        }
        let protocol = services_protocol(self.module)?;
        let future = point.env.value(future)?.value;
        point.env.unwind_values = Some(self.unwind_needed(&point));
        let emission = self.emitter.emit(self.module, &mut self.body, point.block, point.env,
            Operation::Async(AsyncOp::Await { future, result }))?;
        let failed = emission.unwind.ok_or_else(|| flow_error("await has no unwind outcome"))?;
        let failed = self.await_unwind(failed, protocol)?;
        let target = self.unwind_target(&failed.env);
        self.record_failure(failed, target)?;
        let mut normal = emission.normal;
        normal.env.cleanup.primary_before_failure = None;
        if let (Some(original), Some(_)) = (instruction.results.first(), result) {
            let value = normal.result(SlotName::Value)?;
            normal.env.values.insert(*original, LatestValue { value, deps: None });
        }
        Ok(normal)
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

    /// A Future leaving its cache owner needs `cache.rehome` in the exit
    /// protocol (OIR-2 §10.2, C-4), which arrives with R1.
    pub(super) fn contained_futures(&self, values: &[ValueId]) -> Result<(), OwnError> {
        for value in values {
            if is_future(self.module, self.high.values.get(*value).map_err(arena)?.ty) {
                return Err(unsupported_async("a Future escapes its cache owner scope: cache.rehome arrives with R1"));
            }
        }
        Ok(())
    }

    /// D16 schedules for cache duties: each scope that owns duties gets one
    /// schedule covering each duty once. A High schedule of that scope is the
    /// admissible order input: its duties name the Futures and must agree with
    /// the duties own assigns. Without one, own declares the schedule in
    /// creation order.
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
        for (owner, _) in &self.cache_duties {
            if !owners.contains(owner) {
                owners.push(*owner);
            }
        }
        for owner in owners {
            let duties: Vec<_> = self.cache_duties.iter().filter(|(scope, _)| *scope == owner)
                .map(|(_, future)| ScheduleDuty::Value(*future)).collect();
            if let Some(schedule) = self.body.service_schedules.iter_mut().find(|schedule| schedule.owner == owner) {
                if schedule.duties.is_empty() {
                    schedule.duties = duties;
                    continue;
                }
                let declared: BTreeSet<_> = schedule.duties.iter().copied().collect();
                if declared.len() != schedule.duties.len() || declared != duties.iter().copied().collect() {
                    return Err(phase_error(OwnDiagnosticKind::InvalidEquation, format!(
                        "service schedule {} differs from the cache duties of its scope", schedule.name,
                    )));
                }
                continue;
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
