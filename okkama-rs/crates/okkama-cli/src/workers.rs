//! `okkama run -t --workers N` (#938): every selected test in a process of its
//! own, at most N at a time, with its standard output and error captured up to
//! a limit and, with `--test-timeout`, a deadline on the monotonic clock.
//!
//! A worker is this executable run again as `okkama run -t --worker-test NAME
//! --worker-result FILE ...`: it compiles the module, runs the one test whose
//! name is exactly NAME and writes the JSON report of that test (#339) to FILE.
//! What the test prints goes to the worker's own stdout and stderr, which the
//! parent reads on two threads each into a bounded buffer, draining the rest so
//! a flooding test cannot block. A worker that passes its deadline is killed
//! (SIGKILL) and reaped; one that ends without writing its report is a crash.
//! Results are kept by declaration position, so the report's order does not
//! depend on which worker finished first. Workers stay in the parent's process
//! group, so a Ctrl-C at the terminal reaches them too; processes a test starts
//! itself are not stopped with a timed-out worker.
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread::{sleep, spawn};
use std::time::{Duration, Instant};

use sparrow_core::test_report::{CapturedOutput, SubCase, TestCase, TestOutcome, TestReport, execution_order};

/// The most bytes kept of each captured stream; the rest is read and dropped.
pub const CAPTURE_LIMIT: usize = 64 * 1024;

/// The most workers `--workers` accepts.
pub const MAX_WORKERS: usize = 256;

/// The longest `--test-timeout`, in seconds.
pub const MAX_TEST_TIMEOUT: f64 = 86_400.0;

/// How a run of process workers is made.
pub struct Pool {
    /// The executable a worker runs (this one).
    pub exe: PathBuf,
    /// Arguments every worker gets after `run -t`: the filters, then the files.
    pub filters: Vec<String>,
    /// The step, memory and output budgets, as options: each bounds every
    /// test, in its worker as in one process (#1017).
    pub budgets: Vec<String>,
    pub inputs: Vec<String>,
    /// How many run at once, from 1.
    pub workers: usize,
    /// The deadline of each test, if any.
    pub deadline: Option<Duration>,
}

/// One stream being read: the bytes kept, whether more came, and whether
/// the reader reached the end.
type Capture = std::sync::Arc<std::sync::Mutex<(Vec<u8>, bool, bool)>>;

/// How long the parent keeps reading a finished worker's streams: a process
/// the test started may hold them open after the worker is gone (#940).
const DRAIN_GRACE: Duration = Duration::from_millis(500);

struct Running {
    index: usize,
    execution: usize,
    child: Child,
    started: Instant,
    result: PathBuf,
    stdout: Option<Capture>,
    stderr: Option<Capture>,
}

/// `--workers`' value: a whole number from 1 to [`MAX_WORKERS`].
pub fn worker_count(text: &str) -> Result<usize, String> {
    text.parse::<usize>()
        .ok()
        .filter(|count| (1..=MAX_WORKERS).contains(count))
        .ok_or_else(|| format!("--workers needs a whole number from 1 to {MAX_WORKERS}, not `{text}`"))
}

/// `--test-timeout`'s value: seconds, more than 0 and at most a day; a
/// fraction is allowed (`0.5`), counted in whole milliseconds, at least one.
pub fn test_timeout(text: &str) -> Result<Duration, String> {
    let seconds = text.parse::<f64>().ok().filter(|s| s.is_finite() && *s > 0.0 && *s <= MAX_TEST_TIMEOUT);
    match seconds {
        Some(seconds) => Ok(Duration::from_millis(((seconds * 1000.0).round() as u64).max(1))),
        None => Err(format!(
            "--test-timeout needs seconds above 0 and at most {MAX_TEST_TIMEOUT}, not `{text}`"
        )),
    }
}

