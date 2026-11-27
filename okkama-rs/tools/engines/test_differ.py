#!/usr/bin/env python3
"""differ.py's exceptions (#1123): the file parses, each selector explains only what it names, and a line without
a ticket or a design section is refused; `run` fails when nothing built (#1144). Dry: no toolchain."""
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import differ  # noqa: E402



class Exceptions(unittest.TestCase):
    def setUp(self):
        self.rules = differ.load()

    def test_the_repository_file_parses_and_names_its_tickets(self):
        kinds = {kind for kind, _, _ in self.rules}
        self.assertTrue({"both", "either", "jit"} <= kinds)
        # #1122: the interpreter runs everything; a failure of its own is not excused (#1144: no refusal selector).
        self.assertIsNone(differ.excepted(self.rules, None, (0, "ok"), (1, "RuntimeError: bad input (#1122)")))

    def test_the_refusal_selector_is_gone(self):
        # #1144: the interpreter refuses nothing since #1122, so a `refusal:` line is an unknown selector.
        with self.assertRaises(ValueError):
            self.load("refusal:(#1122)  async bodies (#1122)\n")

    def load(self, text):
        with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False) as out:
            out.write(text)
        try:
            return differ.load(out.name)
        finally:
            os.unlink(out.name)

    def test_a_stack_overflow_explains_only_when_both_overflow_with_one_status(self):
        overflow = "RuntimeError: Stack overflow\n  ... 1234 activations omitted"
        self.assertIsNotNone(differ.excepted(self.rules, None, (1, overflow), (1, overflow.replace("1234", "99"))))
        self.assertIsNone(differ.excepted(self.rules, None, (1, overflow), (0, "done")))
        self.assertIsNone(differ.excepted(self.rules, None, (1, overflow), (2, overflow)))

    def test_a_memory_budget_explains_on_either_side(self):
        memory = "Execution budget exhausted: memory"
        self.assertIsNotNone(differ.excepted(self.rules, None, (1, "Execution budget exhausted: steps"), (1, memory)))
        self.assertIsNotNone(differ.excepted(self.rules, None, (1, memory), (0, "done")))
        self.assertIsNone(differ.excepted(self.rules, None, (1, "Execution budget exhausted: steps"), (0, "done")))

    def test_the_jit_compile_limit_explains_the_jit_side_only(self):
        limit = "main: compiling it would take more than 256 MiB"
        self.assertIsNotNone(differ.excepted(self.rules, None, (1, limit), (0, "result")))
        self.assertIsNone(differ.excepted(self.rules, None, (0, "result"), (1, limit)))

    def test_without_the_jit_run_only_a_path_explains(self):
        overflow = "RuntimeError: Stack overflow"
        memory = "Execution budget exhausted: memory"
        self.assertIsNone(differ.excepted(self.rules, None, None, (1, overflow)))
        self.assertIsNone(differ.excepted(self.rules, None, None, (1, memory)))
        rules = self.load("path:t/slow/*.ok  a slow file (#1)\n")
        self.assertIsNotNone(differ.excepted(rules, os.path.join(differ.ROOT, "t", "slow", "a.ok"), None, (1, overflow)))

    def test_a_line_without_a_reason_or_a_ticket_is_refused(self):
        for line in ["both:(#1)", "jit:x  no ticket here", "weird:x  (#1)", "both:  (#1)"]:
            with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False) as out:
                out.write(line + "\n")
            try:
                with self.assertRaises(ValueError, msg=line):
                    differ.load(out.name)
            finally:
                os.unlink(out.name)

    def test_a_path_glob_is_relative_to_the_repository(self):
        with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False) as out:
            out.write("path:examples/slow/*.ok  a slow program (#1)\n")
        try:
            rules = differ.load(out.name)
        finally:
            os.unlink(out.name)
        inside = os.path.join(differ.ROOT, "examples", "slow", "a.ok")
        self.assertIsNotNone(differ.excepted(rules, inside, (0, "a"), (0, "b")))
        self.assertIsNone(differ.excepted(rules, os.path.join(differ.ROOT, "examples", "a.ok"), (0, "a"), (0, "b")))


class Status(unittest.TestCase):
    """`run`'s exit status (#1144): a broken build that compares nothing must not pass."""

    def test_nothing_built_fails(self):
        self.assertEqual(differ.status({"unbuilt": 3}), 1)
        self.assertEqual(differ.status({}), 1)

    def test_a_difference_fails_and_compared_programs_pass(self):
        self.assertEqual(differ.status({"same": 2, "DIFFERS": 1}), 1)
        self.assertEqual(differ.status({"same": 2, "unbuilt": 1}), 0)
        self.assertEqual(differ.status({"excepted": 1}), 0)


if __name__ == "__main__":
    unittest.main()
