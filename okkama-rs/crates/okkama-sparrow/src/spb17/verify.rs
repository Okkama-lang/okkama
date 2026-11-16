//! Structural, type, SSA/CFG and registry result-schema verification.
//! Executable artifacts additionally pass verify_load: service lifetimes, cleanup phases, order and roots.
use std::collections::{BTreeMap, BTreeSet};

use super::*;

type Check = Result<(), Error>;

/// An object field has type never.
const NEVER_FIELD: &str = "NEVER_FIELD";
/// A data or Champ instruction reads a value of type never.
const NEVER_OPERAND: &str = "NEVER_OPERAND";
/// A collection of never leaves is retagged to another shape, or a key-less map to another value type.
const NEVER_RETAG: &str = "NEVER_RETAG";
/// The admission filter of a scope.transfer is anything but nothing or a single flag right after its selection
/// (`true`: admitted identities; `false`: admitted or bound ones, #697).
const TRANSFER_ADMISSION: &str = "TRANSFER_ADMISSION";
/// `array.order` of anything but an array of int, float, rat, char or str or a byte array, or into another type.
const ARRAY_ORDER_TYPE: &str = "ARRAY_ORDER_TYPE";
/// `array.order.dispatch` sorts an array whose elements its dispatch cannot compare: the slot is not a
/// synchronous (observation or owned) slot taking one element and returning a plain three-case enum
/// (`compare.Order`), the elements are not the dispatch's receivers, a comparison may retain an element, or
/// the site carries services.
const ORDER_DISPATCH: &str = "ORDER_DISPATCH";

fn require(condition: bool, message: &str) -> Check {
    if condition { Ok(()) } else { Err(error(message)) }
}

fn at<T>(items: &[T], index: u32) -> Result<&T, Error> {
    items.get(index as usize).ok_or_else(|| error(format!("invalid table/location index {index}")))
}

/// The role declaration that `id` names (`ROLE_TYPE`).
pub(crate) fn role_decl(p: &Program, id: u32) -> Result<&RoleDecl, Error> {
    p.objects.get(id as usize).and_then(|object| object.role.as_ref())
        .ok_or_else(|| violation("ROLE_TYPE", "role type names a declaration that is not a role"))
}

/// The objects that implement `role`, each with its implementation table.
pub(crate) fn implementors(p: &Program, role: u32) -> impl Iterator<Item = (u32, &RoleImpl)> {
    p.objects.iter().enumerate().flat_map(move |(id, object)| {
        object.implements.iter().filter(move |table| table.role == role).map(move |table| (id as u32, table))
    })
}

pub(crate) fn implements(p: &Program, object: u32, role: u32) -> bool {
    p.objects.get(object as usize).is_some_and(|object| object.implements.iter().any(|table| table.role == role))
}

/// A role value is managed when some implementation of the role may reach a resource.
pub fn role_reaches_resource(p: &Program, role: u32) -> bool {
    implementors(p, role).any(|(id, _)| p.objects[id as usize].layout.may_reach_resource)
}

/// An `any` value is managed when some any box may reach a resource.
pub fn any_reaches_resource(p: &Program) -> bool {
    p.objects.iter().any(|object| object.any_box && object.layout.may_reach_resource)
}

/// The function of `slot` in the implementation table of `object` for `role`.
pub fn implementation(p: &Program, object: u32, role: u32, slot: u32) -> Option<u32> {
    let table = p.objects.get(object as usize)?.implements.iter().find(|table| table.role == role)?;
    table.slots.get(slot as usize).copied()
}

pub(crate) fn ty_valid(ty: Type, p: &Program) -> Check {
    match ty {
        // A hidden runtime word is a storage type only; the object storage check admits it (#601 Q1 audit A1).
        Type::RuntimeCell => {
            return Err(violation(RUNTIME_CELL, "a runtime cell is object storage only, never a value type"));
        }
        Type::Object(id) | Type::Successor(id) => {
            enforce(!at(&p.objects, id)?.handle, "HANDLE_TYPE", "an object type names a native handle declaration")?;
            enforce(at(&p.objects, id)?.role.is_none(), "ROLE_TYPE", "object type names a role declaration")?;
        }
        Type::Role(id) | Type::RoleSuccessor(id) => {
            role_decl(p, id)?;
        }
        Type::Handle(id) => {
            enforce(p.objects.get(id as usize).is_some_and(|object| object.handle),
                "HANDLE_TYPE", "a handle type names no native handle declaration")?;
        }
        Type::Variant(id) => {
            enforce(p.objects.get(id as usize).is_some_and(|object| !object.variant_cases.is_empty()),
                "VARIANT_TYPE", "variant type names a declaration without variant cases")?;
        }
        Type::Array(shape) => {
            enforce(shape.depth > 0, "ARRAY_SHAPE", "array depth must be positive")?;
            let element = ArrayType { depth: 1, ..shape }.element()
                .ok_or_else(|| violation("ARRAY_SHAPE", "array without element"))?;
            if let Type::Object(id) | Type::Variant(id) | Type::Map(id) = element {
                enforce(match element {
                    Type::Object(_) => (id as usize) < p.objects.len(),
                    Type::Variant(_) => {
                        p.objects.get(id as usize).is_some_and(|object| !object.variant_cases.is_empty())
                    }
                    _ => matches!(p.composites.get(id as usize), Some(Composite::Map { .. })),
                }, "ARRAY_ELEMENT", "unknown array element declaration")?;
            }
            if let Type::Object(id) = element {
                enforce(!p.objects[id as usize].handle, "HANDLE_TYPE",
                    "an array element names a native handle declaration")?;
            }
            if let Type::Role(id) = element {
                role_decl(p, id)?;
            }
            // An array of native handles (#751) names a native handle declaration, as a handle type does; an array
            // of Futures (#758) names a `future_results` entry, as a Future type does.
            if let Type::Handle(_) | Type::Future(_) = element {
                ty_valid(element, p)?;
            }
            // An object with an async destroy is rejected for every use outside P1, arrays included.
            enforce(collection_leaf(p, element), "ARRAY_PLAIN", "array element graph is not closed plain data")?;
        }
        Type::Map(id) | Type::MapBuilder(id) => {
            enforce(matches!(p.composites.get(id as usize), Some(Composite::Map { .. })),
                "COMPOSITE_TYPE", "map type names no map composite")?;
        }
        Type::Builder(id) => {
            enforce(matches!(p.composites.get(id as usize), Some(Composite::Builder { .. })),
                "COMPOSITE_TYPE", "builder type names no builder composite")?;
        }
        Type::Future(id) => {
            enforce((id as usize) < p.tables.future_results.len(), FUTURE_TYPES,
                "a Future type names no future_results entry")?;
        }
        _ => {}
    }
    Ok(())
}

/// `Type::Future` indexes distinct `future_results`; await, fut.new and fut.publish agree with them; object
/// storage holds a Future, or an array of them, only in a selected field of an object that reaches what its packet
/// carries (D25.1, #771, #758).
pub(crate) const FUTURE_TYPES: &str = "SPB_FUTURE_TYPES";
/// A runtime cell (`Type::RuntimeCell`) is a storage word of an object with source fields, which no source field
/// names and no selection walks; it is no value type (#601 Q1 audit A1).
pub(crate) const RUNTIME_CELL: &str = "SPB_RUNTIME_CELL";

/// The leaf of a collection (an array element or a map value): a resource object, which a managed collection
/// refers to without owning it (own keeps its obligation with its owner), or closed plain data. An object that
/// declares neither a destroy nor a resource is plain only if its whole graph is (`ARRAY_PLAIN`, `MAP_PLAIN`).
/// A role or `any` leaf refers to an implementing object or an any box: the collection is managed when one of
/// them may reach a resource (#711), and plain data otherwise.
/// A storage type that holds Futures directly: a Future, or an array whose leaf is a Future (D25.2, #771, #758).
pub(crate) fn future_storage(ty: Type) -> bool {
    matches!(ty, Type::Future(_) | Type::Array(ArrayType { leaf: ArrayLeaf::Future(_), .. }))
}

fn collection_leaf(p: &Program, leaf: Type) -> bool {
    let resource = match leaf {
        Type::Object(id) => p.objects.get(id as usize)
            .is_some_and(|object| object.layout.has_destroy || object.layout.may_reach_resource),
        Type::Role(id) => role_reaches_resource(p, id),
        Type::Any => any_reaches_resource(p),
        // An array of Futures is a packet whose elements carry service entries (D25.2 revision, #758).
        Type::Future(_) => true,
        _ => false,
    };
    resource || closed_plain(p, leaf)
}

/// Whether a value of `ty` may refer to an identity that reaches a resource: a resource object or successor, a
/// role value one of whose implementations does, an `any` value one of whose boxes does, or a managed array or
/// map of them. Plain collections never do (`ARRAY_PLAIN`, `MAP_PLAIN`).
pub(crate) fn reaches_resource(p: &Program, ty: Type) -> bool {
    let leaf = match ty {
        Type::Array(shape) => ArrayType { depth: 1, ..shape }.element(),
        Type::Variant(id) => return p.objects.get(id as usize).is_some_and(|object| object.layout.may_reach_resource),
        Type::Map(id) => match p.composites.get(id as usize) {
            Some(Composite::Map { value, .. }) => Some(*value),
            _ => None,
        },
        ty => Some(ty),
    };
    match leaf {
        Some(Type::Object(id) | Type::Successor(id)) => {
            p.objects.get(id as usize).is_some_and(|object| object.layout.may_reach_resource)
        }
        Some(Type::Role(id) | Type::RoleSuccessor(id)) => role_reaches_resource(p, id),
        Some(Type::Any) => any_reaches_resource(p),
        _ => false,
    }
}

/// Whether every value of `ty` is closed plain data: no identity with destroy, no resource, no service
/// and no unique builder anywhere in its graph. Unknown declarations are not plain.
pub(crate) fn closed_plain(p: &Program, ty: Type) -> bool {
    plain_graph(p, ty, true)
}

/// Whether values of `ty` compare structurally (`val.eq`, `test.is`, `array.difference`): closed plain data
/// without a native handle, which has no structural value.
pub(crate) fn comparable(p: &Program, ty: Type) -> bool {
    plain_graph(p, ty, false)
}

fn plain_graph(p: &Program, ty: Type, handles: bool) -> bool {
    let mut pending = vec![ty];
    let mut seen = std::collections::HashSet::new();
    while let Some(ty) = pending.pop() {
        if !seen.insert(ty) {
            continue;
        }
        match ty {
            Type::Int | Type::Bool | Type::Str | Type::Range | Type::Bytes | Type::Byte | Type::Float
                | Type::Char | Type::Rat | Type::Never => {}
            // An `any` value is plain when every any box is.
            Type::Any => {
                pending.extend(p.objects.iter().enumerate().filter(|(_, object)| object.any_box)
                    .map(|(id, _)| Type::Object(id as u32)));
            }
            // A role value is plain when every implementation of the role is.
            Type::Role(id) => {
                if role_decl(p, id).is_err() {
                    return false;
                }
                pending.extend(implementors(p, id).map(|(object, _)| Type::Object(object)));
            }
            Type::Object(id) | Type::Variant(id) => {
                let Some(object) = p.objects.get(id as usize) else { return false };
                if object.handle || object.layout.has_destroy || object.layout.may_reach_resource
                    || object.layout.may_async_destroy
                {
                    return false;
                }
                // A runtime cell is the runtime's word, outside the value: equality reads the source fields.
                pending.extend(object.layout.storage_fields.iter().filter(|&&ty| ty != Type::RuntimeCell));
                pending.extend(object.variant_cases.iter().map(|&case| Type::Object(case)));
            }
            // A native handle is the native registry's: it has no Okkama destroy and holds no ownership graph
            // (P2.4 §4.7), so an object that keeps one (a regex match keeps its match state) stays plain data.
            Type::Handle(id) => {
                if !handles || !p.objects.get(id as usize).is_some_and(|object| object.handle) {
                    return false;
                }
            }
            // Nested arrays share their leaf: check it once, whatever the depth.
            Type::Array(shape) if shape.depth > 0 => {
                pending.extend(ArrayType { depth: 1, ..shape }.element());
            }
            Type::Map(id) => match p.composites.get(id as usize) {
                Some(Composite::Map { key, value }) => pending.extend([*key, *value]),
                _ => return false,
            },
            _ => return false,
        }
    }
    true
}

/// A map key: a scalar, string, rat, enum or key object (no destroy, no resource, every source field a key,
/// not containing itself), the frontend's `object_key_problem` rule (OIR-2 §3.1).
pub(crate) fn key_type(p: &Program, ty: Type) -> bool {
    fn key(p: &Program, ty: Type, open: &mut Vec<u32>) -> bool {
        match ty {
            // A `never` key belongs to an uninferred empty map: no key value exists.
            Type::Int | Type::Bool | Type::Byte | Type::Char | Type::Float | Type::Rat | Type::Str | Type::Never => {
                true
            }
            Type::Object(id) => {
                let Some(object) = p.objects.get(id as usize) else { return false };
                if open.contains(&id) || object.layout.has_destroy || object.layout.may_reach_resource
                    || !object.variant_cases.is_empty()
                {
                    return false;
                }
                if object.fields.is_empty() {
                    // A plain enum or Error enum: its single storage word is the case number.
                    return object.layout.storage_fields == [Type::Int]
                        && (object.enum_cases > 0 || !object.error_cases.is_empty());
                }
                open.push(id);
                let fields = object.fields.iter().filter(|field| !field.once)
                    .all(|field| object.layout.storage_fields.get(field.storage as usize)
                        .is_some_and(|&field| key(p, field, open)));
                open.pop();
                fields
            }
            _ => false,
        }
    }
    key(p, ty, &mut Vec::new())
}

/// A multi-case Error variant: a variant whose every case is a plain Error case object. Its values are
/// Error values: they convert to `Error`, are thrown and are caught by the variant's declaration.
fn error_variant(p: &Program, id: u32) -> bool {
    p.objects.get(id as usize).is_some_and(|object| !object.variant_cases.is_empty()
        && object.variant_cases.iter().all(|&case| p.objects.get(case as usize)
            .is_some_and(|case| !case.error_cases.is_empty())))
}

/// Composite references point backwards, so composite types are finite and checkable in order.
fn composite_valid(p: &Program, index: usize, composite: Composite) -> Check {
    let earlier = |ty: Type| -> bool {
        let mut pending = vec![ty];
        while let Some(ty) = pending.pop() {
            match ty {
                Type::Map(id) | Type::MapBuilder(id) | Type::Builder(id) if id as usize >= index => return false,
                Type::Array(shape) => pending.extend(ArrayType { depth: 1, ..shape }.element()),
                _ => {}
            }
        }
        true
    };
    match composite {
        Composite::Map { key, value } => {
            enforce(earlier(key) && earlier(value), "COMPOSITE_ORDER", "map composite refers forward")?;
            ty_valid(key, p)?;
            ty_valid(value, p)?;
            enforce(key_type(p, key), "MAP_KEY", "map key is not a key type")?;
            // A managed map refers to resource objects as values; like a managed array it owns none.
            enforce(collection_leaf(p, value), "MAP_PLAIN",
                "map value graph is neither plain data nor resource objects")?;
            // The leaf rule admits a Future for arrays of Futures (D25.2 revision, #758). A map selection's runtime
            // walk enters object and array values only, never a Future's frame, so a map would keep the entries of
            // its Futures in the closing scope while the static model believes the selection took them. Maps of
            // Futures wait for their own design (#800).
            enforce(!matches!(value, Type::Future(_)), "MAP_PLAIN", "map value is a Future")
        }
        Composite::Builder { array } => {
            enforce(earlier(array), "COMPOSITE_ORDER", "builder composite refers forward")?;
            enforce(matches!(array, Type::Array(_) | Type::Bytes), "BUILDER_ARRAY",
                "builder composite must finish an array")?;
            ty_valid(array, p)
        }
    }
}

pub(crate) fn loc_type(f: &Function, location: Location) -> Result<ProducedType, Error> {
    match location {
        Location::Register(id) => Ok(ProducedType::Value(*at(&f.registers, id)?)),
        Location::Service(id) => Ok(ProducedType::Service(*at(&f.services, id)?)),
    }
}

/// A decimal integer without a plus sign, leading zeros or a negative zero.
fn canonical_integer(text: &str) -> bool {
    let digits = text.strip_prefix('-').unwrap_or(text);
    !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
        && (digits == "0" || !digits.starts_with('0')) && text != "-0"
}

