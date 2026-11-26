#!/usr/bin/env bash
# One coverage target; called from the fuzz directory by run.sh.
# --dry-run prints commands only. The caller supplies the locked Cargo wrapper.
set -uo pipefail
HOST="$1"
TARGET="$2"
build_dir="${CARGO_TARGET_DIR:-target}/coverage"
if [[ "${3:-}" == --dry-run ]]; then
    echo "cargo +nightly fuzz coverage --target $HOST --target-dir $build_dir $TARGET corpus/$TARGET"
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
if ! CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" cargo +nightly fuzz coverage --target "$HOST" --target-dir "$build_dir" "$TARGET" "corpus/$TARGET" >"$log" 2>&1; then
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
