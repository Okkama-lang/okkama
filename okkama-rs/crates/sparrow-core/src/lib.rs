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

// #1124: the features `jit` and `host` are native only; without both Sparrow builds for wasm32-unknown-unknown.
#[cfg(all(target_family = "wasm", feature = "jit"))]
compile_error!("sparrow-core's `jit` feature generates native code: build for wasm32 with `--no-default-features`");
#[cfg(all(target_family = "wasm", feature = "host"))]
compile_error!("sparrow-core's `host` feature needs an operating system: build for wasm32 with `--no-default-features`");

pub mod budget;
// The one switch policy of every crate (#1095), under its old path.
#[doc(hidden)]
pub use okkama_devtrace::env_switch;

pub mod clock;
/// The functions a run can reach (#1135).
pub mod reach;
#[cfg(feature = "jit")]
mod code_memory;
mod contracts;
pub mod drivers;
pub mod failures;
pub mod heap;
pub(crate) mod host;
pub mod io;
pub mod intrinsics;
mod interp;
#[cfg(feature = "jit")]
pub mod jit;
pub mod natives;
pub mod maps;
pub mod ops;
pub mod primitives;
pub mod program;
pub mod runtime;
mod scalar;
mod texts;
mod pointer_free;
pub mod tasks;
pub mod test_report;
pub mod word;
#[cfg(feature = "host")]
pub mod worker;
// #1127: without the host (wasm32, the playground) workers are emulated in the same instance.
#[cfg(not(feature = "host"))]
#[path = "worker_emulated.rs"]
pub mod worker;
/// The values that cross into a worker, process or emulated.
mod worker_wire;
// #1127: with the host, the emulation serves an embedding that asks for it (`Runtime::emulated_workers`).
#[cfg(feature = "host")]
mod worker_emulated;
#[cfg(feature = "host")]
mod worker_process;

/// Machine stack of the VM thread (R2): deep recursion is bounded by this
/// reservation; physical memory is committed only as frames are used. 1 GiB on
/// 64-bit targets; a 32-bit address space (wasm32, i686: #1131) holds a few
/// runs' reservations of 128 MiB, where 1 GiB each fails the second thread.
pub const STACK_BYTES: u64 = if cfg!(target_pointer_width = "64") { 1 << 30 } else { 1 << 27 };

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




/// The engine of a run, whatever `SPARROW_CORE_INTERPRET` says (#1121: the differential test of the two).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// The JIT, in a build with it (#1124: without it the interpreter is the only engine).
    #[cfg(feature = "jit")]
    Jit,
    Interpreter,
}

/// What a run shows, to compare Sparrow's engines (#1121): `main`'s result, or the report and the uncaught failure
/// (category, type, message, sites, causes), and what the program wrote to each stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observed {
    pub result: Result<i64, (String, Option<Uncaught>)>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// `run_described` on `engine`, with both streams captured (#1121).
pub fn observe(engine: Engine, bytes: Vec<u8>, arguments: Vec<Vec<u8>>, limits: budget::Limits) -> Result<Observed, Error> {
    on_vm_thread(move || {
        let program = decode_runtime(&bytes).map_err(|e| Error(e.to_string()))?;
        let mut runtime = configured_runtime(program, arguments, false, Vec::new(), limits)?;
        runtime.use_engine(engine);
        let (result, stdout, stderr) = okkama_runtime_core::stdout::capture_streams(|| execute(&mut runtime));
        let result = result.map_err(|failure| {
            let (error, uncaught) = *failure;
            (error.0, uncaught)
        });
        Ok(Observed { result, stdout, stderr })
    })
}

/// What an embedding gives a run (#1125: the playground, and the native run that checks it).
#[derive(Default)]
pub struct Embedding {
    pub arguments: Vec<Vec<u8>>,
    /// Standard input: these bytes, then end of input; `None` reads the process's.
    pub stdin: Option<Vec<u8>>,
    pub limits: budget::Limits,
    /// Where each write goes as it happens (#1134: the playground streams output, so what a stopped run printed
    /// stays); `None` collects them into [`Embedded::output`].
    pub pieces: Option<PieceSink>,
    /// How [`Embedding::pieces`] gets the writes (#1136).
    pub batching: Batching,
}

