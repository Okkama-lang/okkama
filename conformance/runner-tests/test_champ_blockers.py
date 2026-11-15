#!/usr/bin/env python3
"""tools/champ_blockers.py on synthetic results.jsonl fixtures."""
import importlib.util
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from contextlib import redirect_stdout

SUITE = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("champ_blockers", SUITE / "tools/champ_blockers.py")
blockers = importlib.util.module_from_spec(spec)
spec.loader.exec_module(blockers)


def rec(cid, status, mode="run", response=None, reasons=None, stderr=None, signal=None):
    r = {"id": cid, "level": cid[:3], "mode": mode, "status": status, "reasons": reasons or [],
         "response": response, "stderr_text": stderr, "process": {"exit": None, "signal": signal, "timed_out": False}}
    return r


def unsupported(reason):
    return {"protocol": "ocs-adapter/2", "mode": "run", "outcome": "unsupported", "unsupported_reason": reason}


def internal(message):
    return {"protocol": "ocs-adapter/2", "mode": "run", "outcome": "internal-error",
            "failure": {"stage": "check", "category": "internal", "message": message}}


CHAMP = [
    rec("L00.pass", "PASS", response={"outcome": "completed", "exit_status": 0}),
    rec("L01.both", "FAIL", reasons=["stdout differs at character 0: expected 'a', got 'b'"],
        response={"outcome": "completed", "exit_status": 0}),
    rec("L02.closure-a", "UNSUPPORTED",
        response=unsupported("12..30: unsupported in OIR-2 lowering: closure capture of x in function app.main")),
    rec("L02.closure-b", "UNSUPPORTED",
        response=unsupported("7..9: unsupported in OIR-2 lowering: closure capture of counter in function app.f")),
    rec("L02.closure-c", "UNSUPPORTED",
        response=unsupported("1..2: unsupported in OIR-2 lowering: closure capture of y")),
    rec("L03.defer", "UNSUPPORTED",
        response=unsupported("5..6: unsupported in OIR-2 lowering: rebind of defer capture (#567 S3) in function g")),
    rec("L04.map-a", "UNSUPPORTED",
        response=unsupported('OIR-2 compile_low: GenerateError("type Map(Int, Str): not yet in S1")')),
    rec("L04.map-b", "UNSUPPORTED",
        response=unsupported('OIR-2 compile_low: GenerateError("type Map(Struct(4), Int): not yet in S1")')),
    rec("L04.cleanup", "UNSUPPORTED", response=unsupported("OIR-2 → SPB 17: cleanup.call: not yet in S2")),
    rec("L05.loader", "CRASH", reasons=["implementation internal error: ..."],
        response=internal('OIR-2 verify_load: Error { offset: Some(4), message: "bad", rule: "OPERAND_VALUE" }')),
    rec("L05.loader-run", "CRASH", response=internal("LINEAR_CONSUME: consumed twice")),
    rec("L06.panic", "FAIL", reasons=["expected completed, observed failed (runtime/panic : boom)"],
        response={"outcome": "failed", "failure": {"stage": "runtime", "category": "panic", "message": "boom 42"}}),
    rec("L06.uncaught", "FAIL", reasons=["expected completed, observed failed (runtime/uncaught-error NotFound: k)"],
        response={"outcome": "failed", "failure": {"stage": "runtime", "category": "uncaught-error",
                                                    "error_type": "NotFound", "message": "key 7"}}),
    rec("L06.segv", "CRASH", reasons=["adapter process terminated by signal 11 without a response"], signal=11),
    rec("L07.out-a", "FAIL", reasons=["stdout differs at character 3: expected 'abc', got 'abd'"],
        response={"outcome": "completed", "exit_status": 0}),
    rec("L07.out-b", "FAIL", reasons=["stdout differs at character 9: expected 'x', got 'y'"],
        response={"outcome": "completed", "exit_status": 0}, stderr="ERROR_CODE: printed by the program\n"),
    rec("L07.exit", "FAIL", reasons=["exit status 1, expected 0"], response={"outcome": "completed", "exit_status": 1}),
    rec("L08.slow", "TIMEOUT", reasons=["no observation within the time budget"]),
    rec("L09.test", "UNSUPPORTED", mode="test", response=unsupported("unsupported in OIR-2 lowering: test profile")),
    rec("L10.driver", "CRASH",
        response=internal("Champ OIR-2 driver is not yet available: requires #572 and #573-P1")),
    rec("L11.frontend", "UNSUPPORTED",
        response=unsupported("/w/main.ok:3:7: analysis limitation: generic role `Foo` in position 2")),
    rec("L12.blocked", "BLOCKED", reasons=["environment unavailable: network (no route)"]),
]
SPARROW = [rec(r["id"], "PASS") for r in CHAMP]
SPARROW[1] = rec("L01.both", "FAIL", reasons=["stdout differs"])

