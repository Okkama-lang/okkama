#!/usr/bin/env python3
"""Micro v2: bounded Docker execution, independent oracles, gross ns/iteration.

No host-execution fallback. The image must contain the chosen Okkama toolchain
and (for JVM) JDK. --toolchain mounts a self-contained Linux installation read-only
at /toolchain; use --okkama /toolchain/bin/okkama. Historical v1 results remain
readable archives but are deliberately incompatible with v2 comparisons.
"""
import argparse
import datetime
import hashlib
import json
import math
import os
from pathlib import Path
import random
import re
import selectors
import signal
import statistics
import subprocess
import sys
import tempfile
import time
import uuid
from oracles import expected

HERE=Path(__file__).resolve().parent
SCHEMA=2


def digest(path):
    h=hashlib.sha256()
    with open(path,'rb') as stream:
        for chunk in iter(lambda:stream.read(65536),b''):h.update(chunk)
    return h.hexdigest()


def bounded(cmd,timeout=120,limit=65536):
    """Bound host-side Docker CLI output; container limits bound the workload."""
    p=subprocess.Popen(cmd,stdout=subprocess.PIPE,stderr=subprocess.PIPE,start_new_session=True)
    selector=selectors.DefaultSelector();data={'out':bytearray(),'err':bytearray()}
    for stream,key in [(p.stdout,'out'),(p.stderr,'err')]:selector.register(stream,selectors.EVENT_READ,key)
    end=time.monotonic()+timeout
    try:
        while selector.get_map():
            if time.monotonic()>end:raise RuntimeError('timeout')
            for key,_ in selector.select(min(.1,max(0,end-time.monotonic()))):
                chunk=os.read(key.fileobj.fileno(),8192)
                if not chunk:selector.unregister(key.fileobj);continue
                data[key.data].extend(chunk)
                if len(data[key.data])>limit:raise RuntimeError('output limit exceeded')
        code=p.wait(timeout=max(.01,end-time.monotonic()))
        if code:raise RuntimeError(f'exit {code}: '+data['err'][-2000:].decode(errors='replace'))
        return data['out'].decode('utf-8'),data['err'].decode('utf-8')
    finally:
        # Kill remaining process-group members even if the direct process exited.
        try:os.killpg(p.pid,signal.SIGKILL)
        except ProcessLookupError:pass
        p.wait();selector.close();p.stdout.close();p.stderr.close()


class Docker:
    def __init__(self,args,work):self.args=args;self.work=work
    def run(self,command,timeout=None,env=None):
        a=self.args;name='okkama-micro-'+uuid.uuid4().hex
        cmd=['docker','run','--name',name,'--network','none','--read-only','--cap-drop','ALL','--security-opt','no-new-privileges','--cpus',str(a.cpus),'--memory',a.memory,'--memory-swap',a.memory,'--pids-limit','128','--ulimit','nofile=256:256','--ulimit','fsize=268435456:268435456','--ulimit',f'cpu={a.timeout}:{a.timeout}','--tmpfs','/tmp:rw,nosuid,size=256m','-v',f'{HERE}:/suite:ro','-v',f'{self.work}:/work','-w','/work','-e','OKKAMA_TRACE=off']
        if a.toolchain:cmd+=['-v',f'{Path(a.toolchain).resolve()}:/toolchain:ro']
        if a.stdlib:cmd+=['-v',f'{Path(a.stdlib).resolve()}:/stdlib:ro','-e','OKKAMA_STDLIB_DIR=/stdlib']
        for key,value in (env or {}).items():cmd+=['-e',f'{key}={value}']
        cmd+=[a.image,*command]
        try:return bounded(cmd,timeout or a.timeout,a.output_bytes)
        finally:
            # The CLI's process group is not the container's process tree.
            bounded(['docker','rm','-f',name],timeout=15,limit=8192)


def parse_sample(output,n,warm,name):
    fields=output.split()
    if len(fields)!=6 or fields[0]!='micro-v2':raise ValueError('invalid protocol')
    got_n,got_warm,elapsed,check,warm_check=map(int,fields[1:])
    if (got_n,got_warm)!=(n,warm):raise ValueError('iteration count mismatch')
    if elapsed<=0:raise ValueError('nonpositive elapsed time')
    if (check,warm_check)!=(expected(name,n),expected(name,warm)):raise ValueError('independent checksum mismatch')
    return {'elapsed_ns':elapsed,'checksum':check,'warm_checksum':warm_check,'ns_per_iteration':elapsed/n}


