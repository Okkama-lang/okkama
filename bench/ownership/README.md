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
