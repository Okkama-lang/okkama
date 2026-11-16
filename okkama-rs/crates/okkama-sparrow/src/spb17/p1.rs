//! Closed P1 host contracts, constructed independently of artifact function declarations.
//!
//! Natives (#569 C7): every operation the companion catalogue of the runtime table admits to the
//! synchronous native ABI is a trusted owning entry built from the table's signature, never from
//! the artifact. Handles are matched by their declaring module and name. A canonical symbol that is
//! not in the table may only use the checked unresolved-provider adapter: data or handle transport
//! of its own module, an owning entry that can fail with a native failure, and runtime resolution.
use super::*;
use crate::abi::{self, catalogue};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug)]
pub struct P1Contracts {
    core: Vec<Function>,
    natives: BTreeMap<String, Function>,
    /// Admitted natives of the table with a map parameter or result (#752): their entries name the program's
    /// map composites, so the data adapter rebuilds them per declaration.
    mapped: BTreeMap<String, abi::Native>,
    handles: BTreeMap<String, u32>,
    /// The `fn` roles: one owning slot named `call`.
    callables: Vec<u32>,
    /// The declarations each std async primitive entry creates or throws (#601 Q1, `drivers`).
    related: BTreeMap<String, Vec<u32>>,
}

/// The failure mask of a native entry: a provider refusal (`NativeFailure`, Native = 8) or a program failure of
/// the call (Program = 2: a missing provider, a refused permission), which no catch takes (language spec 9.5,
/// #719). A provider never throws an error value or panics: a Rust panic stops the run.
pub const NATIVE_FAILURES: u32 = 2 | 8;

/// The opaque cleanup intrinsic of an action value (OIR-2 catalogue, #569 C3b).
pub const SYNC_CLOSURE_CLEANUP: &str = "oir.sync_closure.cleanup";
/// The opaque cleanup intrinsic of an `any` box (OIR-2 catalogue, #569 C6).
pub const ANY_CLEANUP: &str = "oir.any.cleanup";

