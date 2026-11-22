"""Check/regenerate the hidden constant from the pinned native data, no downloads."""
import argparse,re,hashlib
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2]
p=argparse.ArgumentParser();p.add_argument('--write',action='store_true');a=p.parse_args()
data=ROOT/'okkama-rs/crates/okkama-runtime-core/data/html_entities.dat'
def literal(s):
 return '"'+''.join('\\u{%x}'%ord(c) if ord(c)<32 or ord(c)>126 else ('\\'+c if c in '\\"' else c) for c in s)+'"'
entries=[]
for line in data.read_text().splitlines():
 if '\t' not in line:continue
 name,points=line.split('\t');assert re.fullmatch('[A-Za-z0-9]+;?',name) and len(name)<=32
 entries.append('    '+literal(name)+': '+literal(''.join(chr(int(c,16)) for c in points.split())))
assert len(entries)==2231
block='hidden const HTML_ENTITIES = [\n'+',\n'.join(entries)+'\n];'
path=ROOT/'std/html/events.ok';source=path.read_text();old=re.search(r'hidden const HTML_ENTITIES = \[.*?\n\];',source,re.S).group()
if a.write:path.write_text(source.replace(old,block))
else:assert old==block
print('2231 entries; native table sha256='+hashlib.sha256(data.read_bytes()).hexdigest())