/// Work bound of the coprimality check of one rational constant, in digit steps.
const RAT_GCD_STEPS: usize = 1 << 22;

/// Whether two canonical non-negative decimals are coprime, by binary GCD on their digits.
/// `None` when the check would exceed `RAT_GCD_STEPS`.
fn coprime_decimals(left: &str, right: &str) -> Option<bool> {
    let mut steps = 0usize;
    let mut step = |cost: usize| {
        steps = steps.saturating_add(cost.max(1));
        steps <= RAT_GCD_STEPS
    };
    let mut left: Vec<u8> = left.bytes().map(|digit| digit - b'0').collect();
    let mut right: Vec<u8> = right.bytes().map(|digit| digit - b'0').collect();
    let even = |number: &Vec<u8>| number.last().is_some_and(|digit| digit % 2 == 0);
    if left == [0] || right == [0] {
        return Some(left == [1] || right == [1]);
    }
    if even(&left) && even(&right) {
        return Some(false);
    }
    loop {
        for number in [&mut left, &mut right] {
            while even(number) {
                if !step(number.len()) {
                    return None;
                }
                let mut carry = 0;
                for digit in number.iter_mut() {
                    let next = carry * 10 + *digit;
                    *digit = next / 2;
                    carry = next % 2;
                }
                trim_decimal(number);
            }
        }
        if !step(left.len() + right.len()) {
            return None;
        }
        let order = left.len().cmp(&right.len()).then_with(|| left.cmp(&right));
        if order == std::cmp::Ordering::Equal {
            return Some(left == [1]);
        }
        let (larger, smaller) = if order == std::cmp::Ordering::Greater {
            (&mut left, &right)
        } else {
            (&mut right, &left)
        };
        let mut borrow = 0i16;
        for (index, digit) in larger.iter_mut().rev().enumerate() {
            let other = smaller.len().checked_sub(index + 1).map_or(0, |index| smaller[index]);
            let next = i16::from(*digit) - i16::from(other) - borrow;
            borrow = i16::from(next < 0);
            *digit = (next + borrow * 10) as u8;
        }
        trim_decimal(larger);
    }
}

fn trim_decimal(number: &mut Vec<u8>) {
    match number.iter().position(|&digit| digit != 0) {
        Some(first) => {
            number.drain(..first);
        }
        None => *number = vec![0],
    }
}

fn constant_type(value: &Constant) -> Result<Type, Error> {
    match value {
        Constant::Int(text) => {
            require(canonical_integer(text), "non-canonical integer")?;
            Ok(Type::Int)
        }
        Constant::Rat(RatValue { numerator, denominator }) => {
            let terms = canonical_integer(numerator) && canonical_integer(denominator)
                && !denominator.starts_with('-') && denominator != "0";
            enforce(terms, "CONST_RAT", "rat constant needs canonical integers and a positive denominator")?;
            let magnitude = numerator.strip_prefix('-').unwrap_or(numerator);
            let lowest = coprime_decimals(magnitude, denominator)
                .ok_or_else(|| violation("LOAD_WORK_BUDGET", "rat constant exceeds the load work budget"))?;
            enforce(lowest, "CONST_RAT", "rat constant is not in lowest terms")?;
            Ok(Type::Rat)
        }
        Constant::Bool(_) => Ok(Type::Bool),
        Constant::Descriptor(_) => Ok(Type::Descriptor),
        Constant::Str(_) => Ok(Type::Str),
        Constant::Byte(value) => {
            enforce(*value <= 255, "CONST_BYTE", "byte constant outside 0..255")?;
            Ok(Type::Byte)
        }
        Constant::Char(value) => {
            enforce(char::from_u32(*value).is_some(), "CONST_CHAR", "char constant is not a Unicode scalar value")?;
            Ok(Type::Char)
        }
        Constant::Float(bits) => {
            // Floats are always finite in Okkama; every other source of a float checks it at run time.
            let canonical = bits.parse::<u64>()
                .is_ok_and(|value| value.to_string() == *bits && f64::from_bits(value).is_finite());
            enforce(canonical, "CONST_FLOAT", "float constant is not a canonical finite binary64 bit pattern")?;
            Ok(Type::Float)
        }
    }
}

/// Module constants (#709): each is closed plain data (`GLOBAL_TYPE`) whose data build exactly its type
/// (`GLOBAL_DATA`): every datum is a constant, an enum case, an array or a map of its declared type, made of
/// earlier data of its element, key and value types, and the last datum is the constant. The runtime
/// builds the data in order before the program runs, so a value is never read as another type.
fn check_globals(p: &Program) -> Check {
    for global in &p.globals {
        ty_valid(global.ty, p)?;
        enforce(closed_plain(p, global.ty), "GLOBAL_TYPE", "a module constant must be closed plain data")?;
        enforce(global.data.last().map(|datum| datum.ty) == Some(global.ty), "GLOBAL_TYPE",
            "the last datum of a module constant must have the constant's type")?;
        for (index, datum) in global.data.iter().enumerate() {
            ty_valid(datum.ty, p)?;
            let parts = |items: &[u32]| -> Result<Vec<Type>, Error> {
                items.iter().map(|&item| {
                    enforce((item as usize) < index, "GLOBAL_DATA", "a datum names a datum that does not precede it")?;
                    Ok(global.data[item as usize].ty)
                }).collect()
            };
            let valid = match &datum.value {
                DatumValue::Constant(Constant::Descriptor(_)) => false,
                DatumValue::Constant(value) => constant_type(value)? == datum.ty,
                DatumValue::Case(case) => match datum.ty {
                    Type::Object(id) => p.objects.get(id as usize).is_some_and(|object| {
                        object.fields.is_empty() && object.layout.storage_fields == [Type::Int]
                            && !object.layout.may_reach_resource
                            && ((*case as usize) < object.error_cases.len() || *case < object.enum_cases)
                    }),
                    _ => false,
                },
                DatumValue::Array(items) => {
                    let element = element_type(datum.ty);
                    element.is_some() && parts(items)?.into_iter().all(|ty| Some(ty) == element)
                }
                DatumValue::Map(items) => match datum.ty {
                    Type::Map(id) => match p.composites.get(id as usize) {
                        Some(&Composite::Map { key, value }) => items.len().is_multiple_of(2)
                            && parts(items)?.chunks(2).all(|pair| pair == [key, value]),
                        _ => false,
                    },
                    _ => false,
                },
            };
            enforce(valid, "GLOBAL_DATA", "a datum does not build a value of its declared type")?;
        }
    }
    Ok(())
}

pub fn verify(p: &Program) -> Check {
    if let Some(entry) = p.entry {
        at(&p.functions, entry)?;
    }
    let mut results = std::collections::HashSet::new();
    for &result in &p.tables.future_results {
        if let Some(ty) = result {
            ty_valid(ty, p)?;
            enforce(!matches!(ty, Type::Builder(_) | Type::MapBuilder(_)), "BUILDER_LINEAR",
                "a Future cannot publish a unique builder")?;
        }
        enforce(results.insert(result), FUTURE_TYPES, "future_results entries must be distinct")?;
    }
    suspension::check_layouts(p)?;
    for (index, &composite) in p.composites.iter().enumerate() {
        composite_valid(p, index, composite)?;
    }
    check_tests(p)?;
    check_globals(p)?;
    let mut handles = BTreeSet::new();
    for object in &p.objects {
        // An Atomic over a role value holds a reference the runtime never releases or destroys: every
        // implementation of the role must be closed plain data, by the witness tables (P2.4 §4.8, #764).
        if object.name.starts_with("async.atomic.Atomic_role_")
            && let Some(&Type::Role(role)) = object.layout.storage_fields.first() {
            enforce(closed_plain(p, Type::Role(role)), "ATOMIC_ROLE",
                "an Atomic holds a role whose implementations are not closed plain data")?;
        }
        // An Atomic over other data (#778: maps, objects, variants, arrays of handles or roles) holds it only as
        // closed plain data, by its whole graph.
        if object.name.starts_with("async.atomic.Atomic_t_")
            && let Some(&value) = object.layout.storage_fields.first() {
            enforce(closed_plain(p, value), "ATOMIC_PLAIN", "an Atomic holds a value that is not closed plain data")?;
        }
        // Natives name their handles by declaring module and name: one name, one declaration.
        enforce(!object.handle || handles.insert(object.name.as_str()), "HANDLE_NAME",
            "two native handle declarations share one name")?;
        let layout = &object.layout;
        let count = layout.storage_fields.len() as u64;
        let words = (1 + count + 4 * u64::from(layout.has_destroy)).max(2);
        require(u64::from(layout.allocation_words) == words, "invalid object allocation_words")?;
        let identity = if layout.has_destroy { Some(8 * (1 + count)) } else { None };
        require(layout.identity_offset.map(u64::from) == identity, "invalid object identity_offset")?;
        require(!layout.may_async_destroy, "async destroy is outside P1")?;
        require(!layout.has_destroy || layout.may_reach_resource, "destroy layout must reach self")?;
        let unique = |indices: &[u32]| -> Check {
            let mut seen = BTreeSet::new();
            for &index in indices {
                at(&layout.storage_fields, index)?;
                require(seen.insert(index), "duplicate object storage index")?;
            }
            Ok(())
        };
        if object.handle {
            let plain = object.fields.is_empty() && object.error_cases.is_empty() && object.enum_cases == 0
                && object.variant_cases.is_empty() && layout.storage_fields.is_empty() && !layout.has_destroy
                && !layout.may_reach_resource && layout.trace_layout.is_empty() && layout.selection_layout.is_empty();
            enforce(plain, "HANDLE_DECLARATION", "a native handle declaration has fields, cases or resources")?;
        }
        // Case texts belong to a plain enum or a plain variant only, one per case; either may also carry none
        // (a variant names its cases for the text of an Error payload, #763).
        enforce(object.enum_names.is_empty()
            || (object.enum_cases > 0 && object.enum_names.len() == object.enum_cases as usize)
            || (object.error_cases.is_empty() && !object.variant_cases.is_empty()
                && object.enum_names.len() == object.variant_cases.len()),
            "ENUM_NAMES", "enum case names belong to a plain enum or variant, one per case")?;
        if !object.variant_cases.is_empty() {
            // A case object is never an identity, but it may refer to resources (Maybe<Resource>). The
            // variant declares that it reaches resources exactly when one of its cases does.
            let mut seen = BTreeSet::new();
            let cases = object.variant_cases.iter().all(|&case| seen.insert(case) && p.objects.get(case as usize)
                .is_some_and(|case| case.variant_cases.is_empty() && !case.layout.has_destroy && !case.handle));
            let reaches = object.variant_cases.iter().any(|&case| {
                p.objects.get(case as usize).is_some_and(|case| case.layout.may_reach_resource)
            });
            let declaration = object.fields.is_empty() && layout.storage_fields.is_empty()
                && layout.selection_layout.is_empty() && layout.may_reach_resource == reaches;
            enforce(cases && declaration, "VARIANT_CASES",
                "variant cases must be distinct case objects without identity, its retention their union")?;
        }
        unique(&object.fields.iter().map(|field| field.storage).collect::<Vec<_>>())?;
        unique(&layout.trace_layout)?;
        unique(&layout.selection_layout)?;
        for (index, &ty) in layout.storage_fields.iter().enumerate() {
            if ty == Type::RuntimeCell {
                // Equality and keys read an object's source fields; a runtime cell is none of them, and an object
                // without source fields would compare its storage words (#601 Q1 audit A1).
                let source = object.fields.iter().any(|field| field.storage == index as u32);
                enforce(!source && !object.fields.is_empty() && !layout.selection_layout.contains(&(index as u32))
                    && !object.any_box && object.role.is_none() && !object.handle && object.error_cases.is_empty()
                    && object.enum_cases == 0 && object.variant_cases.is_empty(),
                    RUNTIME_CELL, "a runtime cell is an unselected hidden word of an object with source fields")?;
                require(layout.trace_layout.contains(&(index as u32)), "incorrect object trace layout")?;
                continue;
            }
            ty_valid(ty, p)?;
            // D25.1 (#771, #758): the service entry of a Future field, or of each element of a Future array field,
            // travels with the packet of the value that holds it, so the field is selected (the runtime selection
            // walk reaches the holder through it) and the object reaches what a packet carries. Where the entry
            // comes from is the ownership verifier's proof (coro's absorb).
            if future_storage(ty) {
                enforce(layout.may_reach_resource && layout.selection_layout.contains(&(index as u32))
                    && !object.any_box && object.role.is_none() && !object.handle && object.error_cases.is_empty(),
                    FUTURE_TYPES, "a Future field is selected in an object that reaches what its packet carries")?;
            }
            enforce(!matches!(ty, Type::Builder(_) | Type::MapBuilder(_)), "BUILDER_LINEAR",
                "object storage cannot hold a unique builder")?;
            // No value of type never exists: a field of it would be read as a forged word.
            enforce(ty != Type::Never, NEVER_FIELD, "object storage cannot have type never")?;
            // Retention flags are checked object by object, so a managed collection's leaf flag can be trusted.
            if let Type::Object(id) | Type::Successor(id) = ty {
                enforce(layout.may_reach_resource || !p.objects[id as usize].layout.may_reach_resource,
                    "OBJECT_RETENTION", "understated object retention metadata")?;
            }
            if let Type::Role(id) | Type::RoleSuccessor(id) = ty {
                enforce(layout.may_reach_resource || !role_reaches_resource(p, id),
                    "OBJECT_RETENTION", "understated object retention metadata")?;
            }
            if ty == Type::Any {
                enforce(layout.may_reach_resource || !any_reaches_resource(p),
                    "OBJECT_RETENTION", "understated object retention metadata")?;
            }
            // The runtime selection walk reaches identities only through selected fields: a field that may refer
            // to a resource, directly or through a managed collection or a variant, is selected. An any box states
            // its selection exactly under ANY_BOX (selected if and only if its value reaches a resource).
            if reaches_resource(p, ty) && !object.any_box {
                enforce(layout.may_reach_resource && layout.selection_layout.contains(&(index as u32)),
                    "COLLECTION_RETENTION", "a field holding resource objects must be selected and reach resources")?;
            }
            require(layout.trace_layout.contains(&(index as u32)) == ty.traced(), "incorrect object trace layout")?;
        }
        if let Some(role) = &object.role {
            check_role_decl(p, object, role)?;
        }
        if object.any_box {
            check_any_box(p, object)?;
        }
    }
    // One any box per boxed type: the descriptor of a box names exactly one value type.
    let mut boxed = std::collections::HashSet::new();
    for object in p.objects.iter().filter(|object| object.any_box) {
        enforce(boxed.insert(object.layout.storage_fields[0]), "ANY_BOX", "two any boxes hold one value type")?;
    }
    // Implementation tables after every role declaration is known to be well formed.
    for (id, object) in p.objects.iter().enumerate() {
        check_role_impls(p, id as u32, object)?;
    }
    for (index, scope) in p.tables.scopes.iter().enumerate() {
        at(&p.functions, scope.function)?;
        require(at(&p.tables.boundaries, scope.boundary)?.scope == index as u32, "scope boundary mismatch")?;
        if let Some(parent) = scope.parent {
            require((parent as usize) < index, "scope parent must precede child")?;
            require(at(&p.tables.scopes, parent)?.function == scope.function, "scope crosses function")?;
        }
    }
    for boundary in &p.tables.boundaries {
        at(&p.tables.scopes, boundary.scope)?;
    }
    for summary in &p.tables.summaries {
        let entry = at(&p.functions, summary.entry)?;
        for &param in &summary.retained_params {
            at(&entry.params, param)?;
        }
    }
    for f in &p.functions {
        verify_function(p, f).map_err(|e| Error { message: format!("{}: {}", f.symbol, e.message), ..e })?;
    }
    Ok(())
}

