//! Conformance cases compiled through OIR-2 and run in Champ, in process (#622).
//!
//! Only pure language cases: no processes, workers, network, outside files or test mode.
#![cfg(feature = "champ-abi")]

use okkama_compiler::lowering::{LoadedModule, ModuleLoader, RootFile, oir2};
use okkama_compiler::syntax::{self, ParseOptions};
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

/// Compiles and runs one case; `Err` describes the first mismatch.
fn run_case(id: &str) -> Result<(), String> {
    let dir = case_dir(id);
    let case: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("case.json")).unwrap()).unwrap();
    assert_eq!(case["mode"], "run", "{id}: only run-mode cases");
    let mut sources = Vec::new();
    for name in case["sources"].as_array().unwrap() {
        let file = std::fs::canonicalize(dir.join(name.as_str().unwrap())).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        sources.push(SourceFile::new(file.to_string_lossy(), text).unwrap());
    }
    let mut programs = Vec::new();
    for source in &sources {
        let program = syntax::parse(source, &ParseOptions::default()).map_err(|error| format!("parse: {error:?}"))?;
        programs.push(syntax::prune::strip_test_items(program));
    }
    let roots = sources.iter()
        .zip(&programs)
        .map(|(source, program)| RootFile { source, program })
        .collect::<Vec<_>>();
    let stdlib = std::fs::canonicalize(root().join("std")).unwrap();
    let mode = okkama_compiler::oir2::VerifyMode::Structural;
    let bytes = oir2::compile_linked_with(&roots, &mut CaseLoader { stdlib }, mode)
        .map_err(|error| format!("compile: {error}"))?;
    let limits = sparrow_core::budget::Limits {
        steps: Some(10_000_000),
        ..sparrow_core::budget::Limits::default()
    };
    let run = sparrow_core::run_captured(bytes, limits).map_err(|error| format!("load: {error}"))?;
    let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    let expect = &case["expect"];
    let expected_stdout = expect["stdout"]["text"].as_str().unwrap_or("");
    match (expect["outcome"].as_str().unwrap(), &run.result) {
        ("success", Ok(code)) => {
            let status = expect["exit_status"].as_i64().unwrap_or(0);
            if *code != status {
                return Err(format!("exit {code}, expected {status}; stdout={stdout:?}"));
            }
        }
        ("success", Err(error)) => return Err(format!("failed: {}; stdout={stdout:?}", error.0)),
        (_, Ok(code)) => return Err(format!("completed with {code}, expected a failure; stdout={stdout:?}")),
        (_, Err(_)) => {}
    }
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

/// Runs every id of the list named by `CHAMP_CASES` (one id per line) and prints the count.
#[test]
#[ignore = "a measurement over a case list: CHAMP_CASES=<file>"]
fn champ_cases_list() {
    let Ok(list) = std::env::var("CHAMP_CASES") else {
        return;
    };
    let text = std::fs::read_to_string(&list).unwrap();
    let ids = text.lines().map(str::trim).filter(|line| !line.is_empty()).collect::<Vec<_>>();
    let mut passed = 0;
    for id in &ids {
        let result = std::panic::catch_unwind(|| run_case(id));
        match result {
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

/// Terminators that left a live service behind a total conversion's raise exit (cases-terminal).
#[test]
fn champ_cases_total_conversions_end_their_services() {
    check(&[
        "L01.gram.call.args",
        "L02.int.arith-basic",
        "L03.if.statement-and-expression",
        "L06.resolve.local-module",
        "L07.defer.lifo-and-lazy",
    ]);
}

/// A total conversion in a function that cannot fail (#658, cases-mask).
#[test]
fn champ_cases_total_conversions_keep_the_failure_mask() {
    check(&["L01.gram.has.forms", "L04.successor.chain", "L04.shorthand.named-only"]);
}

/// A successor alias forwarded after its succ.take (cases-succ).
#[test]
fn champ_cases_taken_successor_aliases() {
    check(&["L04.successor.failed-transition", "L04.successor.new-object"]);
}

/// Byte/Float/Rat/Char, plain enums, value equality and plain variants (cases-s1types).
#[test]
fn champ_cases_scalar_enum_and_variant_types() {
    check(&[
        "L01.lex.float.exponent",
        "L01.lex.char.basic-and-escapes",
        "L02.byte.negative",
        "L02.cast.methods",
        "L02.rat.division-signs",
        "L02.int.div-by-zero",
        "L04.enums.values",
        "L04.equality.includes-private-fields",
        "L05.variants.payload",
        "L05.maybe.presence",
    ]);
}

/// Builders with element journals, maps, lookups and plain collection operations (#622).
#[test]
fn champ_cases_builders_and_collections() {
    check(&[
        "L01.gram.functional.inside-operators",
        "L01.gram.functional.suffix",
        "L01.lex.contextual.has-action-and-field",
        "L01.lex.float.range-not-float",
        "L02.cast.arrays",
        "L02.range.empty-and-single",
        "L02.range.expansion-bounds",
        "L02.string.order-consistent",
        "L03.functional.empty-sources",
        "L03.return.from-functional-block",
        "L05.array.search-and-shape",
        "L05.reduce.accumulator-in-place",
        "L05.collections.array-difference",
        "L05.collections.array-concat",
    ]);
}

#[test]
fn champ_cases_maps() {
    check(&[
        "L01.gram.collections.literals",
        "L01.gram.types.nested-map",
        "L01.lex.whitespace.minimal",
        "L03.functional.arrays-ranges-maps",
        "L05.collections.map-difference",
        "L05.functional.early-exit",
        "L05.map.int-key-order",
        "L05.map.key-types",
        "L05.map.missing-key",
        "L05.map.float-key-fallback",
        "L05.map.rat-keys",
        "L05.typed-identity.empty-results-keep-tag",
    ]);
}

/// array.slice/remove/replace/concat/join, value equality of plain collections and ranges.
#[test]
fn champ_cases_array_operations_and_equality() {
    check(&[
        "L01.lex.string.unicode-escape",
        "L02.cast.array-element-out-of-range",
        "L02.cast.int-arbitrary-precision",
        "L02.equality.composite",
        "L02.equality.composite-boundary",
        "L02.eval-order.cast-operands",
        "L04.harness.deliberate-failures",
        "L05.array.access",
        "L05.array.removed-at-empty",
        "L05.array.removed-at-past-end",
        "L05.array.replaced-negative",
        "L05.array.replaced-past-end",
        "L05.array.take-negative",
        "L05.variants.search",
    ]);
}

/// Arrays and maps that refer to resource objects they do not own, and variants whose case holds one.
#[test]
fn champ_cases_managed_collections_and_variants() {
    check(&[
        "L07.ownership.shared-structure-loop",
        "L07.ownership.shared-structure-returned",
        "L07.ownership.reduce-builder",
        "L05.variants.ownership",
    ]);
}

/// Constructions whose self escapes before the last field is written.
#[test]
fn champ_cases_construction_completion() {
    check(&["L04.equality.cyclic-graphs", "L04.invariant.early-self-in-blocks"]);
}

/// Single blockers of the wave-4 conformance run (#710): an `Atomic` of a float array (the registered
/// specializations cover float, char and rat leaves), a literal choice whose other arm holds a collection
/// the join's empty type cannot take, and natives over float arrays.
#[test]
fn champ_cases_wave4_single_blockers() {
    check(&["L02.equality.signed-zero-composite", "L05.choice.empty-collections", "L09.statistics.exact-samples"]);
}
