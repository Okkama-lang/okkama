#!/usr/bin/env python3
"""Tweed's ownership gate (#908): Okkama against CPython on the same work, in the same container.

    bench/ownership/run.py --okkama LINUX_BIN [--okkama OTHER_BIN ...] [--image okkama-dev] [--repeats 9]

Each `--okkama` is a Linux build of the CLI (for example built in Docker); it runs with this checkout's std,
and is labelled by its parent directory's name. Every engine runs in `IMAGE` with one
CPU, 2 GB and no network, CPython being the image's `python3`.

- **ownership.ok** (the 11-ownership ladder program: 100 000 records behind a role, 1000 shelves, 300 rounds,
  a destroy on one implementation). The median of `REPEATS` in-process repetitions, the first two dropped,
  for the program as is and for its **plain** twin (the `destroy` removed): their difference is the
  ownership tax. ownership.py is the CPython port.
- **Micro pairs** (bench/micro programs, micro-v2 protocol, oracles checked): baseline, constructor,
  successor, destroy, destroy-observed, defer. micro_pairs.py holds the CPython ports.

Timing belongs on a quiet machine: CI runners are bimodal, so run this locally. The target (owner,
2026-11-20): ownership.ok no slower than CPython.
"""
import argparse
import os
import statistics
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
MICRO = ROOT / "bench" / "micro"
sys.path.insert(0, str(MICRO))
from oracles import expected  # noqa: E402

PAIRS = ["baseline", "constructor", "successor", "destroy", "destroy-observed", "defer"]


def docker(image, mounts, command, timeout=300):
    args = ["docker", "run", "--rm", "--network", "none", "--cpus", "1", "--memory", "2g", "-w", "/tmp"]
    for source, target in mounts:
        args += ["-v", f"{source}:{target}:ro"]
    args += ["-e", "OKKAMA_STDLIB_DIR=/std", "-e", "OKKAMA_TRACE=off", image] + command
    result = subprocess.run(args, capture_output=True, text=True, timeout=timeout)
    if result.returncode != 0:
        raise RuntimeError(f"{' '.join(command)}: exit {result.returncode}: {result.stderr[-500:]}")
    return result.stdout, result.stderr


def ladder(image, mounts, command, repeats):
    stdout, stderr = docker(image, mounts, command + [str(repeats)])
    samples = [int(line.split()[1]) for line in stderr.splitlines() if line.startswith("iteration")]
    if len(samples) != repeats or stdout.strip() != "33576":
        raise RuntimeError(f"{command}: unexpected output {stdout.strip()!r}, {len(samples)} samples")
    return statistics.median(samples[2:]) / 1e6


def micro(image, mounts, command, name, n, warm):
    samples = []
    for _ in range(5):
        stdout, _ = docker(image, mounts, command + [str(n), str(warm)])
        fields = stdout.split()
        if not fields or fields[0] != "micro-v2" or int(fields[4]) != expected(name, n):
            raise RuntimeError(f"{name}: wrong protocol or checksum: {stdout.strip()!r}")
        samples.append(int(fields[3]) / n)
    return statistics.median(samples)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--okkama", action="append", required=True, help="a Linux okkama binary (repeatable)")
    parser.add_argument("--image", default="okkama-dev")
    parser.add_argument("--repeats", type=int, default=9)
    parser.add_argument("--n", type=int, default=1_000_000, help="micro iterations (CPython: at most 1M)")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="tweed-bench-") as work:
        plain = Path(work) / "plain.ok"
        plain.write_text((HERE / "ownership.ok").read_text().replace("    destroy { amount; }", ""))
        base = [(str(ROOT / "std"), "/std"), (str(HERE), "/bench"), (work, "/work"), (str(MICRO), "/micro")]
        engines = []
        for binary in args.okkama:
            binary = Path(binary).resolve()
            mounts = base + [(str(binary.parent), "/okkama")]
            run = ["/okkama/" + binary.name, "run"]
            engines.append((binary.parent.name, mounts, run))
        rows = {}
        for label, mounts, run in engines:
            rows[label] = {
                "ownership.ok": ladder(args.image, mounts, run + ["/bench/ownership.ok"], args.repeats),
                "plain twin": ladder(args.image, mounts, run + ["/work/plain.ok"], args.repeats),
            }
            for name in PAIRS:
                rows[label][name] = micro(args.image, mounts, run + [f"/micro/lang/{name}.ok"], name, args.n, args.n)
        python = {"ownership.ok": ladder(args.image, base, ["python3", "/bench/ownership.py"], args.repeats)}
        for name in PAIRS:
            n = min(args.n, 1_000_000)
            python[name] = micro(args.image, base, ["python3", "/bench/micro_pairs.py", name], name, n, n)
    labels = [label for label, _, _ in engines]
    print("| program | " + " | ".join(labels) + " | CPython |")
    print("|---|" + "---:|" * (len(labels) + 1))
    for key, unit in [("ownership.ok", "ms"), ("plain twin", "ms")] + [(name, "ns/it") for name in PAIRS]:
        cells = [f"{rows[label][key]:.1f}" for label in labels]
        cells.append(f"{python[key]:.1f}" if key in python else "—")
        print(f"| {key} ({unit}) | " + " | ".join(cells) + " |")
    worst = max(rows[label]["ownership.ok"] for label in labels[-1:])
    print(f"\nTarget (#903): ownership.ok of `{labels[-1]}` {worst:.1f} ms vs CPython {python['ownership.ok']:.1f} ms:"
          f" {'met' if worst <= python['ownership.ok'] else 'NOT met'}")


if __name__ == "__main__":
    main()
