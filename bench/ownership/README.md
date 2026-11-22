# Ownership gate (#908)

`run.py` measures Okkama against CPython on the same work in the same container (`okkama-dev`, 1 CPU, no
network). It runs the 11-ownership ladder program and its plain twin, plus the micro pairs of `bench/micro`.
Each `--okkama` is a Linux build of the CLI. A Docker build of a worktree:

    docker run --rm --network none -v WORKTREE:/src:ro -v okkama-cargo-registry:/usr/local/cargo/registry \
      -v okkama-perf-NAME:/target -v OUT:/out -e CARGO_NET_OFFLINE=true -e RUSTFLAGS="--cfg okkama_release" \
      okkama-dev sh -c 'cargo build --release --locked --offline --manifest-path /src/okkama-rs/Cargo.toml \
      --target-dir /target -p okkama-cli && cp /target/release/okkama /out/'

Run it on a quiet machine, never in CI.

## 2026-11-20: Tomato (fa45220ba) and Tweed after #905/#906 (902eb0ef7), CPython 3.13

| program | tomato | tweed | CPython |
|---|---:|---:|---:|
| ownership.ok (ms) | 100.5 | 23.0 | 28.2 |
| plain twin (ms) | 4.8 | 4.8 | — |
| baseline (ns/it) | 3.5 | 3.3 | 23.0 |
| constructor (ns/it) | 7.2 | 7.1 | 103.6 |
| successor (ns/it) | 12.7 | 12.1 | 182.8 |
| destroy (ns/it) | 111.4 | 116.9 | 204.6 |
| destroy-observed (ns/it) | 98.9 | 99.6 | 183.0 |
| defer (ns/it) | 11.2 | 11.2 | 96.6 |

The target is met: ownership.ok runs at 23.0 ms against CPython's 28.2. A resource still costs about 100 ns over
a plain object (micro destroy). That cost is region enter, adopt and leave on every call: phase 2, #909.

## 2026-11-20: after #909 2a, #528 (+16) and #909 2b (af676df2e), with Go 1.23.4

Same container (1 CPU, 2 GB, no network). Tomato and the first Tweed are the builds of the table above, rerun;
the machine ran a little slower this time (Tomato 116 ms, CPython 29.8 ms), so compare within the table. Go is
built statically by `run.py` in `golang:1.23.4-bookworm`; a destroy is `defer Close()`.

| program | tomato | tweed | tweed-2b | CPython | Go |
|---|---:|---:|---:|---:|---:|
| ownership.ok (ms) | 116.3 | 26.2 | 24.4 | 29.8 | 5.0 |
| plain twin (ms) | 5.2 | 5.3 | 5.2 | — | — |
| baseline (ns/it) | 3.5 | 3.4 | 3.6 | 23.4 | 0.9 |
| constructor (ns/it) | 7.5 | 7.5 | 7.6 | 106.8 | 1.2 |
| successor (ns/it) | 12.7 | 12.7 | 12.3 | 188.4 | 1.0 |
| destroy (ns/it) | 118.5 | 117.4 | 47.1 | 212.2 | 3.2 |
| destroy-observed (ns/it) | 105.1 | 105.7 | 36.8 | 190.9 | 3.9 |
| defer (ns/it) | 11.5 | 11.3 | 11.4 | 98.0 | 3.1 |

The plain twin runs as fast as Go's program (5.2 against 5.0 ms): what separates Tweed from Go on ownership.ok is
the ownership tax itself, 19 ms, mostly 604 000 `region_add` calls that find nothing to own (#529, #526).

## 2026-11-21: after #529, #530 and the static pass's reach (9f3740325)

Same container and builds as above, rerun; `tweed-final` is release/0.6.0 at 9f3740325.

| program | tomato | tweed-2b | tweed-final | CPython | Go |
|---|---:|---:|---:|---:|---:|
| ownership.ok (ms) | 107.2 | 22.0 | 7.8 | 27.5 | 5.1 |
| plain twin (ms) | 4.7 | 5.2 | 5.2 | — | — |
| baseline (ns/it) | 3.6 | 3.6 | 3.6 | 23.5 | 1.1 |
| constructor (ns/it) | 7.5 | 7.4 | 7.5 | 105.3 | 1.3 |
| successor (ns/it) | 12.6 | 12.8 | 12.7 | 197.2 | 1.0 |
| destroy (ns/it) | 114.6 | 47.7 | 20.6 | 213.8 | 3.2 |
| destroy-observed (ns/it) | 101.9 | 36.5 | 18.2 | 188.7 | 3.6 |
| defer (ns/it) | 11.5 | 11.5 | 11.4 | 96.3 | 3.0 |

ownership.ok now runs 3.5 times faster than CPython and 1.5 times slower than Go. The ownership tax over the
plain twin is 2.6 ms, down from 19 ms after 2b.
