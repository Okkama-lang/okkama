"""Generator tests: recorded production ASTs, no compiler installation required."""
import contextlib
import copy
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import actions


class ActionTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.recording = json.loads((actions.HERE / "fixtures/actions-ast.json").read_text())

    def modules(self):
        return {m: actions.parse_ast(tree) for m, tree in self.recording.items()}

    def manifest(self):
        return {"hex-decode": {"targets": ["encoding/hex"], "actions": ["encoding/hex:HexDecoded.bytes"]}}

    def test_recorded_ast(self):
        modules = self.modules()
        self.assertEqual(modules["encoding/hex"], {
            "HexDecoded.bytes": ["HexDecoded.bytes() is @byte"],
            "HexDecoded.new": ["HexDecoded.new(origin is string) is HexDecoded"],
            "HexEncoded.new": ["HexEncoded.new(origin is @byte) is HexEncoded"],
            "HexEncoded.string": ["HexEncoded.string() is string"],
            "InvalidHex.at": ["InvalidHex.at(offset is int) is InvalidHex"],
        })
        listing = modules["collections/list"]
        self.assertIn("EmptyList.new", listing)
        self.assertEqual(listing["List.prefix"], ["List.prefix(value is T) is List<T> [T]"])
        self.assertFalse(any(a.startswith("LinkedList.") for a in listing))
        buffer = modules["strings/buffer"]
        self.assertTrue(any("... is string" in s for s in buffer["TextBuffer.write"]))
        self.assertEqual(modules["checksum/adler32"]["Adler32.new"], [
            "Adler32.new() is Adler32",
            "Adler32.new(origin is @byte) is Adler32",
            "Adler32.new(origin is @byte, initial is int) is Adler32",
        ])

    def test_hidden_actions_and_shorthand_types(self):
        tree = copy.deepcopy(self.recording["encoding/hex"])
        owner = tree["items"][0]
        action = next(m for m in owner["members"] if m["node"] == "ActionDecl")
        action["visibility"] = "hidden"
        constructor = next(m for m in owner["members"] if m["node"] == "FullConstructor")
        constructor.clear()
        constructor.update(node="ShorthandConstructor", name="HexEncoded", fields=["origin"])
        parsed = actions.parse_ast(tree)
        self.assertNotIn("HexEncoded.string", parsed)
        self.assertEqual(parsed["HexEncoded.new"], ["HexEncoded.new(origin is @byte) is HexEncoded"])
        owner["hidden"] = True
        self.assertFalse(any(a.startswith("HexEncoded.") for a in actions.parse_ast(tree)))

    def test_types(self):
        integer = {"node": "NamedType", "path": ["int"], "args": []}
        self.assertEqual(actions.type_text({"node": "MapType", "key": integer, "value": integer}), "@int:int")
        self.assertEqual(actions.type_text({"node": "NestedType", "owner": integer, "name": "Cap"}), "int::Cap")
        self.assertEqual(actions.type_text({"node": "SuccessorResult", "type": integer}), "^int")
        self.assertEqual(actions.type_text({"node": "ActionType", "async": True, "params": [integer], "result": {"node": "VoidResult"}}), "async (int) is void")
        with self.assertRaisesRegex(ValueError, "unsupported AST type"):
            actions.type_text({"node": "UnknownType"})

    def test_deterministic(self):
        modules = self.modules()
        expected = actions.report(modules, self.manifest())
        reversed_modules = {m: {a: list(reversed(s)) for a, s in reversed(list(v.items()))}
                            for m, v in reversed(list(modules.items()))}
        self.assertEqual(actions.report(reversed_modules, self.manifest()), expected)
        recording = copy.deepcopy(self.recording["encoding/hex"])
        recording["items"].reverse()
        for item in recording["items"]:
            item.get("members", []).reverse()
        self.assertEqual(actions.parse_ast(recording), modules["encoding/hex"])
        self.assertIn("1 of ", expected)
        self.assertIn("HexDecoded.bytes() is @byte", expected)
        self.assertIn("hex-decode", expected)

    def test_invalid_targets(self):
        for target in ["encoding/hex:HexDecoded.typo", "runtime/hex:HexDecoded.bytes", "bad", "builtin:typo"]:
            manifest = self.manifest()
            manifest["hex-decode"]["actions"] = [target]
            with self.subTest(target=target), self.assertRaisesRegex(ValueError, "unknown"):
                actions.report(self.modules(), manifest)
        manifest = self.manifest()
        manifest["hex-decode"]["targets"] = []
        with self.assertRaisesRegex(ValueError, "missing from targets"):
            actions.report(self.modules(), manifest)
        manifest = self.manifest()
        manifest["hex-decode"]["actions"] *= 2
        with self.assertRaisesRegex(ValueError, "duplicate"):
            actions.report(self.modules(), manifest)

    def test_check_staleness(self):
        with tempfile.TemporaryDirectory(dir=actions.HERE) as d:
            root = Path(d)
            manifest = root / "manifest.json"
            manifest.write_text(json.dumps(self.manifest()))
            output = root / "STD-ACTIONS.md"
            argv = ["--manifest", str(manifest), "--output", str(output)]
            with patch.object(actions.shutil, "which", return_value="/fixture/okkama"), patch.object(actions, "inventory", return_value=self.modules()), contextlib.redirect_stderr(io.StringIO()) as errors:
                self.assertEqual(actions.main(argv + ["--check"]), 1)
                self.assertFalse(output.exists())
                self.assertEqual(actions.main(argv), 0)
                original = output.read_text()
                self.assertEqual(actions.main(argv + ["--check"]), 0)
                output.write_text(original + "stale")
                self.assertEqual(actions.main(argv + ["--check"]), 1)
                self.assertEqual(output.read_text(), original + "stale")
                output.write_text(original)
                changed = self.manifest()
                changed["hex-decode"]["actions"] = []
                manifest.write_text(json.dumps(changed))
                self.assertEqual(actions.main(argv + ["--check"]), 1)
                self.assertIn("stale:", errors.getvalue())

    def test_missing_binary(self):
        with patch.object(actions.shutil, "which", return_value=None), contextlib.redirect_stderr(io.StringIO()) as errors:
            self.assertEqual(actions.main(["--okkama", "missing-okkama"]), 1)
        self.assertIn("okkama executable not found: missing-okkama", errors.getvalue())

    def test_ast_command_and_errors(self):
        import subprocess
        result = subprocess.CompletedProcess([], 0, json.dumps(self.recording["encoding/hex"]), "")
        with patch.object(actions.subprocess, "run", return_value=result) as run:
            self.assertEqual(actions.read_module("okkama", Path("std/encoding/hex.ok")), self.modules()["encoding/hex"])
            self.assertEqual(run.call_args.args[0], ["okkama", "ast", "std/encoding/hex.ok", "--format", "json", "--mode", "production", "--prune-tests"])
        for result, message in [(subprocess.CompletedProcess([], 1, "", "parse error"), "AST failed"), (subprocess.CompletedProcess([], 0, "oops", ""), "invalid AST")]:
            with patch.object(actions.subprocess, "run", return_value=result), self.assertRaisesRegex(ValueError, message):
                actions.read_module("okkama", Path("module.ok"))

    def test_discovery_excludes_runtime(self):
        with tempfile.TemporaryDirectory(dir=actions.HERE) as d:
            root = Path(d)
            (root / "runtime").mkdir()
            for name in ["z.ok", "a.ok", "runtime/native.ok"]:
                (root / name).write_text("")
            with patch.object(actions, "read_module", return_value={}) as read:
                self.assertEqual(list(actions.inventory("binary", root)), ["a", "z"])
                self.assertEqual(read.call_count, 2)
        with tempfile.TemporaryDirectory(dir=actions.HERE) as d:
            with self.assertRaisesRegex(ValueError, "no non-runtime"):
                actions.inventory("binary", Path(d))

    def test_scaffold_shape_and_no_overwrite(self):
        with tempfile.TemporaryDirectory(dir=actions.HERE) as d:
            root = Path(d)
            target = "encoding/hex:HexDecoded.bytes"
            source, oracle = actions.scaffold(target, self.modules(), root)
            text = source.read_text()
            for fragment in ['import "../support/harness" take Measurement;', 'import "encoding/hex" take HexDecoded;', "Bench()", "run(i is int, n is int, acc is int)", "if i == n", "measure.n()", "measure.warmup()", "let before = measure.start();", "measure.finish(before, check, warm);", "TODO_INPUT", target]:
                self.assertIn(fragment, text)
            self.assertLess(text.index("let warm ="), text.index("let before ="))
            namespace = {}
            exec(compile(oracle.read_text(), str(oracle), "exec"), namespace)
            with self.assertRaises(NotImplementedError):
                namespace["expected"](17)
            with self.assertRaisesRegex(ValueError, "already exists"):
                actions.scaffold(target, self.modules(), root)
            self.assertEqual(source.read_text(), text)
            with self.assertRaisesRegex(ValueError, "unknown public action"):
                actions.scaffold("../../escape:X.y", self.modules(), root)
            self.assertEqual(len(list(root.iterdir())), 2)



