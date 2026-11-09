"""Figure 8.1: building an array one element at a time without append is
quadratic (issue #145); one map() is linear. Data: data/append-cost.json."""
import json, os
from svg import lines

data = json.load(open(os.path.join(os.path.dirname(__file__), "..", "..", "data", "append-cost.json")))
for language in ("en", "ru"):
    series = [(("one map()", "один map()"), sorted((int(n), t) for n, t in data["map"].items())),
              (("append by copying", "добавление копированием"), sorted((int(n), t) for n, t in data["concat"].items()))]
    lines("append-cost", series, language, ("elements", "элементов"), ("seconds", "секунды"),
          note=("Tomato, Apple M2 Max", "Tomato, Apple M2 Max"))
