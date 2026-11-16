//! The failure contract of synchronous natives (#719): a provider's `NativeFailure` and a program failure of the
//! call keep their kinds across the entry (its mask, `NATIVE_FAILURES`, admits both); a cancellation passes too;
//! a thrown value or a `Panic` failure is outside the trusted contract and stops the machine.
use super::*;
use crate::runtime::{FAILED, OK};

const NAME: &str = "runtime/hex@1::hex_encode/1";

/// Settle a provider refusal of `kind`: the status after the entry, the failure before it, the failure after.
fn settled(kind: FailureKind) -> (Box<Runtime>, u64, Word, Word) {
    let mut runtime = Runtime::new_champ(s::Program::default(), Vec::new()).unwrap();
    let failure = runtime.failure_new(kind);
    let mut ctx = Ctx { status: FAILED, stack_limit: 0, failure, runtime: &mut *runtime, entry_frame: 0, scratch: 0 };
    native_refusal(&mut ctx, NAME);
    let (status, after) = (ctx.status, ctx.failure);
    (runtime, status, failure, after)
}

#[test]
fn a_program_failure_of_a_sync_native_keeps_its_kind_and_message() {
    for message in ["Native symbol \"app@1::missing/1\" is not registered", "Native x needs the Env permission"] {
        let (runtime, status, before, after) = settled(FailureKind::Program(message.into()));
        assert_eq!((status, after), (FAILED, before));
        assert!(matches!(&runtime.failure(after).unwrap().kind, FailureKind::Program(text) if text == message));
    }
}

#[test]
fn a_cancellation_of_a_sync_native_passes_unchanged() {
    let (runtime, status, before, after) = settled(FailureKind::Cancelled);
    assert_eq!((status, after), (FAILED, before));
    assert!(matches!(runtime.failure(after).unwrap().kind, FailureKind::Cancelled));
}

#[test]
fn a_native_failure_of_the_provider_passes_unchanged() {
    let mut runtime = Runtime::new_champ(s::Program::default(), Vec::new()).unwrap();
    let text = runtime.heap.allocate_text("native");
    let object = runtime.heap.object_new(0, &[text, text, text, text]);
    let failure = runtime.failure_new(FailureKind::Native(object));
    let mut ctx = Ctx { status: FAILED, stack_limit: 0, failure, runtime: &mut *runtime, entry_frame: 0, scratch: 0 };
    native_refusal(&mut ctx, "runtime/other@1::other/0");
    assert_eq!((ctx.status, ctx.failure), (FAILED, failure));
    assert!(matches!(runtime.failure(failure).unwrap().kind, FailureKind::Native(word) if word == object));
}

#[test]
fn a_thrown_value_or_panic_of_a_sync_native_stops_the_machine() {
    let mut runtime = Runtime::new_champ(s::Program::default(), Vec::new()).unwrap();
    let thrown = runtime.heap.object_new(0, &[]);
    for kind in [FailureKind::Thrown(thrown), FailureKind::Panic("provider broke".into())] {
        let failure = runtime.failure_new(kind);
        let mut ctx =
            Ctx { status: FAILED, stack_limit: 0, failure, runtime: &mut *runtime, entry_frame: 0, scratch: 0 };
        native_refusal(&mut ctx, NAME);
        assert!(ctx.status != FAILED && ctx.status != OK, "status {}", ctx.status);
    }
}

#[test]
fn a_successful_native_is_left_alone() {
    let mut runtime = Runtime::new_champ(s::Program::default(), Vec::new()).unwrap();
    let mut ctx = Ctx { status: OK, stack_limit: 0, failure: 0, runtime: &mut *runtime, entry_frame: 0, scratch: 0 };
    native_refusal(&mut ctx, NAME);
    assert_eq!((ctx.status, ctx.failure), (OK, 0));
}

/// Object `name` with payload fields of `types` (one storage word each) and these Error cases.
fn declared(name: &str, types: Vec<s::Type>, error_cases: &[&str]) -> s::Object {
    s::Object {
        name: name.into(), error_cases: error_cases.iter().map(|case| (*case).into()).collect(), enum_cases: 0,
        variant_cases: vec![], handle: false, role: None, implements: vec![], any_box: false, enum_names: vec![],
        fields: if error_cases.len() == 1 {
            (0..types.len() as u32).map(|storage| s::Field { name: format!("f{storage}"), storage, once: false })
                .collect()
        } else {
            vec![]
        },
        layout: s::ObjectRuntimeLayout {
            allocation_words: (1 + types.len() as u32).max(2), identity_offset: None, has_destroy: false,
            may_reach_resource: false, may_async_destroy: false,
            trace_layout: types.iter().enumerate().filter(|(_, ty)| ty.traced()).map(|(index, _)| index as u32)
                .collect(),
            storage_fields: types, selection_layout: vec![],
        },
    }
}

