//! Correctness of conformance cases on Champ (#677): each case compiled through OIR-2 and run in process,
//! compared with its case.json (outcome, exit status, error type and stdout). Tomato passes every one.
#![cfg(feature = "champ-abi")]

use okkama_compiler::lowering::{LoadedModule, ModuleLoader, RootFile, oir2};
use okkama_compiler::syntax::{self, ParseMode, ParseOptions};
use okkama_compiler::SourceFile;
use serde_json::Value;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// Imports resolve next to the importing file first, then in the suite's harness module directory (the
/// runner copies it into the case), then in std; std files import from std.
struct CaseLoader {
    stdlib: PathBuf,
}

fn harness() -> PathBuf {
    root().join("conformance/harness/okkama")
}

impl CaseLoader {
    fn locate(&self, path: &str, importer: &str) -> Option<PathBuf> {
        let file = if path.ends_with(".ok") { path.to_string() } else { format!("{path}.ok") };
        let importer = Path::new(importer);
        let itself = std::fs::canonicalize(importer).ok();
        let base = importer.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
        let in_std = itself.as_ref().is_some_and(|file| file.starts_with(&self.stdlib));
        let dirs = if in_std {
            vec![self.stdlib.clone(), base]
        } else {
            vec![base, harness(), self.stdlib.clone()]
        };
        dirs.iter()
            .map(|dir| dir.join(&file))
            .filter(|candidate| candidate.is_file())
            .filter_map(|candidate| std::fs::canonicalize(candidate).ok())
            .find(|candidate| Some(candidate) != itself.as_ref())
    }

    fn read(&self, file: PathBuf) -> Result<Option<LoadedModule>, String> {
        let name = file.to_string_lossy().into_owned();
        let text = std::fs::read_to_string(&file).map_err(|error| error.to_string())?;
        let source = SourceFile::new(name.clone(), text).map_err(|error| error.to_string())?;
        let program = syntax::parse(&source, &ParseOptions::default()).map_err(|error| format!("{error:?}"))?;
        let program = syntax::prune::strip_test_items(program);
        Ok(Some(LoadedModule { id: name, source, program }))
    }
}

impl ModuleLoader for CaseLoader {
    fn load(&mut self, path: &str, importer: &str) -> Result<Option<LoadedModule>, String> {
        match self.locate(path, importer) {
            Some(file) => self.read(file),
            None => Ok(None),
        }
    }

    fn resolve(&mut self, path: &str, importer: &str) -> Result<Option<String>, String> {
        Ok(self.locate(path, importer).map(|file| file.to_string_lossy().into_owned()))
    }

    fn resolve_std(&mut self, path: &str, _importer: &str) -> Result<Option<String>, String> {
        let file = self.stdlib.join(format!("{path}.ok"));
        Ok(std::fs::canonicalize(file).ok().map(|file| file.to_string_lossy().into_owned()))
    }

    fn load_std(&mut self, path: &str, _importer: &str) -> Result<Option<LoadedModule>, String> {
        match std::fs::canonicalize(self.stdlib.join(format!("{path}.ok"))) {
            Ok(file) => self.read(file),
            Err(_) => Ok(None),
        }
    }
}

fn case_dir(id: &str) -> PathBuf {
    let (level, suffix) = id.split_once('.').unwrap();
    let cases = root().join("conformance/cases");
    let prefix = format!("{level}-");
    let level_dir = std::fs::read_dir(&cases)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.file_name().unwrap().to_string_lossy().starts_with(&prefix))
        .unwrap_or_else(|| panic!("{id}: no level directory"));
    level_dir.join(suffix)
}

/// The failure category and error type of a runtime report's first line, as the OCS adapter reads them.
fn report_kind(report: &str) -> (&'static str, Option<String>) {
    let line = report.lines().find(|line| !line.trim().is_empty()).unwrap_or("").trim();
    if line.starts_with("RuntimeError: ") || line.starts_with("Cancelled: ") || line.starts_with("Execution ") {
        return ("runtime", None);
    }
    if line.starts_with("Panic: ") {
        return ("panic", None);
    }
    if let Some(name) = line.strip_prefix("Uncaught ") {
        return ("uncaught-error", Some(name.to_string()));
    }
    match line.split_once(": ") {
        Some((name, _)) if name.chars().next().is_some_and(|c| c.is_ascii_uppercase())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') => {
            ("uncaught-error", Some(name.to_string()))
        }
        _ => ("runtime", None),
    }
}

