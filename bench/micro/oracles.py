"""Independent mathematical/content oracles; never learn expected output from a VM."""
import fractions
import uuid
import base64
import html
import re
import statistics
import hmac
import ipaddress
import gzip
import datetime
import zlib
import hashlib
import json
from functools import lru_cache
import posixpath
from html.parser import HTMLParser
import calendar
import difflib
import math
import email.utils
import configparser
from collections import Counter
import struct


def rem(a, b):
    # Okkama integer remainder follows truncation toward zero.
    return a % b if a >= 0 else -((-a) % b)


def x25519_public(scalar):
    """RFC 7748 section 5: X25519(k, 9), Montgomery ladder over p = 2^255 - 19."""
    p=2**255-19
    k=bytearray(scalar)
    k[0]&=248;k[31]&=127;k[31]|=64
    k=int.from_bytes(k,'little')
    x1,x2,z2,x3,z3,swap=9,1,0,9,1,0
    for t in reversed(range(255)):
        bit=(k>>t)&1
        swap^=bit
        if swap:x2,x3,z2,z3=x3,x2,z3,z2
        swap=bit
        a,b=(x2+z2)%p,(x2-z2)%p
        aa,bb=a*a%p,b*b%p
        e=(aa-bb)%p
        c,d=(x3+z3)%p,(x3-z3)%p
        da,cb=d*a%p,c*b%p
        x3=(da+cb)**2%p
        z3=x1*(da-cb)**2%p
        x2=aa*bb%p
        z2=e*(aa+121665*e)%p
    if swap:x2,x3,z2,z3=x3,x2,z3,z2
    return (x2*pow(z2,p-2,p)%p).to_bytes(32,'little')
class _HtmlEventCount(HTMLParser):
    """Counts events as an HTML tokenizer reports them; a self-closing tag is one start tag."""
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.events=0
        self.starts=0
    def handle_starttag(self,tag,attrs):
        self.events+=1
        self.starts+=1
    def handle_startendtag(self,tag,attrs):
        self.handle_starttag(tag,attrs)
    def handle_endtag(self,tag):
        self.events+=1
    def handle_data(self,data):
        self.events+=1
    def handle_comment(self,data):
        self.events+=1

