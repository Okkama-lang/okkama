//! IR passes keep meaning (H-09, #29): programs print the same with every
//! pass, each pass alone and none, and a deliberately wrong pass is caught.

use std::process::Command;



/// Stdout, stderr and status of `program` on Sparrow with `passes`.
fn run(program: &str, passes: &str) -> (String, String, Option<i32>) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/ir-passes/{program}.ok"));
    let output = Command::new(env!("CARGO_BIN_EXE_okkama"))
        .args(["run"])
        .arg(&path)
        .env("OKKAMA_IR_PASSES", passes)
        .env("OKKAMA_STDLIB_DIR", std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../std"))
        .output()
        .unwrap();
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code(),
    )
}

fn every_pass_selection(program: &str) {
    let reference = run(program, "none");
    for passes in ["all", "none"].into_iter().chain(okkama_compiler::lowering::PASSES) {
        assert_eq!(run(program, passes), reference, "{program} with OKKAMA_IR_PASSES={passes}");
    }
}

#[test]
fn arithmetic_with_every_pass_selection_prints_the_same() {
    let reference = run("arithmetic", "none");
    assert_eq!(reference.2, Some(0), "{}", reference.1);
    assert!(reference.1.is_empty(), "{}", reference.1);
    assert_eq!(reference.0, concat!(
        "-9223372036854775808\n-9223372036854775809\n",
        "85070591730234615865843651857942052864\n9223372036854775808\n",
        "1499500\n-1 1\n",
    ));
    every_pass_selection("arithmetic");
}

#[test]
fn failures_with_every_pass_selection_print_the_same() {
    every_pass_selection("failures");
    let (stdout, stderr, status) = run("failures", "all");
    assert_eq!(stdout, "close inner-4\nclose outer-4\n40\n5000050000\n");
    assert!(stderr.starts_with("RuntimeError: Modulo by zero"), "{stderr}");
    assert_eq!(status, Some(1));
}

#[test]
fn a_wrong_pass_is_caught() {
    assert_ne!(run("arithmetic", "sabotage-int-add"), run("arithmetic", "none"));
}
