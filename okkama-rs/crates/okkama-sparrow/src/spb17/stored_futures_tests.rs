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

/// Review of D25 (F2): an array may hold Futures, a map may not (`MAP_PLAIN`): the runtime walk of a map selection
/// does not enter Future values, so their entries would stay in the closing scope while the static model believes
/// the selection took them.
#[test]
fn a_map_value_is_never_a_future() {
    let mut p = with(holder(vec![FUTURE_INT], vec![0], true));
    p.composites.push(Composite::Map { key: Type::Int, value: Type::Str });
    verify_load(&p, &NoExternalContracts).unwrap();
    p.composites.pop();
    p.composites.push(Composite::Map { key: Type::Int, value: FUTURE_INT });
    reject_with(&p, "MAP_PLAIN", "map value is a Future");
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

// --- PR-B2 (#782): an async body whose start value can bring service entries -----------------------------------

/// `async watch(w: Wrapper) is int { 0; }` where `Wrapper` (object 0) stores a Future: its start packet may carry
/// the Future's entry (`entry_carries`), so the close of its scope settles dynamically (`dynamic`) and plans
/// through the executor (`merge`). `bare` gives it a bare `Future<int>` start value instead, which brings no entry.
fn watch(dynamic: Option<super::async_tests::Dynamic>, merge: bool, bare: bool) -> Program {
    use super::async_tests::{l, p, program, service, value, Body, WORK};
    use Argument as A;
    use Opcode as O;
    use ServiceKind as K;
    let input = if bare { FUTURE_INT } else { Type::Object(0) };
    let (mut b, frame) = Body::asynchronous("watch", 0, vec![Type::Int]);
    if let Some(dynamic) = dynamic {
        b.dynamic.insert(WORK, dynamic);
    }
    b.block("entry", &[frame]);
    let (body, start) = b.resume(frame, 0, &[ProducedType::Value(input), ProducedType::Service(K::Packet)]);
    let [_, inputs] = start[..] else { unreachable!() };
    let q0 = b.emit(O::ScopeBegin, vec![A::Scope(WORK), A::Boundary(WORK)], K::ScopeState);
    let q1 = b.emit(O::OwnAccept, vec![l(q0), l(inputs)], K::ScopeState);
    let zero = b.constant(Constant::Int("0".into()), Type::Int);
    let empty = b.emit(O::OwnEmpty, vec![], K::Packet);
    let selection = b.selection();
    let pending = b.emit(O::ExitPrepare, vec![A::Kind(ExitKind::Return), A::Boundary(WORK), l(zero), l(empty),
        A::Selection(selection)], K::Pending);
    let finished = b.close(q1, Some(pending), None, WORK, &[], 0, merge);
    let [closed, reserved] = [b.service(K::Closed), b.service(K::Pending)];
    b.dispatch(finished, [true, false, false], vec![(Tag::Pending, "done", vec![p(0), p(1)])]);
    b.block("done", &[closed, reserved]);
    let [result, packet, deps] = [b.register(Type::Int), b.service(K::Packet), b.service(K::Deps)];
    b.next(O::ExitCommit, vec![l(reserved), A::Destination(ExitDestination::Caller)], vec![result, packet, deps]);
    b.next(O::FuturePublish, vec![l(body), A::Flag(true), l(result), l(packet), l(deps)], vec![]);
    b.ret();
    let layout = FrameLayout {
        slots: vec![value(input), service(K::Packet)],
        continuations: vec![Continuation { site: None, saved: vec![0, 1], operands: vec![] }],
    };
    let mut p = program(vec![b.finish()], vec![layout], None);
    p.objects.push(holder(vec![FUTURE_INT], vec![0], true));
    p
}

/// A Wrapper start value makes the start packet a carrier: the close settles dynamically and merges.
#[test]
fn a_body_whose_start_value_reaches_a_future_settles_dynamically() {
    use super::async_tests::Dynamic;
    verify_load(&watch(Some(Dynamic::AfterStatic), true, false), &NoExternalContracts).unwrap();
    reject_with(&watch(None, true, false), suspension::SCHEDULE, "settles them dynamically first");
}

/// The inventory of such a body is not known to be empty, so a direct plan is refused.
#[test]
fn a_body_whose_start_value_reaches_a_future_plans_through_the_executor() {
    use super::async_tests::Dynamic;
    reject_with(&watch(Some(Dynamic::AfterStatic), false, false), "SPB_STRUCTURE",
        "direct plan loses owner inventory");
}

/// A bare Future start value is borrowed: the body's inputs stay plain, so a direct plan without a dynamic step
/// loads (the 42 hand-built V8/R5 programs rely on it).
#[test]
fn a_bare_future_start_value_keeps_the_inputs_plain() {
    verify_load(&watch(None, false, true), &NoExternalContracts).unwrap();
    assert!(evidence::plain_inputs(&watch(None, false, true), &watch(None, false, true).functions[0]));
    let wrapper = watch(None, false, false);
    assert!(!evidence::plain_inputs(&wrapper, &wrapper.functions[0]));
}

/// PR-A (D25 tail): an Atomic over an array of Futures, with `selected` and `reaches` as its layout says.
fn borrowing_atomic(selected: Vec<u32>, reaches: bool) -> Object {
    let mut object = holder(vec![FUTURE_ARRAY, Type::Bool], selected, reaches);
    object.name = "async.atomic.Atomic_t_array<future<int>>".into();
    object
}

/// PR-A: an Atomic borrows the Futures it stores. Its value word is traced and never selected, and it reaches no
/// resource (`ATOMIC_BORROW`), although a Future field of any other object is selected (`SPB_FUTURE_TYPES`).
#[test]
fn an_atomic_borrows_the_futures_it_stores() {
    verify_load(&with(borrowing_atomic(vec![], false)), &NoExternalContracts).unwrap();
}

/// PR-A, I-A2: an Atomic that borrows Futures neither reaches a resource nor selects its value word.
#[test]
fn an_atomic_over_futures_that_reaches_or_selects_is_refused() {
    for (selected, reaches) in [(vec![], true), (vec![0], true), (vec![0], false)] {
        reject_with(&with(borrowing_atomic(selected.clone(), reaches)), "ATOMIC_BORROW",
            "an Atomic that borrows Futures reaches a resource or selects its value");
    }
}

/// PR-A, I-A1: the P1 entries of an Atomic over Futures are observations (no packet transition has the Atomic as its
/// source or destination), and there is no `compare_and_replace` (Futures do not compare). An Atomic over ints keeps
/// its owned publication.
#[test]
fn the_entries_of_an_atomic_over_futures_are_observations() {
    let plain = |name: &str, storage: Vec<Type>| {
        let mut object = holder(storage, vec![], false);
        object.name = name.into();
        object.fields.clear();
        object
    };
    let mut closed = plain("async.atomic.AtomicClosed", vec![Type::Int]);
    closed.error_cases = vec!["AtomicClosed.closed".into()];
    let futures = plain("async.atomic.Atomic_t_array<future<int>>", vec![FUTURE_ARRAY, Type::Bool]);
    let ints = plain("async.atomic.Atomic_int", vec![Type::Int, Type::Bool]);
    let contracts = super::p1::P1Contracts::new(&[closed, futures, ints]).unwrap();
    let abi = |name: &str| contracts.entries().iter().find(|entry| entry.symbol == name).map(|entry| entry.entry_abi.clone());
    for op in ["new_value", "value", "replace", "closed", "close"] {
        assert_eq!(abi(&format!("okkama.atomic.Atomic.{op}$t_array<future<int>>")), Some(EntryAbi::Observation), "{op}");
    }
    assert_eq!(abi("okkama.atomic.Atomic.compare_and_replace$t_array<future<int>>"), None);
    assert_eq!(abi("okkama.atomic.Atomic.replace$int"), Some(EntryAbi::Owned));
}
