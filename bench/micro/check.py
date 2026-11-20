#!/usr/bin/env python3
"""The micro-benchmark programs as correctness tests: no timing, no Docker.

    bench/micro/check.py --okkama BIN [--pipeline oir2] [--jobs N] [--cage] [--stdlib DIR] [--only a,b]

Every program of manifest.json runs with N=17 and N=31 (warmup 3), as run.py's
pre-check does, and its checksums must equal the independent oracles of
oracles.py; stderr must be empty and the exit status 0. `--pipeline oir2`
runs Champ (a champ-abi build). The host runs the programs directly: use it
in CI, or with `--cage` (okkama-rs/tools/new-runtime/caged.py) on a personal
machine. Timings belong to run.py. Exit status 0 when every program passed.
"""
import argparse
import concurrent.futures
import json
import os
import re
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
CAGE = ROOT / "okkama-rs" / "tools" / "new-runtime" / "caged.py"
sys.path.insert(0, str(HERE))
from run import parse_sample  # noqa: E402

COUNTS = (17, 31)
WARMUP = 3


def check(name, source, args):
    command = [args.okkama, "run", "--timeout", str(args.timeout), str(source)]
    env = dict(os.environ, OKKAMA_STDLIB_DIR=args.stdlib, OKKAMA_TRACE="off")
    if args.pipeline:
        env["OKKAMA_PIPELINE"] = args.pipeline
    for n in COUNTS:
        run = command + [str(n), str(WARMUP)]
        if args.cage:
            run = [sys.executable, str(CAGE), "--cpu-seconds", str(args.timeout), "--timeout",
                   str(args.timeout + 30), "--memory-mb", "2048", "--"] + run
        try:
            p = subprocess.run(run, cwd=source.parent, env=env, capture_output=True, text=True,
                               timeout=args.timeout + 60)
        except subprocess.TimeoutExpired:
            return name, f"N={n}: timeout"
        last = p.stderr.strip().splitlines()[-1][:160] if p.stderr.strip() else ""
        if p.returncode != 0:
            return name, f"N={n}: exit {p.returncode}: {last}"
        if p.stderr:
            return name, f"N={n}: unexpected stderr: {last}"
        try:
            parse_sample(p.stdout, n, WARMUP, name)
        except ValueError as error:
            return name, f"N={n}: {error}: {p.stdout.strip()[:120]}"
    return name, None


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--okkama", required=True)
    parser.add_argument("--pipeline", choices=["oir2"], help="oir2: Champ (a champ-abi build)")
    parser.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) // 2))
    parser.add_argument("--timeout", type=int, default=60)
    parser.add_argument("--cage", action="store_true")
    parser.add_argument("--stdlib", default=str(ROOT / "std"), help="std of the binary (default: this checkout's)")
    parser.add_argument("--only", default="")
    args = parser.parse_args()
    args.okkama = os.path.abspath(args.okkama)
    if not os.access(args.okkama, os.X_OK):
        parser.error(f"no okkama binary at {args.okkama}")
    manifest = json.loads((HERE / "manifest.json").read_text())
    selected = sorted(args.only.split(",")) if args.only else sorted(manifest)
    unknown = [name for name in selected if name not in manifest]
    if unknown:
        parser.error("unknown benchmark: " + ", ".join(unknown))
    with concurrent.futures.ThreadPoolExecutor(args.jobs) as pool:
        results = list(pool.map(lambda name: check(name, HERE / manifest[name]["path"], args), selected))
    failed = [(name, problem) for name, problem in results if problem]
    for name, problem in results:
        print(f"{name} {'.' * (32 - len(name))} {'ok' if not problem else 'FAILED'}")
        if problem:
            print(f"    {problem}")
    print()
    if failed:
        # Numbers folded, so programs that stop at the same rule count together.
        reasons = {}
        for name, problem in failed:
            reasons.setdefault(re.sub(r"\d+", "N", problem), []).append(name)
        print("Failures by reason:")
        for reason, names in sorted(reasons.items(), key=lambda item: (-len(item[1]), item[0])):
            print(f"  {len(names):4}  {reason[:150]}")
            print(f"        {', '.join(names[:4])}{', ...' if len(names) > 4 else ''}")
        print()
        print(f"Failed {len(failed)}/{len(results)} programs")
    else:
        print("All programs match their oracles.")
    print(f"Programs={len(results)}, N={','.join(map(str, COUNTS))}, pipeline={args.pipeline or 'sparrow'}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