fn expected_failure(expect: &Value, report: &str) -> Result<(), String> {
    let failure = &expect["failure"];
    let (category, error_type) = report_kind(report);
    if let Some(categories) = failure["categories"].as_array()
        && !categories.iter().any(|expected| expected.as_str() == Some(category)) {
        return Err(format!("failure category {category}, expected {categories:?}; report {report:?}"));
    }
    if let Some(expected) = failure["error_type"].as_str()
        && error_type.as_deref() != Some(expected) {
        return Err(format!("error type {error_type:?}, expected {expected}; report {report:?}"));
    }
    Ok(())
}

fn sources(dir: &Path, case: &Value) -> Vec<SourceFile> {
    case["sources"].as_array().unwrap().iter().map(|name| {
        let file = std::fs::canonicalize(dir.join(name.as_str().unwrap())).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        SourceFile::new(file.to_string_lossy(), text).unwrap()
    }).collect()
}

/// Compiles and runs one case; `Err` describes the first mismatch with its case.json.
fn run_case(id: &str) -> Result<(), String> {
    let dir = case_dir(id);
    let case: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("case.json")).unwrap()).unwrap();
    assert_eq!(case["mode"], "run", "{id}: only run-mode cases");
    let expect = &case["expect"];
    let outcome = expect["outcome"].as_str().unwrap();
    let sources = sources(&dir, &case);
    // A program runs in the production grammar: test bodies are not parsed (§14), as `okkama run` does.
    let options = ParseOptions { mode: ParseMode::Production, ..ParseOptions::default() };
    let mut programs = Vec::new();
    for source in &sources {
        let program = syntax::parse(source, &options).map_err(|error| format!("parse: {error:?}"))?;
        programs.push(syntax::prune::strip_test_items(program));
    }
    let roots = sources.iter()
        .zip(&programs)
        .map(|(source, program)| RootFile { source, program })
        .collect::<Vec<_>>();
    let stdlib = std::fs::canonicalize(root().join("std")).unwrap();
    let mode = okkama_compiler::oir2::VerifyMode::Structural;
    let compiled = oir2::compile_linked_with(&roots, &mut CaseLoader { stdlib }, mode);
    let bytes = match (outcome, compiled) {
        (_, Ok(bytes)) => bytes,
        // A static rejection by the shared front end: the check stage of an `error` case.
        ("error", Err(oir2::PipelineError::Phase { phase: "analysis", .. })) => return Ok(()),
        (_, Err(error)) => return Err(format!("compile: {error}")),
    };
    let limits = sparrow_core::budget::Limits {
        steps: Some(100_000_000),
        ..sparrow_core::budget::Limits::default()
    };
    let run = match (outcome, sparrow_core::run_captured(bytes, limits)) {
        (_, Ok(run)) => run,
        ("error", Err(_)) => return Ok(()),
        (_, Err(error)) => return Err(format!("load: {error}")),
    };
    let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    match (outcome, &run.result) {
        ("success", Ok(code)) => {
            let status = expect["exit_status"].as_i64().unwrap_or(0);
            if *code != status {
                return Err(format!("exit {code}, expected {status}; stdout={stdout:?}"));
            }
        }
        ("success", Err(error)) => return Err(format!("failed: {}; stdout={stdout:?}", error.0)),
        (_, Ok(code)) => return Err(format!("completed with {code}, expected a failure; stdout={stdout:?}")),
        (_, Err(error)) => expected_failure(expect, &error.0)?,
    }
    let expected_stdout = expect["stdout"]["text"].as_str().unwrap_or("");
    if stdout != expected_stdout {
        return Err(format!("stdout {stdout:?}, expected {expected_stdout:?}"));
    }
    Ok(())
}

