use super::*;

fn unsupported(function: FuncId, message: &str) -> LowError {
    LowError::Unsupported { function, message: message.into() }
}
/// Async destroy suspends inside the cleanup plan; slice D1 (wave 4) lowers it.
pub(super) const ASYNC_DESTROY: &str = "async destroy is lowered by slice D1 (wave 4)";
/// An async defer entry (C-6) suspends inside the defer prefix; slice R2 (wave 2) lowers it.
pub(super) const ASYNC_DEFER: &str = "an async defer entry (C-6) runs only from an async Coro body";

/// Own input is synchronous only. Coro input (verified by `verify_coro`) keeps its frames and service values;
/// the async forms that a later slice lowers keep a named rejection.
pub(super) fn check(module: &Module, budget: &mut Budget<'_>) -> Result<(), LowError> {
    let coro = module.profile == Profile::Coro;
    let mut entities = 0_usize;
    for count in [module.types.len(), module.scopes.len(), module.functions.len(), module.declarations.len(),
        module.summaries.len(), module.protocols.len(), module.files.len()] {
        entities = entities.checked_add(count).ok_or(LowError::LimitExceeded("input entities"))?;
    }
    if entities > budget.options.max_entities {
        return Err(LowError::LimitExceeded("input entities"));
    }
    for (id, function) in module.functions.iter() {
        let is_async = function.attrs.contains(&FunctionAttr::Async);
        if function.body.is_some() && is_async && !coro {
            return Err(unsupported(id, "async function"));
        }
        if let Some(summary) = function.summary.filter(|_| function.body.is_some()) {
            let contract = &module.summaries.get(summary)?.contract;
            if contract.async_destroy {
                return Err(unsupported(id, ASYNC_DESTROY));
            }
            // An async body of Coro may suspend and be cancelled (D20, D18); everything else is synchronous.
            let async_body = coro && is_async;
            if !async_body && (contract.suspends || contract.failures.contains(&FailureKind::Cancelled)) {
                return Err(unsupported(id, "summary does not prove synchronous execution and owned cleanup"));
            }
        } else if function.body.is_some() {
            return Err(unsupported(id, "missing final function summary"));
        }
        let Some(body) = &function.body else {
            continue;
        };
        if coro && is_async && body.frame_layout.is_none() {
            return Err(LowError::InvalidInput {
                function: Some(id), message: "an async Coro body declares its D21 frame layout".into(),
            });
        }
        for count in [body.values.len(), body.blocks.len(), body.insts.len(), body.bindings.len(),
            body.boundaries.len(), body.deps.len(), body.keys.len(), body.selections.len(),
            body.descriptor_bindings.len(), body.service_schedules.len()] {
            entities = entities.checked_add(count).ok_or(LowError::LimitExceeded("input entities"))?;
        }
        if entities > budget.options.max_entities {
            return Err(LowError::LimitExceeded("input entities"));
        }
        if !body.service_schedules.is_empty() && !coro {
            return Err(unsupported(id, "nonempty service duty inventory"));
        }
        if body.cleanup != CleanupMetadata::default() || !body.high_results.is_empty() {
            return Err(unsupported(id, "input contains High or Low phase metadata"));
        }
        let entry = body.blocks.get(body.entry)?;
        for &parameter in &entry.params {
            budget.step(1)?;
            let opaque = match module.types.get(body.values.get(parameter)?.ty)? {
                Type::Service(ServiceType::Scope(_) | ServiceType::Pending) => true,
                Type::Service(ServiceType::Own) => {
                    let constructor = function.attrs.iter().any(|attr| matches!(attr, FunctionAttr::Ctor(_)));
                    let language_input = entry.params.iter().any(|value| {
                        body.values.get(*value).ok().and_then(|data| module.types.get(data.ty).ok())
                            .is_some_and(|ty| !matches!(ty, Type::Service(_) | Type::Descriptor))
                    });
                    // The verifier bounds a non-internal (or defer-entry) call packet by
                    // its argument retention graph: with no language input it is empty.
                    let bounded = function.summary.and_then(|summary| module.summaries.get(summary).ok())
                        .is_some_and(|summary| summary.entry_kind != EntryKind::Internal
                            || function.attrs.iter().any(|attr| matches!(attr, FunctionAttr::DeferEntry { .. })));
                    !constructor && !language_input && !bounded
                }
                _ => false,
            };
            if opaque {
                return Err(unsupported(id, "entry cleanup inventory has no typed synchronous ownership evidence"));
            }
        }
        for (_, value) in body.values.iter() {
            budget.step(1)?;
            let ty = module.types.get(value.ty)?;
            if matches!(ty, Type::Service(ServiceType::CarrierState)) {
                return Err(unsupported(id, "owned carrier-state values are excluded from 0.6.0"));
            }
            if !coro && matches!(ty, Type::Frame(_)
                | Type::Service(ServiceType::TaskState | ServiceType::CacheState
                    | ServiceType::Pin | ServiceType::Receipt | ServiceType::UniqueCache)) {
                return Err(unsupported(id, "service duty or coroutine state requires Coro input"));
            }
        }
        for binding in &body.descriptor_bindings {
            budget.step(1)?;
            let descriptor = &binding.contract;
            let contract = &module.summaries.get(descriptor.summary)?.contract;
            if descriptor.async_destroy || contract.async_destroy {
                return Err(unsupported(id, ASYNC_DESTROY));
            }
            if contract.suspends || contract.failures.contains(&FailureKind::Cancelled) {
                return Err(unsupported(id, "descriptor permits asynchronous owned cleanup"));
            }
        }
        let context = Context { module, function: id, coro, is_async };
        for (_, block) in body.blocks.iter() {
            for instruction in &block.insts {
                budget.step(1)?;
                context.operation(&body.insts.get(*instruction)?.op)?;
            }
            match &block.term.kind {
                TermKind::InvokeOwn { op, .. } => context.operation(op)?,
                // C-6 (R2): an async body of Coro runs an async defer entry as a suspending child frame; its
                // bracket stays, since the defer runs before the close's cleanup steps.
                TermKind::InvokeDefer { function: target, .. }
                    if coro && is_async && module.functions.get(*target)?.attrs.contains(&FunctionAttr::Async) => {}
                TermKind::InvokeDefer { function: target, call, .. } => {
                    if module.functions.get(*target)?.attrs.contains(&FunctionAttr::Async) {
                        return Err(unsupported(id, ASYNC_DEFER));
                    }
                    call_summary(module, id, call)?;
                }
                TermKind::CleanupNext { .. } | TermKind::ExitDispatch { .. } | TermKind::FailureDispatch { .. } => {
                    return Err(unsupported(id, "Low terminator in Own"));
                }
                _ => {}
            }
        }
        for (_, block) in body.blocks.iter() {
            for instruction in &block.insts {
                graph::operation(body, &body.insts.get(*instruction)?.op, budget)?;
            }
            graph::terminator(body, &block.term.kind, budget)?;
        }
    }
    Ok(())
}
fn call_summary(module: &Module, function: FuncId, call: &CallSite) -> Result<(), LowError> {
    let summary = call.summary.ok_or_else(|| unsupported(function, "call has no checked summary"))?;
    let contract = &module.summaries.get(summary)?.contract;
    if contract.suspends || contract.async_destroy || contract.failures.contains(&FailureKind::Cancelled) {
        return Err(unsupported(function, "call permits suspension, cancellation or async-owned cleanup"));
    }
    Ok(())
}
/// The function whose operations are checked, and its input profile.
struct Context<'a> {
    module: &'a Module,
    function: FuncId,
    coro: bool,
    is_async: bool,
}

