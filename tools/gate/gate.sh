#!/usr/bin/env bash
# Ворота релиза (H-08): всё, что должно пройти перед выпуском версии.
#
#     tools/gate/gate.sh            # полный прогон (~15 минут)
#     tools/gate/gate.sh --quick    # без тестов Rust и clippy
#     make gate                     # в контейнере (tools/gate/docker.sh)
#
# Шаги:
#   1. release-сборка okkama и sparrow с fat LTO (провал — ворота
#      останавливаются), тесты Rust всего workspace (профиль gate —
#      оптимизации release без fat LTO, — и debug: глубина стека и
#      debug-проверки видны только там), clippy без предупреждений;
#   2. conformance на Sparrow, Sparrow без IR-проходов
#      (OKKAMA_IR_PASSES=none) и Sparrow со стресс-GC
#      (SPARROW_CORE_GC_STRESS=1);
#   3. актуальный перечень std в conformance; таблица ABI совпадает с
#      std/runtime, и у каждого натива есть вызывающий (#976, #1022);
#      std в форме `okkama fmt`
#      (fmt --check, #1006); встроенные тесты std на Sparrow;
#   4. t/ на sparrow, sparrow-passes-off, sparrow-gc-stress;
#      TAP-потоки конфигураций обязаны совпадать;
#   5. интерпретатор Sparrow (#1123): conformance sparrow-interp, t/ на
#      sparrow-interp и sparrow-interp-gc-stress, differ.py на examples/ —
#      пока только отчёт (informative): блокирующими станут после первого
#      зелёного прогона (дизайн #1079, решение 11).
#
# Статус каждого шага — код выхода его команды (ocs: 0 только если прошли все
# обязательные кейсы). Итог — gate.json в каталоге журналов: коммит, шаги со
# статусами и временем, известные провалы std. Process workers и сеть
# настоящие: гонять в контейнере или на CI-раннере.
#
# Журналы — в conformance/reports/runs/gate-<время>/. Код выхода 0 — всё прошло.
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
QUICK=0
[[ "${1:-}" == "--quick" ]] && QUICK=1
LOGS="$ROOT/conformance/reports/runs/gate-$(date +%Y%m%dT%H%M%S)"
mkdir -p "$LOGS"
OKKAMA="$ROOT/okkama-rs/target/release/okkama"
export OKKAMA_STDLIB_DIR="$ROOT/std"
FAILED=()
STEPS="$LOGS/steps.tsv"
: >"$STEPS"

step() { printf '\n==> %s\n' "$1"; }
fail() { FAILED+=("$1"); printf '!! %s\n' "$1"; }
# record NAME STATUS SECONDS: pass, fail or informative (a failure that does
# not fail the gate).
record() { printf '%s\t%s\t%s\n' "$1" "$2" "$3" >>"$STEPS"; }

