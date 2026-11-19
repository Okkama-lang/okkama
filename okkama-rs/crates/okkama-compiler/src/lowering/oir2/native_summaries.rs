//! Trusted, finite synchronous P1 boundary catalog. No generic fallback contracts.
//!
//! Atomic's six entries are VM operations, not its placeholder Okkama bodies.
//! `$int`, `$string`, `$bool`, `$byte`, `$char`, `$float`, `$rat` and `$array_<suffix>` identify
//! closed OIR-2 ABI specializations (P2.4 §4.8); no dictionaries remain.
//! All held values are plain: replacement stores a GC reference, never an ownership
//! obligation. Consequently all parameter retention, result aliases and deps are empty.
//! - `new_value(T) -> Atomic<T>`: call boundary, no semantic failure.
//! - `value(Atomic<T>) -> T`: observation, may throw AtomicClosed.
//! - `replace(Atomic<T>, T) -> void`: call boundary, may throw AtomicClosed before storing.
//! - `compare_and_replace(Atomic<T>, T, T) -> bool`: call boundary, may throw AtomicClosed;
//!   publishes the revised value only on equality, and plain equality cannot fail.
//! - `closed(Atomic<T>) -> bool`: observation, no semantic failure.
//! - `close(Atomic<T>) -> void`: observation, idempotent, no semantic failure.
//!
//! OIR-2 §7.2 closes unretained fresh publication inputs at the call boundary.
//! Reading copies the stored borrow without changing its external lifetime
//! (P1.3 elaboration §12). Close ends the capability, not its borrowed values.
//! The value/close observations therefore keep admission at the caller.
//!
//! `Error.text(Error) -> string` observes the builtin plain Error view without failure.
//! Natives (#569 C7) follow the companion catalogue of the runtime table: an admitted
//! synchronous data or handle operation is a call boundary whose declaration must equal the
//! table's signature; any other canonical symbol takes the checked unresolved-provider adapter
//! (see [`native_transport`]). Data and handles carry no ownership graph; failures are the provider's
//! `NativeFailure` (Native) and the call's program failures (Program: missing provider, permission, #719).
//! An async native of the table (#601 N1, D22) gets its driver's body summary: it also suspends and may be
//! cancelled. No entry has an async destroy, a publication receipt or a resource result.
//!
//! Open sync callables and the opaque cleanup intrinsics (callables, #569 O1 and
//! C3b; `any` boxes, C6) come from the runtime catalogue of `okkama-oir`; own and
//! the verifier consult it.
use std::collections::BTreeMap;

use crate::oir2 as ir;
use ir::own::{ExternalBoundary, ExternalSummary, NativeSummaries, NativeSummaryError, Substitution};

#[derive(Default)]
pub(crate) struct P1NativeSummaries {
    natives: BTreeMap<String, (ir::Signature, ir::Summary)>,
    intrinsics: BTreeMap<String, (ir::Signature, ir::Summary)>,
    services: BTreeMap<ir::TypeId, ir::Summary>,
    runtime: ir::catalogue::RuntimeCatalogue,
}

fn error(message: impl Into<String>) -> NativeSummaryError {
    NativeSummaryError { message: message.into() }
}

impl P1NativeSummaries {
    pub fn for_module(module: &ir::Module) -> Result<Self, NativeSummaryError> {
        Self::for_profile(module, module.profile)
    }

