//! Fallible scalar operations compiled from source never continue on failure in Champ (#633 F2).
#![cfg(feature = "champ-abi")]

use okkama_compiler::lowering::{LoadedModule, ModuleLoader, RootFile, oir2};
use okkama_compiler::syntax::{self, ParseOptions};
use okkama_compiler::SourceFile;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

struct StdLoader;

impl ModuleLoader for StdLoader {
    fn load(&mut self, path: &str, _: &str) -> Result<Option<LoadedModule>, String> {
        let file = root().join("std").join(format!("{path}.ok"));
        let text = std::fs::read_to_string(&file).map_err(|error| error.to_string())?;
        let source = SourceFile::new(file.to_string_lossy(), text).map_err(|error| error.to_string())?;
        let program = syntax::parse(&source, &ParseOptions::default()).map_err(|error| format!("{error:?}"))?;
        let program = syntax::prune::strip_test_items(program);
        Ok(Some(LoadedModule { id: file.to_string_lossy().into_owned(), source, program }))
    }
}

fn compile(text: &str) -> Result<Vec<u8>, oir2::PipelineError> {
    let source = SourceFile::new("app.ok", text).unwrap();
    let program = syntax::parse(&source, &ParseOptions::default()).unwrap();
    let roots = [RootFile { source: &source, program: &program }];
    oir2::compile_linked_with(&roots, &mut StdLoader, okkama_compiler::oir2::VerifyMode::Structural)
}

