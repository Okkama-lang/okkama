#![cfg(feature = "champ-abi")]
//! Conformance cases of the #569 lowering lane, in process: OIR-2 high -> own -> low -> SPB 17 -> Champ.
//! A case that builds runs in Champ and must match its `case.json` (stdout and exit). A case that
//! does not build yet must stop in the backend (SPB 17 layout or generation), never before it:
//! the producer, own and low already accept every listed case.

use okkama_compiler::lowering::{LoadedModule, ModuleLoader, RootFile, oir2};
use okkama_compiler::syntax::{self, ParseOptions};
use okkama_compiler::SourceFile;
use serde_json::Value;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// `L02.string.split-join` -> `conformance/cases/L02-values-and-operators/string.split-join`.
fn case_directory(id: &str) -> PathBuf {
    let (level, rest) = id.split_once('.').unwrap();
    let prefix = format!("{level}-");
    let level = std::fs::read_dir(root().join("conformance/cases")).unwrap().map(|entry| entry.unwrap().path())
        .find(|path| path.file_name().unwrap().to_str().unwrap().starts_with(&prefix)).unwrap();
    level.join(rest)
}

/// Support modules next to the case, then the conformance harness, then std.
struct CaseLoader {
    directory: PathBuf,
}

impl ModuleLoader for CaseLoader {
    fn load(&mut self, path: &str, _: &str) -> Result<Option<LoadedModule>, String> {
        let local = self.directory.join(format!("{path}.ok"));
        let file = if local.is_file() {
            local
        } else if path == "conformance/check" {
            root().join("conformance/harness/okkama/conformance/check.ok")
        } else {
            root().join("std").join(format!("{path}.ok"))
        };
        let text = std::fs::read_to_string(&file).map_err(|error| error.to_string())?;
        let source = SourceFile::new(file.to_string_lossy(), text).map_err(|error| error.to_string())?;
        let program = syntax::parse(&source, &ParseOptions::default()).map_err(|error| format!("{error:?}"))?;
        let program = syntax::prune::strip_test_items(program);
        let id = std::fs::canonicalize(&file).map_err(|error| error.to_string())?;
        Ok(Some(LoadedModule { id: id.to_string_lossy().into_owned(), source, program }))
    }
}

const BACKEND: &[&str] = &["layout", "spb_contracts", "compile_low", "verify_load", "encode"];

/// The verdict when the case ran and matched, or stops in the backend; otherwise why it fails.
fn check(id: &str) -> Result<String, String> {
    let directory = case_directory(id);
    let case: Value = serde_json::from_str(&std::fs::read_to_string(directory.join("case.json")).unwrap()).unwrap();
    let text = std::fs::read_to_string(directory.join("main.ok")).unwrap();
    let source = SourceFile::new("main.ok", text).unwrap();
    let program = syntax::parse(&source, &ParseOptions::default()).unwrap();
    let roots = [RootFile { source: &source, program: &program }];
    let mut loader = CaseLoader { directory };
    let bytes = match oir2::compile_linked_with(&roots, &mut loader, okkama_compiler::oir2::VerifyMode::Structural) {
        Ok(bytes) => bytes,
        Err(oir2::PipelineError::Phase { phase, message }) if BACKEND.contains(&phase) => {
            return Ok(format!("stops in SPB 17 ({phase}): {message}"));
        }
        Err(error) => return Err(format!("stopped before the backend: {error}")),
    };
    let limits = sparrow_core::budget::Limits { steps: Some(10_000_000), ..sparrow_core::budget::Limits::default() };
    let run = sparrow_core::run_captured(bytes, limits).map_err(|error| format!("load: {error}"))?;
    let stdout = String::from_utf8(run.stdout).map_err(|error| error.to_string())?;
    let expect = &case["expect"];
    if let Some(expected) = expect["stdout"]["text"].as_str() && stdout != expected {
        return Err(format!("stdout {stdout:?}, expected {expected:?}"));
    }
    match (expect["outcome"].as_str(), run.result) {
        (Some("success"), Ok(code)) if Some(code) == expect["exit_status"].as_i64() => Ok("Champ matched".into()),
        (Some("success"), result) => Err(format!("expected exit {}, got {result:?}", expect["exit_status"])),
        (_, Ok(0)) => Err("expected a failure, the program succeeded".into()),
        (_, _) => Ok("Champ failed as expected".into()),
    }
}

/// The cases that the producer, own and low accept in build mode (see the oir2 case tests).
const CASES: &[&str] = &[
    "L00.stdout.stream-write", "L01.gram.precedence.arithmetic", "L01.gram.precedence.associativity",
    "L01.gram.precedence.bitwise-vs-comparison", "L01.gram.unpack.forms", "L01.lex.float.basic",
    "L01.lex.float.integral-text", "L01.lex.operators.ampersand-forms", "L01.lex.operators.nested-generic-closers",
    "L01.lex.operators.stream-write-vs-less-minus", "L02.bit-methods.int-and-byte",
    "L02.bit-methods.rotate-out-of-range", "L02.bitwise.byte", "L02.bitwise.int", "L02.cast.int-out-of-range",
    "L02.float.arithmetic", "L02.float.binary64", "L02.float.div-by-zero", "L02.float.text-exponent-and-zero",
    "L02.float.text-shortest-round-trip", "L02.int.byte-promotion", "L02.int.pow-exact", "L02.rat.arithmetic",
    "L02.rat.constructor", "L02.rat.division", "L02.shift.int", "L02.shift.reject-byte-count-8",
    "L02.shift.reject-count-64", "L02.shift.reject-count-negative", "L02.string.case-and-repeat",
    "L02.string.length-slice", "L02.string.repeat-negative", "L02.string.replace", "L02.string.replace-edge",
    "L02.string.slice-bounds", "L02.string.slice-negative", "L02.string.slice-past-end", "L02.string.slice-reversed",
    "L02.string.trim-chomp", "L02.string.trim-whitespace-set", "L03.match.enum-and-strings", "L05.choice.enum",
    "L06.file-identity.path-aliases", "L07.catch-pattern.cases-and-types", "L07.catch-pattern.unmatched-propagates",
    "L07.confirm.lazy-else", "L09.strings.formatted", "L09.time.operators",
];

// Collect every verdict; a case stopping before the backend or a Champ mismatch fails the test.
#[test]
fn champ_cases_lowering() {
    let mut failures = vec![];
    for id in CASES {
        match check(id) {
            Ok(verdict) => println!("CASE {id}: {verdict}"),
            Err(reason) => {
                println!("CASE {id}: FAILED {reason}");
                failures.push(format!("{id}: {reason}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
