//! `okkama` — command-line front end for the Okkama compiler library.
//!
//! The binary owns everything the library deliberately does not do: reading
//! files and stdin, parsing command-line options, choosing output streams
//! and exit codes. Results go to stdout, diagnostics to stderr.
//!
//! Exit codes: 0 success, 1 a source/lowering/codegen error, 2 invalid command
//! line, 3 an input/output error.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[global_allocator]
#[cfg(not(feature = "allocation-stats"))]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;


#[cfg(feature = "allocation-stats")]
use okkama_allocation_counter as allocation_stats;
use sparrow_core::test_report;
#[cfg(feature = "allocation-stats")]
#[global_allocator]
static ALLOCATOR: allocation_stats::Counting = allocation_stats::Counting;

mod core;
mod doctest;
mod import_paths;
mod lsp;
mod man;
mod ocs;
mod repl;
mod trace;

use blade::manifest;
use okkama_compiler::codegen::{self, Backend, Options as CodegenOptions};
use okkama_compiler::diagnostics::{Diagnostic, render_all};
use okkama_compiler::ide::{Inspection, Severity};
use okkama_compiler::ir;
use okkama_compiler::lowering::{
    self, LinkError, LoadedModule, LoweringError, ModuleLoader, RootFile,
};
use okkama_compiler::source::{SourceFile, Span};
use okkama_compiler::syntax::print::{
    Format, PrintOptions, print_program, tokens_json, tokens_text,
};
use okkama_compiler::syntax::{self, ParseMode, ParseOptions, ast, lexer, prune};

const EXIT_DIAGNOSTICS: u8 = 1;
const EXIT_USAGE: u8 = 2;
const EXIT_IO: u8 = 3;
/// `run -t` could not reach a verdict: a source error, no test to run, an
/// engine error (#339). Failed tests are 1.
const EXIT_TEST_RUN: u8 = 4;







const HELP: &str = "\
okkama — Okkama language front end and IR

USAGE:
    okkama tokens <FILE|-> [--format text|json]
    okkama ast <FILE|-> [--format tree|json] [--spans] [--mode full|production] [--prune-tests]
    okkama check <FILE|->
    okkama fmt [-w|--check] <FILE|->...
    okkama lint <FILE>...
    okkama man [MODULE [NAME | NAME.ACTION]] [--format text|markdown]
    okkama lsp
    okkama repl
    okkama env
    okkama ir <FILE|-> [--format text]
    okkama build <FILE|-> -o <FILE.spb>
    okkama exec [BUDGETS] <FILE.spb>
    okkama run [-t [--filter TEXT]... [--report FORMAT] [--seed N]] <FILE> [FILE.ok...] [ARGS...]
    okkama <FILE.ok> [FILE.ok...]
    okkama help [COMMAND]
    okkama --version

COMMANDS:
    tokens    Print the token stream of a source file
    ast       Parse a source file and print its syntax tree
    check     Check a program and its imports; report every compile-time error
    fmt       Format source files (`-w` rewrites them, `--check` only reports)
    lint      Check programs and report errors, warnings and lint findings
    man       Show the reference page of a module, a declaration or an action,
              from its doc comments; without MODULE, the index of std
    env       Show where okkama finds std and ICU
    lsp       Run the language server (LSP over stdio) for editors
    repl      Read-eval-print loop: statements, expressions and declarations
              in one live Sparrow session (:help inside)
    ir        Lower a source module to IR and print its functions
    build     Compile a program to Sparrow bytecode (.spb)
    exec      Execute a Sparrow bytecode file in this process
    run       Run a program on Sparrow (JIT)

Use `-` as FILE to read the source from standard input. `check`, `ir` and
`build` accept several FILE paths: the files of one module (spec §5.1) share
declarations, each keeps its own imports. With `--tests` (`-t`) they work in
test mode (spec §14): `build` produces a test runner instead of `main`.

AST OPTIONS:
    --format tree|json     Output format (default: tree)
    --spans                Include source spans (byte offsets, lines, columns)
    --mode full            Parse test and fake bodies (default)
    --mode production      Skip test/fake bodies as balanced braces (spec §14);
                           they appear as SkippedBody nodes
    --prune-tests          Remove test and fake declarations after parsing

TOKENS OPTIONS:
    --format text|json     Output format (default: text)

IR OPTIONS:
    --format text          Output format (default: text); see `okkama help ir`

BUILD OPTIONS:
    -o, --output FILE      Output artifact path (required)

RUN OPTIONS:
    -t, --tests        Run inline tests and documentation examples. Fenced
                          `okkama` examples compile by default; `okkama,ignore`
                          illustrates; `okkama,run` opts into isolated execution
                          and requires an adjacent `output` fence (exact stdout).
                          Indented and other-language examples illustrate.
    --filter TEXT         With -t: run only the tests whose name contains
                          TEXT (literally, case-sensitive); repeated, the
                          tests that contain any of the texts. TEST/CASE
                          runs a test whose name contains TEST, and of its
                          test.case sub-cases only those whose names
                          contain CASE
    --report FORMAT       With -t: human (the default: each test as it
                          runs, then PASS or FAIL), json (one JSON document)
                          or junit (JUnit XML); json and junit own stdout,
                          and what the tests print goes to stderr. Every
                          report lists the tests in declaration order
    --seed N              With -t: run the selected tests in the order the
                          seed N (0..9007199254740991) shuffles them to;
                          without it, in declaration order. The report
                          names the seed. Properties keep their own seeds
    --max-steps N         Stop after N steps: function entries and loop
                          iterations (off by default)
    --max-memory SIZE     Stop when the run holds more than SIZE bytes
                          (K/M/G suffixes); also bounds recursion depth
    --max-output SIZE     Stop when stdout and stderr exceed SIZE bytes
    --timeout SECONDS     Interrupt the run after SECONDS of wall-clock time
                          Running out exits 1: \"Execution budget exhausted:
                          steps|memory|output\" or \"Execution interrupted\".
                          An uncaught failure on Sparrow also
                          prints its source line and the call stack;
                          OKKAMA_TRACE=off prints the failure alone.

EXIT STATUS:
    0  success
    1  a source, lowering or codegen error (including invalid UTF-8);
       with run -t, a test failed
    2  invalid command line
    3  an input/output error
    4  run -t reached no verdict: a source error, no test to run, or an
       engine error
";

const TOKENS_HELP: &str = "\
okkama tokens — print the token stream of a source file

USAGE:
    okkama tokens <FILE|-> [--format text|json]

Text output prints one token per line: the position range LINE:COL-LINE:COL
(end exclusive, columns count Unicode characters), the token kind, the
quoted source spelling and, for literals, the decoded value. On lexical
errors nothing is printed to stdout, diagnostics go to stderr and the exit
status is 1.
";

const AST_HELP: &str = "\
okkama ast — parse a source file and print its syntax tree

USAGE:
    okkama ast <FILE|-> [--format tree|json] [--spans] [--mode full|production] [--prune-tests]

OPTIONS:
    --format tree|json     Output format (default: tree)
    --spans                Include source spans
    --mode full            Parse test and fake bodies (default)
    --mode production      Consume test/fake bodies as balanced braces without
                           parsing them (SkippedBody nodes); lexical errors
                           inside them still count
    --prune-tests          Remove test and fake declarations from the tree

The whole input must parse. On any error nothing is printed to stdout,
diagnostics go to stderr and the exit status is 1. Imports are printed as
syntax; the imported modules are not loaded.
";

const IR_HELP: &str = "\
okkama ir — print module and function IR

USAGE:
    okkama ir <FILE|-> [--format text] [--passes LIST | --after PASS]

OPTIONS:
    --format text     Output format (default: text)
    --passes LIST     Run only these IR passes: none, all, or a comma list of
                      regions, tail-calls, fold, hoist, undef, jumps (in that order)
    --after PASS      The IR after PASS: the passes up to it in that order

Input is a complete Okkama program: one or several source files of one module
(spec §5.1). Imports are loaded (import directories, the importing file's
directory, std) and linked into one OIR-1 module: declarations, functions with
blocks ending in `ret`, `br`, `cond`, … Analysis warnings go to stderr.
Constructs the IR does not support yet produce a lowering error; no source
declarations or statements are silently discarded.

