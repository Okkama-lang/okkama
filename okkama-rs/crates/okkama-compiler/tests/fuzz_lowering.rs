//! Fuzz rejection policy: language errors are ignored, compiler failures crash.
#[path = "../../../fuzz/fuzz_targets/support/lowering.rs"]
mod lowering;
#[path = "../../../fuzz/fuzz_targets/support/linking.rs"]
mod linking;
use lowering as support;
use okkama_compiler::{SourceFile, lowering::{self as compiler, LoweringError}, syntax::{self, ParseOptions, prune}};
use sparrow_core::budget::{Counted, counted_bytes, set_memory_probe};

// The exec target's memory budget reads this allocator, as in the target.
#[global_allocator]
static ALLOCATOR: Counted<std::alloc::System> = Counted(std::alloc::System);

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

/// The `exec` target on one input: `None` when it is not a program, else the run.
fn run(data: &[u8]) -> Option<Result<i64, sparrow_core::Error>> {
    set_memory_probe(counted_bytes);
    linking::run(data)
}

/// Whether the `exec` target runs the input: linking accepted it, codegen generated it.
fn exec(data: &[u8]) -> bool {
    run(data).is_some()
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

/// #1091: the exec target links against the repository's std tree, so std's
/// async intrinsics run under the target's budgets; a host native stays refused.
#[test]
fn a_program_importing_std_runs_under_the_exec_budgets() {
    let atomic = "module app;\nimport \"async/atomic\" take Atomic, AtomicClosed;\n\
        main() is int { let cell = Atomic(41); cell.replace(cell.value() + 1); let seen = cell.value(); cell.close();\n\
        let after = try { cell.value(); } catch error is AtomicClosed { -1; }; seen + after + 1; }";
    assert_eq!(run(atomic.as_bytes()).map(|outcome| outcome.map_err(|error| error.0)), Some(Ok(42)));
    let channel = "module app;\nimport \"async/channel\" take Channel, ChannelClosed;\n\
        async main() is int { let channel is Channel<int> = Channel(capacity: 2); let sending = channel.sending();\n\
        await sending.send(20); await sending.send(22); sending.close(); let receiving = channel.receiving();\n\
        let sum = await receiving.item() + await receiving.item();\n\
        let after = try { await receiving.item(); } catch error is ChannelClosed { 0; }; sum + after; }";
    assert_eq!(run(channel.as_bytes()).map(|outcome| outcome.map_err(|error| error.0)), Some(Ok(42)));
    let host = "module app;\nimport \"os/environment\" take Environment;\nmain() is int { Environment(); 0; }";
    assert!(run(host.as_bytes()).is_some_and(|outcome| outcome.is_err()), "the environment is a host native");
    // Only std: a module path outside it, or out of the tree, is not found.
    for path in ["b", "../std/async/atomic", "/etc/hosts", "async//atomic", "async/./atomic"] {
        assert!(run(format!("module app;\nimport \"{path}\";\nmain() is int {{ 0; }}").as_bytes()).is_none(), "{path}");
    }
    // The same program with a std module it does not use runs.
    for path in ["async/atomic", "async/atomic.ok"] {
        let text = format!("module app;\nimport \"{path}\";\nmain() is int {{ 0; }}");
        assert_eq!(run(text.as_bytes()).map(|outcome| outcome.map_err(|error| error.0)), Some(Ok(0)), "{path}");
    }
}

/// #1098 (fuzz exec): std/runtime/time_zone.ok mutated to natives only, one of
/// them, `timezone_local_candidates`, with a `bool` parameter and a `rat` result
/// where the native table has `int`. Codegen refused its own SPB; lowering now
/// refuses the declaration, so linking accepts only what codegen generates.
#[test]
fn a_native_unlike_the_table_is_a_diagnostic() {
    let data = include_bytes!("../../../fuzz/regressions/exec/crash-02628ffef0d82fc725199af58600524d7026a85a");
    let error = analyze(data).unwrap_err();
    assert!(matches!(error, LoweringError::Invalid { .. }), "{error:?}");
    assert!(error.message().contains("Native \"timezone_local_candidates\" does not match \
        runtime/time_zone@1::timezone_local_candidates/8 of the runtime, which is (string, int, int, int, int, int, int, int) -> int"),
        "{}", error.message());
    let text = std::str::from_utf8(data).unwrap();
    assert!(text[error.span().start as usize..].starts_with("native timezone_local_candidates("));
    assert!(!exec(data));
    // std's own module, natives only and no main, links and generates.
    let std = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../std/runtime/time_zone.ok")).unwrap();
    assert!(exec(&std));
    // A native the table does not name compiles; the run refuses it, never codegen.
    let unknown = "module app;\nnative nothing_here() is int;\nmain() is int { nothing_here(); }";
    assert!(run(unknown.as_bytes()).is_some_and(|outcome| outcome.is_err()), "an unknown native fails the run");
}

/// #1123: the `exec` target's differential mode (`OKKAMA_FUZZ_DIFFERENTIAL=1`) runs a program on the JIT and on
/// the interpreter and requires one observation; the design's divergences pass with their reason.
#[test]
fn the_differential_mode_compares_the_engines_and_passes_the_known_divergences() {
    set_memory_probe(counted_bytes);
    let same = [
        include_str!("../../../fuzz/seeds/sum.ok"),
        "module app;\nenum Oops is Error = (code(n is int));\nobject Thrower {\n    Thrower();\n    \
         raise(n is int) is int { if n > 2 { throw Oops.code(n); } n; }\n}\nmain() is int {\n    \
         let thrower = Thrower();\n    let a = try { thrower.raise(5); } catch Oops.code(n) { n + 1; };\n    a + thrower.raise(9);\n}\n",
        "module app;\nmain() is int { let xs = [1, 2, 3]; xs[5]; }\n",
    ];
    for text in same {
        assert_eq!(linking::differ(text.as_bytes()), Some(linking::Difference::Same), "{text}");
    }
    let deep = "module app;\nobject Down {\n    Down();\n    go(n is int) is int { if n < 0 { 0; } else { 1 + self.go(n + 1); } }\n}\nmain() is int { Down().go(0); }\n";
    let found = linking::differ(deep.as_bytes());
    assert!(matches!(found, Some(linking::Difference::Excepted(reason)) if reason.contains("6.3")), "{found:?}");
    let not_a_program = b"module app; main() is int { unknown; }";
    assert_eq!(linking::differ(not_a_program), None);
}
