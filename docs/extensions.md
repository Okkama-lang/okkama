# Native extensions

A native extension is trusted code loaded into the Okkama process. Use one for a
small typed interface to a native library, low call overhead, or native state
that should live with an Okkama resource. The complete example is
[imagekit and thumbs](../examples/extensions/README.md): concurrent PNG thumbnails
through a Blade package, with deterministic output and CI coverage.

Use [`os/port`](port-protocol.md) and a helper process for code that may crash,
needs a separate runtime, or should run under OS isolation. A helper pays for
process communication but can fail independently. CBOR is a data format, not a
third execution model: serialize deep structures with `cbor` into `@byte` for an
extension, or use the port protocol's CBOR messages and byte attachments. Keep
ordinary scalar arguments typed rather than serializing every call.

## C ABI 1

The contract is [okkama_ext.h](../okkama-rs/crates/okkama-runtime-abi/include/okkama_ext.h).
Include it and export the data symbol `const okk_extension okkama_extension`.
The descriptor names exports, signatures, parameter names and state types; its
`abi` is `OKKAMA_EXT_ABI` (1), its `size` is `sizeof(okk_extension)`. ABI and size
must match exactly. ABI 1 currently loads on Unix with the host feature.

Each `okk_slot` has `bits` and `kind`. INT (signed i64), FLOAT (binary64), BOOL,
BYTE and CHAR hold values directly. BIGINT, RAT, STRING, ARRAY, MAP and STATE
hold opaque call-scoped tokens. NONE represents void or an absent Maybe.
`@byte` is an ARRAY with a bulk byte view. Large integers use signed magnitudes;
rationals use normalized integer numerator and denominator. Arrays have one
element shape and maps one key and value shape. A `Maybe<T>` is NONE or T's slot.

Callbacks receive an `okk_call`; `call->host` is the only host table they use.
It reads text/byte views and collections, constructs values, reads or constructs
big integers and rationals, manages states, reports failure, and checks
interruption. `bytes_reserve` gives zeroed writable result storage until return.
Input buffers passed to constructors are copied. Tokens, views, and call pointers
expire when the callback returns: never keep them in global data or a job.

| Status | Meaning |
|---|---|
| `OKK_OK` | Successful result of the declared shape |
| `OKK_FAILED` | Pending failure set with `host->fail(code, message)` |
| `OKK_STOPPED` | Host resource stop; sticky and not catchable by the program |
| `OKK_PANIC` | Caught Rust panic; stops and poisons this runtime |
| `OKK_BAD_TOKEN`, `OKK_BAD_KIND`, `OKK_BAD_SHAPE` | Sticky contract faults |
| `OKK_BAD_TEXT`, `OKK_RANGE` | Recoverable host-operation errors; handle them before returning |
| `OKK_UNSUPPORTED` | Unimplemented host operation |

Returning an invalid status, a contract fault, or a result of the wrong shape
stops and poisons the runtime. For an ordinary domain error, call `fail` and
return `OKK_FAILED`. Okkama receives `NativeFailure` with `code()` and `message()`;
the host supplies `domain()` and `operation()`. Translate these codes into a
package error enum in the `.ok` wrapper, as imagekit and std do. Exceptions,
`longjmp`, and unwinding must never cross the C ABI.

`state_new(type, payload, bytes)` transfers an opaque payload only on success;
`bytes` contributes collector pressure. The state's drop callback runs once on
the VM thread, at explicit release, dead-cell collection, or runtime teardown.
`state_get` and `state_release` after release fail with `already_closed`.
The `native object` carries no language ownership token. Put it in an ordinary
object with `destroy { release(state); }` for deterministic release on ordinary
scope exit, failure and cancellation. Budget stops do not promise that cleanup.

## Async and threads

**Keep VM-thread callbacks below 1 ms.** Longer work belongs in an async export:

1. `prepare` runs on the VM thread on first poll of the cold Future; copy inputs
   and retain independently owned state references.
2. `execute` runs on the blocking pool without VM access. Poll `okk_cancel` when
   appropriate. Never call the host table here.
3. `complete` runs on the VM thread in a new scope, constructing the result.
4. `drop(job)` runs once after completion, or instead on cancellation/teardown;
   it may run on either thread, after execute returns, and cannot use the host.

Successful prepare transfers job ownership even for a null pointer. On a failed
prepare the extension must free anything it allocated. Complete does not free
the job. Cancellation waits for execute to return, skips complete and then drops
the job; it cannot forcibly stop arbitrary native code. Copy a state reference
in prepare (an Arc in Rust), because the program can release the host state while
a job is executing. Shared-state payload destruction occurs after the last Arc.

Sync, prepare, complete and state-drop callbacks have at least 768 KiB stack
headroom (`OKKAMA_EXT_STACK`). `OKKAMA_DEV=runtime` reports sync calls over 1 ms.
Independent runtimes can use the same library on different threads: protect
process-global state. A library's own Tokio runtime does not share Sparrow's
reactor. Model event streams with pull operations such as `native async next`.

## Rust SDK

The SDK lives in `okkama-rs/crates/okkama-ext` and the proc macros in
`okkama-ext-macros`. A standalone crate in `examples/extensions/imagekit/native`
uses this manifest layout (adjust the relative SDK path for your project):

```toml
[package]
name = "small-extension"
version = "0.1.0"
edition = "2024"
publish = false

[lib]
crate-type = ["cdylib"]

[dependencies]
okkama-ext = { path = "../../../../okkama-rs/crates/okkama-ext" }

[workspace]

[patch.crates-io]
num-bigint = { path = "../../../../okkama-rs/vendor/num-bigint" }
```

