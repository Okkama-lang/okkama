#!/usr/bin/env python3
"""Mutations of std/time/format.ok (#13); each must fail its std tests.

    mutate.py OKKAMA_BINARY
"""
import os, subprocess, sys
ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
F = os.path.join(ROOT, 'std', 'time', 'format.ok')
B = sys.argv[1]
M = [
 ('unknown directive', '"YmdHMSf".find(directive) else -1;', '"YmdHMSf".find(directive) else 0;'),
 ('nanosecond width', '''        else if field == 6 {
            9;''', '''        else if field == 6 {
            8;'''),
 ('zero padding', '"000000000".slice(0, width - digits.length()) + digits', 'digits'),
 ('time field in a date pattern', 'let time = if !local && field >= 3 && time_at < 0 {', 'let time = if false {'),
 ('required local fields', '            let required = if local {\n                63;', '            let required = if local {\n                31;'),
 ('lone % at the end', '''            if index == parts.size() - 1 {
                return offset;
            }''', ''),
 ('%% writes one sign', 'return ["%"] + self.pieces(parts, index + 1, true, values);', 'return ["%%"] + self.pieces(parts, index + 1, true, values);'),
 ('mark conflict', 'if pattern.contains(mark) {', 'if false {'),
 ('repeated field must repeat', 'if value < 0 || (known >= 0 && known != value) {\n            return [at];', 'if value < 0 {\n            return [at];'),
 ('digits only', '            if total < 0 || code < 48 || code > 57 {\n                -1;\n            }\n            else {\n                total * 10 + code - 48;\n            }\n        };\n        let known', '            if total < 0 {\n                -1;\n            }\n            else {\n                total * 10 + code - 48;\n            }\n        };\n        let known'),
 ('whole text', '''            if at != origin.length() {
                return [at];
            }''', ''),
 ('fraction defaults to zero', '            return [-1] + values.take(6) + [if values[6] < 0 {\n                0;\n            }', '            return [-1] + values.take(6) + [if values[6] < 0 {\n                values[6];\n            }'),
 ('mismatch offset', '                return start + index;', '                return start;'),
 ('layout fallback', 'if self.replaced(plain, true, values).replace(1.char().string(), "%") == origin {', 'if true {'),
]
orig = open(F).read()
caught = 0
try:
    for name, a, b in M:
        assert orig.count(a) == 1, (name, orig.count(a))
        open(F, 'w').write(orig.replace(a, b))
        r = subprocess.run([B, 'run', '-t', F], capture_output=True, text=True,
                           env={'OKKAMA_STDLIB_DIR': os.path.join(ROOT, 'std'), 'PATH': '/usr/bin:/bin'})
        out = r.stdout + r.stderr
        fails = [l for l in out.splitlines() if l.startswith('--- FAIL')]
        ok = r.returncode != 0 or bool(fails)
        caught += ok
        print(('CAUGHT ' if ok else 'MISSED ') + name + ': ' + (fails[0] if fails else out.strip().splitlines()[-1][:100]))
finally:
    open(F, 'w').write(orig)
print(caught, 'of', len(M))
