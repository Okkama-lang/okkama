//! Sparrow execution core (rebuild journal: okkama-docs `runtime/new-runtime/core-rebuild.md`).
//!
//! Model, validated by `crates/sparrow-proto` (`docs/new-runtime/core-spike.md`):
//! - synchronous functions are compiled whole and call each other directly on
//!   the native stack; roles dispatch through per-descriptor tables;
//! - async functions are resumable state machines whose heap frame is also
//!   their Future; `spawn` starts eagerly, Tokio drives external waits;
//! - registers carry no tags where the SPB type is static; objects are pointers
//!   to `[header, fields…]` with inline allocation;
//! - a precise collector finds roots through Cranelift stack maps, async
//!   frames and Rust-side visitors, and runs only at safepoints, never inside a
//!   native provider; it does not move objects (decision R1);
//! - no preemption inside synchronous code; under execution budgets
//!   (`budget`) function entries and loop back-edges poll a fuel cell, so
//!   step, memory and output budgets and the host's interrupt flag stop
//!   runaway code (async cancellation still cannot stop a synchronous loop).
//!
//! The `sparrow` binary (`sparrow-cli`) and `okkama run` drive this crate.

// Runtime helpers are `extern "C"` entry points of machine code: they take
// the context as a raw pointer and are sound for the code that calls them.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

pub mod budget;
#[doc(hidden)]
pub mod env_switch;

mod code_memory;
mod contracts;
pub mod drivers;
pub mod failures;
pub mod heap;
pub(crate) mod host;
pub mod io;
pub mod intrinsics;
pub mod jit;
pub mod natives;
pub mod maps;
pub mod ops;
pub mod primitives;
pub mod program;
pub mod runtime;
mod scalar;
mod pointer_free;
pub mod tasks;
pub mod test_report;
pub mod word;
pub mod worker;
mod worker_process;

/// Machine stack of the VM thread (R2): deep recursion is bounded by this
/// reservation; physical memory is committed only as frames are used.
pub const STACK_BYTES: u64 = 1 << 30;

/// Load and run an SPB program's entry on a VM thread.
pub fn run(bytes: Vec<u8>) -> Result<i64, Error> {
    run_with(bytes, Vec::new())
}

/// `run` with the program's command-line arguments.
pub fn run_with(bytes: Vec<u8>, arguments: Vec<Vec<u8>>) -> Result<i64, Error> {
    run_configured(bytes, arguments, false, Vec::new())
}

/// Everything loading checks before a program runs: the SPB verifier and
/// the intrinsic contracts of the core (`sparrow check`).
pub fn check(bytes: &[u8]) -> Result<(), Error> {
    let program = decode_runtime(bytes).map_err(|e| Error(e.to_string()))?;
    match program {
        DecodedProgram::Legacy(program) => contracts::check(&program::Loaded::new(program)),
    }
}

/// `run_with`, optionally printing runtime event counts to stderr at exit
/// (`--stats`; also `SPARROW_CORE_STATS=1`). `worker_command` is the
/// executable and SPB that process workers run (`--worker` goes after the
/// executable); empty, `spawn` of a synchronous action fails.
pub fn run_configured(
    bytes: Vec<u8>,
    arguments: Vec<Vec<u8>>,
    stats: bool,
    worker_command: Vec<std::ffi::OsString>,
) -> Result<i64, Error> {
    run_limited(bytes, arguments, stats, worker_command, budget::Limits::default())
}

/// `run_configured` under execution budgets (`budget::Limits`); a run that
/// exhausts one fails with its report ("Execution budget exhausted: …").
pub fn run_limited(
    bytes: Vec<u8>,
    arguments: Vec<Vec<u8>>,
    stats: bool,
    worker_command: Vec<std::ffi::OsString>,
    limits: budget::Limits,
) -> Result<i64, Error> {
    run_described(bytes, arguments, stats, worker_command, limits)?.map_err(|(error, _)| error)
}

