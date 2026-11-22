"""Run this change's inline tests and every transitive std caller, bounded."""
import argparse,json,os,subprocess,resource
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2]
p=argparse.ArgumentParser();p.add_argument('--okkama',required=True);a=p.parse_args()
env=dict(os.environ,OKKAMA_STDLIB_DIR=str(ROOT/'std'))
def limits():resource.setrlimit(resource.RLIMIT_CPU,(45,45))
modules=json.loads((ROOT/'docs/html-13/callers.json').read_text())+['runtime/html']
logs=[];counts={}
for module in modules:
 r=subprocess.run([a.okkama,'run','-t',str(ROOT/'std'/(module+'.ok'))],cwd=ROOT,env=env,text=True,capture_output=True,timeout=60,preexec_fn=limits)
 log=r.stdout+r.stderr;logs.append('## '+module+'\n'+log);counts[module]={"passed":log.count("--- PASS:"),"failed":log.count("--- FAIL:"),"exit":r.returncode}
 (ROOT/'docs/html-13/tests.txt').write_text('\n'.join(logs))
 print(module,r.returncode,counts[module],flush=True)
 # Keep checking independent callers after an environmental refusal.
(ROOT/'docs/html-13/tests.json').write_text(json.dumps(counts,indent=2)+'\n')

raise SystemExit(1 if any(c["failed"] or c["exit"] for c in counts.values()) else 0)