# The summary for people and for the release workflow.
finish() {
    python3 - "$LOGS" "$ROOT" "${#FAILED[@]}" <<'EOF'
import json, os, subprocess, sys
logs, root, failed = sys.argv[1], sys.argv[2], int(sys.argv[3])
def git(*args):
    try:
        return subprocess.run(["git", "-C", root, *args], capture_output=True, text=True).stdout.strip() or None
    except OSError:
        return None
def toolchain():
    try:
        return subprocess.run(["rustc", "-V"], capture_output=True, text=True, cwd=os.path.join(root, "okkama-rs")).stdout.strip() or None
    except OSError:
        return None
steps = []
with open(os.path.join(logs, "steps.tsv")) as rows:
    for row in rows:
        name, status, seconds = row.rstrip("\n").split("\t")
        entry = {"name": name, "status": status, "seconds": int(seconds)}
        summary = os.path.join(logs, f"{name.replace(' ', '-')}", "summary.json")
        if os.path.exists(summary):
            mandatory = json.load(open(summary)).get("mandatory", {})
            entry["mandatory"] = {"total": mandatory.get("total"), "counts": mandatory.get("counts")}
        steps.append(entry)
known = []
std = os.path.join(logs, "std-tests.log")
if os.path.exists(std):
    known = [line.split("known:", 1)[1].strip() for line in open(std) if line.strip().startswith("known:")]
report = {
    "commit": os.environ.get("GITHUB_SHA") or os.environ.get("GATE_COMMIT") or git("rev-parse", "HEAD"),
    "toolchain": toolchain(),
    "passed": failed == 0,
    "steps": steps,
    "known_std_failures": known,
}
json.dump(report, open(os.path.join(logs, "gate.json"), "w"), indent=2)
EOF
    printf '\nlogs: %s\n' "$LOGS"
    if [[ ${#FAILED[@]} -gt 0 ]]; then
        printf 'GATE FAILED: %s\n' "${FAILED[*]}"
        exit 1
    fi
    echo "GATE PASSED"
    exit 0
}

step "build"
started=$SECONDS
if (cd "$ROOT/okkama-rs" && cargo build --release -p okkama-cli -p sparrow-cli >"$LOGS/build.log" 2>&1); then
    record build pass $((SECONDS - started))
    echo "$("$OKKAMA" --version)"
else
    tail -20 "$LOGS/build.log"
    record build fail $((SECONDS - started))
    fail "build"
    # Nothing later means anything without the binaries.
    finish
fi

if [[ $QUICK -eq 0 ]]; then
    # The gate profile: release optimizations without fat LTO, so the test
    # binaries do not each pay a full LTO link (Cargo.toml).
    step "Rust tests (optimized, workspace)"
    started=$SECONDS
    if (cd "$ROOT/okkama-rs" && cargo test --profile gate --workspace --no-fail-fast >"$LOGS/cargo-test.log" 2>&1); then
        record "rust tests" pass $((SECONDS - started))
        echo "ok: $(grep -c '^test result: ok' "$LOGS/cargo-test.log") test binaries"
    else
        record "rust tests" fail $((SECONDS - started))
        grep -E "^test .* FAILED|panicked|^error" "$LOGS/cargo-test.log" | head -20
        fail "Rust tests"
    fi
fi

if [[ $QUICK -eq 0 ]]; then
    # Debug frames are larger and debug assertions are on: stack depth (drops
    # of deep value graphs, #28) and invariants fail here first (#42).
    step "Rust tests (debug, workspace)"
    started=$SECONDS
    if (cd "$ROOT/okkama-rs" && cargo test --workspace --no-fail-fast >"$LOGS/cargo-test-debug.log" 2>&1); then
        record "rust tests debug" pass $((SECONDS - started))
        echo "ok: $(grep -c '^test result: ok' "$LOGS/cargo-test-debug.log") test binaries"
    else
        record "rust tests debug" fail $((SECONDS - started))
        grep -E "^test .* FAILED|panicked|^error|SIGABRT|overflowed its stack" "$LOGS/cargo-test-debug.log" | head -20
        fail "Rust tests (debug)"
    fi

    step "clippy"
    started=$SECONDS
    if (cd "$ROOT/okkama-rs" && cargo clippy --workspace --all-targets -- -D warnings >"$LOGS/clippy.log" 2>&1); then
        record clippy pass $((SECONDS - started))
    else
        record clippy fail $((SECONDS - started))
        grep -E "^(warning|error)" -A4 "$LOGS/clippy.log" | head -30
        fail "clippy"
    fi
fi

# report NAME COMMAND...: a step that informs and does not fail the gate (the interpreter's, #1123).
report() {
    local name="$1"; shift
    started=$SECONDS
    if "$@" >"$LOGS/$name.log" 2>&1; then
        record "$name" pass $((SECONDS - started))
    else
        record "$name" informative $((SECONDS - started))
        printf '.. %s did not pass (informative): %s\n' "$name" "$(tail -1 "$LOGS/$name.log")"
    fi
}

conformance() {
    local name="$1"; shift
    local out="$LOGS/conformance-$name"
    started=$SECONDS
    (cd "$ROOT/conformance" && ./ocs run --adapter adapters/rust/adapter.json --profile full-release \
        --stdlib-root ../std -j 6 --out "$out" "$@" >"$LOGS/conformance-$name.log" 2>&1)
    local status=$?
    echo "$name: $(tail -1 "$LOGS/conformance-$name.log")"
    if [[ $status -eq 0 ]]; then
        record "conformance $name" pass $((SECONDS - started))
        return
    fi
    grep -E "^\[.*(FAIL|ERROR|CRASH|UNSUPPORTED|TIMEOUT)" "$LOGS/conformance-$name.log" | head -10
    record "conformance $name" fail $((SECONDS - started))
    fail "conformance $name"
}

step "conformance"
conformance sparrow --adapter-env OCS_RUST_ENGINE=sparrow
# Sparrow also runs OIR with the passes off: a second configuration
# shows a pass that changes meaning (#29).
# Without the passes tail calls are real calls: L03.recursion.tail-constant-stack
# makes 10^7 of them in the adapter's debug build, past its budget on CI.
conformance sparrow-passes-off --timeout-scale 4 --adapter-env OCS_RUST_ENGINE=sparrow --adapter-env OKKAMA_IR_PASSES=none
# Sparrow collects at every safepoint and poisons freed cells: a lost or
# wrong root fails at once (#356; it found #355).
conformance sparrow-gc-stress --timeout-scale 6 --adapter-env OCS_RUST_ENGINE=sparrow --adapter-env SPARROW_CORE_GC_STRESS=1

# The std inventory is generated from std/: a stale one hides new modules
# from the coverage report (#282).
step "std inventory"
started=$SECONDS
if (cd "$ROOT/conformance" && python3 tools/inventory_std.py ../std --check >"$LOGS/std-inventory.log" 2>&1); then
    record "std inventory" pass $((SECONDS - started))
else
    record "std inventory" fail $((SECONDS - started))
    tail -3 "$LOGS/std-inventory.log"
    fail "std inventory"
fi

# The native ABI table matches the declarations of std/runtime (#976), and
# every native there has a caller in std or the runtime crates (#1022).
step "natives"
started=$SECONDS
if { python3 "$ROOT/okkama-rs/tools/engines/gen-abi.py" --check \
        && python3 "$ROOT/okkama-rs/tools/engines/native-callers.py"; } >"$LOGS/natives.log" 2>&1; then
    record "natives" pass $((SECONDS - started))
else
    record "natives" fail $((SECONDS - started))
    tail -5 "$LOGS/natives.log"
    fail "natives"
fi

# Every std file is in the shape `okkama fmt` gives it (#1006), generated ones included: their
# generators run their output through the formatter (tools/okkama_fmt.py).
step "std fmt"
started=$SECONDS
if find "$ROOT/std" -name '*.ok' -print0 | sort -z | xargs -0 "$OKKAMA" fmt --check >"$LOGS/std-fmt.log" 2>&1; then
    record "std fmt" pass $((SECONDS - started))
else
    record "std fmt" fail $((SECONDS - started))
    echo "not formatted ($(wc -l <"$LOGS/std-fmt.log" | tr -d ' ') files; okkama fmt -w FILE...):"
    head -20 "$LOGS/std-fmt.log"
    fail "std fmt"
fi

step "std inline tests"
started=$SECONDS
# Every std module compiled to Sparrow also passes the independent register
# allocation check (#30).
if OKKAMA_CHECK_REGISTERS=1 python3 "$ROOT/tools/gate/std-tests.py" | tee "$LOGS/std-tests.log"; [[ ${PIPESTATUS[0]} -eq 0 ]]; then
    record "std tests" pass $((SECONDS - started))
else
    record "std tests" fail $((SECONDS - started))
    fail "std inline tests"
fi

step "t/ suite"
started=$SECONDS
if python3 "$ROOT/tools/t/run.py" --configs sparrow,sparrow-passes-off,sparrow-gc-stress --okkama "$OKKAMA" \
        | tee "$LOGS/t-suite.log"; [[ ${PIPESTATUS[0]} -eq 0 ]]; then
    record "t suite" pass $((SECONDS - started))
else
    record "t suite" fail $((SECONDS - started))
    fail "t/ suite"
fi

# Sparrow's interpreter against the JIT (#1123): the same cases, files and programs; a difference that
# okkama-rs/tools/engines/interp-exceptions.txt does not explain is a defect. Informative until its first
# green run.
step "interpreter"
report conformance-sparrow-interp sh -c "cd '$ROOT/conformance' && ./ocs run --adapter adapters/rust/adapter.json \
    --profile full-release --stdlib-root ../std -j 6 --timeout-scale 4 --out '$LOGS/conformance-sparrow-interp' \
    --adapter-env OCS_RUST_ENGINE=sparrow --adapter-env SPARROW_CORE_INTERPRET=1 >/dev/null; \
    python3 '$ROOT/okkama-rs/tools/engines/differ.py' ocs '$LOGS/conformance-sparrow-interp/results.jsonl'"
report t-sparrow-interp python3 "$ROOT/tools/t/run.py" --no-generate --configs sparrow,sparrow-interp,sparrow-interp-gc-stress \
    --okkama "$OKKAMA"
report differ-examples python3 "$ROOT/okkama-rs/tools/engines/differ.py" run --okkama "$OKKAMA" "$ROOT/examples"

finish
