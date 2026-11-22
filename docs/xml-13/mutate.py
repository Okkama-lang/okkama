#!/usr/bin/env python3
"""Mutations of std/xml/internal/events.ok; each must fail its std tests (#13)."""
import os, subprocess, sys
ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
F = os.path.join(ROOT, 'std', 'xml', 'internal', 'events.ok')
B = sys.argv[1]  # okkama binary
M = [
 ('entity lt', 'if reference == "lt" {', 'if reference == "lx" {'),
 ('plus sign of a reference', 'if digits.starts_with("+")', 'if digits.starts_with("#")'),
 ('u32 bound', 'total * base + digit > 4294967295', 'total * base + digit > 4294967296'),
 ('lone CR in text', '"\\n";\n                    }]);', '"\\r";\n                    }]);'),
 ('tab in attribute', 'else if attribute && (code == 9 || code == 10) {', 'else if attribute && code == 10 {'),
 ('CR in CDATA', 'raw.replace("\\r\\n", "\\n").replace("\\r", "\\n")', 'raw'),
 ('-- in a comment', 'doubled + 1 >= end', 'doubled >= end'),
 ('XML declaration at start', 'confirm at == start', 'confirm true'),
 ('instructions outside the root', 'else if self.inside() {\n            let from', 'else if true {\n            let from'),
 ('text joins text', 'case XmlEvent.text(previous, offset) {', 'case XmlEvent.comment(previous, offset) {'),
 ('depth limit', 'confirm depth.value() < maximum_depth', 'confirm depth.value() <= maximum_depth'),
 ('event limit', 'confirm counted.value() < maximum_events', 'confirm counted.value() <= maximum_events'),
 ('DOCTYPE quotes', 'else if code == 34 || code == 39 {', 'else if code == 34 {'),
 ('byte order mark', 'if self.code(0) == 65279 {', 'if self.code(0) == 65280 {'),
 ('name characters', '|| code == 45 || code == 46', '|| code == 45'),
 ('end tag match', 'confirm stack.first() == name', 'confirm true'),
 ('second root', 'confirm !root_closed.value()', 'confirm true'),
 ('four hex digits', 'shift >= 16', 'shift >= 12'),
 ('text outside the root', '''                    throw XmlStop.at(index, "text outside the root element");''', ''),
 (']]> in text', 'confirm self.find(at, end, 93, 93, 62) < 0', 'confirm true'),
 ('duplicate attributes', 'confirm !names.value().contains(attribute)', 'confirm true'),
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
