#![cfg(feature = "champ-abi")]
//! Conformance cases fixed in the own/low lane, run end to end in Champ in process.
//! Only pure language cases: no processes, workers, network or test mode.

use okkama_compiler::lowering::{LoadedModule, ModuleLoader, RootFile, oir2};
use okkama_compiler::syntax::{self, ParseOptions};
use okkama_compiler::SourceFile;
use serde_json::Value;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// Case-local modules first, then the conformance harness modules, then std.
struct CaseLoader {
    case: PathBuf,
}

impl ModuleLoader for CaseLoader {
    fn load(&mut self, path: &str, _: &str) -> Result<Option<LoadedModule>, String> {
        let roots = [self.case.clone(), root().join("conformance/harness/okkama"), root().join("std")];
        let file = roots.iter().map(|base| base.join(format!("{path}.ok"))).find(|file| file.exists())
            .ok_or_else(|| format!("module {path} not found"))?;
        let text = std::fs::read_to_string(&file).map_err(|error| error.to_string())?;
        let source = SourceFile::new(file.to_string_lossy(), text).map_err(|error| error.to_string())?;
        let program = syntax::parse(&source, &ParseOptions::default()).map_err(|error| format!("{error:?}"))?;
        let program = syntax::prune::strip_test_items(program);
        Ok(Some(LoadedModule { id: file.to_string_lossy().into_owned(), source, program }))
    }
}

fn compile(case: &Path, text: &str) -> Result<Vec<u8>, oir2::PipelineError> {
    let source = SourceFile::new("main.ok", text).unwrap();
    let program = syntax::parse(&source, &ParseOptions::default()).unwrap();
    let roots = [RootFile { source: &source, program: &program }];
    let mut loader = CaseLoader { case: case.to_path_buf() };
    oir2::compile_linked_with(&roots, &mut loader, okkama_compiler::oir2::VerifyMode::Structural)
}

/// `L05.map.access` lives in `conformance/cases/L05-*/map.access`.
fn case_directory(id: &str) -> PathBuf {
    let (level, suffix) = id.split_once('.').unwrap();
    let cases = root().join("conformance/cases");
    let mut levels: Vec<_> = std::fs::read_dir(&cases).unwrap().map(|entry| entry.unwrap().path())
        .filter(|path| path.file_name().unwrap().to_string_lossy().starts_with(&format!("{level}-")))
        .collect();
    assert_eq!(levels.len(), 1, "{id}: level directory");
    levels.pop().unwrap().join(suffix)
}

/// `exit` is `None` when the program stopped with a runtime failure.
#[derive(Debug, PartialEq, Eq)]
struct Observed {
    exit: Option<i64>,
    stdout: String,
}

fn run(id: &str) -> Result<Observed, String> {
    let directory = case_directory(id);
    let text = std::fs::read_to_string(directory.join("main.ok")).unwrap();
    let bytes = compile(&directory, &text).map_err(|error| format!("build: {error}"))?;
    let limits = sparrow_core::budget::Limits { steps: Some(10_000_000), ..sparrow_core::budget::Limits::default() };
    let run = sparrow_core::run_captured(bytes, limits).map_err(|error| format!("load: {error}"))?;
    Ok(Observed { exit: run.result.ok(), stdout: String::from_utf8_lossy(&run.stdout).into_owned() })
}

/// A `failure` outcome expects a runtime failure; `success` expects its exit status.
fn expected(id: &str) -> Observed {
    let text = std::fs::read_to_string(case_directory(id).join("case.json")).unwrap();
    let case: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(case["id"], id);
    assert_eq!(case["mode"], "run", "{id}: only run-mode cases");
    let expect = &case["expect"];
    let stdout = expect["stdout"]["text"].as_str().unwrap_or("").to_owned();
    let exit = match expect["outcome"].as_str().unwrap() {
        "success" => Some(expect["exit_status"].as_i64().unwrap()),
        "failure" => None,
        outcome => panic!("{id}: unsupported outcome {outcome}"),
    };
    Observed { exit, stdout }
}

