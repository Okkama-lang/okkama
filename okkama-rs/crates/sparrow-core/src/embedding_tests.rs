//! The embedded run of the playground (#1125): standard input from a buffer, both streams in order with their
//! moments, the profile's refusal as a program failure, and the natives a program needs that a profile refuses.
use crate::budget::{Limits, PLAYGROUND};
use crate::stdout::Stream;
use crate::{Embedding, Piece, refused_natives, run_embedded};

const HEAD: &str = r#"sparrow 18
module "embedding" "1.0.0"
type t0 int
type t1 str
type t2 byte
type t3 array t2
function f0 "write" native "runtime/stdio@1::stdout_write/1" {
    registers [t3]
    params [r0]
    result t0
}
function f1 "error" native "runtime/stdio@1::stderr_write/1" {
    registers [t3]
    params [r0]
    result t0
}
function f2 "line" native "runtime/stdio@1::stdin_read_line/0" {
    registers []
    params []
    result t3
}
function f3 "host" native "runtime/system@1::system_host_name/0" {
    registers []
    params []
    result t1
}
"#;

fn spb(main: &str) -> Vec<u8> {
    let source = format!("{HEAD}{main}");
    let program = okkama_sparrow::assemble(&source).unwrap_or_else(|error| panic!("{error}\n{source}"));
    okkama_sparrow::encode(&program).unwrap()
}

/// Echo the first line to stdout, the second to stderr, then what is left (end of input: nothing), and return 7.
const ECHO: &str = r#"function f4 "main" {
    registers [t3, t0, t1]
    params []
    result t0
    0: call r0, f2, []
    1: call _, f0, [r0]
    2: call r0, f2, []
    3: call _, f1, [r0]
    4: call r0, f2, []
    5: call _, f0, [r0]
    6: const r1, int 7
    7: ret r1
}
entry f4
"#;

const HOST: &str = r#"function f4 "main" {
    registers [t3, t0, t1]
    params []
    result t0
    0: const r2, str "before\n"
    1: str.utf8 r0, r2
    2: call _, f0, [r0]
    3: call r2, f3, []
    4: const r1, int 0
    5: ret r1
}
entry f4
"#;

fn text(output: &[Piece]) -> Vec<(Stream, String)> {
    output.iter().map(|piece| (piece.stream, String::from_utf8_lossy(&piece.bytes).into_owned())).collect()
}

#[test]
fn standard_input_is_the_buffer_then_its_end() {
    let embedding = Embedding { stdin: Some(b"one\ntwo\n".to_vec()), limits: Limits::playground(), ..Embedding::default() };
    let run = run_embedded(spb(ECHO), embedding).unwrap();
    assert_eq!(run.result, Ok(7));
    let pieces = text(&run.output);
    let streams: Vec<Stream> = pieces.iter().map(|(stream, _)| *stream).collect();
    let joined: String = pieces.iter().map(|(_, text)| text.as_str()).collect();
    assert_eq!(joined, "one\ntwo\n");
    assert_eq!(streams.first(), Some(&Stream::Output));
    assert_eq!(streams.last(), Some(&Stream::Error));
    assert!(run.output.windows(2).all(|pair| pair[0].nanos <= pair[1].nanos));
    // An empty buffer is end of input at once.
    let empty = Embedding { stdin: Some(Vec::new()), ..Embedding::default() };
    let run = run_embedded(spb(ECHO), empty).unwrap();
    assert_eq!((run.result, run.output), (Ok(7), Vec::new()));
}

#[test]
fn the_profile_refusal_is_a_program_failure() {
    let run = run_embedded(spb(HOST), Embedding { limits: Limits::playground(), ..Embedding::default() }).unwrap();
    assert_eq!(text(&run.output), vec![(Stream::Output, "before\n".to_string())]);
    let (report, uncaught) = run.result.unwrap_err();
    assert!(report.contains("Native runtime/system@1::system_host_name/0 is not available in the playground"), "{report}");
    assert_eq!(uncaught.expect("described").category, "runtime");
}

#[test]
fn refused_natives_names_what_the_profile_lacks() {
    assert_eq!(refused_natives(&spb(HOST), &PLAYGROUND).unwrap(), vec!["runtime/system@1::system_host_name/0"]);
    // #1135: ECHO declares the host native too but never calls it.
    assert_eq!(refused_natives(&spb(ECHO), &PLAYGROUND).unwrap(), Vec::<&str>::new());
    let natives = crate::program_natives(&spb(ECHO)).unwrap();
    assert!(natives.declared.contains(&okkama_runtime_abi::Native::SystemSystemHostName));
    assert!(!natives.reachable.contains(&okkama_runtime_abi::Native::SystemSystemHostName));
    assert!(refused_natives(b"not an spb", &PLAYGROUND).is_err());
}