/// `run_limited`, keeping the program's failure apart from a program that
/// does not load: the inner error is the report printed on stderr, with the
/// uncaught failure described (its call stack included).
pub fn run_described(
    bytes: Vec<u8>,
    arguments: Vec<Vec<u8>>,
    stats: bool,
    worker_command: Vec<std::ffi::OsString>,
    limits: budget::Limits,
) -> Result<Result<i64, (Error, Option<Uncaught>)>, Error> {
    on_vm_thread(move || {
        let program = decode_runtime(&bytes).map_err(|e| Error(e.to_string()))?;
        let mut runtime = configured_runtime(program, arguments, stats, worker_command, limits)?;
        Ok(execute(&mut runtime).map_err(|failure| *failure))
    })
}




/// CLI execution result. Loading failures remain the outer `Err`; runtime
/// failures carry the report prefix and an already rendered Sparrow trace
/// followed by cause reports with their own stacks. With tracing disabled,
/// the first field contains the entire plain report and the trace is empty.
#[derive(Debug)]
pub enum Execution {
    Main(Result<i64, (Error, String)>),
    Tests(bool),
}

/// Preserve CLI formatting: SPB/test-profile errors have the launcher prefix;
/// VM setup failures have the runtime report's formatting.
#[derive(Debug)]
pub enum ExecutionError {
    Load(Error),
    Run(Error),
}

/// Launcher policy applied only after SPB decoding and verification.
#[derive(Default)]
pub struct ExecutionOptions {
    pub stats: bool,
    pub trace: bool,
    pub tests: Option<Box<dyn std::io::Write + Send>>,
    /// Optional launcher setup (e.g. an interrupt watchdog). Invalid SPB
    /// must be rejected before this callback has any effects.
    pub on_verified: Option<Box<dyn FnOnce() + Send>>,
}

impl std::fmt::Debug for ExecutionOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecutionOptions").field("stats", &self.stats).field("trace", &self.trace).finish_non_exhaustive()
    }
}

/// Decode and verify once on the VM stack, then execute with trace rendering
/// before the sole program is dropped. `options.tests` selects automatic test-profile
/// dispatch (okkama exec); without it even a test-profile file runs main.
/// No caller-supplied Program can cross this checked byte-input boundary.
pub fn run_executable(
    bytes: Vec<u8>,
    arguments: Vec<Vec<u8>>,
    worker_command: Vec<std::ffi::OsString>,
    limits: budget::Limits,
    options: ExecutionOptions,
) -> Result<Execution, ExecutionError> {
    on_vm_thread(move || {
        let program = match decode_runtime(&bytes) {
            Ok(program) => program,
            Err(error) => return Ok(Err(ExecutionError::Load(Error(error.to_string())))),
        };
        let ExecutionOptions { stats, trace, tests, on_verified } = options;
        if let Some(prepare) = on_verified {
            prepare();
        }
        if program.has_tests() && let Some(out) = tests
        {
            // The budgets bound each test as they bound main (#1017).
            return Ok(test_profile(program, test_report::TestPlan::default(), Some(out), worker_command, &limits).map(|report| Execution::Tests(report.passed())).map_err(ExecutionError::Load));
        }
        let original_functions = program.function_count();
        let mut runtime = configured_runtime(program, arguments, stats, worker_command, limits)?;
        let result = execute(&mut runtime).map_err(|failure| {
            let (error, uncaught) = *failure;
            let Some(mut uncaught) = uncaught.filter(|_| trace) else {
                return (error, String::new());
            };
            // Stops that are not failures (budgets, interrupts) carry no
            // report offsets: their stack, if any, follows the whole message.
            if uncaught.trace_offset == 0 {
                uncaught.trace_offset = error.0.len();
            }
            // Loaded appends worker functions; the launcher only sees SPB functions.
            let report = uncaught.render_report(&error.0, |sites| {
                render_trace_functions(&runtime.loaded.program.functions[..original_functions], sites)
            });
            // Launchers print the prefix with a newline, then this suffix. Split
            // before the first inserted stack (a secondary's cause can precede
            // the root stack) so each cause stays beside its own stack.
            let offset = uncaught.cause_traces.iter().map(|(offset, _)| *offset)
                .fold(uncaught.trace_offset, usize::min);
            let (prefix, suffix) = report.split_at(offset);
            let mut rendered = suffix.strip_prefix('\n').unwrap_or(suffix).to_owned();
            if !rendered.is_empty() {
                rendered.push('\n');
            }
            (Error(prefix.to_owned()), rendered)
        });
        Ok(Ok(Execution::Main(result)))
    }).map_err(ExecutionError::Run)?
}