impl P1Contracts {
    pub fn new(objects: &[Object]) -> Result<Self, Error> {
        let mut entries = Vec::new();
        let closed = objects.iter().any(|object| object.name == "async.atomic.AtomicClosed"
            && object.error_cases == ["AtomicClosed.closed"]
            && object.fields.is_empty() && object.layout.storage_fields == [Type::Int]
            && !object.layout.may_reach_resource);
        for object in objects.iter().filter(|object| !object.error_cases.is_empty()) {
            if !error_layout(objects, object) {
                return Err(error("invalid P1 plain Error layout"));
            }
        }
        for (id, object) in objects.iter().enumerate() {
            let Some(suffix) = object.name.strip_prefix("async.atomic.Atomic_") else {
                continue;
            };
            let Some(value) = atomic_value(suffix) else {
                continue;
            };
            let expected = vec![value, Type::Bool];
            if object.layout.storage_fields != expected || object.layout.may_reach_resource
                || object.layout.has_destroy || object.layout.may_async_destroy || !object.fields.is_empty()
            {
                return Err(error("invalid P1 Atomic runtime layout"));
            }
            let atomic = Type::Object(id as u32);
            // OIR-2 §7.2: publication closes fresh inputs with the call; reads
            // and close preserve the external lifetime of borrowed values.
            for (op, params, results, owned, failures) in [
                ("new_value", vec![value], vec![atomic], true, 0),
                ("value", vec![atomic], vec![value], false, 1),
                ("replace", vec![atomic, value], vec![], true, 1),
                ("compare_and_replace", vec![atomic, value, value], vec![Type::Bool], true, 1),
                ("closed", vec![atomic], vec![Type::Bool], false, 0),
                ("close", vec![atomic], vec![], false, 0),
            ] {
                if failures != 0 && !closed {
                    continue;
                }
                entries.push(entry(&format!("okkama.atomic.Atomic.{op}${suffix}"), params, results, owned, failures));
            }
        }
        entries.push(entry("okkama.Error.text", vec![Type::Error], vec![Type::Str], false, 0));
        // The std async primitives (#601 Q1, D22): synchronous entries and drivers.
        let mut related = BTreeMap::new();
        for (function, objects) in super::drivers::entries(objects) {
            related.insert(function.symbol.clone(), objects);
            entries.push(function);
        }
        // Test intrinsics of the test profile (`check_tests`). Each observes plain values and fails with a
        // program failure outside a running case; `skip` always fails, ending its case with the skip marker.
        for (name, params) in [
            ("skip", vec![Type::Str]),
            ("close", vec![Type::Float, Type::Float, Type::Float]),
            ("text", vec![Type::Str, Type::Str]),
            ("bytes", vec![Type::Bytes, Type::Bytes]),
        ] {
            entries.push(entry(&format!("{TEST_INTRINSIC_PREFIX}{name}"), params, vec![], false, 2));
        }
        let mut handles = BTreeMap::new();
        for (id, object) in objects.iter().enumerate().filter(|(_, object)| object.handle) {
            // A second declaration of one name would silently replace the first (`HANDLE_NAME`).
            enforce(handles.insert(object.name.clone(), id as u32).is_none(), "HANDLE_NAME",
                "two native handle declarations share one name")?;
        }
        let mut natives = BTreeMap::new();
        let mut mapped = BTreeMap::new();
        for &native in abi::Native::ALL {
            if catalogue::admitted(native.symbol()).is_err() {
                continue;
            }
            let prefix = handle_prefix(native.module());
            let signature = native.signature();
            let registers = signature.params.iter().map(|ty| abi_type(ty, &prefix, &handles)).collect::<Option<_>>();
            let results = match &signature.result {
                abi::Type::Void => Some(vec![]),
                ty => abi_type(ty, &prefix, &handles).map(|ty| vec![ty]),
            };
            // An operation over a type this bridge does not carry (char and rat arrays) has no entry yet; one over
            // maps has its data adapter.
            if let (Some(registers), Some(results)) = (registers, results) {
                let function = entry(native.symbol(), registers, results, true, NATIVE_FAILURES);
                natives.insert(native.symbol().to_owned(), function);
            } else if signature.params.iter().chain([&signature.result]).any(|ty| matches!(ty, abi::Type::Map(..))) {
                mapped.insert(native.symbol().to_owned(), native);
            }
        }
        let callables = objects.iter().enumerate().filter(|(_, object)| object.role.as_ref().is_some_and(|role| {
            matches!(role.slots.as_slice(), [slot] if slot.name == "call" && slot.entry_abi == EntryAbi::Owned)
        })).map(|(id, _)| id as u32).collect();
        Ok(Self { core: entries, natives, mapped, handles, callables, related })
    }

    /// The finite P1 entries: Atomic operations, `Error.text`, the test intrinsics and the std async primitives.
    pub fn entries(&self) -> &[Function] {
        &self.core
    }

    /// The declarations a std async primitive entry creates or throws, in Tomato's order (#601 Q1).
    pub fn related(&self, name: &str) -> Option<&[u32]> {
        self.related.get(name).map(Vec::as_slice)
    }

    /// The trusted entry of a native of the runtime table.
    pub fn native(&self, symbol: &str) -> Option<&Function> {
        self.natives.get(symbol)
    }

    /// The checked adapter for `declared` (P2.4 §4.7): an opaque cleanup, the data adapter of a table native with
    /// a map parameter or result (#752), or the unresolved-provider adapter. `None` unless the symbol is canonical
    /// and, for the unresolved provider, absent from the runtime table, with every type data or a handle of its
    /// module. A map in the transport names one of `composites`; its key and value are plain data the adapter
    /// carries ([`plain_data`]).
    pub fn adapter(&self, declared: &Function, composites: &[Composite]) -> Option<Function> {
        if let Some(entry) = self.opaque_cleanup(declared) {
            return Some(entry);
        }
        let name = declared.proofs.entry.as_ref()?.external.as_deref()?;
        let symbol = catalogue::Symbol::parse(name)?;
        if let Some(native) = self.mapped.get(name) {
            return self.mapped_entry(*native, declared, composites);
        }
        if catalogue::admitted(name) != Err(catalogue::Admission::Unknown) {
            return None;
        }
        let prefix = handle_prefix(symbol.module);
        let valid = |ty: &Type| match ty {
            Type::Handle(id) => self.handles.iter().any(|(handle, &index)| index == *id
                && handle.strip_prefix(&prefix).is_some_and(|name| !name.is_empty() && !name.contains('.'))),
            Type::Map(id) => matches!(composites.get(*id as usize),
                Some(Composite::Map { key, value }) if plain_data(*key) && plain_data(*value)),
            ty => plain_data(*ty),
        };
        let registers = declared.registers.get(..symbol.arity)?;
        if declared.registers.len() != symbol.arity || declared.results.len() > 1
            || !registers.iter().chain(&declared.results).all(valid)
        {
            return None;
        }
        Some(entry(name, registers.to_vec(), declared.results.clone(), true, NATIVE_FAILURES))
    }
}

