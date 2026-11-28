#!/usr/bin/env python3
"""Check every Okkama example in docs/agents/*.md against the compiler.

    OKKAMA=BIN OKKAMA_STDLIB_DIR=DIR python3 docs/agents/check.py [--jobs N] [FILE.md ...]

A fenced block is an example when its info string starts with `okkama`.
The words after it (separated by commas or spaces) say what is checked:

    ```okkama          a complete module: `okkama check` must accept it
    ```okkama,run      a complete module: `okkama run` must print exactly the
                       ```output block that follows it (a status other than
                       0 adds a last line `exit N`)
    ```okkama,test     the same with `okkama run -t` (the test report is the output)
    ```okkama,fail     a complete module that `okkama check` must reject; the
                       ```error block that follows is the exact diagnostic
                       (stderr), with the file named example.ok
    ```okkama,ignore   shown, never compiled (grammar sketches)

`okkama run` may be written for `okkama,run`. Any other block (```text,
```output, ```error, ```sh) is prose. Each example is written to
example.ok in a directory of its own and runs there. OKKAMA names the
binary (default: the release build of this checkout) and OKKAMA_STDLIB_DIR
its std (default: this checkout's std/). On macOS the runs go through the
repository's cage (okkama-rs/tools/new-runtime/caged.py);
OKKAMA_AGENTS_RUNNER replaces it with another command prefix, or with
nothing when set to an empty string. Exit status 0 when every example
passes.
"""
import argparse
import concurrent.futures
import glob
import os
import re
import shlex
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
OKKAMA = os.environ.get("OKKAMA", os.path.join(ROOT, "okkama-rs", "target", "release", "okkama"))
STDLIB = os.environ.get("OKKAMA_STDLIB_DIR", os.path.join(ROOT, "std"))
CAGE = os.path.join(ROOT, "okkama-rs", "tools", "new-runtime", "caged.py")
MODES = {"check", "run", "test", "fail", "ignore"}


def blocks(path):
    """(line, info, text) for every fenced block of a Markdown file."""
    found = []
    lines = open(path, encoding="utf-8").read().split("\n")
    i = 0
    while i < len(lines):
        opening = re.match(r"^(`{3,})(.*)$", lines[i])
        if not opening:
            i += 1
            continue
        fence, info, start = opening.group(1), opening.group(2).strip(), i + 1
        i += 1
        body = []
        # A raw string literal (```text```) inside code never stands alone on
        # a line, so only a line of backticks closes the block.
        while i < len(lines) and not re.match(r"^`{%d,}\s*$" % len(fence), lines[i]):
            body.append(lines[i])
            i += 1
        i += 1
        found.append((start, info, "\n".join(body) + "\n"))
    return found


def examples(path):
    """(line, mode, source, expected) for every okkama block of a file."""
    found = []
    items = blocks(path)
    for index, (line, info, text) in enumerate(items):
        words = info.replace(",", " ").split()
        if not words or words[0] != "okkama":
            continue
        flags = words[1:] or ["check"]
        mode = flags[0]
        if mode not in MODES or len(flags) > 1:
            raise SystemExit(f"{path}:{line}: unknown example kind `{info}`")
        expected = None
        if mode in ("run", "test", "fail"):
            wanted = "error" if mode == "fail" else "output"
            following = items[index + 1] if index + 1 < len(items) else None
            if following is None or following[1] != wanted:
                raise SystemExit(f"{path}:{line}: `{info}` needs a ```{wanted} block right after it")
            expected = following[2].rstrip("\n").splitlines()
        if mode != "ignore":
            found.append((line, mode, text, expected))
    return found


def runner():
    if "OKKAMA_AGENTS_RUNNER" in os.environ:
        return shlex.split(os.environ["OKKAMA_AGENTS_RUNNER"])
    if sys.platform == "darwin" and os.path.exists(CAGE):
        return None  # the cage, built per example (it names the writable directory)
    return []


def check(item, timeout):
    path, line, mode, source, expected = item
    with tempfile.TemporaryDirectory(prefix="agents-") as work:
        with open(os.path.join(work, "example.ok"), "w", encoding="utf-8") as out:
            out.write(source)
        if mode in ("check", "fail"):
            command = [OKKAMA, "check", "example.ok"]
        else:
            command = [OKKAMA, "run", *(["-t"] if mode == "test" else []), "example.ok"]
        prefix = runner()
        if prefix is None:
            prefix = [sys.executable, CAGE, "--cpu-seconds", str(timeout), "--timeout", str(timeout + 30),
                      "--memory-mb", "1024", "--writable", work, "--"]
        env = dict(os.environ, OKKAMA_STDLIB_DIR=STDLIB, OKKAMA_TRACE="off")
        try:
            p = subprocess.run(prefix + command, cwd=work, env=env, capture_output=True, text=True,
                               timeout=timeout + 60)
        except subprocess.TimeoutExpired:
            return item, "timeout"
    location = f"{os.path.relpath(path, ROOT)}:{line}"
    if mode == "check":
        return item, None if p.returncode == 0 else f"{location}: rejected:\n{p.stderr.strip()}"
    if mode == "fail":
        if p.returncode == 0:
            return item, f"{location}: accepted, but the example says it is rejected"
        got = p.stderr.rstrip("\n").splitlines()
        return item, None if got == expected else f"{location}: diagnostic differs\n  got:      {got}\n  expected: {expected}"
    got = p.stdout.rstrip("\n").splitlines() if p.stdout else []
    if p.returncode != 0:
        got.append(f"exit {p.returncode}")
    if got == expected:
        return item, None
    return item, f"{location}: output differs\n  got:      {got}\n  expected: {expected}\n  stderr:   {p.stderr.strip()[:400]}"


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("files", nargs="*", help="Markdown files (default: docs/agents/*.md)")
    parser.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) // 2))
    parser.add_argument("--timeout", type=int, default=30)
    args = parser.parse_args()
    if not os.access(OKKAMA, os.X_OK):
        parser.error(f"no okkama binary at {OKKAMA} (set OKKAMA)")
    files = args.files or sorted(glob.glob(os.path.join(HERE, "*.md")))
    items = [(path, *example) for path in files for example in examples(path)]
    with concurrent.futures.ThreadPoolExecutor(args.jobs) as pool:
        results = list(pool.map(lambda item: check(item, args.timeout), items))
    failed = [reason for _, reason in results if reason]
    for reason in failed:
        print(reason)
    counts = {}
    for path, _, mode, _, _ in items:
        counts[mode] = counts.get(mode, 0) + 1
    summary = ", ".join(f"{counts[mode]} {mode}" for mode in sorted(counts))
    print(f"{len(items) - len(failed)}/{len(items)} examples pass ({summary})")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
