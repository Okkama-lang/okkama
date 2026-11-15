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
