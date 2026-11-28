#!/usr/bin/env python3
"""Sparrow's JIT against its interpreter on programs without expectations (#1123).

    okkama-rs/tools/engines/differ.py run [--okkama BIN] [--jobs N] [--cage] [--timeout S] [PATH ...]
    okkama-rs/tools/engines/differ.py ocs RESULTS.jsonl
    okkama-rs/tools/engines/differ.py check

`run` builds every program once (`okkama build`), runs the SPB with `okkama exec` on the JIT and with
SPARROW_CORE_INTERPRET=1, and compares standard output, standard error and the exit status byte for byte. PATH
is a file or a directory of .ok files; the default is examples/ without its archive. A difference is a defect
unless a line of interp-exceptions.txt (next to this file) explains it; the summary names each exception used.
`ocs` reads the results of a conformance run on the interpreter (`--adapter-env SPARROW_CORE_INTERPRET=1`): the
interpreter runs every program (#1122), so a case that did not pass is unexplained. `check` validates the
exceptions file. Exit status 0 when nothing is unexplained; `run` also fails when no program built (#1144), so a
broken build cannot pass.

The same exceptions serve tools/t/run.py's interpreter configurations (`load`, `excepted`) and the fuzz exec
target's differential mode (okkama-rs/fuzz/fuzz_targets/support/linking.rs reads the file at build time, #1144).
"""
import argparse
import concurrent.futures
import fnmatch
import json
import os
import re
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(os.path.dirname(HERE)))
EXCEPTIONS = os.path.join(HERE, "interp-exceptions.txt")
OKKAMA = os.path.join(ROOT, "okkama-rs", "target", "release", "okkama")
CAGE = os.path.join(ROOT, "okkama-rs", "tools", "new-runtime", "caged.py")
KINDS = ("both", "either", "jit", "path")


def load(path=EXCEPTIONS):
    """The exceptions: (kind, text, reason) in file order. A malformed line is an error."""
    rules = []
    for number, line in enumerate(open(path, encoding="utf-8"), 1):
        line = line.rstrip("\n")
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        parts = re.split(r"\t+| {2,}", line.strip(), maxsplit=1)
        if len(parts) != 2 or ":" not in parts[0]:
            raise ValueError(f"{path}:{number}: expected `SELECTOR  REASON`")
        kind, text = parts[0].split(":", 1)
        reason = parts[1].strip()
        if kind not in KINDS or not text:
            raise ValueError(f"{path}:{number}: unknown selector `{parts[0]}` (kinds: {', '.join(KINDS)})")
        if not re.search(r"#\d+|6\.3", reason):
            raise ValueError(f"{path}:{number}: the reason names no ticket (#NNNN) or design section (6.3)")
        rules.append((kind, text, reason))
    return rules


def excepted(rules, path, jit, interp):
    """The reason a difference between two runs is allowed, or None. `jit` and `interp` are
    (exit status, output); `jit` may be None when only the interpreter ran (then only a path explains)."""
    relative = os.path.relpath(os.path.abspath(path), ROOT) if path else ""
    for kind, text, reason in rules:
        if kind == "path" and fnmatch.fnmatch(relative, text):
            return reason
        if jit is None:
            continue
        if kind == "jit" and text in jit[1]:
            return reason
        if kind == "both" and text in jit[1] and text in interp[1] and jit[0] == interp[0]:
            return reason
        if kind == "either" and (text in jit[1] or text in interp[1]):
            return reason
    return None


def programs(paths):
    found = []
    for path in paths:
        path = os.path.abspath(path)
        if os.path.isdir(path):
            for directory, names, files in os.walk(path):
                names[:] = sorted(name for name in names if name != "archive")
                found += [os.path.join(directory, name) for name in sorted(files) if name.endswith(".ok")]
        else:
            found.append(path)
    return found


def execute(args, spb, cwd, interpret):
    env = dict(os.environ)
    env.setdefault("OKKAMA_STDLIB_DIR", os.path.join(ROOT, "std"))
    env.pop("SPARROW_CORE_INTERPRET", None)
    if interpret:
        env["SPARROW_CORE_INTERPRET"] = "1"
    command = [args.okkama, "exec", spb]
    if args.cage:
        command = [sys.executable, CAGE, "--cpu-seconds", str(args.timeout), "--timeout", str(args.timeout + 30),
                   "--memory-mb", "2048", "--writable", cwd, "--"] + command
    try:
        p = subprocess.run(command, cwd=cwd, env=env, capture_output=True, timeout=args.timeout + 60)
    except subprocess.TimeoutExpired:
        return ("timeout", b"", b"")
    return (p.returncode, p.stdout, p.stderr)