/// A role declaration is a placeholder descriptor: no storage, fields, cases, destroy or implementations.
/// Its slots are synchronous plain or owning entries with valid types and at most one result.
fn check_role_decl(p: &Program, object: &Object, role: &RoleDecl) -> Check {
    let layout = &object.layout;
    enforce(object.fields.is_empty() && layout.storage_fields.is_empty() && !layout.has_destroy
        && !layout.may_reach_resource && object.error_cases.is_empty() && object.enum_cases == 0
        && object.variant_cases.is_empty() && object.implements.is_empty() && !object.handle,
        "ROLE_DECL", "a role declaration has no storage, cases, destroy or implementations")?;
    for slot in &role.slots {
        enforce(matches!(slot.entry_abi, EntryAbi::Observation | EntryAbi::Owned) && slot.results.len() <= 1,
            "ROLE_DECL", "a role slot is a plain or owning entry with at most one result")?;
        for &ty in slot.params.iter().chain(&slot.results) {
            ty_valid(ty, p)?;
        }
    }
    Ok(())
}

/// An any box holds one value in its one field: no destroy, identity, cases, role or implementations. The value
/// is not itself `any`; the box is traced and selected exactly as its value is, and reaches what it reaches.
fn check_any_box(p: &Program, object: &Object) -> Check {
    let layout = &object.layout;
    let [value] = layout.storage_fields.as_slice() else {
        return Err(violation("ANY_BOX", "an any box holds exactly one value"));
    };
    let reaches = reaches_resource(p, *value) || match *value {
        Type::Role(id) | Type::RoleSuccessor(id) => role_reaches_resource(p, id),
        Type::Error => p.objects.iter()
            .any(|object| !object.error_cases.is_empty() && object.layout.may_reach_resource),
        _ => false,
    };
    let selection: &[u32] = if reaches { &[0] } else { &[] };
    let linear = matches!(value, Type::Builder(_) | Type::MapBuilder(_) | Type::Future(_));
    let declaration = *value != Type::Any && !linear && object.fields.len() == 1 && object.fields[0].storage == 0
        && !layout.has_destroy
        && object.error_cases.is_empty() && object.enum_cases == 0 && object.variant_cases.is_empty()
        && object.role.is_none() && object.implements.is_empty() && !object.handle
        && layout.selection_layout == selection && layout.may_reach_resource == reaches;
    enforce(declaration, "ANY_BOX", "an any box holds one value with its retention and nothing else")
}

/// Every implementation table names a role once, has a function for each of its slots, and each
/// function has the slot's entry ABI, its parameters after the implementing object and its results.
fn check_role_impls(p: &Program, id: u32, object: &Object) -> Check {
    let mut seen = BTreeSet::new();
    for table in &object.implements {
        let role = role_decl(p, table.role)?;
        enforce(seen.insert(table.role), "ROLE_IMPL", "an object implements a role once")?;
        enforce(table.slots.len() == role.slots.len(), "ROLE_IMPL", "implementation table misses or adds slots")?;
        for (&function, slot) in table.slots.iter().zip(&role.slots) {
            let f = p.functions.get(function as usize)
                .ok_or_else(|| violation("ROLE_IMPL", "implementation names no function"))?;
            let language = f.params.iter().map(|&location| loc_type(f, location))
                .collect::<Result<Vec<_>, _>>()?.into_iter()
                .filter_map(|ty| match ty { ProducedType::Value(ty) => Some(ty), _ => None }).collect::<Vec<_>>();
            let mut expected = vec![Type::Object(id)];
            expected.extend(&slot.params);
            enforce(f.entry_abi == slot.entry_abi && language == expected && f.results == slot.results
                && f.proofs.entry.as_ref().is_some_and(|entry| entry.destroy.is_none()),
                "ROLE_IMPL", &format!("{}: implementation differs from role slot {}", f.symbol, slot.name))?;
        }
    }
    Ok(())
}

/// The tests table of the test profile: unique names, distinct `() -> void` observation bodies that are
/// not the program entry. Test intrinsics belong to the test profile only.
fn check_tests(p: &Program) -> Check {
    for f in &p.functions {
        let name = f.proofs.entry.as_ref().and_then(|entry| entry.external.as_deref());
        enforce(p.tests.is_some() || !name.is_some_and(|name| name.starts_with(TEST_INTRINSIC_PREFIX)),
            "TEST_PROFILE", "test intrinsics need a tests table")?;
    }
    let Some(tests) = &p.tests else {
        return Ok(());
    };
    let mut names = BTreeSet::new();
    let mut functions = BTreeSet::new();
    for test in tests {
        enforce(!test.name.is_empty() && names.insert(test.name.as_str()), "TEST_TABLE",
            "test names must be non-empty and unique")?;
        let f = p.functions.get(test.function as usize)
            .ok_or_else(|| violation("TEST_TABLE", "test function index out of range"))?;
        enforce(functions.insert(test.function), "TEST_TABLE", "two tests share one function")?;
        enforce(p.entry != Some(test.function), "TEST_ENTRY", "the program entry cannot be a test")?;
        let external = f.proofs.entry.as_ref().is_some_and(|entry| entry.external.is_some());
        // A test is a synchronous `() -> void` observation or a void async body (#725): an async body takes only
        // its frame (`check_entry`), and as a fixed root its start holds nothing (`check_layouts`).
        let observation = f.entry_abi == EntryAbi::Observation && f.params.is_empty();
        let asynchronous = matches!(f.entry_abi, EntryAbi::AsyncBody { .. });
        enforce((observation || asynchronous) && f.results.is_empty() && !external && !f.code.is_empty(),
            "TEST_ENTRY", "a test needs a () -> void observation body or a void async body")?;
    }
    Ok(())
}

fn check_proofs(p: &Program, f: &Function) -> Check {
    for &ty in f.registers.iter().chain(&f.results) {
        ty_valid(ty, p)?;
    }
    let locations = |locations: &[Location]| -> Check {
        for &location in locations {
            loc_type(f, location)?;
        }
        Ok(())
    };
    if let EntryAbi::Defer { labels } = &f.entry_abi {
        let mut seen = BTreeSet::new();
        for label in labels {
            require(seen.insert(label.label), "duplicate defer label")?;
            if let Some(ty) = label.result {
                ty_valid(ty, p)?;
            }
        }
    }
    if let Some(entry) = &f.proofs.entry {
        require(entry.failures & !15 == 0, "invalid entry failure mask")?;
        if let Some(object) = entry.destroy { at(&p.objects, object)?; }
    }
    let mut begins = BTreeSet::new();
    for close in &f.proofs.closes {
        at(&f.code, close.begin_pc)?;
        at(&p.tables.boundaries, close.boundary)?;
        at(&f.proofs.envelopes, close.envelope)?;
        require(begins.insert(close.begin_pc), "duplicate close descriptor")?;
    }
    for evidence in &f.proofs.cleanup_plans {
        at(&f.proofs.plans, evidence.plan)?;
        at(&f.proofs.closes, evidence.close)?;
        for slot in &evidence.slots {
            loc_type(f, slot.identity)?;
            loc_type(f, slot.descriptor)?;
            at(&p.functions, slot.destroy)?;
            at(&f.code, slot.active_pc)?;
            match slot.origin {
                CreationOrigin::Singleton => {}
                CreationOrigin::Create(pc) => { at(&f.code, pc)?; }
                CreationOrigin::FreshResult { pc, callee } => {
                    at(&f.code, pc)?;
                    at(&p.functions, callee)?;
                }
            }
        }
        for suffix in &evidence.suffixes {
            locations(&suffix.remaining)?;
            locations(&suffix.roots)?;
        }
        for proof in evidence.preserves.iter().chain(&evidence.joins) {
            loc_type(f, proof.value)?;
            locations(&proof.obligations)?;
        }
    }
    for dependency in &f.proofs.dependencies {
        loc_type(f, dependency.value)?;
        locations(&dependency.obligations)?;
    }
    for registration in &f.proofs.registrations {
        let labels: &[DeferLabel] = match &at(&p.functions, registration.entry)?.entry_abi {
            EntryAbi::Defer { labels } => labels,
            // An async defer entry has no replacement labels (C-6).
            EntryAbi::AsyncDefer { .. } => &[],
            _ => return Err(error("registration requires defer entry")),
        };
        let mut seen = BTreeSet::new();
        for label in &registration.labels {
            require(seen.insert(label.label), "duplicate registration label")?;
            require(labels.iter().any(|declared| declared.label == label.label), "unknown registration label")?;
            at(&p.tables.boundaries, label.boundary)?;
        }
        require(seen.len() == labels.len(), "incomplete registration map")?;
    }
    for selection in &f.proofs.selections {
        locations(&selection.locations)?;
    }
    for plan in &f.proofs.plans {
        at(&f.proofs.selections, plan.selection)?;
        at(&f.proofs.order_witnesses, plan.order_witness)?;
    }
    for witness in &f.proofs.order_witnesses {
        for &pc in &witness.creation_pcs {
            at(&f.code, pc)?;
        }
    }
    for schedule in &f.proofs.schedules {
        let mut seen = BTreeSet::new();
        for &duty in &schedule.services {
            // A Future's cache duty, or a spawned Future's task duty (D24.2).
            let kind = loc_type(f, duty)?;
            enforce(matches!(kind, ProducedType::Service(ServiceKind::CacheState | ServiceKind::TaskState)),
                suspension::SCHEDULE, "a scheduled duty is not a cache-state or a task-state")?;
            enforce(seen.insert(duty), suspension::SCHEDULE, "a schedule names one duty twice")?;
        }
    }
    let asynchronous = matches!(f.entry_abi, EntryAbi::AsyncBody { .. });
    for envelope in &f.proofs.envelopes {
        require(envelope.failures & !(15 | ENVELOPE_CANCEL) == 0, "invalid D18 failure mask")?;
        enforce(asynchronous || envelope.failures & ENVELOPE_CANCEL == 0, suspension::ASYNC_ENTRY,
            "only an async body delivers a deferred cancellation")?;
    }
    for state in &f.proofs.states {
        at(&f.code, state.pc)?;
        locations(&state.locations)?;
    }
    for root in f.proofs.roots.iter().chain(&f.proofs.post_roots) {
        at(&f.code, root.pc)?;
        locations(&root.locations)?;
    }
    Ok(())
}