/// How an embedded run hands its writes to [`Embedding::pieces`] (#1136).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Batching {
    /// Each write as it happens (#1134).
    #[default]
    Write,
    /// A stream's consecutive writes joined while they fall within this many nanoseconds of the first (and within
    /// [`BATCH_BYTES`]); the piece goes when the next write does not join it, at the run's next budget poll (every
    /// [`budget::SLICE`] steps at most), and at the end. A program that writes and then computes hands its writes
    /// over within a slice; one that sleeps (the virtual timeline's skip) starts a new piece with its own moment.
    Window(u64),
}

/// The most bytes [`Batching::Window`] joins into one piece.
pub const BATCH_BYTES: usize = 64 << 10;

/// The receiver of an embedded run's writes, one [`Piece`] per write, on the VM thread.
pub type PieceSink = Box<dyn FnMut(Piece) + Send>;

impl std::fmt::Debug for Embedding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Embedding")
            .field("arguments", &self.arguments.len())
            .field("stdin", &self.stdin.as_ref().map(Vec::len))
            .field("limits", &self.limits)
            .field("pieces", &self.pieces.is_some())
            .field("batching", &self.batching)
            .finish()
    }
}

/// One write of a run to a stream, `nanos` after the run started on Sparrow's clock (`clock`; on wasm32 the
/// virtual timeline, so a sleep shows as time between pieces without being waited for).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    pub stream: stdout::Stream,
    pub bytes: Vec<u8>,
    pub nanos: u64,
}

/// What an embedded run shows: `main`'s result, or the report and the uncaught failure, and the program's writes
/// in order (consecutive writes to one stream at one moment are one piece; empty when [`Embedding::pieces`] took
/// them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Embedded {
    pub result: Result<i64, (String, Option<Uncaught>)>,
    pub output: Vec<Piece>,
}

/// Run on the interpreter, as the playground does (#1125), under `embedding`'s limits, with its standard input,
/// every write marked with its moment.
pub fn run_embedded(bytes: Vec<u8>, embedding: Embedding) -> Result<Embedded, Error> {
    on_vm_thread(move || {
        let program = decode_runtime(&bytes).map_err(|e| Error(e.to_string()))?;
        let Embedding { arguments, stdin, limits, pieces: streamed, batching } = embedding;
        let mut runtime = configured_runtime(program, arguments, false, Vec::new(), limits)?;
        runtime.use_engine(Engine::Interpreter);
        // The playground's workers, natively too (#1127).
        runtime.emulated_workers = true;
        if let (Some(bytes), Some(host)) = (stdin, runtime.host.as_mut()) {
            host.set_stdin(bytes);
        }
        let (result, output) = Receiver::receive(&Receiver::new(streamed, batching), || execute(&mut runtime));
        let result = result.map_err(|failure| {
            let (error, uncaught) = *failure;
            (error.0, uncaught)
        });
        Ok(Embedded { result, output })
    })
}

/// The receiver of an embedded run's writes: each to `streamed` (by `batching`), or collected (a stream's writes at
/// one moment joined). Moments are nanoseconds since it was made on Sparrow's clock.
struct Receiver {
    collected: Vec<Piece>,
    pending: Option<Piece>,
    streamed: Option<PieceSink>,
    batching: Batching,
    start: clock::Instant,
}

type Shared = std::rc::Rc<std::cell::RefCell<Receiver>>;

impl Receiver {
    fn new(streamed: Option<PieceSink>, batching: Batching) -> Shared {
        let start = clock::Instant::now();
        std::rc::Rc::new(std::cell::RefCell::new(Self { collected: Vec::new(), pending: None, streamed, batching, start }))
    }

    fn write(&mut self, stream: stdout::Stream, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let nanos = u64::try_from(self.start.elapsed().as_nanos()).unwrap_or(u64::MAX);
        let Some(streamed) = self.streamed.as_mut() else {
            match self.collected.last_mut() {
                Some(last) if last.stream == stream && last.nanos == nanos => last.bytes.extend_from_slice(bytes),
                _ => self.collected.push(Piece { stream, bytes: bytes.to_vec(), nanos }),
            }
            return;
        };
        let window = match self.batching {
            // A window of 0 joins nothing, so it holds nothing back either.
            Batching::Write | Batching::Window(0) => return streamed(Piece { stream, bytes: bytes.to_vec(), nanos }),
            Batching::Window(window) => window,
        };
        match self.pending.as_mut() {
            Some(pending)
                if pending.stream == stream
                    && nanos.saturating_sub(pending.nanos) < window
                    && pending.bytes.len() + bytes.len() <= BATCH_BYTES =>
            {
                pending.bytes.extend_from_slice(bytes);
            }
            _ => {
                if let Some(pending) = self.pending.replace(Piece { stream, bytes: bytes.to_vec(), nanos }) {
                    streamed(pending);
                }
            }
        }
    }

