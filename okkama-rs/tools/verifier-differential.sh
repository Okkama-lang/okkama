#!/bin/sh
# Reproduce the #634 precise/compact verifier gate across the OIR corpus.
set -eu
cd "$(dirname "$0")/.."
records=$(mktemp -d "${TMPDIR:-/tmp}/okkama-verifier-differential.XXXXXX")
trap 'rm -rf "$records"' EXIT HUP INT TERM
export OKKAMA_634_DIFFERENTIAL=1
export OKKAMA_634_RESULTS="$records/results.tsv"
export OKKAMA_634_RAISED="$records/raised.tsv"
status=0
cargo test --offline -j 4 -p okkama-oir --features verifier-differential || status=$?
if [ "$status" -eq 0 ]; then
    cargo test --offline -j 4 -p okkama-compiler --features verifier-differential --lib oir2 || status=$?
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