pub fn result_context(p: &Program, f: &Function, op: Opcode, args: &[Argument]) -> Result<ResultContext, Error> {
    let mut ctx = ResultContext::default();
    let mut flags = Vec::new();
    for arg in args {
        match *arg {
            Argument::Evaluation(ref evaluation) => { ty_valid(evaluation.result, p)?; }
            Argument::Result(result) => {
                if let Some(ty) = result { ty_valid(ty, p)?; }
            }
            Argument::Location(location) => { loc_type(f, location)?; }
            Argument::Scope(id) => { at(&p.tables.scopes, id)?; }
            Argument::Boundary(id) => { at(&p.tables.boundaries, id)?; }
            Argument::Summary(id) => {
                let summary = at(&p.tables.summaries, id)?;
                ctx.may_fail = summary.may_fail;
                ctx.value = at(&p.functions, summary.entry)?.results.first().copied();
            }
            Argument::Selection(id) => {
                for &location in &at(&f.proofs.selections, id)?.locations {
                    enforce(matches!(loc_type(f, location)?, ProducedType::Value(_)),
                        "OPERAND_VALUE", "selection must contain language values")?;
                }
            }
            Argument::Plan(id) => { at(&f.proofs.plans, id)?; }
            Argument::Schedule(id) => { at(&f.proofs.schedules, id)?; }
            Argument::Envelope(id) => { at(&f.proofs.envelopes, id)?; }
            Argument::Entry(id) => {
                let entry = at(&p.functions, id)?;
                ctx.value = entry.results.first().copied();
                if let EntryAbi::Defer { labels } = &entry.entry_abi {
                    ctx.labels = labels.clone();
                }
            }
            Argument::Object(id) => { at(&p.objects, id)?; }
            Argument::Destination(ExitDestination::Caller) => {}
            Argument::Destination(ExitDestination::Loop(id)) => { at(&p.tables.boundaries, id)?; }
            Argument::Destination(ExitDestination::Scope(id)) => { at(&p.tables.scopes, id)?; }
            Argument::Flag(flag) => flags.push(flag),
            Argument::Registration(id) => { at(&f.proofs.registrations, id)?; }
            Argument::Dispatch(ref dispatch) => {
                let slot = at(&role_decl(p, dispatch.role)?.slots, dispatch.slot)?;
                ctx.value = slot.results.first().copied();
                ctx.may_fail = dispatch.failures != 0;
                let mut targets = BTreeSet::new();
                let arity = 1 + slot.params.len() as u32;
                enforce(dispatch.failures & !15 == 0 && !dispatch.targets.is_empty()
                    && dispatch.targets.iter().all(|&target| {
                        targets.insert(target) && implements(p, target, dispatch.role)
                    })
                    && dispatch.retained_params.iter().all(|&param| param < arity),
                    "DISPATCH_SITE", "dispatch needs distinct implementing targets and an in-range contract")?;
            }
            Argument::Binding(_) | Argument::Field(_) | Argument::Label(_) | Argument::Kind(_)
                | Argument::Continuation(_) | Argument::Protocol(_) => {}
        }
    }
    if op == Opcode::OrderDispatch {
        // A sort yields a new array of its operand's type, not the slot's result.
        ctx.value = args.iter().find_map(|arg| match *arg {
            Argument::Location(location) => match loc_type(f, location) {
                Ok(ProducedType::Value(ty)) => Some(ty),
                _ => None,
            },
            _ => None,
        });
    }
    if op.frame_bound() {
        enforce(f.entry_abi.asynchronous(), suspension::ASYNC_ENTRY,
            &format!("{} belongs to async bodies only", op.name()))?;
    }
    if matches!(op, Opcode::ScopeTransfer | Opcode::OwnStage) {
        ctx.between = flags.first().copied().ok_or_else(|| error("missing Same/Between discriminator"))?;
    }
    if op == Opcode::ScopeTransfer {
        // An admissible transfer moves only what its source admitted (OIR-2 `admissible(...)`), a held one what its
        // source admitted or bound (`held(...)`, #697); a plain transfer takes no filter.
        let filter = args.iter().skip_while(|arg| !matches!(arg, Argument::Selection(_))).skip(1)
            .collect::<Vec<_>>();
        let shape = filter.is_empty() || filter == [&Argument::Flag(true)] || filter == [&Argument::Flag(false)];
        enforce(shape, TRANSFER_ADMISSION, "a transfer's admission filter is one flag after its selection")?;
    }
    if op == Opcode::ExitDispatch {
        require(flags.len() == 3, "exit.dispatch requires reserve/active-failure/may-fail close contract")?;
        ctx.reserve = flags[0];
        ctx.active_failure = flags[1];
        ctx.may_fail = flags[2];
    }
    if matches!(op, Opcode::InvokeCallOwn | Opcode::InvokeDefer) {
        require(args.iter().any(|arg| matches!(arg, Argument::Summary(_))), "missing result contract summary")?;
    }
    if matches!(op, Opcode::InvokeDispatch | Opcode::InvokeDispatchOwn | Opcode::OrderDispatch) {
        enforce(args.iter().any(|arg| matches!(arg, Argument::Dispatch(_))), "DISPATCH_SITE", "missing dispatch")?;
    }
    if op == Opcode::ExitCommit {
        // The load gate checks this schema against the typed pending, including through closes/edges.
        ctx.value = f.results.first().copied();
        if let Some(result) = args.iter().find_map(|arg| match arg {
            Argument::Result(result) => Some(*result), _ => None,
        }) {
            ctx.value = result;
        }
    }
    check_arguments(op, args)?;
    let locations = args.iter().filter_map(|arg| match arg {
        Argument::Location(location) => Some(*location),
        _ => None,
    }).collect::<Vec<_>>();
    let input = |index: usize, expected: ServiceKind| -> Check {
        let location = locations.get(index).ok_or_else(|| error("missing service operand"))?;
        require(loc_type(f, *location)? == ProducedType::Service(expected), "wrong service operand type")
    };
    use Opcode::*;
    use ServiceKind::*;
    let value = |index: usize| -> Check {
        enforce(locations.get(index).is_some_and(|&loc| matches!(loc_type(f, loc), Ok(ProducedType::Value(_)))),
            "OPERAND_VALUE", "value operand cannot contain a service")
    };
    match op {
        Catch => {
            enforce(loc_type(f, locations[0])? == ProducedType::Value(Type::Failure),
                "CATCH_FAILURE", "catch operand must be a failure")?;
            // A plain Error, the Error role, or the system NativeFailure or Cancelled (language spec 9.5, #724).
            ctx.value = Some(match args.get(1) {
                Some(Argument::Object(id)) if !p.objects[*id as usize].error_cases.is_empty() => Type::Object(*id),
                // A multi-case Error variant catches each of its cases (descriptors of its `variant_cases`).
                Some(Argument::Object(id)) if error_variant(p, *id) => Type::Variant(*id),
                Some(Argument::Object(id)) if native_failure(p) == Some(*id) => Type::Object(*id),
                Some(Argument::Object(id)) if cancelled(p) == Some(*id) => Type::Object(*id),
                // The Error role takes a cancellation too and binds an object of the system Cancelled, as Tomato
                // does (#738), so the program must declare it.
                Some(Argument::Flag(true)) => {
                    enforce(cancelled(p).is_some(), "CATCH_TYPE",
                        "a catch of the Error role needs the system Cancelled")?;
                    Type::Error
                }
                _ => return Err(violation("CATCH_TYPE", "catch target must be a declared plain Error")),
            });
        }
        Evaluate => {
            let Some(Argument::Evaluation(evaluation)) = args.first() else {
                return Err(violation("EVALUATE_OPERATION", "missing evaluation operation"));
            };
            let types = locations.iter().map(|&location| match loc_type(f, location)? {
                ProducedType::Value(ty) => Ok(ty),
                _ => Err(violation("EVALUATE_OPERAND", "evaluation cannot consume services")),
            }).collect::<Result<Vec<_>, _>>()?;
            enforce(evaluation.op.fallible(&types, Some(evaluation.result))
                || matches!(evaluation.op, DataOp::RangeEnd | DataOp::RangeLen),
                "EVALUATE_OPERATION", "operation is outside scalar outcomes")?;
            check_data_types(p, evaluation.op, &types, Some(evaluation.result), &evaluation.immediate)?;
            ctx.value = Some(evaluation.result);
            ctx.may_fail = true;
        }
        Fail => {
            let kind = args.iter().find_map(|arg| match arg { Argument::Label(id) => Some(*id), _ => None });
            enforce(matches!(kind, Some(0..=2)), "FAIL_KIND", "unknown failure kind")?;
            let ty = loc_type(f, locations[0])?;
            let valid = if kind == Some(0) {
                matches!(ty, ProducedType::Value(Type::Object(id)) if !p.objects[id as usize].error_cases.is_empty())
                    || matches!(ty, ProducedType::Value(Type::Variant(id)) if error_variant(p, id))
                    || ty == ProducedType::Value(Type::Error)
            } else {
                ty == ProducedType::Value(Type::Str)
            };
            enforce(valid, "FAIL_OPERAND", "failure payload differs from its kind")?;
        }
        ArrayGet => {
            enforce(locations.len() == 2, "ARRAY_GET_OPERANDS", "array.get needs array and index")?;
            // A plain array yields its element; a byte array (`@byte`, its own packed representation)
            // yields a byte. The receiver's declared type is the evidence for the read the machine emits.
            let element = match loc_type(f, locations[0])? {
                ProducedType::Value(array @ (Type::Array(_) | Type::Bytes)) => element_type(array),
                _ => return Err(violation("ARRAY_GET_OPERANDS",
                    "array.get receiver must be a plain array or a byte array")),
            };
            enforce(loc_type(f, locations[1])? == ProducedType::Value(Type::Int),
                "ARRAY_GET_OPERANDS", "array.get index must be int")?;
            ctx.value = element;
            ctx.may_fail = true;
        }
        BuilderPush | BuilderAppend | BuilderRange | BuilderFinish | BuilderAbandon => {
            let ty = |index: usize| match locations.get(index).map(|&location| loc_type(f, location)) {
                Some(Ok(ProducedType::Value(ty))) => Some(ty),
                _ => None,
            };
            let buffer = ty(0);
            let packet = locations.last().map(|&location| loc_type(f, location));
            let (finished, element) = match buffer {
                Some(Type::Builder(id)) => match p.composites.get(id as usize) {
                    Some(&Composite::Builder { array }) => (Some(array), element_type(array).map(|item| (None, item))),
                    _ => (None, None),
                },
                Some(Type::MapBuilder(id)) => match p.composites.get(id as usize) {
                    Some(&Composite::Map { key, value }) => (Some(Type::Map(id)), Some((Some(key), value))),
                    _ => (None, None),
                },
                _ => (None, None),
            };
            let operands = match (op, element) {
                (BuilderPush, Some((None, item))) => locations.len() == 3 && ty(1) == Some(item),
                (BuilderPush, Some((Some(key), value))) => locations.len() == 4 && ty(1) == Some(key)
                    && ty(2) == Some(value),
                (BuilderAppend, Some(_)) => locations.len() == 3 && ty(1) == finished,
                (BuilderRange, Some((None, Type::Int))) => {
                    locations.len() == 4 && ty(1) == Some(Type::Int) && ty(2) == Some(Type::Int)
                }
                (BuilderFinish | BuilderAbandon, Some(_)) => locations.len() == 2,
                _ => false,
            };
            enforce(operands && packet == Some(Ok(ProducedType::Service(Packet))), "BUILDER_OPERANDS",
                "builder transition operands differ from its buffer, elements and journal packet")?;
            ctx.buffer = buffer;
            ctx.value = finished;
            ctx.may_fail = op != BuilderAbandon;
        }
        Lookup => {
            let types = locations.iter().map(|&location| loc_type(f, location)).collect::<Result<Vec<_>, _>>()?;
            ctx.value = match types.as_slice() {
                [ProducedType::Value(array @ (Type::Array(_) | Type::Bytes)), ProducedType::Value(Type::Int)] => {
                    element_type(*array)
                }
                [ProducedType::Value(Type::Map(id)), ProducedType::Value(key)] => match p.composites.get(*id as usize) {
                    Some(Composite::Map { key: declared, value }) if declared == key => Some(*value),
                    _ => None,
                },
                _ => None,
            };
            enforce(ctx.value.is_some(), "LOOKUP_OPERANDS", "lookup needs an array and int or a map and its key")?;
        }
        DepsOf => value(0)?,
        OwnMove | OwnJoin => {
            for index in 0..locations.len() {
                input(index, Packet)?;
            }
        }
        OwnSplit | OwnSplitChecked => {
            input(0, Packet)?;
            for index in 1..locations.len() { value(index)?; }
        }
        OwnDynamic => {
            // A scope state and the packet of unknown retention it takes in (DYNAMIC_SITE).
            enforce(locations.len() == 2 && loc_type(f, locations[0])? == ProducedType::Service(ScopeState)
                && loc_type(f, locations[1])? == ProducedType::Service(Packet), "DYNAMIC_SITE",
                "own.dynamic takes a scope state and a packet")?;
            input(0, ScopeState)?;
            input(1, Packet)?;
        }
        OwnAdmit | OwnAccept | OwnBind | OwnRebind => {
            input(0, ScopeState)?;
            input(locations.len() - 1, Packet)?;
            if matches!(op, OwnBind | OwnRebind) {
                value(1)?;
            }
        }
        ScopeSplit | DeferRegister => {
            input(0, ScopeState)?;
            for index in 1..locations.len() {
                value(index)?;
            }
        }
        ScopeTransfer | OwnStage => {
            let states = if ctx.between { 2 } else { 1 };
            let extra = if op == OwnStage { 2 } else { 0 };
            require(locations.len() == states + extra, "Same/Between operand arity mismatch")?;
            input(0, ScopeState)?;
            if ctx.between {
                input(1, ScopeState)?;
            }
            if op == OwnStage {
                input(locations.len() - 1, Packet)?;
                value(states)?;
                let object = args.iter().find_map(|arg| match arg {
                    Argument::Object(id) => Some(*id), _ => None,
                }).ok_or_else(|| violation("STAGE_FIELD_TYPE", "missing staging descriptor"))?;
                let field = args.iter().find_map(|arg| match arg {
                    Argument::Field(id) => Some(*id), _ => None,
                }).ok_or_else(|| violation("STAGE_FIELD_TYPE", "missing staging field"))?;
                let expected = at(&at(&p.objects, object)?.layout.storage_fields, field)?;
                enforce(loc_type(f, locations[states])? == ProducedType::Value(*expected),
                    "STAGE_FIELD_TYPE", "staged value differs from physical field type")?;
            }
        }
        ExitExtend => {
            input(0, Pending)?;
            input(1, Packet)?;
        }
        ExitPrepare => {
            input(locations.len() - 1, Packet)?;
            if locations.len() == 2 { value(0)?; }
        }
        ExitReplace => {
            input(0, Pending)?;
            input(locations.len() - 1, Packet)?;
            if locations.len() == 3 { value(1)?; }
        }
        ExitCommit => input(0, Pending)?,
        OwnCreateCommit => {
            let object = args.iter().find_map(|arg| match arg { Argument::Object(id) => Some(*id), _ => None })
                .ok_or_else(|| error("create_commit requires descriptor"))?;
            require(matches!(loc_type(f, locations[0])?,
                ProducedType::Value(Type::Object(id) | Type::Successor(id)) if id == object),
                "create_commit identity/descriptor mismatch")?;
        }
        CleanupBegin => {
            require(matches!(loc_type(f, locations[0])?, ProducedType::Service(Packet | ScopeState)),
                "cleanup.begin source must be packet or scope state")?;
            let rest = locations[1..].iter().map(|&loc| loc_type(f, loc)).collect::<Result<Vec<_>, _>>()?;
            require(matches!(rest.as_slice(), [] | [ProducedType::Service(Pending)]
                | [ProducedType::Value(Type::Failure)]
                | [ProducedType::Service(Pending), ProducedType::Value(Type::Failure)]),
                "cleanup.begin pending/failure types or ordering mismatch")?;
        }
        ExitAbort => {
            input(0, Pending)?;
            input(1, Cleanup)?;
        }
        CleanupPlan | CleanupNext | CleanupFinish | CleanupDeliverCancel | CleanupSettleDynamic => {
            input(0, Cleanup)?
        }
        CleanupSettle => {
            input(0, Cleanup)?;
            if let Some(&duty) = locations.get(1) {
                // A cache duty, or a spawned Future's task duty (D24.2).
                enforce(matches!(loc_type(f, duty)?, ProducedType::Service(CacheState | TaskState)),
                    suspension::SCHEDULE, "a settled duty is not a cache-state or a task-state")?;
            }
        }
        FutureNew => {
            let entry = args.iter().find_map(|arg| match arg { Argument::Entry(id) => Some(*id), _ => None })
                .ok_or_else(|| error("fut.new requires an entry"))?;
            let callee = at(&p.functions, entry)?;
            // An async external (D22, #601 Q1): its driver takes the language arguments as its registers.
            let start = if callee.entry_abi == EntryAbi::AsyncExternal {
                drivers::arguments(callee)
            } else {
                let layout = p.frame_layout(callee)
                    .filter(|_| matches!(callee.entry_abi, EntryAbi::AsyncBody { .. }))
                    .ok_or_else(|| violation(suspension::ASYNC_ENTRY, "fut.new must name an async body"))?;
                layout.saved_types(0).ok_or_else(|| error("async body without a start continuation"))?
            };
            // A fixed root starts without a packet: only the root driver starts it (#667).
            enforce(!suspension::fixed_root(p, entry), suspension::ASYNC_ENTRY,
                "fut.new cannot create a fixed root")?;
            let inputs = start.iter().filter(|ty| matches!(ty, ProducedType::Value(_))).count();
            let future = args.iter().find_map(|arg| match arg {
                Argument::Result(result) => *result,
                _ => None,
            });
            let Some(Type::Future(index)) = future else {
                return Err(violation(FUTURE_TYPES, "fut.new must produce a Future type"));
            };
            enforce(p.tables.future_results.get(index as usize) == Some(&callee.results.first().copied()),
                FUTURE_TYPES, "the Future type differs from the async body's result")?;
            require(locations.len() >= inputs + 2, "fut.new needs its arguments, input packet and owner state")?;
            for (&location, expected) in locations.iter().zip(&start[..inputs]) {
                if callee.entry_abi == EntryAbi::AsyncExternal {
                    enforce(loc_type(f, location)? == *expected, drivers::EXTERNAL_ASYNC,
                        "fut.new of an async external passes an argument of another type")?;
                }
                require(loc_type(f, location)? == *expected, "fut.new argument type mismatch")?;
            }
            input(inputs, Packet)?;
            for index in inputs + 1..locations.len() - 1 {
                input(index, Deps)?;
            }
            input(locations.len() - 1, ScopeState)?;
            ctx.value = future;
        }
        Await | FutureCancel | FutureDone | FutureCancelled => {
            let Some(Type::Future(index)) = locations.first().and_then(|&location| match loc_type(f, location) {
                Ok(ProducedType::Value(ty)) => Some(ty),
                _ => None,
            }) else {
                return Err(violation(FUTURE_TYPES, format!("{} needs a Future", op.name())));
            };
            if op == Await {
                ctx.value = *p.tables.future_results.get(index as usize)
                    .ok_or_else(|| violation(FUTURE_TYPES, "a Future type names no future_results entry"))?;
                ctx.may_fail = true;
            }
        }
        AwaitTake | UniqueCacheProof => {
            let Some(Type::Future(index)) = locations.first().and_then(|&location| match loc_type(f, location) {
                Ok(ProducedType::Value(ty)) => Some(ty),
                _ => None,
            }) else {
                return Err(violation(FUTURE_TYPES, format!("{} needs a Future", op.name())));
            };
            let outcome = *p.tables.future_results.get(index as usize)
                .ok_or_else(|| violation(FUTURE_TYPES, "a Future type names no future_results entry"))?;
            if op == AwaitTake {
                input(1, UniqueCache)?;
                ctx.value = outcome;
                ctx.may_fail = true;
            }
        }
        // D24.1: the Future and the cache duty its fut.new gave (`SPB_SPAWN`).
        FutureSpawn => {
            enforce(matches!(loc_type(f, locations[0])?, ProducedType::Value(Type::Future(_))), FUTURE_TYPES,
                "fut.spawn needs a Future")?;
            input(1, CacheState)?;
        }
        CacheRehome if locations.len() == 3 && loc_type(f, locations[1])? == ProducedType::Service(ScopeState) => {
            // The state form (C-4): the reserve, the owner state and the Future whose entry it holds.
            input(0, Pending)?;
            input(1, ScopeState)?;
            enforce(matches!(loc_type(f, locations[2])?, ProducedType::Value(Type::Future(_))), FUTURE_TYPES,
                "the state form of cache.rehome names a Future")?;
            ctx.between = true;
        }
        // The token form: the reserve, the cache duty and, for a spawned Future, its task duty (D24.1).
        CacheRehome => {
            input(0, Pending)?;
            input(1, CacheState)?;
            if locations.len() == 3 {
                input(2, TaskState)?;
            }
        }
        // ABI D25.2: a packet and the Future whose cache duty's entry leaves it.
        ServiceMaterialize => {
            input(0, Packet)?;
            enforce(matches!(loc_type(f, locations[1])?, ProducedType::Value(Type::Future(_))), FUTURE_TYPES,
                "service.materialize names a Future holder")?;
        }
        // A packet and the duty token whose entry joins it: a cache-state, or a task-state of a spawn (D24).
        ServiceAbsorb => {
            input(0, Packet)?;
            require(matches!(loc_type(f, locations[1])?, ProducedType::Service(CacheState | TaskState)),
                "service.absorb takes a cache-state or task-state token")?;
        }
        FuturePublish => {
            input(0, Frame)?;
            let rest = &locations[1..];
            if flags.first() == Some(&true) {
                let value = f.results.first().copied();
                let offset = usize::from(value.is_some());
                require(rest.len() > offset, "fut.publish success needs its value and packet")?;
                if let Some(ty) = value {
                    enforce(loc_type(f, rest[0])? == ProducedType::Value(ty), FUTURE_TYPES,
                        "the published value differs from the Future type")?;
                }
                input(1 + offset, Packet)?;
                for index in 2 + offset..locations.len() {
                    input(index, Deps)?;
                }
            } else {
                require(rest.len() == 1 && loc_type(f, rest[0])? == ProducedType::Value(Type::Failure),
                    "fut.publish failure needs exactly one failure")?;
            }
        }
        FrameSave | FrameResume => {
            input(0, Frame)?;
            let continuation = args.iter().find_map(|arg| match arg {
                Argument::Continuation(id) => Some(*id),
                _ => None,
            }).ok_or_else(|| error("frame operation without a continuation"))?;
            let layout = p.frame_layout(f).ok_or_else(|| error("async body without a frame layout"))?;
            let saved = layout.saved_types(continuation)
                .ok_or_else(|| violation(suspension::RESUME_TABLE, "unknown continuation"))?;
            if op == FrameSave {
                enforce(continuation != 0, suspension::SUSPEND_BRACKET, "the start continuation is never saved")?;
                let actual = locations[1..].iter().map(|&location| loc_type(f, location))
                    .collect::<Result<Vec<_>, _>>()?;
                enforce(actual == saved, suspension::FRAME_LINEAR,
                    "frame.save values differ from the continuation's saved slots")?;
            } else {
                ctx.slots = saved;
            }
        }
        AwaitUnwind => {
            input(0, ScopeState)?;
            require(loc_type(f, locations[1])? == ProducedType::Value(Type::Failure), "expected failure view")?;
            input(2, TaskState)?;
            input(3, Receipt)?;
        }
        CleanupFailure => {
            input(0, Cleanup)?;
            require(loc_type(f, locations[1])? == ProducedType::Value(Type::Failure), "expected failure view")?;
        }
        CleanupDestroy => {
            input(0, Cleanup)?;
            input(1, StableKey)?;
        }
        DestroyDirect => {
            input(0, Cleanup)?;
            let object = args.iter().find_map(|arg| match arg { Argument::Object(id) => Some(*id), _ => None })
                .ok_or_else(|| error("direct destroy requires descriptor"))?;
            require(loc_type(f, locations[1])? == ProducedType::Value(Type::Object(object)),
                "direct destroy identity/descriptor mismatch")?;
        }
        ExitDispatch => {
            input(0, Closed)?;
            input(1, FailureState)?;
            input(2, ExitState)?;
        }
        FailureDispatch => input(0, FailureState)?,
        InvokeObserve => {
            let entry = args.iter().find_map(|arg| match arg { Argument::Entry(id) => Some(*id), _ => None })
                .ok_or_else(|| error("missing observation entry"))?;
            let summary = args.iter().find_map(|arg| match arg { Argument::Summary(id) => Some(*id), _ => None })
                .ok_or_else(|| error("missing observation summary"))?;
            require(at(&p.tables.summaries, summary)?.entry == entry, "entry/summary mismatch")?;
            let callee = at(&p.functions, entry)?;
            require(callee.entry_abi == EntryAbi::Observation, "observation entry ABI mismatch")?;
            let expected = callee.params.iter().map(|&loc| loc_type(callee, loc)).collect::<Result<Vec<_>, _>>()?;
            let actual = locations.iter().map(|&loc| loc_type(f, loc)).collect::<Result<Vec<_>, _>>()?;
            require(actual == expected, "observation argument schema mismatch")?;
        }
        InvokeCallOwn | InvokeDefer => {
            let entry = args.iter().find_map(|arg| match arg { Argument::Entry(id) => Some(*id), _ => None })
                .ok_or_else(|| error("missing entry"))?;
            let summary = args.iter().find_map(|arg| match arg { Argument::Summary(id) => Some(*id), _ => None })
                .ok_or_else(|| error("missing summary"))?;
            require(at(&p.tables.summaries, summary)?.entry == entry, "entry/summary mismatch")?;
            if op == InvokeDefer {
                let map = args.iter().find_map(|arg| match arg {
                    Argument::Registration(id) => Some(*id),
                    _ => None,
                }).ok_or_else(|| error("missing defer registration map"))?;
                require(at(&f.proofs.registrations, map)?.entry == entry, "defer registration/entry mismatch")?;
            }
            let abi = &at(&p.functions, entry)?.entry_abi;
            require(matches!((op, abi), (InvokeCallOwn, EntryAbi::Owned)
                | (InvokeDefer, EntryAbi::Defer { .. } | EntryAbi::AsyncDefer { .. })), "invoke entry ABI mismatch")?;
            let callee = at(&p.functions, entry)?;
            // An async defer entry takes its frame; its captures and its packet arrive through its start (C-2).
            let inputs = match p.frame_layout(callee) {
                Some(layout) => layout.saved_types(0).ok_or_else(|| error("async defer entry without a start"))?,
                None => callee.params.iter().map(|&location| loc_type(callee, location))
                    .collect::<Result<Vec<_>, _>>()?,
            };
            let language = inputs.iter().copied().filter(|ty| matches!(ty, ProducedType::Value(_))).collect::<Vec<_>>();
            require(locations.len() > language.len(), "owning call requires input packet")?;
            for (&location, expected) in locations.iter().zip(&language) {
                require(loc_type(f, location)? == *expected, "owning call language argument type mismatch")?;
            }
            input(language.len(), Packet)?;
            for &location in &locations[language.len() + 1..] {
                require(loc_type(f, location)? == ProducedType::Service(Deps), "expected proof-only call deps")?;
            }
            require(inputs.iter().filter(|&&ty| ty == ProducedType::Service(Packet)).count() == 1,
                "owning entry requires exactly one input packet")?;
            if op == InvokeCallOwn {
                require(flags.len() == 1, "owned call requires explicit cleanup mode")?;
            }
        }
        InvokeDispatch | InvokeDispatchOwn => {
            let Some(Argument::Dispatch(dispatch)) = args.first() else {
                return Err(violation("DISPATCH_SITE", "missing dispatch"));
            };
            let slot = &role_decl(p, dispatch.role)?.slots[dispatch.slot as usize];
            let abi = if op == InvokeDispatch { EntryAbi::Observation } else { EntryAbi::Owned };
            enforce(slot.entry_abi == abi, "DISPATCH_SITE", "dispatch ABI differs from the role slot")?;
            // The receiver is a role value, or the one object a single-target dispatch selects.
            let receiver = match locations.first().map(|&location| loc_type(f, location)).transpose()? {
                Some(ProducedType::Value(Type::Object(object))) if dispatch.targets == [object] => Type::Object(object),
                _ => Type::Role(dispatch.role),
            };
            let mut expected = vec![ProducedType::Value(receiver)];
            expected.extend(slot.params.iter().map(|&ty| ProducedType::Value(ty)));
            let actual = locations.iter().take(expected.len()).map(|&loc| loc_type(f, loc))
                .collect::<Result<Vec<_>, _>>()?;
            enforce(actual == expected, "DISPATCH_SITE", "dispatch receiver or arguments differ from the slot")?;
            if op == InvokeDispatch {
                enforce(locations.len() == expected.len(), "DISPATCH_SITE", "plain dispatch carries no services")?;
            } else {
                require(locations.len() > expected.len(), "owning call requires input packet")?;
                input(expected.len(), Packet)?;
                for &location in &locations[expected.len() + 1..] {
                    require(loc_type(f, location)? == ProducedType::Service(Deps), "expected proof-only call deps")?;
                }
                require(flags.len() == 1, "owned call requires explicit cleanup mode")?;
            }
        }
        OrderDispatch => {
            let Some(Argument::Dispatch(dispatch)) = args.first() else {
                return Err(violation("DISPATCH_SITE", "missing dispatch"));
            };
            let slot = &role_decl(p, dispatch.role)?.slots[dispatch.slot as usize];
            let element = match locations.as_slice() {
                [array] => match loc_type(f, *array)? {
                    ProducedType::Value(Type::Array(shape)) => shape.element(),
                    _ => None,
                },
                _ => None,
            };
            // The elements are the dispatch's receivers: role values, or the one object it selects.
            let receivers = match element {
                Some(Type::Object(object)) => dispatch.targets == [object],
                Some(Type::Role(role)) => role == dispatch.role,
                _ => false,
            };
            let order = match slot.results.as_slice() {
                [Type::Object(id)] => p.objects.get(*id as usize).is_some_and(|object| object.enum_cases == 3
                    && object.fields.is_empty() && object.layout.storage_fields == [Type::Int]),
                _ => false,
            };
            // A comparison borrows its elements: an owning slot gets an empty packet and must return one, and
            // the elements' obligations stay with the array's owner.
            let borrows = dispatch.retained_params.is_empty();
            enforce(matches!(slot.entry_abi, EntryAbi::Observation | EntryAbi::Owned) && borrows && receivers
                && element.is_some_and(|element| {
                slot.params == [element]
            }) && order, ORDER_DISPATCH,
                "array.order.dispatch compares its array's elements by an observation slot returning compare.Order")?;
        }
        RetOwn => {
            require(f.entry_abi == EntryAbi::Owned, "ret.own requires owned entry")?;
            let mut expected = f.results.iter().copied().map(ProducedType::Value).collect::<Vec<_>>();
            expected.push(ProducedType::Service(Packet));
            let actual = locations.iter().map(|&location| loc_type(f, location)).collect::<Result<Vec<_>, _>>()?;
            // ret.own's runtime payload is value?, packet; explicit deps is optional proof metadata.
            if actual.last() == Some(&ProducedType::Service(Deps)) {
                expected.push(ProducedType::Service(Deps));
            }
            require(actual == expected, "ret.own result schema mismatch")?;
        }
        RetDefer => {
            let EntryAbi::Defer { labels } = &f.entry_abi else {
                return Err(error("ret.defer requires defer entry"));
            };
            let returned = args.iter().filter_map(|arg| match arg {
                Argument::Label(label) => Some(*label), _ => None,
            }).collect::<Vec<_>>();
            require(returned.len() <= 1, "ret.defer needs one tag")?;
            let expected = if let Some(&label) = returned.first() {
                Opcode::InvokeDefer.results(Tag::Replace(label), &ResultContext {
                    labels: labels.clone(), ..ResultContext::default()
                })?
            } else {
                vec![]
            };
            let actual = locations.iter().map(|&location| loc_type(f, location)).collect::<Result<Vec<_>, _>>()?;
            require(actual == expected, "ret.defer result schema mismatch")?;
        }
        _ => {}
    }
    Ok(ctx)
}

