import random,json,sys
from pathlib import Path
r=random.Random(130622)
dialects=[(',', '"', '\r\n'), (';', "'", '\n'), ('\t','"','\r\n'), ('😀','§','\n'), ('\0','é','\r\n'), ('|','\\','\n')]
values=['','a','é','😀','\ufeff','x,y','a"b','a\nb','a\rb','a\r\nb','\0',';',"'",'§','\\', ' xy ']
cases=[]
for i in range(6600):
 d,q,e=dialects[(i//4)%len(dialects)] if i%4 else dialects[0]
 rows=[[r.choice(values)+str(r.randrange(5)) if r.randrange(5)==0 else r.choice(values) for _ in range(r.randrange(1,6))] for _ in range(r.randrange(5))]
 def field(v): return q+v.replace(q,q+q)+q if any(x in v for x in [d,q,'\r','\n']) or r.randrange(3)==0 else v
 text=e.join(d.join(map(field,row)) for row in rows)+(e if rows and r.randrange(2) else '')
 if i%4==1:
  at=r.randrange(len(text)+1); text=text[:at]+r.choice([q,'\n','\r',d,'x',q+'x', '\ufeff'])+text[at:]
 if i%4==2 and text: text=text[:r.randrange(len(text))]
 limits=[len(text)+5,100,20,10]
 if i%3==1: limits[r.randrange(4)]=r.randrange(5)
 data=list(text.encode())
 if i%5==0:
  at=r.randrange(len(data)+1); data[at:at]=r.choice([[255],[194],[224,128,128],[237,160,128],[240,128,128,128],[244,144,128,128],[128],[240,159]])
 byte_limit=len(data)+2 if i%4 else r.randrange(len(data)+1)
 cases.append(dict(id=i,text=text,d=d,q=q,e=e,limits=limits,data=data,bytes=byte_limit,chunks=[r.randrange(1,12) for _ in range(11)]))
# Long spans crossing the reader's 4096 byte boundary, exact budgets and every split of a multibyte scalar.
for j,text in enumerate(['"'+('é😀x'*1600)+'"\r\n','a'*4095+'\r\n','"'+('a'*4093)+'""z"\r\n','\ufeff"a"','"\r\n"\r\n']):
 for chunk in [1,2,3,7,4096]:
  cases.append(dict(id=len(cases),text=text,d=',',q='"',e='\r\n',limits=[len(text),10000,100,100],data=list(text.encode()),bytes=len(text.encode()),chunks=[chunk]))
Path(sys.argv[1]).write_text(json.dumps(cases,ensure_ascii=True))
