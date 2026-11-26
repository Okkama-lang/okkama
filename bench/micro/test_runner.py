import importlib.util
import pathlib
import sys
import unittest
from unittest.mock import patch
import run
from oracles import expected

class RunnerTests(unittest.TestCase):
    def sample(self,value=100000000):return {'elapsed_ns':value,'checksum':0,'warm_checksum':0,'ns_per_iteration':value/10000}
    def test_oracle_not_cross_engine_agreement(self):
        with self.assertRaises(ValueError):run.parse_sample('micro-v2 17 3 100 999 999',17,3,'baseline')
    def test_protocol(self):
        c=expected('baseline',17);w=expected('baseline',3)
        self.assertEqual(run.parse_sample(f'micro-v2 17 3 100 {c} {w}',17,3,'baseline')['checksum'],c)
        for text in ['', '100 0',f'micro-v2 18 3 100 {c} {w}',f'micro-v2 17 3 0 {c} {w}',f'micro-v2 17 3 -1 {c} {w}']:
            with self.assertRaises(ValueError):run.parse_sample(text,17,3,'baseline')
    def test_short_noisy_and_partial(self):
        self.assertEqual(run.summarize([self.sample()]*2,3,1000)['status'],'INVALID')
        self.assertEqual(run.summarize([self.sample(10)]*3,3,1000)['status'],'INCONCLUSIVE')
        self.assertEqual(run.summarize([self.sample(100),self.sample(140),self.sample(100)],3,1)['status'],'INCONCLUSIVE')
        self.assertEqual(run.summarize([self.sample()]*3,3,1000)['status'],'VALID')
    def test_failure_cannot_be_compared(self):
        a={'schema':2,'benchmarks':{'x':{'ir':{'status':'INVALID'}}}}
        self.assertEqual(run.compare(a,a,'x','ir')['status'],'INCONCLUSIVE')
    def test_legacy_incompatible(self):self.assertEqual(run.compare({}, {},'x','ir')['status'],'INCOMPATIBLE')
    def test_provenance_mismatch(self):
        for field in ['suite_hash','protocol','environment','n','warmup']:
            self.assertEqual(run.compare({'schema':2,field:1},{field:2},'x','ir')['status'],'INCOMPATIBLE')
    def test_gross_change(self):
        a={'schema':2,'benchmarks':{'x':{'ir':{'status':'VALID','ns_per_iteration':100,'spread':.01}}}}
        b={'benchmarks':{'x':{'ir':{'status':'VALID','ns_per_iteration':120,'spread':.01}}}}
        self.assertEqual(run.compare(a,b,'x','ir')['status'],'CHANGE')
    def test_manifest_oracles(self):
        import json
        for name in json.loads((run.HERE/'manifest.json').read_text()):self.assertIsInstance(expected(name,17),int)
    def test_no_warmup_data_silently_ignored(self):
        with self.assertRaises(ValueError):run.parse_sample(f'micro-v2 17 3 100 {expected("baseline",17)} 999',17,3,'baseline')
    def test_bounded_output(self):
        with self.assertRaises(RuntimeError):run.bounded([sys.executable,'-c','print("x"*1000)'],limit=20)
    def test_bounded_timeout(self):
        with self.assertRaises(RuntimeError):run.bounded([sys.executable,'-c','import time;time.sleep(10)'],timeout=.05)
    def test_exit(self):
        with self.assertRaises(RuntimeError):run.bounded([sys.executable,'-c','raise SystemExit(7)'])
    def test_success(self):self.assertEqual(run.bounded([sys.executable,'-c','print("ok")'])[0],'ok\n')

    def test_late_failure_stays_invalid(self):
        import tempfile,json
        with tempfile.TemporaryDirectory() as directory:
            root=pathlib.Path(directory)
            (root/'lang').mkdir();(root/'lang/baseline.ok').write_text('fixture')
            (root/'manifest.json').write_text(json.dumps({'baseline':{'path':'lang/baseline.ok'}}))
            (root/'run.py').write_text('fixture');(root/'oracles.py').write_text('fixture')
            class FakeDocker:
                def __init__(self,*args):self.timed=0
                def run(self,command,env=None):
                    if '--version' in command:return 'okkama test',''
                    if command[0]=='sha256sum':return 'abc binary',''
                    if command[0]=='sh':return '0\n',''
                    n,w=map(int,command[-2:])
                    if n==101:
                        self.timed+=1
                        if self.timed==2:raise RuntimeError('synthetic later failure')
                    return f'micro-v2 {n} {w} 100000000 {expected("baseline",n)} {expected("baseline",w)}',''
            def probe(command,**kwargs):
                return ('[{"Id":"image-id","Architecture":"arm64"}]' if command[1]=='image' else 'host','')
            argv=['run.py','--okkama','/binary','--image','fixture','--engines','ir','--n','101','--warmup','3','--runs','3','--label','late','--build-profile','fixture','--revision','fixture']
            with patch.object(run,'HERE',root),patch.object(run,'Docker',FakeDocker),patch.object(run,'bounded',probe),patch.object(sys,'argv',argv):
                with self.assertRaises(SystemExit) as failed:run.main()
                self.assertEqual(failed.exception.code,1)
            result=json.loads((root/'results/late.json').read_text())
            self.assertEqual(result['status'],'INVALID')
            cell=result['benchmarks']['baseline']['ir']
            self.assertEqual(cell['status'],'INVALID');self.assertEqual(len(cell['samples']),1)
            self.assertIn('later failure',cell['reason'])


