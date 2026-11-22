"""Deterministic corpus: every pinned spelling plus malformed Unicode HTML."""
import json,random,sys
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2]
r=random.Random(131122)
names=[s.split('\t')[0] for s in (ROOT/'okkama-rs/crates/okkama-runtime-core/data/html_entities.dat').read_text().splitlines() if '\t' in s]
cases=['', '<', '</', '<!--', '<!>', '<??>', 'a<? ?>b', '<![CDATA[&amp;<b>]]>', '<script>x</ſcript>', '<script>x</scrıpt>', '<SCRIPT>x</SCRIPT>', '<style/>a&amp;']
for name in names:
 cases.append('&'+name+'x <P A="&'+name+'=" B=&'+name+'x C=\'&'+name+';\'>&'+name+'</P>')
refs=['&','&;','&#;','&#x;','&notit;','&notin;','&amp=','&amp;lt;','&unknown;']
for point in [0,1,9,13,31,127,*range(128,160),55295,55296,57343,57344,1114111,1114112,4294967295,4294967296,9999999999999999]:
 for text in [str(point),'x'+format(point,'x'),'X'+format(point,'X')]:
  for ending in ['', ';']:
   refs.append('&#'+text+ending);refs.append('&#'+'0'*12+text+ending if not text.startswith(('x','X')) else '&#'+text[0]+'0'*12+text[1:]+ending)
for ref in refs:cases.append(ref+' <p a="'+ref+'" b='+ref+'>')
tags=['p','B','script','style','textarea','title','svg','aÉ','Aİ','x-😀']
ws=[' ','\t','\r\n','\v','\u0085','\u00a0','\u2003','\u2028','\u3000','\u001c']
pieces=['é😀\0','<','</','<!-- x -->','<!--unfinished','<!DOCTYPE "x>y">','<![CDATA[a<b&copy;]]>','<?XML a?>','<? >','<?x>foo?>','<1a>','</   >','<>','&NotEqualTilde;']
while len(cases)<7200:
 tag=r.choice(tags);w=r.choice(ws); ref=r.choice(refs)
 text='<'+tag+w+r.choice(['x="a>b"','A=1 a=2','x='+ref,'x=\''+ref+'\'','disabled','x=""','x=','x=y/z','x=1/  ','=z','a/ b','é=😀'])+r.choice(['>','/>',' / >'])+r.choice(pieces)+ref+'</'+w+tag+r.choice(['>','/ >',' x>',''])
 text=r.choice(pieces)+text if r.randrange(3)==0 else text
 if r.randrange(4)==0:text=text[:r.randrange(len(text)+1)]
 if r.randrange(4)==0:
  at=r.randrange(len(text)+1);text=text[:at]+r.choice(pieces)+text[at:]
 cases.append(text)
cases += ['a'*10000+'&amp;'+'é'*10000, '<p '+'x=1 '*1200+'>', '<p>'*2000+'x'+'</p>'*2000, '&#'+'0'*10000+'65;', '&'+'a'*10000+';']
Path(sys.argv[1]).write_text(json.dumps(cases,ensure_ascii=True))
print(len(cases))
