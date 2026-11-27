//! Sparrow for the playground (#1125, design `okkama-docs/runtime/sparrow-interpreter-1079.md` §5, §8 item 6):
//! the interpreter and the pure providers (sparrow-core without `jit` and `host`, #1124) run the SPB that
//! okkama-wasm built, under the playground's profile ([`sparrow_core::budget::PLAYGROUND`]). It is a module of its
//! own, beside the compiler's (owner's decision 7 of §9, awaiting the owner): a page that only checks code never
//! loads it.
//!
//! Every call returns one JSON text. [`run`] takes the SPB and a request `{stdin?, arguments?}` and answers
//! `{exit, output: [{stream, text, t}], report, uncaught}`: the program's writes in order, each with its moment `t`
//! in nanoseconds on the run's timeline (on wasm32 the virtual one of `sparrow_core::clock`: a sleep moves it
//! without waiting, so the page shows the output by its marks), the exit status by the CLI's rule
//! ([`okkama_runtime_core::failure::main_outcome`]), and for a failure the report and the uncaught failure, whose
//! source stack okkama-wasm's `failureText` draws with the module it keeps. Natively the functions are plain Rust,
//! which is how `okkama-cli/tests/playground_run.rs` compares them with the CLI.
//!
//! A panic is a trap on wasm32, which carries no message: the panic hook keeps the last one for [`last_panic`], as
//! okkama-wasm's does (#1118).

use std::cell::RefCell;

use okkama_runtime_core::failure::{Site, Uncaught, main_outcome};
use serde_json::{Value, json};
use sparrow_core::budget::{Exhausted, Limits, PLAYGROUND};
use sparrow_core::stdout::Stream;
use sparrow_core::{Embedding, Piece, ProgramNatives};

/// Run `spb` as the playground does: request `{stdin?: text, arguments?: [text]}`; the output comes back in the
/// answer's `output` (a stream's writes at one moment joined).
pub fn run(spb: &[u8], request: &str) -> String {
    collected(|emit| run_streaming(spb, request, emit))
}

/// Run the tests of `spb` (built with tests) as `okkama run -t` does, on the playground's interpreter (#1127):
/// request `{stdin?: text, seed?: int}`; the human report and the tests' output in `output`.
pub fn test(spb: &[u8], request: &str) -> String {
    collected(|emit| test_streaming(spb, request, emit))
}

/// The answer of `body` with the chunks it emitted in its `output`.
fn collected(body: impl FnOnce(Box<dyn FnMut(Value) + Send>) -> Value) -> String {
    let chunks = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
    let into = chunks.clone();
    let mut answer = body(Box::new(move |chunk| {
        collect(&mut into.lock().unwrap_or_else(std::sync::PoisonError::into_inner), chunk);
    }));
    if let Some(output) = answer.get_mut("output") {
        *output = json!(std::mem::take(&mut *chunks.lock().unwrap_or_else(std::sync::PoisonError::into_inner)));
    }
    answer.to_string()
}

/// Add `chunk` to the collected output: joined with the last chunk when both are of one stream at one moment.
fn collect(chunks: &mut Vec<Value>, chunk: Value) {
    match chunks.last_mut() {
        Some(last) if last["stream"] == chunk["stream"] && last["t"] == chunk["t"] => {
            let text = format!("{}{}", last["text"].as_str().unwrap_or_default(), chunk["text"].as_str().unwrap_or_default());
            last["text"] = json!(text);
        }
        _ => chunks.push(chunk),
    }
}

