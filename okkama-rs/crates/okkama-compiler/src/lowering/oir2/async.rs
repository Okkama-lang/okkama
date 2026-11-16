//! P2.5 V5 and R3 (#601): async in the High producer.
//!
//! - An async call is an ordinary High `call` of a function with `FunctionAttr::Async`. Its result is
//!   `future<T>` (plan §3.1). High has no packets (H2), so `fut.new` never appears here: own rewrites
//!   the call into `fut.new … environment deferred(…)` (elaboration §7.2). Receivers, arguments and
//!   results that own resources need nothing more in High: own derives the cold inputs and the
//!   cached outcome from the unelaborated call (R3, OIR-2 §§10.1–10.2).
//! - `await e` is the shared `await %f, result T`: a value, no deps. The producer never emits
//!   `await.take`; own decides take, because only own has the alias facts (decision C1). A Future
//!   that is a call's result awaited at once is a temporary: the producer marks it `temp` in the
//!   awaiting scope, its cleanup boundary (OIR-2 §10.3), which is how own finds the temporary take.
//! - `f.done()`, `f.cancel()` and `f.cancelled()` are `fut.done`, `fut.cancel` and `fut.cancelled`.
//! - `spawn e` of a Future is `e` followed by `fut.spawn %f`: D24.1's duty form when `e` is the creating call of an
//!   async action (slice P1), and `fut.spawn.bound` in own for any other Future (ruling PR-E, #766).
//! - `defer await …`: a defer entry whose expression awaits is an async defer entry (async ABI C-6),
//!   `[defer(), async]` with the async envelopes of H6. It has no replacement: a `return` inside it
//!   is refused by name (`async-defer-replace`).
//! - Every async construct outside these keeps a named source diagnostic for its later slice.
use super::body::BodyLowerer;
use super::{LowerError, Result};
use crate::analysis::{registry::Registry, thir, ty::Ty};
use crate::oir2 as ir;
use crate::source::Span;

/// `spawn` of a sync action or of a sync bound action value: a process worker.
const SPAWN_WORKER: &str = "spawn of a process worker (P2.5 slice X1, proc.spawn)";
/// A `return` inside a deferred expression that awaits: an async defer entry has no replacement in 0.6.0
/// (async ABI C-6, verifier rule `async-defer-replace`).
const ASYNC_DEFER_REPLACE: &str = "return inside an awaiting defer (async ABI C-6, async-defer-replace)";

/// Declares the High creation and body summaries of an async entry (elaboration H6).
///
/// High is unelaborated (H2): these are the conservative envelopes that `verify_high` requires,
/// not inferred ownership. Every parameter borrows with no retention, deps or destination; every
/// result is an unbound result with no fresh family or alias. The creation summary returns the
/// Future of the body result, never fails with a thrown error and never suspends: it does not run
/// the body. The body summary returns the body result and may throw, fail, suspend and be
/// cancelled. Own replaces both with its own creation and body equations (elaboration §3.3).
/// An async defer entry (`defer await`, async ABI C-6) creates no Future: it gets the body summary only (#730).
pub(super) fn declare(builder: &mut ir::ModuleBuilder, function: ir::FuncId) -> Result<()> {
    let module = builder.raw_mut();
    if !module.requires.contains(&ir::Feature::Async) {
        module.requires.push(ir::Feature::Async);
    }
    let signature = module.functions.get(function)?.signature.clone();
    let parameters = language(builder.module(), &signature.params)?.len();
    let results = language(builder.module(), &signature.results)?;
    let outcome = match results.as_slice() {
        [] => None,
        [result] => Some(*result),
        _ => return Err(LowerError::invalid(Span::default(), "async action with more than one result")),
    };
    let defer_entry = builder.module().functions.get(function)?.attrs.iter()
        .any(|attr| matches!(attr, ir::FunctionAttr::DeferEntry { .. }));
    let body = builder.add_summary(envelope(parameters, results, false))?;
    let creation = if defer_entry {
        None
    } else {
        let future = builder.intern_type(ir::Type::Future(outcome))?;
        Some(builder.add_summary(envelope(parameters, vec![future], true))?)
    };
    let declaration = builder.raw_mut().functions.get_mut(function)?;
    declaration.async_creation_summary = creation;
    declaration.async_body_summary = Some(body);
    Ok(())
}

/// Language slots of a High signature: service values and dictionaries are not part of a summary.
fn language(module: &ir::Module, types: &[ir::TypeId]) -> Result<Vec<ir::TypeId>> {
    let mut slots = vec![];
    for &ty in types {
        if !matches!(module.types.get(ty)?, ir::Type::Service(_) | ir::Type::Descriptor) {
            slots.push(ty);
        }
    }
    Ok(slots)
}

