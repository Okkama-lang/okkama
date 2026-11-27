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
    assert!(refused_natives(b"not an spb", &PLAYGROUND).is_err());
}
