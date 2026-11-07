"""Figure 5.1: integer, float and big-integer workloads of the benchmark
suite (okkama-docs/benchmarks), wall-clock seconds, Okkama 0.4.0 Sparrow core
against other runtimes. Data: data/benchmarks-0.4.0.json."""
import json, os
from svg import bars

data = json.load(open(os.path.join(os.path.dirname(__file__), "..", "..", "data", "benchmarks-0.4.0.json")))["benchmarks"]
groups = [("01-arith", ("int arithmetic", "арифметика int")), ("03-float", ("float arithmetic", "арифметика float")), ("04-bigint", ("big integers", "большие целые"))]
engines = [("sparrow-core", "Okkama (Sparrow)"), ("jvm", "Okkama (JVM)"), ("python", "Python"), ("ruby", "Ruby"), ("perl", "Perl")]
for language in ("en", "ru"):
    values = {label: {name: data[key][engine]["real"] for engine, name in engines} for key, label in groups}
    bars("numbers-cost", [label for _, label in groups], [name for _, name in engines], values, language,
         ("seconds (log scale)", "секунды (лог. шкала)")[language == "ru"], log=True,
         note=("Apple M2 Max, 2026-11-07", "Apple M2 Max, 7.11.2026"))
