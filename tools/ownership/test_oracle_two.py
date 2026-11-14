"""Two incompatible fake toolchains; never compile or execute Okkama."""
import contextlib
import copy
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest

import oracle
import oracle_two


FAKE = '''import os
from pathlib import Path
import sys
kind, source, binary, config_path = sys.argv[1:]
config = __import__('json').loads(Path(config_path).read_text())
if kind == 'build':
    if config.get('build_fail'):
        sys.exit(1)
    Path(binary).write_bytes(b'SPRW' + config.get('version', 16).to_bytes(2, 'big') + b'fake')
    sys.exit(0)
if config.get('timeout'):
    import time
    time.sleep(2)
trace = config['trace']
if config.get('nondeterministic'):
    state = Path(config_path + '.' + kind + '.state')
    count = int(state.read_text()) if state.exists() else 0
    state.write_text(str(count + 1))
    if count % 2:
        trace += '# test unexpected\\n'
if config.get('source_diff') and kind == 'run':
    trace = ''.join(reversed(trace.splitlines(keepends=True)))
Path(os.environ['SPARROW_TRACE_DESTROY']).write_text(trace)
sys.stdout.write(config.get('stdout', 'checkpoint\\n'))
sys.stderr.write(config.get('stderr', ''))
sys.exit(config.get('code', 0))
'''


class TwoToolchainTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / 'source.ok').write_text('not executable Okkama')
        (self.root / 'model.oir').write_text('independent model reference')
        self.fake = self.root / 'fake.py'
        self.fake.write_text(FAKE)
        self.manifest = self.root / 'manifest.json'
        self.entry = dict(id='nested', source='source.ok', checks=[1], rule='Book 20.2',
                          features=['object-destroy'], checkpoints=['checkpoint'],
                          stdout='checkpoint\n', failure=dict(code=0, stderr=''),
                          destroy=['create-A', 'create-B'], baseline=dict(revision='frozen', toolchain='A'),
                          model_fixture='model.oir', cleanup='static', expected='supported',
                          identities=[dict(event='create-A', A=dict(block=0, serial=4, type='Res'),
                                           B=dict(block=0, serial=1, type='Res')),
                                      dict(event='create-B', A=dict(block=0, serial=7, type='Res'),
                                           B=dict(block=0, serial=2, type='Res'))])
        self.configs = {'A': dict(trace='Res 4\nRes 7\n', version=16),
                        'B': dict(trace='Res 1\nRes 2\n', version=17)}
        self.argv = ['two-toolchains', '--root', str(self.root), '--manifest', str(self.manifest),
                     '--allow-host', '--out', str(self.root / 'results'), '--json', str(self.root / 'report.json')]
        for side in ('A', 'B'):
            prefix = self.root / side
            (prefix / 'lib/okkama/std').mkdir(parents=True)
            (prefix / 'lib/okkama/std/plain.ok').write_text('stdlib ' + side)
            name = side.lower()
            self.argv += [f'--{name}-toolchain', str(prefix), f'--{name}-revision', side + '-frozen-revision',
                          f'--{name}-spb-version', str(self.configs[side]['version'])]
            for kind in ('build', 'run', 'exec'):
                command = [sys.executable, str(self.fake), kind, '{source}', '{binary}', str(self.root / (side + '.json'))]
                self.argv += [f'--{name}-{kind}-command', json.dumps(command)]

    def run_suite(self, extra=()):
        for side, config in self.configs.items():
            (self.root / (side + '.json')).write_text(json.dumps(config))
        self.manifest.write_text(json.dumps(dict(schema='okkama-p1-manifest/1', entries=[self.entry])))
        args = oracle.parser().parse_args(self.argv + list(extra))
        with contextlib.redirect_stdout(io.StringIO()):
            code = oracle_two.run_suite(args)
        return code, json.loads((self.root / 'report.json').read_text())

    def test_mapped_equality_and_report_evidence(self):
        code, report = self.run_suite()
        self.assertEqual(code, 0)
        self.assertEqual(report['schema'], 3)
        self.assertTrue(report['gate']['passed'])
        case = report['programs'][0]
        self.assertEqual(case['status'], 'same')
        self.assertEqual(len(case['runs']), 8)
        self.assertEqual(len(case['builds']), 4)
        self.assertEqual(case['binaries']['B2']['spb_version'], 17)
        self.assertEqual(report['toolchains']['A']['revision'], 'A-frozen-revision')
        self.assertTrue(report['toolchains']['A']['stdlib']['sha256'])
        self.assertTrue(case['inputs']['source'])
        self.assertNotEqual(Path(case['runs']['A1.run']['trace']).read_bytes(),
                            Path(case['runs']['B1.run']['trace']).read_bytes())

    def test_identical_numbering(self):
        self.configs['B']['trace'] = self.configs['A']['trace']
        for identity in self.entry['identities']:
            identity['B'] = copy.deepcopy(identity['A'])
        self.assertEqual(self.run_suite()[0], 0)

    def test_aa_nondeterminism(self):
        self.configs['A']['nondeterministic'] = True
        code, report = self.run_suite()
        self.assertEqual(code, 1)
        self.assertTrue(report['programs'][0]['determinism_diff'])

    def test_bb_nondeterminism(self):
        self.configs['B']['nondeterministic'] = True
        self.assertEqual(self.run_suite()[0], 1)

    def test_real_order_difference(self):
        self.configs['B']['trace'] = 'Res 2\nRes 1\n'
        code, report = self.run_suite()
        self.assertEqual(code, 1)
        self.assertIn('cross-toolchain mapped trace differs', report['programs'][0]['differences'])

    def test_missing_duplicate_and_unmapped_trace_identities(self):
        for trace, problem in [('Res 4\n', 'missing identities'),
                               ('Res 4\nRes 4\nRes 7\n', 'duplicate identity'),
                               ('Res 4\nRes 7\nRes 8\n', 'unmapped identity')]:
            with self.subTest(trace=trace):
                self.configs['A']['trace'] = trace
                code, report = self.run_suite()
                self.assertEqual(code, 1)
                self.assertTrue(any(problem in p for p in report['programs'][0]['problems']))

    def test_invalid_manifest_mapping(self):
        self.entry['identities'][1]['A']['serial'] = 4
        with self.assertRaisesRegex(ValueError, 'duplicate A identity'):
            self.run_suite()

    def test_missing_creation_event(self):
        del self.entry['identities'][1]
        with self.assertRaisesRegex(ValueError, 'every creation-event'):
            self.run_suite()

    def test_unexpected_build_failure_gate(self):
        self.configs['B']['build_fail'] = True
        code, report = self.run_suite()
        self.assertEqual(code, 1)
        self.assertEqual(report['gate']['failed'], ['nested'])
        self.assertEqual(len(report['programs'][0]['builds']), 4)

    def test_unsupported_explicitly_counted_without_execution(self):
        self.entry.update(expected='unsupported', reason='awaiting P1 lowering')
        self.configs['B']['build_fail'] = True
        code, report = self.run_suite()
        self.assertEqual(code, 0)
        self.assertEqual(report['gate']['unsupported'], ['nested'])
        self.assertEqual(report['gate']['executed'], 0)
        self.assertEqual(report['programs'][0]['builds'], {})

    def test_unsupported_requires_reason(self):
        self.entry['expected'] = 'unsupported'
        with self.assertRaisesRegex(ValueError, 'needs reason'):
            self.run_suite()

    def test_unknown_and_empty_selection(self):
        with self.assertRaisesRegex(ValueError, 'unknown manifest'):
            self.run_suite(['--filter', 'typo'])
        self.manifest.write_text('{"schema":"okkama-p1-manifest/1","entries":[]}')
        with self.assertRaisesRegex(ValueError, 'empty manifest'):
            oracle_two.load_manifest(self.manifest, self.root, [])

    def test_timeout_gate(self):
        self.configs['A']['timeout'] = True
        code, report = self.run_suite(['--timeout', '0.05'])
        self.assertEqual(code, 1)
        self.assertTrue(any(r['timeout'] for r in report['programs'][0]['runs'].values()))

    def test_unexpected_tap_skip_gate(self):
        self.entry['stdout'] = 'ok 1 # SKIP unavailable\n1..1\n'
        for config in self.configs.values():
            config['stdout'] = self.entry['stdout']
        code, report = self.run_suite()
        self.assertEqual(code, 1)
        self.assertTrue(any('unexpected skip' in p for p in report['programs'][0]['problems']))

    def test_source_vs_binary_parity(self):
        self.configs['A']['source_diff'] = True
        code, report = self.run_suite()
        self.assertEqual(code, 1)
        self.assertTrue(report['programs'][0]['differences'])

    def test_spb_version_gate(self):
        self.configs['B']['version'] = 16
        code, report = self.run_suite()
        self.assertEqual(code, 1)
        self.assertTrue(any('SPB magic/version' in p for p in report['programs'][0]['problems']))

    def test_failure_kind_primary_secondaries_are_exact(self):
        self.entry['failure'] = dict(code=1, stderr='Body: failed\nSecondary: second\nSecondary: first\n')
        for config in self.configs.values():
            config.update(self.entry['failure'])
        self.assertEqual(self.run_suite()[0], 0)
        self.configs['B']['stderr'] = 'Panic: failed\nSecondary: first\nSecondary: second\n'
        self.assertEqual(self.run_suite()[0], 1)

    def test_test_blocks_preserve_order_and_allow_serial_restart(self):
        mapping = {(1, 1): ('first', 'Res'), (2, 1): ('second', 'Res')}
        path = self.root / 'blocks.trace'
        path.write_bytes(b'# test repeated\nRes 1\n# test repeated\nRes 1\n')
        events, destroyed = oracle_two.mapped_trace(path, mapping)
        self.assertEqual(destroyed, ['first', 'second'])
        self.assertEqual(len(events), 4)

    def test_shipped_manifest_is_valid_and_honest(self):
        root = Path(oracle.__file__).resolve().parents[2]
        entries = oracle_two.load_manifest(Path(oracle.__file__).with_name('p1-manifest.json'), root, [])
        self.assertTrue(entries)
        self.assertTrue(all(e['expected'] == 'unsupported' for e in entries))

    def test_legacy_cli_and_required_toolchain(self):
        for mode in ('engines', 'ab', 'stats', 'tap'):
            args = oracle.parser().parse_args([mode, '--toolchain', '/tc'])
            self.assertEqual(args.toolchain, Path('/tc'))
        from unittest.mock import patch
        with patch.object(sys, 'argv', ['oracle.py', 'ab', '--allow-host']), contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit) as exc:
                oracle.main()
            self.assertEqual(exc.exception.code, 2)

    def test_legacy_modes_keep_schema_two_and_success_policy(self):
        from unittest.mock import patch
        prefix = self.root / 'A'
        (prefix / 'bin').mkdir()
        script = '''import json
import os
from pathlib import Path
import sys
if len(sys.argv) > 1 and sys.argv[1] == 'build':
    Path(sys.argv[sys.argv.index('-o') + 1]).write_bytes(b'fake binary')
    if os.environ.get('OKKAMA_OWNERSHIP_REPORT'):
        Path(os.environ['OKKAMA_OWNERSHIP_REPORT']).write_text(json.dumps(
            {'totals': {key: 0 for key in ('scope', 'result', 'aggregate', 'unknown')}}))
    sys.exit(0)
Path(os.environ['SPARROW_TRACE_DESTROY']).write_text('Res 1\\n')
print('checkpoint')
if os.environ.get('SPARROW_CORE_STATS'):
    print('0 heap bytes allocated', file=sys.stderr)
'''
        for name in ('okkama', 'sparrow'):
            executable = prefix / 'bin' / name
            executable.write_text('#!' + sys.executable + '\n' + script)
            executable.chmod(0o755)
        for mode in ('engines', 'ab', 'stats', 'tap'):
            with self.subTest(mode=mode):
                target = self.root / (mode + '.json')
                argv = ['oracle.py', mode, '--toolchain', str(prefix), '--allow-host',
                        '--root', str(self.root), '--corpus', 'bench',
                        '--bench', str(self.root / 'source.ok'), '--out', str(self.root / 'legacy'),
                        '--json', str(target)]
                with patch.object(sys, 'argv', argv), contextlib.redirect_stdout(io.StringIO()):
                    self.assertEqual(oracle.main(), 0)
                report = json.loads(target.read_text())
                self.assertEqual(report['schema'], 2)
                self.assertEqual(report['programs'][0]['status'], 'same')


if __name__ == '__main__':
    unittest.main()
