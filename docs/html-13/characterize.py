"""Run the new intended-behavior tests against the unchanged native projection."""
import argparse,subprocess,os,resource
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2]
p=argparse.ArgumentParser();p.add_argument('--okkama',required=True);p.add_argument('--scratch',type=Path,required=True);a=p.parse_args();a.scratch.mkdir(parents=True,exist_ok=True)
env=dict(os.environ,OKKAMA_STDLIB_DIR=str(ROOT/'std'))
def limits():resource.setrlimit(resource.RLIMIT_CPU,(45,45))
logs=[]
for name,marker in [('events','test "HTML attributes retain'),('entities','test "HTML numeric references')]:
 original=subprocess.check_output(['git','show','b6b10bdb7fc1ad299dcf71b349ed3ed2ced292f3:std/html/'+name+'.ok'],text=True,cwd=ROOT)
 current=(ROOT/'std/html'/(name+'.ok')).read_text()
 path=a.scratch/('characterization-'+name+'.ok');path.write_text(original+'\n'+current[current.index(marker):])
 r=subprocess.run([a.okkama,'run','-t',str(path)],env=env,text=True,capture_output=True,timeout=60,preexec_fn=limits)
 logs.append('## native '+name+'\n'+r.stdout+r.stderr)
 assert r.returncode==0 and '--- FAIL:' not in r.stdout,r.stdout+r.stderr
(ROOT/'docs/html-13/characterization.txt').write_text('\n'.join(logs))
print('All characterization tests pass against the original native projection.')