fn configured_runtime(
    program: DecodedProgram,
    arguments: Vec<Vec<u8>>,
    stats: bool,
    worker_command: Vec<std::ffi::OsString>,
    limits: budget::Limits,
) -> Result<Box<runtime::Runtime>, Error> {
    let mut runtime = match program {
        DecodedProgram::Legacy(program) => Box::new(runtime::Runtime::new(program::Loaded::new(program))?),
    };
    runtime.worker_command = worker_command;
    runtime.limits = limits;
    if stats && runtime.stats.is_none() {
        runtime.stats = Some(Box::default());
        runtime.heap.allocations = Some(Box::default());
    }
    if let Some(host) = &mut runtime.host {
        host.arguments = arguments;
    }
    Ok(runtime)
}

fn execute(runtime: &mut Box<runtime::Runtime>) -> Result<i64, Box<(Error, Option<Uncaught>)>> {
    let result = runtime.run_entry(STACK_BYTES);
    if let Some(report) = runtime.stats_report() {
        // The program's buffered output goes first (#132).
        let _ = okkama_runtime_core::stdout::flush();
        eprint!("{report}");
    }
    result.map_err(|error| Box::new((error, runtime.uncaught.take())))
}

/// An uncaught failure of `main`: its OCS category (`uncaught-error`,
/// `runtime`, `panic`), the error type of a thrown value or `NativeFailure`,
/// its message, and the activations it unwound through, innermost first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Uncaught {
    pub category: &'static str,
    pub error_type: Option<String>,
    pub message: String,
    /// Function and instruction numbers; they are shared with OIR, so a
    /// compiler can map them to source (`okkama-cli/src/trace.rs`).
    pub trace: Vec<Site>,
    /// Activations of the root's unwind past the recorded limit (#1009).
    pub trace_omitted: u64,
    /// Byte offset after the root and its secondaries in the plain report,
    /// before its cause entries.
    pub trace_offset: usize,
    /// Byte offsets after cause entries and their original unwind stacks.
    /// Secondary cleanup failures deliberately have no diagnostic stack.
    pub cause_traces: Vec<(usize, Vec<Site>)>,
}

impl Uncaught {
    /// Add source or SPB stacks to the plain report, keeping cause stacks
    /// indented under their entries. Only called when traces are requested.
    pub fn render_report(&self, report: &str, render: impl Fn(&[Site]) -> String) -> String {
        let mut output = String::new();
        let mut start = 0;
        let mut traces: Vec<_> = self.cause_traces.iter().map(|(offset, trace)| (*offset, trace.as_slice(), "  "))
            .chain(std::iter::once((self.trace_offset, self.trace.as_slice(), "")))
            .collect();
        traces.sort_by_key(|entry| entry.0);
        for (offset, trace, indent) in traces {
            output.push_str(&report[start..offset]);
            let mut stack = render(trace);
            if indent.is_empty() && self.trace_omitted > 0 && !stack.is_empty() {
                stack = format!("{}\n... and {} more activations, not recorded\n", stack.trim_end_matches('\n'), self.trace_omitted);
            }
            if !stack.is_empty() {
                output.push('\n');
                for (index, line) in stack.trim_end_matches('\n').split('\n').enumerate() {
                    if index != 0 {
                        output.push('\n');
                    }
                    output.push_str(indent);
                    output.push_str(line);
                }
            }
            start = offset;
        }
        output.push_str(&report[start..]);
        output
    }
}