/// The High creation envelope of an async entry returning `future` (H6): what [`declare`] gives an async action,
/// and the creation summary of an async intrinsic (#601 Q1, D22).
pub(super) fn creation_envelope(parameters: usize, future: ir::TypeId) -> ir::Summary {
    envelope(parameters, vec![future], true)
}

fn envelope(parameters: usize, results: Vec<ir::TypeId>, creation: bool) -> ir::Summary {
    let parameter = ir::ParameterEffect {
        convention: ir::FreshConvention::Borrow,
        retention: None,
        deps: vec![],
        destination: ir::TransferDestination::None,
    };
    let relation = ir::ResultRelation {
        fresh: vec![],
        aliases: vec![],
        external_deps: vec![],
        opaque: false,
        provenance: ir::AdmissionProvenance::UnboundResult,
    };
    let failures = if creation {
        vec![ir::FailureKind::Program, ir::FailureKind::Panic, ir::FailureKind::Native]
    } else {
        vec![ir::FailureKind::Thrown, ir::FailureKind::Program, ir::FailureKind::Panic,
            ir::FailureKind::Native, ir::FailureKind::Cancelled]
    };
    ir::Summary {
        entry_kind: ir::EntryKind::CallBoundary,
        parameters: vec![parameter; parameters],
        results: vec![relation; results.len()],
        contract: ir::CallContract {
            results,
            failures,
            suspends: !creation,
            async_destroy: false,
            publication: ir::Publication::Atomic,
            protocol: None,
        },
    }
}

/// Declares the registered Services protocol (`okkama_oir::catalogue::services_protocol`) once in a
/// module whose bodies hold a Future. Own names it in every `await.unwind` and in the D16 schedule of
/// each cache duty (elaboration §13.3, D16); like the open Closure protocol (#569), a registered
/// protocol is declared by the producer, never inferred from a spelling. A module without Futures,
/// such as one whose only async entry is a test, declares nothing.
pub(super) fn attach_services(module: &mut ir::Module) -> Result<()> {
    let declared = module.protocols.iter().any(|(_, protocol)| protocol.kind == ir::ProtocolKind::Services);
    if declared {
        return Ok(());
    }
    let holds_future = module.functions.iter().filter_map(|(_, function)| function.body.as_ref()).any(|body| {
        body.values.iter().any(|(_, value)| matches!(module.types.get(value.ty), Ok(ir::Type::Future(_))))
    });
    if holds_future {
        module.protocols.alloc(ir::catalogue::services_protocol())?;
    }
    Ok(())
}

/// The owners whose async bodies plain async lowers: actions, `async main` and async tests.
/// `async destroy` belongs to slice D1.
pub(super) fn check_owner(registry: &Registry<'_>, body: &thir::Body) -> Result<()> {
    match body.owner {
        thir::BodyOwner::Entry | thir::BodyOwner::Method { .. } | thir::BodyOwner::Test(_) => Ok(()),
        thir::BodyOwner::Destroy { object } => {
            let name = &registry.objects[object.0].name;
            Err(LowerError::unsupported(body.span, format!("async destroy of {name} (P2.5 slice D1)")))
        }
        _ => Err(LowerError::unsupported(body.span, "async body outside an action")),
    }
}

/// A std body the runtime provides: `panic "… is a VM intrinsic";` first, or last after its
/// preparation. Such a body is never lowered (plan §3.1). The std async primitives are the catalogue of
/// slice Q1 (`drivers`, D22); channels arrive with slice Q2, groups, collectors and timeouts with G1 (D23).
pub(super) fn check_intrinsic(registry: &Registry<'_>, body: &thir::Body) -> Result<()> {
    let thir::BodyOwner::Method { object, method } = body.owner else {
        return Ok(());
    };
    if body.module >= registry.root_start {
        return Ok(());
    }
    let marks = |stmt: &thir::Stmt| {
        let thir::StmtKind::Expr(thir::Expr { kind: thir::ExprKind::Panic(message), .. }) = &stmt.kind else {
            return false;
        };
        matches!(&message.kind, thir::ExprKind::Constant(thir::Constant::Str(text))
            if text.ends_with("is a VM intrinsic"))
    };
    let stmts = &body.block.stmts;
    if !stmts.first().is_some_and(marks) && !stmts.last().is_some_and(marks) {
        return Ok(());
    }
    let info = &registry.objects[object.0];
    let name = format!("{}.{}", info.name, info.methods[method].name);
    let slice = match registry.modules[info.module].name.as_str() {
        "async/channel" => "Q2, D23",
        "async/scope" | "async/collect" | "async/timeout" => "G1, D23",
        _ => "Q1, D22",
    };
    Err(LowerError::unsupported(body.span, format!("std async intrinsic {name} (P2.5 slice {slice})")))
}

