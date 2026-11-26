//! Fuzz rejection policy: language errors are ignored, compiler failures crash.
#[path = "../../../fuzz/fuzz_targets/support/lowering.rs"]
mod support;
use okkama_compiler::{SourceFile, lowering::{self, LoweringError}, syntax::{self, ParseOptions}};

#[test]
fn valid_seed_and_language_errors_are_distinguished() {
    for (text, accepted) in [(include_str!("../../../fuzz/seeds/sum.ok"), true),
                             ("module app; main() is int { unknown; }", false)] {
        let source = SourceFile::new("fuzz.ok", text).unwrap();
        let ast = syntax::parse(&source, &ParseOptions::default()).unwrap();
        assert_eq!(support::accepted(lowering::lower_program(&ast)).is_some(), accepted);
    }
    let span = okkama_compiler::Span::new(0, 0);
    assert!(support::accepted(Err(LoweringError::Unsupported { span, message: "unsupported".into() })).is_none());
}

#[test]
#[should_panic(expected = "invariant failure")]
fn internal_error_crashes_without_relying_on_message_prefix() {
    let span = okkama_compiler::Span::new(0, 0);
    support::accepted(Err(LoweringError::Internal { span, message: "invariant failure".into() }));
}
