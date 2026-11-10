"""Figure 8.1: building an array one element at a time. The map() idiom
(issue #145) and items + [x] by copying (0.4.2) are quadratic; one map() and
items + [x] over the shared buffers of 0.4.3 (issue #246) stay flat.
Data: data/append-cost.json."""
import json, os
from svg import lines

data = json.load(open(os.path.join(os.path.dirname(__file__), "..", "..", "data", "append-cost.json")))


def points(key):
    return sorted((int(n), t) for n, t in data[key].items())


for language in ("en", "ru"):
    series = [(("one map()", "один map()"), points("map")),
              (("map() idiom", "приём через map()"), points("idiom")),
              (("items + [x], 0.4.2", "items + [x], 0.4.2"), points("append_copy")),
              (("items + [x], 0.4.3", "items + [x], 0.4.3"), points("append"))]
    lines("append-cost", series, language, ("elements", "элементов"), ("seconds", "секунды"),
          note=("Tomato, Apple M2 Max", "Tomato, Apple M2 Max"))
