from pathlib import Path
import subprocess,os,json,re,argparse
import resource
ROOT=Path(__file__).resolve().parents[2]
parser=argparse.ArgumentParser(description="Run 13 CSV mutations, always restoring the source.")
parser.add_argument('--okkama', required=True)
parser.add_argument('--scratch', type=Path, required=True)
args=parser.parse_args()
BIN=args.okkama
args.scratch.mkdir(parents=True,exist_ok=True)
def limits(): resource.setrlimit(resource.RLIMIT_CPU,(45,45))
os.environ['OKKAMA_STDLIB_DIR']=str(ROOT/'std')
text=ROOT/'std/csv.ok'; stream=ROOT/'std/csv/stream.ok'
mutations=[
 ('default-delimiter',text,'if code == 44','if code == 43'),
 ('default-CR-requires-LF',text,'confirm code == 10 else','confirm code == 13 else'),
 ('default-undoubling',text,'origin.slice(beginning, end - 1).replace("\\\"\\\"", "\\\"")','origin.slice(beginning, end - 1)'),
 ('default-final-column-bound',text,'confirm fields.size() < maximum_columns else throw CsvLimitExceeded.kind(CsvLimit.columns);','confirm fields.size() <= maximum_columns else throw CsvLimitExceeded.kind(CsvLimit.columns);'),
 ('stream-retain-UTF8-tail',stream,'buffer.value().slice(position.value(), buffer.value().size()) + bytes','bytes'),
 ('stream-CR-requires-LF',stream,'if lead != 10','if lead != 13'),
 ('stream-byte-exact-bound',stream,'if size > maximum_bytes - used','if size >= maximum_bytes - used'),
 ('stream-input-bound',stream,'if at >= maximum_input','if at > maximum_input'),
 ('stream-decoded-field-bound',stream,'if amount.value() >= maximum_field','if amount.value() > maximum_field'),
 ('stream-row-bound',stream,'if count.value() >= maximum_rows','if count.value() > maximum_rows'),
 ('stream-column-bound',stream,'if fields.value().size() >= maximum_columns','if fields.value().size() > maximum_columns'),
 ('stream-syntax-offset',stream,'self.failed("syntax", at)','self.failed("syntax", at + 1)'),
 ('stream-surrogate-rejection',stream,'|| (lead == 237 && second >= 160)',''),
]
results=[]
for name,p,old,new in mutations:
 original=p.read_text(); source=original
 # Restrict default mutations to the new default parser, not the old custom one.
 start=source.index('hidden object CsvDefaultPrefix') if p==text else source.index('hidden object CsvScanner')
 pattern=re.escape(old).replace(r'\ ', r'\s+')
 assert re.search(pattern,source[start:]),name
 source=source[:start]+re.sub(pattern,lambda _:new,source[start:])
 try:
  p.write_text(source)
  r=subprocess.run([BIN,'run','-t',str(p)],text=True,capture_output=True,timeout=60,preexec_fn=limits)
  log=r.stdout+r.stderr
  assert r.returncode!=0 and ('FAIL' in log or 'failed' in log.lower()) and 'semantic error' not in log and 'syntax error' not in log,(name,r.returncode,log)
  failed=[x for x in log.splitlines() if 'FAIL' in x or 'expected' in x or 'actual' in x]
  results.append(dict(name=name,status=r.returncode,failed=failed))
  (args.scratch/(name+'.log')).write_text(log)
  print(name,'killed',flush=True)
 finally: p.write_text(original)
(args.scratch/'mutations.json').write_text(json.dumps(results,indent=2))