impl P1Contracts {
    /// The data adapter of a table native whose signature has a map (#752): the table's signature, where each map
    /// is the declaration's composite of exactly the table's key and value types, both plain data the adapter
    /// carries. Every other type is the table's, as in a trusted entry.
    fn mapped_entry(&self, native: abi::Native, declared: &Function, composites: &[Composite]) -> Option<Function> {
        let prefix = handle_prefix(native.module());
        let signature = native.signature();
        let matches = |expected: &abi::Type, declared: &Type| match (expected, declared) {
            (abi::Type::Map(key, value), Type::Map(id)) => {
                let key = abi_type(key, &prefix, &self.handles);
                let value = abi_type(value, &prefix, &self.handles);
                match (key, value, composites.get(*id as usize)) {
                    (Some(key), Some(value), Some(Composite::Map { key: declared_key, value: declared_value })) => {
                        plain_data(key) && plain_data(value) && key == *declared_key && value == *declared_value
                    }
                    _ => false,
                }
            }
            (expected, declared) => abi_type(expected, &prefix, &self.handles) == Some(*declared),
        };
        let results = match &signature.result {
            abi::Type::Void => vec![],
            ty => vec![ty.clone()],
        };
        if declared.registers.len() != signature.params.len() || declared.results.len() != results.len()
            || !signature.params.iter().zip(&declared.registers).all(|(expected, ty)| matches(expected, ty))
            || !results.iter().zip(&declared.results).all(|(expected, ty)| matches(expected, ty))
        {
            return None;
        }
        Some(entry(native.symbol(), declared.registers.clone(), declared.results.clone(), true, NATIVE_FAILURES))
    }

    /// The opaque cleanup family, keyed by its first parameter (OIR-2 catalogue, #569 C3b and C6):
    /// `oir.sync_closure.cleanup` takes an action value (a `fn` role: one owning `call` slot) and
    /// `oir.any.cleanup` an `any` box, each with its deps and the packet of the value's complete owner graph.
    /// The entry releases that graph and returns an empty packet; it cannot fail. It is recomputed from
    /// the key alone, so the declaration must equal it exactly.
    pub fn opaque_cleanup(&self, declared: &Function) -> Option<Function> {
        let name = declared.proofs.entry.as_ref()?.external.as_deref()?;
        let key = *declared.registers.first()?;
        let admitted = match (name, key) {
            (SYNC_CLOSURE_CLEANUP, Type::Role(id)) => self.callables.contains(&id),
            (ANY_CLEANUP, Type::Any) => true,
            _ => false,
        };
        if !admitted {
            return None;
        }
        Some(Function {
            symbol: name.into(), entry_abi: EntryAbi::Owned,
            registers: vec![key], services: vec![ServiceKind::Deps, ServiceKind::Packet],
            params: vec![Location::Register(0), Location::Service(0), Location::Service(1)], results: vec![],
            blocks: vec![], code: vec![],
            proofs: FunctionProofs {
                entry: Some(EntryContract { external: Some(name.into()), ..EntryContract::default() }),
                ..FunctionProofs::default()
            },
        })
    }
}