Use `-` as FILE to read standard input. This command prints IR without
executing the program.
On any error stdout is empty; diagnostics go to stderr. Source and lowering
errors exit with code 1, usage errors with code 2, and input read errors with 3.
";

const BUILD_HELP: &str = "\
okkama build — compile a program to Sparrow bytecode

USAGE:
    okkama build <FILE|-> -o <FILE.spb>

OPTIONS:
    -o, --output FILE      Output artifact path (required)

Use `-` as input to read Okkama source from standard input.
The pipeline is source -> AST -> module IR -> Sparrow bytecode -> output file.

Several FILE paths form one module (spec §5.1). A generation error leaves
output intact.

Sparrow writes a verified SPB 9 file (register bytecode with the program's
type, object, role and function tables), runnable with `okkama exec FILE.spb`
or the standalone `sparrow FILE.spb`. With -t it holds the test profile, run by
`okkama exec` or `sparrow test`. `sparrow --disassemble` prints its assembly.

Source, lowering and codegen errors exit with code 1, usage errors with code 2,
and input/output errors with code 3. Errors go to stderr; stdout stays empty.
";

const EXEC_HELP: &str = "\
okkama exec — execute Sparrow bytecode

USAGE:
    okkama exec [BUDGETS] <FILE.spb> [ARGS...]

BUDGETS (off by default): --max-steps N, --max-memory SIZE, --max-output SIZE,
--timeout SECONDS (see `okkama help run`); running out exits 1.

Loads and verifies an SPB file, then executes it in the Rust process with
ARGS as the program's arguments; a test-profile file (`build -t`) runs its
tests. Source files are not required. main must return an integer
in 0..255, used as the process exit status; an uncaught failure is reported
on stderr with exit 1. File-format errors exit with 1, usage errors with 2,
and read errors with 3.
";

fn sparrow_exec_command(args: &[String]) -> Outcome {
    let mut limits = sparrow_core::budget::Limits::default();
    let mut args = args;
    while let [option, value, rest @ ..] = args {
        match limits.option(option, value) {
            Ok(true) => args = rest,
            Ok(false) => break,
            Err(message) => return Outcome::usage(&message),
        }
    }
    let Some((path, arguments)) = args.split_first() else {
        return Outcome::usage("exec requires a bytecode file");
    };
    if matches!(path.as_str(), "--help" | "-h") {
        return Outcome::success(EXEC_HELP.to_string());
    }
    if path.starts_with('-') {
        return Outcome::usage("exec requires a file path (stdin and options are not supported)");
    }
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) => {
            return Outcome::failure(format!("okkama: cannot read `{path}`: {error}\n"), EXIT_IO);
        }
    };
    // Bound allocation before decoding untrusted length/count fields.
    let mut bytes = Vec::new();
    if let Err(error) = file
        .take(okkama_sparrow::MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
    {
        return Outcome::failure(format!("okkama: cannot read `{path}`: {error}\n"), EXIT_IO);
    }
    let arguments = arguments.iter().map(|a| a.clone().into_bytes()).collect();
    let trace = std::env::var("OKKAMA_TRACE").as_deref() != Ok("off");
    match sparrow_core::run_executable(
        bytes, arguments, core::worker_command(Path::new(path)), limits,
        sparrow_core::ExecutionOptions {
            tests: Some(Box::new(okkama_runtime_core::stdout::Buffered)), trace, ..Default::default()
        },
    ) {
        Ok(sparrow_core::Execution::Tests(passed)) => Outcome {
            stdout: String::new(), stderr: String::new(), code: u8::from(!passed),
        },
        Ok(sparrow_core::Execution::Main(Ok(value))) => core_outcome(Ok(value)),
        Ok(sparrow_core::Execution::Main(Err((error, trace)))) => {
            let mut outcome = core_outcome(Err(error));
            outcome.stderr.push_str(&trace);
            outcome
        }
        Err(sparrow_core::ExecutionError::Load(error)) => Outcome::failure(format!("okkama: {}\n", error.0), EXIT_DIAGNOSTICS),
        Err(sparrow_core::ExecutionError::Run(error)) => core_outcome(Err(error)),
    }
}

/// A test-profile program on the core: the report goes to stdout.
/// The tests of `bytes`, run as `plan` says, reported as `format` (#339): 0 when
/// none failed, 1 when one did, 4 when the run reached no verdict.
fn core_tests(bytes: Vec<u8>, plan: test_report::TestPlan, format: test_report::ReportFormat, suite: &str) -> Outcome {
    use test_report::ReportFormat;
    let progress: Option<Box<dyn Write + Send>> = match format {
        ReportFormat::Human => Some(Box::new(okkama_runtime_core::stdout::Buffered)),
        ReportFormat::Json | ReportFormat::Junit => None,
    };
    match sparrow_core::run_test_report(bytes, plan, progress) {
        Ok(report) => Outcome {
            stdout: match format {
                ReportFormat::Human => String::new(),
                ReportFormat::Json => test_report::json(&report, suite),
                ReportFormat::Junit => test_report::junit(&report, suite),
            },
            stderr: String::new(),
            code: u8::from(!report.passed()),
        },
        Err(error) => Outcome::failure(format!("okkama: {}\n", error.0), EXIT_TEST_RUN),
    }
}

/// `--seed`'s value: a decimal integer in 0..=2^53-1.
fn test_seed(text: &str) -> Result<u64, String> {
    text.parse::<u64>()
        .ok()
        .filter(|seed| *seed <= sparrow_core::test_report::MAX_SEED)
        .ok_or_else(|| {
            format!(
                "--seed needs a whole number from 0 to {}, not `{text}`",
                sparrow_core::test_report::MAX_SEED
            )
        })
}

/// `core_outcome` of `run_described`, with the failure's call stack.
fn core_described(
    result: Result<Result<i64, (sparrow_core::Error, Option<sparrow_core::Uncaught>)>, sparrow_core::Error>,
) -> (Outcome, Option<sparrow_core::Uncaught>) {
    match result {
        Ok(Ok(value)) => (core_outcome(Ok(value)), None),
        Ok(Err((error, uncaught))) => (core_outcome(Err(error)), uncaught),
        Err(error) => (core_outcome(Err(error)), None),
    }
}

