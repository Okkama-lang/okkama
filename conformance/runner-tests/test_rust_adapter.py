#!/usr/bin/env python3
"""The Rust adapter: engine selection, per-engine build caches, and Champ's CLI translation.

Nothing here builds or runs okkama: the build is mocked, and the Champ path is
driven against a fake `okkama` script through a pre-seeded probe cache.
"""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

SUITE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SUITE / "runner"))
from ocs.execute import Adapter  # noqa: E402
from ocs.judge import validate_response  # noqa: E402

path = SUITE / "adapters/rust/okkama_rust_adapter.py"
spec = importlib.util.spec_from_file_location("rust_adapter", path)
adapter = importlib.util.module_from_spec(spec)
spec.loader.exec_module(adapter)


class ProbeCacheTests(unittest.TestCase):
    def test_default_engine_is_sparrow(self):
        with tempfile.TemporaryDirectory() as directory:
            with patch.dict(adapter.os.environ, {
                "OCS_RUST_IMPLEMENTATION_ROOT": directory,
                "OCS_RUST_TARGET_DIR": directory,
            }), patch.object(adapter, "source_fingerprint", return_value="same-source"), \
                    patch.object(adapter, "probe_uncached", return_value={"build_exit": 0, "modes": ["run"]}) as build:
                adapter.os.environ.pop("OCS_RUST_ENGINE", None)
                self.assertEqual(adapter.probe()["engine"], "sparrow")
                self.assertTrue(adapter.probe()["cached"])
                self.assertEqual(build.call_count, 1)

    def test_removed_engine_is_rejected(self):
        for engine in ("ir", "jvm"):
            with patch.dict(adapter.os.environ, {"OCS_RUST_ENGINE": engine}):
                with self.assertRaisesRegex(SystemExit, "Sparrow is the only"):
                    adapter.probe()

    def test_unknown_engine_is_rejected(self):
        with patch.dict(adapter.os.environ, {"OCS_RUST_ENGINE": "tomato"}):
            with self.assertRaisesRegex(SystemExit, "unknown OCS_RUST_ENGINE 'tomato'"):
                adapter.engine_name()

    def test_champ_is_selectable(self):
        with patch.dict(adapter.os.environ, {"OCS_RUST_ENGINE": "champ"}):
            self.assertEqual(adapter.engine_name(), "champ")

    def test_builds_and_target_directories_are_per_engine(self):
        with patch.dict(adapter.os.environ, {"OCS_RUST_TARGET_DIR": "/t/base"}):
            self.assertEqual(adapter.target_dir("sparrow"), "/t/base")
            self.assertEqual(adapter.target_dir(), "/t/base")
            self.assertEqual(adapter.target_dir("champ"), "/t/base/engine-champ")
        # Sparrow's build is exactly the historical one.
        self.assertEqual(adapter.build_command("sparrow"), ["cargo", "build", "--quiet", "--offline"])
        champ = adapter.build_command("champ")
        self.assertEqual(champ[:4], ["cargo", "build", "--quiet", "--offline"])
        self.assertIn("champ-abi", champ[champ.index("--features") + 1])
        self.assertEqual(champ[champ.index("-p") + 1], "okkama-cli")
        # Champ builds with the gate profile (owner, 2026-11-20: a debug build compiles the SQLite cases over their
        # 60 s budget); its binary is found under that profile's directory.
        self.assertEqual(champ[champ.index("--profile") + 1], "gate")
        self.assertEqual(adapter.ENGINES["champ"]["profile"], "gate")
        self.assertEqual(adapter.ENGINES["sparrow"]["profile"], "debug")
        self.assertNotIn("--profile", adapter.build_command("sparrow"))

    def test_binary_environment(self):
        self.assertIsNone(adapter.binary_env("sparrow"))
        with patch.dict(adapter.os.environ, {"OCS_RUST_ENGINE": "champ", "KEEP": "1"}):
            env = adapter.binary_env("champ")
        self.assertEqual(env["OKKAMA_PIPELINE"], "oir2")
        # The native endpoint only knows the `sparrow` code generator name.
        self.assertEqual(env["OCS_RUST_ENGINE"], "sparrow")
        self.assertEqual(env["KEEP"], "1")

    def _probe(self, directory, engine, uncached):
        env = {"OCS_RUST_IMPLEMENTATION_ROOT": directory, "OCS_RUST_TARGET_DIR": os.path.join(directory, "t"),
               "OCS_RUST_ENGINE": engine}
        with patch.dict(adapter.os.environ, env), \
                patch.object(adapter, "source_fingerprint", return_value="same-source"), \
                patch.object(adapter, "probe_uncached", side_effect=uncached) as build:
            return adapter.probe(), build

    def test_engines_never_share_a_cache_entry(self):
        calls = []

        def uncached(engine="sparrow"):
            calls.append(engine)
            return {"build_exit": 0, "modes": ["run"], "binary": "/%s/okkama" % engine}

        with tempfile.TemporaryDirectory() as directory:
            sparrow, _ = self._probe(directory, "sparrow", uncached)
            champ, _ = self._probe(directory, "champ", uncached)
            again_sparrow, _ = self._probe(directory, "sparrow", uncached)
            again_champ, _ = self._probe(directory, "champ", uncached)
            self.assertEqual(calls, ["sparrow", "champ"])
            self.assertEqual(again_sparrow["binary"], "/sparrow/okkama")
            self.assertEqual(again_champ["binary"], "/champ/okkama")
            self.assertTrue(again_sparrow["cached"] and again_champ["cached"])
            self.assertEqual(champ["target_dir"], os.path.join(directory, "t", "engine-champ"))
            self.assertEqual(sparrow["target_dir"], os.path.join(directory, "t"))
            self.assertTrue(os.path.isfile(os.path.join(directory, "t", "ocs-probe.json")))
            self.assertTrue(os.path.isfile(os.path.join(directory, "t", "engine-champ", "ocs-probe.json")))
            with open(os.path.join(directory, "t", "engine-champ", "ocs-probe.json")) as fh:
                self.assertEqual(list(json.load(fh)["engines"]), ["champ"])

    def test_cache_key_includes_the_build_command(self):
        with tempfile.TemporaryDirectory() as directory:
            target = os.path.join(directory, "t")
            os.makedirs(os.path.join(target, "engine-champ"))
            # A Sparrow entry from before engines recorded their build: reused.
            legacy = {"fingerprint": "same-source", "engine": "sparrow", "build_exit": 0, "binary": "/old"}
            with open(os.path.join(target, "ocs-probe.json"), "w") as fh:
                json.dump({"engines": {"sparrow": legacy}}, fh)
            # A Champ entry built without champ-abi: rebuilt.
            wrong = dict(legacy, engine="champ", build_command=adapter.build_command("sparrow"),
                         target_dir=os.path.join(target, "engine-champ"))
            with open(os.path.join(target, "engine-champ", "ocs-probe.json"), "w") as fh:
                json.dump({"engines": {"champ": wrong}}, fh)
            fresh = lambda engine="sparrow": {"build_exit": 0, "modes": ["run"], "binary": "/new"}  # noqa: E731
            ev, build = self._probe(directory, "sparrow", fresh)
            self.assertEqual((ev["binary"], build.call_count), ("/old", 0))
            ev, build = self._probe(directory, "champ", fresh)
            self.assertEqual((ev["binary"], build.call_count), ("/new", 1))

    def test_failed_build_is_not_cached(self):
        with tempfile.TemporaryDirectory() as directory:
            failing = lambda engine="sparrow": {"build_exit": 101, "modes": []}  # noqa: E731
            self._probe(directory, "champ", failing)
            _, build = self._probe(directory, "champ", failing)
            self.assertEqual(build.call_count, 1)