/// [`run`], handing each write to `emit` as a chunk `{stream, text, t}` while the program runs (#1134): the page
/// shows output as it comes and keeps what a stopped run printed. A stream's writes within the request's `window`
/// are one chunk (`batching`, #1136; `window` 0 is one chunk per write); a character split between two writes of
/// a stream goes whole with the later one; the answer's `output` is then empty. Output is bounded by the profile's
/// budget (1 MiB), so are the chunks.
pub fn run_streaming(spb: &[u8], request: &str, emit: impl FnMut(Value) + Send + 'static) -> Value {
    begin();
    let request: Value = match serde_json::from_str(request) {
        Ok(request) => request,
        Err(error) => return json!({"error": format!("the request is not JSON: {error}")}),
    };
    let arguments = request
        .get("arguments")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_str).map(|text| text.as_bytes().to_vec()).collect())
        .unwrap_or_default();
    let run = streamed(emit, |pieces| {
        let embedding = Embedding {
            arguments,
            stdin: Some(stdin(&request)),
            limits: Limits::playground(),
            pieces: Some(pieces),
            batching: batching(&request),
        };
        sparrow_core::run_embedded(spb.to_vec(), embedding)
    });
    let run = match run {
        Ok(run) => run,
        // A program that does not load, with the prefix `okkama run` gives it.
        Err(error) => {
            let (exit, report) = main_outcome(Err(&format!("okkama: {}", error.0)));
            return json!({"exit": exit, "output": [], "report": report, "uncaught": null});
        }
    };
    let (result, uncaught) = match &run.result {
        Ok(value) => (Ok(*value), None),
        Err((report, uncaught)) => (Err(report.as_str()), uncaught.as_ref()),
    };
    let (exit, report) = main_outcome(result);
    json!({
        "exit": exit,
        "output": [],
        "report": if report.is_empty() { Value::Null } else { json!(report) },
        "uncaught": uncaught.map(uncaught_json),
    })
}

/// [`test`], handing each write (the report's lines and the tests' output) to `emit` as it happens: `{exit, output:
/// [], report}`. `exit` is `okkama run -t`'s: 0 when every test passed, 1 when one failed, 4 with `report` (`okkama:
/// …`) when the tests could not run (an SPB without tests, one that does not load).
pub fn test_streaming(spb: &[u8], request: &str, emit: impl FnMut(Value) + Send + 'static) -> Value {
    begin();
    let request: Value = match serde_json::from_str(request) {
        Ok(request) => request,
        Err(error) => return json!({"error": format!("the request is not JSON: {error}")}),
    };
    let seed = match request.get("seed") {
        None | Some(Value::Null) => None,
        Some(seed) => match seed.as_u64().filter(|seed| *seed <= sparrow_core::test_report::MAX_SEED) {
            Some(seed) => Some(seed),
            None => {
                let message = format!("`seed` must be a whole number from 0 to {}", sparrow_core::test_report::MAX_SEED);
                return json!({"error": message});
            }
        },
    };
    let plan = sparrow_core::test_report::TestPlan { seed, ..Default::default() };
    let ran = streamed(emit, |pieces| {
        let embedding = Embedding {
            stdin: Some(stdin(&request)),
            limits: Limits::playground(),
            pieces: Some(pieces),
            batching: batching(&request),
            ..Embedding::default()
        };
        sparrow_core::run_tests_embedded(spb.to_vec(), embedding, plan)
    });
    match ran {
        Ok(ran) => json!({"exit": u8::from(!ran.passed), "output": [], "report": null}),
        Err(error) => json!({"exit": 4, "output": [], "report": format!("okkama: {}\n", error.0)}),
    }
}

/// How a streamed run's writes reach the sink (#1136): the request's `window`, in milliseconds of the run's timeline;
/// a stream's writes within it of the first are one chunk, handed over when the next write does not join it, at the
/// run's next budget poll and at the end. 0 is a chunk per write (#1134). Default [`WINDOW_MS`].
fn batching(request: &Value) -> sparrow_core::Batching {
    let window = request.get("window").and_then(Value::as_f64).filter(|ms| ms.is_finite() && *ms >= 0.0).unwrap_or(WINDOW_MS);
    sparrow_core::Batching::Window((window * 1e6) as u64)
}

/// What the page writes when the user stops a run (#1136), apart from its watchdog's `interrupted`; the wording
/// awaits the owner.
pub const STOPPED: &str = "Execution stopped\n";

/// The default window of a streamed run's chunks, in milliseconds (#1136; measured in decisions.md).
pub const WINDOW_MS: f64 = 16.0;

/// The request's standard input; empty without one (the playground never reads the process's).
fn stdin(request: &Value) -> Vec<u8> {
    request.get("stdin").and_then(Value::as_str).map(|text| text.as_bytes().to_vec()).unwrap_or_default()
}

/// Run `body` with a piece receiver that turns each write into a chunk for `emit`, and hand over what is left of
/// an unfinished character when it ends.
fn streamed<T>(emit: impl FnMut(Value) + Send + 'static, body: impl FnOnce(sparrow_core::PieceSink) -> T) -> T {
    let decoder = std::sync::Arc::new(std::sync::Mutex::new((Decoder::default(), emit)));
    let streaming = decoder.clone();
    let pieces: sparrow_core::PieceSink = Box::new(move |piece| {
        let mut guard = streaming.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let (decoder, emit) = &mut *guard;
        if let Some(chunk) = decoder.push(&piece) {
            emit(chunk);
        }
    });
    let result = body(pieces);
    let mut guard = decoder.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let (decoder, emit) = &mut *guard;
    decoder.finish().into_iter().for_each(emit);
    result
}

