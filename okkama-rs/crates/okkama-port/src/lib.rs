//! The helper side of Okkama ports (#1081): a program that serves calls from an Okkama program over its standard
//! input and output, as `docs/port-protocol.md` (protocol 1) describes. The Okkama side is std's `os/port`:
//!
//! ```okkama
//! let text = Port(command: InheritedCommand(program: Path("bin/text-helper")));
//! let reply = await text.call("upper", Json(JsonField("text", "hello")));
//! ```
//!
//! A helper is a `main` that hands every call to one function:
//!
//! ```no_run
//! use okkama_port::{Call, Failure, Reply};
//!
//! #[derive(serde::Deserialize)]
//! struct Upper { text: String }
//!
//! fn main() {
//!     okkama_port::serve(|call: Call| match call.method() {
//!         "upper" => {
//!             let request: Upper = call.decode()?;
//!             Reply::json(&serde_json::json!({ "text": request.text.to_uppercase() }))
//!         }
//!         "reverse" => Ok(Reply::bytes(call.bytes().iter().rev().copied().collect())),
//!         other => Err(Failure::unknown_method(other)),
//!     });
//! }
//! ```
//!
//! - A call's payload arrives as a [`Value`], whichever codec (JSON or CBOR) the caller chose, and the reply goes
//!   back in that codec. [`Call::decode`] reads it into a serde type; [`Reply::json`] writes one.
//! - Raw bytes travel beside the payload in frames of their own: [`Call::bytes`], [`Reply::bytes`].
//! - A `Failure` becomes the caller's `PortFailure.refused(method, code, message)`; the helper keeps serving.
//! - [`serve`] answers one call at a time. [`Server::threads`] answers several at once, each on a thread of a
//!   fixed pool, and writes each reply whole as it is ready.
//! - The helper ends when its input ends (the port closed it or the program ended). It may write logs on its
//!   standard error: the port keeps the last of it for the failure of a crashed helper.

mod cbor;
mod frame;
mod value;

pub use frame::{FRAME_ATTACHMENT, FRAME_CBOR, FRAME_JSON};
pub use value::{Value, ValueError};

use std::fmt;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::sync::{mpsc, Arc, Mutex};

/// The protocol version this crate speaks; the port refuses a helper of another.
pub const PROTOCOL: i64 = 1;

/// The largest frame a helper accepts unless told otherwise: 16 MiB, as the port's own default.
pub const DEFAULT_FRAME_LIMIT: usize = 16 << 20;

/// The most attachments one message may carry.
const ATTACHMENTS: usize = 1024;

/// The codec of a message: JSON text or CBOR. A reply goes back in the codec of its request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    /// UTF-8 JSON, frame type `J`.
    Json,
    /// CBOR, frame type `C`.
    Cbor,
}

impl Codec {
    fn frame_type(self) -> u8 {
        match self {
            Codec::Json => FRAME_JSON,
            Codec::Cbor => FRAME_CBOR,
        }
    }

    fn encode(self, message: &Value) -> Result<Vec<u8>, ValueError> {
        let mut body = vec![self.frame_type()];
        match self {
            Codec::Json => body.extend(message.json_text()?),
            Codec::Cbor => body.extend(cbor::encode(message)),
        }
        Ok(body)
    }

    fn decode(self, body: &[u8]) -> Result<Value, String> {
        match self {
            Codec::Json => {
                Value::parse_json(body).map_err(|error| format!("a JSON message that does not parse: {error}"))
            }
            Codec::Cbor => cbor::decode(body).map_err(|reason| format!("a CBOR message that does not decode: {reason}")),
        }
    }
}

/// One call from the Okkama program: its method, its payload and the raw bytes attached to it.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    id: i64,
    method: String,
    payload: Value,
    attachments: Vec<Vec<u8>>,
    codec: Codec,
}

impl Call {
    /// The method the caller named.
    pub fn method(&self) -> &str {
        &self.method
    }

    /// The payload; `Value::Null` when the caller sent none.
    pub fn payload(&self) -> &Value {
        &self.payload
    }

    /// The payload read into a serde type; a payload of another shape is `Failure` `invalid_payload`.
    pub fn decode<T: serde::de::DeserializeOwned>(&self) -> Result<T, Failure> {
        let json = self.payload.to_json().map_err(|error| Failure::new("invalid_payload", error.to_string()))?;
        serde_json::from_value(json).map_err(|error| Failure::new("invalid_payload", error.to_string()))
    }

