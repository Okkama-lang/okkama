"""Compare unchanged native API projection with the candidate, no index writes."""
import argparse,subprocess,os,json,resource
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2]
BASE='b6b10bdb7fc1ad299dcf71b349ed3ed2ced292f3'
p=argparse.ArgumentParser();p.add_argument('--okkama',required=True);p.add_argument('--scratch',type=Path,required=True);a=p.parse_args();a.scratch.mkdir(parents=True,exist_ok=True)
env=dict(os.environ,OKKAMA_STDLIB_DIR=str(ROOT/'std'))
def limits():resource.setrlimit(resource.RLIMIT_CPU,(45,45))
for name in ['events','entities']:
 s=subprocess.check_output(['git','show',BASE+':std/html/'+name+'.ok'],text=True,cwd=ROOT)
 s=s.replace('module html/'+name,'module html13/reference_'+name)
 (a.scratch/('reference-'+name+'.ok')).write_text(s)
subprocess.run(['python3',str(ROOT/'docs/html-13/generate.py'),str(a.scratch/'corpus.json')],check=True)
source=(ROOT/'docs/html-13/differential.ok').read_text();observations=[]
for version in ['native','okkama']:
 s=source
 if version=='native':
  for name in ['events','entities']:s=s.replace('import "html/'+name+'"','import "./reference-'+name+'"')
 path=a.scratch/('differential-'+version+'.ok');path.write_text(s)
 # Bound the native registry and JSON reader by independent batches.
 corpus=json.loads((a.scratch/'corpus.json').read_text());lines=[]
 for start in range(0,len(corpus),250):
  batch=a.scratch/'batch.json';batch.write_text(json.dumps(corpus[start:start+250]))
  r=subprocess.run([a.okkama,'run',str(path),str(batch)],env=env,capture_output=True,text=True,timeout=60,preexec_fn=limits)
  if r.returncode or r.stderr:raise RuntimeError(r.stdout[-2000:]+r.stderr)
  lines.extend(r.stdout.rstrip("\n").split("\n"))
 (a.scratch/(version+'.out')).write_text('\n'.join(lines)+'\n');observations.append(lines);print(version,len(lines),flush=True)
diffs=[i for i,(x,y) in enumerate(zip(*observations)) if x!=y]
report={'inputs':len(corpus),'paths':2,'comparisons':len(corpus)*2,'differences':len(diffs),'indices':diffs[:50]}
(ROOT/'docs/html-13/differential.json').write_text(json.dumps(report,indent=2)+'\n');print(report)
assert all(len(x)==len(corpus) for x in observations) and not diffs
