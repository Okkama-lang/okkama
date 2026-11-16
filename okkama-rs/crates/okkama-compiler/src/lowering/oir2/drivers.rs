//! #601 Q1, async ABI D22: the std async primitives whose state the runtime keeps (Event, Lock, Semaphore,
//! Barrier and the cancellation token), as a trusted catalogue of the High producer.
//!
//! - A member of these objects whose std body is `panic "… is a VM intrinsic"` (and the constructors Tomato
//!   overrides) is an `[intrinsic]` declaration without a body, named as Tomato names it
//!   (`okkama.<family>.<Object>.<member>`, OIR-1 `lowering/intrinsics.rs`). Its body is never lowered.
//! - A synchronous member is an ordinary external call with its catalogue summary.
//! - An `async` member is an async external (D22): `[async, intrinsic]`, its creation summary the conservative
//!   High envelope and its body summary the catalogue entry (`suspends=true`, `cancelled` among its failures).
//!   Its call creates a Future in the `external` environment, which the runtime runs with a driver.
//! - Every payload is plain: no parameter retains, no result is fresh or aliases, and nothing has a destroy.
//!   An Event is a runtime cell (`[initial, set, waiters]`), the one builtin layout here; the other objects keep
//!   their source fields (Semaphore and Barrier read `amount`) and the runtime keeps their state by identity.
//!
//! Channels (slice Q2), groups, collectors and timeouts (slice G1) are not in this catalogue.
use crate::analysis::{registry::{CtorShape, Registry}, thir};
use crate::oir2 as ir;
use ir::own::NativeSummaryError;

/// The std objects of the catalogue, by module and name.
const OBJECTS: [(&str, &str); 9] = [
    ("async/event", "Event"),
    ("async/lock", "Lock"),
    ("async/lock", "LockGuard"),
    ("async/semaphore", "Semaphore"),
    ("async/semaphore", "Permit"),
    ("async/barrier", "Barrier"),
    ("async/barrier", "BarrierState"),
    ("async/cancellation", "Cancellation"),
    ("async/cancellation", "CancellationToken"),
];

/// The OIR declaration of the Event cell, whose runtime layout replaces its source field.
pub(super) const EVENT: &str = okkama_sparrow::spb17::drivers::EVENT;

/// One catalogue entry: parameter and result types by name, its entry kind, failures, and whether it is async.
struct Entry {
    params: &'static [&'static str],
    results: &'static [&'static str],
    observation: bool,
    failures: &'static [ir::FailureKind],
    asynchronous: bool,
    /// The objects the runtime creates or throws (`Self` first for a constructor), by declaration name.
    objects: &'static [&'static str],
}

const LOCK: &str = "async.lock.Lock";
const GUARD: &str = "async.lock.LockGuard";
const SEMAPHORE: &str = "async.semaphore.Semaphore";
const PERMIT: &str = "async.semaphore.Permit";
const BARRIER: &str = "async.barrier.Barrier";
const BARRIER_STATE: &str = "async.barrier.BarrierState";
const BROKEN_BARRIER: &str = "async.barrier.BrokenBarrier";
const CANCELLATION: &str = "async.cancellation.Cancellation";
const TOKEN: &str = "async.cancellation.CancellationToken";
const CANCELLING: &str = "async.cancellation.Cancelling";

use ir::FailureKind::{Cancelled, Program, Thrown};