pub use okkama_runtime_core::failure::Site;
pub use okkama_runtime_core::stdout;

/// The call stack of an uncaught failure in Sparrow terms: function, number
/// and text of the instruction of each frame. Empty for an empty stack.
pub fn render_trace(program: &okkama_sparrow::Program, trace: &[Site]) -> String {
    render_trace_functions(&program.functions, trace)
}

fn render_trace_functions(functions: &[okkama_sparrow::Function], trace: &[Site]) -> String {
    use okkama_runtime_core::failure::{frame_name, stack_text_by};
    if trace.is_empty() {
        return String::new();
    }
    // Строки только показанных кадров: стек глубокой рекурсии — миллионы (#241).
    stack_text_by("Stack trace (Sparrow, most recent call first):", trace, |site| {
        match functions.get(site.function as usize) {
            Some(function) => {
                let name = frame_name(&function.symbol);
                match function.code.get(site.pc as usize) {
                    Some(instruction) => format!("{name} [f{} @{}: {}]", site.function, site.pc, instruction.text()),
                    None => format!("{name} [f{} @{}]", site.function, site.pc),
                }
            }
            None => format!("? [f{} @{}]", site.function, site.pc),
        }
    })
}

/// `run_configured`, with the uncaught failure (if any) described for
/// protocol adapters; the error text is the report printed on stderr.
pub fn run_reported(
    bytes: Vec<u8>,
    arguments: Vec<Vec<u8>>,
    worker_command: Vec<std::ffi::OsString>,
) -> Result<Result<i64, (Error, Option<Uncaught>)>, Error> {
    run_described(bytes, arguments, false, worker_command, budget::Limits::default())
}

/// Run the module's test profile and write the standard textual report;
/// true when no test failed. Each test runs on a fresh runtime.
pub fn run_tests(bytes: Vec<u8>, out: Box<dyn std::io::Write + Send>) -> Result<bool, Error> {
    run_test_report(bytes, test_report::TestPlan::default(), Some(out)).map(|report| report.passed())
}

/// Runs the tests of `bytes` in declaration order, or in the order `seed`
/// shuffles them to ([`test_report::execution_order`]), and gives the report
/// (#339). With `progress`, the standard human report streams to it as the
/// tests run; without, what the tests write to standard output goes to standard
/// error, so that a machine-readable report owns standard output.
pub fn run_test_report(
    bytes: Vec<u8>,
    plan: test_report::TestPlan,
    progress: Option<Box<dyn std::io::Write + Send>>,
) -> Result<test_report::TestReport, Error> {
    run_test_report_with(bytes, plan, progress, Vec::new())
}

/// [`run_test_report`] for tests that start process workers: `worker_command`
/// is how a child runs this bytecode (empty: workers are refused).
pub fn run_test_report_with(
    bytes: Vec<u8>,
    plan: test_report::TestPlan,
    progress: Option<Box<dyn std::io::Write + Send>>,
    worker_command: Vec<std::ffi::OsString>,
) -> Result<test_report::TestReport, Error> {
    run_test_report_limited(bytes, plan, progress, worker_command, budget::Limits::default())
}

/// [`run_test_report_with`] under execution budgets (#1017): every test runs
/// on a runtime of its own with `limits`, so the step, memory and output
/// budgets bound each test; all of them share the interrupt flag
/// (`--timeout`). A test that exhausts a budget, or is interrupted, fails with
/// the budget's report.
pub fn run_test_report_limited(
    bytes: Vec<u8>,
    plan: test_report::TestPlan,
    progress: Option<Box<dyn std::io::Write + Send>>,
    worker_command: Vec<std::ffi::OsString>,
    limits: budget::Limits,
) -> Result<test_report::TestReport, Error> {
    on_vm_thread(move || {
        let program = decode_runtime(&bytes).map_err(|e| Error(e.to_string()))?;
        match progress {
            Some(out) => test_profile(program, plan, Some(out), worker_command, &limits),
            None => okkama_runtime_core::stdout::to_stderr(|| test_profile(program, plan, None, worker_command, &limits)),
        }
    })
}


