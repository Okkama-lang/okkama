//! Test-mode conformance cases compiled through OIR-2 and run by Champ's test runner, in process (#569).
#![cfg(feature = "champ-abi")]

use okkama_compiler::lowering::{LoadedModule, ModuleLoader, RootFile, oir2};
use okkama_compiler::syntax::{self, ParseOptions};
use okkama_compiler::SourceFile;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn harness() -> PathBuf {
    root().join("conformance/harness/okkama")
}

/// The test profile parses every module with its tests and fakes (§14), as `okkama run -t` does.
fn parse_tests(source: &SourceFile) -> Result<syntax::ast::Program, String> {
    let program = syntax::parse(source, &ParseOptions::default()).map_err(|error| format!("parse: {error:?}"))?;
    Ok(syntax::prune::desugar_test_items(program))
}

/// Imports resolve next to the importing file first, then in the suite's harness module directory, then in
/// std; std files import from std.
struct TestLoader {
    stdlib: PathBuf,
    production: bool,
}

impl TestLoader {
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
        let program = if self.production {
            let program = syntax::parse(&source, &ParseOptions::default()).map_err(|error| format!("{error:?}"))?;
            syntax::prune::strip_test_items(program)
        } else {
            parse_tests(&source)?
        };
        Ok(Some(LoadedModule { id: name, source, program }))
    }
}

impl ModuleLoader for TestLoader {
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

fn stdlib() -> PathBuf {
    std::fs::canonicalize(root().join("std")).unwrap()
}

/// Compiles the files in the test profile through OIR-2 to SPB 17.
fn compile(files: &[PathBuf]) -> Result<Vec<u8>, String> {
    let mut sources = Vec::new();
    for file in files {
        let file = std::fs::canonicalize(file).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        sources.push(SourceFile::new(file.to_string_lossy(), text).unwrap());
    }
    let programs = sources.iter().map(parse_tests).collect::<Result<Vec<_>, _>>()?;
    let roots = sources.iter()
        .zip(&programs)
        .map(|(source, program)| RootFile { source, program })
        .collect::<Vec<_>>();
    let mode = okkama_compiler::oir2::VerifyMode::Structural;
    oir2::compile_linked_tests_with(&roots, &mut TestLoader { stdlib: stdlib(), production: false }, mode)
        .map_err(|error| format!("compile: {error}"))
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

/// A finished test run, as `okkama run -t` would end: the report and program output, and the exit status.
struct Run {
    stdout: String,
    code: u8,
    error: Option<String>,
}

/// Compiles the files in the test profile and runs the suite on Champ, in process.
fn run_tests(files: &[PathBuf]) -> Result<Run, String> {
    let bytes = compile(files)?;
    let limits = sparrow_core::budget::Limits {
        steps: Some(10_000_000),
        ..sparrow_core::budget::Limits::default()
    };
    let (result, stdout) = sparrow_core::run_tests_captured(bytes, limits).map_err(|error| format!("load: {error}"))?;
    let stdout = String::from_utf8_lossy(&stdout).into_owned();
    Ok(match result {
        Ok(passed) => Run { stdout, code: u8::from(!passed), error: None },
        Err(error) => Run { stdout, code: 1, error: Some(error.0) },
    })
}

/// Compiles and runs one test-mode case; `Err` describes the first mismatch.
fn run_case(id: &str) -> Result<(), String> {
    let dir = case_dir(id);
    let case: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("case.json")).unwrap()).unwrap();
    assert_eq!(case["mode"], "test", "{id}: only test-mode cases");
    let files = case["sources"].as_array().unwrap().iter()
        .map(|name| dir.join(name.as_str().unwrap()))
        .collect::<Vec<_>>();
    let expect = &case["expect"];
    let run = run_tests(&files);
    match (expect["outcome"].as_str().unwrap(), &run) {
        ("success", Ok(Run { error: None, code, stdout })) => {
            let status = expect["exit_status"].as_i64().unwrap_or(0);
            if i64::from(*code) != status {
                return Err(format!("exit {code}, expected {status}; stdout={stdout:?}"));
            }
        }
        ("success", Ok(Run { error: Some(error), .. })) => return Err(format!("failed: {error}")),
        ("success", Err(error)) => return Err(error.clone()),
        (_, Ok(Run { error: None, code, .. })) => return Err(format!("completed with {code}, expected a failure")),
        (_, _) => {}
    }
    if let (Some(expected), Ok(run)) = (expect["stdout"]["text"].as_str(), &run)
        && run.stdout != expected {
        return Err(format!("stdout {:?}, expected {expected:?}", run.stdout));
    }
    Ok(())
}

