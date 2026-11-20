//! Commit 5a, plain async (#601 V2): the async rules of own that hold for the
//! whole module, independently of one function's rewrite.
//!
//! An async entry has two summaries (elaboration H6, §3.3). Its body summary is
//! the ordinary entry summary of the body. Its creation summary describes what
//! `fut.new` keeps before the first start; it never runs the body. The cache
//! duty of a created Future is `ServiceOf{future, Cache}` in the owner state
//! (D12), settled by the owner's service schedule (D16) without waiting (D24.2).
//!
//! Commit 5b (R1): an async entry's inputs and outcome may own resources. The
//! creation retains the fresh inputs (OIR-2 §10.1), and the body's outcome
//! packet goes to the Future's cache, which a shared await only borrows (§10.2).
//! The remaining boundaries below name the slice of the P2.5 plan that lifts them.
use super::*;

/// The provenance bit of a cache duty in the lexical analysis journals. It is
/// neither admissible (1), bound (2) nor staged (4): a Future's duty never
/// travels through an argument offer, a binding or a stage (OIR-2 §10.2, B9).
pub(super) const CACHE_DUTY: u8 = 8;

pub(super) fn unsupported_async(message: impl Into<String>) -> OwnError {
    phase_error(OwnDiagnosticKind::UnsupportedAsync, message)
}

pub(super) fn is_future(module: &Module, ty: TypeId) -> bool {
    matches!(module.types.get(ty), Ok(Type::Future(_)))
}

/// #801 (D25.1, PR-B2): whether the `index`-th result of a synchronous call is a borrowed alias that carries no
/// cache duty: its callee's result relation names only aliases of its parameters (a getter of a Future field, a
/// relay of one), no fresh site. The duty stays with the holder of the aliased Future; the caller must not rehome
/// it. A call without a known summary is taken as giving its duty (D25.1).
pub(super) fn borrowed_result(module: &Module, op: &CallOp, index: usize) -> bool {
    let (CallOp::Direct { call, .. } | CallOp::Native { call, .. } | CallOp::Intrinsic { call, .. }
        | CallOp::Resume { call, .. } | CallOp::Role { call, .. } | CallOp::Order { call, .. }
        | CallOp::Closure { call, .. } | CallOp::Cap { call, .. }) = op;
    // The call of an async entry creates its Future (D12): never an alias, whatever the cold inputs it retains.
    if let CallOp::Direct { function, .. } | CallOp::Intrinsic { function, .. } = op
        && module.functions.get(*function).is_ok_and(|data| data.attrs.contains(&FunctionAttr::Async)) {
        return false;
    }
    let summary = call.summary.or_else(|| match op {
        CallOp::Direct { function, .. } => module.functions.get(*function).ok().and_then(|function| function.summary),
        _ => None,
    });
    summary.and_then(|summary| module.summaries.get(summary).ok())
        .and_then(|summary| summary.results.get(index))
        .is_some_and(|relation| relation.fresh.is_empty() && !relation.aliases.is_empty())
}

/// An async action: its call creates a Future and its body publishes through an exit transaction. An async
/// defer entry (`defer await`, async ABI C-6) is none: it runs inside its owner's cleanup through
/// `invoke.defer` and leaves like any defer entry (#730).
pub(super) fn is_async(module: &Module, function: FuncId) -> bool {
    module.functions.get(function).is_ok_and(|function| function.attrs.contains(&FunctionAttr::Async)
        && !function.attrs.iter().any(|attr| matches!(attr, FunctionAttr::DeferEntry { .. })))
}

/// An async external (D22): a bodyless `[async, intrinsic]` (#601 Q1) or `[async, extern]` (an async native, N1)
/// whose call creates a driver Future. Own treats its `external` environment as `deferred` (async ABI §4.2).
pub(super) fn is_async_external(module: &Module, function: FuncId) -> bool {
    module.functions.get(function).is_ok_and(|function| function.body.is_none()
        && function.attrs.contains(&FunctionAttr::Async)
        && function.attrs.iter().any(|attr| matches!(attr, FunctionAttr::Intrinsic { .. } | FunctionAttr::Extern(_))))
}

/// The language result of an async entry: the value its Future publishes.
pub(super) fn outcome(module: &Module, function: &Function) -> Result<Option<TypeId>, OwnError> {
    let results = language(module, &function.signature.results);
    match results.as_slice() {
        [] => Ok(None),
        [result] => Ok(Some(*result)),
        _ => Err(flow_error(format!("async entry {} has more than one language result", function.symbol))),
    }
}

/// The interned `future<T>` of an async entry, added before classification so
/// that the creation summary can name it.
pub(super) fn intern_futures(module: &mut Module) -> Result<(), OwnError> {
    let mut outcomes = vec![];
    for (_, function) in module.functions.iter() {
        if function.attrs.contains(&FunctionAttr::Async) {
            outcomes.push(outcome(module, function)?);
        }
    }
    for result in outcomes {
        module.types.intern(Type::Future(result)).map_err(arena)?;
    }
    Ok(())
}

pub(super) fn future_type(module: &Module, result: Option<TypeId>) -> Result<TypeId, OwnError> {
    module.types.iter().find_map(|(id, ty)| (*ty == Type::Future(result)).then_some(id))
        .ok_or_else(|| flow_error("async entry has no interned Future type"))
}

