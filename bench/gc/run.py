#!/usr/bin/env python3
"""The collector's benchmark set (#917): the same GC workloads, measured the same way, for one or more builds.

    bench/gc/run.py --okkama NAME=BIN [--okkama NAME=BIN ...] [--repeats 10] [--processes 5]
                    [--only bigint,tiny] [--out results.json]

Workloads (each takes the #236 protocol: an argument N repeats the work N times in one process and prints
"iteration <nanoseconds>" to stderr per run):
- ownership: bench/ownership/ownership.ok, a large long-lived heap (100 000 records, 1000 shelves);
- bigint: big-integer churn, collections triggered by owned Rust memory over a tiny live heap;
- tiny: a million short-lived objects per run in a loop, every tenth a resource with a destroy;
- deep: the same objects by non-tail recursion, ~100 000 frames for every collection's root scan;
- burst: a 200 000-element structure built and dropped, then a trickle of small allocations (#522);
- payload: maps built by updates and joined text, payload cells that own Rust memory.

Three passes per workload and build, all on this machine (timing belongs on a quiet one; CI runners are
bimodal):
1. timing: PROCESSES runs in a shuffled order across builds; per run the median of the second half of its
   REPEATS iterations; reported as the median and IQR over runs, with peak RSS and, on macOS, retired
   instructions per run (`/usr/bin/time -l`);
2. collector: one run with OKKAMA_DEV=gc (development instrumentation, so the build must not be compiled with
   `--cfg okkama_release`): collections, total and longest collect, mark and sweep, cells the sweeps visited;
3. bytes: one run with SPARROW_CORE_STATS=1 for the heap's live and owned bytes. Its census distorts times
   (#918), so only byte counts are taken from it.

Each NAME=BIN is an okkama CLI; it runs with this checkout's std unless --stdlib says otherwise.
"""
import argparse
import json
import os
import platform
import random
import re
import statistics
import subprocess
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
WORKLOADS = {
    "ownership": ROOT / "bench" / "ownership" / "ownership.ok",
    "bigint": HERE / "bigint.ok",
    "tiny": HERE / "tiny.ok",
    "deep": HERE / "deep.ok",
    "burst": HERE / "burst.ok",
    "payload": HERE / "payload.ok",
}
TIME = "/usr/bin/time"


def run(binary, program, repeats, stdlib, extra_env=None, measure=True, timeout=600):
    env = {**os.environ, "OKKAMA_STDLIB_DIR": str(stdlib), "OKKAMA_TRACE": "off"}
    env.pop("OKKAMA_DEV", None)
    env.pop("SPARROW_CORE_STATS", None)
    env.update(extra_env or {})
    command = [str(binary), "run", str(program), str(repeats)]
    macos = platform.system() == "Darwin"
    if measure and macos:
        command = [TIME, "-l"] + command
    started = time.monotonic()
    p = subprocess.run(command, capture_output=True, text=True, env=env, timeout=timeout)
    wall = time.monotonic() - started
    if p.returncode != 0:
        raise SystemExit(f"{binary} {program.name}: exit {p.returncode}\n{p.stderr[-800:]}")
    iterations = [int(m) for m in re.findall(r"^iteration (\d+)$", p.stderr, re.M)]
    out = {"iterations": iterations, "stderr": p.stderr, "stdout": p.stdout.strip(), "wall": wall}
    if measure and macos:
        rss = re.search(r"^\s*(\d+)\s+maximum resident set size", p.stderr, re.M)
        instructions = re.search(r"^\s*(\d+)\s+instructions retired", p.stderr, re.M)
        out["rss"] = int(rss.group(1)) if rss else None
        out["instructions"] = int(instructions.group(1)) if instructions else None
    return out