/// Write "a", count to 1000 (a back-edge each), write "b".
const COUNT: &str = r#"function f4 "main" {
    registers [t3, t0, t1, t0, t0, t4]
    params []
    result t0
    0: const r2, str "a"
    1: str.utf8 r0, r2
    2: call _, f0, [r0]
    3: const r1, int 0
    4: const r3, int 1
    5: const r4, int 1000
    6: int.add r1, r1, r3
    7: int.cmp lt, r5, r1, r4
    8: br r5, @6, @9
    9: const r2, str "b"
    10: str.utf8 r0, r2
    11: call _, f0, [r0]
    12: const r1, int 0
    13: ret r1
}
entry f4
"#;

fn batched(slice: u64, batching: crate::Batching) -> Vec<String> {
    let pieces = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let into = pieces.clone();
    let limits = Limits { steps: Some(1 << 30), slice: Some(slice), ..Limits::default() };
    let embedding = Embedding { limits, pieces: Some(Box::new(move |piece: Piece| into.lock().unwrap().push(piece))), batching, ..Embedding::default() };
    let source = HEAD.replace("type t3 array t2\n", "type t3 array t2\ntype t4 bool\n");
    let program = okkama_sparrow::assemble(&format!("{source}{COUNT}")).unwrap();
    let run = run_embedded(okkama_sparrow::encode(&program).unwrap(), embedding).unwrap();
    assert_eq!((run.result, run.output), (Ok(0), Vec::new()));
    let pieces = pieces.lock().unwrap();
    pieces.iter().map(|piece| String::from_utf8(piece.bytes.clone()).unwrap()).collect()
}

/// #1136: a window joins a stream's writes until a budget poll hands them over: with polls every 100 steps the
/// 1000 back-edges between "a" and "b" poll, so "a" goes alone; with polls past the loop both are one piece; a chunk
/// per write keeps them apart whatever the polls.
#[test]
fn a_budget_poll_hands_over_the_writes_a_window_joined() {
    let wide = crate::Batching::Window(u64::MAX);
    assert_eq!(batched(100, wide), ["a", "b"]);
    assert_eq!(batched(1 << 20, wide), ["ab"]);
    assert_eq!(batched(1 << 20, crate::Batching::Write), ["a", "b"]);
    assert_eq!(batched(1 << 20, crate::Batching::Window(0)), ["a", "b"], "a window of 0 joins nothing");
}

fn streamed_pieces(main: &str) -> Vec<(Stream, usize)> {
    let pieces = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let into = pieces.clone();
    let embedding = Embedding {
        pieces: Some(Box::new(move |piece: Piece| into.lock().unwrap().push(piece))),
        batching: crate::Batching::Window(u64::MAX),
        ..Embedding::default()
    };
    assert_eq!(run_embedded(spb(main), embedding).unwrap().result, Ok(0));
    let pieces = pieces.lock().unwrap();
    pieces.iter().map(|piece| (piece.stream, piece.bytes.len())).collect()
}

/// #1136: a window joins only one stream's writes, and at most `BATCH_BYTES` of them.
#[test]
fn a_window_joins_one_stream_and_a_bounded_size() {
    let streams = r#"function f4 "main" {
    registers [t3, t0, t1]
    params []
    result t0
    0: const r2, str "ab"
    1: str.utf8 r0, r2
    2: call _, f0, [r0]
    3: call _, f0, [r0]
    4: call _, f1, [r0]
    5: call _, f0, [r0]
    6: const r1, int 0
    7: ret r1
}
entry f4
"#;
    assert_eq!(streamed_pieces(streams), [(Stream::Output, 4), (Stream::Error, 2), (Stream::Output, 2)]);
    let half = crate::BATCH_BYTES / 2 + 1;
    let large = format!(
        "function f4 \"main\" {{\n    registers [t3, t0, t1]\n    params []\n    result t0\n    0: const r2, str \"{}\"\n    1: str.utf8 r0, r2\n    2: call _, f0, [r0]\n    3: call _, f0, [r0]\n    4: const r1, int 0\n    5: ret r1\n}}\nentry f4\n",
        "x".repeat(half)
    );
    assert_eq!(streamed_pieces(&large), [(Stream::Output, half), (Stream::Output, half)]);
}

