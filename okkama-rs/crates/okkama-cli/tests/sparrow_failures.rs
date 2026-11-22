#[path = "sparrow_support/mod.rs"]
mod support;
use okkama_compiler::{SourceFile, codegen, ir, lowering, syntax};

fn lower(body: &str) -> ir::Module {
    let source = SourceFile::new("failures.ok", format!("module app v1.0.0; object Worker {{ Worker(); fail(message is string) is int {{ panic message; }} }} main() is int {{ {body} }}")).unwrap();
    lowering::lower_program(&syntax::parse(&source, &syntax::ParseOptions::default()).unwrap())
        .unwrap()
}

#[test]
fn panic_preserves_computed_unicode_message_across_calls() {
    let module = lower("Worker().fail(\"ошибка: \" + \"42\");");

    support::assert_panic(&module, "ошибка: 42");
}

#[test]
fn untaken_panic_is_lazy_and_bytecode_message_must_be_a_string() {
    let module = lower("if false { Worker().fail(\"unused\"); } else { 42; }");

    support::assert_value(&module, "42");
    // Проверка SPB 9 типизирована: не строка в `panic` отвергается до
    // исполнения (#16).
    let module = lower("panic \"stop\";");
    let mut invalid = codegen::sparrow::generate(&module).unwrap();
    let main = invalid.entry.unwrap().0 as usize;
    let load = invalid.functions[main]
        .code
        .iter_mut()
        .find(|inst| matches!(inst, okkama_sparrow::Instruction::Const { value: okkama_sparrow::Constant::Str(_), .. }))
        .unwrap();
    let okkama_sparrow::Instruction::Const { value, .. } = load else {
        unreachable!()
    };
    *value = okkama_sparrow::Constant::Bool(false);
    let error = invalid.verify().unwrap_err().to_string();
    assert!(error.contains("str cannot hold the bool result"), "{error}");
}

#[test]
fn unwind_runs_defer_in_reverse_order_and_keeps_primary_failure() {
    let module = lower(
        "defer Worker().fail(\"first\"); defer Worker().fail(\"second\"); Worker().fail(\"primary\");",
    );
    let error = support::report(&module);
    assert_eq!(
        error.to_string(),
        "Panic: primary\nSecondary cleanup failure:\n  Panic: second\nSecondary cleanup failure:\n  Panic: first"
    );
}

#[test]
fn program_errors_unwind_and_panic_bypasses_error_catches() {
    let module = lower("defer Worker().fail(\"cleanup\"); 7 % 0;");
    let error = support::report(&module);
    assert_eq!(
        error.to_string(),
        "RuntimeError: Modulo by zero\nSecondary cleanup failure:\n  Panic: cleanup"
    );
    let module = lower("try { Worker().fail(\"primary\"); } catch e is Error { 42; }");
    support::assert_panic(&module, "primary");

    let module = lower("try { 7 % 0; } catch e is Error { 42; }");
    support::assert_trap(&module, "Modulo by zero");

}

fn typed(body: &str) -> ir::Module {
    let source = SourceFile::new("typed.ok", format!("module app v1.0.0; enum Missing is Error = (code(value is int)); enum Denied is Error = (denied); object Sender {{ Sender(); fail() is int {{ throw Missing.code(42); }} }} main() is int {{ {body} }}")).unwrap();
    lowering::lower_program(&syntax::parse(&source, &syntax::ParseOptions::default()).unwrap())
        .unwrap()
}

#[test]
fn typed_failure_matches_nominal_catches_and_role_references() {
    for body in [
        "try { Sender().fail(); } catch e is Denied { 1; } catch e is Missing { if e.text() == \"Missing.code(42)\" { 42; } else { 2; } }",
        "let problem is Error = Missing.code(42); try { throw problem; 0; } catch e is Missing { 42; }",
        "try { throw Denied.denied; 0; } catch e is Error { 42; } catch e is Denied { 1; }",
        "try { try { throw Missing.code(42); 0; } catch e is Denied { 1; } } catch e is Missing { 42; }",
        "try { try { throw Missing.code(42); 0; } catch e is Missing { throw; 1; } } catch e is Missing { 42; }",
    ] {

        support::assert_value(&typed(body), "42");
    }
}