    /// Hand over the piece being joined (a budget poll, the end of the run).
    fn tick(&mut self) {
        if let (Some(pending), Some(streamed)) = (self.pending.take(), self.streamed.as_mut()) {
            streamed(pending);
        }
    }

    /// Run `body` with the writes of this VM thread going here, ticked at each budget poll; then hand over what is
    /// left and return what was collected.
    fn receive<T>(shared: &Shared, body: impl FnOnce() -> T) -> (T, Vec<Piece>) {
        let (writes, ticks) = (shared.clone(), shared.clone());
        let sink: stdout::Sink = Box::new(move |stream, bytes| writes.borrow_mut().write(stream, bytes));
        let tick = Box::new(move || ticks.borrow_mut().tick());
        let result = stdout::with_sink(sink, || stdout::with_tick(tick, body));
        let mut receiver = shared.borrow_mut();
        receiver.tick();
        (result, std::mem::take(&mut receiver.collected))
    }
}

/// What an embedded test run shows (#1127): whether every test passed, and the human report (`okkama run -t`'s) with
/// the tests' own writes, in order (empty when [`Embedding::pieces`] took them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddedTests {
    pub passed: bool,
    pub output: Vec<Piece>,
}

/// The test profile of an SPB built with tests, as `okkama run -t` runs it, but on the interpreter and as the
/// playground does (#1127): each test on a runtime of its own under `embedding`'s limits, with its standard input;
/// the report streams through the same receiver as the tests' output. `Err` for a program that does not load or has
/// no tests (the CLI's `okkama: …`, exit 4).
pub fn run_tests_embedded(bytes: Vec<u8>, embedding: Embedding, plan: test_report::TestPlan) -> Result<EmbeddedTests, Error> {
    on_vm_thread(move || {
        let program = decode_runtime(&bytes).map_err(|e| Error(e.to_string()))?;
        let Embedding { arguments: _, stdin, limits, pieces: streamed, batching } = embedding;
        let setup = TestSetup { engine: Some(Engine::Interpreter), stdin, emulated_workers: true };
        let (report, output) = Receiver::receive(&Receiver::new(streamed, batching), || {
            // A line at a time: the report is written in fragments, and each write is a piece the page receives.
            test_profile(program, plan, Some(Box::new(std::io::LineWriter::new(stdout::Buffered))), Vec::new(), &limits, &setup)
        });
        Ok(EmbeddedTests { passed: report?.passed(), output })
    })
}

/// How each test's runtime is set up beyond its budgets (#1127: the playground's engine and standard input).
#[derive(Debug, Default)]
struct TestSetup {
    engine: Option<Engine>,
    stdin: Option<Vec<u8>>,
    /// Workers emulated in this instance (#1127).
    emulated_workers: bool,
}

/// The natives of an SPB program, each once, in the ABI's order (#1125, #1135).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramNatives {
    /// Every native it declares (the imported modules' too).
    pub declared: Vec<okkama_runtime_abi::Native>,
    /// Those a run can call ([`reach`]): from the entry, or the tests of a test profile.
    pub reachable: Vec<okkama_runtime_abi::Native>,
}

impl ProgramNatives {
    /// The symbols of `natives` that `profile` refuses, sorted.
    pub fn refused(natives: &[okkama_runtime_abi::Native], profile: &budget::Profile) -> Vec<&'static str> {
        let mut refused: Vec<&'static str> =
            natives.iter().filter(|native| !profile.offers(**native)).map(|native| native.symbol()).collect();
        refused.sort_unstable();
        refused
    }
}

