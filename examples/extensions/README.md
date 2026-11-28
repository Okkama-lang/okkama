# Native PNG thumbnails

`imagekit` is a Blade package with an `okkama-ext` cdylib. `thumbs` starts one
async task per PNG in a directory, fits each image inside 8×8 with a triangle
filter, writes PNGs, and prints filename, dimensions and SHA-256 in filename
order. Only regular files with the lowercase `.png` suffix are selected.
All decode, resize and encode work runs on the blocking pool. `Picture.destroy`
releases the state; a running job retains its own `Shared<Image>`.

From the repository root, with the matching `blade` and `okkama` on PATH:

```sh
export OKKAMA_STDLIB_DIR="$PWD/std"
export CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=4
cargo build --offline -j 4 --manifest-path okkama-rs/Cargo.toml -p okkama-cli -p blade-cli
cd examples/extensions/thumbs
blade sync
blade build
blade run
blade run -- input output
blade run --no-native
```

Use `BLADE_OKKAMA=/absolute/path/to/okkama` if the binaries are not adjacent.
The Cargo cache must contain the dependencies in `imagekit/native/Cargo.lock`.
CI fetches that lock before the offline test. No Cargo command runs at sync.
Blade builds in `thumbs/target/native/imagekit`; the SDK can also be tried directly:

```sh
cargo build --offline --locked -j 4 --manifest-path examples/extensions/imagekit/native/Cargo.toml
```

The `okkama-rs -> ../../okkama-rs` symlink in this directory keeps the SDK and
vendored num-bigint path dependencies valid after Blade copies imagekit into
`thumbs/pkg/imagekit`. It is development scaffolding for this checkout; a
published package should use a pinned SDK revision instead of this path layout.
Blade does not rewrite Cargo path dependencies.

`thumbs/generate.py` regenerates the three tiny RGB fixtures deterministically.
`expected.txt` pins output dimensions and encoded PNG digests for image 0.25.10.
`no-native.expected.txt` pins the complete refusal and secondary failures before
the stack trace (function numbers and instruction offsets are compiler details).
The integration test also reads PNG headers and independently hashes files.

```sh
cargo test --offline -j 4 --manifest-path okkama-rs/Cargo.toml -p blade --test extension_example
```

Build both CLIs first. The test runs the copied example in a private target
subdirectory, normally and with `OKKAMA_EXT_CHECK=1`, and checks typed decode/size errors and
PNG round trips. Missing offline dependencies
skip with a note locally and fail under `CI`. Linux and macOS CI run it through
nextest; Wave's `rest` part also runs it with `SPARROW_CORE_GC_STRESS=1`.
