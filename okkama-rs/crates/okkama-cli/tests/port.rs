//! std's `os/port` against real Rust helpers (#1081): okkama-port's `text_helper` (the example of
//! `examples/port/text.ok`) and `port_test_helper` (concurrent calls, timeouts, a restart after a crash and after a
//! hang, raw bytes, CBOR, a pool). The helpers are built here by cargo, offline, into a target directory of this
//! test's own: the outer build's directory is locked while `cargo test` runs.
mod tree;

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

/// The examples of okkama-port, built once: the directory that holds their binaries.
fn helpers() -> &'static PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT.get_or_init(|| {
        let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("port-helpers");
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let output = Command::new(cargo)
            .args(["build", "--offline", "--locked", "-p", "okkama-port", "--example", "text_helper", "--example", "port_test_helper"])
            .arg("--target-dir")
            .arg(&target)
            .current_dir(tree::root().join("okkama-rs"))
            .output()
            .unwrap();
        assert!(output.status.success(), "building the helpers failed:\n{}", String::from_utf8_lossy(&output.stderr));
        target.join("debug").join("examples")
    })
}

/// `okkama run program` with a helper named by `variable`: its standard output, after checking it succeeded.
fn run(program: PathBuf, variable: &str, helper: &str) -> String {
    let output = tree::okkama().arg("run").arg(&program).env(variable, helpers().join(helper)).output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{} failed: {stderr}", program.display());
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn the_text_example_calls_its_rust_helper() {
    let example = tree::root().join("examples/port/text.ok");
    let expected = std::fs::read_to_string(tree::root().join("examples/port/text.expected")).unwrap();
    assert_eq!(run(example, "OKKAMA_TEXT_HELPER", "text_helper"), expected);
}

/// The same example against the Python helper written from the protocol's page alone, where a Python 3 is.
#[test]
fn the_text_example_calls_a_python_helper_alike() {
    if Command::new("python3").arg("--version").output().map(|output| !output.status.success()).unwrap_or(true) {
        eprintln!("no python3: the Python helper is not tried");
        return;
    }
    let example = tree::root().join("examples/port/text.ok");
    let expected = std::fs::read_to_string(tree::root().join("examples/port/text.expected")).unwrap();
    let output = tree::okkama()
        .arg("run")
        .arg(&example)
        .env("OKKAMA_TEXT_HELPER", tree::root().join("examples/port/text_helper.py"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
}

#[test]
fn ports_survive_crashes_hangs_and_concurrent_calls() {
    let suite = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/port/suite.ok");
    let expected = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/port/suite.expected")).unwrap();
    assert_eq!(run(suite, "OKKAMA_PORT_TEST_HELPER", "port_test_helper"), expected);
}