/// Runs the tests `names` (declaration order) in workers, in the order `seed`
/// gives, and collects the report in declaration order.
pub fn run(pool: &Pool, names: &[String], seed: Option<u64>) -> TestReport {
    let scratch = match scratch_dir() {
        Ok(scratch) => scratch,
        Err(error) => {
            // No worker could report: each test says why (#1017: the error was
            // swallowed and every worker "ended before it reported").
            let how = format!("the workers' directory could not be made: {error}");
            let cases = execution_order(names.len(), seed)
                .into_iter()
                .enumerate()
                .map(|(execution, index)| ended(&names[index], index, execution, TestOutcome::Crashed(how.clone()), None));
            let mut cases: Vec<TestCase> = cases.collect();
            cases.sort_by_key(|case| case.position);
            return TestReport { seed, cases };
        }
    };
    let mut cases: Vec<Option<TestCase>> = vec![None; names.len()];
    let mut queue = execution_order(names.len(), seed).into_iter().enumerate();
    let mut running: Vec<Running> = Vec::new();
    loop {
        while running.len() < pool.workers {
            let Some((execution, index)) = queue.next() else { break };
            match start(pool, &scratch, &names[index], index, execution) {
                Ok(worker) => running.push(worker),
                Err(error) => {
                    let how = format!("the worker could not start: {error}");
                    cases[index] = Some(ended(&names[index], index, execution, TestOutcome::Crashed(how), None));
                }
            }
        }
        if running.is_empty() {
            break;
        }
        let mut at = 0;
        while at < running.len() {
            let worker = &mut running[at];
            let state = worker.child.try_wait();
            let late = pool.deadline.is_some_and(|limit| worker.started.elapsed() >= limit);
            match state {
                Ok(None) if !late => {
                    at += 1;
                    continue;
                }
                _ => {}
            }
            let mut worker = running.swap_remove(at);
            let status = match state {
                Ok(Some(status)) => Ok(status),
                Ok(None) => {
                    let _ = worker.child.kill();
                    Err(worker.child.wait())
                }
                Err(error) => {
                    let _ = worker.child.kill();
                    let _ = worker.child.wait();
                    Err(Err(error))
                }
            };
            let output = captured(&mut worker);
            let name = &names[worker.index];
            let case = match status {
                Ok(status) => reported(&worker.result, name, worker.index, worker.execution, status, output),
                Err(Ok(_)) => {
                    let limit = pool.deadline.map_or(0, |d| d.as_millis() as u64);
                    ended(name, worker.index, worker.execution, TestOutcome::TimedOut(limit), Some(output))
                }
                Err(Err(error)) => {
                    let how = format!("the worker could not be watched: {error}");
                    ended(name, worker.index, worker.execution, TestOutcome::Crashed(how), Some(output))
                }
            };
            let _ = std::fs::remove_file(&worker.result);
            cases[worker.index] = Some(case);
        }
        sleep(Duration::from_millis(5));
    }
    let _ = std::fs::remove_dir_all(&scratch);
    TestReport { seed, cases: cases.into_iter().flatten().collect() }
}

/// A private directory for the workers' result files: made new (never an
/// existing one or a link planted under its name), readable only by this
/// user, as the doctest directory is.
fn scratch_dir() -> std::io::Result<PathBuf> {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("okkama-workers-{}-{nanos}", std::process::id()));
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&dir)?;
    Ok(dir)
}

fn start(pool: &Pool, scratch: &Path, name: &str, index: usize, execution: usize) -> std::io::Result<Running> {
    let result = scratch.join(format!("{index}.json"));
    let mut command = Command::new(&pool.exe);
    command.args(["run", "-t", "--worker-test", name, "--worker-result"]).arg(&result);
    for filter in &pool.filters {
        command.args(["--filter", filter]);
    }
    command.args(&pool.budgets).args(&pool.inputs).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = command.spawn()?;
    Ok(running(child, index, execution, result))
}

/// A started worker whose streams are read in the background.
fn running(mut child: Child, index: usize, execution: usize, result: PathBuf) -> Running {
    let reader = |pipe: Box<dyn Read + Send>| {
        let capture: Capture = std::sync::Arc::new(std::sync::Mutex::new((Vec::new(), false, false)));
        let shared = capture.clone();
        spawn(move || drain_into(pipe, &shared));
        capture
    };
    let stdout = child.stdout.take().map(|pipe| reader(Box::new(pipe)));
    let stderr = child.stderr.take().map(|pipe| reader(Box::new(pipe)));
    Running { index, execution, child, started: Instant::now(), result, stdout, stderr }
}

/// [`drain`] into a shared capture, marking it ended at the end of the pipe.
fn drain_into(mut pipe: impl Read, capture: &Capture) {
    let mut chunk = [0u8; 8192];
    loop {
        let read = match pipe.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        let mut state = capture.lock().expect("capture");
        let room = CAPTURE_LIMIT - state.0.len();
        state.0.extend_from_slice(&chunk[..read.min(room)]);
        state.1 |= read > room;
    }
    capture.lock().expect("capture").2 = true;
}