/// Stop the process with the interrupt report one second after `flag` is
/// raised, if the run has not stopped by itself by then: the engines poll the
/// flag while they run code and in long natives that can look at it, not in
/// one that cannot (Argon2, a blocking read; #238).
fn watchdog(flag: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    let _ = std::thread::Builder::new().name("okkama-timeout".into()).spawn(move || {
        while !flag.load(std::sync::atomic::Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
        okkama_runtime_core::stdout::try_flush();
        let _ = io::stdout().flush();
        let _ = writeln!(io::stderr(), "{}", sparrow_core::budget::Exhausted::Interrupted.message());
        std::process::exit(1);
    });
}

/// main's result on the core: the exit status, or the failure report (exit 1).
fn core_outcome(result: Result<i64, sparrow_core::Error>) -> Outcome {
    match result {
        Ok(value) => match u8::try_from(value) {
            Ok(code) => Outcome { stdout: String::new(), stderr: String::new(), code },
            Err(_) => Outcome::failure(format!("RuntimeError: main() must return an int in 0..255, got {value}\n"), 1),
        },
        Err(error) => Outcome::failure(core::report(error), 1),
    }
}

/// Диагностика отказа программы (код 1) и его место со стеком вызовов;
/// `OKKAMA_TRACE=off` — без них (например, для сравнения движков).
/// The trace is rendered only when it is printed: with `OKKAMA_TRACE=off`
/// a stack of millions of frames costs nothing (#241).
fn with_trace(
    mut outcome: Outcome,
    uncaught: Option<sparrow_core::Uncaught>,
    render: impl Fn(&[sparrow_core::Site]) -> String,
) -> Outcome {
    if outcome.code == 1 && std::env::var("OKKAMA_TRACE").as_deref() != Ok("off")
        && let Some(uncaught) = uncaught {
        outcome.stderr = uncaught.render_report(&outcome.stderr, render);
    }
    outcome
}

/// A finished command: what to print and how to exit.
struct Outcome {
    stdout: String,
    stderr: String,
    code: u8,
}

impl Outcome {
    fn success(stdout: String) -> Self {
        Outcome {
            stdout,
            stderr: String::new(),
            code: 0,
        }
    }

    fn failure(stderr: String, code: u8) -> Self {
        Outcome {
            stdout: String::new(),
            stderr,
            code,
        }
    }

    fn usage(message: &str) -> Self {
        Outcome::failure(
            format!("okkama: {message}\nTry `okkama --help` for more information.\n"),
            EXIT_USAGE,
        )
    }
}

fn main() -> ExitCode {
    #[cfg(feature = "allocation-stats")]
    let _allocation_report = allocation_stats::Report;
    #[cfg(not(feature = "allocation-stats"))]
    sparrow_core::budget::set_memory_probe(sparrow_core::budget::mimalloc_committed);
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Сервер языка владеет stdin/stdout сам.
    if args.first().map(String::as_str) == Some("lsp") {
        return lsp::run();
    }
    // The REPL reads stdin and writes as it goes.
    if args.first().map(String::as_str) == Some("repl") {
        return repl::run();
    }
    let outcome = run(&args);
    // The program's buffered output goes before the command's own and before
    // a failure report (#132). A closed stdout (`okkama ast big.ok | head`) is
    // not an error worth a panic; anything printed so far is all the reader
    // wanted.
    let _ = okkama_runtime_core::stdout::write(outcome.stdout.as_bytes());
    let _ = okkama_runtime_core::stdout::flush();
    let _ = io::stderr().lock().write_all(outcome.stderr.as_bytes());
    // Development instrumentation (OKKAMA_DEV): flush events, write the counters, print the summary.
    okkama_devtrace::finish();
    ExitCode::from(outcome.code)
}

fn run(args: &[String]) -> Outcome {
    let Some(command) = args.first() else {
        return Outcome::failure(HELP.to_string(), EXIT_USAGE);
    };
    let rest = &args[1..];
    match command.as_str() {
        "-h" | "--help" => Outcome::success(HELP.to_string()),
        "help" => match rest.first().map(String::as_str) {
            None => Outcome::success(HELP.to_string()),
            Some("tokens") => Outcome::success(TOKENS_HELP.to_string()),
            Some("ast") => Outcome::success(AST_HELP.to_string()),
            Some("ir") => Outcome::success(IR_HELP.to_string()),
            Some("build") => Outcome::success(BUILD_HELP.to_string()),
            Some("exec") => Outcome::success(EXEC_HELP.to_string()),
            Some("man") => Outcome::success(man::HELP.to_string()),
            Some(other) => Outcome::usage(&format!("unknown command `{other}`")),
        },
        "-V" | "--version" => Outcome::success(format!("okkama {}\n", env!("CARGO_PKG_VERSION"))),
        // Endpoint conformance-адаптера (`conformance/adapters/PROTOCOL.md`).
        "--ocs-capabilities" => ocs::capabilities(),
        "--ocs-request" | "--ocs-response" => ocs::request(args),
        "tokens" => tokens_command(rest),
        "ast" => ast_command(rest),
        "check" => check_command(rest),
        "fmt" => fmt_command(rest),
        "env" => env_command(rest),
        "lint" => lint_command(rest),
        "man" => man::command(rest),
        "ir" => ir_command(rest),
        "build" => build_command(rest),
        "exec" => sparrow_exec_command(rest),
        // Child side of a process worker of the core (`core.rs`).
        "--worker" => match core::worker(rest) {
            Ok(()) => Outcome { stdout: String::new(), stderr: String::new(), code: 0 },
            Err(message) => Outcome::failure(format!("okkama worker: {message}\n"), 1),
        },
        "--doctest-exec" => doctest::child(rest),
        "run" => run_command(rest),
        // Как у Perl-CLI: `okkama [-t] program.ok args…`.
        "-t" => run_command(args),
        other if other.ends_with(".ok") => run_command(args),
        other => Outcome::usage(&format!("unknown command `{other}`")),
    }
}

/// Command families have separate flags while sharing input/help handling.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum CommandOptions {
    Basic,
    Ast,
    /// `check` и `ir`: программа из одного или нескольких файлов.
    Program,
    Build,
}

#[derive(Default)]
struct CommandLine {
    input: Option<String>,
    /// Остальные файлы многофайлового модуля (`check`, `ir`, `build`).
    more_inputs: Vec<String>,
    /// `--tests`: тестовый режим (§14) — тесты корня вместо `main`.
    tests: bool,
    format: Option<String>,
    spans: bool,
    mode: Option<String>,
    prune_tests: bool,
    output: Option<String>,
    help: bool,
}

fn parse_command_line(args: &[String], options: CommandOptions) -> Result<CommandLine, String> {
    let mut line = CommandLine::default();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let (flag, inline_value) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value.to_string())),
            _ => (arg.as_str(), None),
        };
        let mut value_for = |name: &str| -> Result<String, String> {
            match inline_value.clone() {
                Some(value) => Ok(value),
                None => iter
                    .next()
                    .cloned()
                    .ok_or_else(|| format!("option `{name}` needs a value")),
            }
        };
        match flag {
            "-h" | "--help" => line.help = true,
            "--tests" | "-t"
                if matches!(options, CommandOptions::Program | CommandOptions::Build) =>
            {
                line.tests = true;
            }
            "--format" if options != CommandOptions::Build => {
                line.format = Some(value_for("--format")?);
            }
            "--spans" if options == CommandOptions::Ast => line.spans = true,
            "--mode" if options == CommandOptions::Ast => line.mode = Some(value_for("--mode")?),
            "--prune-tests" if options == CommandOptions::Ast => line.prune_tests = true,
            "-o" | "--output" if options == CommandOptions::Build => {
                line.output = Some(value_for(flag)?);
            }
            "-" => set_input(&mut line, arg, options)?,
            other if other.starts_with('-') => return Err(format!("unknown option `{other}`")),
            _ => set_input(&mut line, arg, options)?,
        }
    }
    Ok(line)
}

fn set_input(line: &mut CommandLine, arg: &str, options: CommandOptions) -> Result<(), String> {
    let program = matches!(options, CommandOptions::Program | CommandOptions::Build);
    if program && line.input.is_some() && arg != "-" && line.input.as_deref() != Some("-") {
        line.more_inputs.push(arg.to_string());
        return Ok(());
    }
    if line.input.is_some() {
        return Err(format!(
            "unexpected extra argument `{arg}`; only one input is accepted"
        ));
    }
    line.input = Some(arg.to_string());
    Ok(())
}

/// Reads the input as a source file, or produces the failing outcome.
fn load(input: Option<&str>) -> Result<SourceFile, Outcome> {
    let Some(input) = input else {
        return Err(Outcome::usage("missing input file (use `-` for stdin)"));
    };
    let (name, bytes) = if input == "-" {
        let mut bytes = Vec::new();
        if let Err(error) = io::stdin().lock().read_to_end(&mut bytes) {
            return Err(Outcome::failure(
                format!("okkama: cannot read standard input: {error}\n"),
                EXIT_IO,
            ));
        }
        ("<stdin>".to_string(), bytes)
    } else {
        match std::fs::read(input) {
            Ok(bytes) => (input.to_string(), bytes),
            Err(error) => {
                return Err(Outcome::failure(
                    format!("okkama: cannot read `{input}`: {error}\n"),
                    EXIT_IO,
                ));
            }
        }
    };
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => {
            let offset = error.utf8_error().valid_up_to();
            return Err(Outcome::failure(
                format!(
                    "{name}: lexical error: Source is not valid UTF-8 (invalid byte at offset {offset}).\n"
                ),
                EXIT_DIAGNOSTICS,
            ));
        }
    };
    SourceFile::new(name, text)
        .map_err(|error| Outcome::failure(format!("okkama: {error}\n"), EXIT_DIAGNOSTICS))
}

/// Все файлы программы: первый (может быть `-`), затем остальные.
fn load_all(line: &CommandLine) -> Result<Vec<SourceFile>, Outcome> {
    let mut sources = vec![load(line.input.as_deref())?];
    for input in &line.more_inputs {
        sources.push(load(Some(input))?);
    }
    // The files of one module are taken in name order, so the order of the
    // arguments changes nothing — which declaration is first, the default
    // entry file, the order of tests and of diagnostics (H-16).
    sources.sort_by(|a, b| a.name().cmp(b.name()));
    Ok(sources)
}

