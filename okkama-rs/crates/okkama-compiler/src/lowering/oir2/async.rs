//! P2.5 V5 (#601): plain async in the High producer.
//!
//! - An async call is an ordinary High `call` of a function with `FunctionAttr::Async`. Its result is
//!   `future<T>` (plan §3.1). High has no packets (H2), so `fut.new` never appears here: own rewrites
//!   the call into `fut.new … environment deferred(…)` (elaboration §7.2).
//! - `await e` is the shared `await %f, result T`: a value, no deps. The producer never emits
//!   `await.take`; own decides take, because only own has the alias facts (decision C1).
//! - `f.done()`, `f.cancel()` and `f.cancelled()` are `fut.done`, `fut.cancel` and `fut.cancelled`.
//! - Every async construct outside plain async keeps a named source diagnostic for its later slice.
use super::body::BodyLowerer;
use super::{LowerError, Result};
use crate::analysis::{registry::Registry, thir, ty::Ty};
use crate::oir2 as ir;
use crate::source::Span;

/// `spawn` of an async action: `fut.spawn` on the created Future (D24).
const SPAWN_ASYNC: &str = "spawn of an async action (P2.5 slice P1, fut.spawn)";
/// `spawn` of a sync action or of a sync bound action value: a process worker.
const SPAWN_WORKER: &str = "spawn of a process worker (P2.5 slice X1, proc.spawn)";
/// A bound async action value and its call (Bible §20.11).
const BOUND_ASYNC: &str = "bound async action value (P2.5 slice F1)";
/// `defer await …`: an async defer entry (async ABI C-6).
const DEFER_AWAIT: &str = "defer await (P2.5 slice R3, async defer entry)";

/// Declares the High creation and body summaries of an async entry (elaboration H6).
///
/// High is unelaborated (H2): these are the conservative envelopes that `verify_high` requires,
/// not inferred ownership. Every parameter borrows with no retention, deps or destination; every
/// result is an unbound result with no fresh family or alias. The creation summary returns the
/// Future of the body result, never fails with a thrown error and never suspends: it does not run
/// the body. The body summary returns the body result and may throw, fail, suspend and be
/// cancelled. Own replaces both with its own creation and body equations (elaboration §3.3).
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
    let future = builder.intern_type(ir::Type::Future(outcome))?;
    let body = builder.add_summary(envelope(parameters, results, false))?;
    let creation = builder.add_summary(envelope(parameters, vec![future], true))?;
    let declaration = builder.raw_mut().functions.get_mut(function)?;
    declaration.async_creation_summary = Some(creation);
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
/// preparation. Such a body is never lowered (plan §3.1); its catalogue entry arrives with Q1 (D22).
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
    Err(LowerError::unsupported(body.span, format!("std async intrinsic {name} (P2.5 slice Q1, D22)")))
}

/// The async native diagnostic: a native whose call creates a driver Future (D22).
pub(super) fn async_native(name: &str, span: Span) -> LowerError {
    LowerError::unsupported(span, format!("async native {name} (P2.5 slice N1, D22)"))
}

/// The async role slot diagnostic: slot summaries with `suspends=true` arrive with F1.
pub(super) fn async_role_slot(role: &str, slot: &str, span: Span) -> LowerError {
    LowerError::unsupported(span, format!("async role slot {role}.{slot} (P2.5 slice F1)"))
}

/// The type of an async action value: only a bound async action has one (Bible §20.11).
pub(super) fn async_action_type(span: Span) -> LowerError {
    LowerError::unsupported(span, BOUND_ASYNC)
}

/// The async expressions outside plain async, with the diagnostic naming each.
pub(super) fn outside_plain(expr: &thir::Expr) -> Option<&'static str> {
    use thir::ExprKind as E;
    let async_action = |ty: &Option<Ty>| matches!(ty, Some(Ty::Action(action)) if action.is_async);
    match &expr.kind {
        E::Spawn(target) if matches!(target.ty, Some(Ty::Future(_))) => Some(SPAWN_ASYNC),
        E::Spawn(_) => Some(SPAWN_WORKER),
        E::BoundAction { .. } | E::BoundRoleAction { .. } if async_action(&expr.ty) => Some(BOUND_ASYNC),
        E::CallValue { callee, .. } if async_action(&callee.ty) => Some(BOUND_ASYNC),
        _ => None,
    }
}

/// `defer await …` and every other deferred expression that awaits.
pub(super) fn check_defer(stmt: &thir::Stmt) -> Result<()> {
    if matches!(stmt.kind, thir::StmtKind::Defer(_))
        && thir::stmt_contains(stmt, &|expr| matches!(expr.kind, thir::ExprKind::Await(_))) {
        return Err(LowerError::unsupported(stmt.span, DEFER_AWAIT));
    }
    Ok(())
}

impl BodyLowerer<'_, '_> {
    /// `await e`: the shared await of the Future's cached outcome. It may fail with the outcome's
    /// failure, suspend and be cancelled, so it observes the current failure continuation (H5).
    pub(super) fn await_value(&mut self, operand: &thir::Expr, result: Option<&Ty>, span: Span)
        -> Result<Option<ir::ValueId>> {
        let Some(future) = self.value(operand)? else {
            return Ok(None);
        };
        let result = result.map(|ty| self.ty(ty, span)).transpose()?;
        self.sync_failure()?;
        let values = self.b.emit_schema(ir::Operation::Async(ir::AsyncOp::Await { future, result }))?;
        Ok(values.first().copied())
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