class ClassifyTests(unittest.TestCase):
    """#244 (owner, 2026-11-22): work actions need a benchmark; the rest is covered through callers."""

    def owner(self, *members, node="ObjectDecl"):
        return {"node": node, "name": "Thing", "members": list(members)}

    def action(self, name, statements):
        return {"node": "ActionDecl", "name": name, "params": [], "result": {"node": "NamedType", "path": ["int"], "args": []},
                "body": {"node": "Block", "statements": statements}}

    def call(self):
        return {"node": "ExpressionStatement", "expression": {"node": "Call", "callee": {"node": "NameExpression", "name": "f"}, "arguments": []}}

    def test_classes(self):
        field = {"node": "ExpressionStatement", "expression": {"node": "NameExpression", "name": "n"}}
        branch = {"node": "IfStatement", "condition": {"node": "BoolLiteral", "value": True}}
        signature = {"node": "ActionSignature", "name": "s", "params": [], "result": {"node": "NamedType", "path": ["int"], "args": []}}
        tree = {"items": [self.owner(
            self.action("field", [field]),
            self.action("delegate", [self.call()]),
            self.action("two", [self.call(), self.call()]),
            self.action("branch", [branch]),
            signature,
        ), {"node": "RoleDecl", "name": "Role", "members": [self.action("r", [self.call(), self.call()])]}]}
        classes = actions.classify_ast(tree)
        self.assertEqual(classes, {"Thing.field": "accessor", "Thing.delegate": "wrapper", "Thing.two": "work",
                                   "Thing.branch": "work", "Thing.s": "signature", "Role.r": "signature"})

    def test_an_overload_with_work_makes_the_target_work(self):
        tree = {"items": [self.owner(self.action("x", []), self.action("x", [self.call(), self.call()]))]}
        self.assertEqual(actions.classify_ast(tree), {"Thing.x": "work"})

    def test_todo_lists_unmeasured_work_only(self):
        modules = {"m": {"A.work": ["sig"], "A.done": ["sig"], "A.get": ["sig"]}}
        classes = {"m": {"A.work": "work", "A.done": "work", "A.get": "accessor"}}
        manifest = {"p": {"actions": ["m:A.done"], "targets": ["m"]}}
        self.assertEqual(actions.todo(modules, manifest, classes), ["m:A.work"])
        text = actions.report(modules, manifest, classes)
        self.assertIn("Work actions: 1 of 2 measured, 0 excluded", text)
        self.assertIn("| `m:A.get` | accessor | `sig` | via callers |", text)
        self.assertIn("| `m:A.work` | work | `sig` | — |", text)

    def test_excluded_targets_leave_the_todo_with_their_reason(self):
        modules = {"net/tcp": {"Tcp.connect": ["sig"]}, "db/postgres/wire": {"Wire.read": ["sig"]}, "m": {"A.x": ["sig"]}}
        classes = {"net/tcp": {"Tcp.connect": "work"}, "db/postgres/wire": {"Wire.read": "work"}, "m": {"A.x": "work"}}
        excluded = {"net/tcp": "opens sockets", "db/postgres/*": "needs a PostgreSQL server"}
        self.assertEqual(actions.todo(modules, {}, classes, excluded), ["m:A.x"])
        text = actions.report(modules, {}, classes, excluded)
        self.assertIn("Work actions: 0 of 3 measured, 2 excluded", text)
        self.assertIn("excluded: needs a PostgreSQL server", text)

if __name__ == "__main__":
    unittest.main()
