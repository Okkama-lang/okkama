#!/usr/bin/env python3
"""One implementation mutation per std/diff test; always restore the file."""
from pathlib import Path
import os
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
source = ROOT / "std/diff.ok"
original = source.read_text()
env = dict(os.environ, OKKAMA_STDLIB_DIR=str(ROOT / "std"))
binary = ROOT / "target-13/release/okkama"
mutations = [
    ("unchanged lines", "DiffEdit(kind: DiffEditKind.equal,", "DiffEdit(kind: DiffEditKind.removed,"),
    ("one comparison", "self.unified(3);", "self.unified(0);"),
    ("the ratio", "(2 * matched).float()", "matched.float()"),
    ("edits walk", "left: x, right: y", "left: x + 1, right: y"),
    ("repeated lines", "if kind == 0 {", "if kind == 1 {"),
    ("line count accepts", "size <= 512", "size <= 511"),
    ("lines accept", "text.length() <= 4096", "text.length() <= 4095"),
    ("aggregate scalar", "scalars <= 1048576", "scalars <= 1048575"),
    ("rendering refuses", "amount <= 1048576", "amount <= 4194304"),
]
try:
    for name, before, after in mutations:
        assert before in original, before
        source.write_text(original.replace(before, after))
        command = [str(binary), "run", "-t", "--filter", name, "std/diff.ok"]
        result = subprocess.run(command, cwd=ROOT, env=env, capture_output=True, text=True, timeout=60)
        print("COMMAND:", "OKKAMA_STDLIB_DIR=" + env["OKKAMA_STDLIB_DIR"], *command, flush=True)
        print("MUTATION:", repr(before), "->", repr(after), flush=True)
        print(result.stdout + result.stderr, flush=True)
        if result.returncode == 0 or "FAIL" not in result.stdout + result.stderr:
            sys.exit("mutation did not produce a test failure: " + name)
finally:
    source.write_text(original)