    /// The raw bytes attached to the call, joined; empty when none.
    pub fn bytes(&self) -> Vec<u8> {
        self.attachments.concat()
    }

    /// Each attachment as it came.
    pub fn attachments(&self) -> &[Vec<u8>] {
        &self.attachments
    }

    /// The codec the call came in, which its reply uses.
    pub fn codec(&self) -> Codec {
        self.codec
    }
}

/// A helper's answer: a payload and raw bytes.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Reply {
    payload: Option<Value>,
    attachments: Vec<Vec<u8>>,
}

impl Reply {
    /// A reply carrying a payload.
    pub fn value(payload: impl Into<Value>) -> Reply {
        Reply { payload: Some(payload.into()), attachments: Vec::new() }
    }

    /// A reply carrying a serde value as its payload.
    pub fn json<T: serde::Serialize + ?Sized>(payload: &T) -> Result<Reply, Failure> {
        // Through text, so a struct's fields keep their order.
        let text = serde_json::to_vec(payload).map_err(|error| Failure::new("invalid_reply", error.to_string()))?;
        Ok(Reply::value(Value::parse_json(&text).map_err(|error| Failure::new("invalid_reply", error.to_string()))?))
    }

    /// A reply carrying raw bytes and no payload.
    pub fn bytes(bytes: Vec<u8>) -> Reply {
        Reply { payload: None, attachments: vec![bytes] }
    }

    /// The same reply with raw bytes attached after any it has.
    pub fn with_bytes(mut self, bytes: Vec<u8>) -> Reply {
        self.attachments.push(bytes);
        self
    }
}

/// A call the helper refuses: the caller's `PortFailure.refused(method, code, message)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    code: String,
    message: String,
}

impl Failure {
    /// A failure with a short code the caller can match ("invalid_payload", "not_found") and a message for people.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Failure {
        Failure { code: code.into(), message: message.into() }
    }

    /// The failure for a method this helper does not have.
    pub fn unknown_method(method: &str) -> Failure {
        Failure::new("unknown_method", format!("no method {method:?}"))
    }
}

impl From<String> for Failure {
    fn from(message: String) -> Failure {
        Failure::new("failed", message)
    }
}

impl From<&str> for Failure {
    fn from(message: &str) -> Failure {
        Failure::new("failed", message)
    }
}

impl From<io::Error> for Failure {
    fn from(error: io::Error) -> Failure {
        Failure::new("io", error.to_string())
    }
}

impl From<serde_json::Error> for Failure {
    fn from(error: serde_json::Error) -> Failure {
        Failure::new("invalid_payload", error.to_string())
    }
}

/// Why serving stopped before the input ended.
#[derive(Debug)]
pub enum Error {
    /// Reading the input or writing the output failed.
    Io(io::Error),
    /// The port sent something the protocol does not allow.
    Malformed(String),
    /// The port speaks another protocol version.
    Incompatible(i64),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(error) => write!(f, "okkama-port: {error}"),
            Error::Malformed(reason) => write!(f, "okkama-port: the port broke the protocol: {reason}"),
            Error::Incompatible(protocol) => write!(f, "okkama-port: the port speaks protocol {protocol}, this helper {PROTOCOL}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Error {
        Error::Io(error)
    }
}

/// Serves calls on standard input and output, one at a time, until the input ends; then the process exits 0.
/// When the port breaks the protocol or speaks another version, the reason goes to standard error and the
/// process exits 2.
pub fn serve<F>(handler: F)
where
    F: Fn(Call) -> Result<Reply, Failure> + Send + Sync + 'static,
{
    Server::new().serve(handler)
}

/// How a helper serves: its frame limit, its threads and the codecs it accepts.
#[derive(Debug, Clone)]
pub struct Server {
    frame_limit: usize,
    threads: usize,
    cbor: bool,
}

impl Default for Server {
    fn default() -> Server {
        Server::new()
    }
}

impl Server {
    /// One call at a time, frames up to 16 MiB, JSON and CBOR.
    pub fn new() -> Server {
        Server { frame_limit: DEFAULT_FRAME_LIMIT, threads: 1, cbor: true }
    }