class PlacementTests(unittest.TestCase):
    """#933: each cell under several machine-code placements, and its layout spread."""
    def sample(self,value,placement=None):
        s={'elapsed_ns':value*10000,'checksum':0,'warm_checksum':0,'ns_per_iteration':value}
        if placement is not None:s['placement']=placement
        return s
    def test_placement_list(self):
        self.assertEqual(run.placements('0,64,1024'),[0,64,1024])
        for bad in ['', '0,0', '-64', '65537', '64k', '0,,64']:
            with self.assertRaises(run.argparse.ArgumentTypeError):run.placements(bad)
    def test_layout_spread_is_over_placement_medians(self):
        samples=[self.sample(100,0),self.sample(102,64),self.sample(100,0),self.sample(110,64)]
        cell=run.summarize(samples,4,1)
        self.assertEqual(cell['layout'],{'0':100,'64':106})
        self.assertAlmostEqual(cell['layout_spread'],.06)
        # The band still uses the full-range spread over every placement.
        self.assertAlmostEqual(cell['spread'],.10)
        self.assertNotIn('layout',run.summarize([self.sample(100)]*3,3,1))
        self.assertNotIn('layout',run.summarize([self.sample(100,0)]*3,3,1))
    def test_different_placements_are_incompatible(self):
        a={'schema':2,'placements':[0,64],'benchmarks':{'x':{'ir':{'status':'VALID','ns_per_iteration':100,'spread':.01}}}}
        b={'placements':[0,1024],'benchmarks':{'x':{'ir':{'status':'VALID','ns_per_iteration':100,'spread':.01}}}}
        self.assertEqual(run.compare(a,b,'x','ir')['status'],'INCOMPATIBLE')
        self.assertEqual(run.compare(a,dict(b,placements=[0,64]),'x','ir')['status'],'WITHIN_NOISE')
    def test_a_toolchain_with_devtrace_compares_with_nothing(self):
        cell={'x':{'ir':{'status':'VALID','ns_per_iteration':100,'spread':.01}}}
        a={'schema':2,'toolchain':{'devtrace':False},'benchmarks':cell}
        self.assertEqual(run.compare(a,dict(a,toolchain={'devtrace':True}),'x','ir')['status'],'INCOMPATIBLE')
        self.assertEqual(run.compare(dict(a,toolchain={'devtrace':True}),a,'x','ir')['status'],'INCOMPATIBLE')
        self.assertEqual(run.compare(a,a,'x','ir')['status'],'WITHIN_NOISE')
    def test_devtrace_is_found_by_the_switch_name_in_the_binary(self):
        class Fake:
            def __init__(self,count):self.count=count;self.commands=[]
            def run(self,command):self.commands.append(command);return (f'{self.count}\n','')
        self.assertTrue(run.devtrace_compiled_in(Fake(1),'/toolchain/bin/okkama'))
        self.assertFalse(run.devtrace_compiled_in(Fake(0),'/toolchain/bin/okkama'))
        fake=Fake(0);run.devtrace_compiled_in(fake,'/toolchain/bin/okkama')
        self.assertIn('OKKAMA_DEV_OUT',fake.commands[0][2]);self.assertEqual(fake.commands[0][-1],'/toolchain/bin/okkama')
    def test_every_round_runs_every_placement(self):
        import tempfile,json
        with tempfile.TemporaryDirectory() as directory:
            root=pathlib.Path(directory)
            (root/'lang').mkdir();(root/'lang/baseline.ok').write_text('fixture')
            (root/'manifest.json').write_text(json.dumps({'baseline':{'path':'lang/baseline.ok'}}))
            (root/'run.py').write_text('fixture');(root/'oracles.py').write_text('fixture')
            seen=[]
            class FakeDocker:
                def __init__(self,*args):pass
                def run(self,command,env=None):
                    if '--version' in command:return 'okkama test',''
                    if command[0]=='sha256sum':return 'abc binary',''
                    if command[0]=='sh':return '0\n',''
                    n,w=map(int,command[-2:])
                    shift=int((env or {}).get(run.PLACEMENT_SWITCH,-1))
                    if n==101:seen.append(shift)
                    return f'micro-v2 {n} {w} {100000000+shift*1000} {expected("baseline",n)} {expected("baseline",w)}',''
            def probe(command,**kwargs):
                return ('[{"Id":"image-id","Architecture":"arm64"}]' if command[1]=='image' else 'host','')
            argv=['run.py','--okkama','/binary','--image','fixture','--engines','ir','--n','101','--warmup','3','--runs','4','--placements','0,64','--label','turns','--build-profile','fixture','--revision','fixture']
            with patch.object(run,'HERE',root),patch.object(run,'Docker',FakeDocker),patch.object(run,'bounded',probe),patch.object(sys,'argv',argv):
                run.main()
            # Each round runs every placement once.
            self.assertEqual([sorted(seen[:2]),sorted(seen[2:])],[[0,64],[0,64]])
            result=json.loads((root/'results/turns.json').read_text())
            self.assertEqual(result['placements'],[0,64])
            self.assertEqual([sorted(item[2] for item in order) for order in result['order']],[[0,64],[0,64]])
            cell=result['benchmarks']['baseline']['ir']
            self.assertEqual([s['placement'] for s in cell['samples']],seen)
            self.assertEqual(set(cell['layout']),{'0','64'})
            self.assertGreater(cell['layout_spread'],0)
    def test_runs_split_evenly_between_placements(self):
        for runs in ['3','5']:
            argv=['run.py','--okkama','/binary','--image','fixture','--runs',runs,'--placements','0,64','--plan']
            with patch.object(sys,'argv',argv),self.assertRaises(SystemExit) as refused:run.main()
            self.assertEqual(refused.exception.code,2)