// Private: only a decoded, verified program reaches test execution.
fn test_profile(
    program: DecodedProgram,
    plan: test_report::TestPlan,
    out: Option<Box<dyn std::io::Write + Send>>,
    worker_command: Vec<std::ffi::OsString>,
    limits: &budget::Limits,
) -> Result<test_report::TestReport, Error> {
    match program {
        DecodedProgram::Legacy(program) => test_program(program, plan, out, worker_command, limits),
    }
}

fn test_program(
    program: okkama_sparrow::Program,
    plan: test_report::TestPlan,
    out: Option<Box<dyn std::io::Write + Send>>,
    worker_command: Vec<std::ffi::OsString>,
    limits: &budget::Limits,
) -> Result<test_report::TestReport, Error> {
    let tests = program.tests.clone().ok_or_else(|| Error("module has no tests".into()))?;
    if tests.is_empty() {
        return Err(Error("Test run requires at least one test".into()));
    }
    let names = tests.iter().map(|(name, _)| name.clone()).collect::<Vec<_>>();
    report_suite(&names, plan.seed, out, |index| {
        let function = tests[index].1;
        let declaration = &program.functions[function.0 as usize];
        if !declaration.params.is_empty() || declaration.result.is_some() {
            return Ok(CaseOutcome {
                failures: vec!["invalid test entry signature".to_owned()],
                skipped: None,
                cases: Vec::new(),
            });
        }
        let only = plan.only.get(&names[index]).cloned();
        let filtered = only.is_some();
        let mut runtime = Box::new(runtime::Runtime::new(program::Loaded::new(program.clone()))?);
        runtime.worker_command = worker_command.clone();
        runtime.limits = limits.clone();
        if let Some(host) = &mut runtime.host {
            host.test_failures = Some(Vec::new());
            host.test_cases = test_report::SubCaseState::new(only);
        }
        let outcome = runtime.run_function(function.0 as usize, STACK_BYTES);
        // `SPARROW_CORE_STATS` counts each test's runtime as well (#555).
        if let Some(report) = runtime.stats_report() {
            eprint!("{report}");
        }
        let Some(host) = runtime.host.as_mut() else {
            return Ok(CaseOutcome { failures: Vec::new(), skipped: None, cases: Vec::new() });
        };
        let mut failures = host.test_failures.take().unwrap_or_default();
        let mut skipped = host.test_skipped.take();
        // A skipped test ends with the failure of its `test.skip`.
        if let (Err(error), None) = (outcome, &skipped) {
            failures.push(error.0);
        }
        // A test that ended inside a sub-case (#809): the uncaught failure or
        // the skip belongs to that sub-case; the sub-cases after it never ran.
        if host.test_cases.running() {
            failures = host.test_cases.finished(failures, skipped.take());
        }
        let cases = host.test_cases.cases();
        if filtered && cases.is_empty() && failures.is_empty() && skipped.is_none() {
            skipped = Some("no sub-case matches the filter".to_owned());
        }
        Ok(CaseOutcome { failures, skipped, cases })
    })
}

/// How one test ended: its failed checks in order, its skip reason when it
/// skipped, and its sub-cases.
pub(crate) struct CaseOutcome {
    pub failures: Vec<String>,
    pub skipped: Option<String>,
    pub cases: Vec<test_report::SubCase>,
}