def steady(iterations):
    second = iterations[len(iterations) // 2:] or iterations
    return statistics.median(second) / 1e6


def quartiles(values):
    if len(values) < 2:
        return values[0], values[0], values[0]
    q = statistics.quantiles(values, n=4, method="inclusive")
    return q[0], statistics.median(values), q[2]


def collector(binary, program, repeats, stdlib):
    with tempfile.TemporaryDirectory() as directory:
        run(binary, program, repeats, stdlib, {"OKKAMA_DEV": "gc", "OKKAMA_DEV_OUT": directory}, measure=False)
        path = Path(directory) / "counters.json"
        if not path.exists():
            return None
        data = json.loads(path.read_text())
    timers = data.get("timers_ms", {})
    counters = data.get("counters", {})

    def timer(name):
        entry = timers.get(f"gc.{name}", {})
        return {"total_ms": entry.get("total", 0.0), "max_ms": entry.get("max", 0.0), "count": entry.get("count", 0)}

    collect = timer("collect")
    return {
        "collections": collect["count"],
        "collect": collect,
        "mark": timer("mark trace"),
        "mark_roots": timer("mark roots"),
        "sweep": timer("sweep"),
        "sweep_cells_visited": counters.get("gc.sweep cells visited", 0),
        "sweep_cells_full_pages": counters.get("gc.sweep cells in full pages", 0),
        "sweep_cells_empty_pages": counters.get("gc.sweep cells in empty pages", 0),
    }


def bytes_pass(binary, program, repeats, stdlib):
    p = run(binary, program, repeats, stdlib, {"SPARROW_CORE_STATS": "1"}, measure=False)
    wanted = {
        "heap live bytes, mean after collection": "live_mean_bytes",
        "owned Rust bytes live after the last collection, estimate": "external_live_bytes",
        "heap bytes allocated": "allocated_bytes",
        "collections triggered by owned Rust memory": "collections_by_owned_memory",
    }
    out = {}
    for line in p["stderr"].splitlines():
        m = re.match(r"^\s*(\d+)\s+(.*\S)\s*$", line)
        if m and m.group(2) in wanted:
            out[wanted[m.group(2)]] = int(m.group(1))
    return out


def other(c):
    """Time inside a collection that is neither marking nor sweeping (what the runtime does around them)."""
    if not c:
        return 0.0
    return c["collect"]["total_ms"] - c["mark"]["total_ms"] - c["mark_roots"]["total_ms"] - c["sweep"]["total_ms"]


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--okkama", action="append", required=True, help="NAME=BIN, repeatable")
    parser.add_argument("--repeats", type=int, default=10, help="iterations per process")
    parser.add_argument("--processes", type=int, default=5, help="timing runs per workload and build")
    parser.add_argument("--only", default="", help="comma-separated workload names")
    parser.add_argument("--stdlib", type=Path, default=ROOT / "std")
    parser.add_argument("--seed", type=int, default=None)
    parser.add_argument("--out", type=Path, default=None, help="JSON results (default: print only)")
    args = parser.parse_args()

    builds = {}
    for item in args.okkama:
        name, _, binary = item.partition("=")
        if not binary:
            raise SystemExit(f"--okkama wants NAME=BIN, got {item!r}")
        builds[name] = Path(binary).resolve()
    names = [n for n in args.only.split(",") if n] or list(WORKLOADS)
    unknown = [n for n in names if n not in WORKLOADS]
    if unknown:
        raise SystemExit(f"unknown workloads: {unknown}; known: {list(WORKLOADS)}")
    seed = args.seed if args.seed is not None else random.randrange(1 << 32)
    rng = random.Random(seed)

    # 1. timing, shuffled across builds and workloads
    order = [(w, b) for w in names for b in builds for _ in range(args.processes)]
    rng.shuffle(order)
    samples = {(w, b): [] for w in names for b in builds}
    outputs = {}
    for index, (workload, build) in enumerate(order, 1):
        result = run(builds[build], WORKLOADS[workload], args.repeats, args.stdlib)
        samples[(workload, build)].append(result)
        outputs.setdefault(workload, set()).add(result["stdout"])
        print(f"\r  timing {index}/{len(order)}", end="", flush=True)
    print()
    for workload, values in outputs.items():
        if len(values) != 1:
            raise SystemExit(f"{workload}: builds disagree on the result: {sorted(values)}")

    report = {"schema": "okkama-gc-bench/1", "seed": seed, "repeats": args.repeats, "processes": args.processes,
              "machine": platform.platform(), "builds": {k: str(v) for k, v in builds.items()}, "workloads": {}}
    for workload in names:
        entry = {}
        for build in builds:
            runs = samples[(workload, build)]
            times = [steady(r["iterations"]) for r in runs]
            low, median, high = quartiles(times)
            rss = [r.get("rss") for r in runs if r.get("rss")]
            instructions = [r.get("instructions") for r in runs if r.get("instructions")]
            entry[build] = {
                "steady_ms": {"median": median, "q1": low, "q3": high, "runs": times},
                "peak_rss_bytes": max(rss) if rss else None,
                "instructions_per_run": statistics.median(instructions) if instructions else None,
                "collector": collector(builds[build], WORKLOADS[workload], args.repeats, args.stdlib),
                "bytes": bytes_pass(builds[build], WORKLOADS[workload], args.repeats, args.stdlib),
            }
        report["workloads"][workload] = entry

    # table
    header = (f"{'workload':10} {'build':12} {'steady ms':>16} {'GCs':>5} {'collect ms':>11} {'max':>7} "
              f"{'roots ms':>9} {'mark ms':>8} {'sweep ms':>9} {'other ms':>9} {'RSS MB':>7}")
    print(header)
    for workload in names:
        for build in builds:
            e = report["workloads"][workload][build]
            s = e["steady_ms"]
            c = e["collector"] or {}
            rss = e["peak_rss_bytes"]
            print(f"{workload:10} {build:12} {s['median']:8.2f} ±{(s['q3'] - s['q1']) / 2:6.2f} "
                  f"{c.get('collections', 0):5} {c.get('collect', {}).get('total_ms', 0):11.2f} "
                  f"{c.get('collect', {}).get('max_ms', 0):7.2f} {c.get('mark_roots', {}).get('total_ms', 0):9.2f} "
                  f"{c.get('mark', {}).get('total_ms', 0):8.2f} "
                  f"{c.get('sweep', {}).get('total_ms', 0):9.2f} {other(c):9.2f} {(rss or 0) / 2**20:7.1f}")
    if args.out:
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(json.dumps(report, indent=1))
        print(f"results: {args.out}")


if __name__ == "__main__":
    main()
