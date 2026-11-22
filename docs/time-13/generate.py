#!/usr/bin/env python3
"""Inputs for the native-vs-Okkama differential of time/format (#13).

    generate.py SEED COUNT OUT

Each record: kind (format|parse), local (0|1), pattern, text, and seven
fields (year month day hour minute second nanosecond), separated by U+0002;
records by U+0003. Patterns mix literals (with Unicode), every directive,
%%, bad directives and a trailing %; texts are written by the pattern and
then often mutated."""
import random, sys

rng = random.Random(int(sys.argv[1]))
COUNT = int(sys.argv[2])
DATE = "Ymd"
TIME = "HMSf"
WIDTH = {"Y": 4, "m": 2, "d": 2, "H": 2, "M": 2, "S": 2, "f": 9}
LITERALS = ["-", "/", ":", ".", " ", "T", "at ", "é", "年", "😀", "", "x", "Z", "100", "%%", "%%%%", ",", "\t"]

def pattern(local):
    parts = []
    fields = list(DATE + (TIME if local else ""))
    if rng.random() < 0.7:
        order = fields[:]
        if rng.random() < 0.3:
            rng.shuffle(order)
        for f in order:
            parts.append(rng.choice(LITERALS))
            parts.append("%" + f)
        if rng.random() < 0.3:
            parts.append("%" + rng.choice(fields))  # a repeated field
    else:
        for _ in range(rng.randint(0, 8)):
            parts.append(rng.choice(LITERALS))
            parts.append("%" + rng.choice(DATE + TIME))
    parts.append(rng.choice(LITERALS))
    text = "".join(parts)
    r = rng.random()
    if r < 0.08:
        at = rng.randint(0, len(text))
        text = text[:at] + "%" + rng.choice("qYyZ%a1 ") + text[at:]
    elif r < 0.12:
        text += "%"
    elif r < 0.15 and not local:
        text += "%" + rng.choice(TIME)
    elif r < 0.18:
        text = text.replace("%" + rng.choice(DATE + TIME), "", 1)
    return text

def values():
    return [rng.choice([1, 9, 10, 99, 999, 2026, 9999, rng.randint(1, 9999)]), rng.randint(1, 12), rng.randint(1, 28),
            rng.randint(0, 23), rng.randint(0, 59), rng.randint(0, 59),
            rng.choice([0, 1, 5, 999999999, 123456789, rng.randint(0, 999999999)])]

def written(p, v):
    out = []
    i = 0
    names = "YmdHMSf"
    while i < len(p):
        if p[i] == "%" and i + 1 < len(p):
            d = p[i + 1]
            if d == "%":
                out.append("%")
            elif d in WIDTH:
                out.append(str(v[names.index(d)]).zfill(WIDTH[d]))
            else:
                out.append("%" + d)
            i += 2
        else:
            out.append(p[i])
            i += 1
    return "".join(out)

def mutate(text):
    for _ in range(rng.randint(1, 2)):
        at = rng.randint(0, len(text))
        r = rng.random()
        noise = rng.choice(["0", "9", "a", "-", " ", "%", "é", "１", ""])
        if r < 0.3:
            text = text[:at] + noise + text[at:]
        elif r < 0.6:
            text = text[:at] + text[at + 1:]
        else:
            text = text[:at] + noise + text[at + 1:]
    return text

records = []
while len(records) < COUNT:
    local = rng.random() < 0.5
    p = pattern(local)
    v = values()
    if rng.random() < 0.5:
        records.append(("format", local, p, "", v))
    else:
        t = written(p, v)
        r = rng.random()
        if r < 0.35:
            t = mutate(t)
        elif r < 0.4:
            t = "".join(rng.choice("0123456789-:/ T") for _ in range(rng.randint(0, 30)))
        records.append(("parse", local, p, t, v))
with open(sys.argv[3], "w", encoding="utf-8", newline="") as out:
    out.write("\u0003".join("\u0002".join([k, "1" if l else "0", p, t] + [str(x) for x in v]) for k, l, p, t, v in records))
print(len(records), "inputs")
