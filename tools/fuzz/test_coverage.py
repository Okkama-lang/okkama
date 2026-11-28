#!/usr/bin/env python3
"""Dry regression of coverage orchestration: only fake Cargo/Rust/LLVM tools run."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

HERE = Path(__file__).resolve().parent


class CoverageTests(unittest.TestCase):
    def run_fixture(self, mode, target='spb'):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            bin_ = root / 'bin'; bin_.mkdir()
            def executable(path, text):
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text('#!/bin/sh\n' + text)
                path.chmod(0o755)
            executable(bin_ / 'cargo', '''
case "$RUSTFLAGS" in *force-frame-pointers=yes*) ;; *) exit 9;; esac
printf '%s\\n' "$@" > args.txt
printf '%s\\n' "$ASAN_OPTIONS" > asan.txt
if [ "$MODE" = build-fail ]; then echo "synthetic coverage build failure" >&2; exit 1; fi
# Check the exact explicit directory is shared by build and report.
mkdir -p target/coverage/host/release coverage/spb coverage/exec
: > target/coverage/host/release/spb
: > target/coverage/host/release/exec
: > coverage/spb/coverage.profdata
: > coverage/exec/coverage.profdata
''')
            executable(bin_ / 'rustc', 'echo "$FIXTURE/toolchain"\n')
            executable(root / 'toolchain/lib/rustlib/host/bin/llvm-cov', '''
if [ "$MODE" = report-fail ]; then echo "synthetic llvm-cov failure" >&2; exit 1; fi
if [ "$MODE" = no-total ]; then echo "no coverage summary"; else echo "TOTAL 10 0 100%"; fi
''')
            env = {**os.environ, 'PATH': str(bin_) + ':' + os.environ['PATH'],
                   'MODE': mode, 'FIXTURE': str(root)}
            env.pop('CARGO_TARGET_DIR', None)
            env['ASAN_OPTIONS'] = 'allocator_may_return_null=1'
            (root / 'coverage').mkdir()
            report = root / 'coverage' / (target + '.txt')
            report.write_text('TOTAL stale\n')
            result = subprocess.run(['bash', str(HERE / 'coverage.sh'), 'host', target],
                                    cwd=root, env=env, capture_output=True, text=True)
            log = (root / 'logs' / (target + '-coverage.log')).read_text()
            self.asan = (root / 'asan.txt').read_text().strip()
            return result, log, report.read_text() if report.exists() else None, (root / 'args.txt').read_text()

    def test_spb_and_exec_report_coverage(self):
        for target in ['spb', 'exec']:
            result, log, report, args = self.run_fixture('success', target)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn('TOTAL 10', report)
            self.assertIn('--target-dir\ntarget/coverage\n', args)
            # #1050: the replay runs without ASan's ODR check, as `cargo fuzz run`
            # does, and keeps the caller's other options.
            self.assertEqual(self.asan, 'allocator_may_return_null=1:detect_odr_violation=0')

    def test_build_report_and_missing_total_are_loud_failures(self):
        for mode, message in [('build-fail', 'synthetic coverage build failure'),
                              ('report-fail', 'synthetic llvm-cov failure'),
                              ('no-total', 'no TOTAL line')]:
            result, log, report, _ = self.run_fixture(mode)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('COVERAGE FAILED: spb', result.stderr)
            self.assertIn(message, log)
            self.assertIn(message, result.stderr)
            self.assertIsNone(report, 'stale coverage must not survive a failed step')

    def test_driver_fails_after_coverage_failure_instead_of_no_findings(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            scripts = root / 'tools/fuzz'; scripts.mkdir(parents=True)
            for name in ['run.sh', 'coverage.sh']:
                path = scripts / name
                path.write_text((HERE / name).read_text()); path.chmod(0o755)
            wrapper = scripts / 'locked-cargo'; wrapper.mkdir()
            (wrapper / 'cargo').write_text((HERE / 'locked-cargo/cargo').read_text())
            (wrapper / 'cargo').chmod(0o755)
            for name in ['std', 'conformance/cases', 'okkama-rs/examples', 'okkama-rs/fuzz/seeds']:
                (root / name).mkdir(parents=True)
            bin_ = root / 'bin'; bin_.mkdir()
            cargo = bin_ / 'cargo'
            cargo.write_text('#!/bin/sh\ncase "$*" in *"fuzz coverage"*) echo "synthetic replay failure" >&2; exit 1;; esac\necho "Done fixture"\n')
            cargo.chmod(0o755)
            rustc = bin_ / 'rustc'
            rustc.write_text('#!/bin/sh\necho "host: fixture"\n'); rustc.chmod(0o755)
            env = {**os.environ, 'PATH': str(bin_) + ':' + os.environ['PATH']}
            result = subprocess.run(['bash', str(scripts / 'run.sh'), '1', '--coverage'],
                                    env=env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
            self.assertIn('COVERAGE FAILED: lex parse analyze ir_verify spb exec cbor ext', result.stderr)
            self.assertIn('synthetic replay failure', result.stderr)
            self.assertNotIn('no findings', result.stdout)
            for target in ['lex', 'parse', 'analyze', 'ir_verify', 'spb', 'exec', 'cbor', 'ext']:
                self.assertTrue((root / 'okkama-rs/fuzz/logs' / (target + '-coverage.log')).exists())


if __name__ == '__main__':
    unittest.main()
