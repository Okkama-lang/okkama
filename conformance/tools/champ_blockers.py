#!/usr/bin/env python3
"""Rank what blocks Champ in a conformance run.

    python3 tools/champ_blockers.py CHAMP_RUN [--baseline SPARROW_RUN]
                                    [--json OUT.json] [--compare PREVIOUS.json]

CHAMP_RUN and SPARROW_RUN are run directories of `ocs run` (or their
`results.jsonl`), the first through `adapters/rust/adapter-champ.json`, the
second through `adapters/rust/adapter.json` on the same profile. A case that
does not pass on the baseline either is counted as "fails on both" and is not
Champ's blocker.

Every Champ-only failure gets a stage and a blocker key: the first diagnostic
line with the parts that vary between cases (spans, numbers, file and symbol
names, quoted text) removed and the construct kept, for example

    unsupported in OIR-2 lowering: closure capture of x in function main
        -> unsupported: closure capture
    OIR-2 compile_low: GenerateError("type Map(Int, Str): not yet in S1")
        -> S1 type Map

The report counts cases per stage and per key. `--json` writes the same data;
`--compare` reads an earlier `--json` file and prints new passes, new failures
and blockers whose case counts moved. Standard library only.
"""

import argparse
import json
import os
import re
import sys

SCHEMA = "ocs-champ-blockers/1"
EXAMPLES = 5

# Stages in report order; the first two are not Champ-only failures.
STAGES = (
    ("pass", "passes"),
    ("both", "fails on both engines"),
    ("lowering", "unsupported in OIR-2 lowering"),
    ("generation", "SPB 17 generation gap"),
    ("loader", "loader rule"),
    ("runtime", "runtime failure or crash"),
    ("wrong-output", "wrong output"),
    ("timeout", "timeout"),
    ("test-mode", "test mode not supported"),
    ("driver", "Champ driver unavailable"),
    ("unsupported", "other unsupported (front end, build, mode)"),
    ("not-run", "not run (blocked or filtered)"),
)
NOT_CHAMP = ("pass", "both")

LOWERING = "unsupported in OIR-2 lowering"
TEST_PROFILE = "unsupported in OIR-2 lowering: test profile"
GAP = re.compile(r'(?:GenerateError\("|SPB 17: |^|: )((?:(?!: )[^"])*?): not yet in (S\d)')
RULE = re.compile(r'rule: \\?"([A-Z][A-Z0-9]*(?:_[A-Z0-9]+)+)\\?"|(?:^|okkama: |\n)([A-Z][A-Z0-9]*(?:_[A-Z0-9]+)+): ')
DRIVER = ("Champ OIR-2 driver is not yet available", "Champ driver requires", "runs only SPB 17")


# ---- reading runs -----------------------------------------------------------

def results_path(path):
    return os.path.join(path, "results.jsonl") if os.path.isdir(path) else path


def load_results(path):
    records = {}
    with open(results_path(path)) as fh:
        for line in fh:
            line = line.strip()
            if line:
                rec = json.loads(line)
                records[rec["id"]] = rec
    return records


def first_line(text):
    for line in (text or "").splitlines():
        if line.strip():
            return line.strip()
    return ""


def strip_tool(text):
    return text[len("okkama: "):] if text.startswith("okkama: ") else text


def diagnostic(rec):
    """The first diagnostic line of a record, and all its text for pattern search."""
    resp = rec.get("response") or {}
    failure = resp.get("failure") or {}
    reasons = rec.get("reasons") or []
    parts = [resp.get("unsupported_reason") or "", failure.get("message") or "", failure.get("raw") or "",
             resp.get("raw") or "", rec.get("stderr_text") or "", "\n".join(reasons)]
    for candidate in (resp.get("unsupported_reason"), failure.get("message"), first_line(rec.get("stderr_text")),
                      reasons[0] if reasons else None):
        if candidate and candidate.strip():
            return strip_tool(first_line(candidate)), "\n".join(p for p in parts if p)
    return "", "\n".join(p for p in parts if p)


# ---- key normalisation ------------------------------------------------------

def normalise_text(text, limit=120):
    """Remove what varies between cases: locations, spans, paths, quoted text, numbers."""
    text = strip_tool(text.strip())
    text = re.sub(r"^\S+?:\d+:\d+:\s*", "", text)               # file:line:col:
    text = re.sub(r"\b\d+\.\.\d+:?\s*", "", text)               # spans 12..40
    text = re.sub(r"`[^`]*`", "`_`", text)
    text = re.sub(r'\\?"(?:[^"\\]|\\.)*\\?"', '"_"', text)
    text = re.sub(r"'[^'\s]*'", "'_'", text)
    text = re.sub(r"(?:/[^\s:,;)]+)+", "<path>", text)
    text = re.sub(r"\b[\w.-]+\.ok\b", "<file>", text)
    text = re.sub(r"\b0x[0-9a-fA-F]+\b", "N", text)
    text = re.sub(r"\d+", "N", text)
    text = re.sub(r"\s+", " ", text).strip()
    return text[:limit]


