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

/// The OIR declaration of the Event cell, whose runtime layout replaces its source field.
pub(super) const EVENT: &str = okkama_sparrow::spb17::drivers::EVENT;

use okkama_sparrow::spb17::drivers::{self as catalogue, Abi, Row};
use ir::FailureKind::{Cancelled, Native, Panic, Program, Thrown};

/// The catalogue entry `name`: the SPB 17 catalogue's row, the one source of the members, their failures and their
/// deps (#774 D3). Program failures are the runtime's refusal of an object it did not create (Tomato's "Invalid
/// async event"); they pass every catch.
fn entry(name: &str) -> Option<&'static Row> {
    catalogue::row_named(name)
}

/// The failure kinds of an SPB 17 entry's failure mask, in bit order (the loader's `EntryContract::failures`).
pub(super) fn mask_failures(mask: u32) -> Vec<ir::FailureKind> {
    let bits = [(catalogue::THROWN, Thrown), (catalogue::PROGRAM, Program), (catalogue::PANIC, Panic),
        (catalogue::NATIVE, Native)];
    bits.iter().filter(|(bit, _)| mask & bit != 0).map(|(_, kind)| *kind).collect()
}

/// The failures of a member, from its row's mask; a cancellation has no bit and is listed last.
fn failures(row: &Row) -> Vec<ir::FailureKind> {
    let mut failures = mask_failures(row.failures);
    if row.cancels {
        failures.push(Cancelled);
    }
    failures
}

/// Whether `name` is a catalogue entry.
pub(super) fn known(name: &str) -> bool {
    entry(name).is_some()
}

/// Whether the catalogue entry `name` is an async external (D22).
pub(super) fn asynchronous(name: &str) -> bool {
    entry(name).is_some_and(|entry| entry.abi == Abi::Driver)
}

/// Whether a std object's runtime layout replaces its source fields: the Event cell.
pub(super) fn builtin(module: &str, object: &str) -> bool {
    module == "async/event" && object == "Event"
}

/// Tomato's name of a std member: `okkama.<family>.<Object>.<action>`, a constructor's action `new` and its
/// parameter names joined by `_`, with the std declaration it belongs to (`async.<family>.<Object>`). None for a
/// body of the program or one that is not a member.
fn member_name(registry: &Registry<'_>, body: &thir::Body) -> Option<(String, String)> {
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
    let family = module.rsplit('/').next().unwrap_or(module);
    // A specialization keeps its source name before its arguments (`Channel<int>`).
    let object = info.name.split('<').next().unwrap_or(&info.name);
    Some((format!("okkama.{family}.{object}.{action}"), format!("{}.{object}", module.replace('/', "."))))
}

/// The D23 holder row (slices Q2 and G1, `spb17::drivers::HOLDERS`) of a std member the runtime provides. Until
/// those slices bring the runtime (`HOLDERS_READY`), the producer refuses the member by its row: its std body is
/// a placeholder (`panic "… is a VM intrinsic"`, or `throw NoFutures.none` where Tomato's driver decides).
pub(super) fn holder_row(registry: &Registry<'_>, body: &thir::Body) -> Option<&'static catalogue::Template> {
    let (name, declaration) = member_name(registry, body)?;
    catalogue::holder_named(&name).filter(|row| catalogue::member_of_holder(row) == declaration)
}

/// The runtime name of the body's member when the runtime provides it, as Tomato names it: the catalogue
/// objects' constructors (`new` and the parameter names, joined by `_`) and actions. None for an ordinary body.
pub(super) fn intrinsic_name(registry: &Registry<'_>, body: &thir::Body) -> Option<String> {
    let (name, declaration) = member_name(registry, body)?;
    // The SPB 17 rows name the objects (#799): `async/<family>`'s object whose members they are.
    if !catalogue::has_members(&declaration) {
        return None;
    }
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
        entry_kind: if entry.abi == Abi::Observation {
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
            results, failures: failures(entry), suspends: entry.abi == Abi::Driver, async_destroy: false,
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
    entry.related.iter().map(|object| {
        module.declarations.iter().find_map(|(id, declaration)| (declaration.name == *object).then_some(id))
            .ok_or_else(|| error(format!("{name} requires the {object} descriptor")))
    }).collect::<Result<Vec<_>, _>>().map(Some)
}

/// The deps parameters own's elaboration gives the catalogue entry `name`: the SPB 17 catalogue's count (#774 D3),
/// one for each Event cell parameter.
pub(super) fn deps(name: &str) -> usize {
    entry(name).map_or(0, catalogue::deps)
}

#[cfg(test)]
#[path = "drivers_tests.rs"]
mod tests;
