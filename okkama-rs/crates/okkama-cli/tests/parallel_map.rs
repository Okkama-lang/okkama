//! #946: the CLI's configured spawn transport, through the real std map.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn scratch(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("okkama-parallel-map-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    path
}
fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_okkama"))
        .args(args)
        .env(
            "OKKAMA_STDLIB_DIR",
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../std"),
        )
        .output()
        .unwrap()
}
fn succeeded(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
fn no_workers(program: &Path) {
    let output = Command::new("ps").args(["-ax", "-o", "command"]).output().unwrap();
    assert!(output.status.success());
    let listing = String::from_utf8_lossy(&output.stdout);
    let name = program.to_str().unwrap();
    assert!(
        !listing
            .lines()
            .any(|line| line.contains("--worker") && line.contains(name)),
        "worker survived settlement: {listing}"
    );
}

#[test]
fn item_failure_preserves_spawn_diagnostic_and_stops_later_items() {
    let dir = scratch("failure");
    let source = dir.join("main.ok");
    let program = dir.join("main.spb");
    std::fs::write(
        &source,
        r#"module test/parallel v1.0.0;
import "parallel/map" take ParallelMapped;
import "io/stdout" take Stdout;
object Worker {
    Worker();
    run(value is int) is int {
        Stdout().writeln("item ", value);
        confirm value != 1 else panic "intentional pool item failure";
        value;
    }
}
async main() is int {
    await ParallelMapped(origins: [0, 1, 2, 3], action: Worker().run, workers: 1).results();
    0;
}
"#,
    )
    .unwrap();
    succeeded(cli(&[
        "build",
        source.to_str().unwrap(),
        "-o",
        program.to_str().unwrap(),
    ]));
    let result = cli(&["exec", program.to_str().unwrap()]);
    assert!(!result.status.success());
    assert_eq!(String::from_utf8_lossy(&result.stdout), "item 0\nitem 1\n");
    let diagnostic = String::from_utf8_lossy(&result.stderr);
    assert!(diagnostic.contains("intentional pool item failure"), "{diagnostic}");
    assert!(!diagnostic.contains("invalid worker response"), "{diagnostic}");
    no_workers(&program);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn inline_map_contracts_leave_no_workers_after_success_cancel_or_dispatch_failure() {
    let dir = scratch("inline");
    let program = dir.join("map.spb");
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../std/parallel/map.ok");
    succeeded(cli(&[
        "build",
        "--tests",
        source.to_str().unwrap(),
        "-o",
        program.to_str().unwrap(),
    ]));
    let output = succeeded(cli(&["exec", program.to_str().unwrap()]));
    assert!(
        output.contains("--- PASS: each item reconstructs the receiver and its once state"),
        "{output}"
    );
    assert!(
        output.contains("--- PASS: caller cancellation settles outstanding workers before returning"),
        "{output}"
    );
    assert!(
        output.contains("--- PASS: dispatch failure cancels started work and prevents later input reads"),
        "{output}"
    );
    no_workers(&program);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn tests_run_by_okkama_run_t_can_start_process_workers() {
    // `okkama run -t` gives its tests the worker command, as `run` gives a
    // program: a test of a module that maps through process workers passes,
    // in the test runner and in its per-test workers (#938).
    let dir = scratch("run-t");
    let source = dir.join("mapped.ok");
    std::fs::write(
        &source,
        r#"module test/mapped v1.0.0;
import "parallel/map" take ParallelMapped;
object Tripling {
    Tripling();
    run(value is int) is int { value * 3; }
}
async test "a parallel map inside a test" {
    let mapped is ParallelMapped<int, int> = ParallelMapped(origins: [1, 2, 3], action: Tripling().run, workers: 2);
    let values = await mapped.results();
    test.is(values.item(0) + values.item(1) + values.item(2), 18);
}
"#,
    )
    .unwrap();
    let out = succeeded(cli(&["run", "-t", source.to_str().unwrap()]));
    assert!(out.contains("PASS: a parallel map inside a test"), "{out}");
    std::fs::remove_dir_all(dir).unwrap();
}
