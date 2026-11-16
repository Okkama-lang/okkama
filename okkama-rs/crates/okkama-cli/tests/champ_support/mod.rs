//! Compile through OIR-2 and run in Champ, in process: conformance case directories and inline sources.
#![allow(dead_code)]

use okkama_compiler::lowering::{LoadedModule, ModuleLoader, RootFile, oir2};
use okkama_compiler::syntax::{self, ParseOptions};
use okkama_compiler::SourceFile;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn harness() -> PathBuf {
    root().join("conformance/harness/okkama")
}

/// Imports resolve next to the importing file first, then in the suite's harness module directory (the
/// runner copies it into the case), then in std; std files import from std.
struct CaseLoader {
    stdlib: PathBuf,
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

pub fn case_dir(id: &str) -> PathBuf {
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

/// SPB 17 of the sources through the OIR-2 pipeline with structural verification.
pub fn compile(sources: &[SourceFile]) -> Result<Vec<u8>, String> {
    let mut programs = Vec::new();
    for source in sources {
        let program = syntax::parse(source, &ParseOptions::default()).map_err(|error| format!("parse: {error:?}"))?;
        programs.push(syntax::prune::strip_test_items(program));
    }
    let roots = sources.iter()
        .zip(&programs)
        .map(|(source, program)| RootFile { source, program })
        .collect::<Vec<_>>();
    let stdlib = std::fs::canonicalize(root().join("std")).unwrap();
    let mode = okkama_compiler::oir2::VerifyMode::Structural;
    oir2::compile_linked_with(&roots, &mut CaseLoader { stdlib }, mode).map_err(|error| format!("compile: {error}"))
}

/// What a Champ run printed and how it ended: the exit status, or the failure report.
pub struct Run {
    pub stdout: String,
    pub result: Result<i64, String>,
}

pub fn run(bytes: Vec<u8>) -> Result<Run, String> {
    run_with_arguments(bytes, Vec::new())
}

/// [`run`] with program arguments (a case's `args`).
pub fn run_with_arguments(bytes: Vec<u8>, arguments: Vec<Vec<u8>>) -> Result<Run, String> {
    let limits = sparrow_core::budget::Limits {
        steps: Some(10_000_000),
        ..sparrow_core::budget::Limits::default()
    };
    let run = sparrow_core::run_captured_with_arguments(bytes, arguments, limits)
        .map_err(|error| format!("load: {error}"))?;
    Ok(Run {
        stdout: String::from_utf8_lossy(&run.stdout).into_owned(),
        result: run.result.map_err(|error| error.0),
    })
}

/// An inline root module, named inside the crate so that std imports resolve.
pub fn run_source(text: &str) -> Result<Run, String> {
    let name = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/champ_support/inline.ok");
    let source = SourceFile::new(name.to_string_lossy(), text.to_string()).unwrap();
    run(compile(&[source])?)
}

/// Compiles and runs one case; `Err` describes the first mismatch.
pub fn run_case(id: &str) -> Result<(), String> {
    let dir = case_dir(id);
    let case: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("case.json")).unwrap()).unwrap();
    assert_eq!(case["mode"], "run", "{id}: only run-mode cases");
    let mut sources = Vec::new();
    for name in case["sources"].as_array().unwrap() {
        let file = std::fs::canonicalize(dir.join(name.as_str().unwrap())).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        sources.push(SourceFile::new(file.to_string_lossy(), text).unwrap());
    }
    let arguments = case["args"].as_array()
        .map(|args| args.iter().map(|arg| arg.as_str().unwrap().as_bytes().to_vec()).collect())
        .unwrap_or_default();
    let run = run_with_arguments(compile(&sources)?, arguments)?;
    let stdout = run.stdout;
    let expect = &case["expect"];
    let expected_stdout = expect["stdout"]["text"].as_str().unwrap_or("");
    match (expect["outcome"].as_str().unwrap(), &run.result) {
        ("success", Ok(code)) => {
            let status = expect["exit_status"].as_i64().unwrap_or(0);
            if *code != status {
                return Err(format!("exit {code}, expected {status}; stdout={stdout:?}"));
            }
        }
        ("success", Err(error)) => return Err(format!("failed: {error}; stdout={stdout:?}")),
        (_, Ok(code)) => return Err(format!("completed with {code}, expected a failure; stdout={stdout:?}")),
        (_, Err(_)) => {}
    }
    if stdout != expected_stdout {
        return Err(format!("stdout {stdout:?}, expected {expected_stdout:?}"));
    }
    Ok(())
}

pub fn check(ids: &[&str]) {
    let failures = ids.iter()
        .filter_map(|id| run_case(id).err().map(|error| format!("{id}: {error}")))
        .collect::<Vec<_>>();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Runs every id of the list named by `CHAMP_CASES` (one id per line) and prints the count.
pub fn measure() {
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
