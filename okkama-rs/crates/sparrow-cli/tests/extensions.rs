//! Native switches on Sparrow's direct, run, test and worker entry points.
#![cfg(unix)]
use std::{path::PathBuf, process::Command};

#[test]
fn native_extensions_on_sparrow_entry_points() {
    if Command::new("cc").arg("--version").output().is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
        eprintln!("no cc on PATH: Sparrow extension test skipped");
        return;
    }
    let crates = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("sparrow-extensions-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let library = dir.join(format!("sample.{}", std::env::consts::DLL_EXTENSION));
    let output = Command::new("cc").args(["-shared", "-fPIC", "-std=c11"])
        .arg("-I").arg(crates.join("okkama-runtime-abi/include"))
        .arg(crates.join("okkama-cli/tests/extensions/c/sample.c"))
        .arg("-o").arg(&library).output().unwrap();
    assert!(output.status.success(), "stdout: {}\nstderr: {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    let mut program = okkama_sparrow::assemble(r#"sparrow 19
module "sample" "0.0.0"
type t0 int
function f0 "add" native "sample@0::add/2" {
 registers [t0, t0]
 params [r0, r1]
 result t0
}
function f1 "main" {
 registers [t0, t0, t0]
 params []
 result t0
 0: const r0, int 17
 1: const r1, int 29
 2: call r2, f0, [r0, r1]
 3: ret r2
}
entry f1
"#).unwrap();
    let path = dir.join("program.spb");
    std::fs::write(&path, okkama_sparrow::encode(&program).unwrap()).unwrap();
    let run = |prefix: &[&str], disable: bool, path: &std::path::Path| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_sparrow"));
        command.args(prefix).arg("--ext").arg(&library);
        if disable { command.arg("--no-native"); }
        command.arg(path).output().unwrap()
    };
    for prefix in [vec![], vec!["run"], vec!["--stats"]] {
        let output = run(&prefix, false, &path);
        assert_eq!(output.status.code(), Some(46), "{}", String::from_utf8_lossy(&output.stderr));
        let output = run(&prefix, true, &path);
        assert_eq!(output.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&output.stderr).contains("is not registered"));
    }
    program.functions[1].result = None;
    *program.functions[1].code.last_mut().unwrap() = okkama_sparrow::Instruction::Return { value: None };
    program.entry = None;
    program.tests = Some(vec![("extension".into(), okkama_sparrow::FuncRef(1))]);
    let path = dir.join("tests.spb");
    std::fs::write(&path, okkama_sparrow::encode(&program).unwrap()).unwrap();
    for prefix in [vec!["test"], vec!["--test"]] {
        let output = run(&prefix, false, &path);
        assert!(output.status.success(), "stdout: {}\nstderr: {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        let output = run(&prefix, true, &path);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("is not registered"));
    }
    let output = Command::new(env!("CARGO_BIN_EXE_sparrow")).args(["--worker", "--ext"])
        .arg(library.with_file_name("missing.so")).arg(&path).output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("missing.so"));
}
