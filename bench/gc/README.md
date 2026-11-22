# The collector's benchmark set (#917)

One set of GC workloads and one way to measure them, so every change to Sparrow's collector (epic #916) is an
A/B on the same programs. `run.py --help` describes the workloads and the three passes:
- timing: shuffled processes, median and IQR, peak RSS, instructions on macOS;
- collector: `OKKAMA_DEV=gc` collections, collect/mark/sweep totals and maxima, sweep cells;
- bytes: `SPARROW_CORE_STATS` live and owned bytes.

```sh
# two plain release builds (no --cfg okkama_release: the collector pass needs devtrace)
python3 bench/gc/run.py --okkama base=../base/okkama-rs/target/release/okkama \
    --okkama new=okkama-rs/target/release/okkama --out ../okkama-docs/benchmarks/gc/<label>.json
```

The `roots ms` column is the root scan (`mark roots`); `other ms` is collect time outside roots, marking and sweeping: what the runtime does around a
collection (identity and state bookkeeping, `forget`, the watch list). Results live in
`okkama-docs/benchmarks/gc/`. Run on a quiet machine, never in CI (bimodal runners).