impl Context<'_> {
    fn operation(&self, op: &Operation) -> Result<(), LowError> {
        if let Operation::Async(op) = op {
            return self.async_operation(op);
        }
        if let Operation::Ownership(OwnershipOp::ScopeEnd { .. } | OwnershipOp::ScopeUnwind { .. }) = op
            && self.coro && self.is_async {
            // D20/D18: an async close may suspend and deliver the final cancellation; the cleanup steps
            // decide whether it still suspends in Low (D20 §2.6).
            return Ok(());
        }
        operation(self.module, self.function, op)
    }

    /// The async forms of plain async (plan V4); the others belong to later slices.
    fn async_operation(&self, op: &AsyncOp) -> Result<(), LowError> {
        let function = self.function;
        if !self.coro {
            return Err(unsupported(function, "async/service operation requires Coro input"));
        }
        match op {
            AsyncOp::FutureNew { environment: FutureEnvironment::Frame(_), .. } | AsyncOp::AwaitShielded { .. }
            | AsyncOp::FrameAlloc { .. } | AsyncOp::FrameLoad { .. } | AsyncOp::FrameStore { .. }
            | AsyncOp::FrameCancel { .. } | AsyncOp::FutureSettle { .. } => Err(LowError::InvalidInput {
                function: Some(function), message: "a form that coro does not produce (D20, C-1)".into(),
            }),
            AsyncOp::FutureNew { .. } | AsyncOp::Await { .. } | AsyncOp::AwaitUnwind { .. }
            | AsyncOp::FrameSave { .. } | AsyncOp::FrameResume { .. } | AsyncOp::FuturePublish { .. }
            | AsyncOp::FutureCancel { .. } | AsyncOp::FutureDone { .. } | AsyncOp::FutureCancelled { .. } => Ok(()),
            // R2: a temporary take keeps its invoke and its proof; the cache duty settles as for a shared await.
            AsyncOp::AwaitTake { .. } | AsyncOp::UniqueCacheProof { .. } => Ok(()),
            // C-4: the token form of a duty created here, the own form of an entry the owner admitted (D25.3).
            AsyncOp::CacheRehome { form: RehomeForm::Token { task: None, .. } | RehomeForm::Scope { .. }, .. } => {
                Ok(())
            }
            AsyncOp::CacheRehome { form: RehomeForm::Token { .. }, .. } => {
                Err(unsupported(function, "the rehome of a spawned task's duty is lowered by slice P1 (wave 3)"))
            }
            AsyncOp::CacheRehome { form: RehomeForm::State(_), .. } => Err(LowError::InvalidInput {
                function: Some(function), message: "the pre-C-4 rehome form is not produced (C-4)".into(),
            }),
            AsyncOp::ServiceMaterialize { .. } | AsyncOp::ServiceAbsorb { .. } => {
                Err(unsupported(function, "packet duties in fields, arrays and groups (D25.2) come with G1"))
            }
            AsyncOp::FutureSpawn { .. } => Err(unsupported(function, "spawn (D24) is lowered by slice P1 (wave 3)")),
            AsyncOp::Pin { .. } | AsyncOp::PinTransfer { .. } | AsyncOp::Unpin { .. } => {
                Err(unsupported(function, "pins (D23) are lowered by slice Q2 (wave 3)"))
            }
            AsyncOp::WorkerBind { .. } | AsyncOp::WorkerSpawn { .. } | AsyncOp::ProcessSpawn { .. } => {
                Err(unsupported(function, "process workers are lowered by slice X1 (wave 4)"))
            }
        }
    }
}