PLACEMENT_SWITCH='SPARROW_CORE_CODE_SHIFT'
MAX_PLACEMENT=65536


def placements(text):
    """--placements: distinct machine-code shifts in bytes (#933), e.g. 0,64,1024."""
    try:values=[int(part) for part in text.split(',')]
    except ValueError:raise argparse.ArgumentTypeError('comma-separated byte counts')
    if not values or len(set(values))!=len(values) or any(not 0<=v<=MAX_PLACEMENT for v in values):
        raise argparse.ArgumentTypeError(f'distinct byte counts from 0 to {MAX_PLACEMENT}')
    return values


def placement_env(placement):
    return {} if placement is None else {PLACEMENT_SWITCH:str(placement)}


def layout(samples):
    """Per-placement medians and their spread (max/min - 1); None without two placements."""
    groups={}
    for sample in samples:
        if sample.get('placement') is not None:groups.setdefault(sample['placement'],[]).append(sample['ns_per_iteration'])
    if len(groups)<2:return None
    medians={str(p):statistics.median(v) for p,v in sorted(groups.items())}
    return {'medians':medians,'spread':max(medians.values())/min(medians.values())-1}


def summarize(samples,runs,min_ns):
    if len(samples)!=runs:return {'status':'INVALID','reason':'incomplete sample set','samples':samples}
    values=[s['ns_per_iteration'] for s in samples]
    median=statistics.median(values);spread=max(values)/min(values)-1
    status='VALID' if min(s['elapsed_ns'] for s in samples)>=min_ns and spread<=.10 else 'INCONCLUSIVE'
    cell={'status':status,'samples':samples,'ns_per_iteration':median,'min':min(values),'max':max(values),'spread':spread,'mad':statistics.median(abs(v-median) for v in values),'reason':None if status=='VALID' else 'short batch or >10% full-range spread; repeat N/2N/4N and inspect GC/JIT separately'}
    # The full-range spread covers every placement, so the comparison band already includes layout (#933).
    shape=layout(samples)
    if shape:cell['layout']=shape['medians'];cell['layout_spread']=shape['spread']
    return cell


def compare(before,after,name,engine):
    if before.get('schema')!=SCHEMA:return {'status':'INCOMPATIBLE','reason':'legacy schema lacks comparable provenance'}
    for key in ['suite_hash','protocol','environment','n','warmup','quality_policy','placements']:
        if before.get(key)!=after.get(key):return {'status':'INCOMPATIBLE','reason':key+' differs'}
    if any(result.get('toolchain',{}).get('version_check',{}).get('status')=='MISMATCH' for result in (before,after)):
        return {'status':'INCOMPATIBLE','reason':'toolchain version does not match declared revision'}
    if any(result.get('toolchain',{}).get('devtrace') for result in (before,after)):
        return {'status':'INCOMPATIBLE','reason':'devtrace is compiled into a toolchain: not built as the packages are (--cfg okkama_release, #1025)'}
    a=before.get('benchmarks',{}).get(name,{}).get(engine,{})
    b=after['benchmarks'][name][engine]
    if a.get('status')!='VALID' or b.get('status')!='VALID':return {'status':'INCONCLUSIVE','reason':'invalid/noisy/missing cell'}
    delta=b['ns_per_iteration']/a['ns_per_iteration']-1
    threshold=max(.10,2*max(a['spread'],b['spread']))
    return {'status':'CHANGE' if abs(delta)>threshold else 'WITHIN_NOISE','relative_change':delta,'threshold':threshold,'metric':'gross ns/iteration; heuristic, not statistical significance'}