/// Разбор файлов программы: без тестовых объявлений или, с `--tests`, с
/// тестами и fakes (§14).
/// Программа без тестов разбирается в production-грамматике: тела `test` и
/// `fake` не разбираются (§14), как у импортов и в OCS.
fn parse_all(sources: &[SourceFile], tests: bool) -> Result<Vec<ast::Program>, Outcome> {
    let mode = if tests { ParseMode::Full } else { ParseMode::Production };
    Ok(prune_all(parse_raw_in(sources, mode)?, tests))
}

/// Разбор файлов программы как есть: тесты и fakes ещё на месте.
fn parse_raw(sources: &[SourceFile]) -> Result<Vec<ast::Program>, Outcome> {
    parse_raw_in(sources, ParseMode::Full)
}

fn parse_raw_in(sources: &[SourceFile], mode: ParseMode) -> Result<Vec<ast::Program>, Outcome> {
    let options = ParseOptions { mode, ..ParseOptions::default() };
    sources
        .iter()
        .map(|source| {
            syntax::parse(source, &options)
                .map_err(|diagnostics| diagnostics_outcome(&diagnostics, source))
        })
        .collect()
}

/// Профиль программы: производственный (тесты и fakes убраны) или тестовый.
fn prune_all(programs: Vec<ast::Program>, tests: bool) -> Vec<ast::Program> {
    programs
        .into_iter()
        .map(|program| {
            if tests {
                syntax::prune::desugar_test_items(program)
            } else {
                syntax::prune::strip_test_items(program)
            }
        })
        .collect()
}

fn loader(tests: bool) -> FileLoader {
    let loader = FileLoader::from_env();
    if tests { loader.for_tests() } else { loader }
}

fn root_files<'a>(sources: &'a [SourceFile], programs: &'a [ast::Program]) -> Vec<RootFile<'a>> {
    sources
        .iter()
        .zip(programs)
        .map(|(source, program)| RootFile { program, source })
        .collect()
}

fn diagnostics_outcome(diagnostics: &[Diagnostic], source: &SourceFile) -> Outcome {
    Outcome::failure(render_all(diagnostics, source), EXIT_DIAGNOSTICS)
}

fn tokens_command(args: &[String]) -> Outcome {
    let line = match parse_command_line(args, CommandOptions::Basic) {
        Ok(line) => line,
        Err(message) => return Outcome::usage(&message),
    };
    if line.help {
        return Outcome::success(TOKENS_HELP.to_string());
    }
    let json = match line.format.as_deref() {
        None | Some("text") => false,
        Some("json") => true,
        Some(other) => {
            return Outcome::usage(&format!(
                "unknown token format `{other}` (use text or json)"
            ));
        }
    };
    let source = match load(line.input.as_deref()) {
        Ok(source) => source,
        Err(outcome) => return outcome,
    };
    let lexed = lexer::lex(source.text());
    if lexed.has_errors() {
        return diagnostics_outcome(&lexed.diagnostics, &source);
    }
    let out = if json {
        tokens_json(&lexed.tokens, &source)
    } else {
        tokens_text(&lexed.tokens, &source)
    };
    Outcome::success(out)
}

fn ast_command(args: &[String]) -> Outcome {
    let line = match parse_command_line(args, CommandOptions::Ast) {
        Ok(line) => line,
        Err(message) => return Outcome::usage(&message),
    };
    if line.help {
        return Outcome::success(AST_HELP.to_string());
    }
    let format = match line.format.as_deref() {
        None | Some("tree") => Format::Tree,
        Some("json") => Format::Json,
        Some(other) => {
            return Outcome::usage(&format!("unknown AST format `{other}` (use tree or json)"));
        }
    };
    let mode = match line.mode.as_deref() {
        None | Some("full") => ParseMode::Full,
        Some("production") => ParseMode::Production,
        Some(other) => {
            return Outcome::usage(&format!("unknown mode `{other}` (use full or production)"));
        }
    };
    let source = match load(line.input.as_deref()) {
        Ok(source) => source,
        Err(outcome) => return outcome,
    };
    let options = ParseOptions {
        mode,
        ..ParseOptions::default()
    };
    let program = match syntax::parse(&source, &options) {
        Ok(program) => program,
        Err(diagnostics) => return diagnostics_outcome(&diagnostics, &source),
    };
    let program = if line.prune_tests {
        prune::strip_test_items(program)
    } else {
        program
    };
    let options = PrintOptions { spans: line.spans };
    Outcome::success(print_program(&program, &source, format, options))
}

fn ir_command(args: &[String]) -> Outcome {
    // `--passes LIST` / `--after PASS`: the IR after some passes (H-19).
    let mut rest = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--passes" | "--after" => {
                let Some(value) = iter.next() else {
                    return Outcome::usage(&format!("{arg} needs a value"));
                };
                let spec = if arg == "--passes" {
                    value.clone()
                } else {
                    let Some(at) = lowering::PASSES.iter().position(|pass| pass == value) else {
                        return Outcome::usage(&format!("unknown pass `{value}` (passes: {})", lowering::PASSES.join(", ")));
                    };

                    lowering::PASSES[..=at].join(",")
                };
                lowering::select_passes(&spec);
            }
            _ => rest.push(arg.clone()),
        }
    }
    let args = &rest[..];
    let line = match parse_command_line(args, CommandOptions::Program) {
        Ok(line) => line,
        Err(message) => return Outcome::usage(&message),
    };
    if line.help {
        return Outcome::success(IR_HELP.to_string());
    }
    match line.format.as_deref() {
        None | Some("text") => {}
        Some(other) => {
            return Outcome::usage(&format!("unknown IR format `{other}` (use text)"));
        }
    }
    let sources = match load_all(&line) {
        Ok(sources) => sources,
        Err(outcome) => return outcome,
    };
    match lower_source(&sources, line.tests) {
        Ok((module, warnings)) => Outcome {
            stdout: module.to_string(),
            stderr: warnings,
            code: 0,
        },
        Err(outcome) => outcome,
    }
}

/// `okkama check`: семантический анализ программы и её импортов без lowering.
/// Печатаются все ошибки, а не только первая.
const FMT_HELP: &str = "okkama fmt — format Okkama source files

USAGE:
    okkama fmt [-w | --check] <FILE|->...

Prints the formatted text of one FILE (or `-`, standard input) to standard
output. `-w` rewrites every FILE in place; `--check` changes nothing and lists
the files that are not formatted (exit status 1 if any). The style is
perlstyle: 4-space indentation, one statement or member per line, `else` and
`catch` on their own line, spaces around binary operators and after commas.
Only whitespace changes: tokens and comments are kept and verified.
";

const LINT_HELP: &str = "okkama lint — errors, warnings and lint findings

USAGE:
    okkama lint <FILE>...

Checks each FILE like `okkama check` (imports included; files with tests in
test mode) and adds the lint rules: unused-import, unused-let,
unused-hidden, unreachable, snake-case, missing-docs and unresolved-doc-link.
Missing public docs warn for user modules. Completed std documentation waves
require full public declaration coverage; other std areas still warn. Prints
`file:line:column: severity[rule]: message`; exit status 1 if anything was
reported. A `let` whose name starts with `_` is never reported as unused.
";

fn lint_command(args: &[String]) -> Outcome {
    let mut files = Vec::new();
    for arg in args {
        match arg.as_str() {
            "-h" | "--help" => return Outcome::success(LINT_HELP.to_string()),
            other if other.starts_with('-') => {
                return Outcome::usage(&format!("unknown lint option `{other}`"));
            }
            other => files.push(other.to_string()),
        }
    }
    if files.is_empty() {
        return Outcome::usage("lint needs a FILE");
    }
    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut code = 0;
    for file in &files {
        let path = std::fs::canonicalize(file).unwrap_or_else(|_| PathBuf::from(file));
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                stderr.push_str(&format!("okkama: cannot read `{file}`: {error}\n"));
                code = EXIT_IO;
                continue;
            }
        };
        let source = match SourceFile::new(path.display().to_string(), text) {
            Ok(source) => source,
            Err(error) => {
                stderr.push_str(&format!("okkama: {error}\n"));
                code = EXIT_IO;
                continue;
            }
        };
        let inspection = inspect_file(&source, loader);
        for found in &inspection.diagnostics {
            let shown = if found.path == source.name() {
                file.clone()
            } else {
                found.path.clone()
            };
            let severity = match found.severity {
                okkama_compiler::ide::Severity::Error => "error",
                okkama_compiler::ide::Severity::Warning => "warning",
            };
            let rule = found
                .code
                .as_ref()
                .map_or(String::new(), |code| format!("[{code}]"));
            stdout.push_str(&format!(
                "{shown}:{}:{}: {severity}{rule}: {}\n",
                found.range.start.line + 1,
                found.range.start.character + 1,
                found.message
            ));
            if code == 0 {
                code = 1;
            }
        }
    }
    Outcome {
        stdout,
        stderr,
        code,
    }
}