    /// Frames from the port of at most `limit` bytes; a longer one is a protocol error.
    pub fn frame_limit(mut self, limit: usize) -> Server {
        self.frame_limit = limit.max(1);
        self
    }

    /// Up to `count` calls answered at once, each on a thread of a fixed pool.
    pub fn threads(mut self, count: usize) -> Server {
        self.threads = count.max(1);
        self
    }

    /// Accept JSON calls only: the hello leaves CBOR out, and the port refuses CBOR calls before sending them.
    pub fn json_only(mut self) -> Server {
        self.cbor = false;
        self
    }

    /// Serves on standard input and output as [`serve`] does.
    pub fn serve<F>(self, handler: F)
    where
        F: Fn(Call) -> Result<Reply, Failure> + Send + Sync + 'static,
    {
        let input = io::stdin().lock();
        let output = io::stdout();
        match self.serve_on(input, output, handler) {
            Ok(()) => std::process::exit(0),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(2);
            }
        }
    }

    /// Serves on any reader and writer until the reader ends; for tests and for helpers with other streams.
    pub fn serve_on<R, W, F>(self, input: R, output: W, handler: F) -> Result<(), Error>
    where
        R: Read,
        W: Write + Send + 'static,
        F: Fn(Call) -> Result<Reply, Failure> + Send + Sync + 'static,
    {
        let mut input = BufReader::new(input);
        let output = Arc::new(Mutex::new(BufWriter::new(output)));
        let mut codecs = vec![Value::from("json")];
        if self.cbor {
            codecs.push(Value::from("cbor"));
        }
        let hello = Value::map([("kind", Value::from("hello")), ("protocol", Value::from(PROTOCOL)), ("codecs", Value::Array(codecs))]);
        write_frames(&output, &[Codec::Json.encode(&hello).expect("the hello is JSON")])?;
        self.handshake(&mut input)?;
        let handler = Arc::new(handler);
        if self.threads == 1 {
            while let Some(call) = self.next_call(&mut input)? {
                answer(&output, handler.as_ref(), call)?;
            }
            return Ok(());
        }
        let (sender, receiver) = mpsc::channel::<Call>();
        let receiver = Arc::new(Mutex::new(receiver));
        let failed: Arc<Mutex<Option<io::Error>>> = Arc::new(Mutex::new(None));
        let workers: Vec<_> = (0..self.threads)
            .map(|_| {
                let (receiver, output, handler, failed) = (receiver.clone(), output.clone(), handler.clone(), failed.clone());
                std::thread::spawn(move || loop {
                    let next = receiver.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).recv();
                    let Ok(call) = next else { break };
                    if let Err(error) = answer(&output, handler.as_ref(), call) {
                        failed.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get_or_insert(error);
                    }
                })
            })
            .collect();
        let read: Result<(), Error> = (|| {
            while let Some(call) = self.next_call(&mut input)? {
                if sender.send(call).is_err() {
                    break;
                }
            }
            Ok(())
        })();
        drop(sender);
        for worker in workers {
            let _ = worker.join();
        }
        read?;
        match failed.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take() {
            Some(error) => Err(Error::Io(error)),
            None => Ok(()),
        }
    }

    fn handshake(&self, input: &mut impl Read) -> Result<(), Error> {
        let frame = frame::read(input, self.frame_limit)?.ok_or_else(|| Error::Malformed("the input ended before the hello".into()))?;
        if frame.first() != Some(&FRAME_JSON) {
            return Err(Error::Malformed("the first frame is not a JSON hello".into()));
        }
        let hello = Codec::Json.decode(&frame[1..]).map_err(Error::Malformed)?;
        if hello.get("kind").and_then(Value::as_str) != Some("hello") {
            return Err(Error::Malformed("the first message is not a hello".into()));
        }
        match hello.get("protocol").and_then(Value::as_i64) {
            Some(PROTOCOL) => Ok(()),
            Some(other) => Err(Error::Incompatible(other)),
            None => Err(Error::Malformed("a hello without a protocol".into())),
        }
    }

    /// The next request, skipping messages of kinds this version does not know; `None` when the input ends.
    fn next_call(&self, input: &mut impl Read) -> Result<Option<Call>, Error> {
        loop {
            let Some(frame) = frame::read(input, self.frame_limit)? else { return Ok(None) };
            let codec = match frame[0] {
                FRAME_JSON => Codec::Json,
                FRAME_CBOR => Codec::Cbor,
                FRAME_ATTACHMENT => return Err(Error::Malformed("an attachment without a message".into())),
                other => return Err(Error::Malformed(format!("a frame of unknown type {other}"))),
            };
            let message = codec.decode(&frame[1..]).map_err(Error::Malformed)?;
            if !matches!(message, Value::Map(_)) {
                return Err(Error::Malformed("a message that is not a map".into()));
            }
            let count = match message.get("attachments") {
                None => 0,
                Some(value) => value.as_i64().and_then(|n| usize::try_from(n).ok()).filter(|n| *n <= ATTACHMENTS)
                    .ok_or_else(|| Error::Malformed("attachments is not a count".into()))?,
            };
            let mut attachments = Vec::with_capacity(count);
            for _ in 0..count {
                let frame = frame::read(input, self.frame_limit)?.ok_or_else(|| Error::Malformed("the input ended before an attachment".into()))?;
                if frame[0] != FRAME_ATTACHMENT {
                    return Err(Error::Malformed("a message where an attachment was due".into()));
                }
                attachments.push(frame[1..].to_vec());
            }
            match message.get("kind").and_then(Value::as_str) {
                Some("request") => {}
                Some("hello") => return Err(Error::Malformed("a second hello".into())),
                Some(_) => continue,
                None => return Err(Error::Malformed("a message without a kind".into())),
            }
            let id = message.get("id").and_then(Value::as_i64).filter(|id| *id > 0)
                .ok_or_else(|| Error::Malformed("a request without a positive id".into()))?;
            let method = message.get("method").and_then(Value::as_str)
                .ok_or_else(|| Error::Malformed("a request without a method".into()))?.to_string();
            let payload = message.get("payload").cloned().unwrap_or(Value::Null);
            return Ok(Some(Call { id, method, payload, attachments, codec }));
        }
    }
}