fn operation(module: &Module, function: FuncId, op: &Operation) -> Result<(), LowError> {
    match op {
        Operation::Lexical(_) => return Err(unsupported(function, "High-only operation in Own")),
        Operation::Async(_) | Operation::Carrier(_) | Operation::Cleanup(_) => {
            return Err(unsupported(function, "async/service or preexisting Low operation"));
        }
        Operation::Ownership(OwnershipOp::Dynamic { reason, protocol, .. }) => {
            if !synchronous_dynamic(*reason, module.protocols.get(*protocol)?) {
                return Err(unsupported(function,
                    "dynamic source needs the storage merge, open Closure or any cleanup protocol"));
            }
        }
        Operation::Ownership(OwnershipOp::DynamicCheck { .. } | OwnershipOp::DynamicRefresh { .. }) => {
            return Err(unsupported(function, "dynamic check/refresh is outside the storage merge protocol"));
        }
        Operation::Ownership(OwnershipOp::ScopeEnd { effects, .. }
            | OwnershipOp::ScopeUnwind { effects, .. }) => {
            if effects.suspends || effects.cancel || effects.failures.contains(&FailureKind::Cancelled) {
                return Err(unsupported(function, "D18 does not prove synchronous close"));
            }
        }
        Operation::Call(call) => {
            let site = match call {
                CallOp::Direct { call, .. } | CallOp::Role { call, .. } | CallOp::Order { call, .. }
                | CallOp::Closure { call, .. }
                | CallOp::Cap { call, .. } | CallOp::Native { call, .. } | CallOp::Intrinsic { call, .. }
                | CallOp::Resume { call, .. } => call,
            };
            call_summary(module, function, site)?;
        }
        _ => {}
    }
    Ok(())
}

/// The checked synchronous transfer of a packet into the dynamic part of the named
/// scope: `packet -> scope`, open on normal, the unaccepted packet back on unwind,
/// no cancellation, every protocol check.
fn synchronous_transfer(rule: &ProtocolRule) -> bool {
    rule.transition == ProtocolTransition::Dynamic
        && rule.from == HolderKind::Packet && rule.to == HolderKind::Scope
        && rule.normal.holder == HolderKind::Scope
        && rule.normal.state == HolderState::Open
        && rule.unwind.as_ref().is_some_and(|post| {
            post.holder == HolderKind::Packet && post.state == HolderState::Unaccepted
        })
        && rule.cancel.is_none()
        && [ProtocolCheck::Owner, ProtocolCheck::LiveDependencies, ProtocolCheck::Snapshot, ProtocolCheck::Transferable]
            .iter().all(|check| rule.checks.contains(check))
}

/// Dynamic sources whose entries the synchronous cleanup automaton can settle: the
/// storage merge protocol, the `any` cleanup protocol (one transfer each) and the
/// registered open Closure protocol of the sync callable ABI (#683). Their entries
/// join the scope inventory in serial order and are released through their runtime
/// descriptors (`oir.any.cleanup`, `oir.sync_closure.cleanup`).
pub(super) fn synchronous_dynamic(reason: DynamicReason, protocol: &ProtocolDecl) -> bool {
    let holders = protocol.holders.iter().all(|holder| matches!(holder, HolderKind::Scope | HolderKind::Packet))
        && protocol.holders.contains(&HolderKind::Scope)
        && protocol.holders.contains(&HolderKind::Packet)
        && protocol.roots.contains(&RootKind::Operands);
    let transfers: Vec<_> = protocol.rules.iter()
        .filter(|rule| rule.transition == ProtocolTransition::Dynamic)
        .collect();
    let transfer = holders && matches!(transfers.as_slice(), [rule] if synchronous_transfer(rule));
    match (reason, &protocol.kind) {
        (DynamicReason::Merge, ProtocolKind::Storage) => transfer,
        (DynamicReason::Any, ProtocolKind::Any) => transfer && transfers.len() == protocol.rules.len(),
        (DynamicReason::Closure, ProtocolKind::Closure) => *protocol == crate::catalogue::open_callable_protocol(),
        _ => false,
    }
}