/// What `spb` needs: `{refused: [symbol], declared: [symbol], files: [name]}`. `refused` are the natives the
/// playground lacks that a run can reach from `main`, or from the tests of a test build (#1135: over the call graph,
/// `sparrow_core::reach`): the page warns before a run, and the site marks the examples that run. `declared` are
/// all it lacks of what the program and its imports declare. `files` are the data files to [`provide`] first: the
/// CLDR tables (0.9 MB compressed) stay out of the module and come only to a program that can read CLDR (dates,
/// numbers, currencies, locales; which family it reads shows only when it runs, so all five).
pub fn requirements(spb: &[u8]) -> String {
    begin();
    let natives = match sparrow_core::program_natives(spb) {
        Ok(natives) => natives,
        Err(error) => return refused(&format!("okkama: {}", error.0)),
    };
    let cldr = natives.reachable.iter().any(|native| native.symbol().starts_with("runtime/cldr@"));
    let files: Vec<&str> =
        if cldr { okkama_runtime_core::native::CLDR_FILES.iter().map(|(_, file)| *file).collect() } else { Vec::new() };
    let reachable = ProgramNatives::refused(&natives.reachable, &PLAYGROUND);
    let declared = ProgramNatives::refused(&natives.declared, &PLAYGROUND);
    json!({"refused": reachable, "declared": declared, "files": files}).to_string()
}

/// Give the data file `name` (one of `requirements`' `files`, as the site publishes it from
/// `okkama-runtime-core/data`) before a run: `{ok: true}` or `{error}`.
pub fn provide(name: &str, bytes: &[u8]) -> String {
    begin();
    match okkama_runtime_core::native::provide_cldr(name, bytes) {
        Ok(()) => json!({"ok": true}).to_string(),
        Err(message) => refused(&message),
    }
}

/// The runtime's version and the profile: `{version, profile: {name, groups, steps, memory, output, interrupted,
/// stopped, exhausted: {steps, memory, output}}}`. The texts are lines as `okkama run` prints them: `interrupted` is
/// what the page writes when its watchdog stops a run (`okkama run --timeout`'s, #1134), `stopped` when the user does
/// (#1136; no CLI counterpart, wording awaiting the owner), `exhausted` what a run that ran out of a budget ends with
/// (the run's own report carries it too).
pub fn version() -> String {
    begin();
    let groups: Vec<String> = PLAYGROUND.groups.iter().map(|group| format!("{group:?}")).collect();
    let line = |what: Exhausted| format!("{}\n", what.message());
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "profile": {
            "name": PLAYGROUND.name, "groups": groups, "steps": PLAYGROUND.steps, "memory": PLAYGROUND.memory,
            "output": PLAYGROUND.output, "interrupted": line(Exhausted::Interrupted), "stopped": STOPPED,
            "exhausted": {"steps": line(Exhausted::Steps), "memory": line(Exhausted::Memory), "output": line(Exhausted::Output)},
        },
    })
    .to_string()
}

fn refused(message: &str) -> String {
    json!({"error": message}).to_string()
}

/// The writes as text: a character split between two writes to one stream goes whole with the later one, so every
/// chunk is valid UTF-8 (bytes that are not UTF-8 become U+FFFD, as the CLI's terminal would show them).
#[derive(Debug, Default)]
struct Decoder {
    /// The unfinished character at the end of each stream (stdout, stderr).
    pending: [Vec<u8>; 2],
    last: u64,
}

impl Decoder {
    fn push(&mut self, piece: &Piece) -> Option<Value> {
        self.last = piece.nanos;
        let index = usize::from(piece.stream == Stream::Error);
        let mut bytes = std::mem::take(&mut self.pending[index]);
        bytes.extend_from_slice(&piece.bytes);
        let keep = match std::str::from_utf8(&bytes) {
            Err(error) if error.error_len().is_none() => bytes.len() - error.valid_up_to(),
            _ => 0,
        };
        self.pending[index] = bytes.split_off(bytes.len() - keep);
        (!bytes.is_empty()).then(|| piece_json(piece.stream, &bytes, piece.nanos))
    }

