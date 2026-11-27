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
use sparrow_core::budget::{Limits, PLAYGROUND};
use sparrow_core::stdout::Stream;
use sparrow_core::{Embedding, Piece};

/// Run `spb` as the playground does: request `{stdin?: text, arguments?: [text]}`.
pub fn run(spb: &[u8], request: &str) -> String {
    begin();
    let request: Value = match serde_json::from_str(request) {
        Ok(request) => request,
        Err(error) => return refused(&format!("the request is not JSON: {error}")),
    };
    let stdin = request.get("stdin").and_then(Value::as_str).map(|text| text.as_bytes().to_vec());
    let arguments = request
        .get("arguments")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_str).map(|text| text.as_bytes().to_vec()).collect())
        .unwrap_or_default();
    let embedding = Embedding { arguments, stdin: Some(stdin.unwrap_or_default()), limits: Limits::playground() };
    let run = match sparrow_core::run_embedded(spb.to_vec(), embedding) {
        Ok(run) => run,
        // A program that does not load, with the prefix `okkama run` gives it.
        Err(error) => {
            let (exit, report) = main_outcome(Err(&format!("okkama: {}", error.0)));
            return json!({"exit": exit, "output": [], "report": report, "uncaught": null}).to_string();
        }
    };
    let (result, uncaught) = match &run.result {
        Ok(value) => (Ok(*value), None),
        Err((report, uncaught)) => (Err(report.as_str()), uncaught.as_ref()),
    };
    let (exit, report) = main_outcome(result);
    json!({
        "exit": exit,
        "output": pieces(&run.output),
        "report": if report.is_empty() { Value::Null } else { json!(report) },
        "uncaught": uncaught.map(uncaught_json),
    })
    .to_string()
}

/// What `spb` needs: `{refused: [symbol], files: [name]}`. `refused` are the natives the playground lacks: the page
/// warns before a run, and the site marks the examples that run. `files` are the data files to [`provide`] first:
/// the CLDR tables (0.9 MB compressed) stay out of the module and come only to a program that reads CLDR (dates,
/// numbers, currencies, locales; which family it reads shows only when it runs, so all five).
pub fn requirements(spb: &[u8]) -> String {
    begin();
    let (natives, refused_natives) = match (sparrow_core::program_natives(spb), sparrow_core::refused_natives(spb, &PLAYGROUND)) {
        (Ok(natives), Ok(refused)) => (natives, refused),
        (Err(error), _) | (_, Err(error)) => return refused(&format!("okkama: {}", error.0)),
    };
    let cldr = natives.iter().any(|native| native.symbol().starts_with("runtime/cldr@"));
    let files: Vec<&str> =
        if cldr { okkama_runtime_core::native::CLDR_FILES.iter().map(|(_, file)| *file).collect() } else { Vec::new() };
    json!({"refused": refused_natives, "files": files}).to_string()
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

/// The runtime's version and the profile: `{version, profile: {name, groups, steps, memory, output}}`.
pub fn version() -> String {
    begin();
    let groups: Vec<String> = PLAYGROUND.groups.iter().map(|group| format!("{group:?}")).collect();
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "profile": {
            "name": PLAYGROUND.name, "groups": groups, "steps": PLAYGROUND.steps, "memory": PLAYGROUND.memory,
            "output": PLAYGROUND.output,
        },
    })
    .to_string()
}

fn refused(message: &str) -> String {
    json!({"error": message}).to_string()
}

/// The writes as text: a character split between two writes to one stream goes whole with the later one, so every
/// piece is valid UTF-8 (bytes that are not UTF-8 become U+FFFD, as the CLI's terminal would show them).
fn pieces(output: &[Piece]) -> Vec<Value> {
    let mut pending: [Vec<u8>; 2] = [Vec::new(), Vec::new()];
    let mut out = Vec::new();
    for piece in output {
        let index = usize::from(piece.stream == Stream::Error);
        let mut bytes = std::mem::take(&mut pending[index]);
        bytes.extend_from_slice(&piece.bytes);
        let keep = match std::str::from_utf8(&bytes) {
            Err(error) if error.error_len().is_none() => bytes.len() - error.valid_up_to(),
            _ => 0,
        };
        pending[index] = bytes.split_off(bytes.len() - keep);
        if !bytes.is_empty() {
            out.push(piece_json(piece.stream, &bytes, piece.nanos));
        }
    }
    let last = output.last().map_or(0, |piece| piece.nanos);
    for (index, bytes) in pending.iter().enumerate() {
        if !bytes.is_empty() {
            out.push(piece_json(if index == 0 { Stream::Output } else { Stream::Error }, bytes, last));
        }
    }
    out
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
    pub fn run(spb: &[u8], request: &str) -> String {
        super::run(spb, request)
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

    /// A program whose main runs `body` (registers r0: bytes, r1: int, r2: text).
    fn spb(body: &str) -> Vec<u8> {
        spb_with(NO_CLDR, body)
    }

    fn spb_with(f3: &str, body: &str) -> Vec<u8> {
        let source = format!("{HEAD}{f3}function f4 \"main\" {{\n    registers [t3, t0, t1]\n    params []\n    result t0\n{body}}}\nentry f4\n");
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
        assert_eq!(needs, json!({"refused": ["runtime/system@1::system_host_name/0"], "files": []}));
        let cldr = spb_with(CLDR, "    0: const r2, str \"numbers.digits\"\n    1: call _, f3, [r2, r2]\n    2: const r1, int 0\n    3: ret r1\n");
        let needs: Value = serde_json::from_str(&requirements(&cldr)).unwrap();
        // A declared native counts, called or not (f2 is declared in every program here).
        assert_eq!(needs["refused"], json!(["runtime/system@1::system_host_name/0"]));
        assert_eq!(needs["files"].as_array().map(Vec::len), Some(5));
        assert!(needs["files"].as_array().unwrap().iter().all(|file| file.as_str().unwrap().starts_with("cldr-")));
        let given: Value = serde_json::from_str(&provide("cldr-nowhere.dat", b"")).unwrap();
        assert_eq!(given["error"], "cldr-nowhere.dat is not a CLDR file");
    }

}