def compare(path, args, rules):
    with tempfile.TemporaryDirectory() as scratch:
        spb = os.path.join(scratch, "program.spb")
        env = dict(os.environ)
        env.setdefault("OKKAMA_STDLIB_DIR", os.path.join(ROOT, "std"))
        built = subprocess.run([args.okkama, "build", path, "-o", spb], cwd=os.path.dirname(path), env=env, capture_output=True)
        if built.returncode != 0:
            return path, "unbuilt", None
        jit = execute(args, spb, os.path.dirname(path), False)
        interp = execute(args, spb, os.path.dirname(path), True)
    if jit == interp:
        return path, "same", None
    text = lambda run: (run[0], (run[1] + run[2]).decode("utf-8", "replace"))
    reason = excepted(rules, path, text(jit), text(interp))
    if reason:
        return path, "excepted", reason
    detail = f"exit {jit[0]} / {interp[0]}; " + (interp[2] or interp[1]).decode("utf-8", "replace").strip()[-200:]
    return path, "DIFFERS", detail


def run(args):
    rules = load()
    paths = programs(args.paths or [os.path.join(ROOT, "examples")])
    counts, reasons, differing = {}, {}, []
    # Programs of one directory run one after another: some write files next to themselves (examples/fs).
    groups = {}
    for path in paths:
        groups.setdefault(os.path.dirname(path), []).append(path)
    serial = lambda group: [compare(path, args, rules) for path in group]
    with concurrent.futures.ThreadPoolExecutor(args.jobs) as pool:
        results = [result for group in pool.map(serial, groups.values()) for result in group]
        for path, verdict, detail in results:
            counts[verdict] = counts.get(verdict, 0) + 1
            name = os.path.relpath(path, ROOT)
            if verdict == "excepted":
                reasons.setdefault(detail, []).append(name)
            elif verdict == "DIFFERS":
                differing.append(name)
                print(f"DIFFERS {name}: {detail}")
    for reason, names in reasons.items():
        print(f"excepted {len(names):4}  {reason}\n         {', '.join(names[:4])}{', ...' if len(names) > 4 else ''}")
    print(f"Programs={len(paths)}, " + ", ".join(f"{key}={value}" for key, value in sorted(counts.items())))
    return status(counts)


def status(counts):
    """`run`'s exit status from its verdict counts: 1 when a program differs, or when none built (an empty or
    broken build compares nothing and must not pass, #1144)."""
    if counts.get("DIFFERS"):
        return 1
    if not any(count for verdict, count in counts.items() if verdict != "unbuilt"):
        print("no program built: nothing was compared", file=sys.stderr)
        return 1
    return 0


def ocs(args):
    """The cases of an interpreter conformance run that did not pass, explained or not."""
    rules = load()
    explained, unexplained = {}, []
    for line in open(args.results, encoding="utf-8"):
        result = json.loads(line)
        if result.get("status") == "PASS":
            continue
        response = result.get("response") or {}
        failure = response.get("failure") or {}
        output = "\n".join(filter(None, [result.get("stderr_text"), result.get("stdout_text"), failure.get("raw"), failure.get("message")]))
        reason = excepted(rules, None, None, (None, output))
        if reason:
            explained.setdefault(reason, []).append(result["id"])
        else:
            unexplained.append((result["id"], result.get("status"), output.strip().splitlines()[-1:] or [""]))
    for reason, ids in explained.items():
        print(f"excepted {len(ids):4}  {reason}\n         {', '.join(ids[:4])}{', ...' if len(ids) > 4 else ''}")
    for case, status, last in unexplained:
        print(f"UNEXPLAINED {case} {status}: {last[0][:200]}")
    print(f"not passed: {sum(map(len, explained.values())) + len(unexplained)}, unexplained: {len(unexplained)}")
    return 1 if unexplained else 0


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    runner = sub.add_parser("run")
    runner.add_argument("paths", nargs="*")
    runner.add_argument("--okkama", default=os.environ.get("OKKAMA", OKKAMA))
    runner.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) // 2))
    runner.add_argument("--timeout", type=int, default=120)
    runner.add_argument("--cage", action="store_true")
    results = sub.add_parser("ocs")
    results.add_argument("results")
    sub.add_parser("check")
    args = parser.parse_args()
    if args.command == "check":
        print(f"{len(load())} exceptions")
        return 0
    if args.command == "ocs":
        return ocs(args)
    args.okkama = os.path.abspath(args.okkama)
    return run(args)


if __name__ == "__main__":
    sys.exit(main())
