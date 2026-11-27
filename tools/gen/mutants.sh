#!/usr/bin/env bash
# Self-test of the program generator (H-18): one seeded defect per engine in a
# scratch copy of the tree, built, then okgen.py must find a disagreement and
# minimize it. Runs many programs: use a container (`make gate`'s image).
#
#     tools/gen/mutants.sh [SEEDS]
#
# A defect in a helper both engines share is found against an unmutated build
# (OKKAMA_REFERENCE, OKKAMA: the release build of this tree by default); a defect
# in the JIT's emitter is found by the mutated build's own interpreter (engines
# sparrow,sparrow-interp, #1123), with no second build. MUTANTS_TARGET is where
# the mutated build goes (default okkama-rs/target/mutants), MUTANTS_OUT where
# the findings go (default conformance/reports/runs/okgen-mutants), and
# MUTANTS_ONLY=NAME runs one mutant.
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SEEDS="${1:-30}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

REFERENCE="${OKKAMA:-$ROOT/okkama-rs/target/release/okkama}"
TARGET="${MUTANTS_TARGET:-$ROOT/okkama-rs/target/mutants}"
OUT="${MUTANTS_OUT:-$ROOT/conformance/reports/runs/okgen-mutants}"

# name | file | text | defect | engines (empty: against the reference build)
MUTANTS=(
    "sparrow|okkama-rs/crates/sparrow-core/src/runtime.rs|            &*l * &*r|            &*l * &*r + 1|"
    "sparrow-jit|okkama-rs/crates/sparrow-core/src/jit.rs|(self.builder.ins().bor_imm_u(product, 1), overflow)|(self.builder.ins().bor_imm_u(product, 3), overflow)|sparrow,sparrow-interp"
)

failed=0
for mutant in "${MUTANTS[@]}"; do
    IFS='|' read -r name file text defect engines <<<"$mutant"
    [[ -n "${MUTANTS_ONLY:-}" && "$name" != "$MUTANTS_ONLY" ]] && continue
    echo "==> defect in $name: $file"
    rm -rf "$WORK/tree"
    rsync -a --exclude /okkama-rs/target --exclude /okkama-rs/fuzz --exclude .git --exclude /conformance/reports "$ROOT/" "$WORK/tree/"
    python3 - "$WORK/tree/$file" "$text" "$defect" <<'PY'
import sys
path, text, defect = sys.argv[1:]
source = open(path).read()
assert source.count(text) == 1, f"mutation point not unique in {path}"
open(path, "w").write(source.replace(text, defect))
PY
    (cd "$WORK/tree/okkama-rs" && CARGO_TARGET_DIR="$TARGET" cargo build --release -q -p okkama-cli) || {
        echo "!! build failed"; failed=1; continue; }
    out="$OUT/$name"
    rm -rf "$out"
    if [[ -n "$engines" ]]; then
        OKKAMA="$TARGET/release/okkama" python3 "$ROOT/tools/gen/okgen.py" run --seeds "$SEEDS" --engines "$engines" --out "$out" | tail -3
    else
        OKKAMA_REFERENCE="$REFERENCE" OKKAMA="$TARGET/release/okkama" python3 "$ROOT/tools/gen/okgen.py" run --seeds "$SEEDS" --out "$out" | tail -3
    fi
    if ls "$out"/*/minimized.ok >/dev/null 2>&1; then
        first="$(ls -d "$out"/*/ | head -1)"
        echo "found and minimized: $(grep -c 'out.writeln' "$first/minimized.ok") statement(s) in $first"
    else
        echo "!! the $name defect was not found in $SEEDS seeds"
        failed=1
    fi
done
exit $failed