fn check_arguments(op: Opcode, args: &[Argument]) -> Check {
    use Argument::*;
    use Opcode::*;
    // S1 freezes references and result positions. Rich consume/lifetime maps
    // and binding/defer contracts are verified by the later ownership gate.
    let kind = |arg: &Argument| -> char {
        match arg {
            Evaluation(_) => 'z', Result(_) => 'r', Location(_) => 'l', Scope(_) => 's', Boundary(_) => 'b',
            Summary(_) => 'u',
            Selection(_) => 'e', Plan(_) => 'p', Schedule(_) => 'd', Envelope(_) => 'v',
            Entry(_) => 'f', Object(_) => 'o', Binding(_) => 'i', Field(_) => 't',
            Label(_) => 'a', Flag(_) => '?', Kind(_) => 'k', Registration(_) => 'm', Destination(_) => 'x',
            Continuation(_) => 'c',
            Dispatch(_) => 'g',
            Protocol(_) => 'q',
        }
    };
    let shape: String = args.iter().map(kind).collect();
    let valid = match op {
        Catch => shape == "lo" || shape == "l?",
        Evaluate => shape.starts_with('z') && shape[1..].chars().all(|ch| ch == 'l'),
        Fail => shape == "al",
        ArrayGet => shape == "ll",
        ScopeBegin => shape == "sb",
        OwnEmpty | DepsEmpty => shape.is_empty(),
        OwnMove | DepsOf => shape == "l",
        OwnJoin => !shape.is_empty() && shape.chars().all(|ch| ch == 'l'),
        OwnAdmit | OwnAccept => shape == "ll",
        OwnDynamic => shape == "qll",
        OwnBind | OwnRebind => shape == "lisll",
        OwnStage => shape == "?lotll" || shape == "?llotll",
        OwnSplit => shape == "le",
        OwnSplitChecked => shape.starts_with("le") && shape[2..].chars().all(|ch| ch == 'l'),
        ScopeSplit => shape.starts_with("le") && shape[2..].chars().all(|ch| ch == 'l' || ch == '?')
            && args.iter().filter(|arg| matches!(arg, Flag(_))).count() <= 1,
        ScopeTransfer => matches!(shape.as_str(), "?le" | "?lle" | "?le?" | "?lle?"),
        DeferRegister => shape.starts_with("lm") && shape[2..].chars().all(|ch| ch == 'l' || ch == 'i'),
        ExitPrepare => shape == "kble" || shape == "kblle",
        ExitExtend | ExitAbort | CleanupFailure | CleanupDestroy => shape == "ll",
        ExitReplace => shape == "lkble" || shape == "lkblle",
        ExitCommit => matches!(shape.as_str(), "lx" | "lb" | "lxr"),
        CleanupBegin => matches!(shape.as_str(), "lbv" | "llbv" | "lllbv"),
        CleanupSettle => shape == "ld" || shape == "ldl",
        CleanupPlan => shape == "lp",
        CleanupNext | CleanupFinish | FailureDispatch => shape == "l",
        DestroyDirect => shape == "llof",
        InvokeObserve => shape.starts_with("fu") && shape[2..].chars().all(|ch| ch == 'l'),
        InvokeCallOwn => shape.starts_with("fu") && shape[2..].chars().all(|ch| ch == 'l' || ch == '?'),
        InvokeDefer => shape.starts_with("fum") && shape[3..].chars().all(|ch| ch == 'l' || ch == 'i'),
        RetOwn | RetDefer => shape.chars().all(|ch| matches!(ch, 'l' | 'a')),
        ExitDispatch => shape == "lll???",
        OwnCreateCommit => shape == "lob",
        InvokeDispatch => shape.starts_with('g') && shape[1..].chars().all(|ch| ch == 'l'),
        OrderDispatch => shape == "gl",
        InvokeDispatchOwn => shape.starts_with('g') && shape[1..].chars().all(|ch| ch == 'l' || ch == '?'),
        BuilderPush => shape == "lll" || shape == "llll",
        BuilderAppend => shape == "lll",
        BuilderRange => shape == "?llll",
        BuilderFinish | BuilderAbandon | Lookup => shape == "ll",
        FutureNew => shape.len() >= 5 && shape.starts_with("fsr") && shape[3..].chars().all(|ch| ch == 'l'),
        Await | FutureCancel | FutureDone | FutureCancelled | CleanupDeliverCancel | UniqueCacheProof => shape == "l",
        // The Future, its take capability and the awaiting scope of the temporary boundary (C5).
        AwaitTake => shape == "lls",
        // The Future and its cache duty (D24.1).
        FutureSpawn => shape == "ll",
        // PR-E: locations only; SPB_SPAWN requires exactly the Future (`suspension::check_spawns`).
        FutureSpawnBound => !shape.is_empty() && shape.chars().all(|ch| ch == 'l'),
        // The exit's destination, the pending reserve and the cache duty with a spawned Future's task duty (C-4,
        // token form), or the owner state and the Future whose admitted entry moves (state form).
        CacheRehome => shape == "xll" || shape == "xlll",
        // The closing scope's cleanup state (D25.3); a packet and the holder or the token (D25.2).
        CleanupSettleDynamic => shape == "l",
        ServiceMaterialize | ServiceAbsorb => shape == "ll",
        FuturePublish => shape.len() > 2 && shape.starts_with("l?") && shape[2..].chars().all(|ch| ch == 'l'),
        FrameSave => shape.starts_with("lc") && shape[2..].chars().all(|ch| ch == 'l'),
        FrameResume => shape == "lc",
        AwaitUnwind => shape == "llll",
    };
    require(valid, &format!("invalid operands for {}", op.name()))
}

fn check_data(p: &Program, f: &Function, op: DataOp, inputs: &[u32], output: Option<u32>, imm: &Immediate) -> Check {
    let types = inputs.iter().map(|&id| at(&f.registers, id).copied()).collect::<Result<Vec<_>, _>>()?;
    let out = output.map(|id| at(&f.registers, id).copied()).transpose()?;
    check_data_types(p, op, &types, out, imm)
}

