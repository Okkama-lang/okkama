#!/usr/bin/env bash
# Usage: oracle-docker.sh MODE [--results HOST_DIR] [--bench HOST_PATH] [oracle options]
# Requires the existing gate image and a populated offline cargo registry volume.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
if [[ $# == 0 || "$1" == --help || "$1" == -h ]]; then
    echo 'Usage: oracle-docker.sh {engines|ab|stats|tap|two-toolchains} [--results HOST_DIR] [--bench HOST_PATH] [oracle options]'
    echo 'two-toolchains requires --a-toolchain HOST_PREFIX --b-toolchain HOST_PREFIX (installed, no builds).'
    echo 'Default results: tools/ownership/results; oracle --json paths are inside /out.'
    exit 0
fi
MODE=$1; shift
case "$MODE" in engines|ab|stats|tap|two-toolchains) ;; *) echo "Unknown mode: $MODE" >&2; exit 2 ;; esac
RESULTS="$ROOT/tools/ownership/results"
BENCH=
A_TOOLCHAIN=
B_TOOLCHAIN=
ARGS=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --a-toolchain|--b-toolchain)
            [[ "$MODE" == two-toolchains && $# -ge 2 && -d "$2" ]] || {
                echo "$1 requires two-toolchains and an installed prefix directory" >&2; exit 2;
            }
            PREFIX="$(cd "$2" && pwd)"
            if [[ "$1" == --a-toolchain ]]; then A_TOOLCHAIN=$PREFIX; else B_TOOLCHAIN=$PREFIX; fi
            shift 2 ;;
        --a-toolchain=*|--b-toolchain=*)
            echo 'Use --a-toolchain HOST_PREFIX --b-toolchain HOST_PREFIX' >&2; exit 2 ;;
        --results|--bench)
            [[ $# -ge 2 && -n "$2" ]] || { echo "Missing value for $1" >&2; exit 2; }
            if [[ "$1" == --results ]]; then RESULTS=$2; else BENCH=$2; fi
            shift 2 ;;
        --root|--root=*|--toolchain|--toolchain=*|--allow-host)
            echo "$1 is managed by the Docker wrapper" >&2; exit 2 ;;
        *) ARGS+=("$1"); shift ;;
    esac
done
mkdir -p "$RESULTS"
RESULTS="$(cd "$RESULTS" && pwd)"
MOUNTS=()
if [[ "$MODE" == two-toolchains ]]; then
    [[ -n "$A_TOOLCHAIN" && -n "$B_TOOLCHAIN" ]] || {
        echo 'two-toolchains requires both installed toolchain prefixes' >&2; exit 2;
    }
    MOUNTS+=(-v "$A_TOOLCHAIN:/tc-a:ro" -v "$B_TOOLCHAIN:/tc-b:ro")
    ARGS+=(--a-toolchain /tc-a --b-toolchain /tc-b)
fi
if [[ -n "$BENCH" ]]; then
    if [[ -d "$BENCH" ]]; then
        BENCH="$(cd "$BENCH" && pwd)"
        MOUNTS+=(-v "$BENCH:/bench:ro")
        ARGS+=(--bench /bench)
    elif [[ -f "$BENCH" ]]; then
        BENCH_DIR="$(cd "$(dirname "$BENCH")" && pwd)"
        MOUNTS+=(-v "$BENCH_DIR:/bench:ro")
        ARGS+=(--bench "/bench/$(basename "$BENCH")")
    else
        echo "Benchmark path does not exist: $BENCH" >&2; exit 2
    fi
fi
exec docker run --rm --network none \
    --cpus "${ORACLE_CPUS:-3}" --memory "${ORACLE_MEMORY:-6g}" --pids-limit 512 \
    -e LANG=C.UTF-8 -e LC_ALL=C.UTF-8 \
    -e CARGO_BUILD_JOBS="${ORACLE_CARGO_JOBS:-2}" \
    -e ORACLE_BUILD_TIMEOUT="${ORACLE_BUILD_TIMEOUT:-1800}" \
    -v "$ROOT:/src:ro" -v "$RESULTS:/out" \
    -v okkama-cargo-registry:/usr/local/cargo/registry \
    ${MOUNTS[@]+"${MOUNTS[@]}"} -w /out okkama-dev bash -c '
        set -euo pipefail
        if [[ "$1" == two-toolchains ]]; then
            exec python3 -B /src/tools/ownership/oracle.py "$@" --root /src
        fi
        /src/tools/ownership/toolchain.sh
        exec python3 /work/tools/ownership/oracle.py "$@" --root /work --toolchain /tc
    ' oracle "$MODE" --out /out/artifacts --json "/out/$MODE.json" ${ARGS[@]+"${ARGS[@]}"}