/// The diagnostic of an async native that no driver provides: its symbol is not an async native of the runtime
/// table (#601 N1, D22). A synchronous native outside the table takes the checked unresolved-provider adapter; an
/// async one has no such adapter.
pub(super) fn async_native(name: &str, span: Span) -> LowerError {
    LowerError::unsupported(span, format!("async native {name} has no driver: it is not an async native of the \
        runtime table (D22)"))
}

/// The async expressions outside plain async, with the diagnostic naming each.
pub(super) fn outside_plain(expr: &thir::Expr) -> Option<&'static str> {
    use thir::ExprKind as E;
    match &expr.kind {
        // Any Future: own keeps D24.1's duty form for a Future its creation just gave and `fut.spawn.bound` for every
        // other one (ruling PR-E, #766).
        E::Spawn(target) if matches!(target.ty, Some(Ty::Future(_))) => None,
        E::Spawn(_) => Some(SPAWN_WORKER),
        _ => None,
    }
}

/// Whether a deferred expression awaits: its defer entry is then an async defer entry (async ABI C-6).
pub(super) fn awaits(expr: &thir::Expr) -> bool {
    thir::first_in(expr, &|expr| matches!(expr.kind, thir::ExprKind::Await(_))).is_some()
}

/// The diagnostic of a `return` inside a deferred expression that awaits (async ABI C-6).
pub(super) fn async_defer_replace(span: Span) -> LowerError {
    LowerError::unsupported(span, ASYNC_DEFER_REPLACE)
}

/// Whether an awaited operand is a temporary Future: the result of a call, awaited at once. A Future
/// read from a binding or a field, or returned by `cancel()`, belongs to its owner (OIR-2 §10.3).
fn temporary(operand: &thir::Expr) -> bool {
    matches!(&operand.kind, thir::ExprKind::Call { callee, .. }
        if !matches!(callee, thir::Callee::Builtin { .. } | thir::Callee::Test(_)))
}

impl BodyLowerer<'_, '_> {
    /// `await e`: the shared await of the Future's cached outcome. It may fail with the outcome's
    /// failure, suspend and be cancelled, so it observes the current failure continuation (H5).
    /// A temporary Future is marked `temp` in the awaiting scope first (OIR-2 §10.3).
    pub(super) fn await_value(&mut self, operand: &thir::Expr, result: Option<&Ty>, span: Span)
        -> Result<Option<ir::ValueId>> {
        let Some(future) = self.value(operand)? else {
            return Ok(None);
        };
        if temporary(operand) {
            let scope = self.scope();
            self.b.emit(ir::Operation::Lexical(ir::LexicalOp::Temp { value: future, scope }), &[])?;
        }
        let result = result.map(|ty| self.ty(ty, span)).transpose()?;
        self.sync_failure()?;
        let values = self.b.emit_schema(ir::Operation::Async(ir::AsyncOp::Await { future, result }))?;
        Ok(values.first().copied())
    }

    /// `spawn R(…).a(…)` of an async action (D24.1, slice P1): the call creates the Future and `fut.spawn %f`
    /// starts it at once as a task of its own. Nothing comes between the creation and the spawn, which is the
    /// freshness `d24-spawn-not-fresh` requires; own forwards the owner state and adds the task duty. The value of
    /// the expression is the same Future, so it can be awaited, cancelled or kept.
    pub(super) fn spawn_value(&mut self, target: &thir::Expr) -> Result<Option<ir::ValueId>> {
        let Some(future) = self.value(target)? else {
            return Ok(None);
        };
        self.b.emit_schema(ir::Operation::Async(ir::AsyncOp::FutureSpawn { future, state: None }))?;
        Ok(Some(future))
    }

    /// `f.done()`, `f.cancel()` and `f.cancelled()` (Bible §20.1). None of them reads the value.
    /// `cancel()` only requests cancellation; its value is the same Future, as the frontend types it.
    pub(super) fn future_builtin(&mut self, op: &thir::Builtin, receiver: &thir::Expr, span: Span)
        -> Result<Option<ir::ValueId>> {
        let Some(future) = self.value(receiver)? else {
            return Ok(None);
        };
        let op = match op {
            thir::Builtin::FutureDone => ir::AsyncOp::FutureDone { future },
            thir::Builtin::FutureCancel => ir::AsyncOp::FutureCancel { future },
            thir::Builtin::FutureCancelled => ir::AsyncOp::FutureCancelled { future },
            _ => return Err(LowerError::invalid(span, "Future builtin dispatch")),
        };
        let request = matches!(op, ir::AsyncOp::FutureCancel { .. });
        let operation = ir::Operation::Async(op);
        let contract = ir::ContractRegistry::new(self.b.module())
            .lookup(&operation).map_err(ir::BuildError::from)?;
        if !contract.effects.failures.is_empty() || contract.effects.suspends {
            self.sync_failure()?;
        }
        let values = self.b.emit_schema(operation)?;
        if request {
            return Ok(Some(future));
        }
        Ok(values.first().copied())
    }
}
