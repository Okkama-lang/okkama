//! Fuzz rejection policy: language errors are ignored, compiler failures crash.
#[path = "../../../fuzz/fuzz_targets/support/lowering.rs"]
mod lowering;
#[path = "../../../fuzz/fuzz_targets/support/linking.rs"]
mod linking;
use lowering as support;
use okkama_compiler::{SourceFile, codegen, lowering::{self as compiler, LoweringError}, syntax::{self, ParseOptions, prune}};

#[test]
fn valid_seed_and_language_errors_are_distinguished() {
    for (text, accepted) in [(include_str!("../../../fuzz/seeds/sum.ok"), true),
                             ("module app; main() is int { unknown; }", false)] {
        let source = SourceFile::new("fuzz.ok", text).unwrap();
        let ast = syntax::parse(&source, &ParseOptions::default()).unwrap();
        assert_eq!(support::accepted(compiler::lower_program(&ast)).is_some(), accepted);
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

/// The parsed program of a fuzz input, as the source targets see it.
fn program(data: &[u8]) -> okkama_compiler::syntax::ast::Program {
    let source = SourceFile::new("fuzz.ok", std::str::from_utf8(data).unwrap()).unwrap();
    prune::strip_test_items(syntax::parse(&source, &ParseOptions::default()).unwrap())
}

/// The `analyze` target on one input: lowering ends in OIR or a language error.
fn analyze(data: &[u8]) -> Result<okkama_compiler::ir::Module, LoweringError> {
    let result = compiler::lower_program(&program(data));
    if let Ok(module) = &result {
        okkama_compiler::ir::verify(module).expect("lowering produced invalid OIR");
    }
    if let Err(error) = &result {
        assert!(!matches!(error, LoweringError::Internal { .. }), "{error}");
    }
    result
}

/// The `exec` target up to the run: whatever linking accepts, codegen generates.
fn exec(data: &[u8]) -> bool {
    let Some(module) = linking::linked(&program(data)) else { return false };
    let program = codegen::sparrow::generate(&module).expect("codegen of linked OIR");
    okkama_sparrow::encode(&program).expect("encoding generated SPB");
    true
}

/// #1088 (fuzz analyze): std/async/barrier.ok with its BrokenBarrier enum turned
/// into a comment. The runtime throws that enum's first case from Barrier.wait,
/// so the module is refused with a diagnostic at the intrinsic, not an internal error.
#[test]
fn a_missing_intrinsic_error_enum_is_a_diagnostic() {
    let data = include_bytes!("../../../fuzz/regressions/analyze/crash-9591cd338d38c4d874f2495dec6c24d96ceca346");
    let error = analyze(data).unwrap_err();
    assert!(matches!(error, LoweringError::Invalid { .. }), "{error:?}");
    assert!(error.message().contains("BrokenBarrier.broken"), "{}", error.message());
    assert!(!exec(data), "the exec target refuses it the same way");
    let text = std::str::from_utf8(data).unwrap();
    let at = &text[error.span().start as usize..];
    assert!(at.starts_with("async wait()"), "{at:?}");
    // The same module with the enum declared again lowers.
    let fixed = text.replace("cancelled.\u{2}enum", "cancelled.\nenum");
    assert!(analyze(fixed.as_bytes()).is_ok());
}

/// #1089 (fuzz exec): an import nothing uses. Unlinked OIR keeps it, so the exec
/// target links first: the import is refused by the loader, and the same module
/// without it compiles to SPB.
#[test]
fn an_import_is_refused_before_codegen() {
    let data = include_bytes!("../../../fuzz/regressions/exec/crash-4f466cbacd07a532a68b9c56264884e764aa19a9");
    assert!(analyze(data).is_ok_and(|module| module.imports.len() == 1), "unlinked OIR keeps the import");
    assert!(!exec(data));
    let text = std::str::from_utf8(data).unwrap().replace("import \"b\";", "");
    assert!(exec(text.as_bytes()));
    assert!(exec(include_bytes!("../../../fuzz/seeds/sum.ok")));
}
