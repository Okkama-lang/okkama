#!/usr/bin/env bash
# Coverage-guided fuzzing of the toolchain (H-17, #34).
#
#     tools/fuzz/run.sh [SECONDS] [--coverage]
#
# Runs every target of okkama-rs/fuzz for SECONDS (default 300) each:
# lex, parse (drops the AST, re-parses formatted source), analyze (lowering
# verifies), ir_verify (mutated OIR), spb (decode, verify, intrinsic
# contracts), exec (programs linked against the repository's std, on the
# Sparrow core under budgets, no host natives; with OKKAMA_FUZZ_DIFFERENTIAL=1
# each program runs on the JIT and on the interpreter too, and the two must agree
# but for the design's known divergences, #1123), cbor (any bytes decoded as CBOR within
# limits; encodings decode again and the deterministic one is a fixed point, #1082),
# ext (hostile native slots, tokens, statuses and host calls under a memory budget, #1083).
# Needs nightly Rust and cargo-fuzz; mass
# runs belong in a container or on CI (.github/workflows/fuzz.yml). Corpora grow in
# okkama-rs/fuzz/corpus/<target> (seeded from std, conformance, examples and
# compiled SPB; past findings from okkama-rs/fuzz/regressions/<target>),
# findings land in okkama-rs/fuzz/artifacts/<target>, and with
# --coverage a summary per target in okkama-rs/fuzz/coverage/<target>.txt;
# each run's output is in okkama-rs/fuzz/logs/<target>.log.
# Exit status 1 when any target found something, 2 when the targets do not
# build (logs/build.log), 3 when coverage fails (logs/<target>-coverage.log).
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TIME="${1:-300}"
COVERAGE=0
[[ "${2:-}" == "--coverage" ]] && COVERAGE=1
FUZZ="$ROOT/okkama-rs/fuzz"
TARGETS=(lex parse analyze ir_verify spb exec cbor ext)
# cargo-fuzz does not expose --locked; its child Cargo invocations use PATH.
# Keep every build/check/metadata invocation on the checked-in graph.
export OKKAMA_FUZZ_CARGO="$(command -v cargo)"
export PATH="$ROOT/tools/fuzz/locked-cargo:$PATH"
# cargo-fuzz supplies RUSTFLAGS, overriding .cargo/config.toml; Sparrow GC
# needs frame pointers through every Rust frame, including coverage replay.
export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-C force-frame-pointers=yes"
# cargo-fuzz builds for the triple it was itself built for unless told; a
# prebuilt musl cargo-fuzz (CI's install-action) then asks for a static libc
# that the sanitizers refuse. Build for the nightly host instead.
HOST="$(rustc +nightly -vV | sed -n 's/^host: //p')"
cd "$FUZZ"

# Seeds: every Okkama source of the repository for the source targets.
for target in lex parse analyze ir_verify exec; do
    mkdir -p "corpus/$target"
    find "$ROOT/std" "$ROOT/conformance/cases" "$ROOT/okkama-rs/examples" "$FUZZ/seeds" -name '*.ok' -print0 |
        while IFS= read -r -d '' file; do
            cp -n "$file" "corpus/$target/$(cksum <"$file" | cut -d' ' -f1).ok"
        done
done
# Regressions: each input a run once crashed on (okkama-rs/fuzz/regressions/<target>,
# also replayed by okkama-compiler's tests/fuzz_lowering.rs) stays in its corpus.
for dir in "$FUZZ/regressions"/*/; do
    [[ -d "$dir" ]] || continue
    target="$(basename "$dir")"
    mkdir -p "corpus/$target"
    cp -n "$dir"* "corpus/$target/"
done
# CBOR seeds: RFC 8949 Appendix A examples and a few malformed items.
mkdir -p corpus/cbor
cp -n "$FUZZ/seeds/cbor/"* corpus/cbor/ 2>/dev/null || true
# Extension seeds: valid operations, malformed slots, stale tokens and ignored errors.
mkdir -p corpus/ext
cp -n "$FUZZ/seeds/ext/"* corpus/ext/ 2>/dev/null || true
# SPB seeds: the examples and the fuzz seeds, compiled.
mkdir -p corpus/spb
(cd "$ROOT/okkama-rs" && cargo build --release -q -p okkama-cli)
for file in "$ROOT/okkama-rs/examples"/*.ok "$FUZZ/seeds"/*.ok; do
    "$ROOT/okkama-rs/target/release/okkama" build "$file" -o "corpus/spb/$(basename "$file" .ok).spb" 2>/dev/null || true
done

# Build every target first: a build killed for memory must not be reported as
# a finding. An ASan build of cranelift-codegen with one codegen unit needs
# several gigabytes on its own, so a failed parallel build is retried with one
# job, continuing from the crates already built.
mkdir -p logs
if ! CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" cargo +nightly fuzz build --target "$HOST" >logs/build.log 2>&1 &&
    ! CARGO_BUILD_JOBS=1 cargo +nightly fuzz build --target "$HOST" >>logs/build.log 2>&1; then
    grep -E "^error|SIGKILL|could not compile" logs/build.log | head -10
    echo "BUILD FAILED (okkama-rs/fuzz/logs/build.log)"
    exit 2
fi

found=()
coverage_failed=()
for target in "${TARGETS[@]}"; do
    echo "==> $target (${TIME}s)"
    mkdir -p "artifacts/$target"
    if ! cargo +nightly fuzz run --target "$HOST" "$target" "corpus/$target" -- \
        -max_total_time="$TIME" -rss_limit_mb=2048 -timeout=30 -max_len=65536 -print_final_stats=1 \
        >"logs/$target.log" 2>&1; then
        found+=("$target")
        grep -E "^==[0-9]+==|panicked|ERROR|SUMMARY|Test unit written|error" "logs/$target.log" | head -20
    fi
    grep -E "^Done|stat::(number_of_executed_units|new_units_added|peak_rss_mb)" "logs/$target.log"
    if [[ $COVERAGE -eq 1 ]]; then
        if ! "$ROOT/tools/fuzz/coverage.sh" "$HOST" "$target"; then
            coverage_failed+=("$target")
        fi
    fi
done
if [[ ${#coverage_failed[@]} -gt 0 ]]; then
    echo "COVERAGE FAILED: ${coverage_failed[*]} (okkama-rs/fuzz/logs/*-coverage.log)" >&2
    [[ ${#found[@]} -gt 0 ]] && echo "FINDINGS: ${found[*]} (okkama-rs/fuzz/artifacts)"
    exit 3
fi
if [[ ${#found[@]} -gt 0 ]]; then
    echo "FINDINGS: ${found[*]} (okkama-rs/fuzz/artifacts)"
    exit 1
fi
echo "no findings"
