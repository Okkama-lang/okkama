"""Bounded sequential micro-v2 runs; generated programs live outside the repo."""
import argparse,json,os,subprocess,resource
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2]
p=argparse.ArgumentParser(); p.add_argument('--okkama',required=True); p.add_argument('--scratch',type=Path,required=True); p.add_argument('--output',type=Path,required=True); p.add_argument('--native',action='store_true'); p.add_argument('--steady',action='store_true'); a=p.parse_args()
a.scratch=a.scratch.resolve()
assert ROOT not in a.scratch.parents and a.scratch!=ROOT, 'scratch must be outside the repo'
a.scratch.mkdir(parents=True,exist_ok=True)
if a.native:
 for name in ['events','entities']:
  old=subprocess.check_output(['git','show','b6b10bdb7fc1ad299dcf71b349ed3ed2ced292f3:std/html/'+name+'.ok'],text=True,cwd=ROOT)
  old=old.replace('module html/'+name,'module html13/reference_'+name)
  (a.scratch/('reference-'+name+'.ok')).write_text(old)
def imports(source):
 if a.native:
  for name in ['events','entities']:source=source.replace('import "html/'+name+'"','import "./reference-'+name+'"')
 return source

env=dict(os.environ,OKKAMA_STDLIB_DIR=str(ROOT/'std'))
def limits(): resource.setrlimit(resource.RLIMIT_CPU,(45,45))
def run(path,n,w):
 r=subprocess.run([a.okkama,'run',str(path),str(n),str(w)],env=env,text=True,capture_output=True,timeout=60,preexec_fn=limits)
 if r.returncode or r.stderr: raise RuntimeError(r.stdout+r.stderr)
 assert r.stdout.startswith('micro-v2 '),r.stdout
 return r.stdout.strip()
(a.scratch/'harness.ok').write_text((ROOT/'bench/micro/support/harness.ok').read_text())
template=imports((ROOT/'docs/html-13/bench.ok.in').read_text())
fragment='<section ID="x" class=card hidden data-x="a&amp;b" title="é &#x1F600;"><p>A &copy; <b>bold</b> &notin;</p><input disabled value=42/><!-- c --></section>'
entity='café &amp; &notin; &#65; &#x1F600; &NotEqualTilde; &unknown; '
result={}
for kind,frag in [('events',fragment),('entities',entity)]:
 for size,rows,n,w in [('small',1,2000,200),('medium',max(1,5120//len(frag.encode())),100,10),('large',max(1,512000//len(frag.encode())),2,1)]:
  if a.steady: n,w={'small':(10000,1000),'medium':(300,30),'large':(5,2)}[size]
  body='for HtmlEvents(text) -> event { total.replace(total.value()+1); }' if kind=='events' else 'total.replace(total.value()+UnescapedHtml(text).string().length());'
  source=template.replace('BODY',body).replace('ROWS',str(rows)).replace('FRAGMENT',json.dumps(frag,ensure_ascii=False))
  key=kind+'-'+size; path=a.scratch/(key+'.ok'); path.write_text(source)
  samples=[run(path,n,w) for _ in range(3)]
  expected=(11 if kind=='events' else 26)*rows
  assert all(int(s.split()[4])==expected*n and int(s.split()[5])==expected*w for s in samples)
  result[key]={'bytes':len(frag.encode())*rows,'n':n,'warmup':w,'raw':samples,'ns_per_call':[int(s.split()[3])/n for s in samples]}
  a.output.write_text(json.dumps(result,indent=2)+'\n'); print(key,result[key],flush=True)
manifest=json.loads((ROOT/'bench/micro/manifest.json').read_text())
for key in manifest:
 if not key.startswith('html-'): continue
 path=ROOT/'bench/micro'/manifest[key]['path']; n,w=(10000,1000) if a.steady else (2000,200)
 if a.native:
  source=imports(path.read_text()).replace('"../support/harness"','"./harness"')
  path=a.scratch/('native-'+key+'.ok');path.write_text(source)
 samples=[run(path,n,w) for _ in range(3)]
 result[key]={'n':n,'warmup':w,'raw':samples,'ns_per_call':[int(s.split()[3])/n for s in samples]}
 a.output.write_text(json.dumps(result,indent=2)+'\n'); print(key,result[key],flush=True)