/// The catalogue (Tomato `intrinsics.rs` and `drivers.rs`). Program failures are the runtime's refusal of an
/// object it did not create (Tomato's "Invalid async event"); they pass every catch.
fn entry(name: &str) -> Option<Entry> {
    let sync = |params, results, observation, failures, objects| Entry {
        params, results, observation, failures, asynchronous: false, objects,
    };
    let driver = |params, results, failures, objects| Entry {
        params, results, observation: false, failures, asynchronous: true, objects,
    };
    Some(match name {
        "okkama.event.Event.new" => sync(&[], &[EVENT], false, &[], &[EVENT]),
        "okkama.event.Event.new_ready" => sync(&["bool"], &[EVENT], false, &[], &[EVENT]),
        "okkama.event.Event.set" => sync(&[EVENT], &[], true, &[Program], &[]),
        "okkama.event.Event.ready" => sync(&[EVENT], &["bool"], true, &[Program], &[]),
        "okkama.event.Event.wait" => driver(&[EVENT], &[EVENT], &[Program, Cancelled], &[]),
        "okkama.lock.Lock.new" => sync(&[], &[LOCK], false, &[], &[LOCK]),
        "okkama.lock.Lock.locked" => sync(&[LOCK], &["bool"], true, &[Program], &[]),
        "okkama.lock.Lock.guard" => driver(&[LOCK], &[GUARD], &[Program, Cancelled], &[GUARD]),
        "okkama.lock.LockGuard.closed" => sync(&[GUARD], &["bool"], true, &[Program], &[]),
        "okkama.lock.LockGuard.close" => sync(&[GUARD], &[], true, &[Program], &[]),
        "okkama.semaphore.Semaphore.permits" => sync(&[SEMAPHORE], &["int"], true, &[Program], &[]),
        "okkama.semaphore.Semaphore.permit" => driver(&[SEMAPHORE], &[PERMIT], &[Program, Cancelled], &[PERMIT]),
        "okkama.semaphore.Permit.closed" => sync(&[PERMIT], &["bool"], true, &[Program], &[]),
        "okkama.semaphore.Permit.close" => sync(&[PERMIT], &[], true, &[Program], &[]),
        "okkama.barrier.Barrier.wait" => {
            driver(&[BARRIER], &["int"], &[Thrown, Program, Cancelled], &[BROKEN_BARRIER])
        }
        "okkama.barrier.Barrier.state" => sync(&[BARRIER], &[BARRIER_STATE], false, &[Program], &[BARRIER_STATE]),
        "okkama.barrier.Barrier.abort" | "okkama.barrier.Barrier.reset" => {
            sync(&[BARRIER], &[], true, &[Program], &[])
        }
        "okkama.barrier.BarrierState.parties" | "okkama.barrier.BarrierState.waiters"
            | "okkama.barrier.BarrierState.generation" => sync(&[BARRIER_STATE], &["int"], true, &[Program], &[]),
        "okkama.barrier.BarrierState.broken" => sync(&[BARRIER_STATE], &["bool"], true, &[Program], &[]),
        "okkama.cancellation.Cancellation.new" => sync(&[], &[CANCELLATION], false, &[], &[CANCELLATION]),
        "okkama.cancellation.Cancellation.cancelling" => {
            sync(&[CANCELLATION], &[CANCELLING], false, &[Program], &[TOKEN])
        }
        "okkama.cancellation.Cancellation.cancel" => sync(&[CANCELLATION], &[], true, &[Program], &[]),
        "okkama.cancellation.CancellationToken.new" => sync(&[], &[TOKEN], false, &[], &[TOKEN]),
        "okkama.cancellation.CancellationToken.requested" => sync(&[TOKEN], &["bool"], true, &[Program], &[]),
        // A requested cancellation raises Cancelled at the check, as Tomato's `TokenCheck`.
        "okkama.cancellation.CancellationToken.check" => sync(&[TOKEN], &[], true, &[Program, Cancelled], &[]),
        "okkama.cancellation.CancellationToken.wait" => driver(&[TOKEN], &[CANCELLING], &[Program, Cancelled], &[]),
        _ => return None,
    })
}

/// Whether `name` is a catalogue entry.
pub(super) fn known(name: &str) -> bool {
    entry(name).is_some()
}

/// Whether the catalogue entry `name` is an async external (D22).
pub(super) fn asynchronous(name: &str) -> bool {
    entry(name).is_some_and(|entry| entry.asynchronous)
}

/// Whether a std object's runtime layout replaces its source fields: the Event cell.
pub(super) fn builtin(module: &str, object: &str) -> bool {
    module == "async/event" && object == "Event"
}

/// The runtime name of the body's member when the runtime provides it, as Tomato names it: the catalogue
/// objects' constructors (`new` and the parameter names, joined by `_`) and actions. None for an ordinary body.
pub(super) fn intrinsic_name(registry: &Registry<'_>, body: &thir::Body) -> Option<String> {
    let (object, action) = match body.owner {
        thir::BodyOwner::Method { object, method } => (object, registry.objects[object.0].methods[method].name.clone()),
        thir::BodyOwner::Ctor { object, ctor } => {
            let info = &registry.objects[object.0];
            let names: Vec<String> = match &info.ctors[ctor].shape {
                CtorShape::Full { params, .. } => params.iter().map(|(name, _)| name.clone()).collect(),
                CtorShape::Shorthand { fields, .. } => fields.iter().map(|&field| info.fields[field].name.clone())
                    .collect(),
            };
            (object, std::iter::once("new".to_string()).chain(names).collect::<Vec<_>>().join("_"))
        }
        _ => return None,
    };
    let info = &registry.objects[object.0];
    if info.module >= registry.root_start {
        return None;
    }
    let module = registry.modules[info.module].name.as_str();
    if !OBJECTS.contains(&(module, info.name.as_str())) {
        return None;
    }
    let family = module.rsplit('/').next().unwrap_or(module);
    let name = format!("okkama.{family}.{}.{action}", info.name);
    known(&name).then_some(name)
}

