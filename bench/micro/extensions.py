"""Build the micro extension once, outside every measured interval."""
import os
from pathlib import Path
import subprocess
import sys

HERE = Path(__file__).resolve().parent


def build():
    target = Path(os.environ.get('CARGO_TARGET_DIR', HERE.parents[1] / 'okkama-rs/target')).resolve() / 'micro-ext'
    subprocess.run([os.environ.get('CARGO', 'cargo'), 'build', '--offline', '--locked', '-j', '4',
                    '--manifest-path', str(HERE / 'extension/Cargo.toml'), '--target-dir', str(target)], check=True)
    suffix = 'dylib' if sys.platform == 'darwin' else 'so'
    return str(target / 'debug' / ('libokkama_micro_ext.' + suffix))