/// [`drain_into`] a fresh capture: what was kept and whether more came. The
/// test checks the working path, not a copy of it (#1017).
#[cfg(test)]
fn drain(pipe: impl Read) -> (Vec<u8>, bool) {
    let capture: Capture = std::sync::Arc::new(std::sync::Mutex::new((Vec::new(), false, false)));
    drain_into(pipe, &capture);
    let state = capture.lock().expect("capture");
    assert!(state.2, "the capture reached the end");
    (state.0.clone(), state.1)
}

/// What the worker's streams held. A reader that has not reached the end
/// within [`DRAIN_GRACE`] (a process the test started still holds the pipe)
/// is left to finish on its own, and that stream is marked cut (#940).
fn captured(worker: &mut Running) -> CapturedOutput {
    let deadline = Instant::now() + DRAIN_GRACE;
    let take = |capture: Option<Capture>| {
        let Some(capture) = capture else {
            return (String::new(), false);
        };
        loop {
            let state = capture.lock().expect("capture");
            if state.2 || Instant::now() >= deadline {
                return (String::from_utf8_lossy(&state.0).into_owned(), state.1 || !state.2);
            }
            drop(state);
            sleep(Duration::from_millis(5));
        }
    };
    let (stdout, stdout_truncated) = take(worker.stdout.take());
    let (stderr, stderr_truncated) = take(worker.stderr.take());
    CapturedOutput { stdout, stderr, stdout_truncated, stderr_truncated }
}

fn ended(name: &str, index: usize, execution: usize, outcome: TestOutcome, output: Option<CapturedOutput>) -> TestCase {
    TestCase { name: name.to_owned(), position: index, execution, outcome, cases: Vec::new(), output }
}

/// What a worker writes for its one test (#938, #940): the test's own outcome
/// apart from its sub-cases', so a parent's own skip or failure survives next
/// to failing sub-cases.
pub fn record(report: &TestReport) -> String {
    let outcome = |outcome: &TestOutcome| match outcome {
        TestOutcome::Passed => serde_json::json!({"outcome": "passed"}),
        TestOutcome::Failed(failures) => serde_json::json!({"outcome": "failed", "failures": failures}),
        TestOutcome::Skipped(reason) => serde_json::json!({"outcome": "skipped", "reason": reason}),
        TestOutcome::TimedOut(limit) => serde_json::json!({"outcome": "timed_out", "limit": limit}),
        TestOutcome::Crashed(how) => serde_json::json!({"outcome": "crashed", "reason": how}),
    };
    let tests: Vec<serde_json::Value> = report
        .cases
        .iter()
        .map(|case| {
            serde_json::json!({
                "name": case.name,
                "own": outcome(&case.outcome),
                "cases": case.cases.iter().map(|sub| {
                    let mut entry = outcome(&sub.outcome);
                    entry["name"] = serde_json::Value::String(sub.name.clone());
                    entry
                }).collect::<Vec<_>>(),
            })
        })
        .collect();
    serde_json::json!({"worker": 1, "tests": tests}).to_string()
}

/// The test as its worker recorded it, or a crash when the worker wrote no
/// record.
fn reported(
    result: &Path,
    name: &str,
    index: usize,
    execution: usize,
    status: ExitStatus,
    output: CapturedOutput,
) -> TestCase {
    let parsed = std::fs::read_to_string(result).ok().and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
    let test = parsed.as_ref().and_then(|document| document["tests"].get(0)).filter(|test| test["name"] == name);
    let Some(test) = test else {
        let how = format!("the worker ended ({}) before it reported", describe(status));
        return ended(name, index, execution, TestOutcome::Crashed(how), Some(output));
    };
    let cases: Vec<SubCase> = test["cases"]
        .as_array()
        .map(|cases| {
            cases
                .iter()
                .map(|case| SubCase { name: case["name"].as_str().unwrap_or("").to_owned(), outcome: outcome(case) })
                .collect()
        })
        .unwrap_or_default();
    TestCase { name: name.to_owned(), position: index, execution, outcome: outcome(&test["own"]), cases, output: Some(output) }
}

