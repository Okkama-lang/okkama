//! #771, #758 (async ABI D25.2, revision of 2026-11-16): Futures stored in object fields and in arrays. A Future
//! field, or a field of an array of Futures (`ArrayLeaf::Future`, wire tag 23), is selected in an object that
//! reaches what its packet carries (`SPB_FUTURE_TYPES`); a value that reaches a Future is no plain input
//! (`CALL_PLAIN_PACKET`); an object that reaches no identity takes no position of a static selection. Each negative
//! is valid up to the rule it names.
use super::*;
use super::async_tests::{owner_program, reject_with, FUTURE_INT};

const FUTURE_ARRAY: Type = Type::Array(ArrayType { leaf: ArrayLeaf::Future(0), depth: 1 });

/// An object without destroy holding `storage`, with `selected` fields selected and `reaches` as its retention.
fn holder(storage: Vec<Type>, selected: Vec<u32>, reaches: bool) -> Object {
    let mut object = super::async_tests::res();
    object.name = "Holder".into();
    let trace = (0..storage.len() as u32).filter(|&index| storage[index as usize].traced()).collect();
    object.layout = ObjectRuntimeLayout {
        allocation_words: 1 + storage.len() as u32, identity_offset: None, has_destroy: false,
        may_reach_resource: reaches, may_async_destroy: false, storage_fields: storage, trace_layout: trace,
        selection_layout: selected,
    };
    object
}

fn with(object: Object) -> Program {
    let mut p = owner_program(true);
    p.objects.push(object);
    p
}

#[test]
fn a_selected_future_field_of_a_reaching_object_loads() {
    verify_load(&with(holder(vec![Type::Str, FUTURE_INT], vec![1], true)), &NoExternalContracts).unwrap();
}

#[test]
fn an_unselected_future_field_is_refused() {
    reject_with(&with(holder(vec![FUTURE_INT], vec![], true)), verify::FUTURE_TYPES, "a Future field is selected");
}

#[test]
fn a_future_field_of_an_object_that_does_not_reach_is_refused() {
    reject_with(&with(holder(vec![FUTURE_INT], vec![0], false)), verify::FUTURE_TYPES, "a Future field is selected");
}

#[test]
fn a_future_array_field_follows_the_future_field_rule() {
    verify_load(&with(holder(vec![FUTURE_ARRAY], vec![0], true)), &NoExternalContracts).unwrap();
    reject_with(&with(holder(vec![FUTURE_ARRAY], vec![], true)), verify::FUTURE_TYPES, "a Future field is selected");
}

#[test]
fn a_future_array_names_a_future_results_entry() {
    let unknown = Type::Array(ArrayType { leaf: ArrayLeaf::Future(9), depth: 1 });
    reject_with(&with(holder(vec![unknown], vec![0], true)), verify::FUTURE_TYPES, "names no future_results entry");
}

#[test]
fn the_future_leaf_is_an_array_element() {
    assert_eq!(ArrayType::of(FUTURE_INT), Some(FUTURE_ARRAY));
    let Type::Array(shape) = FUTURE_ARRAY else { unreachable!() };
    assert_eq!(shape.element(), Some(FUTURE_INT));
    let nested = ArrayType::of(FUTURE_ARRAY).unwrap();
    assert_eq!(nested, Type::Array(ArrayType { leaf: ArrayLeaf::Future(0), depth: 2 }));
}

#[test]
fn the_future_leaf_round_trips() {
    let mut p = with(holder(vec![FUTURE_ARRAY], vec![0], true));
    p.functions[0].registers.push(Type::Array(ArrayType { leaf: ArrayLeaf::Future(1), depth: 2 }));
    let bytes = encode(&p).unwrap();
    assert_eq!(decode(&bytes).unwrap(), p);
}

#[test]
fn a_value_that_reaches_a_future_is_no_plain_input() {
    let p = with(holder(vec![FUTURE_INT], vec![0], true));
    let object = p.objects.len() as u32 - 1;
    let f = |ty: Type| Function {
        symbol: "f".into(), entry_abi: EntryAbi::Owned, registers: vec![ty], services: vec![],
        params: vec![Location::Register(0)], results: vec![], blocks: vec![], proofs: FunctionProofs::default(),
        code: vec![],
    };
    assert!(plain_inputs(&p, &f(Type::Int)));
    assert!(!plain_inputs(&p, &f(FUTURE_INT)));
    assert!(!plain_inputs(&p, &f(FUTURE_ARRAY)));
    assert!(!plain_inputs(&p, &f(Type::Object(object))));
}

#[test]
fn an_object_that_reaches_no_identity_is_identity_free() {
    let mut p = with(holder(vec![FUTURE_INT], vec![0], true));
    let futures = p.objects.len() as u32 - 1;
    assert!(identity_free(&p.objects, Type::Object(futures)));
    p.objects.push(holder(vec![FUTURE_ARRAY, Type::Object(futures)], vec![0, 1], true));
    let nested = p.objects.len() as u32 - 1;
    assert!(identity_free(&p.objects, Type::Object(nested)));
    // `Res` (object 0) has a destroy: it is an identity, and so is a holder that reaches it.
    assert!(!identity_free(&p.objects, Type::Object(0)));
    p.objects.push(holder(vec![FUTURE_INT, Type::Object(0)], vec![0, 1], true));
    assert!(!identity_free(&p.objects, Type::Object(p.objects.len() as u32 - 1)));
    // A map is not known to be free; an unknown declaration neither.
    assert!(!identity_free(&p.objects, Type::Map(0)));
    assert!(!identity_free(&p.objects, Type::Object(99)));
}
