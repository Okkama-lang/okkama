//! Champ counts steps as Tomato does: function entries and loop back-edges, a self tail call compiled as a
//! jump included. An endless loop under a step budget stops with Tomato's report (#692).
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

fn run(text: &str, steps: u64) -> Result<i64, String> {
    let source = SourceFile::new("app.ok", text).unwrap();
    let program = syntax::parse(&source, &ParseOptions::default()).unwrap();
    let roots = [RootFile { source: &source, program: &program }];
    let bytes = oir2::compile_linked_with(&roots, &mut StdLoader, okkama_compiler::oir2::VerifyMode::Structural)
        .unwrap_or_else(|error| panic!("{error}\n{text}"));
    let limits = sparrow_core::budget::Limits { steps: Some(steps), ..sparrow_core::budget::Limits::default() };
    sparrow_core::run_captured(bytes, limits).unwrap().result.map_err(|error| error.0)
}

/// A self call in tail position; it never reaches its base case from `start`.
fn tail_recursion(start: i64) -> String {
    format!("module app;

object Counter {{
    Counter();
    spin(n is int) is int {{
        if n < 0 {{
            n + 1;
        }} else {{
            self.spin(n + 1);
        }}
    }}
}}

main() is int {{
    Counter().spin({start});
}}
")
}

const EXHAUSTED: &str = "Execution budget exhausted: steps";

#[test]
fn endless_tail_recursion_exhausts_the_step_budget() {
    assert_eq!(run(&tail_recursion(0), 100_000), Err(EXHAUSTED.into()));
}

#[test]
fn tail_recursion_that_ends_runs_within_the_step_budget() {
    // The base case returns at once: 0.
    assert_eq!(run(&tail_recursion(-1), 100_000), Ok(0));
}

#[test]
fn endless_loop_exhausts_the_step_budget() {
    let text = "module app;

main() is int {
    for 0..1000000000000 -> i {
        i;
    }
    0;
}
";
    assert_eq!(run(text, 100_000), Err(EXHAUSTED.into()));
}