/// Диагностики файла для `okkama lint` и `okkama lsp`. Файл с тестами или
/// fakes проверяется в тестовом режиме, чтобы их тела тоже анализировались;
/// тестовый режим разрешает `Role.fake()`, поэтому ошибки, которые находит
/// только производственный режим, добавляются вторым проходом без повторов
/// (#649). Предупреждения и находки линтера — только тестового прохода: без
/// тестов импорт или `hidden`, нужные лишь тестам, выглядели бы лишними.
fn inspect_file(source: &SourceFile, loader_for: impl Fn(bool) -> FileLoader) -> Inspection {
    // One full parse decides the mode and is the first pass's parse (#472):
    // without test items it is also the production parse.
    let full = okkama_compiler::syntax::parse(source, &okkama_compiler::syntax::ParseOptions::default()).ok();
    let test_mode = full.as_ref().is_none_or(okkama_compiler::ide::program_has_tests);
    let mut loader = loader_for(test_mode);
    let mut inspection = lowering::inspect_parsed(source, full, &mut loader, test_mode);
    if !test_mode || !inspection.parsed {
        return inspection;
    }
    let mut loader = loader_for(false);
    let production = lowering::inspect(source, &mut loader, false);
    for found in production.diagnostics {
        let error = found.severity == Severity::Error;
        if error && !inspection.diagnostics.contains(&found) {
            inspection.diagnostics.push(found);
        }
    }
    inspection
}

fn fmt_command(args: &[String]) -> Outcome {
    let mut write = false;
    let mut check = false;
    let mut files = Vec::new();
    for arg in args {
        match arg.as_str() {
            "-h" | "--help" => return Outcome::success(FMT_HELP.to_string()),
            "-w" | "--write" => write = true,
            "--check" => check = true,
            other if other.starts_with('-') && other != "-" => {
                return Outcome::usage(&format!("unknown fmt option `{other}`"));
            }
            other => files.push(other.to_string()),
        }
    }
    if files.is_empty() {
        return Outcome::usage("fmt needs a FILE (or `-` for standard input)");
    }
    if write && check {
        return Outcome::usage("use either -w or --check");
    }
    if !write && !check && files.len() > 1 {
        return Outcome::usage(
            "printing to standard output takes one FILE; use -w or --check for several",
        );
    }
    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut failed = false;
    let mut io_failed = false;
    let mut unformatted = false;
    for file in &files {
        let text = if file == "-" {
            let mut text = String::new();
            if let Err(error) = io::stdin().read_to_string(&mut text) {
                return Outcome::failure(
                    format!("okkama: cannot read standard input: {error}\n"),
                    EXIT_DIAGNOSTICS,
                );
            }
            text
        } else {
            match std::fs::read_to_string(file) {
                Ok(text) => text,
                Err(error) => {
                    stderr.push_str(&format!("okkama: cannot read `{file}`: {error}\n"));
                    io_failed = true;
                    continue;
                }
            }
        };
        let name = if file == "-" {
            "<stdin>"
        } else {
            file.as_str()
        };
        let source = match SourceFile::new(name, text.clone()) {
            Ok(source) => source,
            Err(error) => {
                stderr.push_str(&format!("okkama: {error}\n"));
                failed = true;
                continue;
            }
        };
        let formatted = match okkama_compiler::syntax::format::format(&source) {
            Ok(formatted) => formatted,
            Err(okkama_compiler::syntax::format::FormatError::Syntax(diagnostics)) => {
                stderr.push_str(&render_all(&diagnostics, &source));
                failed = true;
                continue;
            }
            Err(okkama_compiler::syntax::format::FormatError::Internal(message)) => {
                stderr.push_str(&format!(
                    "{name}: formatter error (file left unchanged): {message}\n"
                ));
                failed = true;
                continue;
            }
        };
        if check {
            if formatted != text {
                stdout.push_str(&format!("{name}\n"));
                unformatted = true;
            }
        } else if write && file != "-" {
            if formatted != text
                && let Err(error) = std::fs::write(file, &formatted) {
                    stderr.push_str(&format!("okkama: cannot write `{file}`: {error}\n"));
                    io_failed = true;
                }
        } else {
            stdout.push_str(&formatted);
        }
    }
    Outcome {
        stdout,
        stderr,
        code: if io_failed {
            EXIT_IO
        } else if failed {
            EXIT_DIAGNOSTICS
        } else if unformatted {
            1
        } else {
            0
        },
    }
}

fn check_command(args: &[String]) -> Outcome {
    let line = match parse_command_line(args, CommandOptions::Program) {
        Ok(line) => line,
        Err(message) => return Outcome::usage(&message),
    };
    if line.help {
        return Outcome::success(HELP.to_string());
    }
    let sources = match load_all(&line) {
        Ok(sources) => sources,
        Err(outcome) => return outcome,
    };
    if let Err(outcome) = doctest::gate(&sources) { return outcome; }
    let parsed = match parse_raw(&sources) {
        Ok(parsed) => parsed,
        Err(outcome) => return outcome,
    };
    // Code that exists only in test mode — tests and fakes — is checked too
    // (#649): a second pass in test mode. Test mode is not a superset of
    // production (`Role.fake()` is allowed there), so production keeps its
    // own pass; diagnostics found by both are reported once.
    let test_profile = !line.tests && parsed.iter().any(okkama_compiler::ide::program_has_tests);
    let tested = test_profile.then(|| parsed.clone());
    let mut report = check_profile(&sources, prune_all(parsed, line.tests), line.tests);
    if let Some(parsed) = tested {
        report.merge(check_profile(&sources, prune_all(parsed, true), true));
    }
    report.outcome()
}

/// Итог `okkama check` в одном профиле: строки ошибок и предупреждений.
#[derive(Default)]
struct CheckReport {
    errors: Vec<String>,
    warnings: Vec<String>,
}

impl CheckReport {
    /// Добавляет диагностики другого профиля, которых ещё нет.
    fn merge(&mut self, other: CheckReport) {
        for error in other.errors {
            if !self.errors.contains(&error) {
                self.errors.push(error);
            }
        }
        for warning in other.warnings {
            if !self.warnings.contains(&warning) {
                self.warnings.push(warning);
            }
        }
    }

    /// Ошибки — код 1 и только они; без ошибок — предупреждения и код 0.
    fn outcome(self) -> Outcome {
        if self.errors.is_empty() {
            return Outcome {
                stdout: String::new(),
                stderr: self.warnings.concat(),
                code: 0,
            };
        }
        Outcome::failure(self.errors.concat(), EXIT_DIAGNOSTICS)
    }
}

/// Анализ программы в одном профиле (производственном или тестовом).
fn check_profile(sources: &[SourceFile], programs: Vec<ast::Program>, tests: bool) -> CheckReport {
    let source = &sources[0];
    let mut loader = loader(tests);
    let roots = root_files(sources, &programs);
    let checked = if tests {
        lowering::check_linked_tests(&roots, &mut loader)
    } else {
        lowering::check_linked_files(&roots, &mut loader)
    };
    let mut report = CheckReport::default();
    match checked {
        Ok(warnings) => {
            for found in &warnings {
                report.warnings.push(render_warnings(source, std::slice::from_ref(found)));
            }
        }
        Err(Err(text)) => report.errors.push(text),
        Err(Ok(errors)) => {
            for found in errors {
                let file = found.source.as_ref().unwrap_or(source);
                let location = file.location(found.error.span.start);
                let kind = if found.error.unsupported {
                    "analysis limitation"
                } else {
                    "semantic error"
                };
                report.errors.push(format!(
                    "{}:{}:{}: {kind}: {}\n",
                    file.name(),
                    location.line,
                    location.column,
                    found.error.message
                ));
            }
        }
    }
    report
}