def normalise_construct(text):
    """Keep the construct of an OIR-2 limitation, drop the symbols it was found on."""
    text = text.strip()
    text = re.sub(r"\s*\(#\d+[^)]*\)", "", text)                # ticket references
    text = re.sub(r"\s+in (?:function|action|object|module) \S+.*$", "", text)
    text = re.sub(r"\btype\s+([A-Za-z_][\w:]*)\s*[\(\{<\[].*$", r"type \1", text)
    text = re.sub(r"\s+(?:of|for|on|named) [`'\"]?[\w.:<>$]+[`'\"]?$", "", text)
    text = re.sub(r"\s*`[^`]*`", "", text)
    return normalise_text(text)


def lowering_key(diag, text):
    source = diag if LOWERING in diag else next(l for l in text.splitlines() if LOWERING in l)
    construct = source.split(LOWERING, 1)[1].lstrip(": ").strip()
    return "unsupported: " + (normalise_construct(construct) or "?")


def generation_key(diag, text):
    for source in (diag, text):
        m = GAP.search(source)
        if m:
            return "%s %s" % (m.group(2), normalise_construct(m.group(1)) or "?")
    return "S? " + normalise_text(diag)


def rule_of(text):
    m = RULE.search(text)
    if not m:
        return None
    return m.group(1) or m.group(2)


# ---- classification ---------------------------------------------------------

def classify(rec, baseline=None):
    """(stage, key) of one Champ record; baseline is the Sparrow record or None."""
    status = rec.get("status")
    if status == "PASS":
        return "pass", None
    if baseline is not None and baseline.get("status") != "PASS":
        return "both", None
    if status in ("BLOCKED", "NOT_RUN"):
        return "not-run", "not run: " + normalise_text((rec.get("reasons") or [""])[0])
    if status == "TIMEOUT":
        return "timeout", "timeout"
    diag, text = diagnostic(rec)
    if TEST_PROFILE in text:
        return "test-mode", "test mode: okkama run -t is not supported under OIR-2"
    if LOWERING in text:
        return "lowering", lowering_key(diag, text)
    if GAP.search(text):
        return "generation", generation_key(diag, text)
    resp = rec.get("response") or {}
    outcome = resp.get("outcome")
    reasons = rec.get("reasons") or [""]
    # A rule code in a program's own output is not a load rejection.
    rule = rule_of(text) if status != "FAIL" or outcome in ("failed", "internal-error") else None
    if rule:
        return "loader", "loader " + rule
    if any(d in text for d in DRIVER):
        return "driver", "driver: " + normalise_text(next(l for l in text.splitlines() if any(d in l for d in DRIVER)))
    if status == "UNSUPPORTED":
        return "unsupported", "unsupported (other): " + normalise_text(diag)
    if status == "HARNESS_ERROR":
        return "runtime", "harness: " + normalise_text(reasons[0])
    if status == "CRASH":
        signal = (rec.get("process") or {}).get("signal")
        if signal and outcome != "internal-error":
            return "runtime", "crash: signal %s" % signal
        return "runtime", "crash: " + normalise_text(diag)
    if status == "FAIL":
        if outcome == "failed" and any("observed failed" in r for r in reasons):
            failure = resp.get("failure") or {}
            what = failure.get("error_type") or normalise_text(failure.get("message") or diag, 80)
            return "runtime", "runtime: %s %s" % (failure.get("category") or "?", what)
        return "wrong-output", "wrong output: " + normalise_reason(reasons[0])
    return "runtime", "%s: %s" % ((status or "?").lower(), normalise_text(diag))


def normalise_reason(reason):
    """A judge reason without its varying detail: `stdout differs at character 12: ...` -> `stdout differs`."""
    if re.match(r"^exit status \d+, expected \d+", reason):
        return "exit status differs"
    m = re.match(r"^(\S+(?: \S+)? (?:differs|bytes differ|lines|is not UTF-8 text|does not end with a newline|violates))", reason)
    if m:
        return m.group(1)
    reason = re.sub(r"\s*\(.*$", "", reason)                    # failure detail in parentheses
    return normalise_text(reason.split(":", 1)[0], 80)


def analyse(champ, baseline=None):
    cases = {}
    for cid in sorted(champ):
        base = baseline.get(cid) if baseline is not None else None
        stage, key = classify(champ[cid], base)
        cases[cid] = {"status": champ[cid].get("status"), "stage": stage, "key": key}
        if baseline is not None:
            cases[cid]["baseline_status"] = base.get("status") if base else None
    stages = {name: 0 for name, _ in STAGES}
    blockers = {}
    for cid, case in cases.items():
        stages[case["stage"]] += 1
        if case["stage"] in NOT_CHAMP:
            continue
        entry = blockers.setdefault(case["key"], {"key": case["key"], "stage": case["stage"], "count": 0, "cases": []})
        entry["count"] += 1
        entry["cases"].append(cid)
    ranked = sorted(blockers.values(), key=lambda b: (-b["count"], b["key"]))
    for b in ranked:
        b["examples"] = b["cases"][:EXAMPLES]
    champ_only = sum(n for s, n in stages.items() if s not in NOT_CHAMP)
    return {
        "schema": SCHEMA,
        "totals": {"cases": len(cases), "pass": stages["pass"], "both": stages["both"], "champ_only": champ_only},
        "stages": stages,
        "blockers": [{"key": b["key"], "stage": b["stage"], "count": b["count"], "examples": b["examples"],
                      "cases": b["cases"]} for b in ranked],
        "cases": cases,
    }


