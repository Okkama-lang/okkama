"""Run generated Okkama source through `okkama fmt` (#1006).

Generators that build source from data join it compactly and let the formatter lay it out,
so a generated std file is already fmt-clean and the release gate's `fmt --check` passes.
The binary is $OKKAMA, else the repository's release or debug build, else `okkama` on PATH;
use one built from the tree whose std you regenerate.
"""
import os
import shutil
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def okkama_binary():
    if os.environ.get('OKKAMA'):
        return os.environ['OKKAMA']
    for profile in ('release', 'debug'):
        candidate = ROOT / 'okkama-rs' / 'target' / profile / 'okkama'
        if candidate.is_file():
            return str(candidate)
    found = shutil.which('okkama')
    if found:
        return found
    raise SystemExit('okkama not found: set OKKAMA to a binary built from this tree')


def formatted(text, name):
    """`text` laid out by `okkama fmt`; `name` names the module in an error."""
    result = subprocess.run([okkama_binary(), 'fmt', '-'], input=text.encode('utf-8'),
                            capture_output=True)
    if result.returncode != 0:
        raise SystemExit(f'okkama fmt rejected generated {name}:\n'
                         + result.stderr.decode('utf-8', 'replace'))
    return result.stdout.decode('utf-8')