    /// What is left when the run ends: an unfinished character, as U+FFFD.
    fn finish(&mut self) -> Vec<Value> {
        let last = self.last;
        let streams = [Stream::Output, Stream::Error];
        streams
            .into_iter()
            .zip(self.pending.iter_mut())
            .filter(|(_, bytes)| !bytes.is_empty())
            .map(|(stream, bytes)| piece_json(stream, &std::mem::take(bytes), last))
            .collect()
    }
}

fn piece_json(stream: Stream, bytes: &[u8], nanos: u64) -> Value {
    let stream = match stream {
        Stream::Output => "stdout",
        Stream::Error => "stderr",
    };
    json!({"stream": stream, "text": String::from_utf8_lossy(bytes), "t": nanos})
}

/// `{category, errorType, message, trace, traceOmitted, traceOffset, causeTraces}`: a stack is a list of runs
/// `[function, pc, count]` of equal consecutive sites (a deep recursion is one run), and a cause's is `[offset,
/// runs, omitted]`. okkama-wasm reads it back.
fn uncaught_json(uncaught: &Uncaught) -> Value {
    let causes: Vec<Value> = uncaught
        .cause_traces
        .iter()
        .map(|(offset, trace, omitted)| json!([offset, runs(trace), omitted]))
        .collect();
    json!({
        "category": uncaught.category, "errorType": uncaught.error_type, "message": uncaught.message,
        "trace": runs(&uncaught.trace), "traceOmitted": uncaught.trace_omitted, "traceOffset": uncaught.trace_offset,
        "causeTraces": causes,
    })
}

fn runs(trace: &[Site]) -> Vec<[u64; 3]> {
    let mut runs: Vec<[u64; 3]> = Vec::new();
    for site in trace {
        match runs.last_mut() {
            Some([function, pc, count]) if *function == u64::from(site.function) && *pc == u64::from(site.pc) => *count += 1,
            _ => runs.push([u64::from(site.function), u64::from(site.pc), 1]),
        }
    }
    runs
}

thread_local! {
    /// The message and location of the last call's panic.
    static LAST_PANIC: RefCell<Option<(String, Option<String>)>> = const { RefCell::new(None) };
}

/// Every call starts here: the panic hook and (wasm32) the page's clocks, once; the last panic forgotten.
fn begin() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let payload = info.payload();
            let message = payload
                .downcast_ref::<&str>()
                .map(|text| text.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "a panic without a message".to_string());
            let location = info.location().map(|at| format!("{}:{}:{}", at.file(), at.line(), at.column()));
            let _ = LAST_PANIC.try_with(|last| {
                if let Ok(mut last) = last.try_borrow_mut() {
                    *last = Some((message, location));
                }
            });
            previous(info);
        }));
        #[cfg(target_arch = "wasm32")]
        bindings::install_clocks();
    });
    LAST_PANIC.with(|last| *last.borrow_mut() = None);
}

/// The last call's panic: `{panic: {message, location} | null}`.
pub fn last_panic() -> String {
    let panic = LAST_PANIC.with(|last| {
        last.borrow().as_ref().map(|(message, location)| json!({"message": message, "location": location}))
    });
    json!({"panic": panic}).to_string()
}

/// The exports to JavaScript and the page's clocks.
#[cfg(target_arch = "wasm32")]
mod bindings {
    use wasm_bindgen::prelude::wasm_bindgen;

    #[wasm_bindgen]
    extern "C" {
        /// Milliseconds, fractional, from the page's origin (a worker's and Node's `performance` too).
        #[wasm_bindgen(js_namespace = performance, js_name = now)]
        fn performance_now() -> f64;
        /// Milliseconds since the Unix epoch.
        #[wasm_bindgen(js_namespace = Date, js_name = now)]
        fn date_now() -> f64;
    }

    fn monotonic() -> u64 {
        (performance_now() * 1e6) as u64
    }

    fn wall() -> u64 {
        (date_now() * 1e6) as u64
    }

    /// The real clocks (owner's decision 9 of §9, awaiting the owner); the scheduler's skips go on top of them.
    pub(super) fn install_clocks() {
        sparrow_core::clock::install(sparrow_core::clock::Source { monotonic, wall });
    }