EXPECTED_STAGE = {
    "L00.pass": ("pass", None),
    "L01.both": ("both", None),
    "L02.closure-a": ("lowering", "unsupported: closure capture"),
    "L02.closure-b": ("lowering", "unsupported: closure capture"),
    "L02.closure-c": ("lowering", "unsupported: closure capture"),
    "L03.defer": ("lowering", "unsupported: rebind of defer capture"),
    "L04.map-a": ("generation", "S1 type Map"),
    "L04.map-b": ("generation", "S1 type Map"),
    "L04.cleanup": ("generation", "S2 cleanup.call"),
    "L05.loader": ("loader", "loader OPERAND_VALUE"),
    "L05.loader-run": ("loader", "loader LINEAR_CONSUME"),
    "L06.panic": ("runtime", "runtime: panic boom N"),
    "L06.uncaught": ("runtime", "runtime: uncaught-error NotFound"),
    "L06.segv": ("runtime", "crash: signal 11"),
    "L07.out-a": ("wrong-output", "wrong output: stdout differs"),
    "L07.out-b": ("wrong-output", "wrong output: stdout differs"),
    "L07.exit": ("wrong-output", "wrong output: exit status differs"),
    "L08.slow": ("timeout", "timeout"),
    "L09.test": ("test-mode", "test mode: okkama run -t is not supported under OIR-2"),
    "L10.driver": ("driver", "driver: Champ OIR-N driver is not yet available: requires #N and #N-PN"),
    "L11.frontend": ("unsupported", "unsupported (other): analysis limitation: generic role `_` in position N"),
    "L12.blocked": ("not-run", "not run: environment unavailable: network (no route)"),
}


def write_jsonl(path, records):
    with open(path, "w") as fh:
        for r in records:
            fh.write(json.dumps(r, sort_keys=True) + "\n")


class ClassifierTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.champ = os.path.join(self.tmp.name, "champ")
        os.makedirs(self.champ)
        write_jsonl(os.path.join(self.champ, "results.jsonl"), CHAMP)
        self.sparrow = os.path.join(self.tmp.name, "sparrow.jsonl")
        write_jsonl(self.sparrow, SPARROW)

    def tearDown(self):
        self.tmp.cleanup()

    def run_tool(self, *args):
        out = io.StringIO()
        with redirect_stdout(out):
            self.assertEqual(blockers.main(list(args)), 0)
        return out.getvalue()

    def test_every_case_gets_its_stage_and_key(self):
        doc = blockers.analyse(blockers.load_results(self.champ), blockers.load_results(self.sparrow))
        got = {cid: (c["stage"], c["key"]) for cid, c in doc["cases"].items()}
        self.assertEqual(got, EXPECTED_STAGE)

    def test_totals_stages_and_ranking(self):
        doc = blockers.analyse(blockers.load_results(self.champ), blockers.load_results(self.sparrow))
        self.assertEqual(doc["totals"], {"cases": 22, "pass": 1, "both": 1, "champ_only": 20})
        self.assertEqual(doc["stages"]["lowering"], 4)
        self.assertEqual(doc["stages"]["generation"], 3)
        self.assertEqual(sum(doc["stages"].values()), 22)
        first, second = doc["blockers"][:2]
        self.assertEqual((first["key"], first["count"]), ("unsupported: closure capture", 3))
        self.assertEqual(first["examples"], ["L02.closure-a", "L02.closure-b", "L02.closure-c"])
        self.assertEqual(second["count"], 2)
        counts = [b["count"] for b in doc["blockers"]]
        self.assertEqual(counts, sorted(counts, reverse=True))
        self.assertEqual(sum(counts), 20)

    def test_without_baseline_everything_failing_is_champ_only(self):
        doc = blockers.analyse(blockers.load_results(self.champ))
        self.assertEqual(doc["totals"]["both"], 0)
        self.assertEqual(doc["cases"]["L01.both"]["stage"], "wrong-output")

    def test_examples_are_capped(self):
        many = [rec("L02.c%02d" % i, "UNSUPPORTED",
                    response=unsupported("unsupported in OIR-2 lowering: closure capture of v%d" % i))
                for i in range(8)]
        doc = blockers.analyse({r["id"]: r for r in many})
        self.assertEqual(doc["blockers"][0]["count"], 8)
        self.assertEqual(len(doc["blockers"][0]["examples"]), blockers.EXAMPLES)
        self.assertEqual(len(doc["blockers"][0]["cases"]), 8)

    def test_text_and_json_output(self):
        out_json = os.path.join(self.tmp.name, "b.json")
        text = self.run_tool(self.champ, "--baseline", self.sparrow, "--json", out_json)
        self.assertIn("cases 22: pass 1, fails on both 1, Champ-only failures 20", text)
        self.assertIn("unsupported in OIR-2 lowering", text)
        self.assertIn("[lowering] unsupported: closure capture", text)
        with open(out_json) as fh:
            doc = json.load(fh)
        self.assertEqual(doc["schema"], blockers.SCHEMA)
        self.assertEqual(doc["totals"]["champ_only"], 20)
        self.assertTrue(doc["baseline_run"].endswith("sparrow.jsonl"))

    def test_compare(self):
        previous = os.path.join(self.tmp.name, "previous.json")
        self.run_tool(self.champ, "--baseline", self.sparrow, "--json", previous)
        today = [dict(r) for r in CHAMP]
        # Closures got lowered: two pass now, one hits a generation gap.
        today[2] = rec("L02.closure-a", "PASS")
        today[3] = rec("L02.closure-b", "PASS")
        today[4] = rec("L02.closure-c", "UNSUPPORTED",
                       response=unsupported("OIR-2 → SPB 17: closure.env: not yet in S1"))
        # A regression.
        today[0] = rec("L00.pass", "FAIL", reasons=["exit status 1, expected 0"],
                       response={"outcome": "completed", "exit_status": 1})
        champ2 = os.path.join(self.tmp.name, "champ2.jsonl")
        write_jsonl(champ2, today)
        out_json = os.path.join(self.tmp.name, "today.json")
        text = self.run_tool(champ2, "--baseline", self.sparrow, "--json", out_json, "--compare", previous)
        with open(out_json) as fh:
            c = json.load(fh)["compare"]
        self.assertEqual(c["new_passes"], ["L02.closure-a", "L02.closure-b"])
        self.assertEqual(c["new_failures"], ["L00.pass"])
        moved = {m["key"]: (m["before"], m["now"]) for m in c["moved_blockers"]}
        self.assertEqual(moved["unsupported: closure capture"], (3, 0))
        self.assertEqual(moved["S1 closure.env"], (0, 1))
        self.assertEqual(moved["wrong output: exit status differs"], (1, 2))
        self.assertEqual(c["rekeyed_cases"], ["L02.closure-c"])
        self.assertIn("new passes (2): L02.closure-a, L02.closure-b", text)
        self.assertIn("(gone)", text)
        self.assertIn("(new)", text)


class NormalisationTests(unittest.TestCase):
    def test_construct_keys(self):
        n = blockers.normalise_construct
        self.assertEqual(n("closure capture of x in function app.main"), "closure capture")
        self.assertEqual(n("type Map(Int, Str)"), "type Map")
        self.assertEqual(n("generic object, capability or role witness"), "generic object, capability or role witness")
        self.assertEqual(n("field write from defer (#567 S3)"), "field write from defer")

    def test_text_keys(self):
        n = blockers.normalise_text
        self.assertEqual(n("okkama: /tmp/ocs-1/work/main.ok:12:4: semantic error: `x` at 12..14"),
                         "semantic error: `_` at")
        self.assertEqual(n('message "different every time" 0x1f 99'), 'message "_" N N')


if __name__ == "__main__":
    unittest.main()
