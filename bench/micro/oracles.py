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
import gettext
import csv
import io
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

# The http, web and mail programs (#244), answered by http244_contribution.
HTTP244_PROGRAMS = frozenset({
    'hpack-decoded', 'hpack-primitives-bytes', 'hpack-static-names', 'hpack-tables-added',
    'http-auth-authorization', 'http-auth-challenge-values', 'http-body-coding-contained',
    'http-body-opening', 'http-cache-admission-join', 'http-cache-control-seconds',
    'http-cache-freshness-age', 'http-cache-memory-headers', 'http-cache-response',
    'http-cache-validation-request', 'http-client-policy-http1', 'http-client-protocols-selected',
    'http-client-request', 'http-content-coding-string', 'http-conversation-response',
    'http-cookie-client-context', 'http-cookie-jar-store', 'http-cookie-pair', 'http-cookie-policy-matches',
    'http-digest-response-digest', 'http-endpoint-allowed', 'http-environment-proxy', 'http-exchange-string',
    'http-form-parameters', 'http-handler-response', 'http-header-int', 'http-message-new',
    'http-proxy-policy-url', 'http-proxy-request', 'http-proxy-tunnel-ready', 'http-redirect-response',
    'http-request-context-parameters', 'http-retry-response', 'http-robots-allowed', 'http-routing-match',
    'http-server-policy-handling-timeout', 'http-sse-retry', 'http-static-indexes',
    'http-tls-session-version', 'http-token-string', 'http-trailers-check',
    'http-transport-observation-begin', 'http-version', 'http-wire-bytes', 'http-xmlrpc-bytes',
    'http1-body-bytes', 'http1-body-check-known', 'http1-body-writing-output', 'http1-chunk-size',
    'http1-continue-begun', 'http1-framing-plan', 'http1-head', 'http1-message-writing-output',
    'http1-reading-head', 'http1-session-response', 'http1-writing-bytes', 'http2-body-known',
    'http2-controls-frame', 'http2-fields', 'http2-flow-sent', 'http2-frame-bytes', 'http2-headers-frames',
    'http2-input-run', 'http2-life-allocated', 'http2-output-submit', 'http2-preface-settings',
    'http2-reading-frame', 'http2-receiving-bodies', 'http2-sending-close', 'http2-session-response',
    'http2-settings-bytes', 'http2-settings-setting', 'http2-tls-version', 'imap-command-bytes',
    'imap-command-reading-string', 'imap-data-tokens', 'imap-exchange-perform', 'imap-message-uid',
    'imap-metadata-uids', 'imap-name', 'imap-name-wire-name', 'imap-reading-read', 'imap-response',
    'imap-search-key', 'imap-session-expire', 'imap-state-messages', 'imap-uid-argument',
    'mail-mailbox-membership', 'mail-message-string', 'mbox-string', 'pop3-reading-read', 'pop3-reply',
    'pop3-session-bytes', 'pop3-size', 'smtp-data-bytes', 'smtp-maximum-size', 'smtp-reading-read',
    'smtp-reply-fed', 'smtp-session-response', 'web-around-routes', 'web-exception-response',
    'web-internal-reply-response', 'web-reply-response', 'web-request-link', 'web-routing-routes',
    'web-session-text', 'web-testing-get', 'websocket-client-limits', 'websocket-frame-bytes',
    'websocket-handshake-string', 'websocket-messages-accept', 'websocket-origin-allowed',
    'websocket-output-message', 'websocket-reading-frame', 'websocket-server-protocols', 'websocket-text',
    'websocket-upgrade-bytes',
})
# The RSA-2048 key of the rsa-pkcs1-sign benchmark (its PKCS#8 PEM is in that program).
RSA_TEST_KEY = {
    'prime1': 162361397268632976590698668520293315391989932528885618395605771025135312302908481441354204980634880370398703294807749768882263304214744859677047835321986749050428346485021662099292511460068622787509832232659479884887779834887536866510631440437662281668525006576207179643345932903776859487385761647858631878639,
    'prime2': 159136473945357531731946905247436674558942081048427570697105912577667669023773088813119039868893471616548306359995554935067290123655497262028936776432146366763531965869181190834953252909077651861116460382487503440474685251220161403417655821943079772563970619744888799393895618483287060436974443514350757956147,
    'exponent1': 80106745512935399820868233922878440471794474393419236016630492791823707574569262361813757389700914853851138609298695222192166012203545707579799834523039527756009591278105122968409041580501379645606762977295944002286439125396165594681191809612155116617357463534212111536499851083260215626663085018897857115763,
    'exponent2': 98968283642291869910595418833254863385161990011318963737623674944543980561681852294812179791207411327147655074548710316973200037535296968213000368247271337054610920013062651266475802707908310336991084307634247298882573530513013083914381433247723352765038291645363347203814602715135175691444594179744391607437,
    'coefficient': 119148132139312305680700028627364090970081565327820650652007413624058338594928045993755915670426038010035077902738030248012060749859108852937574068794435768675202906239244488805738395092936155496125494557839534027519185279781096040631424602575522021210897242731272798863025729323634889487599613203599515138222,
}


# The GNU .mo catalog of the gettext-catalog benchmark (the same bytes are in that program).
GETTEXT_TEST_MO = b'\xde\x12\x04\x95\x00\x00\x00\x00\x03\x00\x00\x00\x1c\x00\x00\x004\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00L\x00\x00\x00\n\x00\x00\x00M\x00\x00\x00\x05\x00\x00\x00X\x00\x00\x00S\x00\x00\x00^\x00\x00\x00\x18\x00\x00\x00\xb2\x00\x00\x00\x05\x00\x00\x00\xcb\x00\x00\x00\x00file\x00files\x00hello\x00Content-Type: text/plain; charset=UTF-8\nPlural-Forms: nplurals=2; plural=(n != 1);\n\x00eine Datei\x00viele Dateien\x00Hallo\x00'


