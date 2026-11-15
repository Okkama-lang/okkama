//! Conformance cases blocked on scalar and string operations of SPB 17 (#675), compiled through OIR-2 and
//! run in Champ in process, each compared with its case.json.
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

/// Stages after verify_low belong to the SPB 17 / Champ lanes.
const BACKEND: [&str; 4] = ["layout", "compile_low", "verify_load", "encode"];

/// Compiles and runs one case; `Err` describes the first mismatch. With `through_low`, a case that passes
/// own and low but stops in a backend stage outside this ticket prints where it stops; once that stage
/// accepts it, the case is checked in full against case.json.
fn run_case(id: &str, through_low: bool) -> Result<(), String> {
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
    let bytes = match oir2::compile_linked_with(&roots, &mut CaseLoader { stdlib }, mode) {
        Ok(bytes) => bytes,
        Err(oir2::PipelineError::Phase { phase, message }) if through_low && BACKEND.contains(&phase) => {
            println!("{id}: own and low passed; stops in {phase}: {message}");
            return Ok(());
        }
        Err(error) => return Err(format!("compile: {error}")),
    };
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

macro_rules! case {
    ($name:ident, $id:literal) => {
        #[test]
        fn $name() {
            run_case($id, false).unwrap_or_else(|error| panic!("{}: {error}", $id));
        }
    };
}

macro_rules! through_low {
    ($name:ident, $id:literal) => {
        #[test]
        fn $name() {
            run_case($id, true).unwrap_or_else(|error| panic!("{}: {error}", $id));
        }
    };
}

// Floats: float.add/sub/mul/div/mod/pow/neg/abs/sign/sqrt.
case!(float_lex_basic, "L01.lex.float.basic");
case!(float_lex_integral_text, "L01.lex.float.integral-text");
case!(float_binary64, "L02.float.binary64");
case!(float_text_shortest_round_trip, "L02.float.text-shortest-round-trip");
case!(float_arithmetic, "L02.float.arithmetic");
case!(float_text_exponent_and_zero, "L02.float.text-exponent-and-zero");
case!(float_div_by_zero, "L02.float.div-by-zero");
case!(cast_int_out_of_range, "L02.cast.int-out-of-range");

// Strings: str.slice/split/replace/trim/chomp/repeat/case.
case!(string_length_slice, "L02.string.length-slice");
case!(string_slice_bounds, "L02.string.slice-bounds");
case!(string_slice_negative, "L02.string.slice-negative");
case!(string_slice_past_end, "L02.string.slice-past-end");
case!(string_slice_reversed, "L02.string.slice-reversed");
case!(strings_formatted, "L09.strings.formatted");
case!(string_replace, "L02.string.replace");
case!(string_replace_edge, "L02.string.replace-edge");
case!(string_trim_chomp, "L02.string.trim-chomp");
case!(string_trim_whitespace_set, "L02.string.trim-whitespace-set");
case!(string_case_and_repeat, "L02.string.case-and-repeat");
case!(string_repeat_negative, "L02.string.repeat-negative");
// str.split passes; array.join is a collection operation of another lane.
through_low!(string_split_edge, "L02.string.split-edge");
through_low!(string_split_join, "L02.string.split-join");
through_low!(harness_all_pass, "L04.harness.all-pass");

// Int bits and shifts, exact ints beyond 64 bits.
case!(precedence_arithmetic, "L01.gram.precedence.arithmetic");
case!(precedence_bitwise_vs_comparison, "L01.gram.precedence.bitwise-vs-comparison");
case!(precedence_associativity, "L01.gram.precedence.associativity");
case!(shift_int, "L02.shift.int");
case!(shift_reject_count_64, "L02.shift.reject-count-64");
case!(shift_reject_count_negative, "L02.shift.reject-count-negative");
case!(operators_ampersand_forms, "L01.lex.operators.ampersand-forms");
case!(operators_nested_generic_closers, "L01.lex.operators.nested-generic-closers");
case!(bitwise_int, "L02.bitwise.int");
case!(bit_methods_int_and_byte, "L02.bit-methods.int-and-byte");
case!(bit_methods_rotate_out_of_range, "L02.bit-methods.rotate-out-of-range");
case!(int_pow_exact, "L02.int.pow-exact");

// Bytes, including `byte >>> int`.
case!(bitwise_byte, "L02.bitwise.byte");
case!(shift_byte, "L02.shift.byte");
case!(shift_reject_byte_count_8, "L02.shift.reject-byte-count-8");

// Rats: rat.add/div/numerator, rat constants.
case!(rat_arithmetic, "L02.rat.arithmetic");
case!(rat_division, "L02.rat.division");
case!(rat_constructor, "L02.rat.constructor");

// `any` coercion passes own and low; maps and closures are SPB 17 types of other lanes.
through_low!(types_forms, "L01.gram.types.forms");

// Char conversions: `cvt` char -> int, int -> char, char -> str (#679).
case!(char_raw_string_keeps_cr, "L01.lex.crlf.raw-string-keeps-cr");
case!(char_code_round_trip, "L02.char.code-round-trip");
case!(char_string_cast, "L02.char.string-cast");
case!(char_surrogate_rejected, "L02.char.surrogate-rejected");
// `LETTER.string()` lowers; the collection constants are globals, which the SPB 17 layout does not take yet.
through_low!(const_values, "L06.const.values");