/// The natives an SPB program declares and those a run can reach (#1125: what the playground needs for it).
pub fn program_natives(bytes: &[u8]) -> Result<ProgramNatives, Error> {
    let DecodedProgram::Legacy(program) = decode_runtime(bytes).map_err(|e| Error(e.to_string()))?;
    let reached = reach::functions(&program);
    let mut declared = Vec::new();
    let mut reachable = Vec::new();
    for (function, reached) in program.functions.iter().zip(reached) {
        if let Some(native) = function.native.as_deref().and_then(okkama_runtime_abi::Native::resolve) {
            declared.push(native);
            if reached {
                reachable.push(native);
            }
        }
    }
    for natives in [&mut declared, &mut reachable] {
        natives.sort_unstable_by_key(|native| *native as usize);
        natives.dedup();
    }
    Ok(ProgramNatives { declared, reachable })
}

/// The natives a run of an SPB program can reach that `profile` refuses, by symbol, sorted, each once (#1125,
/// #1135: the playground warns before a run, and the site marks the examples that run).
pub fn refused_natives(bytes: &[u8], profile: &budget::Profile) -> Result<Vec<&'static str>, Error> {
    Ok(ProgramNatives::refused(&program_natives(bytes)?.reachable, profile))
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

/// Preserve CLI formatting: SPB, runtime setup and test-profile errors have the
/// launcher prefix (setup since #1097: everything before `main` runs is the
/// load); a failure of the run itself has the runtime report's formatting.
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
            return Ok(test_profile(program, test_report::TestPlan::default(), Some(out), worker_command, &limits, &TestSetup::default()).map(|report| Execution::Tests(report.passed())).map_err(ExecutionError::Load));
        }
        let original_functions = program.function_count();
        // A runtime that cannot be built (an intrinsic's contract, a malformed `SPARROW_CORE_CODE_SHIFT`) is a
        // program that does not load, reported as an SPB error is (#1097).
        let mut runtime = match configured_runtime(program, arguments, stats, worker_command, limits) {
            Ok(runtime) => runtime,
            Err(error) => return Ok(Err(ExecutionError::Load(error))),
        };
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
            let offset = uncaught.cause_traces.iter().map(|(offset, ..)| *offset)
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

/// An uncaught failure of `main` (`okkama_runtime_core::failure::Uncaught` since #1125: the playground's compiler
/// renders it without Sparrow).
pub use okkama_runtime_core::failure::Uncaught;
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
            Some(out) => test_profile(program, plan, Some(out), worker_command, &limits, &TestSetup::default()),
            None => okkama_runtime_core::stdout::to_stderr(|| {
                test_profile(program, plan, None, worker_command, &limits, &TestSetup::default())
            }),
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
    setup: &TestSetup,
) -> Result<test_report::TestReport, Error> {
    match program {
        DecodedProgram::Legacy(program) => test_program(program, plan, out, worker_command, limits, setup),
    }
}

fn test_program(
    program: okkama_sparrow::Program,
    plan: test_report::TestPlan,
    out: Option<Box<dyn std::io::Write + Send>>,
    worker_command: Vec<std::ffi::OsString>,
    limits: &budget::Limits,
    setup: &TestSetup,
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
        if let Some(engine) = setup.engine {
            runtime.use_engine(engine);
        }
        runtime.emulated_workers |= setup.emulated_workers;
        if let (Some(bytes), Some(host)) = (&setup.stdin, runtime.host.as_mut()) {
            host.set_stdin(bytes.clone());
        }
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
#[cfg(feature = "host")]
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
#[cfg(feature = "host")]
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
/// Run `body` on a VM thread with a stack of `STACK_BYTES` (R2); with the host, inside a host session.
#[cfg(not(target_family = "wasm"))]
fn on_vm_thread<T: Send + 'static>(body: impl FnOnce() -> Result<T, Error> + Send + 'static) -> Result<T, Error> {
    std::thread::Builder::new()
        .name("sparrow-core".into())
        .stack_size(STACK_BYTES as usize)
        .spawn(move || {
            // The program's cwd and environment live as long as this run (#40).
            #[cfg(feature = "host")]
            let _host = okkama_runtime_core::host::enter(
                okkama_runtime_core::host::Context::process().without("SPARROW_INTERNAL_WORKER").without("SPARROW_INTERNAL_WORKER_POOL"),
            );
            body()
        })
        .map_err(|e| Error(format!("VM thread: {e}")))?
        .join()
        .map_err(|payload| Error(format!("internal error: {}", runtime::panic_text(&*payload))))?
}