fn check(ids: &[&str]) {
    let failures = ids.iter()
        .filter_map(|id| run_case(id).err().map(|error| format!("{id}: {error}")))
        .collect::<Vec<_>>();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A non-matching catch or a rethrow inside a nested try reaches the enclosing catch.
#[test]
fn champ_correctness_nested_try_rethrow() {
    check(&["L07.catch.non-matching-propagates", "L07.rethrow.preserves-identity", "L01.gram.throw.forms"]);
}

/// Error variants with payloads (#705): their cases are caught by case and by declaration, thrown uncaught
/// with their error type, and written by `text()` as Tomato writes them.
#[test]
fn champ_correctness_error_variants() {
    check(&[
        "L07.catch-pattern.cases-and-types",
        "L07.error-enum.text-escaping",
        "L07.error-enum.uncaught",
    ]);
}

/// Compiles one source file through OIR-2 and runs it in Champ.
fn run_source(text: &str) -> sparrow_core::CapturedRun {
    let source = SourceFile::new("app.ok", text).unwrap();
    let program = syntax::parse(&source, &ParseOptions::default()).unwrap();
    let roots = [RootFile { source: &source, program: &program }];
    let stdlib = std::fs::canonicalize(root().join("std")).unwrap();
    let mode = okkama_compiler::oir2::VerifyMode::Structural;
    let bytes = oir2::compile_linked_with(&roots, &mut CaseLoader { stdlib }, mode)
        .unwrap_or_else(|error| panic!("compile: {error}"));
    // Champ counts steps as Tomato does (#692): a call is a step, so the recursion depth tests need more than
    // a million. The budget only stops a runaway program, as in `check`.
    let limits = sparrow_core::budget::Limits { steps: Some(100_000_000), ..sparrow_core::budget::Limits::default() };
    sparrow_core::run_captured(bytes, limits).unwrap()
}

/// An integer literal beyond 63 bits is an unbounded integer constant in Champ.
#[test]
fn champ_big_integer_literals() {
    let run = run_source(concat!(
        "module app; main() is int { let big = 9223372036854775808; ",
        "confirm big - 1 == 9223372036854775807; confirm big + big == 18446744073709551616; 3; }",
    ));
    assert_eq!(run.result, Ok(3));
}

/// A program without `main` fails at start with a runtime report.
#[test]
fn champ_program_without_main_fails_at_start() {
    let run = run_source("module app; object Marker { Marker(); }");
    let error = run.result.expect_err("no entry");
    assert_eq!(error.0, "RuntimeError: Program requires main() declaration");
}

/// Exit status: unparsed test bodies, big integer constants, multi-file roots, a missing entry.
#[test]
fn champ_correctness_exit_status() {
    check(&[
        "L01.gram.test.inner-grammar-not-parsed",
        "L04.fields.scalar-replacement",
        "L06.multifile.shared-declarations",
        "L00.entry.missing-main-fails",
    ]);
}

/// A constructor branches before binding its fields: the unbound fields never cross an edge.
#[test]
fn champ_correctness_constructor_branches_before_binding() {
    check(&["L04.ctor.field-bind-forms"]);
}

/// A constructor whose body leaves a field unbound on some path checks it after the body, as Tomato does: the
/// construction fails with "Object \"Gate\" field \"a\" was not initialized" (#708); a constructor that binds
/// some fields and then fails, directly or in a nested construction, destroys the bound fields once and never
/// completes.
#[test]
fn champ_correctness_constructor_fields_bound_on_some_paths() {
    check(&["L04.ctor.unbound-field-dies-after-body", "L12.constructor-nested-resource-failure"]);
}

/// An early-self constructor (`self` handed out before its fields are bound) that leaves a field unbound on one
/// path fails after the body as Tomato does (#746): `Gate(true)` completes and prints 1, `Gate(false)` fails with
/// "Object \"Gate\" field \"a\" was not initialized" before the next line.
#[test]
fn champ_correctness_early_self_fields_bound_on_some_paths() {
    let source = r#"module app v1.0.0;

import "io/stdout" take Stdout;

object Gate {
    has a is int;
    Gate(flag is bool) {
        let alias = self;
        if flag {
            self.a = 1;
        }
    }
    a() is int { a; }
}

main() is int {
    Stdout().writeln(Gate(true).a().string());
    let h = Gate(false);
    Stdout().writeln("unreachable");
    0;
}"#;
    let run = run_source(source);
    assert_eq!(String::from_utf8_lossy(&run.stdout), "1\n");
    let failure = run.result.unwrap_err().0;
    assert!(failure.contains("Object \"Gate\" field \"a\" was not initialized"), "{failure}");
}

/// A field bound inside a `try` whose body fails before the binding stays unbound on the handler's path: the
/// constructor checks its flag after the body and fails as Tomato does (#770; the undef value and the flag
/// cross the unwind and catch edges).
#[test]
fn champ_correctness_field_bound_in_a_failing_try() {
    let source = r#"module app v1.0.0;

import "io/stdout" take Stdout;

enum Refused is Error = (refused);

object Source {
    has n is int;
    Source(:n);
    value() is int {
        confirm n > 0 else throw Refused.refused;
        n;
    }
}

object Holder {
    has a is int;
    Holder(v is int) {
        let staged = try {
            a = Source(n: v).value();
            1;
        } catch e is Error {
            0;
        };
    }
    a() is int { a; }
}

main() is int {
    Stdout().writeln(Holder(1).a().string());
    let h = Holder(0);
    Stdout().writeln("unreachable");
    0;
}"#;
    let run = run_source(source);
    assert_eq!(String::from_utf8_lossy(&run.stdout), "1\n");
    let failure = run.result.unwrap_err().0;
    assert!(failure.contains("Object \"Holder\" field \"a\" was not initialized"), "{failure}");
}

/// The same early self binds its field on every path: no check fails and both objects complete.
#[test]
fn champ_correctness_early_self_fields_bound_on_every_path() {
    let source = r#"module app v1.0.0;

import "io/stdout" take Stdout;

object Gate {
    has a is int;
    Gate(flag is bool) {
        let alias = self;
        if flag {
            self.a = 1;
        } else {
            self.a = 2;
        }
    }
    a() is int { a; }
}

main() is int {
    Stdout().writeln(Gate(true).a().string());
    Stdout().writeln(Gate(false).a().string());
    0;
}"#;
    let run = run_source(source);
    let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    assert_eq!(run.result.map_err(|error| error.0), Ok(0), "stdout={stdout:?}");
    assert_eq!(stdout, "1\n2\n");
}

/// Left over from #705 (a dead unwind landing of a total construction): #702's pruning of dead blocks made it run.
#[test]
fn champ_correctness_csv_rows_and_round_trip() {
    check(&["L09.csv.rows-and-round-trip"]);
}

/// A compiled observation callee is called directly: a million non-tail levels fit the VM stack.
#[test]
fn champ_deep_recursion_takes_one_machine_frame_per_level() {
    let run = run_source(concat!(
        "module app; object Sums { Sums(); sum(n is int) is int { if n == 0 { 0; } else { n + self.sum(n - 1); } } } ",
        "main() is int { confirm Sums().sum(1000000) == 500000500000; 0; }",
    ));
    assert_eq!(run.result, Ok(0));
}

/// A self call whose result is returned at once runs in constant stack, deeper than any frame chain fits.
#[test]
fn champ_self_tail_calls_run_in_constant_stack() {
    let run = run_source(concat!(
        "module app; object Counter { Counter(); ",
        "count(n is int, total is int) is int { if n == 0 { total; } else { self.count(n - 1, total + n); } } } ",
        "main() is int { confirm Counter().count(4000000, 0) == 8000002000000; 0; }",
    ));
    assert_eq!(run.result, Ok(0));
}

/// Recursion depth: a million non-tail levels, ten million self tail calls.
#[test]
fn champ_correctness_recursion_depth() {
    check(&["L03.recursion.deep-non-tail", "L03.recursion.tail-constant-stack"]);
}