/// Runs the tests named `names` in the order `seed` gives and collects the
/// report; `case` runs one test by its declaration index. With `out`, the
/// standard human report streams to it in execution order: `=== SEED n` first
/// when seeded, each test as it runs, then the verdict.
pub(crate) fn report_suite(
    names: &[String],
    seed: Option<u64>,
    mut out: Option<Box<dyn std::io::Write + Send>>,
    mut case: impl FnMut(usize) -> Result<CaseOutcome, Error>,
) -> Result<test_report::TestReport, Error> {
    use test_report::{TestCase, TestOutcome};
    let io = |e: std::io::Error| Error(format!("test report: {e}"));
    let mut cases: Vec<Option<TestCase>> = vec![None; names.len()];
    if let (Some(out), Some(seed)) = (out.as_mut(), seed) {
        writeln!(out, "=== SEED  {seed}").map_err(io)?;
    }
    for (execution, index) in test_report::execution_order(names.len(), seed).into_iter().enumerate() {
        let name = &names[index];
        failures::Resources::trace_test(name);
        if let Some(out) = out.as_mut() {
            writeln!(out, "=== RUN   {name}").map_err(io)?;
            out.flush().map_err(io)?;
        }
        let CaseOutcome { failures, skipped, cases: subcases } = case(index)?;
        let outcome = if !failures.is_empty() {
            TestOutcome::Failed(failures)
        } else if let Some(reason) = skipped {
            TestOutcome::Skipped(reason)
        } else {
            TestOutcome::Passed
        };
        let case = TestCase { name: name.clone(), position: index, execution, outcome, cases: subcases, output: None };
        if let Some(out) = out.as_mut() {
            // Sub-cases first, indented, as `Test/sub-case`; then the test's
            // verdict with its own failures.
            for sub in &case.cases {
                human_outcome(out, &format!("{name}/{}", sub.name), &sub.outcome, "    ").map_err(io)?;
            }
            let own = match (&case.outcome, case.verdict()) {
                (TestOutcome::Failed(_), _) | (_, TestOutcome::Passed) | (_, TestOutcome::Skipped(_)) => {
                    case.outcome.clone()
                }
                (_, verdict) => verdict,
            };
            human_outcome(out, name, &own, "").map_err(io)?;
            out.flush().map_err(io)?;
        }
        cases[index] = Some(case);
    }
    let report = test_report::TestReport { seed, cases: cases.into_iter().flatten().collect() };
    if let Some(out) = out.as_mut() {
        writeln!(out, "{}", if report.passed() { "PASS" } else { "FAIL" }).map_err(io)?;
        out.flush().map_err(io)?;
    }
    Ok(report)
}

/// The human report of a finished run (#938, process workers): each test in
/// declaration order as the streamed report shows it, a failed test's
/// captured output under it, then the verdict.
pub fn human_report(report: &test_report::TestReport) -> String {
    let mut out = std::io::Cursor::new(std::vec::Vec::<u8>::new());
    let _ = human_into(&mut out, report);
    String::from_utf8_lossy(out.get_ref()).into_owned()
}

fn human_into(out: &mut dyn std::io::Write, report: &test_report::TestReport) -> std::io::Result<()> {
    use test_report::TestOutcome;
    if let Some(seed) = report.seed {
        writeln!(out, "=== SEED  {seed}")?;
    }
    for case in &report.cases {
        writeln!(out, "=== RUN   {}", case.name)?;
        for sub in &case.cases {
            human_outcome(out, &format!("{}/{}", case.name, sub.name), &sub.outcome, "    ")?;
        }
        let own = match (&case.outcome, case.verdict()) {
            (own, _) if own.fails() => case.outcome.clone(),
            (_, TestOutcome::Passed) | (_, TestOutcome::Skipped(_)) => case.outcome.clone(),
            (_, verdict) => verdict,
        };
        human_outcome(out, &case.name, &own, "")?;
        if let Some(output) = case.output.as_ref().filter(|_| case.verdict().fails()) {
            for (stream, text, cut) in [
                ("stdout", &output.stdout, output.stdout_truncated),
                ("stderr", &output.stderr, output.stderr_truncated),
            ] {
                if text.is_empty() && !cut {
                    continue;
                }
                writeln!(out, "    {stream}:")?;
                for line in text.lines() {
                    writeln!(out, "        {line}")?;
                }
                if cut {
                    writeln!(out, "        [truncated]")?;
                }
            }
        }
    }
    writeln!(out, "{}", if report.passed() { "PASS" } else { "FAIL" })
}

