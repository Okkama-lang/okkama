#!/bin/sh
# Champ conformance: the full-release profile on Sparrow and on Champ, then the
# blocker ranking. Meant for an isolated container (okkama-dev); offline.
#
#   conformance/tools/run_champ.sh REPO_ROOT OUT_DIR
#
# Environment:
#   OCS_JOBS         parallel cases (default 6)
#   OCS_PROFILE      profile (default full-release)
#   OCS_BASELINE     an existing Sparrow run directory to reuse instead of
#                    running Sparrow again (same profile and suite tree)
#   OCS_COMPARE      an earlier blockers.json to diff against
#   OCS_TARGET_DIR   cargo target root for both builds (default: the adapters'
#                    .target; Champ builds into its engine-champ subdirectory, gate profile)
#
# Writes OUT_DIR/sparrow/, OUT_DIR/champ/ (ocs run directories),
# OUT_DIR/blockers.json and OUT_DIR/blockers.txt. Exits non-zero only when a
# run or the ranking could not be produced, not because cases fail.
set -eu

if [ "$#" -ne 2 ]; then
    echo "usage: $0 REPO_ROOT OUT_DIR" >&2
    exit 2
fi
REPO=$(cd "$1" && pwd)
mkdir -p "$2"
OUT=$(cd "$2" && pwd)
SUITE="$REPO/conformance"
JOBS=${OCS_JOBS:-6}
PROFILE=${OCS_PROFILE:-full-release}

export CARGO_NET_OFFLINE=true

set -- --profile "$PROFILE" -j "$JOBS" --quiet
if [ -n "${OCS_TARGET_DIR:-}" ]; then
    set -- "$@" --adapter-env "OCS_RUST_TARGET_DIR=$OCS_TARGET_DIR"
fi

cd "$SUITE"

run_engine() {
    # ocs run exits 1 when some mandatory case does not pass: that is data
    # here. A missing results.jsonl is the real failure.
    name=$1
    adapter=$2
    shift 2
    rm -rf "$OUT/$name"
    echo "== $name: ./ocs run --adapter $adapter $*" >&2
    status=0
    ./ocs run --adapter "$adapter" --out "$OUT/$name" "$@" || status=$?
    if [ ! -f "$OUT/$name/results.jsonl" ]; then
        echo "run_champ: $name produced no results.jsonl (ocs exit $status)" >&2
        exit 1
    fi
}

if [ -n "${OCS_BASELINE:-}" ]; then
    BASELINE=$(cd "$OCS_BASELINE" && pwd)
    echo "== sparrow: reusing $BASELINE" >&2
else
    run_engine sparrow adapters/rust/adapter.json "$@"
    BASELINE="$OUT/sparrow"
fi
run_engine champ adapters/rust/adapter-champ.json "$@"

set -- "$OUT/champ" --baseline "$BASELINE" --json "$OUT/blockers.json"
if [ -n "${OCS_COMPARE:-}" ]; then
    set -- "$@" --compare "$OCS_COMPARE"
fi
python3 tools/champ_blockers.py "$@" > "$OUT/blockers.txt"
cat "$OUT/blockers.txt"