/// `body` is the method body after an optional live resource; `call` passes the failing arguments.
fn program(resource: bool, body: &str, call: &str) -> String {
    let (parameter, argument, held) = if resource {
        (", log is Atomic<string>", ", log", "        let r = Res(log);\n")
    } else {
        ("", "", "")
    };
    format!("module app;
import \"async/atomic\" take Atomic;
import \"io/stdout\" take Stdout;
object Res {{
    has log is Atomic<string>;
    Res(log is Atomic<string>) {{
        self.log = log;
    }}
    destroy {{
        log.replace(log.value() + \"D\");
    }}
}}
object Calc {{
    Calc();
    run(a is int, b is int{parameter}) is int {{
{held}{body}
    }}
}}
main() is int {{
    let log = Atomic(\"\");
    defer log.close();
    defer Stdout().writeln(log.value());
    Calc().run({call}{argument});
}}
")
}

fn fails(resource: bool, body: &str, call: &str, message: &str) {
    let text = program(resource, body, call);
    let bytes = compile(&text).unwrap_or_else(|error| panic!("{error}\n{text}"));
    let limits = sparrow_core::budget::Limits { steps: Some(100_000), ..sparrow_core::budget::Limits::default() };
    let run = sparrow_core::run_captured(bytes, limits).unwrap();
    let error = run.result.expect_err("the scalar failure leaves main");
    assert_eq!(error.0, format!("RuntimeError: {message}"), "{text}");
    let (stdout, trace) = if resource { ("D\n", "app.Res 1\n") } else { ("\n", "") };
    assert_eq!(String::from_utf8(run.stdout).unwrap(), stdout, "destroy logs exactly once:\n{text}");
    assert_eq!(String::from_utf8(run.destroy_trace).unwrap(), trace, "{text}");
}

const RANGE: &str = "        for a..b -> i {\n            i;\n        }\n        7;";
const RANGE_MESSAGE: &str = "Range start must be less than or equal to range end";

#[test]
fn range_start_after_end_fails_in_a_plain_function() {
    fails(false, RANGE, "3, 1", RANGE_MESSAGE);
}

#[test]
fn range_start_after_end_destroys_a_live_resource_once() {
    fails(true, RANGE, "3, 1", RANGE_MESSAGE);
}

#[test]
fn modulo_by_zero_fails_in_a_plain_function() {
    fails(false, "        a % b;", "3, 0", "Modulo by zero");
}

#[test]
fn modulo_by_zero_destroys_a_live_resource_once() {
    fails(true, "        a % b;", "3, 0", "Modulo by zero");
}

/// 512 references to one 16 MiB string: the 8 GiB result is refused before anything is allocated (#692).
const JOIN: &str = concat!(
    "        let s = \"x\".repeat(16777216);\n",
    "        let p = [s, s, s, s, s, s, s, s];\n",
    "        let q = p + p + p + p + p + p + p + p;\n",
    "        let w = q + q + q + q + q + q + q + q;\n",
    "        w.join(\"\").length();",
);
const JOIN_MESSAGE: &str = "join: the result would pass the 4 GiB a string can hold";

#[test]
fn join_past_four_gib_fails_in_a_plain_function() {
    fails(false, JOIN, "3, 1", JOIN_MESSAGE);
}

#[test]
fn join_past_four_gib_destroys_a_live_resource_once() {
    fails(true, JOIN, "3, 1", JOIN_MESSAGE);
}

// #698: each family of Champ's program failures reports Tomato's message for the same program, as the program
// sees it (no engine prefix). The messages are those Tomato prints for these programs (okkama run, default build).

/// A scalar family operation (`rt_machine_scalar`): a negative repeat count, a shift count past 63, a float
/// division by zero.
#[test]
fn scalar_failures_report_tomato_messages() {
    for resource in [false, true] {
        fails(resource, "        \"x\".repeat(b - a).length();", "3, 1", "repeat needs a non-negative count");
        fails(resource, "        a << 70;", "3, 1", "Shift count must be in 0..63");
        fails(resource, "        let f = a.float() / (b - b).float();\n        f.int();", "3, 1", "Division by zero");
    }
}

/// A conversion (`rt_machine_convert`): an int outside a byte.
#[test]
fn conversion_failures_report_tomato_messages() {
    for resource in [false, true] {
        fails(resource, "        (a * 100).byte().int();", "3, 1",
            "Cannot convert int value 300 to byte: expected 0..255");
    }
}

/// A builder transition (`rt_machine_builder`): a range part whose start is past its end.
#[test]
fn builder_failures_report_tomato_messages() {
    for resource in [false, true] {
        fails(resource, "        let xs is @int = [0, a..b];\n        xs.size();", "3, 1",
            "Range start must be less than or equal to range end");
    }
}

/// Value equality (`rt_machine_value_eq`): a constructor compares itself before it binds every field of type
/// `ty`, whose values are `one`.
fn early_equality(ty: &str, one: &str) -> String {
    format!(r#"module app;
import "io/stdout" take Stdout;
object P {{
    has a is {ty};
    has b is {ty};
    P(x is {ty}, other is P) {{
        self.a = x;
        Stdout().writeln((self == other).string());
        self.b = x;
    }}
    P(x is {ty}) {{
        self.a = x;
        self.b = x;
    }}
}}
main() is int {{
    let q = P({one});
    let p = P({one}, q);
    0;
}}
"#)
}

#[test]
fn value_equality_failures_report_tomato_messages() {
    // #713: an unbound raw scalar field (float, bool, byte, char) is detected as in Tomato.
    let cases = [("string", "\"x\""), ("int", "1"), ("float", "1.5"), ("bool", "true"), ("byte", "7.byte()"),
        ("char", "'x'")];
    for (ty, one) in cases {
        let text = early_equality(ty, one);
        let bytes = compile(&text).unwrap_or_else(|error| panic!("{error}"));
        let limits = sparrow_core::budget::Limits { steps: Some(100_000), ..sparrow_core::budget::Limits::default() };
        let run = sparrow_core::run_captured(bytes, limits).unwrap();
        let error = run.result.expect_err("the comparison of an unbound field leaves main");
        assert_eq!(error.0, "RuntimeError: cannot compare unbound object fields", "{text}");
        assert_eq!(String::from_utf8(run.stdout).unwrap(), "", "{text}");
    }
}