/// One `--- PASS|FAIL|SKIP` line of the human report, its failures or reason
/// under it, all after `indent`.
fn human_outcome(
    out: &mut dyn std::io::Write,
    name: &str,
    outcome: &test_report::TestOutcome,
    indent: &str,
) -> std::io::Result<()> {
    use test_report::TestOutcome;
    let under = format!("\n{indent}    ");
    match outcome {
        TestOutcome::Failed(failures) => {
            writeln!(out, "{indent}--- FAIL: {name}")?;
            for failure in failures {
                writeln!(out, "{indent}    {}", failure.replace('\n', &under))?;
            }
        }
        TestOutcome::Skipped(reason) => {
            writeln!(out, "{indent}--- SKIP: {name}")?;
            if !reason.is_empty() {
                writeln!(out, "{indent}    {}", reason.replace('\n', &under))?;
            }
        }
        TestOutcome::Passed => writeln!(out, "{indent}--- PASS: {name}")?,
        TestOutcome::TimedOut(limit) => {
            writeln!(out, "{indent}--- FAIL: {name}")?;
            writeln!(out, "{indent}    deadline of {limit} ms passed: the worker was stopped")?;
        }
        TestOutcome::Crashed(how) => {
            writeln!(out, "{indent}--- FAIL: {name}")?;
            writeln!(out, "{indent}    {}", how.replace('\n', &under))?;
        }
    }
    Ok(())
}

/// The child side of a process worker: answer the framed `request` for the
/// program `bytes`; nested spawns run `worker_command`. For the CLIs' worker
/// processes only, not an embedding entry point: a worker runs without
/// budgets or native permissions.
#[doc(hidden)]
pub fn run_worker(bytes: Vec<u8>, request: Vec<u8>, worker_command: Vec<std::ffi::OsString>) -> Result<Vec<u8>, Error> {
    on_vm_thread(move || {
        let program = decode_runtime(&bytes).map_err(|e| Error(e.to_string()))?;
        let mut runtime = Box::new(runtime::Runtime::new(program::Loaded::new(program.legacy()?))?);
        runtime.worker_command = worker_command;
        Ok(worker::respond(&mut runtime, &request, STACK_BYTES))
    })
}

/// Internal CLI worker transport. One-shot spawn keeps its EOF protocol;
/// run-local pools use a duplex control socket, separate from program output.
#[doc(hidden)]
pub fn serve_worker(bytes: Vec<u8>, worker_command: Vec<std::ffi::OsString>) -> Result<(), Error> {
    use std::io::{Read, Write};
    #[cfg(unix)]
    if crate::env_switch::enabled("SPARROW_INTERNAL_WORKER_POOL") {
        use std::os::fd::AsFd;
        let fd = std::io::stdin()
            .as_fd()
            .try_clone_to_owned()
            .map_err(|e| Error(e.to_string()))?;
        let mut control = std::fs::File::from(fd);
        // Language stdin is EOF, as with an ordinary spawn after its request.
        rustix::stdio::dup2_stdin(std::fs::File::open("/dev/null").map_err(|e| Error(e.to_string()))?)
            .map_err(|e| Error(e.to_string()))?;
        loop {
            let mut length = [0; 8];
            match control.read(&mut length[..1]) {
                Ok(0) => return Ok(()),
                Ok(_) => {}
                Err(e) => return Err(Error(e.to_string())),
            }
            control.read_exact(&mut length[1..]).map_err(|e| Error(e.to_string()))?;
            let length = usize::try_from(u64::from_be_bytes(length)).map_err(|e| Error(e.to_string()))?;
            let mut request = Vec::new();
            request.try_reserve_exact(length).map_err(|e| Error(e.to_string()))?;
            request.resize(length, 0);
            control.read_exact(&mut request).map_err(|e| Error(e.to_string()))?;
            // Fresh VM, heap, globals, receiver and host session for EVERY item.
            let response = run_worker(bytes.clone(), request, worker_command.clone())?;
            stdout::flush().map_err(|e| Error(e.to_string()))?;
            std::io::stdout().flush().map_err(|e| Error(e.to_string()))?;
            std::io::stderr().flush().map_err(|e| Error(e.to_string()))?;
            control
                .write_all(&(response.len() as u64).to_be_bytes())
                .and_then(|()| control.write_all(&response))
                .and_then(|()| control.flush())
                .map_err(|e| Error(e.to_string()))?;
        }
    }
    let mut request = Vec::new();
    std::io::stdin()
        .read_to_end(&mut request)
        .map_err(|e| Error(format!("worker request: {e}")))?;
    let response = run_worker(bytes, request, worker_command)?;
    stdout::flush().map_err(|e| Error(e.to_string()))?;
    let mut out = std::io::stdout().lock();
    out.write_all(&response)
        .and_then(|()| out.flush())
        .map_err(|e| Error(format!("worker response: {e}")))
}
fn on_vm_thread<T: Send + 'static>(body: impl FnOnce() -> Result<T, Error> + Send + 'static) -> Result<T, Error> {
    std::thread::Builder::new()
        .name("sparrow-core".into())
        .stack_size(STACK_BYTES as usize)
        .spawn(move || {
            // The program's cwd and environment live as long as this run (#40).
            let _host = okkama_runtime_core::host::enter(
                okkama_runtime_core::host::Context::process().without("SPARROW_INTERNAL_WORKER").without("SPARROW_INTERNAL_WORKER_POOL"),
            );
            body()
        })
        .map_err(|e| Error(format!("VM thread: {e}")))?
        .join()
        .map_err(|payload| Error(format!("internal error: {}", runtime::panic_text(&*payload))))?
}

