#![cfg(feature = "champ-abi")]
//! #569 C3b (open action invocation) conformance cases, run end to end in Champ in
//! process and compared with their case.json; a case that stops before Low is ignored
//! with its ticket. The C6 (`any`) cases are in `champ_cases_any.rs`.
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
    /// The category of the failure report when the program failed.
    failure: Option<&'static str>,
}

/// The failure category of a Champ report, as the conformance runner reads its first line: a runtime error
/// (also a cancellation or an exhausted budget), a panic, or an uncaught error.
fn category(report: &str) -> &'static str {
    let line = report.lines().find(|line| !line.trim().is_empty()).unwrap_or("").trim();
    if line.starts_with("RuntimeError: ") || line.starts_with("Cancelled: ") || line.starts_with("Execution ") {
        return "runtime";
    }
    if line.starts_with("Panic: ") {
        return "panic";
    }
    match line.split_once(": ") {
        Some((name, _)) if line.starts_with("Uncaught ") || name.chars().next().is_some_and(|c| c.is_ascii_uppercase())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') => "uncaught-error",
        _ => "runtime",
    }
}

/// The one failure category a failing case expects.
fn expected_category(id: &str, expect: &Value) -> Option<&'static str> {
    let categories = expect["failure"]["categories"].as_array()?;
    let [only] = categories.as_slice() else { panic!("{id}: one expected failure category") };
    Some(match only.as_str() {
        Some("runtime") => "runtime",
        Some("panic") => "panic",
        Some("uncaught-error") => "uncaught-error",
        other => panic!("{id}: unsupported failure category {other:?}"),
    })
}


fn run(id: &str) -> Result<Observed, String> {
    let directory = case_directory(id);
    let text = std::fs::read_to_string(directory.join("main.ok")).unwrap();
    let bytes = compile(&directory, &text).map_err(|error| format!("build: {error}"))?;
    let limits = sparrow_core::budget::Limits { steps: Some(10_000_000), ..sparrow_core::budget::Limits::default() };
    let run = sparrow_core::run_captured(bytes, limits).map_err(|error| format!("load: {error}"))?;
    let failure = run.result.as_ref().err().map(|error| category(&error.0));
    Ok(Observed { exit: run.result.ok(), stdout: String::from_utf8_lossy(&run.stdout).into_owned(), failure })
}

/// A `failure` or `error` outcome expects a runtime failure; `success` expects its exit status.
fn expected(id: &str) -> Observed {
    let text = std::fs::read_to_string(case_directory(id).join("case.json")).unwrap();
    let case: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(case["id"], id);
    assert_eq!(case["mode"], "run", "{id}: only run-mode cases");
    let expect = &case["expect"];
    let stdout = expect["stdout"]["text"].as_str().unwrap_or("").to_owned();
    let exit = match expect["outcome"].as_str().unwrap() {
        "success" => Some(expect["exit_status"].as_i64().unwrap()),
        "failure" | "error" => None,
        outcome => panic!("{id}: unsupported outcome {outcome}"),
    };
    let failure = exit.is_none().then(|| expected_category(id, expect)).flatten();
    Observed { exit, stdout, failure }
}

