//! `okkama run -t --workers N [--test-timeout SECS]` (#938): tests in process
//! workers, with captured output, deadlines and crashes, reported in
//! declaration order. Runs `tests/test_workers/suite.ok`.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A copy of the suite under a name of its own, so that the tests, which run
/// in parallel, see only their own workers.
fn suite(tag: &str) -> PathBuf {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/test_workers/suite.ok");
    let dir = std::env::temp_dir().join(format!("okkama-test-workers-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch directory");
    let copy = dir.join("suite.ok");
    std::fs::copy(&source, &copy).expect("copy the suite");
    copy
}

fn okkama(args: &[&str]) -> Output {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    Command::new(env!("CARGO_BIN_EXE_okkama"))
        .args(args)
        .env("OKKAMA_STDLIB_DIR", here.join("../../../std"))
        .output()
        .expect("run okkama")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// No process of ours still runs this suite.
fn no_worker_left(path: &Path) {
    let ps = Command::new("ps").args(["-ax", "-o", "command"]).output().expect("ps");
    let running = text(&ps.stdout);
    let needle = path.display().to_string();
    assert!(
        !running.lines().any(|line| {
            line.starts_with(env!("CARGO_BIN_EXE_okkama")) && line.contains("--worker-test") && line.contains(&needle)
        }),
        "a worker is left:\n{running}"
    );
}

#[test]
fn workers_report_every_ending_in_declaration_order() {
    let path = suite("human");
    let file = path.to_str().unwrap();
    let output = okkama(&["run", "-t", "--workers", "3", "--test-timeout", "3", file]);
    let stdout = text(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{stdout}\n{}", text(&output.stderr));
    let runs: Vec<&str> = stdout.lines().filter_map(|line| line.strip_prefix("=== RUN   ")).collect();
    assert_eq!(runs, ["slow", "passes", "fails", "skips", "floods", "hangs", "crashes"], "{stdout}");
    for wanted in [
        "--- PASS: slow",
        "--- PASS: passes",
        "--- FAIL: fails\n    expected 5, got 4\n    stdout:\n        before the check\n",
        "--- SKIP: skips\n    not here\n",
        "--- FAIL: hangs\n    deadline of 3000 ms passed: the worker was stopped\n",
        "--- FAIL: crashes\n    the worker ended (killed by signal 9) before it reported\n",
        "        [truncated]\n",
    ] {
        assert!(stdout.contains(wanted), "missing {wanted:?} in\n{stdout}");
    }
    assert!(!stdout.contains("to stdout"), "a passing test's output is not shown in the human report");
    assert!(stdout.ends_with("FAIL\n"), "{stdout}");
    no_worker_left(&path);
}

#[test]
fn json_carries_the_captured_streams_and_the_new_outcomes() {
    let path = suite("json");
    let file = path.to_str().unwrap();
    let output = okkama(&["run", "-t", "--workers", "1", "--test-timeout", "3", "--report", "json", file]);
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).expect("one JSON document");
    let tests = document["tests"].as_array().unwrap();
    let by = |name: &str| tests.iter().find(|test| test["name"] == name).unwrap().clone();
    assert_eq!(by("passes")["stdout"], "to stdout\n");
    assert_eq!(by("passes")["stderr"], "to stderr\n");
    assert_eq!(by("passes")["truncated"], serde_json::json!([]));
    assert_eq!(by("floods")["stdout"].as_str().unwrap().len(), 64 * 1024);
    assert_eq!(by("floods")["truncated"], serde_json::json!(["stdout"]));
    assert_eq!(by("hangs")["outcome"], "timed_out");
    assert_eq!(by("crashes")["outcome"], "crashed");
    assert_eq!(by("slow")["position"], 0);
    assert_eq!(document["totals"], serde_json::json!({"failed": 4, "passed": 2, "skipped": 1, "tests": 7}));
    no_worker_left(&path);
}

#[test]
fn worker_options_are_checked() {
    let path = suite("options");
    let file = path.to_str().unwrap();
    for (args, message) in [
        (vec!["run", "-t", "--workers", "0", file], "--workers needs a whole number from 1"),
        (vec!["run", "--workers", "2", file], "--workers runs tests"),
        (vec!["run", "-t", "--test-timeout", "1", file], "--test-timeout needs --workers"),
        (vec!["run", "-t", "--workers", "1", "--test-timeout", "0", file], "--test-timeout needs seconds"),
    ] {
        let output = okkama(&args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(text(&output.stderr).contains(message), "{args:?}: {}", text(&output.stderr));
    }
    let only = okkama(&["run", "-t", "--workers", "2", "--filter", "pass", file]);
    assert_eq!(only.status.code(), Some(0), "{}", text(&only.stdout));
    assert_eq!(text(&only.stdout), "=== RUN   passes\n--- PASS: passes\nPASS\n");
}

/// #1011: a test that `--test-timeout` kills keeps what it printed: the
/// worker writes its stdout through, nothing waits in a buffer at the kill.
#[test]
fn a_killed_test_keeps_its_output() {
    let dir = std::env::temp_dir().join(format!("okkama-test-workers-{}-killed", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch directory");
    let file = dir.join("slow.ok");
    std::fs::write(
        &file,
        "module slow;\nimport \"io/stdout\" take Stdout;\nimport \"time/clock\" take SystemClock;\nimport \"time/duration\" take Duration;\n\
         async test \"prints then hangs\" {\n    Stdout().writeln(\"progress: step 1 done\");\n    await SystemClock().wait(Duration(seconds: 5.rat()));\n    test.ok(true);\n}\n",
    )
    .expect("write the suite");
    let path = file.display().to_string();
    let output = okkama(&["run", "-t", "--workers", "1", "--test-timeout", "0.5", "--report", "json", &path]);
    let report = text(&output.stdout);
    assert!(report.contains("progress: step 1 done"), "{report}");
    no_worker_left(&file);
}
