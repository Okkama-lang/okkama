#![cfg(feature = "champ-abi")]
//! #569 C3b (open action invocation) and C6 (`any`) conformance cases, run end to
//! end in Champ in process and compared with their case.json. A case the SPB 17
//! backend does not take yet is checked through own and Low (`through_low!`);
//! a case that stops before Low is ignored with its ticket.
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

macro_rules! through_low {
    ($name:ident, $id:literal) => {
        #[test]
        fn $name() {
            check_through_low($id);
        }
    };
}

macro_rules! stops_before_low {
    ($name:ident, $id:literal, $reason:literal) => {
        #[test]
        #[ignore = $reason]
        fn $name() {
            check($id);
        }
    };
}

// C6: `any` boundaries. High, own and Low pass; SPB 17 has no `any` type and no
// role layout yet (`compile_low`: "type Any: not yet in S1"; `layout`).
through_low!(any_overloads_selection, "L03.overloads.selection");
through_low!(any_variadic_overload_by_tail_type, "L03.variadic.overload-by-tail-type");
through_low!(any_ctor_typed_overload, "L04.ctor.typed-overload");
through_low!(any_transport, "L05.any.transport");
through_low!(any_array_replaced, "L05.array.replaced");
through_low!(any_boundary_typed_values, "L05.boundary.typed-values");
through_low!(any_generic_inference, "L05.generic.inference");
through_low!(any_literal_empty_collections, "L05.literal.empty-collections");
through_low!(any_literal_heterogeneous_boundaries, "L05.literal.heterogeneous-boundaries");
through_low!(any_literal_heterogeneous_to_any, "L05.literal.heterogeneous-to-any");
through_low!(any_literal_homogeneous_inference, "L05.literal.homogeneous-inference");
stops_before_low!(any_variadic_string_tail_converts, "L03.variadic.string-tail-converts",
    "#569: OIR-2 has no char-to-string conversion");

// C3b: open action invocation. High and own pass; Low accepts own.dynamic only for
// the storage merge protocol, not the Closure transfer of an open call result.
stops_before_low!(open_anon_capture_and_escape, "L03.anon.capture-and-escape", "#641: Low and open calls");
stops_before_low!(open_anon_captures_self, "L03.anon.captures-self", "#641: Low and open calls");
stops_before_low!(open_anon_compose, "L03.anon.compose", "#641: Low and open calls");
stops_before_low!(open_anon_self_retained_only_when_read, "L03.anon.self-retained-only-when-read",
    "#641: Low and open calls");
stops_before_low!(open_anonymous_lexical_scope, "L03.anonymous.lexical-scope", "#641: Low and open calls");
stops_before_low!(open_caps_ordinary_block_body, "L04.caps.ordinary-block-body", "#641: Low and open calls");
stops_before_low!(open_invariant_escaped_self_bound_action, "L04.invariant.escaped-self-bound-action",
    "#641: Low and open calls");
stops_before_low!(open_privacy_closure_reads_self_field, "L04.privacy.closure-reads-self-field",
    "#641: Low and open calls");
stops_before_low!(open_successor_bound_action_invoked, "L04.successor.bound-action-invoked",
    "#641: Low and open calls");
stops_before_low!(open_successor_closure_transferred, "L04.successor.closure-transferred",
    "#641: Low and open calls");
stops_before_low!(open_regions_role_instances, "L07.regions.role-instances", "#641: Low and open calls");
stops_before_low!(open_collections_ordered_sets, "L09.collections.ordered-sets", "#641: Low and open calls");