def contribution(name, i):
    if name in {'baseline','constructor-generic','destroy','defer','async-await','spawn','channel','cancel','cancel-async-cleanup','event-wake','map-bigint-key','map-long-key','map-replace','map-empty','rat-add','rat-int-round-trip','json-parse','map-plus','array-plus','map-literal','map-literal-8','map-literal-12'}: return i % 7
    if name=='baseline-async':return i%7
    if name in {'defer-observed','destroy-observed'}:return i%7+1
    if name=='array-literal': return (i+i%3)%7
    if name=='array-index': return (i%8+1)%7
    if name=='array-minus': return (2+(i%5!=2))%7
    if name=='byte': return ((i%256)|3)%7
    if name in {'closure-create','constructor','map-miss'}:return (i+1)%7
    if name in {'closure-call','successor','loop-last'}:return (i+3)%7
    if name in {'constructor-defaults','reduce-build'}:return (i+8)%7
    if name=='compare':return 1 if i<1000 and i!=500 else 2
    if name=='event-many-waiters':return sum((i+k)%7 for k in range(8))
    if name=='filter-map':return (10+(2*i if i>1 else 0))%7
    if name=='float':return (3*i)%7
    if name in {'for-loop','method-call'}:return i%7
    if name=='int-big':return ((2*(9223372036854775807+i))%1000)%7
    if name=='int-small':return rem(i*3+i%5-7,7)
    if name=='map-index':return (i+2)%7
    if name=='map-int-literal':return (i+(2 if i in (2,3) else 3))%7
    if name=='map-keys':return 2
    if name=='map-merge-overlap':return (i+3)%7
    if name=='map-reduce':return (i+66)%7
    if name=='match':return (0 if i%3==0 else i if i%3==1 else i+1)%7
    if name=='rat-big':return ((3*(10**30+i))//7)%7
    if name=='rat-compare':return 1 if 3*i<1000 else 2
    if name=='rat-div':return (3*i//2)%7
    if name=='rat':return ((154*i+66)//21)%7
    if name=='rat-mod':return 3 if i%2 else 0
    if name=='rat-mul':return (2*i//3)%7
    if name=='rat-neg':return rem(-i,7)
    if name=='rat-new':return (i//7)%7
    if name=='recursion':return (i+16)%7
    if name=='role-call':return 2 if i%2==0 else 5
    if name=='string-concat':return (2+len(str(i)))%7
    if name=='string-length':return (10+i)%7
    if name=='string-slice':return (10-i%5)%7
    if name=='try-catch':return (1 if i%2 else i)%7
    if name=='base64-encode':return base64.b64encode(bytes([i%256,102,111,111,98,97,114,33]))[0]%7
    if name=='hex-encode':return ord(bytes([i%256,1,2,3,250,251,252,253]).hex()[0])%7
    if name=='int-parse':return (i%100000)%7
    if name=='json-round-trip':return len(json.dumps({'id':i,'tags':['a','b']},separators=(',',':')))%7
    if name=='regex-first-match':return len(str(i))%7
    if name=='sha256':return hashlib.sha256(bytes([i%256,1,2,3,4,5,6,7])).digest()[0]%7
    if name=='sorted':return min(5,3,i%9,1)%7
    if name=='string-replace':return (12+len(str(i)))%7
    if name=='string-split':return 4
    if name=='utf8-encode':return (13+len(str(i)))%7
    data8=bytes([i%256,1,2,3,4,5,6,7])
    if name=='hex-decode':return (i%256)%7
    if name=='crc32':return zlib.crc32(data8)%7
    if name=='adler32':return zlib.adler32(data8)%7
    if name=='sha512':return hashlib.sha512(data8).digest()[0]%7
    if name=='percent-encode':return len('a%20b%2F'+str(i))%7
    if name=='percent-decode':return len('a b/'+str(i))%7
    if name=='date-shift':return (datetime.date(2026,8,11)+datetime.timedelta(days=i%400)).day%7
    if name=='locale-case':return len('İSTANBUL'+str(i))%7
    if name=='normalize-nfc':return len('é'+str(i))%7
    if name=='json-render':return len(json.dumps({'id':i,'ok':True},separators=(',',':')))%7
    if name=='template-render':return len('Hello, Ada #'+str(i)+'!')%7
    if name=='base32-encode':return base64.b32encode(data8)[0]%7
    if name=='ip-parse':return len(str(ipaddress.ip_address('192.168.1.'+str(i%256))))%7
    if name=='hex-number-parse':return i%7
    if name=='hmac-sha256':return hmac.new(b'key',data8,hashlib.sha256).digest()[0]%7
    if name=='sha3-256':return hashlib.sha3_256(data8).digest()[0]%7
    if name=='gzip-round-trip':return (i%256)%7
    if name=='uuid-parse':return (4+36)%7
    if name=='xml-attribute':return len(str(i))%7
    if name=='toml-parse':return (1+len(str(i)))%7
    if name=='statistics-median':return int(statistics.median([1.0,5.0,float(i%10)]))%7
    if name=='text-indent':return len('  first\n  second'+str(i))%7
    if name=='html-escape':return len(html.escape('<a&'+str(i)+'>',quote=False))%7
    if name=='html-unescape':return len('café <'+str(i))%7
    if name=='query-parse':return len(str(i))%7
    if name=='regex-replace':return len(re.sub(r'\s+',' ','a  b   c'+str(i)))%7
    if name=='utf16-encode':return len(('A\U0001F600'+str(i)).encode('utf-16-be'))%7
    if name=='plural-ru':
        if i%10==1 and i%100!=11:return 1
        if 2<=i%10<=4 and not 12<=i%100<=14:return 2
        return 3
    if name=='date-format':return ord((datetime.date(2026,8,11)+datetime.timedelta(days=i%400)).strftime('%d/%m/%Y')[0])%7
    if name=='locale-decimal':return len(f'{i:,}'.replace(',','.')+',50')%7
    if name=='heap-top':return min(i%9,5,3)%7
    if name=='decimal-plus':
        digits=str(i*100+75)
        exponent=len(digits)-3
        text=digits[0]+('.'+digits[1:] if len(digits)>1 else '')+('e'+str(exponent) if exponent else '')
        return len(text)%7
    if name=='md5':return hashlib.md5(data8).digest()[0]%7
    if name=='sha1':return hashlib.sha1(data8).digest()[0]%7
    if name=='blake2b':return hashlib.blake2b(data8,digest_size=32).digest()[0]%7
    if name=='shake128':return hashlib.shake_128(data8).digest(16)[0]%7
    if name=='zlib-round-trip':return (i%256)%7
    if name=='locale-list':return len('tea, milk, and '+str(i))%7
    if name=='joined':return len('a, b, '+str(i))%7
    if name=='form-parse':return len(str(i))%7
    if name=='aes-gcm-round-trip':return (i%256)%7
    if name=='ed25519-sign-verify':return 1
    if name=='iso-date':return ord((datetime.date(2026,8,11)+datetime.timedelta(days=i%400)).isoformat()[9])%7
    if name=='regex-escape':return (len('price: $5.00 '+str(i))+4)%7
    if name=='unicode-category':return 1 if i%52<26 else 2
    if name=='xml-escape':return len('a &lt; b &amp; c'+str(i))%7
    if name=='csv-round-trip':return len('name,note\r\nAda,"x, '+str(i)+'"\r\n')%7
    if name=='text-buffer':return len('<li>'+str(i)+'</li>!\n')%7
    if name=='formatted':return len(str(i)+' of 9 done')%7
    if name == 'aggregate-extremes': return (min(i % 19, 7, 3) + max(i % 19, 7, 3)) % 7
    if name == 'message-header': return (len(str(i))) % 7
    if name == 'mime-disposition': return (len('attachment; filename=file'+str(i)+'.txt')) % 7
    if name == 'mime-media-type': return (len(str(i))) % 7
    if name == 'mime-text-body': return (len(('Привет '+str(i)).encode('utf-8'))) % 7
    if name == 'mime-transfer': return (base64.b64encode(bytes([i % 256, 1, 2]))[0]) % 7
    if name == 'mime-extension': return (len('image/png' if i % 2 == 0 else 'application/pdf')) % 7
    if name == 'mime-encoded-word': return (len('=?UTF-8?B?'+base64.b64encode(('café '+str(i)).encode()).decode()+'?=')) % 7
    if name == 'dns-fixed-record': return (len('node'+str(i)+'.example')) % 7
    if name == 'host-endpoint': return (len('example.com:'+str(i % 65536))) % 7
    if name == 'network-failure': return (len('failure '+str(i))) % 7
    if name == 'network-interface': return (len('if'+str(i)) + i + 1) % 7
    if name == 'ip-classification': return (128) % 7
    if name == 'multicast-options': return (i % 256 + int(i % 2 == 0)) % 7
    if name == 'public-suffix': return (len('node'+str(i)+'.co.uk')) % 7
    if name == 'connection-endpoints': return (2*i+1) % 7
    if name == 'tcp-options': return (i % 1024) % 7
    if name == 'tls-details': return (len('h2-'+str(i))) % 7
    if name == 'udp-datagram': return (i % 256) % 7
    if name == 'unix-address': return (len('run/service'+str(i % 10000)+'.sock')) % 7
    if name == 'bigint-add': return (10**30+i+987654321) % 7
    if name == 'bigrational-normalize': return (fractions.Fraction(i * 6, 18).numerator) % 7
    if name == 'shortest-float': return (int(str(i % 1000)+"5")) % 7
    if name == 'operator-adding': return (i+9) % 7
    if name == 'plist-parse': return (i) % 7
    if name == 'plist-integer': return (10**12+i) % 7
    if name == 'plist-write': return len(('<?xml version="1.0" encoding="UTF-8"?>\n<plist version="1.0"><string>item'+str(i)+'</string></plist>\n').encode()) % 7
    if name == 'pcg-bytes': return (pcg_first_word(i) & 255) % 7
    if name == 'seeded-uniform': return (pcg_integer16(i)) % 7
    if name == 'regex-capture': return (len(str(i))) % 7
    if name == 'regex-range': return (i % 13) % 7
    if name == 'regex-parts': return (1+len(str(i))) % 7
    if name == 'sizing-role': return (i % 16) % 7
    if name == 'text-template': return (len('Hi '+str(i)+'!')) % 7
    if name == 'html-template': return (len('<p>&lt;&amp;'+str(i)+'</p>')) % 7
    if name == 'html-template-library': return (len('<p>{{.}}</p>')+len('card'+str(i))) % 7
    if name == 'template-json-value': return (len(str(i))) % 7
    if name == 'text-template-library': return (len('Hi {{n}}!')+len('item'+str(i))) % 7
    if name == 'template-binding': return (len(str(i))) % 7
    if name == 'template-path': return (len('user.item'+str(i))) % 7
    if name == 'template-stream': return (len(('Hi '+str(i)+'!').encode())) % 7
    if name == 'template-sections': return (len(('value=' if i % 2 == 0 else '')+str(i))) % 7
    if name == 'template-tree': return (len('item'+str(i))+2) % 7
    if name == 'terminal-size': return (i % 200) % 7
    if name == 'terminal-failure': return (len('invalid')+len('input '+str(i))) % 7
    if name == 'terminal-color': return (len('38;5;'+str(i % 256))) % 7
    if name == 'display-width': return (4+len(str(i))) % 7
    if name == 'instant-shift': return (i*10**9) % 7
    if name == 'time-of-day': return (8+(1+len(f"{i%1000:03}".rstrip("0")) if i%1000 else 0)) % 7
    if name == 'duration-add': return (i*10**6+250000000) % 7
    if name == 'iso-text': return (i % 28 + 1) % 7
    if name == 'local-datetime-shift': return (i % 100000 % 60) % 7
    if name == 'rfc3339-parse': return ((i % 60)*10**9) % 7
    if name == 'fixed-zone': return ((i % 1440)*60*10**9) % 7
    if name == 'url-parts': return (len('https://example.com/a/'+str(i)+'?x=1')) % 7
    if name == 'uuid5-name': return (uuid.uuid5(uuid.NAMESPACE_DNS, 'item'+str(i)).bytes[0]) % 7
    if name == 'xml-name': return (len('t:item'+str(i))) % 7
    # #244, modules a-l: oracles follow the RFCs and Python's codecs, not VM output.
    if name=='qp-encode':
        b=i%256
        literal=(33<=b<=126 and b!=61) or b==32
        return ((1 if literal else 3)+11+(b if literal else 61))%7
    if name=='qp-decode':return (i%256+5)%7
    if name=='base85-encode':
        text=base64.a85encode(bytes([i%256,1,2,3,4])).decode()
        return (len(text)+ord(text[0]))%7
    if name=='base85-decode':return (i%256+6)%7
    if name=='latin1-decode':return (4+i%256)%7
    if name=='ascii-encode':return (len('row '+str(i))+ord(str(i)[0]))%7
    if name=='binary-int-round-trip':
        packed=struct.pack('>i',(i*7919)%2000000-1000000)
        return (packed[0]+packed[3]+1)%7
    if name=='binary-float-round-trip':
        packed=struct.pack('<d',i/8-1.5)
        return (packed[7]+packed[6]+1)%7
    if name=='pem-encode':
        body=base64.b64encode(bytes((i+7*k)%256 for k in range(40))).decode()
        text='-----BEGIN TEST DATA-----\n'+body+'\n-----END TEST DATA-----\n'
        return (len(text)+ord(text[26]))%7
    if name=='pem-parse':return (i%256+40+9)%7
    if name=='constant-time-equal':return 2 if i%3==0 else 1
    if name=='hkdf-sha256':
        prk=hmac.new(bytes([1,2,3,4]),bytes([i%256]+list(range(1,16))),hashlib.sha256).digest()
        key=hmac.new(prk,b'info\x01',hashlib.sha256).digest()
        return (key[0]+key[31])%7
    if name=='pbkdf2-sha256':
        key=hashlib.pbkdf2_hmac('sha256',('password-'+str(i)).encode(),b'salt',16,32)
        return (key[0]+key[31])%7
    if name=='chacha20-round-trip':return (i%256+8+16)%7
    if name=='jwt-hs256-sign':
        segment=lambda raw:base64.urlsafe_b64encode(raw).rstrip(b'=').decode()
        signing=segment(b'{"alg":"HS256","typ":"JWT"}')+'.'+segment(json.dumps({'sub':'user-'+str(i),'n':i},separators=(',',':')).encode())
        token=signing+'.'+segment(hmac.new(b'a-benchmark-secret-of-32-or-more-bytes',signing.encode(),hashlib.sha256).digest())
        return (len(token)+ord(token[-1]))%7
    if name in {'deflate-round-trip','zstd-round-trip','brotli-round-trip'}:return (i%256+8)%7
    if name=='keyed-set':
        values={i%10,(i*3)%10,5,7}
        return (len(values)+(1 if i%4 in values else 0))%7
    if name=='ordered-set-union':return len({'a','b',str(i%5)}|{'b','c',str(i%7)})%7
    if name=='deque-ends':return (i+1+4)%7
    if name=='frequencies-count':
        counts=Counter(['a','b','a',['a','b','c'][i%3]])
        return (counts['a']+len(counts)+4)%7
    if name=='ini-parse':
        parser=configparser.ConfigParser()
        parser.read_string('[server]\nhost=example\nport='+str(i)+'\n')
        return (len(parser['server']['port'])+(1 if 'host' in parser['server'] else 0))%7
    if name=='http-media-type':
        level=str(i%3)
        return (len('text/html; charset=utf-8; level='+level)+len('utf-8')+len(level))%7
    if name=='http-date-format':
        text=email.utils.formatdate(1700000000+i*86413,usegmt=True)
        return (len(text)+ord(text[5]))%7
    if name=='http-date-parse':
        return int(datetime.datetime(1994,11,6,8,49,i%60,tzinfo=datetime.timezone.utc).timestamp())%7
    if name=='http-status-phrase':
        # RFC 9110 section 15 reason phrases.
        phrases={200:'OK',201:'Created',204:'No Content',301:'Moved Permanently',304:'Not Modified',
                 400:'Bad Request',404:'Not Found',500:'Internal Server Error',503:'Service Unavailable'}
        code=[200,201,204,301,304,400,404,500,503][i%9]
        return (len(phrases[code])+code)%7
    if name=='http-method-semantics':
        # RFC 9110 sections 9.2.1 (safe) and 9.2.2 (idempotent).
        method=['GET','HEAD','POST','PUT','PATCH','DELETE','OPTIONS','TRACE'][i%8]
        safe=method in {'GET','HEAD','OPTIONS','TRACE'}
        return ((1 if safe else 0)+(2 if safe or method in {'PUT','DELETE'} else 0))%7
    if name=='http-entity-tags':
        # RFC 9110 section 8.8.3.2: W/"b1" weakly matches "b1"; a strong match needs two strong tags.
        return (2+(1 if i%3==1 else 0))%7
    if name=='http-request-target':return (len('/search/'+str(i))+len('q='+str(i)))%7
    if name=='http-quality':return (100*(i%10)+10*((i//10)%10))%7
    if name=='calendar-month':
        year,month=2000+i%30,1+i%12
        return (len(calendar.Calendar(0).monthdatescalendar(year,month))+calendar.monthrange(year,month)[1])%7
    if name=='diff-edits':
        mark=str(i%5)
        left=['alpha','beta','gamma',mark,'delta']
        right=['alpha','gamma','beta',mark,'delta',str(i%3)]
        # Edits list every line once: kept lines (a longest common subsequence) plus removed and added ones.
        common=[[0]*(len(right)+1) for _ in range(len(left)+1)]
        for a in range(len(left)):
            for b in range(len(right)):
                common[a+1][b+1]=common[a][b]+1 if left[a]==right[b] else max(common[a][b+1],common[a+1][b])
        edits=len(left)+len(right)-common[len(left)][len(right)]
        ratio=difflib.SequenceMatcher(None,left,right).ratio()
        return (edits+int(ratio*1000.0)+(1 if ratio==1.0 else 0))%7
    if name=='byte-search':
        source=bytes((k*31+7)%251 for k in range(512))
        start=i%300
        return source.find(source[start+40:start+43],start)%7
    if name=='byte-range-integer':return int.from_bytes(bytes([i%256,(i*7)%256,200]),'big')%7
    if name=='combinations-count':
        size=1+i%4
        return (math.comb(5,size)*size)%7
    if name=='x25519-public-key':
        raw=x25519_public(bytes([i%256]+list(range(1,32))))
        return (raw[0]+raw[31])%7
    if name=='cookie-date':
        return int(datetime.datetime(2015,10,21,7,28,i%60,tzinfo=datetime.timezone.utc).timestamp())%7
    if name=='fs-path':
        path=posixpath.normpath('/srv'+'/d'*(i%4)+'/./logs//file'+str(i)+'.tar.gz')
        parts=['/']+path.strip('/').split('/')
        name_part=posixpath.basename(path)
        return (len(parts)+len(parts)-1+len(name_part)+len(name_part.rsplit('.',1)[1]))%7
    if name=='html-events':
        parser=_HtmlEventCount()
        parser.feed('<p class=x>Hello <b>'+str(i)+'</b>'+('<br/>' if i%2==0 else '')+'</p><!-- c -->')
        parser.close()
        return (parser.events+3*parser.starts)%7
    if name=='currency-digits':
        # ISO 4217 minor units.
        code=['USD','JPY','BHD','EUR','KWD','CLP','GBP','TND'][i%8]
        return ({'USD':2,'JPY':0,'BHD':3,'EUR':2,'KWD':3,'CLP':0,'GBP':2,'TND':3}[code]+3)%7
    if name=='views-filtered-chunked':
        kept=[v for v in [i%5,1,2,3,4,5,6] if v%2==0]
        chunks=[kept[k:k+2] for k in range(0,len(kept),2)]
        return (3*len(chunks)+sum(chunk[0] for chunk in chunks))%7
    raise ValueError('No independent oracle: '+name)


def pcg_words(seed):
    """PCG-XSH-RR 64/32, from the published random/source contract."""
    mask = (1 << 64) - 1
    increment = 1442695040888963407
    state = (increment + seed) & mask
    state = (state * 6364136223846793005 + increment) & mask
    while True:
        old = state
        state = (old * 6364136223846793005 + increment) & mask
        shifted = (((old >> 18) ^ old) >> 27) & 0xffffffff
        rotation = old >> 59
        yield ((shifted >> rotation) | (shifted << ((-rotation) & 31))) & 0xffffffff


def pcg_first_word(seed):
    return next(pcg_words(seed))


def pcg_integer16(seed):
    words = pcg_words(seed)
    return ((next(words) << 32) | next(words)) % 65536


@lru_cache(maxsize=512)
def expected(name,n):
    if n<0:raise ValueError('negative iteration count')
    return sum(contribution(name,i) for i in range(n))
