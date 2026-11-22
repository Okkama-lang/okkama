#!/usr/bin/env python3
"""Run the unchanged Diff API against the experimental algorithm; restore std."""
import os
from pathlib import Path
import subprocess
import sys
ROOT = Path(__file__).resolve().parents[2]
p = ROOT / "std/diff.ok"
original = p.read_text()
try:
    assert 'import "runtime/diff" take diff_script;' in original
    p.write_text(original.replace('import "runtime/diff" take diff_script;',
        'import "../docs/diff-13/candidate" take DiffAlgorithm;').replace(
        'diff_script(left, right);', 'DiffAlgorithm(left: left, right: right).script();'))
    env = dict(os.environ, OKKAMA_STDLIB_DIR=str(ROOT / "std"))
    args = sys.argv[1:]
    rss = args[:1] == ["--rss"]
    command = [str(ROOT / "target-13/release/okkama"), "run", *(args[1:] if rss else args)]
    if rss:
        command = [sys.executable, str(ROOT / "docs/diff-13/rss.py"), *command]
    result = subprocess.run(command, cwd=ROOT, env=env, timeout=60)
finally:
    p.write_text(original)
sys.exit(result.returncode)