/// The same source-to-IR pipeline serves both inspection and artifact builds.
///
/// IR describes the production program: test and fake declarations are removed
/// by the explicit pruning pass before lowering, never skipped by it.
fn lower_source(sources: &[SourceFile], tests: bool) -> Result<(ir::Module, String), Outcome> {
    doctest::gate(sources)?;
    let programs = parse_all(sources, tests)?;
    let source = &sources[0];
    let mut loader = loader(tests);
    let roots = root_files(sources, &programs);
    let lowered = if tests {
        lowering::lower_linked_tests(&roots, &mut loader)
    } else {
        lowering::lower_linked_files(&roots, &mut loader)
    };
    lowered
        .map(|(module, warnings)| (module, render_warnings(source, &warnings)))
        .map_err(|error| match error {
            LinkError::Lowering {
                source: imported,
                error,
            } => {
                let kind = match error {
                    LoweringError::Unsupported { .. } => "lowering error",
                    LoweringError::Invalid { .. } => "semantic error",
                };
                ir_error_outcome(
                    imported.as_ref().unwrap_or(source),
                    error.span(),
                    kind,
                    error.message(),
                )
            }
            LinkError::Load(text) => Outcome::failure(text, EXIT_DIAGNOSTICS),
        })
}

/// Импорты из файлов, как в Perl: сначала рядом с импортирующим файлом и в
/// корне его пакета (`blade.toml`), затем системные каталоги
/// `OKKAMA_IMPORT_DIRS` (через `:`), затем std (`OKKAMA_STDLIB_DIR`, по
/// умолчанию `/usr/local/lib/okkama/std`). `pkg/…` — зависимости из `pkg/`
/// корня проекта; файлы std ищут импорты только в std.
struct FileLoader {
    import_dirs: Vec<PathBuf>,
    stdlib: PathBuf,
    /// Тестовый режим (§14): импорты разбираются с fakes и тестами.
    test_mode: bool,
    /// Несохранённые тексты открытых в редакторе файлов (`okkama lsp`), по
    /// каноническому пути.
    overlay: std::collections::HashMap<PathBuf, std::rc::Rc<str>>,
}

impl FileLoader {
    /// Std из запроса conformance; каталоги импорта — из окружения.
    fn with_stdlib(stdlib: PathBuf) -> Self {
        Self {
            stdlib,
            ..Self::from_env()
        }
    }

    fn from_env() -> Self {
        let import_dirs = std::env::var("OKKAMA_IMPORT_DIRS")
            .map(|dirs| {
                dirs.split(':')
                    .filter(|dir| !dir.is_empty())
                    .map(PathBuf::from)
                    .collect()
            })
            .unwrap_or_default();
        Self {
            import_dirs,
            stdlib: stdlib_dir(),
            test_mode: false,
            overlay: std::collections::HashMap::new(),
        }
    }

    fn for_tests(mut self) -> Self {
        self.test_mode = true;
        self
    }
}

impl ModuleLoader for FileLoader {
    fn load(&mut self, path: &str, importer: &str) -> Result<Option<LoadedModule>, String> {
        let (file, dirs) = self.candidates(path, importer);
        self.load_from(&file, &dirs)
    }

    fn resolve(&mut self, path: &str, importer: &str) -> Result<Option<String>, String> {
        let (file, dirs) = self.candidates(path, importer);
        Ok(locate(&file, &dirs).map(|found| found.display().to_string()))
    }

    fn resolve_std(&mut self, path: &str, _importer: &str) -> Result<Option<String>, String> {
        Ok(locate(&format!("{path}.ok"), std::slice::from_ref(&self.stdlib)).map(|found| found.display().to_string()))
    }

    /// В blade-проекте `pkg/…` — только прямые зависимости пакета, которому
    /// принадлежит импортирующий файл; вне проекта `pkg/` — обычный каталог.
    fn check_import(&mut self, path: &str, importer: &str) -> Result<(), String> {
        if !manifest::is_dependency_path(path) {
            return Ok(());
        }
        let importer = importer_path(importer);
        if self.in_std(&importer) {
            return Ok(());
        }
        let Some(package) = manifest::package_root(&importer) else {
            return Ok(());
        };
        let found = manifest::Manifest::read(&package)?;
        if found.dependency_of(path).is_none() {
            let name = path.split('/').nth(1).unwrap_or_default();
            return Err(format!(
                "\"{name}\" is not a dependency: add it to [dependencies] in {}",
                package.join(manifest::MANIFEST).display()
            ));
        }
        Ok(())
    }

    fn in_project(&mut self, file: &str) -> bool {
        let file = importer_path(file);
        !self.in_std(&file) && manifest::package_root(&file).is_some()
    }

    /// `std.…`: модуль только из std, а не из каталога импорта или программы.
    fn load_std(&mut self, path: &str, _importer: &str) -> Result<Option<LoadedModule>, String> {
        let stdlib = self.stdlib.clone();
        self.load_from(&format!("{path}.ok"), &[stdlib])
    }
}

/// Путь импортирующего файла; у виртуальных (`<stdin>`, `<repl>`) — в текущем каталоге.
fn importer_path(importer: &str) -> PathBuf {
    let path = if importer.starts_with('<') {
        Path::new(".").join("<input>")
    } else {
        PathBuf::from(importer)
    };
    let absolute = std::path::absolute(&path).unwrap_or(path);
    match (absolute.parent(), absolute.file_name()) {
        (Some(dir), Some(name)) => std::fs::canonicalize(dir)
            .map(|dir| dir.join(name))
            .unwrap_or(absolute),
        _ => absolute,
    }
}

/// The first file `file` of the directories `dirs`, canonical.
fn locate(file: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    let candidate = dirs.iter().map(|dir| dir.join(file)).find(|candidate| candidate.is_file())?;
    Some(std::fs::canonicalize(&candidate).unwrap_or(candidate))
}

impl FileLoader {
    /// The file name of import `path` and the directories to look in.
    fn candidates(&self, path: &str, importer: &str) -> (String, Vec<PathBuf>) {
        let file = if path.ends_with(".ok") {
            path.to_string()
        } else {
            format!("{path}.ok")
        };
        // A module never imports itself: `"json"` from app/json.ok skips
        // app/json.ok and finds std/json.
        let itself = std::fs::canonicalize(importer_path(importer)).ok();
        let dirs = self
            .search_dirs(path, importer)
            .into_iter()
            .filter(|dir| itself.is_none() || std::fs::canonicalize(dir.join(&file)).ok() != itself)
            .collect();
        (file, dirs)
    }

    fn in_std(&self, file: &Path) -> bool {
        let stdlib = std::fs::canonicalize(&self.stdlib).unwrap_or_else(|_| self.stdlib.clone());
        file.starts_with(stdlib)
    }

    /// Каталоги поиска импорта `path` из файла `importer`, по порядку.
    fn search_dirs(&self, path: &str, importer: &str) -> Vec<PathBuf> {
        let importer = importer_path(importer);
        let base = importer
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        // Std imports name modules from the std root (`"template/model"`), so
        // the root comes first: `"json"` from std/template/json.ok is std/json.
        if self.in_std(&importer) {
            return vec![self.stdlib.clone(), base];
        }
        let package = manifest::package_root(&importer);
        if let Some(package) = &package
            && manifest::is_dependency_path(path) {
                return vec![manifest::project_root(package)];
            }
        let mut dirs = vec![base];
        dirs.extend(package.filter(|package| !dirs.contains(package)));
        dirs.extend(self.import_dirs.iter().cloned());
        dirs.push(self.stdlib.clone());
        dirs
    }

    /// Первый файл `file` из каталогов `dirs`, разобранный как модуль.
    fn load_from(&mut self, file: &str, dirs: &[PathBuf]) -> Result<Option<LoadedModule>, String> {
        if let Some(canonical) = locate(file, dirs) {
            let name = canonical.display().to_string();
            let text = match self.overlay.get(&canonical) {
                Some(text) => text.to_string(),
                None => std::fs::read_to_string(&canonical)
                    .map_err(|error| format!("okkama: cannot read import `{name}`: {error}\n"))?,
            };
            let source = SourceFile::new(name.clone(), text)
                .map_err(|error| format!("okkama: {error}\n"))?;
            // Production — без тестов (§14); тестовый режим — с fakes импортов.
            let options = ParseOptions {
                mode: if self.test_mode {
                    ParseMode::Full
                } else {
                    ParseMode::Production
                },
                ..ParseOptions::default()
            };
            let program = syntax::parse(&source, &options)
                .map_err(|diagnostics| render_all(&diagnostics, &source))?;
            let program = if self.test_mode {
                prune::desugar_test_items(program)
            } else {
                prune::strip_test_items(program)
            };
            return Ok(Some(LoadedModule {
                id: name,
                program,
                source,
            }));
        }
        Ok(None)
    }
}

