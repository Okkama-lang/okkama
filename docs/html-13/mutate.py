"""Mutations run only on scratch copies; no production source is overwritten."""
import argparse,json,os,re,subprocess,resource
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2]
p=argparse.ArgumentParser();p.add_argument('--okkama',required=True);p.add_argument('--scratch',type=Path,required=True);a=p.parse_args();a.scratch.mkdir(parents=True,exist_ok=True)
env=dict(os.environ,OKKAMA_STDLIB_DIR=str(ROOT/'std'))
def limits():resource.setrlimit(resource.RLIMIT_CPU,(45,45))
source=(ROOT/'std/html/events.ok').read_text()
entities=(ROOT/'std/html/entities.ok').read_text().replace('import "html/events"','import "./mutant-events"')
(a.scratch/'mutant-entities.ok').write_text(entities)
# name, original expression, replacement, root test module, intended test filter
mutations=[
 ('ASCII-lowercase','(c + 32).char().string()','c.char().string()','events','Unicode spaces'),
 ('Unicode-whitespace','|| c == 133','|| c == 134','events','Unicode spaces'),
 ('attribute-ambiguity','&& !(attribute','&& !(false','events','attribute ambiguity'),
 ('self-closing-slash','let closing = tail.value() > at + 1 && codes[tail.value() - 1].code() == 47;','let closing = false;','events','slash boundaries'),
 ('style-raw-mode','name == "script" || name == "style"','name == "script"','events','raw text ends'),
 ('raw-close-delimiter','same.value() && (self.space(after) || after == 47 || after == 62)','same.value()','events','raw text ends'),
 ('raw-Unicode-case','if c == 383','if c == 382','events','raw text ends'),
 ('comment-terminator','self.sequence(at + 4, 45, 45, 62)','self.sequence(at + 4, 45, 45, 33)','events','comments declarations'),
 ('declaration-trimming','origin.slice(at + 2, end).trim()','origin.slice(at + 2, end)','events','comments declarations'),
 ('instruction-target','let target = origin.slice(begin.value(), split.value());','let target = "wrong";','events','comments declarations'),
 ('quoted-tag-end','if point == 34 || point == 39','if point == 34','events','malformed markup'),
 ('unquoted-NUL','(quote != 0 && c == quote)','c == quote','events','preserves NUL'),
 ('numeric-zero','n == 0 || n > 1114111','n == 1 || n > 1114111','entities','scalar limits'),
 ('surrogate-boundary','n >= 55296 && n <= 57343','n > 55296 && n <= 57343','entities','scalar limits'),
 ('invalid-scalar-replacement','65533;','63;','entities','scalar limits'),
 ('numeric-C1','128: 8364','128: 128','entities','scalar limits'),
 ('hexadecimal-base','base.replace(16);','base.replace(10);','entities','scalar limits'),
 ('named-case','HTML_ENTITIES[name] else ""','HTML_ENTITIES[name.lower()] else ""','entities','longest case'),
 ('two-scalar-entity','"fjlig;": "fj"','"fjlig;": "f"','entities','longest case'),
 ('text-flush','if pieces.value().size() > 0','if pieces.value().size() > 1','events','malformed markup'),
 ('empty-cursor','position < origin.size();','position <= origin.size();','events','cursors repeat'),
 ('cursor-snapshot-identity','self.snapshot = snapshot;','self.snapshot = () is string {"";};','events','snapshot identity'),
 ('invented-input-limit','let codes = origin.characters();','let codes = origin.slice(0, if origin.length()>65536 {65536;} else {origin.length();}).characters();','events','no implicit'),
]
results=[]
for name,old,new,module,filter_ in mutations:
 assert old in source,name
 mutant=source.replace(old,new,1)
 path=a.scratch/'mutant-events.ok';path.write_text(mutant)
 root=path if module=='events' else a.scratch/'mutant-entities.ok'
 r=subprocess.run([a.okkama,'run','-t','--filter',filter_,str(root)],env=env,text=True,capture_output=True,timeout=60,preexec_fn=limits)
 log=r.stdout+r.stderr
 killed='--- FAIL:' in log and 'semantic error' not in log and 'syntax error' not in log
 results.append(dict(name=name,old=old,new=new,test=filter_,killed=killed,exit=r.returncode,log=log))
 (ROOT/'docs/html-13/mutations.json').write_text(json.dumps(results,indent=2,ensure_ascii=False)+'\n')
 print(name,'caught' if killed else 'NOT CAUGHT',flush=True)
 assert killed,log
print(len(results),'mutations caught')
