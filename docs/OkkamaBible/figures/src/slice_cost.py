"""Figure 6.1: walking a string with slice(i, i + 1) is linear for ASCII and
quadratic for non-ASCII text on the 0.4.1 Sparrow core (issue #52).
Data: data/slice-cost.json."""
import json, os
from svg import lines

data = json.load(open(os.path.join(os.path.dirname(__file__), "..", "..", "data", "slice-cost.json")))
for language in ("en", "ru"):
    series = [(("ASCII text", "текст ASCII"), sorted((int(n), t) for n, t in data["ascii"].items())),
              (("Cyrillic text", "кириллица"), sorted((int(n), t) for n, t in data["cyrillic"].items()))]
    lines("slice-cost", series, language, ("characters", "символов"), ("seconds", "секунды"),
          note=("Sparrow core, Apple M2 Max", "ядро Sparrow, Apple M2 Max"))