class ConfigTests(unittest.TestCase):
    def test_champ_config_selects_champ(self):
        sparrow = Adapter(str(SUITE / "adapters/rust/adapter.json"))
        champ = Adapter(str(SUITE / "adapters/rust/adapter-champ.json"))
        self.assertEqual(champ.config["command"], sparrow.config["command"])
        self.assertNotEqual(champ.name, sparrow.name)
        env = champ.env(str(SUITE))
        self.assertEqual(env["OCS_RUST_ENGINE"], "champ")
        self.assertEqual(env["OKKAMA_PIPELINE"], "oir2")
        self.assertEqual(env["OCS_RUST_TARGET_DIR"], sparrow.env(str(SUITE))["OCS_RUST_TARGET_DIR"])
        self.assertEqual(sparrow.env(str(SUITE))["OCS_RUST_ENGINE"], "sparrow")
        self.assertNotIn("OKKAMA_PIPELINE", sparrow.env(str(SUITE)))

    def test_overrides_still_win(self):
        champ = Adapter(str(SUITE / "adapters/rust/adapter-champ.json"), {"OCS_RUST_TARGET_DIR": "/elsewhere"})
        self.assertEqual(champ.env(str(SUITE))["OCS_RUST_TARGET_DIR"], "/elsewhere")