fn error(message: impl Into<String>) -> NativeSummaryError {
    NativeSummaryError { message: message.into() }
}

/// The interned type a catalogue type name stands for.
fn named_type(module: &ir::Module, name: &str) -> Result<ir::TypeId, NativeSummaryError> {
    let find = |ty: ir::Type| module.types.iter().find_map(|(id, value)| (*value == ty).then_some(id));
    let found = match name {
        "bool" => find(ir::Type::Bool),
        "int" => find(ir::Type::Int),
        _ => module.types.iter().find_map(|(id, ty)| match ty {
            ir::Type::Object { decl, args } | ir::Type::Role { decl, args } if args.is_empty() => {
                module.declarations.get(*decl).ok().filter(|declaration| declaration.name == name).map(|_| id)
            }
            _ => None,
        }),
    };
    found.ok_or_else(|| error(format!("missing catalogue type {name}")))
}

/// The catalogue summary of `name` over the language signature `signature`: the entry summary of a synchronous
/// member, the body summary of an async one. `None` when `name` is not in the catalogue.
pub(super) fn contract(module: &ir::Module, name: &str, signature: &ir::Signature)
    -> Result<Option<ir::Summary>, NativeSummaryError>
{
    let Some(entry) = entry(name) else {
        return Ok(None);
    };
    let params = entry.params.iter().map(|ty| named_type(module, ty)).collect::<Result<Vec<_>, _>>()?;
    let results = entry.results.iter().map(|ty| named_type(module, ty)).collect::<Result<Vec<_>, _>>()?;
    if signature.params != params || signature.results != results {
        return Err(error(format!("invalid Q1 signature for {name}")));
    }
    Ok(Some(ir::Summary {
        entry_kind: if entry.observation {
            ir::EntryKind::ObservationAdmissionStaysCaller
        } else {
            ir::EntryKind::CallBoundary
        },
        parameters: params.iter().map(|_| ir::ParameterEffect {
            convention: ir::FreshConvention::Borrow,
            retention: None,
            deps: vec![],
            destination: ir::TransferDestination::None,
        }).collect(),
        results: results.iter().map(|_| ir::ResultRelation {
            fresh: vec![], aliases: vec![], external_deps: vec![], opaque: false,
            provenance: ir::AdmissionProvenance::UnboundResult,
        }).collect(),
        contract: ir::CallContract {
            results, failures: entry.failures.to_vec(), suspends: entry.asynchronous, async_destroy: false,
            publication: ir::Publication::Atomic, protocol: None,
        },
    }))
}

/// The declarations the runtime creates or throws for `name` (the intrinsic's descriptors): `None` when `name`
/// is not in the catalogue.
pub(super) fn objects(module: &ir::Module, name: &str) -> Result<Option<Vec<ir::DeclId>>, NativeSummaryError> {
    let Some(entry) = entry(name) else {
        return Ok(None);
    };
    entry.objects.iter().map(|object| {
        module.declarations.iter().find_map(|(id, declaration)| (declaration.name == *object).then_some(id))
            .ok_or_else(|| error(format!("{name} requires the {object} descriptor")))
    }).collect::<Result<Vec<_>, _>>().map(Some)
}

/// The deps parameters own's elaboration gives a catalogue entry over its language parameters `params` (own's
/// external signatures): one for each managed reference, which among these types is the Event cell (a builtin
/// capability). The other objects of the catalogue are plain values without references. A member that is a role slot
/// of its object is reached through an adapter of the slot's ABI, never named by a witness table (ruling PR-D, #773),
/// so its receiver takes no deps.
///
/// The same rule builds the trusted SPB 17 entries (`okkama_sparrow::spb17::drivers::deps`): change both together.
pub(super) fn deps(module: &ir::Module, params: &[ir::TypeId]) -> usize {
    params.iter().filter(|&&ty| match module.types.get(ty) {
        Ok(ir::Type::Object { decl, .. }) => module.declarations.get(*decl).is_ok_and(|declaration| {
            matches!(declaration.kind, ir::DeclarationKind::Object { builtin: true, .. })
        }),
        Ok(ir::Type::Role { .. }) => true,
        _ => false,
    }).count()
}

#[cfg(test)]
#[path = "drivers_tests.rs"]
mod tests;
