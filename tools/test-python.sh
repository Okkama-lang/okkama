#!/usr/bin/env bash
# Every Python unit test of the repository's tooling: each tracked test_*.py
# (tools/fuzz, tools/t, tools/ownership, bench/micro, conformance/runner-tests …),
# run as a script from the repository root (#1092). They are dry regressions:
# no toolchain, nightly, Docker or network. CI (ci.yml, job "Tool tests") and
# the Wave (maps job) run this on every change; a new test_*.py is picked up
# without editing a workflow.
#
#     tools/test-python.sh
#
# Exit status 1 when any test file fails; each failure is named at the end.
set -uo pipefail
cd "$(dirname "$0")/.."
failed=()
count=0
while IFS= read -r -d '' test; do
    count=$((count + 1))
    echo "==> $test"
    python3 "$test" || failed+=("$test")
done < <(git ls-files -z -- ':(glob)**/test_*.py')
if [[ $count -eq 0 ]]; then
    echo "no test_*.py found" >&2
    exit 1
fi
if [[ ${#failed[@]} -gt 0 ]]; then
    echo "FAILED: ${failed[*]}" >&2
    exit 1
fi
echo "$count Python test files passed"