class ChampTranslationTests(unittest.TestCase):
    def outcome(self, code, stderr, mode="run"):
        fields, exit_code = adapter.champ_outcome(mode, code, stderr)
        doc = dict(fields, protocol=adapter.PROTOCOL, mode=mode)
        self.assertIsNone(validate_response(doc, mode), doc)
        if fields["outcome"] == "completed":
            self.assertEqual(exit_code, fields["exit_status"])
        if fields["outcome"] == "failed":
            self.assertNotEqual(exit_code, 0)
        return fields, exit_code

    def test_command_line(self):
        req = {"sources": ["/w/main.ok", "/w/part.ok"], "args": ["a.ok", "-x", ""], "stdlib_root": "/std"}
        self.assertEqual(adapter.champ_command("/b/okkama", "run", req),
                         ["/b/okkama", "run", "--stdlib", "/std", "/w/main.ok", "/w/part.ok", "--", "a.ok", "-x", ""])
        self.assertEqual(adapter.champ_command("/b/okkama", "test", req)[:3], ["/b/okkama", "run", "-t"])

    def test_empty_test_suite_is_a_runtime_failure(self):
        fields, exit_code = self.outcome(1, "okkama: Test run requires at least one test\n", mode="test")
        self.assertEqual(fields["outcome"], "failed")
        self.assertEqual(fields["failure"]["category"], "runtime")
        self.assertEqual(fields["failure"]["message"], "Test run requires at least one test")
        self.assertEqual(exit_code, 1)
        # Outside test mode the same prefix still means the pipeline or the driver.
        self.assertEqual(self.outcome(1, "okkama: something else\n")[0]["outcome"], "internal-error")

    def test_completed(self):
        self.assertEqual(self.outcome(0, "")[0]["exit_status"], 0)
        self.assertEqual(self.outcome(3, "")[1], 3)
        # main() returning 1 with no failure report.
        self.assertEqual(self.outcome(1, "some note\n")[0]["outcome"], "completed")
        # Only status 1 carries a report: other statuses are main()'s result.
        fields, code = self.outcome(2, "Warning: careful\n")
        self.assertEqual((fields["outcome"], code), ("completed", 2))
        # A silent 4 is main()'s result, not the CLI's unsupported status, which always comes with its report
        # (L01.gram.test.inner-grammar-not-parsed).
        fields, code = self.outcome(4, "")
        self.assertEqual((fields["outcome"], code), ("completed", 4))

    def test_lowering_limit_is_unsupported(self):
        fields, code = self.outcome(4, "okkama: 10..20: unsupported in OIR-2 lowering: closure capture of x\n")
        self.assertEqual(fields["outcome"], "unsupported")
        self.assertTrue(fields["unsupported_reason"].startswith("10..20: unsupported in OIR-2 lowering"))
        self.assertEqual(code, 3)

    def test_test_mode_is_unsupported(self):
        fields, _ = self.outcome(4, "okkama: unsupported in OIR-2 lowering: test profile\n", mode="test")
        self.assertEqual(fields["unsupported_reason"], "unsupported in OIR-2 lowering: test profile")

    def test_generation_gap_is_unsupported(self):
        fields, _ = self.outcome(1, 'okkama: OIR-2 compile_low: GenerateError("type Map(..): not yet in S1")\n')
        self.assertEqual(fields["outcome"], "unsupported")
        self.assertIn("not yet in S1", fields["unsupported_reason"])

    def test_pipeline_and_driver_failures_are_internal(self):
        for stderr in ('okkama: OIR-2 verify_load: Error { offset: None, message: "m", rule: "OPERAND_VALUE" }\n',
                       "okkama: OIR-2 verify_own: broken\n",
                       "Champ OIR-2 driver is not yet available: requires #572 and #573-P1\n",
                       "okkama: this build has the champ-abi feature and runs only SPB 17: set ...\n"):
            fields, code = self.outcome(1, stderr)
            self.assertEqual((fields["outcome"], code), ("internal-error", 70), stderr)

    def test_load_rejection_at_run_is_internal(self):
        fields, _ = self.outcome(1, "LINEAR_CONSUME: value consumed twice\n")
        self.assertEqual(fields["outcome"], "internal-error")
        self.assertEqual(fields["failure"]["message"], "LINEAR_CONSUME: value consumed twice")

    def test_runtime_reports(self):
        cases = [
            ("Panic: boom\n", "panic", None, "boom"),
            ("RuntimeError: division by zero\n", "runtime", None, "division by zero"),
            ("NotFound: no such key\n", "uncaught-error", "NotFound", "no such key"),
            ("Uncaught Oops\n", "uncaught-error", "Oops", ""),
            ("NativeFailure: denied\ncode: 13\ndomain: os\noperation: open\n", "runtime", "NativeFailure", "denied"),
            ("Execution budget exhausted: steps\n", "runtime", None, "Execution budget exhausted: steps"),
            # The program's own stderr comes first; the report is last.
            ("Note: starting\nPanic: late\n", "panic", None, "late"),
            # Nested entries after a label are not the root.
            ("BadInput: root\nCaused by:\n  Panic: inner\n", "uncaught-error", "BadInput", "root"),
        ]
        for stderr, category, error_type, message in cases:
            fields, code = self.outcome(1, stderr)
            self.assertEqual(fields["outcome"], "failed", stderr)
            failure = fields["failure"]
            self.assertEqual((failure["category"], failure.get("error_type"), failure["message"]),
                             (category, error_type, message), stderr)
            self.assertEqual(failure["raw"], stderr)
            self.assertEqual(code, 1)

    def test_signal_is_internal(self):
        fields, code = self.outcome(-11, "")
        self.assertEqual((fields["outcome"], code), ("internal-error", 70))