#[test]
fn uncaught_typed_failure_formats_text_only_at_the_boundary() {
    let error = support::report(&typed("Sender().fail();"));
    assert_eq!(error.to_string(), "Missing: Missing.code(42)");
    let module = typed("defer Sender().fail(); panic \"primary\";");
    assert_eq!(
        support::report(&module),
        "Panic: primary\nSecondary cleanup failure:\n  Missing: Missing.code(42)"
    );
}

#[test]
fn new_failure_keeps_handled_failure_as_its_cause() {
    let module = typed("try { throw Missing.code(42); 0; } catch e is Missing { throw Denied.denied; 0; }");
    assert_eq!(support::report(&module), "Denied: Denied.denied\nCaused by:\n  Missing: Missing.code(42)");
}

#[test]
fn sparrow_unwind_preserves_primary_and_cleanup_failures_across_calls() {
    for (body, expected) in [
        ("defer Worker().fail(\"first\"); defer Worker().fail(\"second\"); Worker().fail(\"primary\");", "Panic: primary\nSecondary cleanup failure:\n  Panic: second\nSecondary cleanup failure:\n  Panic: first"),
        ("defer Worker().fail(\"cleanup\"); 7 % 0;", "RuntimeError: Modulo by zero\nSecondary cleanup failure:\n  Panic: cleanup"),
        ("defer 1 + 2; Worker().fail(\"primary\");", "Panic: primary"),
    ] {
        let module = lower(body);
        assert_eq!(support::report(&module), expected, "{body}");
    }
}

#[test]
fn typed_diagnostics_and_uncatchable_failures() {
    for (body, expected) in [
        ("Sender().fail();", "Missing: Missing.code(42)"),
        ("defer Sender().fail(); panic \"primary\";", "Panic: primary\nSecondary cleanup failure:\n  Missing: Missing.code(42)"),
        ("try { panic \"primary\"; } catch e is Error { 42; }", "Panic: primary"),
        ("try { 7 % 0; } catch e is Error { 42; }", "RuntimeError: Modulo by zero"),
    ] {
        assert_eq!(support::report(&typed(body)), expected, "{body}");
    }
}

/// A cancellation caught by the Error role in a program that never names `Cancelled` (#744).
fn cancelled_as_error() -> ir::Module {
    let source = SourceFile::new("cancelled.ok", r#"module app v1.0.0;
object Job {
    has result is int;
    Job(:result);
    async run() is int { result; }
}
async main() is int {
    let pending = Job(result: 1).run();
    pending.cancel();
    let text = try {
        (await pending).string();
    } catch e is Error {
        "error: " + e.text();
    };
    confirm text == "error: Operation was cancelled";
    0;
}"#).unwrap();
    lowering::lower_program(&syntax::parse(&source, &syntax::ParseOptions::default()).unwrap()).unwrap()
}

/// The catch binds a `Cancelled` value whose text is Tomato's (#744; it was an unbound word and `e.text()`
/// crashed): every catch of the Error role brings the system `Cancelled` object into the program.
#[test]
fn a_cancellation_caught_as_error_binds_cancelled_without_naming_it() {
    let module = cancelled_as_error();
    let program = codegen::sparrow::generate(&module).unwrap();
    assert!(program.objects.iter().any(|object| object.builtin && object.name == "Cancelled"));
    support::assert_value(&module, "0");
}

/// A program without the builtin `Cancelled` object (only a crafted one can lack it) has no value to bind:
/// the Error catch does not take the cancellation, which ends the program as an uncaught cancellation.
#[test]
fn without_the_builtin_cancelled_an_error_catch_leaves_the_cancellation_uncaught() {
    let mut program = codegen::sparrow::generate(&cancelled_as_error()).unwrap();
    let object = program.objects.iter_mut().find(|object| object.builtin && object.name == "Cancelled").unwrap();
    object.builtin = false;
    match support::run_program(&program) {
        support::Ending::Failure { message, .. } => assert_eq!(message, "Operation was cancelled"),
        other => panic!("expected the uncaught cancellation, got {other:?}"),
    }
}