def revision_version(revision):
    """Read the declared source's version without checking out or executing it."""
    root = HERE.parent.parent
    commit = subprocess.run(['git', '-C', str(root), 'rev-parse', '--verify', '--end-of-options',
                             revision + '^{commit}'], capture_output=True, text=True)
    if commit.returncode:
        return None
    manifest = subprocess.run(['git', '-C', str(root), 'show',
                               commit.stdout.strip() + ':okkama-rs/Cargo.toml'],
                              capture_output=True, text=True)
    if manifest.returncode:
        return None
    # Only the workspace package table is authoritative, not dependency versions.
    section = re.search(r'(?ms)^\[workspace\.package\]\s*$(.*?)(?=^\[|\Z)', manifest.stdout)
    version = re.search(r"(?m)^version\s*=\s*['\"]([^'\"]+)['\"]", section[1]) if section else None
    return version[1] if version else None


# A binary built without --cfg okkama_release keeps devtrace (#1025): its gates cost the async and allocation
# helpers 2-5%, and only such a binary carries the name of the switch that sends its output.
DEVTRACE_MARKER='OKKAMA_DEV_OUT'


def devtrace_compiled_in(docker,okkama):
    return int(docker.run(['sh','-c',f'grep -c -a {DEVTRACE_MARKER} "$0" || true',okkama])[0].strip() or 0)>0


def version_check(revision, actual):
    expected = revision_version(revision)
    if expected is None:
        return {'status': 'UNVERIFIED', 'reason': 'revision manifest unavailable locally'}
    return {'status': 'MATCH' if actual == 'okkama ' + expected else 'MISMATCH',
            'expected': 'okkama ' + expected, 'actual': actual}


