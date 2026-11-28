//! The server against frames written by hand, as a port would write them.

use super::*;

/// Output shared with the test after the server is done with it.
#[derive(Debug, Clone, Default)]
struct Sink(Arc<Mutex<Vec<u8>>>);

impl Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn framed(bodies: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for body in bodies {
        frame::write(&mut out, body).unwrap();
    }
    out
}

fn json(text: &str) -> Vec<u8> {
    let mut body = vec![FRAME_JSON];
    body.extend(text.as_bytes());
    body
}

fn hello() -> Vec<u8> {
    json(r#"{"kind":"hello","protocol":1}"#)
}

/// The frames the server wrote, each decoded: a message as its value, an attachment as `Value::Bytes`.
fn written(sink: &Sink) -> Vec<Value> {
    let bytes = sink.0.lock().unwrap().clone();
    let mut input = &bytes[..];
    let mut frames = Vec::new();
    while let Some(body) = frame::read(&mut input, DEFAULT_FRAME_LIMIT).unwrap() {
        frames.push(match body[0] {
            FRAME_JSON => Codec::Json.decode(&body[1..]).unwrap(),
            FRAME_CBOR => Codec::Cbor.decode(&body[1..]).unwrap(),
            _ => Value::Bytes(body[1..].to_vec()),
        });
    }
    frames
}

fn echo(call: Call) -> Result<Reply, Failure> {
    match call.method() {
        "echo" => Ok(Reply::value(call.payload().clone()).with_bytes(call.bytes())),
        "fail" => Err(Failure::new("bad", "no good")),
        "huge" => Ok(Reply::value(Value::Int(1 << 70))),
        other => Err(Failure::unknown_method(other)),
    }
}

fn serve(server: Server, input: Vec<u8>) -> (Result<(), Error>, Vec<Value>) {
    let sink = Sink::default();
    let outcome = server.serve_on(&input[..], sink.clone(), echo);
    (outcome, written(&sink))
}

#[test]
fn the_hello_names_the_protocol_and_the_codecs() {
    let (outcome, frames) = serve(Server::new(), framed(&[hello()]));
    outcome.unwrap();
    assert_eq!(frames, vec![Value::map([
        ("kind", Value::from("hello")),
        ("protocol", Value::from(1)),
        ("codecs", Value::Array(vec![Value::from("json"), Value::from("cbor")])),
    ])]);
    let (_, frames) = serve(Server::new().json_only(), framed(&[hello()]));
    assert_eq!(frames[0].get("codecs"), Some(&Value::Array(vec![Value::from("json")])));
}

#[test]
fn requests_are_answered_in_their_codec_with_their_bytes() {
    let cbor_request = {
        let mut body = vec![FRAME_CBOR];
        body.extend(cbor::encode(&Value::map([
            ("kind", Value::from("request")),
            ("id", Value::from(2)),
            ("method", Value::from("echo")),
            ("payload", Value::Bytes(vec![0, 255])),
        ])));
        body
    };
    let input = framed(&[
        hello(),
        json(r#"{"kind":"request","id":1,"method":"echo","payload":{"n":7},"attachments":2}"#),
        vec![FRAME_ATTACHMENT, 1, 2],
        vec![FRAME_ATTACHMENT, 3],
        cbor_request,
        json(r#"{"kind":"request","id":3,"method":"fail"}"#),
        json(r#"{"kind":"cancel","id":3}"#),
        json(r#"{"kind":"request","id":4,"method":"nope","attachments":0}"#),
        json(r#"{"kind":"request","id":5,"method":"huge"}"#),
    ]);
    let (outcome, frames) = serve(Server::new(), input);
    outcome.unwrap();
    assert_eq!(&frames[1..], &[
        Value::map([("kind", Value::from("reply")), ("id", Value::from(1)), ("payload", Value::map([("n", Value::from(7))])), ("attachments", Value::from(1))]),
        Value::Bytes(vec![1, 2, 3]),
        Value::map([("kind", Value::from("reply")), ("id", Value::from(2)), ("payload", Value::Bytes(vec![0, 255])), ("attachments", Value::from(1))]),
        Value::Bytes(vec![]),
        Value::map([("kind", Value::from("failure")), ("id", Value::from(3)), ("code", Value::from("bad")), ("message", Value::from("no good"))]),
        Value::map([("kind", Value::from("failure")), ("id", Value::from(4)), ("code", Value::from("unknown_method")), ("message", Value::from("no method \"nope\""))]),
        Value::map([("kind", Value::from("failure")), ("id", Value::from(5)), ("code", Value::from("invalid_reply")), ("message", Value::from("integer 1180591620717411303424 has no JSON form"))]),
    ]);
}

#[test]
fn threads_answer_every_call() {
    let mut bodies = vec![hello()];
    for id in 1..=50 {
        bodies.push(json(&format!(r#"{{"kind":"request","id":{id},"method":"echo","payload":{id}}}"#)));
    }
    let (outcome, frames) = serve(Server::new().threads(4), framed(&bodies));
    outcome.unwrap();
    let mut ids: Vec<i64> = frames[1..].iter().filter(|frame| !matches!(frame, Value::Bytes(_))).map(|frame| {
        assert_eq!(frame.get("payload"), frame.get("id"));
        frame.get("id").and_then(Value::as_i64).unwrap()
    }).collect();
    ids.sort();
    assert_eq!(ids, (1..=50).collect::<Vec<_>>());
}

#[test]
fn a_port_of_another_version_or_a_broken_port_stops_the_helper() {
    let incompatible = framed(&[json(r#"{"kind":"hello","protocol":2}"#)]);
    assert!(matches!(serve(Server::new(), incompatible).0, Err(Error::Incompatible(2))));
    let malformed = |bodies: &[Vec<u8>]| match serve(Server::new(), framed(bodies)).0 {
        Err(Error::Malformed(reason)) => reason,
        other => panic!("{other:?}"),
    };
    assert!(malformed(&[]).contains("before the hello"));
    assert!(malformed(&[json(r#"{"kind":"request","id":1,"method":"echo"}"#)]).contains("not a hello"));
    assert!(malformed(&[hello(), json("{nope")]).contains("does not parse"));
    assert!(malformed(&[hello(), vec![b'X']]).contains("unknown type 88"));
    assert!(malformed(&[hello(), vec![FRAME_ATTACHMENT]]).contains("attachment without"));
    assert!(malformed(&[hello(), json(r#"{"kind":"request","id":1,"method":"echo","attachments":1}"#), hello()]).contains("attachment was due"));
    assert!(malformed(&[hello(), json(r#"{"kind":"request","method":"echo"}"#)]).contains("positive id"));
    assert!(malformed(&[hello(), json(r#"{"kind":"request","id":1}"#)]).contains("without a method"));
    assert!(malformed(&[hello(), hello()]).contains("second hello"));
    assert!(malformed(&[hello(), json("[1]")]).contains("not a map"));
    let big = framed(&[hello(), json(&format!(r#"{{"kind":"request","id":1,"method":"{}"}}"#, "x".repeat(64)))]);
    assert!(matches!(serve(Server::new().frame_limit(40), big).0, Err(Error::Malformed(reason)) if reason.contains("over the limit")));
}

#[test]
fn typed_payloads_decode_and_encode() {
    #[derive(serde::Deserialize, serde::Serialize, Debug, PartialEq)]
    struct Resize {
        width: u32,
        name: String,
    }
    let call = Call {
        id: 1,
        method: "resize".into(),
        payload: Value::map([("width", Value::from(200)), ("name", Value::from("cat"))]),
        attachments: vec![vec![1], vec![2, 3]],
        codec: Codec::Cbor,
    };
    let request: Resize = call.decode().unwrap();
    assert_eq!(request, Resize { width: 200, name: "cat".into() });
    assert_eq!(call.bytes(), vec![1, 2, 3]);
    let wrong = Call { payload: Value::from("x"), ..call.clone() };
    assert_eq!(wrong.decode::<Resize>().unwrap_err().code, "invalid_payload");
    let reply = Reply::json(&request).unwrap();
    assert_eq!(reply.payload, Some(call.payload.clone()));
}