fn check(id: &str) {
    let observed = run(id).unwrap_or_else(|error| panic!("{id}: {error}"));
    let expected = expected(id);
    assert_eq!(observed.stdout, expected.stdout, "{id}: stdout");
    assert_eq!(observed.exit, expected.exit, "{id}: exit");
    if expected.failure.is_some() {
        assert_eq!(observed.failure, expected.failure, "{id}: failure category");
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

// C3b: open action invocation. An open call dispatches over every closure layout of its `fn` type with the
// registered open callable contract, and its result packet joins the scope through the Closure protocol
// (`own.dynamic`, #676); the scope's cleanup releases what it holds once.
case!(open_anon_capture_and_escape, "L03.anon.capture-and-escape");
case!(open_anon_captures_self, "L03.anon.captures-self");
case!(open_anon_compose, "L03.anon.compose");
case!(open_anon_self_retained_only_when_read, "L03.anon.self-retained-only-when-read");
case!(open_anonymous_lexical_scope, "L03.anonymous.lexical-scope");
case!(open_caps_ordinary_block_body, "L04.caps.ordinary-block-body");
case!(open_invariant_escaped_self_bound_action, "L04.invariant.escaped-self-bound-action");
case!(open_privacy_closure_reads_self_field, "L04.privacy.closure-reads-self-field");
case!(open_successor_bound_action_invoked, "L04.successor.bound-action-invoked");
case!(open_successor_closure_transferred, "L04.successor.closure-transferred");
// Managed arrays and maps of role values that reach resources (#711).
case!(open_regions_role_instances, "L07.regions.role-instances");
// A holder of action values whose closures capture only plain data is no identity (#723).
case!(open_collections_ordered_sets, "L09.collections.ordered-sets");

/// A single-file program of this test, run in Champ: its exit status and stdout.
fn run_source(text: &str) -> Observed {
    let directory = root().join("conformance/harness/okkama");
    let bytes = compile(&directory, text).unwrap_or_else(|error| panic!("build: {error}"));
    let limits = sparrow_core::budget::Limits { steps: Some(10_000_000), ..sparrow_core::budget::Limits::default() };
    let run = sparrow_core::run_captured(bytes, limits).unwrap_or_else(|error| panic!("load: {error}"));
    let failure = run.result.as_ref().err().map(|error| category(&error.0));
    Observed { exit: run.result.ok(), stdout: String::from_utf8_lossy(&run.stdout).into_owned(), failure }
}

/// An open call that returns a fresh resource: the Closure protocol moves it into the calling scope, which
/// destroys it once when it ends. Tomato prints the same.
#[test]
fn an_open_call_result_is_destroyed_by_its_scope_once() {
    let observed = run_source(r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Res {
    has name is string;
    Res(:name);
    label() is string { name; }
    destroy { Stdout().writeln("destroy " + name); }
}
object Factory {
    Factory();
    maker(name is string) is () is Res { () is Res { Res(name: name); }; }
}
main() is int {
    let out = Stdout();
    let make = Factory().maker("made");
    {
        let r = make();
        out.writeln("got " + r.label());
    }
    let s = make();
    out.writeln("again " + s.label());
    out.writeln("end");
    0;
}"#);
    let stdout = "got made\ndestroy made\nagain made\nend\ndestroy made\n";
    assert_eq!(observed, Observed { exit: Some(0), stdout: stdout.into(), failure: None });
}

/// An opaque action value that owns a resource it captured: each call reads it, and the resource is destroyed
/// once, when the value's owner scope ends. Tomato prints the same.
#[test]
fn an_opaque_action_value_releases_its_captured_resource_once() {
    let observed = run_source(r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Res {
    has name is string;
    Res(:name);
    label() is string { name; }
    destroy { Stdout().writeln("destroy " + name); }
}
object Factory {
    Factory();
    reader(res is Res) is () is string { () is string { res.label() + "!"; }; }
}
main() is int {
    let out = Stdout();
    let read = Factory().reader(Res(name: "kept"));
    out.writeln(read());
    out.writeln(read());
    out.writeln("end");
    0;
}"#);
    assert_eq!(observed, Observed { exit: Some(0), stdout: "kept!\nkept!\nend\ndestroy kept\n".into(), failure: None });
}

/// The failure report of a case run in Champ.
fn report(id: &str) -> String {
    let directory = case_directory(id);
    let text = std::fs::read_to_string(directory.join("main.ok")).unwrap();
    let bytes = compile(&directory, &text).unwrap_or_else(|error| panic!("{id}: build: {error}"));
    let limits = sparrow_core::budget::Limits { steps: Some(10_000_000), ..sparrow_core::budget::Limits::default() };
    let run = sparrow_core::run_captured(bytes, limits).unwrap_or_else(|error| panic!("{id}: load: {error}"));
    run.result.expect_err("a failing case").0
}

/// #712: a bound action of a receiver whose construction is pending fails when it is called, with Tomato's
/// runtime error; a closure trampoline that passed its input packet to the failing call has consumed it.
#[test]
fn a_bound_action_of_a_pending_receiver_fails_like_tomato() {
    assert_eq!(report("L04.invariant.escaped-self-bound-action"),
        "RuntimeError: Action \"name\" on object \"Parent\" was called before its construction completed");
}