def positive(value):
    value=int(value)
    if value<=0:raise argparse.ArgumentTypeError('must be positive')
    return value


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--okkama',required=True,help='absolute executable path inside the container')
    p.add_argument('--image',required=True,help='local Docker image; resolved to immutable image ID')
    p.add_argument('--toolchain');p.add_argument('--stdlib')
    p.add_argument('--build-profile',help='declared build flags/profile, e.g. release fat-LTO; recorded, not inferred')
    p.add_argument('--revision',help='source revision of the candidate toolchain')
    p.add_argument('--allow-version-mismatch',action='store_true',help='retain a loudly flagged mismatched campaign; never release evidence')
    p.add_argument('--engines',default='sparrow');p.add_argument('--only',default='')
    p.add_argument('--runs',type=positive,default=7);p.add_argument('--n',type=positive,default=10000)
    p.add_argument('--warmup',type=positive,default=10000)
    p.add_argument('--timeout',type=positive,default=120);p.add_argument('--output-bytes',type=positive,default=65536)
    p.add_argument('--cpus',type=positive,default=1);p.add_argument('--memory',default='2g')
    p.add_argument('--min-ms',type=positive,default=10);p.add_argument('--seed',type=int,default=1729)
    p.add_argument('--label',default=datetime.datetime.now(datetime.timezone.utc).strftime('micro-%Y%m%dT%H%M%SZ'))
    p.add_argument('--placements',type=placements,help=f'machine-code shifts in bytes ({PLACEMENT_SWITCH}, #933), e.g. 0,64,1024: --runs is split evenly between them, each round runs every placement; reports the layout spread of each cell')
    p.add_argument('--require-valid',action='store_true',help='exit 2 if any completed cell remains inconclusive')
    p.add_argument('--compare');p.add_argument('--plan',action='store_true')
    p.add_argument('--results',default=str(HERE/'results'),help='directory of result files (the archive lives in okkama-docs/benchmarks/micro/results)')
    a=p.parse_args()
    if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_.-]*',a.label):p.error('label must be a plain filename stem')
    if a.toolchain and not a.stdlib:p.error('--toolchain requires explicit --stdlib for reproducible comparisons')
    if a.runs<3:p.error('at least three process samples required')
    if a.placements and (a.runs%len(a.placements) or a.runs<2*len(a.placements)):
        p.error('--runs must be a multiple of the placements, with at least two runs per placement')
    engines=[{'tomato':'sparrow'}.get(e,e) for e in a.engines.split(',')]
    if len(set(engines))!=len(engines) or not set(engines)<={'ir','sparrow','jvm'}:p.error('unknown or duplicate engine')
    manifest=json.loads((HERE/'manifest.json').read_text());selected=set(a.only.split(',')) if a.only else set(manifest)
    if not selected<=manifest.keys():p.error('unknown benchmark: '+','.join(sorted(selected-manifest.keys())))
    if not a.okkama.startswith('/'):p.error('--okkama must be absolute inside container')
    jobs=[(name,e) for name in sorted(selected) for e in engines]
    out=Path(a.results)/f'{a.label}.json'
    if out.exists():p.error('result already exists; use a new label (no overwrite)')
    if not a.plan and (not a.build_profile or not a.revision):p.error('execution requires --build-profile and --revision provenance')
    if a.plan:print(json.dumps({'jobs':jobs,'n':a.n,'warmup':a.warmup,'output':str(out)},indent=2));return
    old=json.loads(Path(a.compare).read_text()) if a.compare else None
    # Exclusive reservation prevents concurrent writers from reusing a release name.
    out.parent.mkdir(exist_ok=True)
    with out.open('x') as f:f.write('{"status":"INCOMPLETE","schema":2}\n')
    files=sorted([*HERE.glob('lang/*.ok'),*HERE.glob('std/*.ok'),*HERE.glob('support/*.ok'),HERE/'manifest.json',HERE/'oracles.py',HERE/'run.py'])
    hashes={str(f.relative_to(HERE)):digest(f) for f in files}
    suite_hash=hashlib.sha256(json.dumps(hashes,sort_keys=True).encode()).hexdigest()
    result={'schema':SCHEMA,'status':'INCOMPLETE','label':a.label,'protocol':'micro-v2 warmed batch; no baseline subtraction','suite_hash':suite_hash,'source_hashes':hashes,'n':a.n,'warmup':a.warmup,'runs':a.runs,'quality_policy':{'min_ms':a.min_ms,'max_spread':.10},'seed':a.seed,**({'placements':a.placements} if a.placements else {}),'order':[],'workloads':{name:manifest[name] for name in sorted(selected)},'benchmarks':{},'started_utc':datetime.datetime.now(datetime.timezone.utc).isoformat()}
    def save():
        tmp=out.with_suffix('.json.tmp');tmp.write_text(json.dumps(result,indent=2)+'\n');os.replace(tmp,out)
    try:
        image=json.loads(bounded(['docker','image','inspect',a.image],limit=262144)[0])[0]
        a.image=image['Id']
        result['environment']={'image':a.image,'architecture':image['Architecture'],'cpus':a.cpus,'memory':a.memory,'pids':128,'network':'none','normal_priority':True,'build_profile_declared':a.build_profile,'host':bounded(['docker','info','--format','{{.Name}}/{{.OSType}}/{{.Architecture}}/{{.KernelVersion}}'])[0].strip()}
        with tempfile.TemporaryDirectory(prefix='micro-v2-') as work:
            docker=Docker(a,work)
            result['toolchain']={'revision_declared':a.revision,'version':docker.run([a.okkama,'--version'])[0].strip(),'binary_sha256':docker.run(['sha256sum',a.okkama])[0].split()[0]}
            result['toolchain']['devtrace']=devtrace_compiled_in(docker,a.okkama)
            if result['toolchain']['devtrace']:
                print('WARNING: devtrace is compiled into the toolchain (no --cfg okkama_release): this campaign compares with no other',file=sys.stderr)
            provenance = version_check(a.revision, result['toolchain']['version'])
            result['toolchain']['version_check'] = provenance
            if provenance['status'] != 'MATCH':
                print('WARNING: toolchain provenance: ' + json.dumps(provenance), file=sys.stderr)
            if provenance['status'] == 'MISMATCH' and not a.allow_version_mismatch:
                raise ValueError('toolchain version does not match --revision; use --allow-version-mismatch only for explicitly unverified experiments')
            if a.stdlib:result['environment']['stdlib_hashes']={str(f.relative_to(a.stdlib)):digest(f) for f in sorted(Path(a.stdlib).rglob('*.ok'))}
            if a.toolchain:result['toolchain']['installation_hashes']={str(f.relative_to(a.toolchain)):digest(f) for f in sorted(Path(a.toolchain).rglob('*')) if f.is_file()}
            if 'jvm' in engines:result['toolchain']['java']=docker.run(['java','-version'])[1].strip()
            # Since 0.6.0 (Tweed) Sparrow is the only engine and `run` has no --engine; a toolchain from before
            # that still takes it (and before #584 defaulted to IR), so pass it only where the binary knows it.
            def legacy_flag():
                if any(e!='sparrow' for e in engines):return True
                try:return '--engine' in docker.run([a.okkama,'run','--help'])[0]
                except (RuntimeError,ValueError,OSError):return False
            legacy=legacy_flag()
            commands={}
            for name,e in jobs:
                source='/suite/'+manifest[name]['path']
                if e=='jvm':
                    artifact='/work/'+name.replace('-','_')+'.jar'
                    docker.run([a.okkama,'build',source,'--target','jvm','-o',artifact])
                    commands[name,e]=['java','--enable-native-access=ALL-UNNAMED','-jar',artifact]
                else:commands[name,e]=[a.okkama,'run',*(['--engine',e] if legacy else []),source]
            samples={key:[] for key in jobs};broken=set();rng=random.Random(a.seed)
            # Short independent-oracle checks exercise more than one iteration count.
            for name,e in jobs:
                for n in (17,31):
                    stdout,stderr=docker.run([*commands[name,e],str(n),'3'],env=placement_env(a.placements[0] if a.placements else None))
                    parse_sample(stdout,n,3,name)
                    if stderr:raise ValueError(f'{name}/{e}: unexpected stderr: {stderr[:1000]}')
            shifts=a.placements or [None]
            for round_ in range(a.runs//len(shifts)):
                # #933: a round runs every cell under every placement, shuffled together, so a burst of
                # host load falls on all placements alike instead of on the placement of one round.
                order=[(name,e,placement) for name,e in jobs for placement in shifts];rng.shuffle(order)
                result['order'].append([item if item[2] is not None else item[:2] for item in order])
                for name,e,placement in order:
                    key=(name,e)
                    if key in broken:continue
                    try:
                        stdout,stderr=docker.run([*commands[key],str(a.n),str(a.warmup)],env=placement_env(placement))
                        if stderr:raise ValueError('unexpected stderr: '+stderr[:1000])
                        sample=parse_sample(stdout,a.n,a.warmup,name)
                        if placement is not None:sample['placement']=placement
                        samples[key].append(sample)
                        cell=summarize(samples[key],a.runs,a.min_ms*1000000)
                    except (RuntimeError,ValueError,OSError) as error:
                        broken.add(key);cell={'status':'INVALID','reason':str(error),'samples':samples[key]}
                    result['benchmarks'].setdefault(name,{})[e]=cell
                    save()
                    print(round_+1,name,e,cell['status'],flush=True)
            if hashes!={str(f.relative_to(HERE)):digest(f) for f in files}:raise ValueError('suite changed during measurement')
            result['status']='INVALID' if broken else 'COMPLETE'
            if old:result['comparison']={name:{e:compare(old,result,name,e) for e in engines} for name in sorted(selected)}
    except (RuntimeError,ValueError,OSError,subprocess.SubprocessError) as error:
        result['status']='INVALID';result['error']=str(error)
    finally:save()
    print('benchmark / engine                 gross ns/iteration  status'+('  layout spread over placements '+','.join(map(str,a.placements)) if a.placements else ''))
    for name,row in sorted(result['benchmarks'].items()):
        for engine,cell in sorted(row.items()):
            value=cell.get('ns_per_iteration')
            shown=f'{value:.3f}' if value is not None else 'unavailable'
            shape=f'  layout ±{cell["layout_spread"]:.1%}' if 'layout_spread' in cell else ''
            print(f'{name}/{engine:8} {shown:>20}  {cell["status"]}{shape}')
            if cell.get('reason'):print('  '+cell['reason'])
            comparison=result.get('comparison',{}).get(name,{}).get(engine)
            if comparison:print('  comparison: '+json.dumps(comparison))
    print('results:',out)
    if result.get('error'):print('INVALID: '+result['error'],file=sys.stderr)
    if result['status']!='COMPLETE':raise SystemExit(1)
    if a.require_valid and any(cell['status']!='VALID' for row in result['benchmarks'].values() for cell in row.values()):raise SystemExit(2)


if __name__=='__main__':main()
