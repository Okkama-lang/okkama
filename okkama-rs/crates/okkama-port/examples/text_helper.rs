//! A text helper for Okkama's `os/port` (#1081): the Rust side of `examples/port/text.ok`.
//!
//! ```text
//! cargo build -p okkama-port --example text_helper
//! OKKAMA_TEXT_HELPER=okkama-rs/target/debug/examples/text_helper okkama run examples/port/text.ok
//! ```

use okkama_port::{Call, Failure, Reply, Server};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct Text {
    text: String,
}

#[derive(Serialize)]
struct Counted {
    words: usize,
    longest: String,
}

/// CRC-32 (IEEE), the checksum of zip and PNG.
fn crc32(bytes: &[u8]) -> u32 {
    !bytes.iter().fold(!0u32, |crc, byte| {
        (0..8).fold(crc ^ *byte as u32, |crc, _| if crc & 1 == 1 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 })
    })
}

fn main() {
    // Four calls at once: a slow one does not hold up the others.
    Server::new().threads(4).serve(|call: Call| match call.method() {
        "upper" => {
            let request: Text = call.decode()?;
            Reply::json(&serde_json::json!({ "text": request.text.to_uppercase() }))
        }
        "words" => {
            let request: Text = call.decode()?;
            let longest = request.text.split_whitespace().max_by_key(|word| word.chars().count()).unwrap_or("");
            Reply::json(&Counted { words: request.text.split_whitespace().count(), longest: longest.to_string() })
        }
        "checksum" => Reply::json(&serde_json::json!({ "crc32": crc32(&call.bytes()), "size": call.bytes().len() })),
        "reverse" => Ok(Reply::bytes(call.bytes().into_iter().rev().collect())),
        other => Err(Failure::unknown_method(other)),
    });
}