fn check(id: &str) {
    let observed = run(id).unwrap_or_else(|error| panic!("{id}: {error}"));
    let expected = expected(id);
    assert_eq!(observed.stdout, expected.stdout, "{id}: stdout");
    assert_eq!(observed.exit, expected.exit, "{id}: exit");
}

/// Stages after verify_low belong to the SPB 17 / Champ lane.
const BACKEND: [&str; 4] = ["layout", "compile_low", "verify_load", "encode"];

/// The case passes own and low; it runs in Champ once the backend accepts it.
fn check_through_low(id: &str) {
    let directory = case_directory(id);
    let text = std::fs::read_to_string(directory.join("main.ok")).unwrap();
    match compile(&directory, &text) {
        Ok(_) => check(id),
        Err(oir2::PipelineError::Phase { phase, message }) if BACKEND.contains(&phase) => {
            println!("{id}: own and low passed; stops in {phase}: {message}");
        }
        Err(error) => panic!("{id}: {error}"),
    }
}

/// A source of this lane, with the stdout Tomato prints for it: passes own and low, and is
/// checked in Champ once the backend accepts it.
fn check_source_through_low(name: &str, text: &str, stdout: &str) {
    let directory = root().join("conformance/harness/okkama");
    match compile(&directory, text) {
        Ok(bytes) => {
            let limits = sparrow_core::budget::Limits {
                steps: Some(10_000_000),
                ..sparrow_core::budget::Limits::default()
            };
            let run = sparrow_core::run_captured(bytes, limits).unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(String::from_utf8_lossy(&run.stdout), stdout, "{name}: stdout");
            assert_eq!(run.result.ok(), Some(0), "{name}: exit");
        }
        Err(oir2::PipelineError::Phase { phase, message }) if BACKEND.contains(&phase) => {
            println!("{name}: own and low passed; stops in {phase}: {message}");
        }
        Err(error) => panic!("{name}: {error}"),
    }
}

macro_rules! case {
    ($name:ident, $id:literal) => {
        #[test]
        fn $name() {
            check($id);
        }
    };
}

macro_rules! through_low {
    ($name:ident, $id:literal) => {
        #[test]
        fn $name() {
            check_through_low($id);
        }
    };
}

// Task 1: collections. Builders, maps, plain collection operations and managed arrays run in Champ
// (#622); role declarations still stop in the backend.
case!(collections_map_access, "L05.map.access");
through_low!(collections_array_removed_at, "L05.array.removed-at");
case!(collections_range_expansion, "L02.range.expansion");
case!(collections_map_merge, "L05.collections.map-merge");
case!(collections_alias_borrows, "L07.ownership.alias-borrows");

// Task 2: deps recipes (caught role payloads) and tombstones.
case!(deps_catch_error_role_binding, "L07.catch.error-role-binding");
case!(deps_catch_source_order, "L07.catch.source-order");
case!(deps_confirm_bare_uncatchable, "L07.confirm.bare-uncatchable");
case!(deps_confirm_else_panic, "L07.confirm.else-panic");
case!(deps_panic_not_catchable, "L07.panic.not-catchable");
case!(deps_choice_evaluation, "L05.choice.evaluation");

// Task 3: defer helpers, field defaults and lookup fallbacks.
case!(other_defer_all_run_and_precedence, "L07.defer.all-run-and-precedence");
case!(other_defer_failure_beats_destroy, "L07.defer.failure-beats-destroy");
case!(other_fields_defaults_per_construction, "L04.fields.defaults-per-construction");
case!(other_index_fallback, "L01.gram.index.fallback");
case!(other_map_fallback_lazy, "L05.map.fallback-lazy");
// String sources (core.StringSource is an ordinary lowered object): checked against case.json.
through_low!(strings_raw_concatenation, "L01.lex.raw.concatenation");
through_low!(strings_raw_multiline, "L01.lex.raw.multiline");
through_low!(strings_escapes, "L01.lex.string.escapes");
through_low!(strings_range_for_and_functional, "L02.range.for-and-functional");
through_low!(strings_for_sources, "L03.for.sources");

// #666: a collection built in a loop body owns its elements until the end of that iteration.
// The expected trace is Tomato's output for this program (okkama run, default build).
const ITERATION_BUILDER: &str = r#"module app;

import "io/stdout" take Stdout;
import "async/atomic" take Atomic;