/// An outcome as a worker record writes it.
fn outcome(entry: &serde_json::Value) -> TestOutcome {
    let texts = |key: &str| -> Vec<String> {
        entry[key].as_array().map_or_else(Vec::new, |items| {
            items.iter().map(|item| item.as_str().unwrap_or("").to_owned()).collect()
        })
    };
    match entry["outcome"].as_str() {
        Some("passed") => TestOutcome::Passed,
        Some("skipped") => TestOutcome::Skipped(entry["reason"].as_str().unwrap_or("").to_owned()),
        Some("failed") => TestOutcome::Failed(texts("failures")),
        Some("timed_out") => TestOutcome::TimedOut(entry["limit"].as_u64().unwrap_or(0)),
        Some("crashed") => TestOutcome::Crashed(entry["reason"].as_str().unwrap_or("").to_owned()),
        other => TestOutcome::Crashed(format!("the worker reported an unknown outcome {other:?}")),
    }
}

fn describe(status: ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return format!("killed by signal {signal}");
        }
    }
    status.code().map_or_else(|| "without a status".to_owned(), |code| format!("exit status {code}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_counts_and_timeouts_are_checked() {
        assert_eq!(worker_count("1"), Ok(1));
        assert_eq!(worker_count("256"), Ok(256));
        for wrong in ["0", "-1", "257", "x", ""] {
            assert!(worker_count(wrong).is_err(), "{wrong}");
        }
        assert_eq!(test_timeout("0.5"), Ok(Duration::from_millis(500)));
        assert_eq!(test_timeout("0.0001"), Ok(Duration::from_millis(1)));
        assert_eq!(test_timeout("86400"), Ok(Duration::from_secs(86_400)));
        for wrong in ["0", "-2", "86401", "inf", "NaN", "x"] {
            assert!(test_timeout(wrong).is_err(), "{wrong}");
        }
    }

    /// #940: a process the test started holds the worker's stdout open after
    /// the worker ends; the parent stops waiting after the grace and marks the
    /// stream cut, while a stream that ended is kept whole.
    #[test]
    fn a_stream_held_by_a_lingering_process_is_cut_after_the_grace() {
        let child = Command::new("sh")
            .args(["-c", "echo early; echo note >&2; exec 2>&-; sleep 5 &"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("sh");
        let mut worker = running(child, 0, 0, PathBuf::new());
        assert!(worker.child.wait().expect("wait").success());
        let started = Instant::now();
        let output = captured(&mut worker);
        let waited = started.elapsed();
        assert!(waited >= DRAIN_GRACE && waited < DRAIN_GRACE * 4, "waited {waited:?}");
        assert_eq!(output.stdout, "early\n");
        assert!(output.stdout_truncated);
        assert_eq!(output.stderr, "note\n");
        assert!(!output.stderr_truncated);
    }

    /// #940: a parent's own skip next to a failing sub-case survives the
    /// worker record.
    #[test]
    fn a_record_keeps_the_parent_outcome_apart_from_its_sub_cases() {
        let case = TestCase {
            name: "table".into(),
            position: 0,
            execution: 0,
            outcome: TestOutcome::Skipped("no more cases".into()),
            cases: vec![SubCase { name: "first".into(), outcome: TestOutcome::Failed(vec!["expected 1, got 2".into()]) }],
            output: None,
        };
        let text = record(&TestReport { seed: None, cases: vec![case.clone()] });
        let dir = std::env::temp_dir().join(format!("okkama-worker-record-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("0.json");
        std::fs::write(&path, text).unwrap();
        let back = reported(&path, "table", 0, 0, success_status(), CapturedOutput::default());
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(back.outcome, case.outcome);
        assert_eq!(back.cases, case.cases);
        let report = TestReport { seed: None, cases: vec![back] };
        let totals = report.totals();
        assert_eq!((totals.tests, totals.failed, totals.skipped), (2, 1, 1));
    }

    fn success_status() -> ExitStatus {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            ExitStatus::from_raw(0)
        }
    }

    /// #1017: the workers' directory is new and private to this user.
    #[test]
    #[cfg(unix)]
    fn the_scratch_directory_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch_dir().expect("a directory");
        let mode = std::fs::metadata(&dir).expect("metadata").permissions().mode();
        let _ = std::fs::remove_dir(&dir);
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn a_flood_is_kept_up_to_the_limit_and_marked() {
        let flood = vec![b'x'; CAPTURE_LIMIT * 3 + 7];
        let (kept, cut) = drain(flood.as_slice());
        assert_eq!(kept.len(), CAPTURE_LIMIT);
        assert!(cut);
        let (kept, cut) = drain(&b"short"[..]);
        assert_eq!((kept.as_slice(), cut), (&b"short"[..], false));
    }
}
