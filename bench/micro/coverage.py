#!/usr/bin/env python3
"""Report declared measurement targets, not incidental harness imports."""
import argparse
import json
from pathlib import Path
HERE=Path(__file__).resolve().parent

def report(std):
    manifest=json.loads((HERE/'manifest.json').read_text())
    covered={}
    for name,entry in manifest.items():
        for module in entry.get('targets',[]):
            if not module.startswith('builtin:'):covered.setdefault(module,[]).append(name)
    modules=sorted(str(p.relative_to(std).with_suffix('')) for p in std.rglob('*.ok') if not p.relative_to(std).as_posix().startswith('runtime/'))
    if not modules:raise ValueError('stdlib directory has no module files')
    missing=set(covered)-set(modules)
    if missing:raise ValueError('unknown measurement targets: '+', '.join(sorted(missing)))
    lines=['# std micro-benchmark targets','',f'{len(covered)} of {len(modules)} non-runtime module files have an explicitly declared measured action.','',
    'This is not API, branch or test coverage. A benchmark targeting one action does not cover the entire module. Harness imports do not count. Builtin string operations are listed in manifest.json separately.','', '| module | measured by |','|---|---|']
    lines += [f'| `{m}` | {", ".join(covered.get(m,[])) or "—"} |' for m in modules]
    return '\n'.join(lines)+'\n'

def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--stdlib',type=Path,required=True);p.add_argument('--output',type=Path,default=HERE/'STD-COVERAGE.md');a=p.parse_args()
    a.output.write_text(report(a.stdlib.resolve()))
if __name__=='__main__':main()
