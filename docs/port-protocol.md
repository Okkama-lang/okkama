# The port protocol, version 1

A **helper** is a program that an Okkama program starts and calls through std's `os/port` (#1081), in the manner
of Erlang's ports. The helper reads requests on its standard input and writes replies on its standard output. This
page is the whole contract: a helper can be written from it in any language. `okkama-rs/crates/okkama-port` is the
Rust implementation, and `examples/port/text_helper.py` is a Python one in about a hundred lines.

The **port** is the Okkama side. It starts the helper, sends requests, matches the replies to them and restarts a
helper that failed.

## Streams

- **Standard input** carries frames from the port to the helper. **Standard output** carries frames from the helper
  to the port. Nothing else may be written to standard output.
- **Standard error** is free for the helper's logs. The port keeps the last 2 KiB of it and puts them into the
  failure of a crashed helper.
- **The end of standard input** means the port is done: the program closed the port or ended. The helper may finish
  the calls it has started, then it exits with status 0.

## Frames

Every frame is a 4-byte unsigned big-endian length `N`, then `N` bytes of body. `N` is at least 1. The first body
byte is the frame's type:

| Byte | Type | Rest of the body |
|---|---|---|
| `J` (0x4A) | a message in JSON | one JSON object, UTF-8 |
| `C` (0x43) | a message in CBOR | one CBOR map (RFC 8949) with text keys |
| `B` (0x42) | an attachment | raw bytes that belong to the message before it |

A receiver refuses a frame longer than its limit. The port's limit is 16 MiB unless the program sets another
(`Port(frame_limit:)`). okkama-port's limit is 16 MiB too (`Server::frame_limit`).

## Messages

A message is a JSON object or a CBOR map. Its `kind` member, a text, says what it is:

| Kind | From | Members |
|---|---|---|
| `hello` | both | `protocol` (integer); from the helper also `codecs` (array of texts) |
| `request` | port | `id`, `method` (text), `payload` (any value), `attachments` |
| `reply` | helper | `id`, `payload`, `attachments` |
| `failure` | helper | `id`, `code` (text), `message` (text) |

- **`id`** is an integer from 1 to 2⁵³−1. The port numbers the requests to one helper process 1, 2, 3 and so on. A
  reply or a failure carries the `id` of its request.
- **`payload`** is any value of the message's codec. When it is absent, the payload is null.
- **`attachments`** is the count of `B` frames that follow the message, 0 when absent, at most 1024.
- **Unknown members are ignored**, and so are messages of an unknown kind (with their attachments). This lets a
  later version add members and kinds within protocol 1.

## Handshake

1. Each side's first frame is a `J` hello. Neither side waits for the other's hello before sending its own.
2. The port's hello is `{"kind":"hello","protocol":1}`.
3. The helper's hello is `{"kind":"hello","protocol":1,"codecs":["json","cbor"]}`. `codecs` lists the codecs the
   helper accepts. `json` must be accepted. When `codecs` is absent, it means `["json"]`.
4. The port refuses a helper whose `protocol` is not 1. The program sees `PortFailure.incompatible(protocol)`, and
   the helper is stopped.
5. A helper that receives another `protocol` writes a line to standard error and exits with status 2.
6. The port sends no request before the helper's hello. A helper that writes anything else first is broken (see
   [Errors](#errors)).

## Calls

The port writes a request, then its attachments. A reply or a failure comes back in the codec of its request: a `J`
request gets a `J` answer, and a `C` request gets a `C` answer. The port sends a `C` request only to a helper that
named `cbor` in its hello. Otherwise the call fails with `PortFailure.unsupported("cbor")` before anything is sent.

A request with bytes, in JSON, as the bytes on the wire (lengths in hex):

```text
00 00 00 54  J{"kind":"request","id":1,"method":"resize","payload":{"width":200},"attachments":1}
00 00 00 05  B 89 50 4E 47
```

The reply:

```text
00 00 00 31  J{"kind":"reply","id":1,"payload":{"bytes":1234}}
```

- **Concurrency.** Many requests may be in flight on one helper. The helper may answer them in any order, and from
  several threads, as long as each message and its attachments are written together, with no other frame in
  between.
- **Failures.** A failure answers its request with a short `code` the program can match (`unknown_method`,
  `invalid_payload`, `not_found`) and a `message` for people. The program sees
  `PortFailure.refused(method, code, message)`, and the helper keeps serving.
- **Late answers.** The port ignores a reply or a failure whose `id` it is no longer waiting for: a call that its
  caller stopped waiting for.

## Errors

**What the port does when the helper breaks the protocol.** These are protocol errors:

- a frame of length 0 or over the limit;
- a frame of an unknown type;
- a `B` frame where no attachment is due, or another frame where one is;
- a body that does not parse as a JSON object or a CBOR map;
- a `kind`, `id` or `protocol` that is missing or of the wrong type;
- a message before the hello, or a second hello;
- output that ends inside a frame.

On a protocol error the port kills the helper. Every call in flight on it fails with
`PortFailure.malformed(method, reason)`, and the next call starts a new helper.

**What the port does when the helper ends or hangs.**

- **A helper that ends** (any exit status, a signal, or closed standard output) fails the calls in flight with
  `PortFailure.crashed(method, detail)`. The detail gives the exit status and the tail of standard error. The next
  call starts a new helper.
- **A call that is not answered within its timeout** fails with `PortFailure.timed_out(method)`. The port treats the
  helper as hung and kills it, because a stuck helper would hold up every call after it. The other calls in flight
  there fail with `crashed`, and the next call starts a new helper. The timeout covers starting the helper and its
  hello, when the call needs a new one.

**What a helper does when the port breaks the protocol** (the same list, seen from the helper's side): it writes a
line to standard error and exits with status 2.

## Values

- **JSON** is RFC 8259 text. A helper keeps integers exact where its language allows. The port reads numbers
  exactly.
- **CBOR** is RFC 8949, read with definite and indefinite lengths. It carries what JSON cannot:
  - byte strings;
  - integers of any size (tags 2 and 3 beyond 64 bits);
  - `rat` as tag 30 over `[numerator, denominator]`;
  - maps with keys that are not text;
  - other tags.

  The port writes definite lengths only. A deep structure can travel as one CBOR value (`Cbor` in Okkama) in the
  payload.
- **Attachments** are for large raw data: images, files, model weights. They are never base64'd.

## Finding the helper

The program names the helper by its command: `Port(command: InheritedCommand(program: Path("bin/helper")))`. A path
with a directory runs that file, and a bare name is looked up in `PATH`. Arguments and an environment go in the
command like any other (`os/command`). For now Blade does not carry helper binaries. A project builds its helper
(for example `cargo build --release`) and names the path. Packaging helpers in Blade shares its step with native
extensions (#1083, step 6).

## Writing a helper in Rust

```rust
use okkama_port::{Call, Failure, Reply};

fn main() {
    okkama_port::serve(|call: Call| match call.method() {
        "reverse" => Ok(Reply::bytes(call.bytes().into_iter().rev().collect())),
        other => Err(Failure::unknown_method(other)),
    });
}
```

`Call::decode` reads the payload into a serde type, and `Reply::json` writes one. `okkama_port::Server::new()
.threads(4).serve(…)` answers four calls at once. The crate's documentation and `examples/text_helper.rs` show the
rest.

## Costs

A call costs a few tens of microseconds: two pipe writes and the encoding. That is fine for resizing an image or
asking a model, and too slow for a hot inner loop. Data is copied both ways. The playground cannot start helpers.