    /// Elaboration registers the runtime entries with the ABI of its target profile.
    pub fn for_profile(module: &ir::Module, profile: ir::Profile) -> Result<Self, NativeSummaryError> {
        let runtime = ir::catalogue::RuntimeCatalogue::for_profile(module, profile);
        let mut catalog = Self { runtime, ..Self::default() };
        for (_, function) in module.functions.iter() {
            for attr in &function.attrs {
                let (native, name) = match attr {
                    ir::FunctionAttr::Extern(name) => (true, name),
                    ir::FunctionAttr::Intrinsic { name, .. } => (false, name),
                    _ => continue,
                };
                let runtime = [ir::catalogue::SYNC_CLOSURE_CLEANUP, ir::catalogue::ANY_CLEANUP];
                if !native && runtime.contains(&name.as_str()) {
                    if catalog.runtime.cleanup_signature(name, &function.signature).is_none() {
                        return Err(error(format!("invalid runtime intrinsic ABI {name}")));
                    }
                    continue;
                }
                let mut language = function.signature.clone();
                if module.profile != ir::Profile::High {
                    language.params.retain(|ty| !matches!(module.types.get(*ty),
                        Ok(ir::Type::Service(ir::ServiceType::Own | ir::ServiceType::Deps))));
                    language.results.retain(|ty| !matches!(module.types.get(*ty),
                        Ok(ir::Type::Service(ir::ServiceType::Own | ir::ServiceType::Deps))));
                    language.ownership_abi = ir::OwnershipAbi::Unelaborated;
                }
                let summary = contract(module, name, native, &language)?;
                // An async native is exactly an async native of the runtime table (#601 N1).
                if native && summary.contract.suspends != function.attrs.contains(&ir::FunctionAttr::Async) {
                    return Err(error(format!("native {name}: async differs from the runtime table")));
                }
                if module.profile != ir::Profile::High {
                    let mut expected = language.clone();
                    let managed = if !native && super::drivers::known(name) {
                        super::drivers::deps(name)
                    } else {
                        language.params.iter().filter(|ty| carries_deps(module, **ty)).count()
                    };
                    if managed != 0 {
                        let deps = module.types.iter().find_map(|(id, ty)| {
                            (*ty == ir::Type::Service(ir::ServiceType::Deps)).then_some(id)
                        }).ok_or_else(|| error("missing external deps type"))?;
                        expected.params.extend(std::iter::repeat_n(deps, managed));
                    }
                    if summary.entry_kind == ir::EntryKind::ObservationAdmissionStaysCaller {
                        expected.ownership_abi = ir::OwnershipAbi::Observation;
                    } else {
                        let own = module.types.iter().find_map(|(id, ty)| {
                            (*ty == ir::Type::Service(ir::ServiceType::Own)).then_some(id)
                        }).ok_or_else(|| error("missing external packet type"))?;
                        expected.ownership_abi = ir::OwnershipAbi::Explicit {
                            input_packet: expected.params.len() as u16,
                            output_packet: expected.results.len() as u16,
                        };
                        expected.params.push(own);
                        expected.results.push(own);
                    }
                    if expected != function.signature {
                        return Err(error(format!("invalid elaborated P1 ABI {name}")));
                    }
                }
                // A ready holder row's driver calls the program's helpers, one per kind the row lists (#810).
                let helper_count = okkama_sparrow::spb17::drivers::ready_holder(name).map_or(0, |row| row.helpers.len());
                if let ir::FunctionAttr::Intrinsic { objects, helpers, .. } = attr
                    && (*objects != intrinsic_objects(module, name, &language)? || helpers.len() != helper_count) {
                    return Err(error(format!("invalid P1 intrinsic descriptors for {name}")));
                }
                let entries = if native { &mut catalog.natives } else { &mut catalog.intrinsics };
                let entry = (function.signature.clone(), summary);
                if entries.get(name).is_some_and(|existing| existing != &entry) {
                    return Err(error(format!("conflicting P1 ABI {name}")));
                }
                entries.insert(name.clone(), entry);
            }
        }
        // Classification treats builtin objects/roles as services. These closed
        // plain specializations have no automatic close/destroy work. Atomic.close
        // remains an explicit source operation; plain Error views own no carrier.
        for (id, ty) in module.types.iter() {
            let decl = match ty {
                ir::Type::Object { decl, args } | ir::Type::Role { decl, args } if args.is_empty() => *decl,
                _ => continue,
            };
            let decl = module.declarations.get(decl).map_err(|err| error(err.to_string()))?;
            // The Event cell (#601 Q1) is a builtin capability of the same kind: plain, nothing to close. Its one
            // source field `initial` is a bool.
            let cell = decl.name.strip_prefix("async.atomic.Atomic_")
                .and_then(|suffix| atomic_value(module, suffix)).is_some();
            let event = decl.name == super::drivers::EVENT;
            let bool_type = module.types.iter().find_map(|(id, ty)| (*ty == ir::Type::Bool).then_some(id));
            let atomic = matches!(&decl.kind, ir::DeclarationKind::Object {
                    builtin: true, destroy: None, fields, witnesses,
                } if witnesses.is_empty() && (cell && fields.is_empty() || event && matches!(fields.as_slice(),
                    [field] if field.name == "initial" && Some(field.ty) == bool_type && !field.once)));
            let plain_error = decl.name == "Error"
                && matches!(decl.kind, ir::DeclarationKind::Role { builtin: true, .. });
            // #810: a D23 holder (FutureGroup) is a service of its own; its settle seals the group and settles the
            // registered members by their cache rows, whose effects their own cache settles carry.
            let holder = matches!(&decl.kind, ir::DeclarationKind::Object { builtin: true, destroy: None, fields, .. }
                if fields.is_empty()) && ir::catalogue::service_holder(&decl.name);
            if atomic || plain_error || holder {
                catalog.services.insert(id, ir::Summary {
                    entry_kind: ir::EntryKind::Internal,
                    parameters: vec![], results: vec![],
                    contract: ir::CallContract {
                        results: vec![], failures: vec![], suspends: false, async_destroy: false,
                        publication: ir::Publication::Atomic, protocol: None,
                    },
                });
            }
        }
        Ok(catalog)
    }
}