class VersionTests(unittest.TestCase):
    def test_binary_version_is_checked_against_revision_not_label(self):
        with patch.object(run, 'revision_version', return_value='0.6.3'):
            self.assertEqual(run.version_check('commit', 'okkama 0.6.3')['status'], 'MATCH')
            result = run.version_check('commit', 'okkama 0.6.2')
            self.assertEqual(result['status'], 'MISMATCH')
            self.assertEqual(result['expected'], 'okkama 0.6.3')
            self.assertEqual(result['actual'], 'okkama 0.6.2')
        with patch.object(run, 'revision_version', return_value=None):
            self.assertEqual(run.version_check('missing', 'okkama 0.6.3')['status'], 'UNVERIFIED')

    def test_override_cannot_be_compared_as_release_evidence(self):
        result = {'schema': 2, 'toolchain': {'version_check': {'status': 'MISMATCH'}}}
        self.assertEqual(run.compare(result, result, 'x', 'sparrow')['status'], 'INCOMPATIBLE')

    def test_local_manifest_version_is_used(self):
        self.assertEqual(run.revision_version('7d2b3403e'), '0.6.3')
        self.assertIsNone(run.revision_version('not-a-real-revision-1052'))

    def test_mismatch_stops_campaign_before_workloads_and_is_saved(self):
        import tempfile, json
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            (root / 'manifest.json').write_text(json.dumps({'baseline': {'path': 'lang/baseline.ok'}}))
            for name in ['run.py', 'oracles.py']:
                (root / name).write_text('fixture')
            class FakeDocker:
                def __init__(self, *args): pass
                def run(self, command):
                    if '--version' in command: return 'okkama 0.6.2', ''
                    if command[0] == 'sha256sum': return 'hash binary', ''
                    if command[0] == 'sh': return '0\n', ''
                    raise AssertionError('workload must not run')
            def probe(command, **kwargs):
                return ('[{"Id":"image-id","Architecture":"arm64"}]' if command[1] == 'image' else 'host', '')
            argv = ['run.py', '--okkama', '/binary', '--image', 'fixture', '--build-profile', 'release',
                    '--revision', '7d2b3403e', '--label', 'mismatch']
            with patch.object(run, 'HERE', root), patch.object(run, 'Docker', FakeDocker), \
                 patch.object(run, 'bounded', probe), patch.object(run, 'revision_version', return_value='0.6.3'), \
                 patch.object(sys, 'argv', argv):
                with self.assertRaises(SystemExit) as failed: run.main()
                self.assertEqual(failed.exception.code, 1)
            result = json.loads((root / 'results/mismatch.json').read_text())
            self.assertEqual(result['status'], 'INVALID')
            self.assertEqual(result['benchmarks'], {})
            self.assertEqual(result['toolchain']['version'], 'okkama 0.6.2')
            self.assertEqual(result['toolchain']['version_check']['status'], 'MISMATCH')

if __name__=='__main__':unittest.main()