/// A plain enum `L` (0), an Error variant `Inner` (1) of `Inner.named(label)` (2), `Ferr.text(...)` with every
/// scalar payload (3), `Ferr.kind(L)` (4), `Ferr.nested(Inner)` (5), `Ferr.plain` (6) and an Error enum (7).
fn errors() -> Box<Runtime> {
    let objects = vec![
        s::Object { enum_cases: 2, enum_names: vec!["L.input".into(), "L.rows".into()],
            ..declared("app.L", vec![s::Type::Int], &[]) },
        s::Object { variant_cases: vec![2], ..declared("app.Inner", vec![], &[]) },
        declared("app.Inner.named", vec![s::Type::Str], &["Inner.named"]),
        declared("app.Ferr.text", vec![s::Type::Str, s::Type::Int, s::Type::Bool, s::Type::Byte, s::Type::Float,
            s::Type::Rat], &["Ferr.text"]),
        declared("app.Ferr.kind", vec![s::Type::Object(0)], &["Ferr.kind"]),
        declared("app.Ferr.nested", vec![s::Type::Variant(1)], &["Ferr.nested"]),
        declared("app.Ferr.plain", vec![], &["Ferr.plain"]),
        declared("app.Db", vec![s::Type::Int], &["Db.closed", "Db.reset"]),
    ];
    Runtime::new_champ(s::Program { objects, ..s::Program::default() }, Vec::new()).unwrap()
}

fn text_of(runtime: &Runtime, object: Word) -> String {
    let source = runtime.champ.machine.as_ref().unwrap().source.clone();
    error_text(runtime, &source, object)
}

#[test]
fn error_text_writes_every_layout_of_an_error_as_tomato_does() {
    let mut runtime = errors();
    let plain = runtime.heap.object_new(6, &[]);
    assert_eq!(text_of(&runtime, plain), "Ferr.plain");
    let tag = runtime.heap.int(1);
    let reset = runtime.heap.object_new(7, &[tag]);
    assert_eq!(text_of(&runtime, reset), "Db.reset");
    // Strings are Okkama literals: quote, backslash, newline, tab, return, other controls as \u{hex}.
    let fields = [
        runtime.heap.allocate_text("a \"q\"\\\n\t\r\u{1b}\u{7f}é😀"),
        runtime.heap.int(-7),
        1,
        200,
        2.5f64.to_bits(),
        runtime.rational_new(okkama_runtime_core::rat::Rat::from_float(0.5)),
    ];
    let text = runtime.heap.object_new(3, &fields);
    assert_eq!(text_of(&runtime, text),
        "Ferr.text(\"a \\\"q\\\"\\\\\\n\\t\\r\\u{1b}\\u{7f}é😀\", -7, true, 200, 2.5, 1/2)");
    let rows = runtime.heap.int(1);
    let case = runtime.heap.object_new(0, &[rows]);
    let kind = runtime.heap.object_new(4, &[case]);
    assert_eq!(text_of(&runtime, kind), "Ferr.kind(L.rows)");
    let label = runtime.heap.allocate_text("in\"ner");
    let inner = runtime.heap.object_new(2, &[label]);
    let nested = runtime.heap.object_new(5, &[inner]);
    assert_eq!(text_of(&runtime, nested), "Ferr.nested(Inner.named(\"in\\\"ner\"))");
}