fn check(ids: &[&str]) {
    let failures = ids.iter()
        .filter_map(|id| run_case(id).err().map(|error| format!("{id}: {error}")))
        .collect::<Vec<_>>();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Runs every id of the list named by `CHAMP_CASES` (one id per line) and prints the count.
#[test]
#[ignore = "a measurement over a case list: CHAMP_CASES=<file>"]
fn champ_test_cases_list() {
    let Ok(list) = std::env::var("CHAMP_CASES") else {
        return;
    };
    let text = std::fs::read_to_string(&list).unwrap();
    let ids = text.lines().map(str::trim).filter(|line| !line.is_empty()).collect::<Vec<_>>();
    let mut passed = 0;
    for id in &ids {
        match std::panic::catch_unwind(|| run_case(id)) {
            Ok(Ok(())) => {
                passed += 1;
                println!("PASS {id}");
            }
            Ok(Err(error)) => println!("FAIL {id}: {error}"),
            Err(_) => println!("FAIL {id}: panicked"),
        }
    }
    println!("TOTAL {list}: {passed}/{}", ids.len());
}

/// Runs every file of the `:`-separated list `CHAMP_TEST_FILES` as a test-mode root (std modules, for
/// example) and prints each run's counts, or where it stops.
#[test]
#[ignore = "a measurement over files: CHAMP_TEST_FILES=<file>:<file>"]
fn champ_test_files_list() {
    let Ok(files) = std::env::var("CHAMP_TEST_FILES") else {
        return;
    };
    for file in files.split(':') {
        match std::panic::catch_unwind(|| run_tests(&[PathBuf::from(file)])) {
            Ok(Ok(run)) => {
                let counts = ["--- PASS", "--- FAIL", "--- SKIP"].map(|tag| run.stdout.matches(tag).count());
                println!("RAN {file}: exit {} pass/fail/skip {counts:?} error {:?}", run.code, run.error);
            }
            Ok(Err(error)) => println!("STOP {file}: {error}"),
            Err(_) => println!("STOP {file}: panicked"),
        }
    }
}

/// The test-runner cases that run on Champ: report, isolation of a thrown failure, skip after a failed
/// check, successor assertions and the empty suite.
#[test]
fn champ_test_mode_cases() {
    check(&[
        "L01.gram.hidden.test-among-actions",
        "L07.test-runner.empty-suite",
        "L07.test-runner.isolated-failures",
        "L07.test-runner.skip-after-failure",
        "L07.test-runner.successor-assertions",
    ]);
}

/// A source in a file of its own, removed when dropped.
struct Source(PathBuf);

impl Source {
    fn new(name: &str, text: &str) -> Source {
        let path = std::env::temp_dir().join(format!("okkama-champ-tests-{}-{name}", std::process::id()));
        std::fs::write(&path, text).unwrap();
        Source(path)
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

const SKIPS: &str = "module app v1.0.0;\n\
    object Account {\n\
        Account();\n\
        test \"public\" { test.ok(true); }\n\
        hidden helper() is int { 1; }\n\
        test \"among private\" { test.ok(true); }\n\
    }\n\
    object Lookup { Lookup(); value() is int { test.skip(\"in an action\"); 1; } }\n\
    test \"skipped\" { test.skip(\"not today\"); test.ok(false); }\n\
    test \"from an action\" { test.is(Lookup().value(), 2); }\n\
    test \"failed first\" { test.is(1, 2); test.skip(\"too late\"); }\n";

const SKIPS_REPORT: &str = "=== RUN   Account :: public\n--- PASS: Account :: public\n\
    === RUN   Account :: among private\n--- PASS: Account :: among private\n\
    === RUN   skipped\n--- SKIP: skipped\n    not today\n\
    === RUN   from an action\n--- SKIP: from an action\n    in an action\n\
    === RUN   failed first\n--- FAIL: failed first\n    expected 2, got 1\nFAIL\n";

/// Skip past a called action, a check after skip, a skip after a failed check: Tomato's report.
#[test]
fn champ_test_mode_skip_reports_like_tomato() {
    let source = Source::new("skips.ok", SKIPS);
    let run = run_tests(std::slice::from_ref(&source.0)).unwrap();
    assert_eq!((run.code, run.error.as_deref()), (1, None));
    assert_eq!(run.stdout, SKIPS_REPORT);
}

/// Failed checks of every kind are recorded in order and the case continues; a thrown failure ends
/// only its own case.
#[test]
fn champ_test_mode_checks_continue_and_cases_are_isolated() {
    let source = Source::new("checks.ok", "module app v1.0.0;\n\
        enum Denied is Error = (denied);\n\
        test \"checks\" {\n\
            test.is(\"a\", \"b\"); test.isnt(true, true); test.ok(false); test.fail(\"said so\");\n\
            test.close(1.0, 1.5, 0.1); test.close(1.0, 1.0, -1.0); test.text(\"one\\ntwo\", \"one\\ntoo\");\n\
            test.is(3, 3); test.close(-2.5, -2.4, 0.1000001); test.text(\"a\", \"a\");\n\
        }\n\
        test \"throws\" { test.ok(false); throw Denied.denied; }\n\
        test \"passes\" { test.is(1 + 1, 2); }\n");
    let run = run_tests(std::slice::from_ref(&source.0)).unwrap();
    assert_eq!((run.code, run.error.as_deref()), (1, None));
    assert_eq!(run.stdout, "=== RUN   checks\n--- FAIL: checks\n\
        \x20   expected \"b\", got \"a\"\n\
        \x20   expected values to differ, both were true\n\
        \x20   expected true, got false\n\
        \x20   said so\n\
        \x20   expected 1.5 within 0.1, got 1 (off by 0.5)\n\
        \x20   tolerance must be zero or more, got -1\n\
        \x20   text differs at scalar 5 (line 2, column 2): expected \"one\\ntoo\", got \"one\\ntwo\"; \
        lengths 7 and 7\n\
        === RUN   throws\n--- FAIL: throws\n    expected true, got false\n    Denied: Denied.denied\n\
        === RUN   passes\n--- PASS: passes\nFAIL\n");
}

/// Unequal objects, variants and nested values in a failing check show their values, not heap
/// identities (#756); Tomato shows the same text (`cli.rs`).
const SHOWN_VALUES: &str = "module app v1.0.0;\n\
    import \"time/duration\" take Duration;\n\
    enum Shape = (dot, rect(w is int, h is int));\n\
    enum Down is Error = (unavailable(code is int));\n\
    object Point { has x is int; has label is string; Point(:x, :label); }\n\
    object Box<T> { has v is T; Box(:v); }\n\
    object Cached { has n is int; Cached(:n); twice() is int { once let doubled = n * 2; doubled; } }\n\
    test \"values\" {\n\
    \x20   test.is(Duration(seconds: 1), Duration(seconds: 2));\n\
    \x20   test.is(Point(x: 1, label: \"a\\\"b\"), Point(x: 2, label: \"c\"));\n\
    \x20   test.is(Box(v: Shape.rect(1, 2)), Box(v: Shape.dot));\n\
    \x20   test.is(Box(v: [1, 2]), Box(v: [3]));\n\
    \x20   test.is(Down.unavailable(3), Down.unavailable(4));\n\
    \x20   test.isnt(Maybe.some(Point(x: 1, label: \"\")), Maybe.some(Point(x: 1, label: \"\")));\n\
    \x20   test.is([Point(x: 1, label: \"p\")], [Point(x: 2, label: \"q\")]);\n\
    \x20   let cached = Cached(n: 4);\n\
    \x20   test.is(cached.twice(), 8);\n\
    \x20   test.isnt(cached, Cached(n: 4));\n\
    }\n";

const SHOWN_VALUES_REPORT: &str = "=== RUN   values\n\
    --- FAIL: values\n\
    \x20   expected Duration(origin: 2), got Duration(origin: 1)\n\
    \x20   expected Point(x: 2, label: \"c\"), got Point(x: 1, label: \"a\\\"b\")\n\
    \x20   expected Box<Shape>(v: Shape.dot), got Box<Shape>(v: Shape.rect(1, 2))\n\
    \x20   expected Box<@int>(v: [3]), got Box<@int>(v: [1, 2])\n\
    \x20   expected Down.unavailable(4), got Down.unavailable(3)\n\
    \x20   expected values to differ, both were Maybe.some(Point(x: 1, label: \"\"))\n\
    \x20   expected [Point(x: 2, label: \"q\")], got [Point(x: 1, label: \"p\")]\n\
    \x20   expected values to differ, both were Cached(n: 4)\n\
    FAIL\n";

#[test]
fn champ_test_mode_shows_objects_and_variants_by_value() {
    let source = Source::new("shown.ok", SHOWN_VALUES);
    let run = run_tests(std::slice::from_ref(&source.0)).unwrap();
    assert_eq!((run.code, run.error.as_deref()), (1, None));
    assert_eq!(run.stdout, SHOWN_VALUES_REPORT);
}

/// Async tests run on Champ beside the synchronous ones, as on Tomato: one awaits and passes, one fails after its
/// await (#570; `L04.fakes.typed-state`, `L07.test-runner.all-pass-async`).
#[test]
fn champ_test_mode_runs_async_tests() {
    let source = Source::new("async.ok", "module app v1.0.0;\n\
        import \"async/event\" take Event;\n\
        test \"sync\" { test.ok(true); }\n\
        async test \"later\" { let e = Event(ready: true); await e.wait(); test.ok(true); }\n\
        async test \"fails\" { await Event(ready: true).wait(); test.ok(false); }\n");
    let run = run_tests(std::slice::from_ref(&source.0)).unwrap();
    assert_eq!((run.code, run.error.as_deref()), (1, None));
    assert_eq!(run.stdout, "=== RUN   sync\n--- PASS: sync\n=== RUN   later\n--- PASS: later\n=== RUN   fails\n\
        --- FAIL: fails\n    expected true, got false\nFAIL\n");
}

/// Inline tests of std modules that the OIR-2 pipeline compiles today.
#[test]
fn champ_test_mode_std_modules() {
    for module in ["http/version", "http/http2_tls"] {
        let file = stdlib().join(format!("{module}.ok"));
        let run = run_tests(&[file]).unwrap_or_else(|error| panic!("{module}: {error}"));
        assert_eq!((run.code, run.error.as_deref()), (0, None), "{module}: {}", run.stdout);
        assert!(run.stdout.starts_with("=== RUN   ") && run.stdout.ends_with("\nPASS\n"), "{module}: {}", run.stdout);
    }
}

fn okkama(args: &[&std::ffi::OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_okkama"))
        .env("OKKAMA_PIPELINE", "oir2")
        .env("OKKAMA_STDLIB_DIR", stdlib())
        .args(args)
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// `okkama run -t` (with `--filter`), and `okkama build -t` then `okkama exec` of the artifact.
#[test]
fn champ_test_mode_cli_run_build_and_exec() {
    let source = Source::new("cli.ok", SKIPS);
    let path = source.0.as_os_str();
    let os = std::ffi::OsStr::new;
    let output = okkama(&[os("run"), os("-t"), path]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), SKIPS_REPORT);
    let output = okkama(&[os("run"), os("-t"), os("--filter"), os("skip"), os("--filter"), os("private"), path]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "=== RUN   Account :: among private\n--- PASS: Account :: among private\n\
        === RUN   skipped\n--- SKIP: skipped\n    not today\nPASS\n");
    let output = okkama(&[os("run"), os("-t"), os("--filter"), os("nothing"), path]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(text(&output.stderr), "okkama: no test name contains \"nothing\"\n");
    let artifact = Source::new("cli.spb", "");
    let output = okkama(&[os("build"), os("-t"), path, os("-o"), artifact.0.as_os_str()]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    let output = okkama(&[os("exec"), artifact.0.as_os_str()]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), SKIPS_REPORT);
    // An async test runs (#570).
    let asynchronous = Source::new("cli-async.ok", "module app v1.0.0;\nasync test \"later\" { test.ok(true); }\n");
    let output = okkama(&[os("run"), os("-t"), asynchronous.0.as_os_str()]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "=== RUN   later\n--- PASS: later\nPASS\n");
    let empty = Source::new("cli-empty.ok", "module app v1.0.0;\nmain() is int { 0; }\n");
    let output = okkama(&[os("run"), os("-t"), empty.0.as_os_str()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(text(&output.stderr), "okkama: Test run requires at least one test\n");
}
