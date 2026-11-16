//! #601 N1: async natives on Champ (OIR-2 async ABI D22, ruling PR-N), compiled through OIR-2 and run in process.
//! Conformance cases are checked in full against their case.json; the inline programs' expected output is Tomato's
//! for the same program (okkama 0.5.8, run in the cage), except where the language spec decides against it (named
//! at the test). The file runs twice: as is, and with `SPARROW_CORE_GC_STRESS=1`, which collects at every safepoint.
#![cfg(feature = "champ-abi")]

mod champ_support;

use champ_support::{case_dir, compile, run_case, run_source, run_with_arguments};
use okkama_compiler::SourceFile;

macro_rules! cases {
    ($($name:ident => $id:literal,)*) => {
        $(
            #[test]
            fn $name() {
                run_case($id).unwrap_or_else(|error| panic!("{}: {error}", $id));
            }
        )*
    };
}

cases! {
    n1_clock_monotonic_order => "L10.clock.monotonic-order",
    n1_deadlock_after_timer => "L08.deadlock.after-timer",
}

fn expect(source: &str, stdout: &str, status: i64) {
    let run = run_source(source).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!((run.stdout.as_str(), run.result), (stdout, Ok(status)));
}

/// The sources of a conformance case.
fn sources(id: &str) -> Vec<SourceFile> {
    let file = std::fs::canonicalize(case_dir(id).join("main.ok")).unwrap();
    vec![SourceFile::new(file.to_string_lossy(), std::fs::read_to_string(&file).unwrap()).unwrap()]
}

/// `L08.deadlock.after-timer`'s stderr: a pending timer is progress; once it has fired, main's wait on an event
/// nobody sets is Tomato's deadlock report.
#[test]
fn n1_deadlock_after_timer_reports_no_progress() {
    let run = champ_support::run(compile(&sources("L08.deadlock.after-timer")).unwrap()).unwrap();
    assert_eq!(run.stdout, "alarm: rang\nmain: woke\n");
    let error = run.result.unwrap_err();
    assert!(error.contains("async main cannot complete: no task can make progress"), "{error}");
}

/// Tomato: `sleep` of 0 completes when it starts, so the Future of a zero wait is done as soon as `spawn` returns;
/// one of a millisecond is still waiting on its timer.
#[test]
fn n1_a_zero_sleep_completes_at_once() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "time/clock" take SystemClock;
import "time/duration" take Duration;

async main() is int {
    let zero = spawn SystemClock().wait(Duration(milliseconds: 0));
    Stdout().writeln("zero done at spawn " + zero.done().string());
    let one = spawn SystemClock().wait(Duration(milliseconds: 1));
    Stdout().writeln("one done at spawn " + one.done().string());
    await one;
    Stdout().writeln("one done after await " + one.done().string());
    0;
}
"#, "zero done at spawn true\none done at spawn false\none done after await true\n", 0);
}

/// A duration that is not a canonical non-negative integer is a program failure of the driver (memo §7.1, Tomato's
/// `drivers::native`), which passes every catch (language spec 9.5). Tomato 0.5.8 lets `catch e is Error` take it
/// after an await: that is #797, not the intended behaviour.
#[test]
fn n1_a_non_canonical_duration_is_a_program_failure() {
    let run = run_source(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "runtime/time" take sleep_nanoseconds;

async main() is int {
    Stdout().writeln("before");
    let r = try { await sleep_nanoseconds("01"); 1; } catch e is Error { 2; };
    Stdout().writeln("caught " + r.string());
    0;
}
"#).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(run.stdout, "before\n");
    let error = run.result.unwrap_err();
    assert!(error.contains("Wait duration must be a canonical non-negative integer"), "{error}");
}

/// A native whose permission the embedder refuses fails with a program failure that no catch takes (#719): here the
/// clock group, which `sleep_nanoseconds` needs.
#[test]
fn n1_a_refused_permission_is_a_program_failure() {
    let source = r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "runtime/time" take sleep_nanoseconds;

async main() is int {
    Stdout().writeln("before");
    let r = try { await sleep_nanoseconds("1"); 1; } catch e is Error { 2; };
    Stdout().writeln("caught " + r.string());
    0;
}
"#;
    let name = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/champ_support/inline.ok");
    let source = SourceFile::new(name.to_string_lossy(), source.to_string()).unwrap();
    let limits = sparrow_core::budget::Limits {
        steps: Some(10_000_000),
        natives: Some(vec![]),
        ..sparrow_core::budget::Limits::default()
    };
    let run = sparrow_core::run_captured_with_arguments(compile(&[source]).unwrap(), vec![], limits).unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), "before\n");
    let error = run.result.unwrap_err().0;
    assert!(error.contains("runtime/time@1::sleep_nanoseconds/1 needs the Clock permission"), "{error}");
}