/// An Error payload of a plain variant (language spec: enums whose payloads satisfy the same rule) shows the
/// case by the variant's case names, with its own payload, nested variants included (#763): `Shape.dot`,
/// `Shape.rect(2, 3)`, `Box.of(Shape.circle(1))`.
#[test]
fn error_text_shows_a_plain_variant_payload_by_its_case_names() {
    let objects = vec![
        s::Object { variant_cases: vec![1, 2, 3], enum_names: vec!["Shape.dot".into(), "Shape.circle".into(),
            "Shape.rect".into()], ..declared("app.Shape", vec![], &[]) },
        declared("app.Shape.dot", vec![], &[]),
        s::Object { fields: vec![s::Field { name: "r".into(), storage: 0, once: false }],
            ..declared("app.Shape.circle", vec![s::Type::Int], &[]) },
        s::Object { fields: vec![s::Field { name: "w".into(), storage: 0, once: false },
            s::Field { name: "h".into(), storage: 1, once: false }],
            ..declared("app.Shape.rect", vec![s::Type::Int, s::Type::Int], &[]) },
        s::Object { variant_cases: vec![5], enum_names: vec!["Box.of".into()], ..declared("app.Box", vec![], &[]) },
        s::Object { fields: vec![s::Field { name: "shape".into(), storage: 0, once: false }],
            ..declared("app.Box.of", vec![s::Type::Variant(0)], &[]) },
        declared("app.Fault.major", vec![s::Type::Int, s::Type::Variant(0)], &["Fault.major"]),
        declared("app.Fault.boxed", vec![s::Type::Variant(4)], &["Fault.boxed"]),
    ];
    let mut runtime = Runtime::new_champ(s::Program { objects, ..s::Program::default() }, Vec::new()).unwrap();
    let dot = runtime.heap.object_new(1, &[]);
    let one = runtime.heap.int(1);
    let dotted = runtime.heap.object_new(6, &[one, dot]);
    assert_eq!(text_of(&runtime, dotted), "Fault.major(1, Shape.dot)");
    let (two, three) = (runtime.heap.int(2), runtime.heap.int(3));
    let rect = runtime.heap.object_new(3, &[two, three]);
    let major = runtime.heap.object_new(6, &[one, rect]);
    assert_eq!(text_of(&runtime, major), "Fault.major(1, Shape.rect(2, 3))");
    let circle = runtime.heap.object_new(2, &[one]);
    let boxed = runtime.heap.object_new(5, &[circle]);
    let fault = runtime.heap.object_new(7, &[boxed]);
    assert_eq!(text_of(&runtime, fault), "Fault.boxed(Box.of(Shape.circle(1)))");
}

#[test]
fn catch_of_an_error_variant_takes_each_of_its_cases() {
    let mut runtime = errors();
    let label = runtime.heap.allocate_text("x");
    let inner = runtime.heap.object_new(2, &[label]);
    let failure = runtime.failure_new(FailureKind::Thrown(inner));
    let mut ctx = Ctx { status: FAILED, stack_limit: 0, failure, runtime: &mut *runtime, entry_frame: 0, scratch: 0 };
    let catch = |ctx: &mut Ctx, descriptor| super::super::machine::rt_machine_catch(ctx, failure, descriptor);
    // The variant, its case and any Error take it; another declaration does not.
    assert_eq!(catch(&mut ctx, 1), inner);
    assert_eq!(catch(&mut ctx, 2), inner);
    assert_eq!(catch(&mut ctx, u64::MAX), inner);
    assert_eq!(catch(&mut ctx, 6), 0);
    assert_eq!(catch(&mut ctx, 0), 0);
}

/// A cancellation is caught by the system Cancelled and by the Error role (`u64::MAX`), binding a new Cancelled
/// object whose Error text is Tomato's (#724, #738); a catch of another declaration lets it pass, and so does every
/// catch in a program without Cancelled.
#[test]
fn a_cancellation_is_caught_as_cancelled_and_as_error() {
    let mut cancelled = declared("Cancelled", vec![], &[]);
    cancelled.layout.allocation_words = 2;
    let objects = vec![declared("app.Ferr.plain", vec![], &["Ferr.plain"]), cancelled];
    let mut runtime = Runtime::new_champ(s::Program { objects, ..s::Program::default() }, Vec::new()).unwrap();
    let failure = runtime.failure_new(FailureKind::Cancelled);
    let mut ctx = Ctx { status: FAILED, stack_limit: 0, failure, runtime: &mut *runtime, entry_frame: 0, scratch: 0 };
    let catch = |ctx: &mut Ctx, descriptor| super::super::machine::rt_machine_catch(ctx, failure, descriptor);
    assert_eq!(catch(&mut ctx, 0), 0);
    for descriptor in [1, u64::MAX] {
        let object = catch(&mut ctx, descriptor);
        assert_ne!(object, 0);
        assert_eq!(heap::payload(runtime.heap.header_of(object)), 1);
        assert_eq!(text_of(&runtime, object), "Operation was cancelled");
    }
    let mut bare = errors();
    let failure = bare.failure_new(FailureKind::Cancelled);
    let mut ctx = Ctx { status: FAILED, stack_limit: 0, failure, runtime: &mut *bare, entry_frame: 0, scratch: 0 };
    assert_eq!(super::super::machine::rt_machine_catch(&mut ctx, failure, u64::MAX), 0);
}