def compare(current, previous):
    now, before = current["cases"], previous.get("cases", {})
    new_passes = sorted(c for c, v in now.items() if v["stage"] == "pass" and before.get(c, {}).get("stage") != "pass")
    new_failures = sorted(c for c, v in now.items()
                          if v["stage"] not in NOT_CHAMP and before.get(c, {}).get("stage") == "pass")
    counts_now = {b["key"]: b["count"] for b in current["blockers"]}
    counts_before = {b["key"]: b["count"] for b in previous.get("blockers", [])}
    moved = []
    for key in sorted(set(counts_now) | set(counts_before)):
        a, b = counts_before.get(key, 0), counts_now.get(key, 0)
        if a != b:
            moved.append({"key": key, "before": a, "now": b, "delta": b - a})
    moved.sort(key=lambda m: (-abs(m["delta"]), m["key"]))
    rekeyed = sorted(c for c, v in now.items() if c in before and v["stage"] not in NOT_CHAMP
                     and before[c].get("stage") not in NOT_CHAMP and before[c].get("key") != v["key"])
    return {"new_passes": new_passes, "new_failures": new_failures, "moved_blockers": moved,
            "rekeyed_cases": rekeyed,
            "totals_before": previous.get("totals", {}), "totals_now": current["totals"]}


# ---- output -----------------------------------------------------------------

def render(doc, out):
    t = doc["totals"]
    out.write("Champ conformance: %s\n" % doc.get("champ_run"))
    out.write("baseline (Sparrow): %s\n" % (doc.get("baseline_run") or "none: every failure counts as Champ-only"))
    out.write("cases %d: pass %d, fails on both %d, Champ-only failures %d\n\n" % (
        t["cases"], t["pass"], t["both"], t["champ_only"]))
    out.write("By stage:\n")
    for name, title in STAGES:
        out.write("  %-46s %5d\n" % (title, doc["stages"].get(name, 0)))
    out.write("\nBlockers (Champ-only failures by key):\n")
    if not doc["blockers"]:
        out.write("  none\n")
    for b in doc["blockers"]:
        out.write("  %5d  [%s] %s\n" % (b["count"], b["stage"], b["key"]))
        out.write("         %s%s\n" % (", ".join(b["examples"]), ", ..." if b["count"] > len(b["examples"]) else ""))
    if "compare" in doc:
        c = doc["compare"]
        out.write("\nCompared with %s:\n" % doc.get("compare_with"))
        out.write("  pass %s -> %s, Champ-only %s -> %s\n" % (
            c["totals_before"].get("pass", "?"), c["totals_now"]["pass"],
            c["totals_before"].get("champ_only", "?"), c["totals_now"]["champ_only"]))
        out.write("  new passes (%d): %s\n" % (len(c["new_passes"]), ", ".join(c["new_passes"]) or "-"))
        out.write("  new failures (%d): %s\n" % (len(c["new_failures"]), ", ".join(c["new_failures"]) or "-"))
        out.write("  moved blockers:\n")
        if not c["moved_blockers"]:
            out.write("    none\n")
        for m in c["moved_blockers"]:
            note = " (new)" if m["before"] == 0 else " (gone)" if m["now"] == 0 else ""
            out.write("    %+5d  %s: %d -> %d%s\n" % (m["delta"], m["key"], m["before"], m["now"], note))
        if c["rekeyed_cases"]:
            out.write("  cases now blocked by another key (%d): %s\n" % (
                len(c["rekeyed_cases"]), ", ".join(c["rekeyed_cases"][:20])))


def main(argv=None):
    parser = argparse.ArgumentParser(description="Classify Champ conformance failures and rank the blockers.")
    parser.add_argument("champ", help="Champ run directory or its results.jsonl")
    parser.add_argument("--baseline", help="Sparrow run of the same profile (directory or results.jsonl)")
    parser.add_argument("--json", dest="json_out", help="also write the analysis as JSON to this file")
    parser.add_argument("--compare", help="an earlier --json file to diff against")
    args = parser.parse_args(argv)
    champ = load_results(args.champ)
    baseline = load_results(args.baseline) if args.baseline else None
    doc = analyse(champ, baseline)
    doc["champ_run"] = os.path.abspath(results_path(args.champ))
    doc["baseline_run"] = os.path.abspath(results_path(args.baseline)) if args.baseline else None
    if args.compare:
        with open(args.compare) as fh:
            previous = json.load(fh)
        doc["compare_with"] = os.path.abspath(args.compare)
        doc["compare"] = compare(doc, previous)
    if args.json_out:
        with open(args.json_out, "w") as fh:
            json.dump(doc, fh, indent=1, sort_keys=True)
            fh.write("\n")
    render(doc, sys.stdout)
    return 0


if __name__ == "__main__":
    sys.exit(main())
