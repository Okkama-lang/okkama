"""Figure 6.1: walking Cyrillic text with slice(i, i + 1) on Tomato,
quadratic before issue #52 and linear after it.
Data: data/slice-cost.json."""
import json, os
from svg import lines

data = json.load(open(os.path.join(os.path.dirname(__file__), "..", "..", "data", "slice-cost.json")))
for language in ("en", "ru"):
    series = [(("before #52", "до #52"), sorted((int(n), t) for n, t in data["cyrillic_before_52"].items())),
              (("0.4.1", "0.4.1"), sorted((int(n), t) for n, t in data["cyrillic_slice"].items()))]
    lines("slice-cost", series, language, ("characters", "символов"), ("seconds", "секунды"),
          note=("Tomato, Cyrillic text, Apple M2 Max", "Tomato, кириллица, Apple M2 Max"))