FAKE_OKKAMA = r'''#!%(python)s
import json, os, sys
log = os.environ["FAKE_LOG"]
with open(log, "a") as fh:
    fh.write(json.dumps({"argv": sys.argv[1:], "pipeline": os.environ.get("OKKAMA_PIPELINE"),
                         "engine": os.environ.get("OCS_RUST_ENGINE"), "cwd": os.getcwd()}) + "\n")
args = sys.argv[1:]
if args[0] == "--ocs-request":
    req = json.load(open(args[1]))
    text = open(req["sources"][0]).read()
    if "REJECT" in text:
        sys.stderr.write("main.ok:1:1: semantic error: nope\n")
        resp = {"protocol": "ocs-adapter/2", "mode": req["mode"], "outcome": "rejected", "stage_reached": "check",
                "failure": {"stage": "check", "category": "semantic", "message": "main.ok:1:1: semantic error: nope"}}
        code = 1
    else:
        resp = {"protocol": "ocs-adapter/2", "mode": req["mode"], "outcome": "accepted", "stage_reached": "check"}
        code = 0
    json.dump(resp, open(args[3], "w"))
    sys.exit(code)
assert args[0] == "run"
if "-t" in args:
    sys.stderr.write("okkama: unsupported in OIR-2 lowering: test profile\n")
    sys.exit(4)
sources = [a for a in args if a.endswith(".ok")]
text = open(sources[0]).read()
program_args = args[args.index("--") + 1:]
if "PANIC" in text:
    sys.stdout.write("partial\n")
    sys.stderr.write("Panic: boom\n")
    sys.exit(1)
if "UNSUPPORTED" in text:
    sys.stderr.write("okkama: 3..9: unsupported in OIR-2 lowering: closure capture of x in function main\n")
    sys.exit(4)
sys.stdout.write("hello %%s\n" %% " ".join(program_args))
stdin = sys.stdin.read()
if stdin:
    sys.stdout.write("stdin " + stdin)
sys.exit(0)
'''