/// A variable the launcher adds to the program's environment when the
/// process lacks it (`okkama run` passes the pinned ICU this way).
pub fn default_environment(name: &str, value: &str) {
    okkama_runtime_core::host::default_environment(name, value);
}

/// Natives and whether this core provides them (`--native-capabilities`).
pub fn native_capabilities() -> impl Iterator<Item = (&'static str, bool)> {
    okkama_runtime_abi::Native::ALL.iter().map(|&n| (n.symbol(), natives::supported(n)))
}

/// Engine-level failure: an invalid program or an internal invariant, never a
/// language-level error (those are values unwound by the program).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// Select a reader by the explicit version header; neither reader retries the other ABI.
#[derive(Debug)]
enum DecodedProgram {
    Legacy(okkama_sparrow::Program),
}

impl DecodedProgram {
    fn legacy(self) -> Result<okkama_sparrow::Program, Error> {
        match self {
            Self::Legacy(program) => Ok(program),
        }
    }

    fn has_tests(&self) -> bool {
        match self {
            Self::Legacy(program) => program.tests.is_some(),
        }
    }

    fn function_count(&self) -> usize {
        match self {
            Self::Legacy(program) => program.functions.len(),
        }
    }
}

fn decode_runtime(bytes: &[u8]) -> Result<DecodedProgram, okkama_sparrow::CodecError> {
    okkama_sparrow::decode(bytes).map(DecodedProgram::Legacy)
}

#[cfg(test)]
mod session_tests;

#[cfg(test)]
mod uncaught_report_tests {
    use super::{Site, Uncaught};

    /// #1009: a trace cut at the recording limit says how many activations
    /// it left out, under the root's stack only.
    #[test]
    fn an_omitted_tail_of_the_trace_is_reported() {
        let uncaught = Uncaught {
            category: "runtime",
            error_type: None,
            message: "Stack overflow".into(),
            trace: vec![Site { function: 0, pc: 1 }],
            trace_omitted: 7,
            trace_offset: 14,
            cause_traces: Vec::new(),
        };
        let report = uncaught.render_report("Stack overflow", |_| "  at f\n".into());
        assert_eq!(report, "Stack overflow\n  at f\n... and 7 more activations, not recorded");
        let whole = Uncaught { trace_omitted: 0, ..uncaught };
        assert_eq!(whole.render_report("Stack overflow", |_| "  at f\n".into()), "Stack overflow\n  at f");
    }
}