fn build_command(args: &[String]) -> Outcome {
    let line = match parse_command_line(args, CommandOptions::Build) {
        Ok(line) => line,
        Err(message) => return Outcome::usage(&message),
    };
    if line.help {
        return Outcome::success(BUILD_HELP.to_string());
    }
    let backend = Backend::Sparrow;
    let output = match line.output.as_deref() {
        Some("") => return Outcome::usage("output file path must not be empty"),
        Some(output) => output,
        None => return Outcome::usage("missing output file (use -o FILE)"),
    };
    let Some(artifact_name) = Path::new(output).file_stem().and_then(|name| name.to_str()) else {
        return Outcome::usage("output path must contain a file name");
    };
    let sources = match load_all(&line) {
        Ok(sources) => sources,
        Err(outcome) => return outcome,
    };
    let (module, warnings) = match lower_source(&sources, line.tests) {
        Ok(lowered) => lowered,
        Err(outcome) => return outcome,
    };
    let options = CodegenOptions {
        artifact_name: artifact_name.to_string(),
    };
    let artifact = match codegen::generate(&module, backend, &options) {
        Ok(artifact) => artifact,
        Err(error) => {
            return Outcome::failure(
                format!("okkama: codegen error: {error}\n"),
                EXIT_DIAGNOSTICS,
            );
        }
    };
    // Open the destinations only after generation succeeds. Backend errors must
    // leave a previous artifact intact and must not create an empty bytecode file.
    let directory = Path::new(output).parent().unwrap_or(Path::new(""));
    for (index, file) in artifact.files.iter().enumerate() {
        let path = if index == 0 {
            Path::new(output).to_path_buf()
        } else {
            directory.join(&file.name)
        };
        if let Some(parent) = path.parent().filter(|_| index > 0)
            && let Err(error) = std::fs::create_dir_all(parent) {
                return Outcome::failure(
                    format!("okkama: cannot create `{}`: {error}\n", parent.display()),
                    EXIT_IO,
                );
            }
        if let Err(error) = std::fs::write(&path, &file.bytes) {
            return Outcome::failure(
                format!("okkama: cannot write `{}`: {error}\n", path.display()),
                EXIT_IO,
            );
        }
    }
    Outcome {
        stdout: String::new(),
        stderr: warnings,
        code: 0,
    }
}

/// `okkama run`: Sparrow (JIT) в процессе, единственный движок (Tweed, 2026-11-20).
/// Файлы модуля — первый и следующие
/// за ним существующие `.ok`; остальное (после необязательного `--`) —
/// аргументы программы.
fn run_command(args: &[String]) -> Outcome {
    let mut rest = args;
    let mut tests = false;
    let mut filters: Vec<String> = Vec::new();
    let mut format: Option<test_report::ReportFormat> = None;
    let mut seed: Option<u64> = None;
    let mut limits = sparrow_core::budget::Limits::default();
    while let Some(first) = rest.first() {
        match first.as_str() {
            // Служебный ключ процесса-воркера `spawn`: корень std родителя.
            "--stdlib" => {
                let Some(value) = rest.get(1) else {
                    return Outcome::usage("--stdlib needs a directory");
                };
                let _ = STDLIB_OVERRIDE.set(PathBuf::from(value));
                rest = &rest[1..];
            }
            "-t" | "--tests" => tests = true,
            "--filter" => {
                let Some(value) = rest.get(1) else {
                    return Outcome::usage("--filter needs a text to look for in test names");
                };
                filters.push(value.clone());
                rest = &rest[1..];
            }
            "--report" => {
                let Some(value) = rest.get(1) else {
                    return Outcome::usage("--report needs a format: human, json or junit");
                };
                match test_report::ReportFormat::named(value) {
                    Ok(named) => format = Some(named),
                    Err(message) => return Outcome::usage(&message),
                }
                rest = &rest[1..];
            }
            "--seed" => {
                let Some(value) = rest.get(1) else {
                    return Outcome::usage("--seed needs a whole number");
                };
                match test_seed(value) {
                    Ok(value) => seed = Some(value),
                    Err(message) => return Outcome::usage(&message),
                }
                rest = &rest[1..];
            }
            "-h" | "--help" => return Outcome::success(HELP.to_string()),
            "--" => {
                rest = &rest[1..];
                break;
            }
            budget if budget.starts_with("--max-") || budget == "--timeout" => {
                let Some(value) = rest.get(1) else {
                    return Outcome::usage(&format!("{budget} needs a value"));
                };
                match limits.option(budget, value) {
                    Ok(true) => {},
                    Ok(false) => return Outcome::usage(&format!("unknown option `{budget}`")),
                    Err(message) => return Outcome::usage(&message),
                }
                rest = &rest[1..];
            }
            other if other.starts_with('-') && other != "-" => {
                return Outcome::usage(&format!("unknown option `{other}`"));
            }
            _ => break,
        }
        rest = &rest[1..];
    }
    if !filters.is_empty() && !tests {
        return Outcome::usage("--filter selects tests: use it with -t");
    }
    if format.is_some() && !tests {
        return Outcome::usage("--report reports tests: use it with -t");
    }
    if seed.is_some() && !tests {
        return Outcome::usage("--seed orders tests: use it with -t");
    }
    let Some((input, mut program_args)) = rest.split_first() else {
        return Outcome::usage("missing input file");
    };
    let mut line = CommandLine {
        input: Some(input.clone()),
        tests,
        ..CommandLine::default()
    };
    while let Some((next, tail)) = program_args.split_first() {
        if !(next.ends_with(".ok") && Path::new(next).is_file()) {
            break;
        }
        line.more_inputs.push(next.clone());
        program_args = tail;
    }
    if program_args.first().map(String::as_str) == Some("--") {
        program_args = &program_args[1..];
    }
    // A test run that reaches no verdict exits 4, failed tests 1 (#339).
    let unrun = |outcome: Outcome| if tests && outcome.code == EXIT_DIAGNOSTICS {
        Outcome { code: EXIT_TEST_RUN, ..outcome }
    } else {
        outcome
    };
    let sources = match load_all(&line) {
        Ok(sources) => sources,
        Err(outcome) => return unrun(outcome),
    };
    let docs = if tests { doctest::check(&sources, &filters) } else { Outcome::success(String::new()) };
    if docs.code != 0 { return docs; }
    if tests && !docs.stdout.is_empty() {
        let parsed = match parse_raw(&sources) { Ok(parsed) => parsed, Err(outcome) => return outcome };
        if !parsed.iter().any(okkama_compiler::ide::program_has_tests) { return docs; }
    }
    let (mut module, warnings) = match lower_source(&sources, line.tests) {
        Ok(lowered) => lowered,
        Err(outcome) => return unrun(outcome),
    };
    let _ = io::stderr().lock().write_all(warnings.as_bytes());
    let mut plan = test_report::TestPlan { seed, ..Default::default() };
    if let Some(tests) = module.tests.as_mut()
        && !filters.is_empty()
    {
        // `--filter` (#277): the tests whose names contain one of the texts,
        // whole. A text `T/S` (#809) also runs a test whose name contains T,
        // and of its sub-cases only those whose names contain S.
        tests.retain(|(name, _)| {
            if filters.iter().any(|text| name.contains(text.as_str())) {
                return true;
            }
            let parts: Vec<String> = filters
                .iter()
                .filter_map(|text| text.split_once('/'))
                .filter(|(test, _)| name.contains(test))
                .map(|(_, case)| case.to_owned())
                .collect();
            if parts.is_empty() {
                return false;
            }
            plan.only.insert(name.clone(), parts);
            true
        });
        if tests.is_empty() {
            if !docs.stdout.is_empty() { return docs; }
            let texts: Vec<String> = filters.iter().map(|text| format!("{text:?}")).collect();
            // The owner's review of #339: a filter that selects nothing keeps
            // its existing error behaviour, exit 1 with this diagnostic.
            return Outcome::failure(format!("okkama: no test name contains {}\n", texts.join(" or ")), EXIT_DIAGNOSTICS);
        }
    }
    if std::env::var_os("OKKAMA_ICU_LIBDIR").is_none()
        && let Some(icu) = pinned_icu()
    {
        sparrow_core::default_environment("OKKAMA_ICU_LIBDIR", &icu.display().to_string());
    }
    let program = match codegen::sparrow::generate(&module) {
        Ok(program) => program,
        Err(error) => return unrun(Outcome::failure(format!("okkama: {error}\n"), EXIT_DIAGNOSTICS)),
    };
    let bytes = match okkama_sparrow::encode(&program) {
        Ok(bytes) => bytes,
        Err(error) => return unrun(Outcome::failure(format!("okkama: {error}\n"), EXIT_DIAGNOSTICS)),
    };
    if tests {
        let format = format.unwrap_or(test_report::ReportFormat::Human);
        let mut outcome = core_tests(bytes, plan, format, input);
        // Doctest results (#514) follow the human stream; a json or junit
        // stdout stays the report alone (#339), so they go to stderr.
        if format == test_report::ReportFormat::Human {
            outcome.stdout.push_str(&docs.stdout);
        } else {
            outcome.stderr.push_str(&docs.stdout);
        }
        return outcome;
    }
    let (command, _file) = match core::workers_for(&program, &bytes) {
        Ok(workers) => workers,
        Err(message) => return Outcome::failure(format!("okkama: {message}\n"), EXIT_IO),
    };
    let arguments = program_args.iter().map(|a| a.clone().into_bytes()).collect();
    if let Some(flag) = &limits.interrupt {
        watchdog(flag.clone());
    }
    // Номера функций и инструкций ядра совпадают с OIR: место — по исходнику.
    let (outcome, trace) = core_described(sparrow_core::run_described(bytes, arguments, false, command, limits));
    with_trace(outcome, trace, |sites| trace::render_trace(&module, sites))
}

