#!/usr/bin/env python3
"""Reproduce #13 CSV differential and timings without swapping std or touching git.

Only `git show` reads the fixed native baseline. All generated sources, corpus
and observations go to --scratch, outside the repository. Every Okkama process
uses the current checkout's std, a 45-second CPU limit and 60-second timeout.
"""
import argparse
import json
import os
from pathlib import Path
import resource
import subprocess
import sys

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
BASE = "2046083434eda07103f3c7aedd5bd58208c7da1b"


def limits():
    resource.setrlimit(resource.RLIMIT_CPU, (45, 45))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--okkama", required=True)
    parser.add_argument("--scratch", type=Path, required=True)
    parser.add_argument("--bench", action="store_true")
    args = parser.parse_args()
    scratch = args.scratch.resolve()
    if scratch == ROOT or ROOT in scratch.parents:
        parser.error("--scratch must be outside the repository")
    scratch.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, OKKAMA_STDLIB_DIR=str(ROOT / "std"))

    def run(path, *arguments):
        result = subprocess.run([args.okkama, "run", str(path), *map(str, arguments)],
                                cwd=ROOT, env=env, capture_output=True, text=True,
                                timeout=60, preexec_fn=limits)
        if result.returncode or result.stderr:
            raise RuntimeError(result.stdout + result.stderr)
        return result.stdout

    for name, source, module in [("reference-csv", "std/csv.ok", "csv13/reference"),
                                  ("reference-stream", "std/csv/stream.ok", "csv13/reference_stream"),
                                  ("reference-input", "std/csv/input.ok", "csv13/reference_input")]:
        text = subprocess.check_output(["git", "show", f"{BASE}:{source}"], cwd=ROOT, text=True)
        text = text.replace(text.splitlines()[0], f"module {module} v1.0.0;", 1)
        text = text.replace('import "csv"', 'import "./reference-csv"')
        (scratch / f"{name}.ok").write_text(text)
    (scratch / "harness.ok").write_text((ROOT / "bench/micro/support/harness.ok").read_text())
    corpus = scratch / "corpus.json"
    subprocess.run([sys.executable, str(HERE / "generate.py"), str(corpus)], check=True)
    observations = []
    source = (HERE / "differential.ok").read_text()
    for version in ["native", "okkama"]:
        text = source
        if version == "native":
            text = text.replace('import "csv"', 'import "./reference-csv"')
            text = text.replace('import "csv/stream"', 'import "./reference-stream"')
        path = scratch / f"differential-{version}.ok"
        path.write_text(text)
        output = run(path, corpus)
        (scratch / f"{version}.out").write_text(output)
        lines = output.splitlines()
        assert len(lines) == 6625, (version, len(lines))
        observations.append(lines)
    differences = [i for i, (a, b) in enumerate(zip(*observations)) if a != b]
    print(f"6625 inputs x 4 public paths = 26500 observations; differences={len(differences)}", flush=True)
    assert not differences, differences[:20]
    if not args.bench:
        return
    template = (HERE / "bench.ok.in").read_text()
    result = {}
    for version in ["native", "okkama"]:
        result[version] = {}
        for kind in ["parse", "stream"]:
            for size, rows, n, warm in [("small", 1, 10000, 1000), ("medium", 100, 200, 20), ("large", 10000, 3, 1)]:
                body = 'for CsvRows(text) -> row { total.replace(total.value() + row.size() + row[3].length()); }'
                if kind == "stream":
                    body = ('let stream = CsvStream(Chunks(data: data)); loop { '
                            'let row = try { stream.row(); } catch error is EndOfCsv { @string(); }; '
                            'last if row.size() == 0; total.replace(total.value() + row.size() + row[3].length()); }')
                text = template.replace("ROWS", str(rows)).replace("BODY", body)
                if version == "native":
                    text = text.replace('import "csv"', 'import "./reference-csv"')
                    text = text.replace('import "csv/stream"', 'import "./reference-stream"')
                key = f"{kind}-{size}"
                path = scratch / f"{version}-{key}.ok"
                path.write_text(text)
                samples = [run(path, n, warm).strip() for _ in range(3)]
                for sample in samples:
                    parts = sample.split()
                    assert parts[0] == "micro-v2" and int(parts[4]) == 13 * rows * n
                    assert int(parts[5]) == 13 * rows * warm
                result[version][key] = samples
                print(version, key, samples, flush=True)
        for key in ["csv-input", "csv-stream", "csv-round-trip"]:
            text = (ROOT / f"bench/micro/std/{key}.ok").read_text().replace('"../support/harness"', '"./harness"')
            if version == "native":
                for name in ["csv", "csv/input", "csv/stream"]:
                    target = {"csv": "csv", "csv/input": "input", "csv/stream": "stream"}[name]
                    text = text.replace(f'import "{name}"', f'import "./reference-{target}"')
            path = scratch / f"{version}-{key}.ok"
            path.write_text(text)
            result[version][key] = [run(path, 10000, 1000).strip() for _ in range(3)]
            print(version, key, result[version][key], flush=True)
    (scratch / "timings.json").write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