def contribution(name, i):
    if name in WORK244_ORACLES:
        v = i % 251 + 1
        return WORK244_ORACLES[name](i, v, str(v))
    if name in HTTP244_PROGRAMS:
        return http244_contribution(name, i)
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
    # #244, the rest of std outside http/web/mail/db: oracles from the module contracts and Python.
    if name=='cell-replace':return (i%5+i%3)%7
    if name=='zipped-chained':
        left=[i%4,1,2,3]
        right=['a','bb',str(i%10)]
        total=sum(a+len(b) for a,b in zip(left,right))
        total+=sum([i%3]+[4,5]+left)
        return total%7
    if name=='enumerated':
        return sum((i%5+k)*len(item) for k,item in enumerate(['x','yy',str(i%100)]))%7
    if name=='repeated':
        count,value=1+i%6,i%9
        return (value*count+count+value)%7
    if name=='archive-path':
        return (len('guide-'+str(i)+'.md')+len('docs/'+str(i%50))+3)%7
    if name=='tar-round-trip':return (2+len('hello '+str(i))+2+1)%7
    if name=='setting-key':return (4+len('server.http.'+str(i%20)+'.port')+2)%7
    if name=='process-arguments':return (3+len('--n'+str(i%10))+len('é')+len('é'.encode()))%7
    if name=='counter-nonce':
        counter=(i*65537).to_bytes(8,'big')
        return (counter[7]+counter[5]+12)%7
    if name=='process-exit':
        exited=i%5==0
        return ((2 if exited and i%3==0 else 0)+(i%3 if exited else 0))%7
    if name=='environment-values':return (i%50+len(('/bin:/usr/bin:/opt/'+str(i%7)).split(':'))+1)%7
    if name=='layered-settings':
        port=8000+i%100
        return (port+len('b'+str(i%3))+len(str(port)))%7
    if name=='cli-invocation':
        return ((i%50+1+2) if i%2==0 else (i%50+0+1))%7
    if name=='dumper-shape':return (3+len('x')+len('y')+len('label')+len('object')+3+len('t'+str(i)))%7
    if name=='message-catalog':
        # German plurals (CLDR): one for 1, other otherwise.
        return (len('Hallo')+len('eine Datei' if i%3==1 else 'viele Dateien')+len('Beenden'))%7
    if name=='likely-locale':
        # CLDR likely subtags.
        likely={'en':'en-Latn-US','ru':'ru-Cyrl-RU','zh':'zh-Hans-CN','ja':'ja-Jpan-JP','de':'de-Latn-DE',
                'ar':'ar-Arab-EG','sr':'sr-Cyrl-RS','pt':'pt-Latn-BR','hi':'hi-Deva-IN'}
        text=likely[['en','ru','zh','ja','de','ar','sr','pt','hi'][i%9]]
        return (len(text)+ord(text[3]))%7
    if name=='basic-language-range':
        # RFC 4647 section 3.3.1 basic filtering and section 3.4 lookup.
        available=['en-US','en-GB','de-DE','de','fr-CA']
        given=['en','en-GB','de','*','fr-FR','es'][i%6]
        matches=lambda tag:given=='*' or tag.lower()==given.lower() or tag.lower().startswith(given.lower()+'-')
        matched=sum(len(tag) for tag in available if matches(tag))
        return (matched+len(given)+(1 if matches('en-GB') else 0))%7
    if name=='localized-date':
        # CLDR en-US long date: MMMM d, y.
        year,month,day=2000+i%30,1+i%12,1+i%28
        text=calendar.month_name[month]+' '+str(day)+', '+str(year)
        return (len(text)+ord(text[0]))%7
    if name=='json-log-line':
        level=['debug','info','warning','error','critical'][i%5]
        moment=datetime.datetime.fromtimestamp(1700000000+i,datetime.timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')
        line=json.dumps({'time':moment,'level':level,'name':'app.http','process':42,'message':'request '+str(i),
                         'fields':{'id':i}},separators=(',',':'))
        return (len(line)+ord(line[-3]))%7
    if name=='log-pipeline':
        level,login=i%5,(i+2)%5
        messages=(['start '+str(i)] if level>=1 else [])+(['query'] if level>=3 else [])+(['login'] if login>=1 else [])
        return (len(messages)+sum(len(m) for m in messages))%7
    if name=='logger-levels':
        parsed=i%5
        return (1+(1 if i%2==0 else 0)+(1 if parsed>=2 else 0)+len(['debug','info','warning','error','critical'][parsed]))%7
    if name=='x509-certificate':
        # The leaf and CA test certificates of net/tls: CN=localhost (DNS:localhost) and CN=Okkama Test CA,
        # both issued by the CA and valid until 2048-10-07 11:42:04 UTC.
        ends=int(datetime.datetime(2048,10,7,11,42,4,tzinfo=datetime.timezone.utc).timestamp())
        subject,names=(('localhost',1) if i%2==0 else ('Okkama Test CA',0))
        return (len(subject)+len('Okkama Test CA')+names+ends%7)%7
    if name=='ecdsa-sign-verify':return 1
    if name=='parsed-key-algorithm':
        return ((1 if i%2==0 else 2)+2*(1 if (i//2)%2==0 else 2))%7
    if name=='rsa-pkcs1-sign':
        # RFC 8017 RSASSA-PKCS1-v1_5 with SHA-256, by CRT with the key's own numbers.
        message=('message '+str(i)).encode()
        digest=bytes.fromhex('3031300d060960864801650304020105000420')+hashlib.sha256(message).digest()
        encoded=int.from_bytes(b'\x00\x01'+b'\xff'*(256-3-len(digest))+b'\x00'+digest,'big')
        m1=pow(encoded,RSA_TEST_KEY['exponent1'],RSA_TEST_KEY['prime1'])
        m2=pow(encoded,RSA_TEST_KEY['exponent2'],RSA_TEST_KEY['prime2'])
        h=(RSA_TEST_KEY['coefficient']*(m1-m2))%RSA_TEST_KEY['prime1']
        signature=(m2+h*RSA_TEST_KEY['prime2']).to_bytes(256,'big')
        return (signature[0]+signature[255]+256)%7
    if name=='argon2-password':
        # PHC string: $argon2id$v=19$m=8,t=1,p=1$ + 16-byte salt and 32-byte hash in unpadded Base64.
        return (len('$argon2id$v=19$m=8,t=1,p=1$')+22+1+43+(0 if i%3==0 else 1))%7
    if name=='decimal-argument':
        text=str(i*31-500)
        return (len(text)+ord(text[0]))%7
    if name=='async-channel-items':return (i%5+i%3)%7
    if name=='async-event':return (2+i%3)%7
    if name=='async-lock-guard':return (1+2+i%2)%7
    if name=='async-semaphore':
        permits=1+i%3
        return (permits-1+permits)%7
    if name=='async-barrier':
        # One party: its arrival index is parties-1 = 0 and the release starts generation 1.
        return (0+1+1+i%4)%7
    if name=='async-cancellation':return (2 if i%2==0 else 0)%7
    if name=='async-all-futures':return (i%7+(i+1)%7+(2*i)%7)%7
    if name=='async-future-group':return (i%7+(i+3)%7)%7
    if name=='async-within':return (i%7)%7
    if name=='parallel-map':return (i*i+1+(i+3)*(i+3)+1)%7
    if name=='read-until-end':return len('line '+str(i)+'\nend')%7
    if name=='byte-input-lines':return (len('first '+str(i))+len('second'))%7
    if name=='csv-input':
        rows=list(csv.reader(io.StringIO('name,note\r\nAda,'+str(i)+'\r\nBob,"x, y"\r\n')))
        return (len(rows)+len(rows[1][1])+len(rows[2][1]))%7
    if name=='csv-stream':
        rows=list(csv.reader(io.StringIO('a,b\r\n'+str(i)+',"q""t"\r\n')))
        return sum(1+len(row[1]) for row in rows)%7
    if name=='read-digest':
        digest=hashlib.sha256(bytes((i+k)%256 for k in range(40))).digest()
        return (digest[0]+digest[31])%7
    if name=='compressed-reading':return (i%256+64+(100-64))%7
    if name=='native-codec':return (i%256+8)%7
    if name=='zip-round-trip':return (2+len('a.txt')+len('hello '+str(i))+i%256+8)%7
    if name=='fs-temporary-tree':
        # One file, one symbolic link beside it; *.txt matches the file only.
        return (len('hello '+str(i))+1+1+2+len('data-'+str(i)+'.txt')+5+1)%7
    if name=='error-matching':return (1+(2 if i%3==1 else 0))%7
    if name=='test-report-counts':
        statuses=['passed','failed','skipped','passed','timed_out']
        results=[statuses[(i+k)%5] for k in range(1+i%4)]
        return (len(results)+2*results.count('passed')+i%3+1)%7
    if name=='text-snapshot':return (len(('snapshot '+str(i)+'\n').encode())+i%256+2)%7
    if name=='integer-generation':
        value=10+i%10
        return (value+len(str(value)))%7
    if name=='size-rotation':
        value=(i*37)%2000
        size=value+1-value%2
        return ((1 if size>1000 else 0)+size%5)%7
    if name=='json-stream-logs':
        level=['debug','info','warning','error','critical'][i%5]
        moment=datetime.datetime.fromtimestamp(1700000000+i,datetime.timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')
        line=json.dumps({'time':moment,'level':level,'name':'app.http','process':42,'message':'request '+str(i),
                         'fields':{'id':i}},separators=(',',':'))+'\n'
        return (len(line.encode())+10)%7
    if name=='language-preferences':
        return (len('de-CH')+len('en')+len('fr' if i%2==0 else 'it-IT')+(1 if i%3==0 else 0))%7
    if name=='locale-fallbacks':
        # CLDR lookup chains: truncation, except sr-Latn, whose parent locale is root.
        chains=[['de-CH','de','root'],['fr','root'],['sr-Latn-RS','sr-Latn','root'],['en-US','en','root']]
        return (sum(1+len(key) for key in chains[i%4])+len('pt-BR'))%7
    if name=='verified-chain':return (len('localhost')+i%3)%7
    if name=='gettext-catalog':
        translations=gettext.GNUTranslations(io.BytesIO(GETTEXT_TEST_MO))
        return (len(translations.gettext('hello'))+len(translations.ngettext('file','files',i%3))+1)%7
    if name=='command-string':
        quoted=lambda text:'"'+text.replace('\\','\\\\').replace('"','\\"').replace('\n','\\n').replace('\r','\\r').replace('\t','\\t')+'"'
        arguments=['--name','a "quoted" '+str(i),'tab\there']
        text='Command(program: '+quoted('/usr/bin/tool')+', arguments: ['+', '.join(quoted(a) for a in arguments)+'], directory: '+quoted('/srv/work')+')'
        return (len(text)+ord(text[20]))%7
    if name=='host-digest':
        digest=hashlib.sha256(bytes((i*3+k)%256 for k in range(24))).digest()
        return (digest[0]+digest[31])%7
    # #244, the remaining measurable std modules.
    if name=='sqlite-memory-aggregate':
        import sqlite3
        db=sqlite3.connect(':memory:',isolation_level=None)
        db.execute('CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT, amount INTEGER)')
        for k in range(5):
            db.execute('INSERT INTO t(name, amount) VALUES (?, ?)',('n'+str(k),i%7+k))
        count,total=db.execute('SELECT COUNT(*), SUM(amount) FROM t WHERE amount > ?',(2,)).fetchone()
        db.execute('BEGIN')
        deleted=db.execute('DELETE FROM t WHERE amount < ?',(i%4+1,)).rowcount
        db.execute('COMMIT' if i%2==0 else 'ROLLBACK')
        left=db.execute('SELECT COUNT(*) FROM t').fetchone()[0]
        db.close()
        return (count+total+left+2*deleted)%7
    if name=='sqlite-prepared-blob':
        import sqlite3
        db=sqlite3.connect(':memory:',isolation_level=None)
        db.executescript('CREATE TABLE v(id INTEGER PRIMARY KEY, n INTEGER, data BLOB); INSERT INTO v(n, data) VALUES (1, zeroblob(8));')
        for k in range(3):
            db.execute('INSERT INTO v(n, data) VALUES (?, ?)',(i%5+k,bytes([k,i%256])))
        rows=db.execute('SELECT n FROM v WHERE n >= ? ORDER BY n',(2,)).fetchall()
        db.close()
        return (2+len(rows)+rows[0][0]+i%256+9+8)%7
    if name=='log-file':return (len('line '+str(i)+'\n')+len('second\n'))%7
    if name=='queued-logs':return (1+i%3)%7
    if name=='process-capture':
        return (len('hello '+str(i)+'\n')+len('in '+str(i))+len('p'+str(i)+'\n')+1)%7
    if name=='host-facts':return (1+2+1+i%3)%7
    if name=='cldr-tables':
        # IANA preferred values, CLDR likely subtags, parent locales, canonical zone ids
        # (CLDR keeps Asia/Calcutta), numbering-system digits and ISO 4217 minor units.
        k=i%4
        alias=['he','ro','id','en'][k]
        full=['ru-Cyrl-RU','ja-Jpan-JP','zh-Hans-CN','sr-Latn-ME'][k]
        parent=['en-001','es-419','pt-PT',''][k]
        zone=['Asia/Calcutta','Asia/Calcutta','Europe/Paris','America/Buenos_Aires'][k]
        one=['\u0661','1','\u0967','\u0e51'][k]
        minor=['0','2','3','2'][k]
        return (len(alias)+len(full)+len(parent)+len(zone)+ord(one)+len(minor)+ord(minor))%7
    if name=='test-reports':
        # TAP 13: passed and skipped (with # SKIP) are "ok" lines, failures "not ok"; JUnit: one
        # <testcase> per case and a <failure> per failed case.
        statuses=[['passed','failed','skipped','passed'][(i+k)%4] for k in range(1+i%4)]
        failed=statuses.count('failed')
        return ((len(statuses)-failed)+2*failed+1+3*len(statuses)+failed)%7
    if name=='sqlite-file-pool':
        # The backup copies every page in one 16-page step; the page count comes from SQLite itself.
        import sqlite3
        db=sqlite3.connect(':memory:',isolation_level=None)
        db.execute('CREATE TABLE t(x INTEGER)')
        db.execute('INSERT INTO t(x) VALUES (?)',(i%9,))
        db.execute('INSERT INTO t(x) VALUES (?)',(3,))
        pages=db.execute('PRAGMA page_count').fetchone()[0]
        db.close()
        return (i%9+pages+1)%7
    if name=='db-column-reader':return (i%1000+len('v'+str(i))+i%13+1)%7
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


# BEGIN #244 HTTP / WEB / MAIL (independent, hand-written formulas)
def http244_contribution(name, i):
    v = i % 251 + 1
    x = str(v)
    # Basic: сборка Authorization и Base64
    if name == 'http-auth-authorization': return sum(b"Basic " + base64.b64encode(("user"+x+":pw"+x).encode()))
    # Разбор Basic challenge и quoted realm
    if name == 'http-auth-challenge-values': return len("area"+x)
    # Текстовое тело: UTF-8 и длина
    if name == 'http-body-opening': return len("body"+x)
    # Счётчик участников cache admission
    if name == 'http-cache-admission-join': return 2+len("key"+x)
    # Разбор Cache-Control max-age
    if name == 'http-cache-control-seconds': return v
    # Копирование сохраняемых cache headers
    if name == 'http-cache-memory-headers': return len(x)
    # Преобразование лимитов HTTP/1
    if name == 'http-client-policy-http1': return 1024+v
    # Подготовка прямого HTTP request
    if name == 'http-client-request': return len("/a"+x)
    # Валидация и lowercase content coding
    if name == 'http-content-coding-string': return ord("x")+len("x-coding"+x)
    # Рендеринг cookie pair
    if name == 'http-cookie-pair': return len("sid="+x)
    # Граница сегмента пути cookie
    if name == 'http-cookie-policy-matches': return v
    # Ключ origin соединения
    if name == 'http-exchange-string': return len("https://example.test:"+str(8000+v))
    # Case-insensitive поиск и целочисленный header
    if name == 'http-header-int': return v
    # Размер chunk и extension (цифры трактуются как hex)
    if name == 'http1-chunk-size': return int(x,16)
    # Парсинг request line и Host
    if name == 'http1-head': return len("/a"+x)
    # Выбор fixed-length framing
    if name == 'http1-framing-plan': return v
    # Сериализация SETTINGS
    if name == 'http2-settings-bytes': return sum(bytes.fromhex("0001000010000002000000000003000000640004")+(65535+v).to_bytes(4,"big")+bytes.fromhex("000500004000"))
    # HPACK prefix integer (RFC 7541 §5.1)
    if name == 'hpack-primitives-bytes': return 127 + (v if v<128 else (v%128+128)+(v//128))
    # HPACK static table первые семь записей
    if name == 'hpack-static-names': return len([":authority",":method",":method",":path",":path",":scheme",":scheme"][i%7])+len(["","GET","POST","/","/index.html","http","https"][i%7])
    # Вставка HPACK dynamic entry и accounting
    if name == 'hpack-tables-added': return 32+len("x-id")+len(x)
    # Проверка HTTP/2 Content-Length
    if name == 'http2-body-known': return v
    # Девятибайтовый HTTP/2 frame header
    if name == 'http2-frame-bytes': return 4+sum(v.to_bytes(4,"big"))
    # RST_STREAM frame
    if name == 'http2-controls-frame': return 4+3+8+sum(v.to_bytes(4,"big"))
    # Flow-control списание window
    if name == 'http2-flow-sent': return 65535-v
    # Выделение нечётного client stream id
    if name == 'http2-life-allocated': return 2*v+3
    # Применение INITIAL_WINDOW_SIZE
    if name == 'http2-settings-setting': return 65535+v
    # XOR WebSocket masking
    if name == 'websocket-frame-bytes': return (v%256)^1
    # Ограниченная конкатенация wire chunks
    if name == 'http-wire-bytes': return v%256+5
    # Сборка response и длина тела
    if name == 'http-message-new': return len("body"+x)
    # Валидация proxy URL
    if name == 'http-proxy-policy-url': return len("http://proxy"+x+".test:8080")
    # Сборка CONNECT request
    if name == 'http-proxy-request': return len("example.test:"+str(8000+v))
    # Robots longest-match allow/disallow
    if name == 'http-robots-allowed': return v
    # Извлечение именованного route parameter
    if name == 'http-routing-match': return len(x)
    # Валидация server timeout policy
    if name == 'http-server-policy-handling-timeout': return v
    # Создание SSE event с retry
    if name == 'http-sse-retry': return v
    # Проверка имён index в static policy без файлов
    if name == 'http-static-indexes': return len("index"+x+".html")
    # Проверка HTTP token
    if name == 'http-token-string': return len("X-token-"+x)
    # Состояние наблюдения первых response bytes
    if name == 'http-transport-observation-begin': return v if i%2==0 else 0
    # Парсинг и рендеринг HTTP version
    if name == 'http-version': return ord(str(i%2))
    # UTF-8 WebSocket text message
    if name == 'websocket-text': return sum(("text"+x).encode())
    # Рендеринг списка WebSocket subprotocols
    if name == 'websocket-handshake-string': return len("chat"+x+", binary")
    # Проверка same-origin WebSocket
    if name == 'websocket-origin-allowed': return v
    # XML-RPC сериализация и парсинг integer call
    if name == 'http-xmlrpc-bytes': return v
    # Канонизация IMAP capability
    if name == 'imap-name': return ord("X")
    # Сериализация IMAP CREATE
    if name == 'imap-command-bytes': return sum(('A1 CREATE "box'+x+'"\r\n').encode())
    # Валидация IMAP command tag
    if name == 'imap-command-reading-string': return len("A"+x)
    # Лексинг IMAP списка number/NIL
    if name == 'imap-data-tokens': return 4+len(x)
    # IMAP modified UTF-7 с Unicode и ampersand (RFC 3501)
    if name == 'imap-name-wire-name': return sum(("&"+base64.b64encode("Папка".encode("utf-16-be")).decode().rstrip("=").replace("/",",")+"-&-"+x).encode())
    # Разворачивание диапазона IMAP UID
    if name == 'imap-metadata-uids': return 3
    # Разбор tagged IMAP response
    if name == 'imap-response': return len("A"+x)+2
    # Сортировка, дедупликация и рендеринг IMAP UID set
    if name == 'imap-uid-argument': return len(str(v)+","+str(v+2))
    # Рендеринг IMAP SINCE date
    if name == 'imap-search-key': return len("SINCE "+str(i%28+1)+"-Nov-2026")
    # Набор mail flags и поиск keyword
    if name == 'mail-mailbox-membership': return v
    # Валидация mbox separator без файлов
    if name == 'mbox-string': return len("From user"+x+"@example.test Sat Jan  1 00:00:00 2022")
    # Валидация RFC 5322 Message-ID
    if name == 'mail-message-string': return len("<id"+x+"@example.test>")
    # Разбор POP3 STAT
    if name == 'pop3-size': return v
    # POP3 multiline и dot-unstuffing
    if name == 'pop3-reply': return len(".line"+x+"\r\n")
    # Рендеринг POP3 TOP
    if name == 'pop3-session-bytes': return sum(("TOP "+x+" 3\r\n").encode())
    # Парсинг SMTP SIZE extension
    if name == 'smtp-maximum-size': return v
    # Парсинг SMTP multiline reply
    if name == 'smtp-reply-fed': return len("test\nvalue"+x)
    # SMTP AUTH PLAIN response (RFC 4616)
    if name == 'smtp-session-response': return sum(base64.b64encode(("\0u"+x+"\0pw"+x).encode()))
    # TextReply response с framing
    if name == 'web-reply-response': return len("body"+x)
    # Общая сборка web response
    if name == 'web-internal-reply-response': return 201
    # RFC 9457 problem JSON
    if name == 'web-exception-response': return len(json.dumps({"type":"about:blank","title":"Not Found","status":404,"detail":"missing"+x},separators=(",",":")))
    # Обратный routing и percent encoding
    if name == 'web-request-link': return len("/notes/a%20"+x)
    # Lookup session value без random/clock
    if name == 'web-session-text': return len("u"+x)
    # RFC 9111 age: Age + response delay + residence
    if name == 'http-cache-freshness-age': return v+12
    # First-party cookie context
    if name == 'http-cookie-client-context': return len("https://site"+x+".test/a")
    # Proxy из явно заданного изолированного Environment
    if name == 'http-environment-proxy': return len("http://proxy"+x+".test:8080")
    # Endpoint policy: loopback IPv4 без DNS
    if name == 'http-endpoint-allowed': return v
    # Точная проверка h2 ALPN, включая отказ
    if name == 'http2-tls-version': return v if i%2==0 else 0
    # HTTP/1 ALPN и отсутствие ALPN без TLS socket
    if name == 'http-tls-session-version': return v
    # Выбор разрешённого ALPN с пустым explicit trust store
    if name == 'http-client-protocols-selected': return v
    # Обёртка результата HTTP handler
    if name == 'http-handler-response': return 200+v%50
    # Проверка тела против fixed framing
    if name == 'http1-body-check-known': return len(x)
    # Рендеринг HTTP/1 request head
    if name == 'http1-writing-bytes': return len("GET /a"+x+" HTTP/1.1\r\nHost: example.test\r\n\r\n")
    # HPACK literal header encode/decode
    if name == 'hpack-decoded': return 4+len(x)
    # HTTP/2 request pseudo-fields
    if name == 'http2-fields': return len(":methodGET:schemehttps:authorityexample.test:path/a"+x)
    # HTTP/2 HEADERS с indexed HPACK GET
    if name == 'http2-headers-frames': return 2+1+5+sum(v.to_bytes(4,"big"))+32+130
    # Проверка разрешённого trailer field
    if name == 'http-trailers-check': return len(x)
    # Пустой receiving slot с stream id
    if name == 'http2-receiving-bodies': return v
    # Закрытие sending slot
    if name == 'http2-sending-close': return v
    # Валидация server upgrade policy
    if name == 'websocket-server-protocols': return len("chat"+x)
    # Валидация WebSocket client limits без подключения
    if name == 'websocket-client-limits': return 1024+v
    # Асинхронный HTTP/1 reader на памяти
    if name == 'http1-reading-head': return len("GET /a"+x+" HTTP/1.1\r\nHost: example.test\r\n\r\n")
    # Асинхронный frame reader на памяти
    if name == 'http2-reading-frame': return v+7
    # WebSocket frame reader на памяти
    if name == 'websocket-reading-frame': return v%256
    # Чтение сохранённого prefix и следующего input
    if name == 'websocket-upgrade-bytes': return v%256+7
    # Асинхронный ImapResponseReading на памяти
    if name == 'imap-reading-read': return len("A"+x+" OK done\r\n")
    # Асинхронный SmtpReplyReading на памяти
    if name == 'smtp-reading-read': return len("250 value"+x+"\r\n")
    # Асинхронный PopReplyReading на памяти
    if name == 'pop3-reading-read': return len("+OK value"+x+"\r\n")
    # URL-encoded form parser на памяти
    if name == 'http-form-parameters': return v
    # Сборка binary WebSocket message
    if name == 'websocket-messages-accept': return v%256
    # Gzip body в памяти с проверкой распакованных байтов
    if name == 'http-body-coding-contained': return sum(("body"+x).encode())
    # RFC 7616 SHA-256 Digest response, фиксированный cnonce
    if name == 'http-digest-response-digest': return sum(hashlib.sha256((hashlib.sha256(("user"+x+":realm:pw").encode()).hexdigest()+":nonce:00000001:fixed:auth:"+hashlib.sha256(b"GET:/path").hexdigest()).encode()).hexdigest().encode())
    # Разбор FETCH UID
    if name == 'imap-message-uid': return v
    # Инвалидация selection token без соединения
    if name == 'imap-session-expire': return v
    # Сборка SELECT state из EXISTS/FLAGS/UIDVALIDITY
    if name == 'imap-state-messages': return v
    # SMTP DATA dot-stuffing и terminator
    if name == 'smtp-data-bytes': return sum(("Subject: x\r\n\r\n..line"+x+"\r\n.\r\n").encode())
    # Построение CORS preflight route
    if name == 'web-around-routes': return 1
    # Компиляция web route с prefix
    if name == 'web-routing-routes': return len("/api/a"+x)
    # In-process web dispatch без TCP
    if name == 'web-testing-get': return 202
    # Cookie jar store/select с фиксированным clock
    if name == 'http-cookie-jar-store': return len(x)
    # 100-continue deadline state с фиксированным clock
    if name == 'http1-continue-begun': return v if i%2 else 0
    # Запись chunked body в checksum sink
    if name == 'http1-body-writing-output': return sum((format(len(x),"x")+"\r\n"+x+"\r\n0\r\n\r\n").encode())
    # Запись полного HTTP/1 response в checksum sink
    # The response carries its known length (#942).
    if name == 'http1-message-writing-output': return sum(("HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: "+str(len(x.encode()))+"\r\n\r\n"+x).encode())
    # Чтение fixed body из памяти
    if name == 'http1-body-bytes': return sum(x.encode())
    # Обновление route parameters в request context
    if name == 'http-request-context-parameters': return len(x)
    # Снимок Http1Exchange и response без session I/O
    if name == 'http1-session-response': return 200+v%50
    # Снимок Http2Exchange и response без session I/O
    if name == 'http2-session-response': return 200+v%50
    # Conversation exchange snapshot и освобождение lease
    if name == 'http-conversation-response': return 200+v%50
    # Локальный Retrying decorator без сетевого origin
    if name == 'http-retry-response': return v
    # Локальный Redirecting decorator без сетевого origin
    if name == 'http-redirect-response': return v
    # Локальный Caching decorator без сетевого origin
    if name == 'http-cache-response': return v
    # Условный запрос из ETag cached entry
    if name == 'http-cache-validation-request': return len('"tag'+x+'"')
    # Очередь frame group и byte accounting
    if name == 'http2-output-submit': return 11
    # Подготовка WebSocket output ticket с seeded random
    if name == 'websocket-output-message': return len("data"+x)+1
    # HTTP/2 client preface и SETTINGS ACK на памяти
    if name == 'http2-preface-settings': return v
    # CONNECT exchange с заданным ответом в памяти
    if name == 'http-proxy-tunnel-ready': return sum(("CONNECT site"+x+".test:443 HTTP/1.1\r\nHost: site"+x+".test:443\r\n\r\n").encode())
    # IMAP command exchange с ответом в памяти
    if name == 'imap-exchange-perform': return sum(('O1 CREATE "box'+x+'"\r\n').encode())
    # Диспетчер SETTINGS, ACK и EOF на памяти
    if name == 'http2-input-run': return v+2
    raise ValueError("No independent HTTP oracle: " + name)
# END #244 HTTP / WEB / MAIL

# Work-action additions for #244 (2026-11-22).
WORK244_ORACLES = {
    'web-json-readers': lambda i, v, x: 4*v + len("a"+x) + 8,
    'web-json-optionals': lambda i, v, x: 4*v + len("a"+x) + 2,
    'web-request-values': lambda i, v, x: 4*v + 2*len(x) + 4,
    'web-request-bodies': lambda i, v, x: v+24+5*len(x),
    'web-reply-html': lambda i, v, x: len("<p>"+x+"</p>")+200,
    'web-reply-json': lambda i, v, x: len(x)+6+200,
    'web-reply-created': lambda i, v, x: len(x)+6+201,
    'web-reply-bytes': lambda i, v, x: 201,
    'web-reply-stream': lambda i, v, x: 201,
    'web-reply-empty': lambda i, v, x: 509+len(x),
    'web-reply-fields': lambda i, v, x: 405+v,
    'web-routing-methods': lambda i, v, x: 16+2*len(x),
    'web-testing-methods': lambda i, v, x: 1400,
    'http-body-stream': lambda i, v, x: 3*v+1,
    'http-body-content': lambda i, v, x: 8+2*len(x),
    'http-body-gzip': lambda i, v, x: 2*v+1,
    'http-body-brotli': lambda i, v, x: 2*v+1,
    'http-body-brotli-contained': lambda i, v, x: v,
    'http-cache-entry': lambda i, v, x: 128+2*len("http://example.test/"+x)+1+len("Cache-Control")+len("max-age=600")+4+len("ETag")+len(x)+2+4+100+200+1,
    'http-cache-storage': lambda i, v, x: v+201,
    'http-cache-revalidate': lambda i, v, x: v+1,
    'http-cache-admissions': lambda i, v, x: v+1,
    'http-conditional-tags': lambda i, v, x: 2*len(x)+4+v,
    'http-conditional-ranges': lambda i, v, x: len("bytes="+x+"-"+str(v+2)+", -1")+5+v,
    'http-header-queries': lambda i, v, x: v+2*len(x)+7,
    'http-header-transforms': lambda i, v, x: v+2,
    'http-media-parameters': lambda i, v, x: 21+3*len(x)+v,
    'http-media-scanning': lambda i, v, x: len(x)+2*v+1,
    'http-negotiation-quality': lambda i, v, x: 3*len(("0."+str(v).zfill(3)).rstrip("0"))+16+v,
    'http-negotiation-codings': lambda i, v, x: v+1+11+len(("0."+str(v).zfill(3)).rstrip("0")),
    'http-negotiation-charsets': lambda i, v, x: v+1+13+len(("0."+str(v).zfill(3)).rstrip("0")),
    'http-negotiation-languages': lambda i, v, x: v+1+7+len(("0."+str(v).zfill(3)).rstrip("0")),
    'http-negotiation-media': lambda i, v, x: 2*v+37+2*len(("0."+str(v).zfill(3)).rstrip("0")),
    'http-auth-challenge-fields': lambda i, v, x: v+1+len(x),
    'http-auth-scoped': lambda i, v, x: 3*len("Bearer token"+x)+len("Basic "+base64.b64encode(("u:"+x).encode()).decode())+1,
    'http-cookie-render': lambda i, v, x: 2*(4+len(x)+len("; Path=/; Expires=Thu, 01 Jan 1970 00:00:00 GMT; SameSite=Lax; Secure; HttpOnly"))+len("; Path=/; Expires=Thu, 01 Jan 1970 00:00:00 GMT; SameSite=Lax; Secure; HttpOnly")+2+100+v+len("sid"+x+"=; Path=/; SameSite=Lax; Secure; HttpOnly; Max-Age=0"),
    'http-cookie-read': lambda i, v, x: 2*v+len(x)+2,
    'http-cookie-storage': lambda i, v, x: v+2+2*len(x),
    'http-target-forms': lambda i, v, x: 32+6*len(x),
    'http-target-kind-errors': lambda i, v, x: v+10,
    'web-session-memory': lambda i, v, x: v,
    'web-session-lifecycle': lambda i, v, x: len(x)+43+43+200+len("sid=; Path=/; SameSite=Lax; Secure; HttpOnly; Max-Age=0"),
    'web-around-auth': lambda i, v, x: 400+v,
    'imap-uid-scope': lambda i, v, x: v+1,
    'imap-search-operators': lambda i, v, x: len("OR ((ALL SINCE "+str(v%28+1)+"-Jan-2020)) (NOT (UNSEEN))")+9+len("SINCE "+str(v%28+1)+"-Jan-2020"),
    'imap-data-values': lambda i, v, x: v+7+3*len(x),
    'imap-response-decoder': lambda i, v, x: 18+3*len(x),
    'imap-response-progress': lambda i, v, x: len("* 1 FETCH (BODY[TEXT] {"+str(len(x))+"}\r\n"+x+")\r\n")+len(x),
    'imap-command-input': lambda i, v, x: 12+2*len(x),
    'imap-capabilities-construct': lambda i, v, x: len("* CAPABILITY IMAP4rev1 X-"+x+"\r\n")+4,
    'imap-status-metadata': lambda i, v, x: 2*v+15+len(x),
    'pop3-reply-decoder': lambda i, v, x: 10+2*len(x),
    'smtp-reply-tail': lambda i, v, x: 259+len(x),
    'mail-reply-snapshots': lambda i, v, x: 23+3*len(x),
    'imap-name-decoded': lambda i, v, x: 4+len(x),
    'pop3-listing': lambda i, v, x: v+4+len(x),
    'imap-fetch-fields': lambda i, v, x: v+len(x)+5,
    'http2-flow-windows': lambda i, v, x: 4096-3*v,
    'http2-flow-payloads': lambda i, v, x: 2*v+3,
    'http2-stream-life': lambda i, v, x: 4*v+1,
    'http2-body-reading': lambda i, v, x: 2*v+1025,
    'http2-body-failure': lambda i, v, x: v+1,
    'http2-output-delivery': lambda i, v, x: v+5,
    'http2-output-release': lambda i, v, x: v+10,
    'http2-receive-ledger': lambda i, v, x: v+65539,
    'http2-control-frames': lambda i, v, x: 4*v,
    'http2-frame-validation': lambda i, v, x: 12+v+len(x),
    'hpack-primitives-parse': lambda i, v, x: 2*v+48+v%2,
    'hpack-table-lookup': lambda i, v, x: 42+2*len(x),
    'http2-field-sections': lambda i, v, x: 2*v+5+len(x),
    'http2-send-reservations': lambda i, v, x: 131070-v,
    'http2-send-data': lambda i, v, x: v+4,
    'http1-framing-fields': lambda i, v, x: 4*v+3,
    'http1-head-objects': lambda i, v, x: 13+len(x)+v,
    'http1-trailer-parse': lambda i, v, x: v+1,
    'http1-body-remainder': lambda i, v, x: ord(x[0])+4+v,
    'http1-wire-plans': lambda i, v, x: len("HTTP/1.1 204 No Content\r\n\r\n")+sum(("GET /"+x+" HTTP/1.1\r\nHost: example.test\r\n\r\n"+"HTTP/1.1 204 No Content\r\n\r\n"*2).encode()),
    'http1-session-memory': lambda i, v, x: 200+sum(("GET /"+x+" HTTP/1.1\r\nHost: example.test\r\n\r\n").encode()),
    'http-cache-directives': lambda i, v, x: v+2,
    'http-policy-protocols': lambda i, v, x: v+65,
    'http-endpoint-selection': lambda i, v, x: 8081+v,
    'http-exchange-body': lambda i, v, x: 13+2*len(x)+v+len("http://example.test/"+x),
    'http-form-read': lambda i, v, x: 2*len(x),
    'http-robots-fetch': lambda i, v, x: 2*v+1,
    'http-exchange-decorators': lambda i, v, x: 200+len(x),
    'http-digest-fields': lambda i, v, x: 211+3*len(x),
    'http-digest-exchange': lambda i, v, x: len(x),
    'http-sse-read': lambda i, v, x: 2*len(x)+4,
    'http-sse-body': lambda i, v, x: 211+len(x),
    'http-xmlrpc-containers': lambda i, v, x: 9*v+4076,
    'http-xmlrpc-exchange': lambda i, v, x: v,
    'pop3-session-list': lambda i, v, x: 4*v+5,
    'pop3-session-mail': lambda i, v, x: 3*19+4*len(x),
    'smtp-session-submit': lambda i, v, x: v,
    'smtp-client-settings': lambda i, v, x: v+len(x),
    'imap-session-select': lambda i, v, x: 3*v+4097,
    'imap-session-examine': lambda i, v, x: v+1,
    'imap-session-mailboxes': lambda i, v, x: 7*(3+len(x)),
    'imap-session-search': lambda i, v, x: v+10,
    'imap-session-copy': lambda i, v, x: v+3,
    'imap-session-append': lambda i, v, x: 2*v+23+len(x),
    'imap-session-events': lambda i, v, x: 2*v+4,
    'imap-idle-cancellation': lambda i, v, x: v+3,
    'imap-copy-mapping': lambda i, v, x: 2*v+1,
    'mail-message-values': lambda i, v, x: len("Subject: "+x+"\r\n\r\nhello\r\n")+len("Subject: "+x+"\nBcc: hidden@example.test\n\nhello\n")+len("Wed, 01 Jan 2020 00:00:00 +0000"),
    'mail-flags-stored': lambda i, v, x: v+7,
    'mail-mbox-roundtrip': lambda i, v, x: len(x)+len("From a@example.test fixed\nSubject: "+x+"\n\n>From sender\n\n")+len("archive"+x),
    'mail-maildir-files': lambda i, v, x: 2+len(x),
    'http-cookie-files': lambda i, v, x: 1,
    'http-static-files': lambda i, v, x: 216,
    'websocket-frame-headers': lambda i, v, x: 4001+3*v,
    'websocket-output-frames': lambda i, v, x: 3*v+400,
    'websocket-output-tickets': lambda i, v, x: 2*v+263,
    'websocket-session-memory': lambda i, v, x: 3*v+sum(map(ord,x))*2+4*len(x)+sum(map(ord,'{"n":}'))+6+130*2+129*2+137+1+3,
    'websocket-handshake-pair': lambda i, v, x: 102+len(x),
    'websocket-server-memory': lambda i, v, x: v,
    'websocket-client-upgrade': lambda i, v, x: len(x),
    'http-routing-dispatch': lambda i, v, x: v+405,
    'web-routing-dispatch': lambda i, v, x: 400+len('127.0.0.1 "GET http://example.test/'+x+'" 200 0.000 ms\n')+10,
    'http1-request-progress': lambda i, v, x: 2*v+2*sum(("GET /"+x+" HTTP/1.1\r\nHost: example.test\r\n\r\n").encode()),
    'http1-direct-exchange': lambda i, v, x: 200+v,
    'http-connection-policies': lambda i, v, x: len("http://example.test/"+x)+6+4*((len(x)+5)//3)+1,
    'http-acquisition-cleanup': lambda i, v, x: v,
    'xmlrpc-float-print': lambda i, v, x: v,
    'http2-header-blocks': lambda i, v, x: len(x)+1+sum([1,5,1,5+len(x),32,0,1,120,len(x)])+sum(map(ord,x)),
    'http2-server-preface': lambda i, v, x: 2*v,
    'http2-session-exchange': lambda i, v, x: 200+v,
    'http2-conversation-exchange': lambda i, v, x: 200+v,
    'http2-server-lifecycle': lambda i, v, x: v,
}
