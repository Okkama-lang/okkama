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
