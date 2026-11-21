#!/usr/bin/env python3
"""Ticket #329: five interleaved, checked process timings of the supplied 2M-call benchmark."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess
import time

ROOT = Path(__file__).resolve().parents[2]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("binary", type=Path)
parser.add_argument("--label", required=True)
parser.add_argument("--out", type=Path, required=True)
args = parser.parse_args()
env = os.environ.copy()
for name in ("OKKAMA_DEV", "OKKAMA_DEV_OUT", "SPARROW_CORE_STATS"):
    env.pop(name, None)
env["OKKAMA_STDLIB_DIR"] = str(ROOT / "std")
binary = str(args.binary.resolve())
benchmark = str(ROOT / "bench/ownership/guard-329.ok")


def run(mode):
    started = time.perf_counter_ns()
    result = subprocess.run([binary, "run", benchmark, mode], env=env,
                            capture_output=True, text=True, timeout=45, cwd=ROOT)
    elapsed = (time.perf_counter_ns() - started) / 1e9
    if result.returncode or result.stdout.strip() != "2000000":
        raise RuntimeError(f"{mode}: {result.returncode}: {result.stdout}\n{result.stderr}")
    return elapsed


# Warm file caches and both modes. Each measured process still compiles/JITs normally.
for mode in ("destroy", "close"):
    run(mode)
values = {"destroy": [], "close": []}
for index in range(5):
    for mode in (("destroy", "close") if index % 2 == 0 else ("close", "destroy")):
        elapsed = run(mode)
        values[mode].append(elapsed)
        print(f"{args.label} {mode} {index + 1}: {elapsed:.6f} s", flush=True)
record = {
    "binary_sha256": hashlib.sha256(Path(binary).read_bytes()).hexdigest(),
    "iterations": 2000000,
    "metric": "whole-process wall seconds, including frontend and JIT; no devtrace/stats",
    "runs": values,
    "median_seconds": {mode: statistics.median(times) for mode, times in values.items()},
}
data = json.loads(args.out.read_text()) if args.out.exists() else {}
data[args.label] = record
args.out.write_text(json.dumps(data, indent=2) + "\n")
print(json.dumps(record["median_seconds"]))