fn check_data_types(p: &Program, op: DataOp, types: &[Type], out: Option<Type>, imm: &Immediate) -> Check {
    use DataOp::*;
    let expect = |args: &[Type], result: Option<Type>| -> Check {
        require(types == args && out == result, "scalar operand/result type or arity mismatch")
    };
    let immediate_none = !matches!(op,
        Const | RangeNew | ObjNew | ObjInit | ObjGet | ObjNext | EnumCase | VariantGet | GlobalGet);
    if immediate_none {
        require(*imm == Immediate::None, "unexpected scalar immediate")?;
    }
    if let Some((operands, result)) = op.scalar_signature() {
        return enforce(types == operands && out == Some(result), "SCALAR_OP_TYPE",
            "operand or result types differ from the scalar operation's signature");
    }
    match op {
        EnumCase => {
            let Immediate::Updates(indices) = imm else {
                return Err(violation("ENUM_CASE", "enum.case requires descriptor and case"));
            };
            let [id, case] = indices.as_slice() else {
                return Err(violation("ENUM_CASE", "enum.case requires descriptor and case"));
            };
            let object = at(&p.objects, *id)?;
            enforce(object.fields.is_empty() && object.layout.storage_fields == [Type::Int]
                && !object.layout.may_reach_resource
                && ((*case as usize) < object.error_cases.len() || *case < object.enum_cases),
                "ENUM_CASE", "invalid plain enum case")?;
            expect(&[], Some(Type::Object(*id)))
        }
        GlobalGet => {
            let Immediate::Global(index) = *imm else {
                return Err(violation("GLOBAL_GET", "global.get requires a module constant index"));
            };
            let global = p.globals.get(index as usize)
                .ok_or_else(|| violation("GLOBAL_GET", "global.get names no module constant"))?;
            enforce(types.is_empty() && out == Some(global.ty), "GLOBAL_GET",
                "global.get takes no operand and produces the constant's type")
        }
        ArrayNew => {
            let Some(element) = out.and_then(element_type) else {
                return Err(violation("ARRAY_NEW_TYPE", "array.new must produce a plain array"));
            };
            enforce(types.iter().all(|ty| *ty == element),
                "ARRAY_NEW_TYPE", "array.new element types differ from the result element")
        }
        ArrayLen => enforce(matches!(types, [Type::Array(_) | Type::Bytes]) && out == Some(Type::Int),
            "ARRAY_LEN_TYPE", "array.len needs a plain array and an int result"),
        MapNew | MapLen | MapHas | MapGet | MapKeys | MapValues | MapKeyAt | MapValueAt | MapMerge
            | MapDifference => enforce(map_types(p, op, types, out), "MAP_OP_TYPE",
                "map operation operand/result types mismatch"),
        ArrayConcat | ArrayJoin | ArraySlice | ArrayRemove | ArrayReplace | ArrayDifference => {
            // array.difference compares elements structurally, which only plain data allows.
            let comparable = op != ArrayDifference
                || types.first().is_some_and(|&ty| self::comparable(p, ty));
            enforce(array_types(op, types, out) && comparable, "ARRAY_OP_TYPE",
                "array operation operand/result types mismatch")
        }
        ArrayOrder => enforce(types.len() == 1 && out == Some(types[0]) && sortable(types[0]),
            ARRAY_ORDER_TYPE,
            "array.order sorts an array of int, float, rat, char or str, or a byte array, into its own type"),
        BuilderNew => enforce(types.is_empty() && matches!(out, Some(Type::Builder(_) | Type::MapBuilder(_))),
            "BUILDER_NEW_TYPE", "builder.new must produce a builder"),
        Const => {
            let Immediate::Constant(value) = imm else { return Err(error("missing constant")); };
            if let Constant::Descriptor(id) = value {
                at(&p.objects, *id)?;
            }
            expect(&[], Some(constant_type(value)?))
        }
        FailSecondary | FailCaused => expect(&[Type::Failure, Type::Failure], Some(Type::Failure)),
        IntAdd | IntSub | IntMul | IntMod => expect(&[Type::Int, Type::Int], Some(Type::Int)),
        IntNeg => expect(&[Type::Int], Some(Type::Int)),
        IntEq | IntNe | IntLt | IntLe | IntGt | IntGe => expect(&[Type::Int, Type::Int], Some(Type::Bool)),
        BoolNot => expect(&[Type::Bool], Some(Type::Bool)),
        BoolEq | BoolNe => expect(&[Type::Bool, Type::Bool], Some(Type::Bool)),
        StrConcat => expect(&[Type::Str, Type::Str], Some(Type::Str)),
        StrEq | StrNe | StrLt | StrLe | StrGt | StrGe => expect(&[Type::Str, Type::Str], Some(Type::Bool)),
        StrLen => expect(&[Type::Str], Some(Type::Int)),
        ByteEq | ByteNe | ByteLt | ByteLe | ByteGt | ByteGe => expect(&[Type::Byte, Type::Byte], Some(Type::Bool)),
        CharEq | CharNe | CharLt | CharLe | CharGt | CharGe => expect(&[Type::Char, Type::Char], Some(Type::Bool)),
        FloatEq | FloatNe | FloatLt | FloatLe | FloatGt | FloatGe => {
            expect(&[Type::Float, Type::Float], Some(Type::Bool))
        }
        RatEq | RatNe | RatLt | RatLe | RatGt | RatGe => expect(&[Type::Rat, Type::Rat], Some(Type::Bool)),
        RatNew => expect(&[Type::Int, Type::Int], Some(Type::Rat)),
        EnumEq | EnumNe => {
            let enumeration = match types {
                [Type::Object(left), Type::Object(right)] if left == right => {
                    let object = at(&p.objects, *left)?;
                    object.fields.is_empty() && object.layout.storage_fields == [Type::Int]
                        && (!object.error_cases.is_empty() || object.enum_cases > 0)
                }
                _ => false,
            };
            enforce(enumeration && out == Some(Type::Bool), "ENUM_EQ_TYPE",
                "enum comparison needs two values of one plain enum and a bool result")
        }
        VariantTag => enforce(matches!(types, [Type::Variant(_)]) && out == Some(Type::Int), "VARIANT_TAG",
            "variant.tag needs a variant value and an int result"),
        VariantGet => {
            let field = match (types, imm) {
                ([Type::Variant(variant)], Immediate::Updates(indices)) => match indices.as_slice() {
                    [case, storage] => p.objects[*variant as usize].variant_cases.get(*case as usize)
                        .and_then(|&case| p.objects.get(case as usize))
                        .filter(|case| case.fields.iter().any(|field| field.storage == *storage))
                        .and_then(|case| case.layout.storage_fields.get(*storage as usize).copied()),
                    _ => None,
                },
                _ => None,
            };
            enforce(field.is_some() && out == field, "VARIANT_GET",
                "variant.get needs a case of the variant and one of its payload fields")
        }
        ValEq | ValNe => {
            let comparable = match types {
                // Two values of one role compare as their objects: different implementations are unequal.
                [Type::Object(left), Type::Object(right)] | [Type::Role(left), Type::Role(right)] => left == right,
                [left @ (Type::Array(_) | Type::Bytes | Type::Map(_) | Type::Variant(_) | Type::Range), right] => {
                    left == right
                }
                // A generic `==` specialized to a scalar (`Frequencies<string>`) compares two values of it.
                [left @ (Type::Int | Type::Bool | Type::Str | Type::Float | Type::Char | Type::Byte | Type::Rat),
                    right] => left == right,
                _ => false,
            };
            enforce(comparable && out == Some(Type::Bool), "VAL_EQ_TYPE",
                "value comparison needs two values of one scalar, object or plain collection type")?;
            // Structural equality is defined on plain values; identities and error payloads are not compared.
            enforce(self::comparable(p, types[0]), "VAL_EQ_TYPE", "value comparison needs closed plain operands")
        }
        Convert => enforce(matches!((types, out), ([from], Some(to)) if conversion(*from, to).is_some()),
            "CONVERT_TYPE", "conversion between these types is not defined"),
        TestOk | TestFail | TestIs | TestIsnt => {
            enforce(p.tests.is_some(), "TEST_PROFILE", "test operations need a tests table")?;
            let valid = match (op, types) {
                (TestOk, [Type::Bool]) | (TestFail, [Type::Str]) => true,
                (TestIs | TestIsnt, [left, right]) => left == right && matches!(left,
                    Type::Int | Type::Bool | Type::Byte | Type::Float | Type::Char | Type::Rat | Type::Str
                    | Type::Range | Type::Bytes | Type::Array(_) | Type::Object(_) | Type::Successor(_)
                    | Type::Variant(_)),
                _ => false,
            };
            enforce(valid && out == Some(Type::Bool), "TEST_OPERAND",
                "test operation operand types differ from its check, or its result is not bool")?;
            // `test.is` compares as `val.eq` does: objects and arrays are compared structurally only when they
            // are closed plain data without native handles (an array of handles has no structural value, #751).
            let compared = match types {
                [Type::Successor(id), _] => Some(Type::Object(*id)),
                [ty @ (Type::Object(_) | Type::Variant(_) | Type::Array(_)), _] => Some(*ty),
                _ => None,
            };
            enforce(compared.is_none_or(|ty| comparable(p, ty)), "TEST_OPERAND",
                "test.is/test.isnt compare objects and arrays only when they are closed plain data")
        }
        RoleNarrow => enforce(matches!((types, out), ([Type::Role(_)], Some(Type::Role(_)))), "ROLE_NARROW",
            "role narrowing converts a role value to another role"),
        // any.box: an any box becomes an `any` value.
        Move if out == Some(Type::Any) && types.len() == 1 && types[0] != Type::Any => {
            let boxed = matches!(types[0], Type::Object(id)
                if p.objects.get(id as usize).is_some_and(|object| object.any_box));
            enforce(boxed, "ANY_WRAP", "only an any box converts to an any value")
        }
        Move if matches!(out, Some(Type::Role(_) | Type::RoleSuccessor(_))) && types.len() == 1
            && out != Some(types[0]) =>
        {
            // role.wrap: an implementing object becomes a role value; succ.take drops a role successor's permission.
            let valid = match (types[0], out) {
                (Type::Object(object), Some(Type::Role(role)))
                    | (Type::Successor(object), Some(Type::RoleSuccessor(role))) => implements(p, object, role),
                (Type::RoleSuccessor(source), Some(Type::Role(role))) => source == role,
                _ => false,
            };
            enforce(valid, "ROLE_WRAP", "only an implementing object converts to a role value")
        }
        Move => {
            require(types.len() == 1, "move requires one source")?;
            let valid = out == Some(types[0]) || matches!((types[0], out),
                (Type::Successor(a), Some(Type::Object(b))) if a == b)
                || matches!((types[0], out), (Type::Object(id), Some(Type::Error))
                    if !p.objects[id as usize].error_cases.is_empty())
                || matches!((types[0], out), (Type::Variant(id), Some(Type::Error)) if error_variant(p, id))
                || matches!((types[0], out), (Type::Object(case), Some(Type::Variant(variant)))
                    if p.objects[variant as usize].variant_cases.contains(&case))
                || out.is_some_and(|out| never_retag(&p.composites, types[0], out));
            let never_source = match types[0] {
                Type::Array(shape) => shape.leaf == ArrayLeaf::Never,
                Type::Map(id) => matches!(p.composites.get(id as usize), Some(Composite::Map { key: Type::Never, .. })),
                _ => false,
            };
            enforce(valid || !never_source, NEVER_RETAG,
                "a collection of never leaves retags only to its own shape, keeping a map's value type")?;
            require(valid, "invalid move/succ.take type")
        }
        Undef => require(types.is_empty() && out.is_some(), "invalid undef"),
        RangeNew => {
            require(matches!(imm, Immediate::Inclusive(_)), "range.new requires inclusive flag")?;
            expect(&[Type::Int, Type::Int], Some(Type::Range))
        }
        RangeStart | RangeEnd | RangeLen => expect(&[Type::Range], Some(Type::Int)),
        ObjNew => {
            let Immediate::Object(id) = *imm else { return Err(error("missing object descriptor")); };
            let object = at(&p.objects, id)?;
            let fields = object.fields.iter().map(|field| object.layout.storage_fields[field.storage as usize])
                .collect::<Vec<_>>();
            expect(&fields, Some(Type::Object(id)))
        }
        ObjGet | ObjInit | ObjNext => {
            let Some(Type::Object(id) | Type::Successor(id)) = types.first().copied() else {
                return Err(error("expected object receiver"));
            };
            let layout = &at(&p.objects, id)?.layout;
            let field_type = |index: u32| -> Result<Type, Error> {
                require(at(&p.objects, id)?.fields.iter().any(|field| field.storage == index),
                    "field is hidden storage or identity tail")?;
                Ok(*at(&layout.storage_fields, index)?)
            };
            if op == ObjNext {
                let Immediate::Updates(indices) = imm else { return Err(error("missing successor updates")); };
                let mut fields = vec![types[0]];
                let mut seen = BTreeSet::new();
                for &index in indices {
                    require(seen.insert(index), "duplicate successor update")?;
                    fields.push(field_type(index)?);
                }
                expect(&fields, Some(Type::Successor(id)))
            } else {
                let Immediate::Field(index) = *imm else { return Err(error("missing field index")); };
                let field = field_type(index)?;
                match op {
                    ObjGet => expect(&[types[0]], Some(field)),
                    ObjInit => expect(&[types[0], field], None),
                    _ => unreachable!("object operation"),
                }
            }
        }
        _ => unreachable!("scalar family operations are checked by their signature"),
    }
}

/// Operand and result types of a map operation.
fn map_types(p: &Program, op: DataOp, types: &[Type], out: Option<Type>) -> bool {
    use DataOp::*;
    let map = |ty: Type| match ty {
        Type::Map(id) => match p.composites.get(id as usize) {
            Some(Composite::Map { key, value }) => Some((*key, *value)),
            _ => None,
        },
        _ => None,
    };
    if op == MapNew {
        let Some((key, value)) = out.and_then(map) else { return false };
        return types.len().is_multiple_of(2) && types.chunks(2).all(|pair| pair == [key, value]);
    }
    if op == MapMerge {
        return !types.is_empty() && out.and_then(map).is_some() && types.iter().all(|&ty| Some(ty) == out);
    }
    let Some(&receiver) = types.first() else { return false };
    let Some((key, value)) = map(receiver) else { return false };
    match op {
        MapLen => types.len() == 1 && out == Some(Type::Int),
        MapHas => types == [receiver, key] && out == Some(Type::Bool),
        MapGet => types == [receiver, key] && out == Some(value),
        MapKeys => types.len() == 1 && out.is_some() && out == ArrayType::of(key),
        MapValues => types.len() == 1 && out.is_some() && out == ArrayType::of(value),
        MapKeyAt => types == [receiver, Type::Int] && out == Some(key),
        MapValueAt => types == [receiver, Type::Int] && out == Some(value),
        MapDifference => out == Some(receiver)
            && types[1..].iter().all(|&ty| map(ty).is_some_and(|(removed, _)| removed == key)),
        _ => false,
    }
}

/// An array whose elements have Tomato's natural order: int, float, rat, char, str, or a byte array.
fn sortable(ty: Type) -> bool {
    match ty {
        Type::Bytes => true,
        Type::Array(shape) => shape.depth == 1
            && matches!(shape.leaf,
                ArrayLeaf::Int | ArrayLeaf::Float | ArrayLeaf::Rat | ArrayLeaf::Char | ArrayLeaf::Str),
        _ => false,
    }
}

/// Operand and result types of an array operation; the receiver is an array or a byte array.
fn array_types(op: DataOp, types: &[Type], out: Option<Type>) -> bool {
    use DataOp::*;
    let Some(&receiver) = types.first() else { return false };
    let Some(element) = element_type(receiver) else { return false };
    match op {
        ArrayConcat => out == Some(receiver) && types.iter().all(|&ty| ty == receiver),
        ArrayJoin => types == [receiver, Type::Str] && element == Type::Str && out == Some(Type::Str),
        ArraySlice => types == [receiver, Type::Int, Type::Int] && out == Some(receiver),
        ArrayRemove => types == [receiver, Type::Int] && out == Some(receiver),
        ArrayReplace => types == [receiver, Type::Int, element] && out == Some(receiver),
        ArrayDifference => out == Some(receiver) && types.iter().all(|&ty| ty == receiver),
        _ => false,
    }
}

fn terminates(inst: &Instruction) -> bool {
    match inst {
        Instruction::Data { .. } => false,
        Instruction::Champ { op, flow, .. } => matches!(op, Opcode::RetOwn | Opcode::RetDefer)
            || matches!(flow, Flow::Outcomes(_)),
        _ => true,
    }
}

fn edges(inst: &Instruction) -> Vec<(&Edge, Vec<ProducedType>)> {
    match inst {
        Instruction::Br(edge) => vec![(edge, vec![])],
        Instruction::Cond { yes, no, .. } => vec![(yes, vec![]), (no, vec![])],
        _ => vec![],
    }
}