/// An async entry that plain async cannot elaborate at all: a bodyless one that is
/// no external driver, and async role slots (F1). An async intrinsic (#601 Q1) or
/// native (N1) is a D22 driver: its call creates a Future in the `external`
/// environment, and its body summary is the trusted catalogue entry. Only async
/// entries pay for the check.
pub(super) fn check_boundary(module: &Module, id: FuncId, budget: &mut Budget<'_>) -> Result<(), OwnError> {
    let function = module.functions.get(id).map_err(arena)?;
    if !function.attrs.contains(&FunctionAttr::Async) {
        return Ok(());
    }
    if function.body.is_none() && !is_async_external(module, id) {
        return Err(unsupported_async(format!(
            "async function {} has neither a body nor an external driver (D22)", function.symbol,
        )));
    }
    for (_, declaration) in module.declarations.iter() {
        budget.work(1)?;
        let DeclarationKind::Object { witnesses, .. } = &declaration.kind else {
            continue;
        };
        if witnesses.iter().any(|witness| witness.slots.contains(&id)) {
            return Err(unsupported_async(format!(
                "async role slot {}: async role slots arrive with F1", function.symbol,
            )));
        }
    }
    Ok(())
}

/// The creation summary of an async entry (elaboration H6, §3.3; plan §3.2).
///
/// Creating the Future never runs the body, so it neither fails nor suspends.
/// It retains every offered cold input, that is the fresh part of each managed
/// language parameter, for the Future's inputs holder, and the Future depends
/// on the external deps of those parameters, even when the body only borrows
/// them. The cache duty is no result packet: `fut.new` adds it to the owner
/// state (D12), so the Future result has no fresh family.
pub(super) fn creation_summary(
    module: &Module,
    function: &Function,
    types: &BTreeMap<TypeId, TypeClassification>,
    budget: &mut Budget<'_>,
) -> Result<Summary, OwnError> {
    let future = future_type(module, outcome(module, function)?)?;
    let mut managed = vec![];
    let mut parameters = vec![];
    let mut borrowed_futures = vec![];
    for (index, ty) in language(module, &function.signature.params).into_iter().enumerate() {
        let index = u16::try_from(index)
            .map_err(|_| budget.fail(OwnDiagnosticKind::IncompatibleSignature, "too many language parameters"))?;
        // #866 (PR-B2, OIR-2 §10.2): a creation borrows a bare Future argument; the Future it creates depends on that
        // Future's cache and holds nothing of it.
        if is_future(module, ty) {
            borrowed_futures.push(index);
            parameters.push(ParameterEffect {
                convention: FreshConvention::Borrow,
                retention: None,
                deps: vec![index],
                destination: TransferDestination::None,
            });
        } else if types.get(&ty).is_none_or(TypeClassification::may_own) {
            managed.push(index);
            parameters.push(ParameterEffect {
                convention: FreshConvention::Take,
                retention: Some(SummarySelection::Parameter(index)),
                deps: vec![index],
                destination: TransferDestination::Cache,
            });
        } else {
            parameters.push(ParameterEffect {
                convention: FreshConvention::Borrow,
                retention: None,
                deps: vec![],
                destination: TransferDestination::None,
            });
        }
    }
    let result = ResultRelation { beside: vec![], held: vec![], several: false,
        fresh: vec![],
        aliases: managed.iter().map(|index| SummarySelection::Parameter(*index)).collect(),
        external_deps: managed.into_iter().chain(borrowed_futures).collect(),
        opaque: false,
        provenance: AdmissionProvenance::UnboundResult,
    };
    let mut summary = Summary {
        entry_kind: EntryKind::CallBoundary,
        parameters,
        results: vec![result],
        contract: CallContract {
            results: vec![future],
            failures: vec![],
            suspends: false,
            async_destroy: false,
            publication: Publication::Atomic,
            protocol: None,
        },
    };
    normal::normalize(&mut summary, budget)?;
    Ok(summary)
}

/// The settlement of a Future's cache duty in its owner's close (async ABI
/// D24.2, parity). No service settle waits in 0.6.0: a cold Future settles as
/// an observed Cancelled, a running one detaches to the root registry, and a
/// settled one releases its outcome packet into the owner's serial plan. The
/// outcome's own destroy effects come from the Future type's classification,
/// which reaches the outcome type, so the settle step itself adds none.
pub(super) fn settle_summary() -> Summary {
    Summary {
        entry_kind: EntryKind::Internal,
        parameters: vec![],
        results: vec![],
        contract: CallContract {
            results: vec![],
            failures: vec![],
            suspends: false,
            async_destroy: false,
            publication: Publication::Atomic,
            protocol: None,
        },
    }
}

/// The module's protocol of service duties (D16): `await.unwind` and the cache
/// schedules name it.
pub(super) fn services_protocol(module: &Module) -> Result<ProtocolId, OwnError> {
    let mut found = module.protocols.iter().filter(|(_, protocol)| protocol.kind == ProtocolKind::Services);
    match (found.next(), found.next()) {
        (Some((id, _)), None) => Ok(id),
        (None, _) => Err(phase_error(OwnDiagnosticKind::MissingBoundary,
            "await unwind and cache schedules need a Services protocol declaration (D16)")),
        _ => Err(phase_error(OwnDiagnosticKind::MissingBoundary, "the module declares several Services protocols")),
    }
}

#[cfg(test)]
mod tests;
