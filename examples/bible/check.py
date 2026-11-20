#!/usr/bin/env python3
"""The Okkama Bible's examples: every NAME.ok against NAME.expected.

    examples/bible/check.py --okkama BIN [--jobs N] [--cage] [--stdlib DIR] [NAME ...]

The examples are copied from the Bible's sources (docs/OkkamaBible/examples on
the Bible's branch, where build.py --check runs them for the book); this copy
runs them on every engine in CI. The expected text is the stdout, then
`exit N` when the status is not 0. A `-tests` example runs with `-t` (its
report is the output); a `-rejected` example is compared by its diagnostic
(stderr), with this folder's path removed. Subfolders and data files are what
the examples import or read; each example runs in its own copy of the folder,
so examples that write files can run in parallel. On a personal machine use
`--cage`. Exit status 0 when
every example printed what it should.
"""
import argparse
import concurrent.futures
import os
import re
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
CAGE = os.path.join(ROOT, "okkama-rs", "tools", "new-runtime", "caged.py")


def check(stem, args):
    expected = open(os.path.join(HERE, stem + ".expected")).read().rstrip("\n").splitlines()
    with tempfile.TemporaryDirectory(prefix="bible-") as scratch:
        work = os.path.join(scratch, "examples")
        shutil.copytree(HERE, work, ignore=shutil.ignore_patterns("check.py", "__pycache__"))
        tests = ["-t"] if stem.endswith("-tests") else []
        command = [args.okkama, "run", *tests, stem + ".ok"]
        if args.cage:
            command = [sys.executable, CAGE, "--cpu-seconds", str(args.timeout), "--timeout", str(args.timeout + 30),
                       "--memory-mb", "1024", "--writable", work, "--"] + command
        env = dict(os.environ, OKKAMA_STDLIB_DIR=args.stdlib, OKKAMA_TRACE="off")
        try:
            p = subprocess.run(command, cwd=work, env=env, capture_output=True, text=True, timeout=args.timeout + 60)
        except subprocess.TimeoutExpired:
            return stem, "timeout", ""
    rejected = stem.endswith("-rejected")
    shown = (p.stderr if rejected else p.stdout).replace(work + os.sep, "")
    output = shown.rstrip("\n").splitlines() if shown else []
    if p.returncode != 0 and not rejected:
        output.append(f"exit {p.returncode}")
    if output == expected:
        return stem, None, ""
    last = p.stderr.strip().splitlines()[-1][:160] if p.stderr.strip() and not rejected else ""
    reason = f"exit {p.returncode}: {last}" if p.returncode != 0 and not rejected else "wrong output"
    return stem, reason, f"printed {output[-6:]}, expected {expected[-6:]}"


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("names", nargs="*")
    parser.add_argument("--okkama", required=True)
    parser.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) // 2))
    parser.add_argument("--timeout", type=int, default=60)
    parser.add_argument("--cage", action="store_true")
    parser.add_argument("--stdlib", default=os.path.join(ROOT, "std"),
                        help="std of the binary (default: this checkout's)")
    args = parser.parse_args()
    args.okkama = os.path.abspath(args.okkama)
    if not os.access(args.okkama, os.X_OK):
        parser.error(f"no okkama binary at {args.okkama}")
    stems = sorted(name[:-3] for name in os.listdir(HERE) if name.endswith(".ok"))
    missing = [stem for stem in stems if not os.path.exists(os.path.join(HERE, stem + ".expected"))]
    if missing:
        parser.error("examples without .expected: " + ", ".join(missing))
    if args.names:
        unknown = [name for name in args.names if name not in stems]
        if unknown:
            parser.error("unknown examples: " + ", ".join(unknown))
        stems = sorted(args.names)
    with concurrent.futures.ThreadPoolExecutor(args.jobs) as pool:
        results = list(pool.map(lambda stem: check(stem, args), stems))
    failed = [(stem, reason) for stem, reason, _ in results if reason]
    for stem, reason, detail in results:
        print(f"{stem} {'.' * (40 - len(stem))} {'ok' if not reason else 'FAILED'}")
        if reason:
            print(f"    {reason}\n    {detail[:400]}")
    print()
    if failed:
        # Numbers folded, so examples that stop at the same rule count together.
        reasons = {}
        for stem, reason in failed:
            reasons.setdefault(re.sub(r"\d+", "N", reason), []).append(stem)
        print("Failures by reason:")
        for reason, names in sorted(reasons.items(), key=lambda item: (-len(item[1]), item[0])):
            print(f"  {len(names):4}  {reason[:150]}")
            print(f"        {', '.join(names[:4])}{', ...' if len(names) > 4 else ''}")
        print()
        print(f"Failed {len(failed)}/{len(results)} examples")
    else:
        print("All examples print what the Bible says.")
    print(f"Examples={len(results)}, engine=sparrow")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
