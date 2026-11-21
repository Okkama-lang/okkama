"""Independent mathematical/content oracles; never learn expected output from a VM."""
import base64
import statistics
import hmac
import ipaddress
import gzip
import datetime
import zlib
import hashlib
import json
from functools import lru_cache


def rem(a, b):
    # Okkama integer remainder follows truncation toward zero.
    return a % b if a >= 0 else -((-a) % b)


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
    raise ValueError('No independent oracle: '+name)


@lru_cache(maxsize=512)
def expected(name,n):
    if n<0:raise ValueError('negative iteration count')
    return sum(contribution(name,i) for i in range(n))
