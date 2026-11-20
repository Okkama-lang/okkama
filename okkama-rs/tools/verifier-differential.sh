#!/bin/sh
# Reproduce the #634 precise/compact verifier gate across the OIR corpus.
# Usage: verifier-differential.sh [all|oir|compiler]; the Wave CI runs the two parts as parallel jobs.
# OKKAMA_DIFFERENTIAL_CARGO adds cargo options (the CI: "--profile gate --locked").
set -eu
cd "$(dirname "$0")/.."
part="${1:-all}"
cargo_options="${OKKAMA_DIFFERENTIAL_CARGO:-}"
case "$part" in
    all|oir|compiler) ;;
    *) echo "usage: $0 [all|oir|compiler]" >&2; exit 2 ;;
esac
records=$(mktemp -d "${TMPDIR:-/tmp}/okkama-verifier-differential.XXXXXX")
trap 'rm -rf "$records"' EXIT HUP INT TERM
export OKKAMA_634_DIFFERENTIAL=1
# #803: every edge also checks the partition and the class remap against the full remap.
export OKKAMA_803_EDGE_CHECK=1
export OKKAMA_634_RESULTS="$records/results.tsv"
export OKKAMA_634_RAISED="$records/raised.tsv"
status=0
if [ "$part" != compiler ]; then
    # shellcheck disable=SC2086
    cargo test --offline -j 4 $cargo_options -p okkama-oir --features verifier-differential || status=$?
fi
if [ "$status" -eq 0 ] && [ "$part" != oir ]; then
    # shellcheck disable=SC2086
    cargo test --offline -j 4 $cargo_options -p okkama-compiler --features verifier-differential --lib oir2 \
        || status=$?
fi
python3 - "$OKKAMA_634_RESULTS" "$status" <<'PY'
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
rows = [line.split("\t") for line in path.read_text().splitlines()] if path.exists() else []
if any(len(row) != 12 for row in rows):
    sys.exit("Malformed differential record")
compared = len(rows)
precise_complete = sum(row[4] == "true" for row in rows)
rejected = sum(row[5] == "Some(Rejected)" for row in rows)
both_incomplete = sum(row[5] == row[6] == "Some(Incomplete)" for row in rows)
mismatches = sum(row[11] == "true" for row in rows)
candidate_incomplete = sum(row[4] == "true" and row[6] == "Some(Incomplete)" for row in rows)
print(f"compared: {compared}")
print(f"precise complete: {precise_complete}")
print(f"rejected (precise): {rejected}")
print(f"both Incomplete: {both_incomplete}")
print(f"mismatches: {mismatches}")
print(f"candidate Incomplete where precise completes: {candidate_incomplete}")
if not compared or mismatches or candidate_incomplete or int(sys.argv[2]):
    sys.exit(1)
PY