/// wasm32 has no threads: the run takes the caller's stack (#1124). The interpreter's frames are on its own
/// register stack, so only Rust's re-entries (closures called by natives, destroys) use it; a panic is a trap.
#[cfg(target_family = "wasm")]
fn on_vm_thread<T: Send + 'static>(body: impl FnOnce() -> Result<T, Error> + Send + 'static) -> Result<T, Error> {
    body()
}

/// A variable the launcher adds to the program's environment when the
/// process lacks it (`okkama run` passes the pinned ICU this way).
#[cfg(feature = "host")]
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
    #[cfg(feature = "host")]
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
mod embedding_tests;

#[cfg(test)]
mod uncaught_report_tests {
    use super::{Site, Uncaught};

    /// #1009, #1034: a trace cut at the recording limit says how many
    /// activations it left out, under the root's stack and under each cause's.
    #[test]
    fn an_omitted_middle_of_a_trace_is_reported() {
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
        assert_eq!(report, "Stack overflow\n  at f\n... and 7 more activations in the middle, not recorded");
        let whole = Uncaught { trace_omitted: 0, ..uncaught.clone() };
        assert_eq!(whole.render_report("Stack overflow", |_| "  at f\n".into()), "Stack overflow\n  at f");
        // A stack overflow caught and rethrown as the cause of another failure.
        let text = "E: rethrown\nCaused by:\n  RuntimeError: Stack overflow";
        let caused = Uncaught {
            trace_omitted: 0,
            trace_offset: 11,
            cause_traces: vec![(text.len(), vec![Site { function: 0, pc: 1 }], 5)],
            ..uncaught
        };
        assert_eq!(
            caused.render_report(text, |_| "  at f\n".into()),
            "E: rethrown\n  at f\nCaused by:\n  RuntimeError: Stack overflow\n    at f\n  ... and 5 more activations in the middle, not recorded"
        );
    }
}

#[cfg(test)]
mod feature_tests {
    use okkama_runtime_abi::Native;

    /// #1124: every build provides the pure natives (text, numbers, JSON, regex, IDNA, digests, the clocks), and
    /// only a build with the host provides the host's (files, the environment, processes, networks, SQLite, the
    /// terminal through runtime-core); a native a build lacks is unsupported, never routed to a refusal.
    #[test]
    fn the_build_provides_the_pure_natives_and_the_host_its_own() {
        for native in [
            Native::Utf8Encode,
            Native::NetIpParse,
            Native::RegexRegexEscape,
            Native::CryptoDigest,
            Native::LayoutDedent,
            Native::TimeWallClock,
            Native::TimeMonotonicClock,
            Native::TimeSleepNanoseconds,
        ] {
            assert!(crate::natives::supported(native), "{}", native.symbol());
        }
        for native in [
            Native::FsOpen,
            Native::FsPathJoin,
            Native::EnvironmentEnvironmentSnapshot,
            Native::ProcessProcessStateStart,
            Native::SystemSystemCpuCount,
            Native::AccountAccountCurrentUser,
            Native::X509SystemCertificates,
        ] {
            let host = cfg!(feature = "host");
            assert_eq!(crate::natives::supported(native), host, "{}", native.symbol());
        }
    }

    /// #1124: without the JIT every runtime runs on the interpreter, switch or not; with it, the switch chooses.
    #[test]
    fn a_runtime_interprets_without_the_jit_or_when_chosen() {
        let program = okkama_sparrow::assemble(
            "sparrow 18\nmodule \"m\" \"1.0.0\"\ntype t0 int\nfunction f0 \"main\" {\n registers [t0]\n params []\n result t0\n 0: const r0, int 3\n 1: ret r0\n}\nentry f0\n",
        )
        .unwrap();
        let mut runtime = Box::new(crate::runtime::Runtime::new(crate::program::Loaded::verified(program).unwrap()).unwrap());
        let chosen = crate::env_switch::enabled("SPARROW_CORE_INTERPRET");
        assert_eq!(runtime.interp.is_some(), !cfg!(feature = "jit") || chosen);
        assert_eq!(runtime.run_entry(64 << 20).unwrap(), 3);
        assert_eq!(runtime.invocations[0].code != 0, cfg!(feature = "jit") && !chosen, "only the JIT publishes code");
    }
}