    #[wasm_bindgen]
    extern "C" {
        /// The page's output sink (#1134): a function called with one chunk's JSON text, `{stream, text, t}`.
        pub type OutputSink;
        /// A sink that throws loses that chunk; the run goes on.
        #[wasm_bindgen(method, js_name = call, catch)]
        fn call_with(this: &OutputSink, receiver: &wasm_bindgen::JsValue, chunk: &str) -> Result<(), wasm_bindgen::JsValue>;
    }

    thread_local! {
        /// The sink of the run in progress; the run is one synchronous call on this thread.
        static SINK: std::cell::RefCell<Option<OutputSink>> = const { std::cell::RefCell::new(None) };
    }

    /// With `sink`, every write goes to it while the program runs and the answer's `output` is empty; without it
    /// the output comes back in the answer.
    #[wasm_bindgen]
    pub fn run(spb: &[u8], request: &str, sink: Option<OutputSink>) -> String {
        let Some(sink) = sink else { return super::run(spb, request) };
        with_sink(sink, |emit| super::run_streaming(spb, request, emit))
    }

    /// `body` with an `emit` that hands each chunk's JSON text to `sink`, kept in [`SINK`] for the call.
    fn with_sink(sink: OutputSink, body: impl FnOnce(fn(serde_json::Value)) -> serde_json::Value) -> String {
        fn emit(chunk: serde_json::Value) {
            let text = chunk.to_string();
            SINK.with(|current| {
                if let Some(sink) = current.borrow().as_ref() {
                    let _ = sink.call_with(&wasm_bindgen::JsValue::NULL, &text);
                }
            });
        }
        let previous = SINK.with(|current| current.replace(Some(sink)));
        let answer = body(emit);
        SINK.with(|current| *current.borrow_mut() = previous);
        answer.to_string()
    }

    /// The tests of an SPB built with tests (#1127), as `run`: with `sink` the report streams to it.
    #[wasm_bindgen]
    pub fn test(spb: &[u8], request: &str, sink: Option<OutputSink>) -> String {
        let Some(sink) = sink else { return super::test(spb, request) };
        with_sink(sink, |emit| super::test_streaming(spb, request, emit))
    }

    #[wasm_bindgen]
    pub fn requirements(spb: &[u8]) -> String {
        super::requirements(spb)
    }

    #[wasm_bindgen]
    pub fn provide(name: &str, bytes: &[u8]) -> String {
        super::provide(name, bytes)
    }

    #[wasm_bindgen]
    pub fn version() -> String {
        super::version()
    }