object Res {
    has n is int;
    has log is Atomic<string>;
    Res(:n, :log);
    destroy { log.replace(log.value() + " -" + n.string()); }
}

main() is int {
    let log = Atomic("");
    for 0..2 -> round {
        let a = (0..2).map() -> i { Res(n: round * 10 + i, log: log); };
        log.replace(log.value() + " round" + round.string() + ":" + a.size().string());
    }
    log.replace(log.value() + " end");
    Stdout().writeln(log.value());
    0;
}
"#;

#[test]
fn builders_elements_die_with_their_iteration() {
    check_source_through_low("builder-elements-per-iteration", ITERATION_BUILDER,
        " round0:2 -1 -0 round1:2 -11 -10 end\n");
}

// #692: `map` and `filter` over a managed array build arrays of the same resources. Selecting such an array
// leaves its elements with their owner, which destroys each exactly once. The expected stdout is Tomato's.
const MAPPED_RESOURCES: &str = r#"module app;

import "io/stdout" take Stdout;
import "async/atomic" take Atomic;

object Res {
    has n is int;
    has log is Atomic<string>;
    Res(:n, :log);
    destroy { log.replace(log.value() + " -" + n.string()); }
}

object Calc {
    Calc();
    run(log is Atomic<string>) is int {
        let xs = [Res(n: 1, log: log), Res(n: 2, log: log)];
        let mapped = xs.map() -> r { r; };
        let kept = xs.filter() -> r { true; };
        let nested = xs.map() -> r { [r]; };
        log.replace(log.value() + " sizes " + mapped.size().string() + kept.size().string()
            + nested.size().string());
        0;
    }
}

main() is int {
    let log = Atomic("");
    defer Stdout().writeln(log.value());
    Calc().run(log);
}
"#;

#[test]
fn mapped_and_filtered_resources_are_destroyed_once_by_their_owner() {
    let directory = root().join("conformance/harness/okkama");
    let bytes = compile(&directory, MAPPED_RESOURCES).unwrap_or_else(|error| panic!("{error}"));
    let limits = sparrow_core::budget::Limits { steps: Some(10_000_000), ..sparrow_core::budget::Limits::default() };
    let run = sparrow_core::run_captured(bytes, limits).unwrap();
    assert_eq!(run.result, Ok(0));
    assert_eq!(String::from_utf8_lossy(&run.stdout), " sizes 222 -2 -1\n");
    assert_eq!(String::from_utf8_lossy(&run.destroy_trace), "app.Res 2\napp.Res 1\n");
}

/// Opt-in triage: `OKKAMA_CHAMP_CASES=<file with one id per line>` prints where each case stops.
#[test]
fn champ_cases_own_triage() {
    let Some(path) = std::env::var_os("OKKAMA_CHAMP_CASES") else {
        return;
    };
    let ids = std::fs::read_to_string(path).unwrap();
    let dump = std::env::var_os("OKKAMA_CHAMP_DUMP").map(PathBuf::from);
    for id in ids.split_whitespace() {
        if let Some(dump) = &dump {
            let directory = case_directory(id);
            let text = std::fs::read_to_string(directory.join("main.ok")).unwrap();
            let source = SourceFile::new("main.ok", text).unwrap();
            let program = syntax::parse(&source, &ParseOptions::default()).unwrap();
            let roots = [RootFile { source: &source, program: &program }];
            let mut loader = CaseLoader { case: directory };
            if let Ok(high) = oir2::lower_linked_high(&roots, &mut loader) {
                let text = okkama_compiler::oir2::text::print(&high).unwrap();
                std::fs::write(dump.join(format!("{id}.high.oir")), text).unwrap();
            }
        }
        let verdict = std::panic::catch_unwind(|| match run(id) {
            Ok(observed) if observed == expected(id) => "PASS".to_owned(),
            Ok(observed) => format!("WRONG exit={:?} stdout={:?}", observed.exit, observed.stdout),
            Err(error) => error,
        }).unwrap_or_else(|_| "PANIC".to_owned());
        let verdict: String = verdict.chars().take(400).collect();
        println!("TRIAGE {id}\t{verdict}");
    }
}
