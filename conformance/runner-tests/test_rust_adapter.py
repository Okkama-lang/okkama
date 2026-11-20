#!/usr/bin/env python3
"""The Rust adapter: Sparrow, the only engine (Tweed, 2026-11-20), and its build cache.

Nothing here builds or runs okkama: the build is mocked.
"""
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

SUITE = Path(__file__).resolve().parents[1]

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

    def test_removed_engines_are_rejected(self):
        for engine in ("ir", "jvm", "champ"):
            with patch.dict(adapter.os.environ, {"OCS_RUST_ENGINE": engine}):
                with self.assertRaisesRegex(SystemExit, "Sparrow is the only"):
                    adapter.probe()

    def test_unknown_engine_is_rejected(self):
        with patch.dict(adapter.os.environ, {"OCS_RUST_ENGINE": "tomato"}):
            with self.assertRaisesRegex(SystemExit, "unknown OCS_RUST_ENGINE 'tomato'"):
                adapter.engine_name()

    def test_the_build_and_its_target_directory(self):
        with patch.dict(adapter.os.environ, {"OCS_RUST_TARGET_DIR": "/t/base"}):
            self.assertEqual(adapter.target_dir("sparrow"), "/t/base")
            self.assertEqual(adapter.target_dir(), "/t/base")
        self.assertEqual(adapter.build_command("sparrow"), ["cargo", "build", "--quiet", "--offline"])
        self.assertEqual(adapter.ENGINES["sparrow"]["profile"], "debug")
        self.assertIsNone(adapter.binary_env("sparrow"))

    def _probe(self, directory, uncached):
        env = {"OCS_RUST_IMPLEMENTATION_ROOT": directory, "OCS_RUST_TARGET_DIR": os.path.join(directory, "t"),
               "OCS_RUST_ENGINE": "sparrow"}
        with patch.dict(adapter.os.environ, env), \
                patch.object(adapter, "source_fingerprint", return_value="same-source"), \
                patch.object(adapter, "probe_uncached", side_effect=uncached) as build:
            return adapter.probe(), build

    def test_an_entry_from_before_builds_were_recorded_is_reused(self):
        with tempfile.TemporaryDirectory() as directory:
            target = os.path.join(directory, "t")
            os.makedirs(target)
            legacy = {"fingerprint": "same-source", "engine": "sparrow", "build_exit": 0, "binary": "/old"}
            with open(os.path.join(target, "ocs-probe.json"), "w") as fh:
                json.dump({"engines": {"sparrow": legacy}}, fh)
            fresh = lambda engine="sparrow": {"build_exit": 0, "modes": ["run"], "binary": "/new"}  # noqa: E731
            ev, build = self._probe(directory, fresh)
            self.assertEqual((ev["binary"], build.call_count), ("/old", 0))

    def test_failed_build_is_not_cached(self):
        with tempfile.TemporaryDirectory() as directory:
            failing = lambda engine="sparrow": {"build_exit": 101, "modes": []}  # noqa: E731
            self._probe(directory, failing)
            _, build = self._probe(directory, failing)
            self.assertEqual(build.call_count, 1)


if __name__ == "__main__":
    unittest.main()