class ChampEndToEndTests(unittest.TestCase):
    """The adapter process on the Champ engine, against a fake okkama."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = self.tmp.name
        self.root = os.path.join(self.dir, "impl")
        os.makedirs(self.root)
        base = os.path.join(self.dir, "target")
        champ_target = os.path.join(base, "engine-champ")
        os.makedirs(champ_target)
        self.fake = os.path.join(self.dir, "okkama")
        with open(self.fake, "w") as fh:
            fh.write(FAKE_OKKAMA % {"python": sys.executable})
        os.chmod(self.fake, 0o755)
        self.log = os.path.join(self.dir, "log.jsonl")
        # A probe cache as a successful champ-abi build would leave it.
        ev = {"fingerprint": adapter.source_fingerprint(self.root), "engine": "champ", "build_exit": 0,
              "build_command": adapter.build_command("champ"), "target_dir": champ_target,
              "binary": self.fake, "modes": ["parse", "check", "run", "test"], "root": self.root}
        with open(os.path.join(champ_target, "ocs-probe.json"), "w") as fh:
            json.dump({"engines": {"champ": ev}}, fh)
        self.env = {"PATH": os.environ.get("PATH", ""), "OCS_RUST_IMPLEMENTATION_ROOT": self.root,
                    "OCS_RUST_TARGET_DIR": base, "OCS_RUST_ENGINE": "champ", "FAKE_LOG": self.log}
        self.work = os.path.join(self.dir, "work")
        os.makedirs(self.work)

    def tearDown(self):
        self.tmp.cleanup()

    def request(self, mode, source, args=(), stdin=b""):
        main = os.path.join(self.work, "main.ok")
        with open(main, "w") as fh:
            fh.write(source)
        req = {"protocol": "ocs-adapter/2", "mode": mode, "sources": [main], "args": list(args), "cwd": self.work,
               "stdlib_root": "/std", "timeout_s": 10}
        req_path = os.path.join(self.dir, "request.json")
        resp_path = os.path.join(self.dir, "response.json")
        if os.path.exists(resp_path):
            os.remove(resp_path)
        with open(req_path, "w") as fh:
            json.dump(req, fh)
        p = subprocess.run([sys.executable, "-I", str(path), "--request", req_path, "--response", resp_path],
                           cwd=self.work, input=stdin, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                           env=self.env, timeout=60)
        with open(resp_path) as fh:
            resp = json.load(fh)
        self.assertIsNone(validate_response(resp, mode), resp)
        log = []
        if os.path.exists(self.log):
            with open(self.log) as fh:
                log = [json.loads(line) for line in fh]
            os.remove(self.log)
        return p, resp, log

    def test_run_passes_streams_and_selects_oir2(self):
        p, resp, log = self.request("run", "ok", args=["a", "b.ok"], stdin=b"in\n")
        self.assertEqual(p.stdout, b"hello a b.ok\nstdin in\n")
        self.assertEqual(p.stderr, b"")
        self.assertEqual((resp["outcome"], resp["exit_status"], p.returncode), ("completed", 0, 0))
        self.assertEqual(resp["evidence"]["engine"], "champ")
        frontend, cli = log
        self.assertEqual(frontend["argv"][0], "--ocs-request")
        self.assertEqual((frontend["pipeline"], frontend["engine"]), ("oir2", "sparrow"))
        self.assertEqual(cli["argv"][:3], ["run", "--stdlib", "/std"])
        self.assertEqual(cli["pipeline"], "oir2")
        self.assertEqual(os.path.realpath(cli["cwd"]), os.path.realpath(self.work))

    def test_runtime_failure(self):
        p, resp, _ = self.request("run", "PANIC")
        self.assertEqual(p.stdout, b"partial\n")
        self.assertEqual(p.stderr, b"Panic: boom\n")
        self.assertEqual(resp["outcome"], "failed")
        self.assertEqual((resp["failure"]["category"], p.returncode), ("panic", 1))

    def test_frontend_rejection_keeps_its_stage(self):
        p, resp, log = self.request("run", "REJECT")
        self.assertEqual((resp["outcome"], resp["mode"]), ("rejected", "run"))
        self.assertEqual(resp["failure"]["category"], "semantic")
        self.assertEqual(p.stderr, b"main.ok:1:1: semantic error: nope\n")
        self.assertEqual(len(log), 1)  # the CLI never ran

    def test_lowering_limit_and_test_mode_are_classified_unsupported(self):
        _, resp, _ = self.request("run", "UNSUPPORTED")
        self.assertEqual(resp["outcome"], "unsupported")
        self.assertIn("unsupported in OIR-2 lowering: closure capture", resp["unsupported_reason"])
        p, resp, log = self.request("test", "ok")
        self.assertEqual(resp["outcome"], "unsupported")
        self.assertIn("test profile", resp["unsupported_reason"])
        self.assertEqual(log[0]["argv"][:2], ["run", "-t"])

    def test_check_mode_goes_to_the_native_endpoint(self):
        p, resp, log = self.request("check", "ok")
        self.assertEqual((resp["outcome"], p.returncode), ("accepted", 0))
        self.assertEqual([entry["argv"][0] for entry in log], ["--ocs-request"])
        self.assertEqual(log[0]["pipeline"], "oir2")


if __name__ == "__main__":
    unittest.main()