/// Workers (#1127): `twice` prints "worker <n>" and doubles it; `fails` fails; `unboxed` takes an object.
const WORKERS: &str = r#"type t4 fn [t0] -> t0
type t5 future t0
type t6 object obj0
type t7 fn [t6] -> t0
object obj0 "Box" { field "n" t0 }
function f4 "twice" {
    registers [t0, t0, t1, t1, t1, t3]
    params [r0]
    result t0
    0: int.add r1, r0, r0
    1: cvt r2, r0
    2: const r3, str "worker "
    3: str.concat r4, r3, r2
    4: const r3, str "\n"
    5: str.concat r4, r4, r3
    6: str.utf8 r5, r4
    7: call _, f0, [r5]
    8: ret r1
}
function f5 "fails" {
    registers [t0, t1]
    params [r0]
    result t0
    0: const r1, str "worker gave up"
    1: panic r1
}
function f6 "unboxed" {
    registers [t6, t0]
    params [r0]
    result t0
    0: obj.get r1, r0, 0
    1: ret r1
}
function f7 "spins" {
    registers [t0]
    params [r0]
    result t0
    0: jmp @0
}
"#;

/// `main` of a worker program: registers r0 int, r1 r2 actions, r3 future, r4 int, r5 text, r6 bytes, r7 box,
/// r8 box action.
fn workers(body: &str) -> Vec<u8> {
    let source = format!(
        "{HEAD}{WORKERS}function f8 \"main\" async {{\n    registers [t0, t4, t4, t5, t0, t1, t3, t6, t7]\n    params []\n    result t0\n{body}}}\nentry f8\n"
    );
    let program = okkama_sparrow::assemble(&source).unwrap_or_else(|error| panic!("{error}\n{source}"));
    okkama_sparrow::encode(&program).unwrap()
}

fn emulated(body: &str) -> (Result<i64, (String, Option<crate::Uncaught>)>, String) {
    emulated_under(body, Limits::playground())
}

fn emulated_under(body: &str, limits: Limits) -> (Result<i64, (String, Option<crate::Uncaught>)>, String) {
    let run = run_embedded(workers(body), Embedding { limits, ..Embedding::default() }).unwrap();
    let text = run.output.iter().map(|piece| String::from_utf8_lossy(&piece.bytes).into_owned()).collect();
    (run.result, text)
}

/// #1127: a worker runs at its spawn on a runtime of its own; its output reaches the parent whole when the parent
/// takes its result, before the result; its failure comes back as text; an object does not cross.
#[test]
fn workers_are_emulated_in_the_same_instance() {
    let spawn_twice = "    0: const r0, int 20\n    1: fn.closure r1, f4, []\n    2: worker.bind r2, r1, r1, []\n    3: worker.spawn r3, r2, [r0], false\n";
    let (result, text) = emulated(&format!(
        "{spawn_twice}    4: const r5, str \"parent\\n\"\n    5: str.utf8 r6, r5\n    6: call _, f0, [r6]\n    7: await r4, r3\n    8: cvt r5, r4\n    9: const r0, int 0\n    10: ret r4\n"
    ));
    assert_eq!(result, Ok(40));
    assert_eq!(text, "parent\nworker 20\n");
    let (result, text) = emulated("    0: const r0, int 1\n    1: fn.closure r1, f5, []\n    2: worker.bind r2, r1, r1, []\n    3: worker.spawn r3, r2, [r0], false\n    4: await r4, r3\n    5: ret r4\n");
    let (report, _) = result.unwrap_err();
    assert!(report.contains("worker gave up"), "{report}");
    assert_eq!(text, "");
    let (result, _) = emulated("    0: const r0, int 1\n    1: obj.new r7, obj0, [r0]\n    2: fn.closure r8, f6, []\n    3: worker.spawn r3, r8, [r7], true\n    4: await r4, r3\n    5: ret r4\n");
    let (report, _) = result.unwrap_err();
    assert!(report.contains("spawn cannot transfer objects"), "{report}");
    // The worker's budgets are the parent's: one that spins runs out of its steps, and the parent's await fails
    // with that text.
    let few = Limits { steps: Some(10_000), ..Limits::playground() };
    let (result, _) = emulated_under("    0: const r0, int 1\n    1: fn.closure r1, f7, []\n    2: worker.spawn r3, r1, [r0], true\n    3: await r4, r3\n    4: ret r4\n", few);
    let (report, _) = result.unwrap_err();
    assert!(report.contains("Execution budget exhausted: steps"), "{report}");
}

/// #1127: without the host one processor runs everything, emulated workers too.
#[cfg(not(feature = "host"))]
#[test]
fn one_processor_without_the_host() {
    let source = "sparrow 18\nmodule \"cpus\" \"1.0.0\"\ntype t0 int\nfunction f0 \"cpus\" native \"runtime/system@1::system_cpu_count/0\" {\n    registers []\n    params []\n    result t0\n}\nfunction f1 \"main\" {\n    registers [t0]\n    params []\n    result t0\n    0: call r0, f0, []\n    1: ret r0\n}\nentry f1\n";
    let bytes = okkama_sparrow::encode(&okkama_sparrow::assemble(source).unwrap()).unwrap();
    let run = run_embedded(bytes, Embedding { limits: Limits::playground(), ..Embedding::default() }).unwrap();
    assert_eq!(run.result, Ok(1));
}