/// A plain Error layout whose text `okkama.Error.text` writes: an Error enum (one case-number word), a
/// case of an Error variant without payload (no storage), or a case with payload fields of the types an
/// error payload may have and the text can show: scalars, plain enums that name their cases, and Error
/// enums and Error variants (an `Error` role payload has no text on Tomato either).
pub fn error_layout(objects: &[Object], object: &Object) -> bool {
    let layout = &object.layout;
    if layout.may_reach_resource || layout.has_destroy || object.enum_cases != 0 || !object.variant_cases.is_empty()
        || object.handle || object.role.is_some() || object.any_box
    {
        return false;
    }
    if object.fields.is_empty() {
        return layout.storage_fields == [Type::Int]
            || (layout.storage_fields.is_empty() && object.error_cases.len() == 1);
    }
    object.error_cases.len() == 1 && payload_shown(objects, object, &mut BTreeSet::new())
}

/// Whether every storage word of a case object is one payload field, in declaration order, of a type its text
/// shows (language spec, error enums: scalars, and enums whose payloads satisfy the same rule, #763): a scalar,
/// an Error value, a plain enum that names its cases, or a variant whose cases are Error cases or the named
/// cases of a plain variant with shown payloads. `visiting` stops a recursive variant.
fn payload_shown(objects: &[Object], object: &Object, visiting: &mut BTreeSet<u32>) -> bool {
    let layout = &object.layout;
    let mut storage = object.fields.iter().map(|field| field.storage as usize).collect::<Vec<_>>();
    storage.sort_unstable();
    object.fields.iter().all(|field| !field.once)
        && storage.iter().copied().eq(0..layout.storage_fields.len())
        && layout.storage_fields.iter().all(|ty| shown(objects, *ty, visiting))
}

fn shown(objects: &[Object], ty: Type, visiting: &mut BTreeSet<u32>) -> bool {
    match ty {
        Type::Str | Type::Int | Type::Bool | Type::Byte | Type::Float | Type::Rat => true,
        Type::Object(id) => objects.get(id as usize).is_some_and(|payload| !payload.error_cases.is_empty()
            || (payload.enum_cases > 0 && payload.enum_names.len() == payload.enum_cases as usize
                && payload.fields.is_empty() && payload.layout.storage_fields == [Type::Int])),
        Type::Variant(id) => {
            let Some(variant) = objects.get(id as usize) else { return false };
            if variant.variant_cases.is_empty() {
                return false;
            }
            if variant.variant_cases.iter().all(|&case| objects.get(case as usize)
                .is_some_and(|case| !case.error_cases.is_empty())) {
                return true;
            }
            // A plain variant names its cases; each case shows its payload. A variant met again is shown by the
            // outer check.
            if variant.enum_names.len() != variant.variant_cases.len() {
                return false;
            }
            if !visiting.insert(id) {
                return true;
            }
            let shown = variant.variant_cases.iter().all(|&case| objects.get(case as usize).is_some_and(|case| {
                case.error_cases.is_empty() && !case.layout.has_destroy && !case.layout.may_reach_resource
                    && payload_shown(objects, case, visiting)
            }));
            visiting.remove(&id);
            shown
        }
        _ => false,
    }
}

/// The runtime word type held by a registered plain Atomic specialization (P2.4 §4.8): a scalar, or
/// `array_` and a plain element suffix that SPB 17 can carry.
pub fn atomic_value(suffix: &str) -> Option<Type> {
    Some(match suffix {
        "int" => Type::Int,
        "string" => Type::Str,
        "bool" => Type::Bool,
        "byte" => Type::Byte,
        "char" => Type::Char,
        "float" => Type::Float,
        "rat" => Type::Rat,
        "array_byte" => Type::Bytes,
        _ => {
            let leaf = match atomic_value(suffix.strip_prefix("array_")?)? {
                Type::Int => ArrayLeaf::Int,
                Type::Bool => ArrayLeaf::Bool,
                Type::Str => ArrayLeaf::Str,
                Type::Bytes => ArrayLeaf::Bytes,
                Type::Float => ArrayLeaf::Float,
                Type::Char => ArrayLeaf::Char,
                Type::Rat => ArrayLeaf::Rat,
                Type::Array(shape) => {
                    let depth = shape.depth.checked_add(1)?;
                    return Some(Type::Array(ArrayType { depth, ..shape }));
                }
                _ => return None,
            };
            Type::Array(ArrayType { leaf, depth: 1 })
        }
    })
}

