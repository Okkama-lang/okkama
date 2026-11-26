#!/usr/bin/env bash
# One coverage target; called from the fuzz directory by run.sh.
# --dry-run prints commands only. The caller supplies the locked Cargo wrapper.
set -uo pipefail
HOST="$1"
TARGET="$2"
build_dir="${CARGO_TARGET_DIR:-target}/coverage"
if [[ "${3:-}" == --dry-run ]]; then
    echo "ASAN_OPTIONS=detect_odr_violation=0 cargo +nightly fuzz coverage --target $HOST --target-dir $build_dir $TARGET corpus/$TARGET"
    echo "llvm-cov report <coverage binary> -instr-profile=coverage/$TARGET/coverage.profdata"
    exit 0
fi
export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-C force-frame-pointers=yes"
mkdir -p logs coverage
log="logs/$TARGET-coverage.log"
report="coverage/$TARGET.txt"
# A stale TOTAL must never masquerade as this run's coverage.
rm -f "$report"
fail() {
    echo "COVERAGE FAILED: $TARGET ($log)" >&2
    tail -40 "$log" >&2
    exit 1
}
# The replay runs the target as `cargo fuzz run` does, with ASan's ODR check
# off (#1050). cargo-fuzz adds detect_odr_violation=0 to every run's
# ASAN_OPTIONS but not to its coverage replay, which then aborts at start-up in
# spb and exec, the targets that link sparrow-core and so SQLite: its
# amalgamation has two function-local `static const unsigned short aiOff[]`
# tables (FTS3's and FTS5's copies of one Unicode case-folding routine) with the
# same contents, placed at one address, and ASan reports that one global as
# registered twice. Nothing is linked twice; the run itself has never checked ODR.
replay_asan="${ASAN_OPTIONS:+$ASAN_OPTIONS:}detect_odr_violation=0"
if ! ASAN_OPTIONS="$replay_asan" CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" cargo +nightly fuzz coverage --target "$HOST" --target-dir "$build_dir" "$TARGET" "corpus/$TARGET" >"$log" 2>&1; then
    fail
fi
llvm="$(rustc +nightly --print sysroot)/lib/rustlib/$HOST/bin"
# Use the same explicit build directory for cargo-fuzz and llvm-cov.
binary="$build_dir/$HOST/release/$TARGET"
if [[ ! -f "$binary" ]]; then
    echo "coverage binary missing: $binary" >>"$log"
    fail
fi
if ! "$llvm/llvm-cov" report "$binary" -instr-profile="coverage/$TARGET/coverage.profdata" \
    -ignore-filename-regex='(\.cargo|rustc)/' >"$report.tmp" 2>>"$log"; then
    cat "$report.tmp" >>"$log"
    rm -f "$report.tmp"
    fail
fi
if ! grep -q '^TOTAL[[:space:]]' "$report.tmp"; then
    echo "llvm-cov report has no TOTAL line" >>"$log"
    cat "$report.tmp" >>"$log"
    rm -f "$report.tmp"
    fail
fi
mv "$report.tmp" "$report"
grep '^TOTAL[[:space:]]' "$report"