pub(crate) fn checked_edges<'a>(p: &Program, f: &Function, inst: &'a Instruction)
    -> Result<Vec<(&'a Edge, Vec<ProducedType>)>, Error>
{
    if let Instruction::Champ { op, arguments, flow } = inst {
        let ctx = result_context(p, f, *op, arguments)?;
        let tags = op.tags(&ctx);
        if *op == Opcode::ArrayGet {
            enforce(matches!(flow, Flow::Outcomes(outcomes)
                if outcomes.iter().map(|outcome| outcome.tag).collect::<Vec<_>>() == tags),
                "ARRAY_GET_OUTCOMES", "array.get requires ordered normal and unwind outcomes")?;
        }
        match flow {
            Flow::Next(destinations) => {
                require(!op.is_terminator() || matches!(op, Opcode::RetOwn | Opcode::RetDefer)
                    || matches!(op, Opcode::InvokeCallOwn | Opcode::InvokeObserve | Opcode::InvokeDispatch
                        | Opcode::InvokeDispatchOwn | Opcode::OrderDispatch) && !ctx.may_fail,
                    "terminator requires outcomes")?;
                if *op == Opcode::DepsOf {
                    let [Argument::Location(anchor)] = arguments.as_slice() else { unreachable!() };
                    require(destinations.len() == 1 && f.proofs.dependencies.iter().any(|dependency| {
                        dependency.value == destinations[0] && dependency.obligations.contains(anchor)
                    }), "deps.of must retain its full-deps anchor")?;
                }
                let results = op.results(Tag::Normal, &ctx)?;
                require(destinations.len() == results.len(), "bad multi-result arity")?;
                for (&loc, &ty) in destinations.iter().zip(&results) {
                    require(loc_type(f, loc)? == ty, "bad multi-result type")?;
                }
                Ok(vec![])
            }
            Flow::Outcomes(outcomes) => {
                require(op.is_terminator() && !matches!(op, Opcode::RetOwn | Opcode::RetDefer),
                    "non-outcome operation has outcomes")?;
                let actual = outcomes.iter().map(|outcome| outcome.tag).collect::<Vec<_>>();
                require(actual == tags, "invalid outcome set or order")?;
                outcomes.iter().map(|outcome| Ok((&outcome.edge, op.results(outcome.tag, &ctx)?))).collect()
            }
        }
    } else {
        Ok(edges(inst))
    }
}

fn uses(inst: &Instruction) -> Vec<Location> {
    let mut uses = match inst {
        Instruction::Data { inputs, .. } | Instruction::Ret(inputs) =>
            inputs.iter().copied().map(Location::Register).collect(),
        Instruction::Cond { condition, .. } => vec![Location::Register(*condition)],
        Instruction::Raise(value) => vec![Location::Register(*value)],
        Instruction::Champ { arguments, .. } => arguments.iter().filter_map(|arg| match arg {
            Argument::Location(location) => Some(*location),
            _ => None,
        }).collect(),
        _ => vec![],
    };
    let ordinary = edges(inst);
    let champ = match inst {
        Instruction::Champ { flow: Flow::Outcomes(outcomes), .. } =>
            outcomes.iter().map(|outcome| &outcome.edge).collect::<Vec<_>>(),
        _ => vec![],
    };
    for edge in ordinary.iter().map(|(edge, _)| *edge).chain(champ) {
        uses.extend(edge.assignments.iter().filter_map(|assignment| match assignment.source {
            Source::Location(location) => Some(location),
            Source::ProducedSlot(_) => None,
        }));
    }
    uses
}

fn verify_function(p: &Program, f: &Function) -> Check {
    check_proofs(p, f)?;
    drivers::check_declaration(f)?;
    suspension::check_entry(f)?;
    if f.proofs.entry.as_ref().is_some_and(|entry| entry.external.is_some()) {
        enforce(f.code.is_empty() && f.blocks.is_empty(), "EXTERNAL_SIGNATURE", "external entry cannot carry code")?;
        for &param in &f.params {
            loc_type(f, param)?;
        }
        return Ok(());
    }
    require(f.results.len() <= 1, "P1 entry has at most one language result")?;
    require(!f.blocks.is_empty() && !f.code.is_empty(), "function needs a body")?;
    require(f.blocks[0].pc == 0 && f.blocks[0].params == f.params, "invalid entry block")?;
    let mut pcs = BTreeMap::new();
    let mut definitions = BTreeMap::new();
    for (index, block) in f.blocks.iter().enumerate() {
        at(&f.code, block.pc)?;
        require(pcs.insert(block.pc, index).is_none(), "duplicate block pc")?;
        if index > 0 {
            require(block.pc > f.blocks[index - 1].pc, "blocks must be ordered by pc")?;
        }
        for &param in &block.params {
            loc_type(f, param)?;
            require(definitions.insert(param, (index, None)).is_none(), "duplicate block parameter")?;
        }
    }
    let metadata = allocation::metadata_uses(f);
    let mut predecessors = vec![Vec::new(); f.blocks.len()];
    for (block_index, block) in f.blocks.iter().enumerate() {
        let end = f.blocks.get(block_index + 1).map_or(f.code.len(), |block| block.pc as usize);
        for pc in block.pc as usize..end {
            let inst = &f.code[pc];
            require(terminates(inst) == (pc + 1 == end), "missing or early block terminator")?;
            if matches!(inst, Instruction::Data { .. } | Instruction::Champ { .. }) {
                // A never value comes only from an unreachable read (an element of an empty collection, an
                // undef placeholder): no operation may consume it as a word of any type.
                let never = operand_reads(inst).into_iter()
                    .any(|location| matches!(loc_type(f, location), Ok(ProducedType::Value(Type::Never))));
                enforce(!never, NEVER_OPERAND, &format!("an operation reads a value of type never at {pc}"))?;
            }
            match inst {
                Instruction::Data { op, inputs, output, immediate } => {
                    check_data(p, f, *op, inputs, *output, immediate)?;
                    if let Some(dst) = output {
                        require(definitions.insert(Location::Register(*dst), (block_index, Some(pc))).is_none(),
                            "register has multiple SSA definitions")?;
                    }
                }
                Instruction::Cond { condition, .. } => {
                    require(*at(&f.registers, *condition)? == Type::Bool, "condition must be bool")?;
                }
                Instruction::Raise(value) => {
                    require(at(&f.registers, *value)? == &Type::Failure, "raise needs failure")?;
                }
                Instruction::Ret(values) if f.entry_abi.asynchronous() => {
                    enforce(values.is_empty(), suspension::ASYNC_ENTRY,
                        "an async body publishes instead of returning")?;
                }
                Instruction::Ret(values) => {
                    enforce(f.entry_abi == EntryAbi::Observation, "ENTRY_RETURN_ABI",
                        "owning entries must return through ret.own/ret.defer")?;
                    let types = values.iter().map(|&id| at(&f.registers, id).copied())
                        .collect::<Result<Vec<_>, _>>()?;
                    require(types == f.results, "return type/arity mismatch")?;
                }
                Instruction::Champ { flow: Flow::Next(destinations), .. } => {
                    for &dst in destinations {
                        require(definitions.insert(dst, (block_index, Some(pc))).is_none(),
                            "location has multiple SSA definitions")?;
                    }
                }
                _ => {}
            }
            for (edge, produced) in checked_edges(p, f, inst)? {
                let target = *pcs.get(&edge.target_pc).ok_or_else(|| error("edge target is not a block"))?;
                // The evidence reads an entry parameter's caller-supplied value only when the
                // parameter has no incoming edge; a back edge to block 0 would hide that value.
                enforce(target != 0, "ENTRY_BLOCK_TARGET", "no edge may enter the entry block")?;
                let params = &f.blocks[target].params;
                require(edge.assignments.len() == params.len(), "bad edge arity")?;
                for (assignment, &param) in edge.assignments.iter().zip(params) {
                    require(assignment.destination == param, "edge destinations must match ordered block parameters")?;
                    let source = match assignment.source {
                        Source::Location(location) => loc_type(f, location)?,
                        Source::ProducedSlot(slot) => *at(&produced, slot)?,
                    };
                    require(loc_type(f, param)? == source, "edge assignment type mismatch")?;
                }
                predecessors[target].push(block_index);
            }
        }
    }
    // The bound of the dominator bitsets that #779 replaced by a dominator tree stays the bound on a CFG:
    // hostile CFGs cannot cause unbounded verifier work. This check is independent of the decoder's budget.
    let count = f.blocks.len();
    let words = count.div_ceil(64);
    require(count.saturating_mul(words).saturating_mul(8) <= 64 * 1024 * 1024,
        "CFG exceeds dominator memory budget")?;
    let dominance = Dominance::of(&predecessors);
    // check_proofs has checked every dependency location; the table closes a set in time linear in what it adds,
    // where `close_dependencies` walks every record until nothing changes (#779).
    let obligations = allocation::Obligations::new(f)?;
    for (block_index, block) in f.blocks.iter().enumerate() {
        let end = f.blocks.get(block_index + 1).map_or(f.code.len(), |block| block.pc as usize);
        for (pc, metadata) in metadata.iter().enumerate().take(end).skip(block.pc as usize) {
            let mut used = allocation::direct_uses(f, pc);
            used.extend(uses(&f.code[pc]));
            used.extend(metadata);
            obligations.close(&mut used);
            for used in used {
                let &(def_block, def_pc) = definitions.get(&used).ok_or_else(|| error("undefined location"))?;
                require(dominance.dominates(def_block, block_index), "definition does not dominate use")?;
                require(def_block != block_index || def_pc.is_none_or(|defined| defined < pc),
                    "location used before definition")?;
                if let Location::Register(reg) = used
                    && let Some(defined) = def_pc
                    && matches!(f.code[defined], Instruction::Data { op: DataOp::Undef, .. })
                {
                    // Forwarding along an edge is not a read: check_unbound tracks the parameter it binds.
                    let forwarded = !operand_reads(&f.code[pc]).contains(&used) && !metadata.contains(&used)
                        && edge_sources(&f.code[pc]).contains(&used);
                    let allowed = construction_operand(&f.code[pc], reg) || forwarded;
                    require(allowed, &format!("undef r{reg} outside construction fields at {pc}: {:?}", f.code[pc]))?;
                }
            }
        }
    }
    suspension::check_structure(p, f)?;
    check_unbound(f)?;
    check_successors(p, f, &pcs)?;
    check_builders(p, f, &pcs)?;
    Ok(())
}

/// Dominance among the blocks of a function whose entry, block 0, has no predecessor (#779): Cooper, Harvey and
/// Kennedy's iterative algorithm over the reverse postorder of the entry's reach, then the entry and exit order of
/// a walk of the dominator tree. A block outside the entry's reach is dominated by itself only, and dominates no
/// other block. Time and memory stay linear in the blocks and edges for each pass, where the bitsets it replaces
/// took blocks² / 64 words per pass.
pub(super) struct Dominance {
    /// Entry and exit order in the dominator tree, `u32::MAX` outside the entry's reach.
    enter: Vec<u32>,
    exit: Vec<u32>,
}

impl Dominance {
    pub(super) fn of(predecessors: &[Vec<usize>]) -> Self {
        let count = predecessors.len();
        let mut successors = vec![Vec::new(); count];
        for (block, sources) in predecessors.iter().enumerate() {
            for &source in sources {
                successors[source].push(block);
            }
        }
        // Reverse postorder of the blocks the entry reaches, by an explicit stack.
        let mut postorder = Vec::with_capacity(count);
        let mut visited = vec![false; count];
        let mut stack = Vec::new();
        if count > 0 {
            visited[0] = true;
            stack.push((0, 0));
        }
        while let Some((block, next)) = stack.last_mut() {
            if let Some(&successor) = successors[*block].get(*next) {
                *next += 1;
                if !visited[successor] {
                    visited[successor] = true;
                    stack.push((successor, 0));
                }
            } else {
                postorder.push(*block);
                stack.pop();
            }
        }
        let mut rank = vec![usize::MAX; count];
        for (position, &block) in postorder.iter().rev().enumerate() {
            rank[block] = position;
        }
        let mut idom = vec![usize::MAX; count];
        if count > 0 {
            idom[0] = 0;
        }
        let mut changed = true;
        while changed {
            changed = false;
            for &block in postorder.iter().rev().skip(1) {
                let mut new = usize::MAX;
                for &source in &predecessors[block] {
                    if idom[source] == usize::MAX {
                        continue;
                    }
                    new = if new == usize::MAX { source } else { Self::intersect(&idom, &rank, source, new) };
                }
                if idom[block] != new {
                    idom[block] = new;
                    changed = true;
                }
            }
        }
        let mut children = vec![Vec::new(); count];
        for &block in postorder.iter().rev().skip(1) {
            children[idom[block]].push(block);
        }
        let (mut enter, mut exit) = (vec![u32::MAX; count], vec![u32::MAX; count]);
        let mut clock = 0u32;
        let mut stack = Vec::new();
        if count > 0 {
            enter[0] = clock;
            stack.push((0, 0));
        }
        while let Some((block, next)) = stack.last_mut() {
            if let Some(&child) = children[*block].get(*next) {
                *next += 1;
                clock += 1;
                enter[child] = clock;
                stack.push((child, 0));
            } else {
                clock += 1;
                exit[*block] = clock;
                stack.pop();
            }
        }
        Self { enter, exit }
    }

    fn intersect(idom: &[usize], rank: &[usize], mut left: usize, mut right: usize) -> usize {
        while left != right {
            while rank[left] > rank[right] {
                left = idom[left];
            }
            while rank[right] > rank[left] {
                right = idom[right];
            }
        }
        left
    }

    /// Whether every path from the entry to `block` passes `dominator`; a block always dominates itself.
    pub(super) fn dominates(&self, dominator: usize, block: usize) -> bool {
        // A block beyond the reach has both orders at u32::MAX: as `block` it fails the exit order.
        dominator == block || self.enter[dominator] != u32::MAX
            && self.enter[dominator] <= self.enter[block] && self.exit[block] <= self.exit[dominator]
    }
}

/// A construction field operand: any operand of obj.new, a non-receiver operand of obj.init/obj.next.
fn construction_operand(inst: &Instruction, reg: u32) -> bool {
    match inst {
        Instruction::Data { op: DataOp::ObjNew, .. } => true,
        Instruction::Data { op: DataOp::ObjInit | DataOp::ObjNext, inputs, .. } => inputs.first() != Some(&reg),
        _ => false,
    }
}

/// Locations an instruction reads as operands, without its edges.
fn operand_reads(inst: &Instruction) -> BTreeSet<Location> {
    match inst {
        Instruction::Data { inputs, .. } | Instruction::Ret(inputs) => {
            inputs.iter().copied().map(Location::Register).collect()
        }
        Instruction::Cond { condition: id, .. } | Instruction::Raise(id) => BTreeSet::from([Location::Register(*id)]),
        Instruction::Champ { arguments, .. } => arguments.iter().filter_map(|argument| match argument {
            Argument::Location(location) => Some(*location),
            _ => None,
        }).collect(),
        _ => BTreeSet::new(),
    }
}

fn edge_sources(inst: &Instruction) -> BTreeSet<Location> {
    allocation::outgoing(inst).into_iter().flat_map(|edge| edge.assignments.iter()).filter_map(|assignment| {
        match assignment.source {
            Source::Location(location) => Some(location),
            Source::ProducedSlot(_) => None,
        }
    }).collect()
}

/// A block parameter that an edge may bind to an undef placeholder is itself possibly unbound: like the
/// placeholder, it is read only as a construction field operand, or forwarded to another parameter.
fn check_unbound(f: &Function) -> Check {
    let mut unbound = f.code.iter().filter_map(|inst| match inst {
        Instruction::Data { op: DataOp::Undef, output: Some(id), .. } => Some(Location::Register(*id)),
        _ => None,
    }).collect::<BTreeSet<_>>();
    let placeholders = unbound.clone();
    loop {
        let mut changed = false;
        for inst in &f.code {
            for edge in allocation::outgoing(inst) {
                for assignment in &edge.assignments {
                    if let Source::Location(source) = assignment.source
                        && unbound.contains(&source)
                    {
                        changed |= unbound.insert(assignment.destination);
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    for (pc, inst) in f.code.iter().enumerate() {
        for location in operand_reads(inst) {
            let Location::Register(reg) = location else { continue };
            if unbound.contains(&location) && !placeholders.contains(&location) {
                enforce(construction_operand(inst, reg), "UNBOUND_READ",
                    &format!("possibly unbound r{reg} read outside construction fields at {pc}"))?;
            }
        }
    }
    Ok(())
}

fn builder_transition(op: Opcode) -> bool {
    matches!(op, Opcode::BuilderPush | Opcode::BuilderAppend | Opcode::BuilderRange | Opcode::BuilderFinish
        | Opcode::BuilderAbandon)
}

/// A builder is unique: `builder.new` or a transition outcome creates it, exactly one transition or edge
/// consumes it on every reachable path, and nothing else reads it (`BUILDER_LINEAR`).
fn check_builders(p: &Program, f: &Function, pcs: &BTreeMap<u32, usize>) -> Check {
    let builder = |location| matches!(loc_type(f, location),
        Ok(ProducedType::Value(Type::Builder(_) | Type::MapBuilder(_))));
    let linear = |condition: bool, message: &str| enforce(condition, "BUILDER_LINEAR", message);
    linear(!f.params.iter().any(|&param| builder(param))
        && !f.results.iter().any(|ty| matches!(ty, Type::Builder(_) | Type::MapBuilder(_))),
        "a builder cannot cross an entry")?;
    let mut incoming: Vec<Option<BTreeSet<Location>>> = vec![None; f.blocks.len()];
    incoming[0] = Some(BTreeSet::new());
    let mut stored = 0usize;
    let mut work = vec![0];
    while let Some(index) = work.pop() {
        let mut active = incoming[index].as_ref().expect("queued block").clone();
        let block = &f.blocks[index];
        let end = f.blocks.get(index + 1).map_or(f.code.len(), |block| block.pc as usize);
        for pc in block.pc as usize..end {
            let inst = &f.code[pc];
            let reads = match inst {
                Instruction::Data { inputs, .. } | Instruction::Ret(inputs) => {
                    inputs.iter().copied().map(Location::Register).collect()
                }
                Instruction::Cond { condition: id, .. } | Instruction::Raise(id) => vec![Location::Register(*id)],
                Instruction::Champ { arguments, .. } => arguments.iter().filter_map(|argument| match argument {
                    Argument::Location(location) => Some(*location),
                    _ => None,
                }).collect(),
                _ => vec![],
            };
            for (position, location) in reads.into_iter().enumerate() {
                if !builder(location) {
                    continue;
                }
                let transition = matches!(inst, Instruction::Champ { op, .. } if builder_transition(*op));
                linear(transition && position == 0, "a builder is read outside its own transition")?;
                linear(active.remove(&location), "a builder is used after its transition")?;
            }
            if let Instruction::Data { op, output: Some(output), .. } = inst
                && builder(Location::Register(*output))
            {
                linear(*op == DataOp::BuilderNew, "only builder.new or a transition creates a builder")?;
                linear(active.insert(Location::Register(*output)), "builder overwritten")?;
            }
            if let Instruction::Champ { flow: Flow::Next(outputs), .. } = inst {
                linear(!outputs.iter().any(|&output| builder(output)), "a builder needs transition outcomes")?;
            }
            let terminal = matches!(inst, Instruction::Ret(_) | Instruction::Raise(_) | Instruction::Unreachable)
                || matches!(inst, Instruction::Champ { op: Opcode::RetOwn | Opcode::RetDefer, .. });
            if terminal {
                linear(active.is_empty(), "a live builder is neither finished nor abandoned")?;
            }
            for (edge, produced) in checked_edges(p, f, inst)? {
                let mut next = active.clone();
                let mut consumed = BTreeSet::new();
                let mut destinations = Vec::new();
                // The builder an outcome produces continues in exactly one location, like a produced service.
                for (slot, ty) in produced.iter().enumerate() {
                    if !matches!(ty, ProducedType::Value(Type::Builder(_) | Type::MapBuilder(_))) {
                        continue;
                    }
                    let uses = edge.assignments.iter()
                        .filter(|assignment| assignment.source == Source::ProducedSlot(slot as u32)).count();
                    linear(uses != 0, "an outcome drops its produced builder")?;
                    linear(uses == 1, "an outcome duplicates its produced builder")?;
                }
                for assignment in &edge.assignments {
                    if !builder(assignment.destination) {
                        continue;
                    }
                    if let Source::Location(source) = assignment.source {
                        linear(active.contains(&source) && consumed.insert(source),
                            "an edge forwards a consumed or duplicated builder")?;
                    }
                    destinations.push(assignment.destination);
                }
                for source in consumed {
                    next.remove(&source);
                }
                for destination in destinations {
                    linear(next.insert(destination), "an edge overwrites a live builder")?;
                }
                let target = pcs[&edge.target_pc];
                if let Some(expected) = &incoming[target] {
                    linear(*expected == next, "inconsistent live builders at a CFG join")?;
                } else {
                    stored = stored.saturating_add(next.len());
                    enforce(stored <= 8 * 1024 * 1024, "LOAD_WORK_BUDGET",
                        "builder storage exceeds verification work budget")?;
                    incoming[target] = Some(next);
                    work.push(target);
                }
            }
        }
    }
    Ok(())
}

/// A permission is used after it was taken, returned or moved into a pending exit.
const SUCCESSOR_TAKEN: &str = "SUCCESSOR_TAKEN";
/// A live permission is lost on a normal path: overwritten, left at a return, or dropped with its exit.
const SUCCESSOR_DROPPED: &str = "SUCCESSOR_DROPPED";
/// A permission appears from nothing or is duplicated.
const SUCCESSOR_FORGED: &str = "SUCCESSOR_FORGED";
/// Paths that join disagree on the live permissions.
const SUCCESSOR_JOIN: &str = "SUCCESSOR_JOIN";

/// What happens to the successor permission held by an exit service that `op` consumes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Carry {
    /// The produced result slot of this outcome continues the same exit and holds the permission.
    Slot(usize),
    /// The exit ends on a failure path: must-use is a normal-path promise, so the permission may lapse.
    Lapses,
    /// exit.commit hands the permission to its successor-typed value result.
    Commit,
    /// Nothing continues the exit on this outcome: the permission would be lost on a normal path.
    Lost,
}

/// The exit protocol threads one pending exit through its cleanup: exit.prepare/extend/replace produce the
/// pending service, cleanup.begin turns it into the cleanup state, which every cleanup step consumes and
/// produces again, cleanup.finish leaves it in the exit state, and exit.dispatch returns it as the pending
/// service that exit.commit completes.
pub(super) fn exit_carry(op: Opcode, tag: Tag) -> Carry {
    use Opcode::*;
    match (op, tag) {
        // A rehome puts a cache duty into the reserve; the reserve continues the same exit.
        // cleanup.deliver_cancel gives the cleanup state back (D18; #741's async successor body).
        (ExitExtend | CleanupBegin | CleanupSettle | CleanupPlan | CleanupFailure | DestroyDirect
            | CleanupDestroy | CacheRehome | CleanupSettleDynamic | CleanupDeliverCancel, Tag::Normal) => {
            Carry::Slot(0)
        }
        (CleanupNext, Tag::Selected | Tag::Done) => Carry::Slot(0),
        (CleanupFinish, Tag::Normal) => Carry::Slot(2),
        (ExitDispatch, Tag::Pending) => Carry::Slot(1),
        (ExitDispatch, Tag::Failure) | (ExitAbort, Tag::Normal) => Carry::Lapses,
        (ExitCommit, Tag::Normal) => Carry::Commit,
        // exit.replace replaces the pending value: a permission the exit held would be lost with it.
        _ => Carry::Lost,
    }
}

/// A value of this type carries a successor's must-use permission: an object successor or a role successor.
pub(crate) fn holds_permission(ty: Type) -> bool {
    matches!(ty, Type::Successor(_) | Type::RoleSuccessor(_))
}

/// Register operands of `op` whose successor permission moves away from the register: returned values of
/// ret.own/ret.defer, the value of exit.prepare and the replacement value of exit.replace (into the exit).
fn successor_operands(f: &Function, op: Opcode, arguments: &[Argument]) -> Vec<Location> {
    // frame.save moves a saved permission into the frame (its resume produces it again); fut.publish into the
    // Future's outcome.
    let moves = matches!(op, Opcode::RetOwn | Opcode::RetDefer | Opcode::ExitPrepare | Opcode::ExitReplace
        | Opcode::FrameSave | Opcode::FuturePublish);
    arguments.iter().filter_map(|argument| match argument {
        Argument::Location(location @ Location::Register(_)) if moves => Some(*location),
        _ => None,
    }).filter(|&location| matches!(loc_type(f, location), Ok(ProducedType::Value(ty)) if holds_permission(ty)))
        .collect()
}

/// Successor references are copyable, but the independent must-use permission
/// must be taken or returned exactly once on every reachable normal path. Edge renames
/// consume all sources before creating destinations, including cyclic moves.
/// The active set holds successor registers and the exit services that hold a permission moved into
/// an exit: it travels with the exit's linear service until exit.commit hands it to the returned value.
fn check_successors(p: &Program, f: &Function, pcs: &BTreeMap<u32, usize>) -> Check {
    let successor = |location| matches!(loc_type(f, location), Ok(ProducedType::Value(ty)) if holds_permission(ty));
    let mut incoming: Vec<Option<BTreeSet<Location>>> = vec![None; f.blocks.len()];
    incoming[0] = Some(f.params.iter().copied().filter(|&param| successor(param)).collect());
    let mut stored = incoming[0].as_ref().map_or(0, BTreeSet::len);
    let mut work = vec![0];
    while let Some(index) = work.pop() {
        let mut active = incoming[index].as_ref().expect("queued block").clone();
        let block = &f.blocks[index];
        let end = f.blocks.get(index + 1).map_or(f.code.len(), |block| block.pc as usize);
        for pc in block.pc as usize..end {
            let inst = &f.code[pc];
            if let Instruction::Data { op, inputs, output, .. } = inst {
                if *op == DataOp::Move && inputs.first().is_some_and(|&id| successor(Location::Register(id))) {
                    enforce(active.remove(&Location::Register(inputs[0])), SUCCESSOR_TAKEN,
                        "successor permission already taken")?;
                }
                if let Some(dst) = output {
                    let location = Location::Register(*dst);
                    if successor(location) {
                        enforce(matches!(op, DataOp::ObjNext | DataOp::Move), SUCCESSOR_FORGED,
                            "invalid successor producer")?;
                        enforce(active.insert(location), SUCCESSOR_DROPPED, "successor permission overwritten")?;
                    }
                }
            }
            if let Instruction::Ret(values) = inst {
                for &value in values {
                    let location = Location::Register(value);
                    if successor(location) {
                        enforce(active.remove(&location), SUCCESSOR_TAKEN, "successor returned twice or after take")?;
                    }
                }
                enforce(active.is_empty(), SUCCESSOR_DROPPED, "return loses successor must-use permission")?;
            }
            if matches!(inst, Instruction::Unreachable) {
                enforce(active.is_empty(), SUCCESSOR_DROPPED, "unreachable loses successor must-use permission")?;
            }
            // Whether this instruction consumes an exit service that holds a permission.
            let mut carried = false;
            if let Instruction::Champ { op, arguments, flow } = inst {
                let moved = successor_operands(f, *op, arguments);
                for &location in &moved {
                    enforce(active.remove(&location), SUCCESSOR_TAKEN,
                        &format!("{} uses a taken successor permission", op.name()))?;
                }
                for location in load::consumed(f, *op, arguments) {
                    if matches!(location, Location::Service(_)) && active.remove(&location) {
                        // One continuation cannot hold two permissions.
                        enforce(!carried, SUCCESSOR_DROPPED, "one operation consumes two successor-holding exits")?;
                        carried = true;
                    }
                }
                // The operand shapes give exit.prepare/exit.replace at most one value register.
                let entering = !moved.is_empty() && matches!(op, Opcode::ExitPrepare | Opcode::ExitReplace);
                if matches!(op, Opcode::RetOwn | Opcode::RetDefer) {
                    enforce(active.is_empty(), SUCCESSOR_DROPPED,
                        &format!("{} loses successor must-use permission", op.name()))?;
                }
                if let Flow::Next(outputs) = flow {
                    // Without a held permission there is nothing to carry: Lapses is a no-op.
                    // The state form of cache.rehome gives the state back before the reserve.
                    let carry = match (*op, outputs.len()) {
                        _ if !carried => Carry::Lapses,
                        (Opcode::CacheRehome, 2) => Carry::Slot(1),
                        _ => exit_carry(*op, Tag::Normal),
                    };
                    let committed = match carry {
                        Carry::Commit => outputs.first().copied().filter(|&location| successor(location)),
                        _ => None,
                    };
                    enforce(carry != Carry::Commit || committed.is_some(), SUCCESSOR_DROPPED,
                        "exit.commit stores a successor-holding exit value in a non-successor register")?;
                    enforce(carry != Carry::Lost, SUCCESSOR_DROPPED,
                        &format!("{} drops the successor held by its exit", op.name()))?;
                    for (slot, &location) in outputs.iter().enumerate() {
                        // The pending service (slot 0) of exit.prepare/exit.replace receives the value's permission.
                        let holds = entering && slot == 0 || matches!(carry, Carry::Slot(held) if held == slot);
                        if successor(location) {
                            enforce(*op != Opcode::ExitCommit || committed == Some(location), SUCCESSOR_FORGED,
                                "exit.commit produces a successor from an exit that holds none")?;
                        } else if !holds {
                            continue;
                        }
                        enforce(active.insert(location), SUCCESSOR_DROPPED, "successor permission overwritten")?;
                    }
                }
            }
            let tags = match inst {
                Instruction::Champ { flow: Flow::Outcomes(outcomes), .. } => {
                    outcomes.iter().map(|outcome| Some(outcome.tag)).collect()
                }
                _ => vec![],
            };
            for (ordinal, (edge, produced)) in checked_edges(p, f, inst)?.into_iter().enumerate() {
                let carry = match (inst, tags.get(ordinal).copied().flatten()) {
                    (Instruction::Champ { op, .. }, Some(tag)) if carried => exit_carry(*op, tag),
                    _ => Carry::Lapses,
                };
                enforce(!matches!(carry, Carry::Lost | Carry::Commit), SUCCESSOR_DROPPED,
                    "an outcome drops the successor held by its exit")?;
                for (slot, ty) in produced.iter().enumerate() {
                    let holds = matches!(ty, ProducedType::Value(ty) if holds_permission(*ty))
                        || matches!(carry, Carry::Slot(held) if held == slot);
                    if !holds {
                        continue;
                    }
                    let uses = edge.assignments.iter()
                        .filter(|assignment| assignment.source == Source::ProducedSlot(slot as u32)).count();
                    enforce(uses != 0, SUCCESSOR_DROPPED, "an outcome drops a produced successor permission")?;
                    enforce(uses == 1, SUCCESSOR_FORGED, "an outcome duplicates a produced successor permission")?;
                }
                let mut next = active.clone();
                let mut consumed = BTreeSet::new();
                let mut destinations = Vec::new();
                for assignment in &edge.assignments {
                    let tracked = match assignment.source {
                        Source::Location(source @ Location::Service(_)) => active.contains(&source),
                        Source::Location(Location::Register(_)) => successor(assignment.destination),
                        Source::ProducedSlot(slot) => successor(assignment.destination)
                            || matches!(carry, Carry::Slot(held) if held == slot as usize),
                    };
                    if !tracked {
                        continue;
                    }
                    if let Source::Location(source) = assignment.source {
                        enforce(active.contains(&source), SUCCESSOR_TAKEN, "edge forwards a taken successor")?;
                        enforce(consumed.insert(source), SUCCESSOR_FORGED, "edge duplicates successor permission")?;
                    }
                    destinations.push(assignment.destination);
                }
                for source in consumed {
                    next.remove(&source);
                }
                for destination in destinations {
                    enforce(next.insert(destination), SUCCESSOR_DROPPED, "edge overwrites live successor permission")?;
                }
                let target = pcs[&edge.target_pc];
                if let Some(expected) = &incoming[target] {
                    enforce(*expected == next, SUCCESSOR_JOIN, "inconsistent successor permissions at CFG join")?;
                } else {
                    stored = stored.saturating_add(next.len());
                    enforce(stored <= 8 * 1024 * 1024, "LOAD_WORK_BUDGET",
                        "successor permission storage exceeds verification work budget")?;
                    incoming[target] = Some(next);
                    work.push(target);
                }
            }
        }
    }
    Ok(())
}
