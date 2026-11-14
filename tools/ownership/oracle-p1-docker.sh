#!/usr/bin/env bash
# Build two isolated toolchains, then run only the explicit P1 manifest in bounded Docker.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
RESULTS="${ORACLE_RESULTS:-$ROOT/tools/ownership/results}"
BASELINE="$(git -C "$ROOT" rev-parse "${ORACLE_BASELINE:-34342aa6}^{commit}")"
CANDIDATE="$(git -C "$ROOT" rev-parse HEAD)"
mkdir -p "$RESULTS"
RESULTS="$(cd "$RESULTS" && pwd)"
# A worktree's .git points outside /src. Freeze the baseline on the host without executing it.
git -C "$ROOT" archive --format=tar "$BASELINE" > "$RESULTS/p1-baseline.tar"
git -C "$ROOT" diff --binary HEAD > "$RESULTS/p1-candidate.patch"
exec docker run --rm --network none --cpus "${ORACLE_CPUS:-4}" \
    --memory "${ORACLE_MEMORY:-7g}" --pids-limit 512 \
    -e LANG=C.UTF-8 -e LC_ALL=C.UTF-8 \
    -e ORACLE_BUILD_TIMEOUT="${ORACLE_BUILD_TIMEOUT:-1800}" \
    -v "$ROOT:/src:ro" -v "$RESULTS:/out" \
    -v okkama-cargo-registry:/usr/local/cargo/registry \
    -w /out okkama-dev bash -c '
        set -euo pipefail
        baseline=$1
        candidate=$2
        shift 2
        mkdir -p /work/a /work/b /tc-a/bin /tc-b/bin /tc-a/lib/okkama /tc-b/lib/okkama
        tar -xf /out/p1-baseline.tar -C /work/a
        rsync -a --exclude .git --exclude target --exclude results /src/ /work/b/
        tar -C /work/b -cf /out/p1-candidate-source.tar .
        sha256sum /out/p1-candidate-source.tar > /out/p1-candidate-source.sha256
        for side in a b; do
            cd "/work/$side/okkama-rs"
            features=()
            if [[ "$side" == b ]]; then
                features=(--features okkama-cli/champ-abi,sparrow-cli/champ-abi)
            fi
            CARGO_TARGET_DIR="/work/target-$side" timeout --kill-after=10s "$ORACLE_BUILD_TIMEOUT" \
                cargo build --offline -j 4 --release -p okkama-cli -p sparrow-cli "${features[@]}"
            cp "/work/target-$side/release/okkama" "/work/target-$side/release/sparrow" "/tc-$side/bin/"
            cp -a "/work/$side/std" "/tc-$side/lib/okkama/std"
        done
        cd /out
        exec python3 -B /src/tools/ownership/oracle.py two-toolchains \
            --root /src --manifest /src/tools/ownership/p1-manifest.json \
            --a-toolchain /tc-a --a-revision "$baseline" --a-spb-version 16 \
            --b-toolchain /tc-b --b-revision "$candidate+working-tree" --b-spb-version 17 \
            --b-env OKKAMA_PIPELINE=oir2 --a-env OKKAMA_TRACE=off --b-env OKKAMA_TRACE=off \
            --out /out/p1-artifacts --json /out/p1.json "$@"
    ' oracle "$BASELINE" "$CANDIDATE" "$@"