Keep its own Cargo.lock. Build with `cargo build --offline --locked -j 4` after
resolving its dependencies. Here is a complete library:

```rust
use okkama_ext::{extension, native, state, Failure, Shared};

#[state(shared)]
struct Text(String);

#[native]
fn create(value: &str) -> Result<Text, Failure> {
    if value.len() > 4096 {
        return Err(Failure::new("too_long", "at most 4096 UTF-8 bytes"));
    }
    Ok(Text(value.to_owned()))
}

#[native(blocking)]
fn uppercase(text: Shared<Text>) -> String {
    text.0.to_uppercase()
}

#[native(release)]
fn release(text: Shared<Text>) { drop(text); }

extension!("small/native", [create, uppercase, release], states: [Text]);
```

`#[native]` generates the descriptor and a shim with panic containment; exported
functions remain ordinary Rust functions. Use unwinding builds: `panic = "abort"`
ends the process. `#[native(blocking)]` copies borrowed inputs into the job and
publishes its result on the VM thread. It accepts `Shared<T>` for states, with
`T: Send + Sync`. It does not expose the C cancellation flag to the Rust function;
a blocking function must return for cancellation to settle.

`#[state]` uses Box storage and supports `&T`, `&mut T`, and a by-value argument
that releases and moves the payload. `#[state(shared)]` uses Arc storage. Release
shared state with `#[native(release)]`, not a by-value T. Release shims always
release after the function, including on failure or panic. Conflicting borrows
fail with `state_borrowed`; shared mutable access requires a unique Arc.

The SDK maps `i64`, `BigInt`, `f64`, `bool`, `u8`, `char`, `&str`/`String`,
`&[u8]`/`Vec<u8>`, `Vec<T>`, `BTreeMap<K,V>`, `Option<T>`, states and `()`.
Use `Result<T, Failure>` for domain errors. An Okkama int outside i64 is
`out_of_range` when read as i64; use BigInt when arbitrary precision is needed.

## Okkama declarations

The corresponding module can be written by hand:

```okkama
module small/native;

// Native storage, owned by TextOwner.
native object Text;
// Copies at most 4096 UTF-8 bytes; too_long on larger input.
native create(value is string) is Text;
// Uppercase on the blocking pool.
native async uppercase(text is Text) is string;
// Releases storage; active jobs retain their own references.
native release(text is Text) is void;

// Owns the state and releases it on scope exit.
object TextOwner {
    has state is Text;
    TextOwner(value is string) { state = create(value); }
    destroy { release(state); }
    async upper() is string { await uppercase(state); }
}
```

Keep doc comments on the public wrapper for `okkama man`. Imagekit's
[wrapper](../examples/extensions/imagekit/src/image.ok) also maps failure codes
into typed `ImageError` cases.

Symbols are `module@major::name/arity`, for example
`small/native@0::uppercase/1`. Package modules without a version have major 0.
State identity uses the full module and type name. A signature mismatch refuses
binding before any native callback; an absent provider refuses when called.
Duplicate symbols and names in the reserved `runtime/` namespace are refused.

```sh
okkama ext declare path/to/libsmall_extension.so > native.ok
```

This loads a trusted library and prints an editable module skeleton with native
objects and parameter names. Add author documentation and wrappers yourself.
It supports one export module per library. Loading can run OS constructors even
though no native callback is called. macOS libraries normally end in `.dylib`.

## Loading and Blade packages

```sh
okkama run --ext path/to/library.so main.ok
okkama exec --ext path/to/library.so main.spb
OKKAMA_EXT_CHECK=1 okkama run --ext path/to/library.so main.ok
okkama run --no-native --ext path/to/library.so main.ok
```

`--ext` is repeatable; `--no-native` suppresses loading regardless of option
order. It does not disable std natives. Checked mode copies and invalidates
views at callback return to expose lifetime mistakes. Libraries stay loaded
until process exit. An SPB never chooses a library path. Restricted native
groups and embedding profiles refuse external extensions; wasm has no loader.

A Blade package declares its source crate:

```toml
[native]
crate = "native"
```

The application's root manifest opts into every native dependency, including
transitive ones:

```toml
[dependencies]
imagekit = { path = "../imagekit", version = "1.0.0" }
[native]
allow = ["imagekit"]
```

`blade sync` records and reports native packages but runs no Cargo. `blade build`
builds allowed cdylibs with `cargo build --release --locked` into
`target/native/NAME`; its stamp covers package contents, tool versions and the
library digest. `blade run` builds as needed and passes each library to
`okkama exec --ext`. `blade run --no-native` skips native builds and loading.
Set `CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=4` for offline builds. Cargo path
dependencies must remain valid from the installed `pkg/` tree; Blade does not
rewrite them. Prebuilt extension distribution is not part of ABI 1.

## Limits and trust

ABI 1 has no callbacks into Okkama, actions/closures, `any`, ordinary object
layouts, or persistent VM references. Nested Maybe is not representable. The C
ABI supports `rat`, but the Rust SDK has no rational type mapping yet. Use a
byte payload such as CBOR for deep application structures.

Native work consumes no VM steps. Host allocations obey the configured memory
budget, but extension allocations do not; native stdout/stderr bypass the output
budget. Timeout and cancellation remain cooperative. Building runs Cargo build
scripts and loading runs OS constructors. Read [security-model §6](security-model.md#6-native-extensions-1083)
before deciding to load a library.