    #[wasm_bindgen(js_name = lastPanic)]
    pub fn last_panic() -> String {
        super::last_panic()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn piece(stream: Stream, bytes: &[u8], nanos: u64) -> Piece {
        Piece { stream, bytes: bytes.to_vec(), nanos }
    }

    fn pieces(output: &[Piece]) -> Vec<Value> {
        let mut decoder = Decoder::default();
        let mut chunks: Vec<Value> = output.iter().filter_map(|piece| decoder.push(piece)).collect();
        chunks.extend(decoder.finish());
        chunks
    }

    /// A character split between writes goes whole with the later write of its stream; the other stream between
    /// them is not held back; an unfinished character at the end comes out as U+FFFD.
    #[test]
    fn pieces_are_whole_characters() {
        let output = [
            piece(Stream::Output, b"a\xd0", 1),
            piece(Stream::Error, b"e", 2),
            piece(Stream::Output, b"\x9f!", 3),
            piece(Stream::Error, b"\xe2\x82", 4),
        ];
        let texts: Vec<(String, String, u64)> = pieces(&output)
            .iter()
            .map(|piece| (piece["stream"].as_str().unwrap().into(), piece["text"].as_str().unwrap().into(), piece["t"].as_u64().unwrap()))
            .collect();
        let expected = vec![
            ("stdout".to_string(), "a".to_string(), 1),
            ("stderr".to_string(), "e".to_string(), 2),
            ("stdout".to_string(), "П!".to_string(), 3),
            ("stderr".to_string(), "\u{fffd}".to_string(), 4),
        ];
        assert_eq!(texts, expected);
        let invalid = pieces(&[piece(Stream::Output, b"x\xffy", 5)]);
        assert_eq!(invalid[0]["text"], "x\u{fffd}y");
    }

    #[test]
    fn equal_consecutive_sites_are_one_run() {
        let site = |function, pc| Site { function, pc };
        let trace = [site(1, 2), site(1, 2), site(1, 2), site(3, 4), site(1, 2)];
        assert_eq!(runs(&trace), vec![[1, 2, 3], [3, 4, 1], [1, 2, 1]]);
        assert!(runs(&[]).is_empty());
    }

    #[test]
    fn a_request_that_is_not_json_or_a_program_that_does_not_load_is_refused() {
        let response: Value = serde_json::from_str(&run(b"", "{")).unwrap();
        assert!(response["error"].as_str().unwrap().starts_with("the request is not JSON"));
        let response: Value = serde_json::from_str(&run(b"not an spb", "{}")).unwrap();
        assert_eq!(response["exit"], 1);
        assert!(response["report"].as_str().unwrap().starts_with("okkama: "));
        let response: Value = serde_json::from_str(&requirements(b"not an spb")).unwrap();
        assert!(response["error"].as_str().unwrap().starts_with("okkama: "));
        let response: Value = serde_json::from_str(&version()).unwrap();
        assert_eq!(response["profile"]["name"], "playground");
        assert_eq!(response["profile"]["steps"], 50_000_000);
        assert_eq!(response["profile"]["interrupted"], "Execution interrupted\n");
        assert_eq!(response["profile"]["stopped"], "Execution stopped\n");
        assert_eq!(response["profile"]["exhausted"]["steps"], "Execution budget exhausted: steps\n");
        assert_eq!(last_panic(), r#"{"panic":null}"#);
    }

    const HEAD: &str = r#"sparrow 18
module "playground" "1.0.0"
type t0 int
type t1 str
type t2 byte
type t3 array t2
type t4 array t1
function f0 "write" native "runtime/stdio@1::stdout_write/1" {
    registers [t3]
    params [r0]
    result t0
}
function f1 "line" native "runtime/stdio@1::stdin_read_line/0" {
    registers []
    params []
    result t3
}
function f2 "host" native "runtime/system@1::system_host_name/0" {
    registers []
    params []
    result t1
}
"#;

    /// f3: the CLDR native, or a function of the same shape without one.
    const CLDR: &str = "function f3 \"cldr\" native \"runtime/cldr@1::cldr_fields/2\" {\n    registers [t1, t1]\n    params [r0, r1]\n    result t4\n}\n";
    const NO_CLDR: &str = "function f3 \"plain\" {\n    registers [t1, t1, t4]\n    params [r0, r1]\n    result t4\n    0: str.split r2, r0, r1\n    1: ret r2\n}\n";

    /// A program whose main runs `body` (registers r0: bytes, r1: int, r2: text, r3: int).
    fn spb(body: &str) -> Vec<u8> {
        spb_with(NO_CLDR, body)
    }

    fn spb_with(f3: &str, body: &str) -> Vec<u8> {
        let source = format!("{HEAD}{f3}function f4 \"main\" {{\n    registers [t3, t0, t1, t0]\n    params []\n    result t0\n{body}}}\nentry f4\n");
        let program = okkama_sparrow::assemble(&source).unwrap_or_else(|error| panic!("{error}\n{source}"));
        okkama_sparrow::encode(&program).unwrap()
    }

    fn ran(spb: &[u8], request: &str) -> Value {
        serde_json::from_str(&run(spb, request)).unwrap()
    }

    /// The exit status follows the CLI's rule: 0..255 is the status without a report, anything else is status 1
    /// with the CLI's text; standard input is the request's, then its end.
    #[test]
    fn the_exit_status_and_the_report_follow_the_cli() {
        let seven = ran(&spb("    0: call r0, f1, []\n    1: call _, f0, [r0]\n    2: const r1, int 7\n    3: ret r1\n"), r#"{"stdin": "in\nrest"}"#);
        assert_eq!((&seven["exit"], &seven["report"], &seven["uncaught"]), (&json!(7), &Value::Null, &Value::Null));
        assert_eq!(seven["output"][0]["text"], "in\n");
        assert_eq!(seven["output"][0]["stream"], "stdout");
        let wide = ran(&spb("    0: const r1, int 300\n    1: ret r1\n"), "{}");
        assert_eq!(wide["exit"], 1);
        assert_eq!(wide["report"], "main() must return an int in 0..255, got 300\n");
        assert_eq!(wide["uncaught"], Value::Null);
        assert_eq!(wide["output"], json!([]));
    }

    /// A native the profile lacks fails the program with the profile's text and a described failure, and
    /// `requirements` names it before the run; a program that reads CLDR is told the files to provide.
    #[test]
    fn a_refused_native_is_named_before_and_fails_the_run() {
        let host = spb("    0: call r2, f2, []\n    1: const r1, int 0\n    2: ret r1\n");
        let failed = ran(&host, "{}");
        assert_eq!(failed["exit"], 1);
        let report = failed["report"].as_str().unwrap();
        assert!(report.starts_with("RuntimeError: Native runtime/system@1::system_host_name/0 is not available in the playground"), "{report}");
        assert!(report.ends_with('\n'));
        let uncaught = &failed["uncaught"];
        assert_eq!(uncaught["category"], "runtime");
        assert_eq!(uncaught["trace"], json!([[4, 0, 1]]));
        assert_eq!(uncaught["causeTraces"], json!([]));
        let needs: Value = serde_json::from_str(&requirements(&host)).unwrap();
        let host_name = json!(["runtime/system@1::system_host_name/0"]);
        assert_eq!(needs, json!({"refused": host_name, "declared": host_name, "files": []}));
        let cldr = spb_with(CLDR, "    0: const r2, str \"numbers.digits\"\n    1: call _, f3, [r2, r2]\n    2: const r1, int 0\n    3: ret r1\n");
        let needs: Value = serde_json::from_str(&requirements(&cldr)).unwrap();
        // #1135: only what a run can reach is refused (f2 is declared in every program here, called in `host`).
        assert_eq!((&needs["refused"], &needs["declared"]), (&json!([]), &host_name));
        assert_eq!(needs["files"].as_array().map(Vec::len), Some(5));
        assert!(needs["files"].as_array().unwrap().iter().all(|file| file.as_str().unwrap().starts_with("cldr-")));
        let unread = spb_with(CLDR, "    0: const r1, int 0\n    1: ret r1\n");
        let needs: Value = serde_json::from_str(&requirements(&unread)).unwrap();
        assert_eq!(needs["files"], json!([]), "CLDR declared, never read");
        let given: Value = serde_json::from_str(&provide("cldr-nowhere.dat", b"")).unwrap();
        assert_eq!(given["error"], "cldr-nowhere.dat is not a CLDR file");
    }


    /// #1134: streaming hands every write to the sink as it happens (window 0), in order, before the run's answer,
    /// whose `output` is then empty; collected, the same writes come back in `output`.
    #[test]
    fn a_streamed_run_hands_over_each_write_as_it_happens() {
        let echo = spb("    0: call r0, f1, []\n    1: call _, f0, [r0]\n    2: call r0, f1, []\n    3: call _, f0, [r0]\n    4: const r1, int 0\n    5: ret r1\n");
        let chunks = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let into = chunks.clone();
        let answer = run_streaming(&echo, r#"{"stdin": "a\nb\n", "window": 0}"#, move |chunk| into.lock().unwrap().push(chunk));
        assert_eq!((&answer["exit"], &answer["output"]), (&json!(0), &json!([])));
        let texts: Vec<String> = chunks.lock().unwrap().iter().map(|chunk| chunk["text"].as_str().unwrap().to_string()).collect();
        assert_eq!(texts, ["a\n", "b\n"], "window 0: a chunk per write");
        // #1136: by default the writes of a moment's window are one chunk.
        let batched = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let into = batched.clone();
        run_streaming(&echo, r#"{"stdin": "a\nb\n"}"#, move |chunk| into.lock().unwrap().push(chunk));
        let texts: Vec<String> = batched.lock().unwrap().iter().map(|chunk| chunk["text"].as_str().unwrap().to_string()).collect();
        assert_eq!(texts, ["a\nb\n"]);
        assert!(chunks.lock().unwrap().iter().all(|chunk| chunk["stream"] == "stdout" && chunk["t"].is_u64()));
        let collected = ran(&echo, r#"{"stdin": "a\nb\n"}"#);
        let joined: String = collected["output"].as_array().unwrap().iter().map(|chunk| chunk["text"].as_str().unwrap()).collect();
        assert_eq!(joined, "a\nb\n");
    }


    /// Collected, writes of one stream at one moment are one chunk; another moment or stream starts a new one.
    #[test]
    fn collected_chunks_join_one_stream_at_one_moment() {
        let chunk = |stream: &str, text: &str, t: u64| json!({"stream": stream, "text": text, "t": t});
        let mut chunks = Vec::new();
        for (stream, text, t) in [("stdout", "a", 1), ("stdout", "b", 1), ("stdout", "c", 2), ("stderr", "d", 2), ("stdout", "e", 2)] {
            collect(&mut chunks, chunk(stream, text, t));
        }
        assert_eq!(chunks, vec![chunk("stdout", "ab", 1), chunk("stdout", "c", 2), chunk("stderr", "d", 2), chunk("stdout", "e", 2)]);
    }

    /// A run that ends inside a character still hands over its last bytes, as U+FFFD, streamed or not.
    #[test]
    fn an_unfinished_character_at_the_end_comes_out() {
        let half = spb("    0: const r2, str \"aЯ\"\n    1: str.utf8 r0, r2\n    2: const r1, int 0\n    3: const r3, int 2\n    4: arr.slice r0, r0, r1, r3\n    5: call _, f0, [r0]\n    6: ret r1\n");
        let chunks = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let into = chunks.clone();
        run_streaming(&half, "{}", move |chunk| into.lock().unwrap().push(chunk));
        let texts: Vec<String> = chunks.lock().unwrap().iter().map(|chunk| chunk["text"].as_str().unwrap().to_string()).collect();
        assert_eq!(texts, ["a", "\u{fffd}"]);
        let collected = ran(&half, "{}");
        let joined: String = collected["output"].as_array().unwrap().iter().map(|chunk| chunk["text"].as_str().unwrap()).collect();
        assert_eq!(joined, "a\u{fffd}");
    }


    /// #1127: a test build's tests run as `okkama run -t` runs them: the human report and the tests' output streamed
    /// in order, a line at a time, each test with the request's standard input, exit 1 when one failed (here on a
    /// native the profile refuses), 4 without tests.
    #[test]
    fn the_tests_of_a_test_build_report_as_okkama_run_t() {
        let body = "    0: const r1, int 0\n    1: ret r1\n";
        let tests = "function f5 \"writes\" {\n    registers [t1, t3]\n    params []\n    result void\n    0: const r0, str \"hi\\n\"\n    1: str.utf8 r1, r0\n    2: call _, f0, [r1]\n    3: ret _\n}\n\
                     function f6 \"refused\" {\n    registers [t1]\n    params []\n    result void\n    0: call r0, f2, []\n    1: ret _\n}\n\
                     function f7 \"reads\" {\n    registers [t3]\n    params []\n    result void\n    0: call r0, f1, []\n    1: call _, f0, [r0]\n    2: ret _\n}\n";
        let source = format!("{HEAD}{NO_CLDR}function f4 \"main\" {{\n    registers [t3, t0, t1, t0]\n    params []\n    result t0\n{body}}}\n{tests}entry f4\ntests {{\n    test \"writes\" f5\n    test \"refused\" f6\n    test \"reads\" f7\n}}\n");
        let program = okkama_sparrow::assemble(&source).unwrap_or_else(|error| panic!("{error}\n{source}"));
        let spb = okkama_sparrow::encode(&program).unwrap();
        let chunks = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let into = chunks.clone();
        let answer = test_streaming(&spb, r#"{"stdin": "in\n"}"#, move |chunk| into.lock().unwrap().push(chunk));
        assert_eq!((&answer["exit"], &answer["output"], &answer["report"]), (&json!(1), &json!([]), &Value::Null));
        let text: String = chunks.lock().unwrap().iter().map(|chunk| chunk["text"].as_str().unwrap().to_string()).collect();
        assert!(text.starts_with("=== RUN   writes\nhi\n--- PASS: writes\n=== RUN   refused\n"), "{text}");
        assert!(text.contains("is not available in the playground"), "{text}");
        // Each test reads the request's standard input afresh.
        assert!(text.contains("=== RUN   reads\nin\n--- PASS: reads\n"), "{text}");
        assert!(text.ends_with("FAIL\n"), "{text}");
        let collected: Value = serde_json::from_str(&test(&spb, r#"{"seed": 1, "window": 0}"#)).unwrap();
        assert_eq!(collected["output"][0]["text"], "=== SEED  1\n", "a line at a time: {collected}");
        let bad: Value = serde_json::from_str(&test(&spb, r#"{"seed": -1}"#)).unwrap();
        assert!(bad["error"].as_str().unwrap().starts_with("`seed` must be"));
        let plain: Value = serde_json::from_str(&test(&spb_with(NO_CLDR, body), "{}")).unwrap();
        assert_eq!((&plain["exit"], &plain["report"]), (&json!(4), &json!("okkama: module has no tests\n")));
    }

}