/// A cancellation right after `spawn` reaches a driver that waits on its timer; a Future spawned and left running
/// when main returns is cancelled at shutdown, before the root close, and the run ends normally (Tomato).
#[test]
fn n1_a_pending_sleep_is_cancelled_and_a_detached_one_drained() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "time/clock" take SystemClock;
import "time/duration" take Duration;

async main() is int {
    let waiting = spawn SystemClock().wait(Duration(milliseconds: 0));
    waiting.cancel();
    let r = try { await waiting; "completed"; } catch e is Cancelled { "cancelled"; };
    Stdout().writeln(r + " " + waiting.cancelled().string());
    let long = spawn SystemClock().wait(Duration(seconds: 60));
    long.cancel();
    let s = try { await long; "completed"; } catch e is Cancelled { "cancelled"; };
    Stdout().writeln(s + " " + long.cancelled().string());
    let detached = spawn SystemClock().wait(Duration(seconds: 60));
    Stdout().writeln("leaving");
    0;
}
"#, "completed false\ncancelled true\nleaving\n", 0);
}

/// An external request always suspends its caller once (Tomato `drivers::native_io`): a task that waits on a file
/// watch has not finished when `spawn` returns, so a cancellation right after it is observed.
#[test]
fn n1_a_host_request_suspends_its_caller_once() {
    let directory = std::env::temp_dir().join(format!("okkama-n1-watch-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let name = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/champ_support/inline.ok");
    let source = SourceFile::new(name.to_string_lossy(), r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "fs/watch" take FileWatch;
import "fs/path" take Path;
import "cli/arguments" take ProcessArguments;

object Waiter {
    has watch is FileWatch;
    Waiter(:watch);
    async run() is string {
        await watch.event();
        "event";
    }
}

async main() is int {
    let watch = FileWatch(path: Path(ProcessArguments().strings()[0]));
    let waiting = spawn Waiter(watch: watch).run();
    Stdout().writeln("spawned " + waiting.done().string());
    waiting.cancel();
    let r = try { await waiting; } catch e is Cancelled { "cancelled"; };
    Stdout().writeln(r + " " + waiting.cancelled().string());
    watch.close();
    0;
}
"#.to_string()).unwrap();
    let argument = directory.to_string_lossy().as_bytes().to_vec();
    let run = run_with_arguments(compile(&[source]).unwrap(), vec![argument]).unwrap();
    let _ = std::fs::remove_dir(&directory);
    assert_eq!((run.stdout.as_str(), run.result), ("spawned false\ncancelled true\n", Ok(0)));
}

/// `L10.env.process-snapshot` reads variables its runner sets (`OCS_CASE_VALUE`, `TZ`); in process the same
/// program reads one that cargo sets for every test run.
#[test]
fn n1_env_process_snapshot() {
    let expected = format!("{}\n", env!("CARGO_MANIFEST_DIR"));
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "os/environment" take Environment, MissingEnvironmentVariable;

main() is int {
    let system = Environment();
    Stdout().writeln(system.text("CARGO_MANIFEST_DIR"));
    0;
}
"#, &expected, 0);
}

/// A run that may spawn host processes: in a container or in CI, never on a developer's machine, where the
/// project's rules forbid process spawning outside isolation (docs/LLM-GUIDE.md §1, §4).
fn isolated() -> bool {
    std::path::Path::new("/.dockerenv").exists() || std::env::var_os("CI").is_some()
}

/// `L10.process.capture` spawns `printf` and `false`; its expected output is case.json's.
#[test]
fn n1_process_capture() {
    if !isolated() {
        eprintln!("L10.process.capture spawns host processes: it runs in a container or in CI only");
        return;
    }
    run_case("L10.process.capture").unwrap_or_else(|error| panic!("L10.process.capture: {error}"));
}