/// `<prefix>/lib/okkama` установки, в которой лежит этот `okkama`
/// (`<prefix>/bin/okkama`); ссылки на исполняемый файл разрешаются.
fn install_root() -> Option<PathBuf> {
    let exe = std::fs::canonicalize(std::env::current_exe().ok()?).ok()?;
    let root = exe.parent()?.parent()?.join("lib").join("okkama");
    root.join("std").is_dir().then_some(root)
}

/// Каталог std: `OKKAMA_STDLIB_DIR`; std установки (`<prefix>/lib/okkama/std`);
/// `std/` репозитория, если `okkama` запущен из его сборки
/// (`okkama-rs/target/release/okkama`); иначе `/usr/local/lib/okkama/std`.
/// Корень std, заданный родителем воркера (`--stdlib`).
static STDLIB_OVERRIDE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

fn stdlib_dir() -> PathBuf {
    if let Some(dir) = STDLIB_OVERRIDE.get() {
        return dir.clone();
    }
    if let Some(dir) = std::env::var_os("OKKAMA_STDLIB_DIR").filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir);
    }
    if let Some(root) = install_root() {
        return root.join("std");
    }
    let exe = std::env::current_exe()
        .ok()
        .and_then(|exe| std::fs::canonicalize(exe).ok());
    if let Some(exe) = exe {
        for ancestor in exe.ancestors().skip(1).take(5) {
            let std = ancestor.join("std");
            if std.join("io").join("stdout.ok").is_file() {
                return std;
            }
        }
    }
    PathBuf::from("/usr/local/lib/okkama/std")
}

const ENV_HELP: &str = "okkama env — where this okkama finds its parts

USAGE:
    okkama env

Prints the version, the executable, the std directory and the pinned ICU
used by locale/collation. Host cryptography is built into Sparrow.
OKKAMA_STDLIB_DIR and
OKKAMA_ICU_LIBDIR override the installed locations.
";

fn env_command(args: &[String]) -> Outcome {
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        return Outcome::success(ENV_HELP.to_string());
    }
    if let Some(other) = args.first() {
        return Outcome::usage(&format!("unknown env argument `{other}`"));
    }
    let platform = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
    let found = |path: Option<PathBuf>| match path {
        Some(path) if path.exists() => path.display().to_string(),
        Some(path) => format!("{} (missing)", path.display()),
        None => "(not found)".to_string(),
    };
    let stdlib = stdlib_dir();
    let icu = std::env::var_os("OKKAMA_ICU_LIBDIR")
        .map(PathBuf::from)
        .or_else(pinned_icu);
    let exe = std::env::current_exe()
        .ok()
        .and_then(|exe| std::fs::canonicalize(exe).ok());
    let text = format!(
        "okkama {}\nplatform  {platform}\nbinary    {}\nstd       {}\nicu       {}\n",
        env!("CARGO_PKG_VERSION"),
        found(exe),
        found(Some(stdlib)),
        found(icu),
    );
    Outcome::success(text)
}

/// Закреплённая ICU 76.1 для `runtime/collation`: рядом с std — в
/// репозитории `.runtime/icu-76.1` (`tools/build-icu-collation.sh`), в
/// установке `<prefix>/lib/okkama/icu-76.1`.
fn pinned_icu() -> Option<PathBuf> {
    let root = std::fs::canonicalize(stdlib_dir()).ok()?;
    let parent = root.parent()?;
    [
        parent.join(".runtime/icu-76.1/lib"),
        parent.join("icu-76.1/lib"),
    ]
    .into_iter()
    .find(|dir| dir.is_dir())
}

/// Предупреждения анализа (`file:line:col: warning: …`).
fn render_warnings(source: &SourceFile, warnings: &[lowering::CheckError]) -> String {
    let mut text = String::new();
    for found in warnings {
        let file = found.source.as_ref().unwrap_or(source);
        let location = file.location(found.error.span.start);
        text.push_str(&format!(
            "{}:{}:{}: warning: {}\n",
            file.name(),
            location.line,
            location.column,
            found.error.message
        ));
    }
    text
}

fn ir_error_outcome(source: &SourceFile, span: Span, kind: &str, message: &str) -> Outcome {
    let location = source.location(span.start);
    Outcome::failure(
        format!(
            "{}:{}:{}: {kind}: {message}\n",
            source.name(),
            location.line,
            location.column,
        ),
        EXIT_DIAGNOSTICS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| arg.to_string()).collect()
    }

    /// A program run outside test mode does not parse test bodies (§14, #677).
    #[test]
    fn program_files_skip_test_bodies_outside_test_mode() {
        let text = "module app v1.0.0;\ntest \"t\" {\n    let = = ;\n}\nmain() is int { 4; }\n";
        let sources = vec![SourceFile::new("main.ok", text).unwrap()];
        let programs = parse_all(&sources, false).unwrap_or_else(|outcome| panic!("{}", outcome.stderr));
        assert_eq!(programs.len(), 1);
        let outcome = parse_all(&sources, true).expect_err("test mode parses the test body");
        assert!(outcome.stderr.contains("syntax error"), "{}", outcome.stderr);
    }

    #[test]
    fn options_accept_separate_and_inline_values() {
        let line = parse_command_line(
            &args(&["x.ok", "--format", "json", "--mode=production"]),
            CommandOptions::Ast,
        )
        .unwrap();
        assert_eq!(line.input.as_deref(), Some("x.ok"));
        assert_eq!(line.format.as_deref(), Some("json"));
        assert_eq!(line.mode.as_deref(), Some("production"));
    }

    #[test]
    fn ast_only_options_are_rejected_for_tokens() {
        assert!(parse_command_line(&args(&["x.ok", "--spans"]), CommandOptions::Basic).is_err());
        assert!(parse_command_line(&args(&["x.ok", "--spans"]), CommandOptions::Ast).is_ok());
    }

    #[test]
    fn missing_values_and_extra_inputs_are_usage_errors() {
        assert!(parse_command_line(&args(&["x.ok", "--format"]), CommandOptions::Ast).is_err());
        assert!(parse_command_line(&args(&["a.ok", "b.ok"]), CommandOptions::Ast).is_err());
        assert!(parse_command_line(&args(&["--frobnicate"]), CommandOptions::Ast).is_err());
    }

    #[test]
    fn unknown_command_is_a_usage_error() {
        let outcome = run(&args(&["frobnicate", "x.ok"]));
        assert_eq!(outcome.code, EXIT_USAGE);
        assert!(outcome.stdout.is_empty());
    }
}
