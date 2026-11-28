//! The helper of okkama-cli's `tests/port.rs` (#1081): methods that answer, wait, fail, crash and hang, to test
//! std's `os/port` against a real helper. Not an example to copy; `text_helper.rs` is.

use std::time::Duration;

use okkama_port::{Call, Failure, Reply, Server, Value};

fn main() {
    let server = if std::env::args().any(|argument| argument == "--json-only") { Server::new().json_only() } else { Server::new() };
    server.threads(8).serve(|call: Call| match call.method() {
        // The payload and the bytes back, in the codec of the call.
        "echo" => {
            let reply = Reply::value(call.payload().clone());
            Ok(if call.bytes().is_empty() { reply } else { reply.with_bytes(call.bytes()) })
        }
        // Waits `ms` milliseconds, then says so.
        "sleep" => {
            let ms = call.payload().get("ms").and_then(Value::as_i64).ok_or_else(|| Failure::new("invalid_payload", "ms"))?;
            std::thread::sleep(Duration::from_millis(ms as u64));
            Ok(Reply::value(Value::map([("slept", Value::from(ms))])))
        }
        // Which process answered: a restart or another helper of a pool answers with another pid.
        "pid" => Ok(Reply::value(Value::map([("pid", Value::from(std::process::id() as i64))]))),
        "fail" => Err(Failure::new("not_found", "no such thing")),
        "crash" => {
            eprintln!("boom: the helper crashed on purpose");
            std::process::exit(3);
        }
        "hang" => loop {
            std::thread::sleep(Duration::from_secs(60));
        },
        other => Err(Failure::unknown_method(other)),
    });
}
