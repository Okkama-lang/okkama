#![cfg(feature = "champ-abi")]

use okkama_compiler::lowering::{LoadedModule, ModuleLoader, RootFile, oir2};
use okkama_compiler::syntax::{self, ParseOptions};
use okkama_compiler::SourceFile;
use serde_json::Value;
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
    oir2::compile_linked(&roots, &mut StdLoader)
}

fn manifest() -> Value {
    serde_json::from_str(include_str!("../../../../tools/ownership/p1-manifest.json")).unwrap()
}

fn check(id: &str) {
    let manifest = manifest();
    let entry = manifest["entries"].as_array().unwrap().iter().find(|entry| entry["id"] == id).unwrap();
    assert_eq!(entry["expected"], "supported");
    let text = std::fs::read_to_string(root().join(entry["source"].as_str().unwrap())).unwrap();
    let bytes = compile(&text).unwrap_or_else(|error| panic!("{id}: {error}"));
    let limits = sparrow_core::budget::Limits {
        steps: Some(100_000),
        ..sparrow_core::budget::Limits::default()
    };
    let run = sparrow_core::run_captured(bytes, limits).unwrap_or_else(|error| panic!("{id}: {error}"));
    let (code, stderr) = match run.result {
        Ok(code) => (code, String::new()),
        Err(error) => (1, format!("{}\n", error.0)),
    };
    assert_eq!(code, entry["failure"]["code"].as_i64().unwrap(), "{id}: exit");
    assert_eq!(stderr, entry["failure"]["stderr"].as_str().unwrap(), "{id}: stderr");
    let stdout = String::from_utf8(run.stdout).unwrap();
    assert_eq!(stdout, entry["stdout"].as_str().unwrap(), "{id}: stdout");
    let checkpoints = entry["checkpoints"].as_array().unwrap().iter().map(|value| value.as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(stdout.lines().collect::<Vec<_>>(), checkpoints, "{id}: checkpoints");
    let identities = entry["identities"].as_array().unwrap();
    let expected_trace = entry["destroy"].as_array().unwrap().iter().map(|event| {
        let identity = identities.iter().find(|identity| identity["event"] == *event).unwrap();
        let reference = &identity["B"];
        assert_eq!(reference["block"], 0);
        format!("{} {}\n", reference["type"].as_str().unwrap(), reference["serial"].as_u64().unwrap())
    }).collect::<String>();
    assert_eq!(String::from_utf8(run.destroy_trace).unwrap(), expected_trace, "{id}: creation-event trace");
    println!("{id}: checkpoints={checkpoints:?}; trace={expected_trace:?}");
}

macro_rules! fixture {
    ($name:ident, $id:literal) => {
        #[test]
        fn $name() {
            check($id);
        }
    };
}

#[test]
fn p1_pipeline_manifest_inventory() {
    let manifest = manifest();
    let entries = manifest["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 25, "review the accepted P1 test count before changing it");
    for entry in entries {
        assert_eq!(entry["rust_test"], entry["id"].as_str().unwrap().replace('-', "_"));
    }
}

#[test]
fn p1_pipeline_scalar_run() {
    let bytes = compile("module app; main() is int { 7; }").unwrap();
    let run = sparrow_core::run_captured(bytes, sparrow_core::budget::Limits::default()).unwrap();
    assert_eq!(run.result.unwrap(), 7);
    assert!(run.stdout.is_empty());
    assert!(run.destroy_trace.is_empty());
}

#[test]
fn p1_pipeline_unsupported_does_not_fallback() {
    let error = compile("module app; main() is int { let array = [1]; 0; }").unwrap_err();
    assert!(error.is_unsupported(), "{error}");
    assert!(error.to_string().contains("unsupported in OIR-2 lowering"));
}
fixture!(scopes_nested_shadowing, "scopes-nested-shadowing");
fixture!(scopes_temporary_tail, "scopes-temporary-tail");
fixture!(branches_discarded_if_308, "branches-discarded-if-308");
fixture!(branches_wanted_match, "branches-wanted-match");
fixture!(temporary_arguments, "temporary-arguments");
fixture!(temporary_receivers, "temporary-receivers");
fixture!(call_results, "call-results");
fixture!(call_chains, "call-chains");
fixture!(rebind_history_defer, "rebind-history-defer");
fixture!(successor_staging, "successor-staging");
fixture!(loops_next_last, "loops-next-last");
fixture!(loops_valued_nested, "loops-valued-nested");
fixture!(defer_order, "defer-order");
fixture!(unwind_late_argument, "unwind-late-argument");
fixture!(unwind_nested, "unwind-nested");
fixture!(destroy_failures, "destroy-failures");
fixture!(try_resource_results, "try-resource-results");
fixture!(partial_construction, "partial-construction");
fixture!(secondary_failures, "secondary-failures");
fixture!(panic_unwind, "panic-unwind");
fixture!(direct_singleton, "direct-singleton");
fixture!(late_old_identity, "late-old-identity");
fixture!(mixed_merge, "mixed-merge");
fixture!(reserve_r_a, "reserve-r-a");
fixture!(reserve_a_r, "reserve-a-r");
