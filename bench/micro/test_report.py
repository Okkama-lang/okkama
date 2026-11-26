"""Tiny campaign fixtures for release aggregation and standalone rendering."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import report


class ReportTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)

    def campaign(self, name, value, status='VALID', **overrides):
        result = dict(schema=2, status='COMPLETE', n=100, runs=3, suite_hash='suite',
                      protocol='micro-v2', environment={}, warmup=10, quality_policy={},
                      toolchain={'version': 'okkama 1.0'},
                      benchmarks={'x': {'sparrow': {'status': status, 'ns_per_iteration': value, 'spread': .01}}})
        result.update(overrides)
        path = self.root / (name + '.json')
        path.write_text(json.dumps(result))
        return path

    def cell(self, old, new):
        return report.cells(old, new)[0][0]['engines']['sparrow']

    def test_all_rounds_contribute_geometrically_and_order_is_irrelevant(self):
        old = [self.campaign('a1', 1), self.campaign('a2', 9)]
        new = [self.campaign('b1', 4), self.campaign('b2', 16)]
        cell = self.cell(old, new)
        self.assertAlmostEqual(cell['before'], 3)
        self.assertAlmostEqual(cell['after'], 8)
        self.assertAlmostEqual(cell['change'], 8 / 3 - 1)
        self.assertEqual(cell['roundsBefore'], 2)
        self.assertEqual(cell['verdict'], 'slower')
        self.assertEqual(cell, self.cell(old[::-1], new[::-1]))

    def test_any_nonvalid_or_missing_round_excludes_pair(self):
        good = self.campaign('good', 100)
        for status in ['INVALID', 'INCONCLUSIVE']:
            bad = self.campaign('bad', 100, status)
            self.assertEqual(self.cell([bad, good], [good])['verdict'], 'inconclusive')
            self.assertEqual(self.cell([good], [bad, good])['verdict'], 'inconclusive')
        missing = self.campaign('missing', 100, benchmarks={'y': {'sparrow': {'status': 'VALID', 'ns_per_iteration': 1, 'spread': 0}}})
        self.assertEqual(self.cell([good, missing], [good])['verdict'], 'inconclusive')
        empty = self.campaign('empty', 100, benchmarks={})
        self.assertEqual(self.cell([good, empty], [good])['verdict'], 'inconclusive')

    def test_earlier_round_incompatibility_cannot_be_overwritten(self):
        a = self.campaign('a', 1)
        b = self.campaign('b', 1, n=999)
        self.assertEqual(self.cell([b, a], [a])['verdict'], 'incompatible')

    def test_mixed_versions_cannot_be_labeled_as_one(self):
        a = self.campaign('a', 1)
        b = self.campaign('b', 1, toolchain={'version': 'okkama 2.0'})
        with self.assertRaises(ValueError):
            report.merge([a, b])

    def test_duplicate_file_rejected(self):
        a = self.campaign('a', 1)
        with self.assertRaises(ValueError):
            report.merge([a, a])

    def test_provenance_override_is_not_release_evidence(self):
        a = self.campaign('a', 1)
        b = self.campaign('b', 1, toolchain={'version_check': {'status': 'MISMATCH'}})
        self.assertEqual(self.cell([a], [b])['verdict'], 'incompatible')

    def test_layout_spread_is_carried_and_placements_must_match(self):
        cell = lambda value, layout: {'x': {'sparrow': {'status': 'VALID', 'ns_per_iteration': value, 'spread': .05, 'layout_spread': layout}}}
        old = [self.campaign('a1', 100, benchmarks=cell(100, .01), placements=[0, 64]),
               self.campaign('a2', 100, benchmarks=cell(100, .03), placements=[0, 64])]
        new = [self.campaign('b', 100, benchmarks=cell(101, .02), placements=[0, 64])]
        result = self.cell(old, new)
        self.assertEqual((result['layoutBefore'], result['layoutAfter']), (.03, .02))
        self.assertEqual(result['verdict'], 'noise')
        other = self.campaign('c', 100, benchmarks=cell(101, .02), placements=[0, 1024])
        self.assertEqual(self.cell(old, [other])['verdict'], 'incompatible')
        plain = self.cell([self.campaign('d', 100)], [self.campaign('e', 100)])
        self.assertEqual((plain['layoutBefore'], plain['layoutAfter']), (None, None))

    def test_template_is_in_repo_and_embedded_data_uses_all_rounds(self):
        old = [self.campaign('a1', 1), self.campaign('a2', 9)]
        new = [self.campaign('b', 6)]
        texts = self.root / 'texts.json'; texts.write_text('{}')
        output = self.root / 'report.html'
        subprocess.run([sys.executable, str(report.HERE / 'report.py'), '--old', *map(str, old),
                        '--new', *map(str, new), '-o', str(output), str(texts)], check=True)
        page = output.read_text()
        data = json.loads(page.split('const DATA = ', 1)[1].split(';\n', 1)[0])
        self.assertAlmostEqual(data['rows'][0]['engines']['sparrow']['before'], 3)
        self.assertIn('<style>', page)
        self.assertIn('геометрическим средним', page)


if __name__ == '__main__':
    unittest.main()