/// Plain data a native adapter carries without a handle: a scalar, a string, a byte array, or an array of ints,
/// bools, strings or byte arrays. A map's key and value are restricted to it (#752).
fn plain_data(ty: Type) -> bool {
    match ty {
        Type::Int | Type::Bool | Type::Byte | Type::Float | Type::Char | Type::Rat | Type::Str | Type::Bytes => true,
        Type::Array(shape) => {
            matches!(shape.leaf, ArrayLeaf::Int | ArrayLeaf::Bool | ArrayLeaf::Str | ArrayLeaf::Bytes)
        }
        _ => false,
    }
}

fn handle_prefix(module: &str) -> String {
    format!("{}.", module.replace('/', "."))
}

/// The SPB 17 type of a table type; handles name a handle object of the operation's module.
fn abi_type(ty: &abi::Type, prefix: &str, handles: &BTreeMap<String, u32>) -> Option<Type> {
    Some(match ty {
        abi::Type::Int => Type::Int,
        abi::Type::Float => Type::Float,
        abi::Type::Rat => Type::Rat,
        abi::Type::Bool => Type::Bool,
        abi::Type::Byte => Type::Byte,
        abi::Type::Char => Type::Char,
        abi::Type::Str => Type::Str,
        abi::Type::Handle(name) => Type::Handle(*handles.get(&format!("{prefix}{name}"))?),
        abi::Type::Array(item) if **item == abi::Type::Byte => Type::Bytes,
        abi::Type::Array(item) => {
            let leaf = match abi_type(item, prefix, handles)? {
                Type::Int => ArrayLeaf::Int,
                Type::Bool => ArrayLeaf::Bool,
                Type::Float => ArrayLeaf::Float,
                Type::Str => ArrayLeaf::Str,
                Type::Bytes => ArrayLeaf::Bytes,
                // An array of the module's handles (`@JsonNodeState`, #751) is plain data, as a handle is.
                Type::Handle(id) => ArrayLeaf::Handle(id),
                Type::Array(shape) => {
                    let depth = shape.depth.checked_add(1)?;
                    return Some(Type::Array(ArrayType { depth, ..shape }));
                }
                _ => return None,
            };
            Type::Array(ArrayType { leaf, depth: 1 })
        }
        abi::Type::Map(..) | abi::Type::Any | abi::Type::Void => return None,
    })
}

impl ExternalContracts for P1Contracts {
    fn entry(&self, name: &str) -> Option<&Function> {
        self.core.iter().find(|f| f.proofs.entry.as_ref().and_then(|entry| entry.external.as_deref()) == Some(name))
            .or_else(|| self.natives.get(name))
    }

    fn adapter(&self, declared: &Function, composites: &[Composite]) -> Option<Function> {
        P1Contracts::adapter(self, declared, composites)
    }
}

fn entry(name: &str, registers: Vec<Type>, results: Vec<Type>, owned: bool, failures: u32) -> Function {
    let mut params = (0..registers.len() as u32).map(Location::Register).collect::<Vec<_>>();
    if owned {
        params.push(Location::Service(0));
    }
    let deps = registers.iter().filter(|ty| matches!(ty, Type::Object(_) | Type::Error)).count();
    let mut services = vec![ServiceKind::Deps; deps];
    // Proof parameters precede the owning packet, as in elaborated OIR signatures.
    if owned {
        params.pop();
    }
    params.extend((0..deps as u32).map(Location::Service));
    if owned {
        params.push(Location::Service(services.len() as u32));
        services.push(ServiceKind::Packet);
    }
    Function {
        symbol: name.into(), entry_abi: if owned { EntryAbi::Owned } else { EntryAbi::Observation },
        registers, services, params, results,
        blocks: vec![], code: vec![],
        proofs: FunctionProofs {
            entry: Some(EntryContract { external: Some(name.into()), failures, ..EntryContract::default() }),
            ..FunctionProofs::default()
        },
    }
}