/// Runs the handler on one call and writes its reply or failure whole.
fn answer<W: Write>(output: &Mutex<W>, handler: &(dyn Fn(Call) -> Result<Reply, Failure> + Send + Sync), call: Call) -> io::Result<()> {
    let (id, codec) = (call.id, call.codec);
    // A panicking handler answers its call with a failure instead of leaving it to time out (a pool worker
    // would die silently) or ending the helper with every other call in flight.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handler(call))).unwrap_or_else(|panic| {
        let message = panic.downcast_ref::<&str>().map(|s| s.to_string())
            .or_else(|| panic.downcast_ref::<String>().cloned()).unwrap_or_else(|| "the handler panicked".into());
        Err(Failure::new("panic", message))
    });
    let (message, attachments) = match outcome {
        Ok(reply) => {
            let mut entries = vec![("kind", Value::from("reply")), ("id", Value::from(id))];
            if let Some(payload) = reply.payload {
                entries.push(("payload", payload));
            }
            if !reply.attachments.is_empty() {
                entries.push(("attachments", Value::Int(reply.attachments.len() as i128)));
            }
            (Value::map(entries), reply.attachments)
        }
        Err(failure) => (
            Value::map([("kind", Value::from("failure")), ("id", Value::from(id)), ("code", Value::from(failure.code)), ("message", Value::from(failure.message))]),
            Vec::new(),
        ),
    };
    let frames = match codec.encode(&message) {
        Ok(body) => {
            let mut frames = vec![body];
            frames.extend(attachments.into_iter().map(|bytes| {
                let mut frame = Vec::with_capacity(bytes.len() + 1);
                frame.push(FRAME_ATTACHMENT);
                frame.extend(bytes);
                frame
            }));
            frames
        }
        // A payload with no form in the codec (an integer past JSON's range) is the call's failure, not the helper's.
        Err(error) => vec![codec
            .encode(&Value::map([("kind", Value::from("failure")), ("id", Value::from(id)), ("code", Value::from("invalid_reply")), ("message", Value::from(error.0))]))
            .expect("a failure has a form in every codec")],
    };
    write_frames(output, &frames)
}

fn write_frames<W: Write>(output: &Mutex<W>, frames: &[Vec<u8>]) -> io::Result<()> {
    let mut output = output.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    for body in frames {
        frame::write(&mut *output, body)?;
    }
    output.flush()
}

#[cfg(test)]
mod tests;