impl NativeSummaries for P1NativeSummaries {
    fn lookup(&self, boundary: ExternalBoundary<'_>, subst: &Substitution)
        -> Result<ExternalSummary, NativeSummaryError> {
        if !subst.types.is_empty() || !subst.descriptors.is_empty() {
            return Err(error("P1 boundaries require a closed specialization"));
        }
        if let ExternalBoundary::Service(ty) = boundary {
            return self.services.get(&ty).cloned().map(|summary| ExternalSummary { summary })
                .ok_or_else(|| error(format!("unregistered P1 service {ty:?}")));
        }
        let runtime = match boundary {
            ExternalBoundary::Callable(ty) => self.runtime.open_callable(ty),
            ExternalBoundary::Intrinsic(name) => self.runtime.cleanup_summary(name),
            _ => None,
        };
        if let Some(summary) = runtime {
            return Ok(ExternalSummary { summary: summary.clone() });
        }
        let entry = match boundary {
            ExternalBoundary::Native(name) => self.natives.get(name),
            ExternalBoundary::Intrinsic(name) => self.intrinsics.get(name),
            _ => None,
        };
        entry.map(|(_, summary)| ExternalSummary { summary: summary.clone() })
            .ok_or_else(|| error(format!("unregistered P1 boundary {boundary:?}")))
    }
}

impl ir::verify::ExternalContracts for P1NativeSummaries {
    fn native(&self, name: &str) -> Option<&ir::Signature> {
        self.natives.get(name).map(|entry| &entry.0)
    }
    fn intrinsic(&self, name: &str) -> Option<&ir::Signature> {
        self.intrinsics.get(name).map(|entry| &entry.0)
    }
    fn intrinsic_declaration(&self, name: &str, declared: &ir::Signature) -> Option<&ir::Signature> {
        self.intrinsics.get(name).map(|entry| &entry.0).or_else(|| self.runtime.cleanup_signature(name, declared))
    }
    fn native_summary(&self, name: &str) -> Option<&ir::Summary> {
        self.natives.get(name).map(|entry| &entry.1)
    }
    fn intrinsic_summary(&self, name: &str) -> Option<&ir::Summary> {
        self.intrinsics.get(name).map(|entry| &entry.1).or_else(|| self.runtime.cleanup_summary(name))
    }
    fn descriptor_cleanup(&self, name: &str) -> Option<&ir::Summary> {
        self.runtime.cleanup_summary(name)
    }
    fn open_callable(&self, callable: ir::TypeId) -> Option<ir::Summary> {
        self.runtime.open_callable(callable).cloned()
    }
}

pub(super) fn contract(module: &ir::Module, name: &str, native: bool, signature: &ir::Signature)
    -> Result<ir::Summary, NativeSummaryError> {
    // The std async primitives (#601 Q1, D22): the body summary of a driver, the entry summary otherwise.
    if !native && let Some(summary) = super::drivers::contract(module, name, signature)? {
        return Ok(summary);
    }
    let find = |ty: ir::Type| module.types.iter().find_map(|(id, value)| (*value == ty).then_some(id));
    let string = || find(ir::Type::Str).ok_or_else(|| error("missing string"));
    let mut kind = ir::EntryKind::CallBoundary;
    let mut failures = vec![];
    let mut suspends = false;
    let (params, results) = match (native, name) {
        (true, name) => {
            // A provider refuses with its own `NativeFailure` or with a program failure of the call (a missing
            // provider, a refused permission), which passes every catch uncaught (language spec 9.5, #719).
            failures.extend(super::drivers::mask_failures(okkama_sparrow::spb17::p1::NATIVE_FAILURES));
            // An async native (#601 N1, D22): this is its body summary, the driver's. It suspends and can be
            // cancelled; its inputs are plain data and handles, borrowed, and its result is plain data or a handle
            // the provider registered (async ABI §4.3: no fresh identity).
            if native_transport(module, name, signature)? {
                failures.push(ir::FailureKind::Cancelled);
                suspends = true;
            }
            (signature.params.clone(), signature.results.clone())
        }
        (false, "okkama.Error.text") => {
            kind = ir::EntryKind::ObservationAdmissionStaysCaller;
            let role = module.types.iter().find_map(|(id, ty)| match ty {
                ir::Type::Role { decl, args } if args.is_empty() => {
                    module.declarations.get(*decl).ok().filter(|decl| decl.name == "Error"
                        && matches!(decl.kind, ir::DeclarationKind::Role { builtin: true, .. })).map(|_| id)
                }
                _ => None,
            }).ok_or_else(|| error("missing builtin Error"))?;
            (vec![role], vec![string()?])
        }
        (false, name) if name.starts_with("okkama.test.") => {
            // Test intrinsics observe plain operands; outside a running case they fail (Program).
            // `skip` always fails: its program failure is the skip marker, never a thrown value.
            kind = ir::EntryKind::ObservationAdmissionStaysCaller;
            failures.push(ir::FailureKind::Program);
            let float = || find(ir::Type::Float).ok_or_else(|| error("missing float"));
            let bytes = || find(ir::Type::Byte).and_then(|byte| find(ir::Type::Array(byte)))
                .ok_or_else(|| error("missing @byte"));
            let params = match name.strip_prefix("okkama.test.") {
                Some("skip") => vec![string()?],
                Some("close") => vec![float()?, float()?, float()?],
                Some("text") => vec![string()?, string()?],
                Some("bytes") => vec![bytes()?, bytes()?],
                _ => return Err(error(format!("unknown test intrinsic {name}"))),
            };
            (params, vec![])
        }
        (false, name) => {
            let (operation, suffix) = name.strip_prefix("okkama.atomic.Atomic.")
                .and_then(|name| name.split_once('$')).ok_or_else(|| error(format!("unknown intrinsic {name}")))?;
            // Other closed plain data (#778) has no registered spelling: the operation being declared names its
            // value (`closed` and `close` take only the Atomic).
            let declared = match operation {
                "new_value" => signature.params.first().copied(),
                "value" => signature.results.first().copied(),
                "replace" | "compare_and_replace" => signature.params.get(1).copied(),
                _ => atomic_operand(module, suffix),
            };
            let known = atomic_value(module, suffix).or_else(|| declared.filter(|_| suffix.starts_with("t_")));
            let value = || known.ok_or_else(|| error(format!("unsupported Atomic value {suffix}")));
            if !suffix.starts_with("t_") {
                value()?;
            }
            let object = module.types.iter().find_map(|(id, ty)| match ty {
                ir::Type::Object { decl, args } if args.is_empty() => module.declarations.get(*decl).ok()
                    .filter(|decl| decl.name == format!("async.atomic.Atomic_{suffix}"))
                    .filter(|decl| matches!(&decl.kind, ir::DeclarationKind::Object {
                        builtin: true, destroy: None, fields, witnesses,
                    } if fields.is_empty() && witnesses.is_empty())).map(|_| id),
                _ => None,
            }).ok_or_else(|| error("missing plain Atomic specialization"))?;
            // PR-A (D25 tail): an Atomic over a value that can hold a Future borrows it. Every entry is an observation:
            // publication offers nothing, and the stored Future's duty stays with its owner.
            let borrows = known.is_some_and(|value| borrowed_future(module, value));
            if borrows {
                kind = ir::EntryKind::ObservationAdmissionStaysCaller;
            }
            match operation {
                "new_value" => (vec![value()?], vec![object]),
                "value" => {
                    kind = ir::EntryKind::ObservationAdmissionStaysCaller;
                    failures.push(ir::FailureKind::Thrown);
                    (vec![object], vec![value()?])
                }
                "replace" => {
                    failures.push(ir::FailureKind::Thrown);
                    (vec![object, value()?], vec![])
                }
                // Publishes `revised` only when the held value equals `expected`; a failed comparison
                // publishes nothing. Plain values compare without failure.
                // Futures do not compare (PR-A item 5).
                "compare_and_replace" if borrows => {
                    return Err(error(format!("unsupported Atomic operation {operation} over Futures (PR-A)")));
                }
                "compare_and_replace" => {
                    failures.push(ir::FailureKind::Thrown);
                    (vec![object, value()?, value()?], vec![find(ir::Type::Bool).ok_or_else(|| error("missing bool"))?])
                }
                "closed" => {
                    kind = ir::EntryKind::ObservationAdmissionStaysCaller;
                    (vec![object], vec![find(ir::Type::Bool).ok_or_else(|| error("missing bool"))?])
                }
                "close" => {
                    kind = ir::EntryKind::ObservationAdmissionStaysCaller;
                    (vec![object], vec![])
                }
                _ => return Err(error(format!("unknown Atomic operation {operation}"))),
            }
        }
    };
    if signature.params != params || signature.results != results {
        return Err(error(format!("invalid P1 signature for {name}")));
    }
    Ok(ir::Summary {
        entry_kind: kind,
        parameters: params.iter().map(|_| ir::ParameterEffect {
            convention: ir::FreshConvention::Borrow,
            retention: None,
            deps: vec![],
            destination: ir::TransferDestination::None,
        }).collect(),
        results: results.iter().map(|_| ir::ResultRelation { beside: vec![], held: vec![],
            fresh: vec![], aliases: vec![], external_deps: vec![], opaque: false,
            provenance: ir::AdmissionProvenance::UnboundResult,
        }).collect(),
        contract: ir::CallContract {
            results, failures, suspends, async_destroy: false, publication: ir::Publication::Atomic,
            protocol: None,
        },
    })
}

/// The interned value type of a registered Atomic specialization suffix (P2.4 §4.8): a scalar, `array_` and a
/// plain element suffix, or `role_` and a role declaration (#764); `None` for any other suffix or a type the
/// module lacks.
pub(super) fn atomic_value(module: &ir::Module, suffix: &str) -> Option<ir::TypeId> {
    // Other closed plain data (#778): the value type the specialization's operations declare.
    if suffix.starts_with("t_") {
        return atomic_operand(module, suffix);
    }
    // A role value (#764): the role declared under that name; the loader admits only a plain one.
    if let Some(name) = suffix.strip_prefix("role_") {
        return module.types.iter().find_map(|(id, ty)| match ty {
            ir::Type::Role { decl, args } if args.is_empty() => module.declarations.get(*decl).ok()
                .filter(|declaration| declaration.name == name).map(|_| id),
            _ => None,
        });
    }
    let ty = match suffix {
        "int" => ir::Type::Int,
        "string" => ir::Type::Str,
        "bool" => ir::Type::Bool,
        "byte" => ir::Type::Byte,
        "char" => ir::Type::Char,
        "float" => ir::Type::Float,
        "rat" => ir::Type::Rat,
        _ => ir::Type::Array(atomic_value(module, suffix.strip_prefix("array_")?)?),
    };
    module.types.iter().find_map(|(id, value)| (*value == ty).then_some(id))
}

/// PR-A: whether an Atomic's value is a Future or an array of Futures, which the Atomic borrows. The same structural
/// test as SPB 17's `p1::borrowed_future`, so the entry kinds of both sides agree.
fn borrowed_future(module: &ir::Module, ty: ir::TypeId) -> bool {
    let mut leaf = ty;
    while let Ok(ir::Type::Array(element)) = module.types.get(leaf) {
        leaf = *element;
    }
    matches!(module.types.get(leaf), Ok(ir::Type::Future(_)))
}

/// Whether an external's parameter of `ty` carries deps in its elaborated signature, as own gives them: an object
/// or a role value, and (#778, an Atomic over closed plain data) an array of role values, which own tracks as
/// references; a Future and an array of Futures (PR-A, an Atomic that borrows them). A plain variant or map carries none. The SPB 17 P1 entries count the same (`p1::entry`).
fn carries_deps(module: &ir::Module, ty: ir::TypeId) -> bool {
    let mut leaf = ty;
    while let Ok(ir::Type::Array(element)) = module.types.get(leaf) {
        leaf = *element;
    }
    matches!(module.types.get(ty), Ok(ir::Type::Object { .. } | ir::Type::Role { .. } | ir::Type::Future(_)))
        || leaf != ty && matches!(module.types.get(leaf), Ok(ir::Type::Role { .. } | ir::Type::Future(_)))
}

/// The value type of the Atomic specialization `suffix`, as its registered operations declare it (#778): the
/// value `new_value` publishes, `value` reads or `replace` stores. All of them must agree.
pub(super) fn atomic_operand(module: &ir::Module, suffix: &str) -> Option<ir::TypeId> {
    let mut found = None;
    for (_, function) in module.functions.iter() {
        let Some(name) = function.attrs.iter().find_map(|attr| match attr {
            ir::FunctionAttr::Intrinsic { name, .. } => Some(name.as_str()),
            _ => None,
        }) else { continue };
        let Some((operation, rest)) = name.strip_prefix("okkama.atomic.Atomic.").and_then(|name| name.split_once('$'))
            else { continue };
        if rest != suffix {
            continue;
        }
        let value = match operation {
            "new_value" => function.signature.params.first().copied(),
            "value" => function.signature.results.first().copied(),
            "replace" | "compare_and_replace" => function.signature.params.get(1).copied(),
            _ => None,
        };
        if let Some(value) = value {
            if found.is_some_and(|found| found != value) {
                return None;
            }
            found = Some(value);
        }
    }
    found
}

/// The general synchronous native ABI (P2.4 §4.7). A symbol of the runtime table is admitted only
/// as its companion catalogue classifies it (synchronous, data or handle transport) and only when
/// the declaration is exactly the table's signature, handle identities included. Any other
/// canonical symbol has no provider in the table: the checked unresolved-provider adapter carries
/// it when every type is data or a handle of its own module. Invocation then resolves the provider
/// at run time and fails as a missing provider; this is not a frontend rejection.
///
/// Returns whether the table's native is asynchronous (#601 N1): a driver whose call creates a Future. A symbol
/// outside the table is synchronous.
pub(super) fn native_transport(module: &ir::Module, name: &str, signature: &ir::Signature)
    -> Result<bool, NativeSummaryError> {
    use okkama_sparrow::abi::catalogue::{self, Admission, Symbol};
    let symbol = Symbol::parse(name).ok_or_else(|| error(format!("native symbol {name} is not canonical")))?;
    let handle_prefix = format!("{}.", symbol.module.replace('/', "."));
    match catalogue::admitted(name) {
        Ok(contract) => {
            let expected = contract.native.signature();
            let result = match &expected.result {
                okkama_sparrow::abi::Type::Void => None,
                ty => Some(ty),
            };
            let params = signature.params.len() == expected.params.len()
                && signature.params.iter().zip(&expected.params)
                    .all(|(&id, ty)| abi_type(module, id, ty, &handle_prefix));
            let results = match (signature.results.as_slice(), result) {
                ([], None) => true,
                ([id], Some(ty)) => abi_type(module, *id, ty, &handle_prefix),
                _ => false,
            };
            if !params || !results {
                return Err(error(format!("native {name} does not match the runtime table signature")));
            }
            Ok(contract.abi == catalogue::AbiKind::Async)
        }
        Err(Admission::Unknown) => {
            let transport = signature.params.iter().chain(&signature.results)
                .all(|&id| adapter_type(module, id, &handle_prefix));
            if symbol.arity != signature.params.len() || signature.results.len() > 1 || !transport {
                return Err(error(format!("native {name} has no provider contract and no data or handle transport")));
            }
            Ok(false)
        }
        Err(rejection) => Err(error(format!("native {name}: {rejection}"))),
    }
}

/// The handle declared by the native's module, by name.
fn own_handle(module: &ir::Module, decl: ir::DeclId, prefix: &str) -> Option<String> {
    let declaration = module.declarations.get(decl).ok()?;
    let name = declaration.name.strip_prefix(prefix)?;
    let valid = matches!(declaration.kind, ir::DeclarationKind::Handle) && declaration.type_params.is_empty()
        && !name.is_empty() && !name.contains('.');
    valid.then(|| name.to_owned())
}

fn abi_type(module: &ir::Module, id: ir::TypeId, expected: &okkama_sparrow::abi::Type, prefix: &str) -> bool {
    use okkama_sparrow::abi::Type as A;
    let Ok(ty) = module.types.get(id) else {
        return false;
    };
    match (ty, expected) {
        (ir::Type::Int, A::Int) | (ir::Type::Float, A::Float) | (ir::Type::Rat, A::Rat) | (ir::Type::Bool, A::Bool)
        | (ir::Type::Byte, A::Byte) | (ir::Type::Char, A::Char) | (ir::Type::Str, A::Str) => true,
        (ir::Type::Array(item), A::Array(expected)) => abi_type(module, *item, expected, prefix),
        (ir::Type::Map(key, value), A::Map(expected_key, expected_value)) => {
            abi_type(module, *key, expected_key, prefix) && abi_type(module, *value, expected_value, prefix)
        }
        (ir::Type::Handle(decl), A::Handle(name)) => own_handle(module, *decl, prefix).as_deref() == Some(*name),
        _ => false,
    }
}

fn adapter_type(module: &ir::Module, id: ir::TypeId, prefix: &str) -> bool {
    match module.types.get(id) {
        Ok(ir::Type::Int | ir::Type::Float | ir::Type::Rat | ir::Type::Bool | ir::Type::Byte | ir::Type::Char
            | ir::Type::Str) => true,
        Ok(ir::Type::Array(item)) => adapter_type(module, *item, prefix),
        Ok(ir::Type::Map(key, value)) => adapter_type(module, *key, prefix) && adapter_type(module, *value, prefix),
        Ok(ir::Type::Handle(decl)) => own_handle(module, *decl, prefix).is_some(),
        _ => false,
    }
}

/// Allocation and typed-failure descriptors required independently of source catch use.
/// The constructor allocates Self; value/replace construct AtomicClosed.closed.
pub(super) fn intrinsic_objects(module: &ir::Module, name: &str, signature: &ir::Signature)
    -> Result<Vec<ir::DeclId>, NativeSummaryError> {
    if let Some(objects) = super::drivers::objects(module, name)? {
        return Ok(objects);
    }
    if name.starts_with("okkama.atomic.Atomic.new_value$") {
        let result = signature.results.first().ok_or_else(|| error("Atomic constructor needs Self"))?;
        return match module.types.get(*result) {
            Ok(ir::Type::Object { decl, .. }) => Ok(vec![*decl]),
            _ => Err(error("Atomic constructor needs an object descriptor")),
        };
    }
    if name.starts_with("okkama.atomic.Atomic.value$") || name.starts_with("okkama.atomic.Atomic.replace$")
        || name.starts_with("okkama.atomic.Atomic.compare_and_replace$") {
        let decl = module.declarations.iter().find_map(|(id, decl)| {
            (decl.name == "async.atomic.AtomicClosed" && matches!(&decl.kind,
                ir::DeclarationKind::Enum { cases, is_error: true } if cases == &["closed"])).then_some(id)
        }).ok_or_else(|| error("Atomic access requires AtomicClosed.closed descriptor"))?;
        return Ok(vec![decl]);
    }
    Ok(vec![])
}
