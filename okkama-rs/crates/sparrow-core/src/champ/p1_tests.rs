//! The failure contract of synchronous natives: whatever a provider refuses with, the entry fails with a
//! native failure, the only kind its mask (`NATIVE_FAILURES`) admits.
use super::*;
use crate::runtime::{FAILED, OK};

const NAME: &str = "runtime/hex@1::hex_encode/1";

/// Settle a provider refusal of `kind` and return the resulting failure.
fn settled(kind: FailureKind) -> (Box<Runtime>, Word) {
    let mut runtime = Runtime::new_champ(s::Program::default(), Vec::new()).unwrap();
    let failure = runtime.failure_new(kind);
    let mut ctx = Ctx { status: FAILED, stack_limit: 0, failure, runtime: &mut *runtime, entry_frame: 0, scratch: 0 };
    native_refusal(&mut ctx, NAME);
    assert_eq!(ctx.status, FAILED);
    let failure = ctx.failure;
    (runtime, failure)
}

/// The operation and message of a `NativeFailure`.
fn native(runtime: &Runtime, failure: Word) -> (String, String) {
    let kind = &runtime.failure(failure).unwrap().kind;
    let FailureKind::Native(object) = *kind else { panic!("not a native failure: {kind:?}") };
    let field = |index| runtime.heap.text(runtime.heap.field(object, index)).to_owned();
    assert_eq!((field(0), field(1)), ("runtime".to_owned(), "native".to_owned()));
    (field(2), field(3))
}

#[test]
fn every_refusal_of_a_sync_native_is_a_native_failure() {
    for (kind, message) in [
        (FailureKind::Program("bad input".into()), "bad input"),
        (FailureKind::Cancelled, "Operation was cancelled"),
        (FailureKind::Panic("provider broke".into()), "provider broke"),
    ] {
        let (runtime, settled) = settled(kind);
        assert_eq!(native(&runtime, settled), (NAME.to_owned(), message.to_owned()));
    }
}

#[test]
fn a_native_failure_of_the_provider_passes_unchanged() {
    let (mut runtime, first) = settled(FailureKind::Program("first".into()));
    let FailureKind::Native(object) = runtime.failure(first).unwrap().kind else { panic!("native failure") };
    let failure = runtime.failure_new(FailureKind::Native(object));
    let mut ctx = Ctx { status: FAILED, stack_limit: 0, failure, runtime: &mut *runtime, entry_frame: 0, scratch: 0 };
    native_refusal(&mut ctx, "runtime/other@1::other/0");
    assert_eq!(ctx.failure, failure);
    assert!(matches!(runtime.failure(failure).unwrap().kind, FailureKind::Native(word) if word == object));
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
