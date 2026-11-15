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

pub(super) fn is_async(module: &Module, function: FuncId) -> bool {
    module.functions.get(function).is_ok_and(|function| function.attrs.contains(&FunctionAttr::Async))
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

/// An async entry that plain async cannot elaborate at all: async natives and
/// intrinsics arrive with the D22 driver bridge (Q1), async role slots with F1.
/// Only async entries pay for the check.
pub(super) fn check_boundary(module: &Module, id: FuncId, budget: &mut Budget<'_>) -> Result<(), OwnError> {
    let function = module.functions.get(id).map_err(arena)?;
    if !function.attrs.contains(&FunctionAttr::Async) {
        return Ok(());
    }
    if function.body.is_none() {
        return Err(unsupported_async(format!(
            "async native or intrinsic {}: driver Futures arrive with D22 (Q1)", function.symbol,
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
    for (index, ty) in language(module, &function.signature.params).into_iter().enumerate() {
        let index = u16::try_from(index)
            .map_err(|_| budget.fail(OwnDiagnosticKind::IncompatibleSignature, "too many language parameters"))?;
        if types.get(&ty).is_none_or(TypeClassification::may_own) {
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
    let result = ResultRelation {
        fresh: vec![],
        aliases: managed.iter().map(|index| SummarySelection::Parameter(*index)).collect(),
        external_deps: managed,
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
