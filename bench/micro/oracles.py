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
    if name in LAST244_ORACLES:
        return LAST244_ORACLES[name](i)
    if name in TAIL244_ORACLES:
        return TAIL244_ORACLES[name](i, i % 23 + 1, str(i % 23 + 1))
    if name in WORK244_ORACLES:
        v = i % 251 + 1
        return WORK244_ORACLES[name](i, v, str(v))
    if name in HTTP244_PROGRAMS:
        return http244_contribution(name, i)
    if name in WORK244_PROGRAMS:
        return work244_contribution(name, i)
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
    if name=='map-int-literal':return (i+3)%7
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

# BEGIN #244 WORK (independent, hand-written formulas)


def sha512_256_hex(message):
    """FIPS 180-4 SHA-512/256: SHA-512 with its own initial values, truncated to 256 bits."""
    mask = (1 << 64) - 1
    primes = []
    c = 2
    while len(primes) < 80:
        if all(c % p for p in primes if p * p <= c):
            primes.append(c)
        c += 1
    def root(p, e):
        # The first 64 fractional bits of p ** (1/e), exactly.
        lo, hi = 0, 1 << 80
        n = p << (64 * e)
        while lo < hi:
            mid = (lo + hi + 1) // 2
            if mid ** e <= n: lo = mid
            else: hi = mid - 1
        return lo & mask
    k = [root(p, 3) for p in primes]
    h = [0x22312194FC2BF72C, 0x9F555FA3C84C64C2, 0x2393B86B6F53B151, 0x963877195940EABD,
         0x96283EE2A88EFFE3, 0xBE5E1E2553863992, 0x2B0199FC2C85B8AA, 0x0EB72DDC81C52CA2]
    rot = lambda x, n: ((x >> n) | (x << (64 - n))) & mask
    data = message + b"\x80" + b"\x00" * ((111 - len(message)) % 128) + (8 * len(message)).to_bytes(16, "big")
    for start in range(0, len(data), 128):
        w = [int.from_bytes(data[start + 8 * t:start + 8 * t + 8], "big") for t in range(16)]
        for t in range(16, 80):
            s0 = rot(w[t - 15], 1) ^ rot(w[t - 15], 8) ^ (w[t - 15] >> 7)
            s1 = rot(w[t - 2], 19) ^ rot(w[t - 2], 61) ^ (w[t - 2] >> 6)
            w.append((w[t - 16] + s0 + w[t - 7] + s1) & mask)
        a, b, c, d, e, f, g, hh = h
        for t in range(80):
            t1 = (hh + (rot(e, 14) ^ rot(e, 18) ^ rot(e, 41)) + ((e & f) ^ (~e & g)) + k[t] + w[t]) & mask
            t2 = ((rot(a, 28) ^ rot(a, 34) ^ rot(a, 39)) + ((a & b) ^ (a & c) ^ (b & c))) & mask
            hh, g, f, e, d, c, b, a = g, f, e, (d + t1) & mask, c, b, a, (t1 + t2) & mask
        h = [(x + y) & mask for x, y in zip(h, [a, b, c, d, e, f, g, hh])]
    return b"".join(x.to_bytes(8, "big") for x in h)[:32].hex()


def pem_der(text):
    """The DER bytes of a PEM block (RFC 7468): base64 between the boundary lines."""
    lines = [l for l in text.split("\n") if l and not l.startswith("-----")]
    return base64.b64decode("".join(lines))


ED_P = 2 ** 255 - 19
ED_L = 2 ** 252 + 27742317777372353535851937790883648493
ED_D = -121665 * pow(121666, ED_P - 2, ED_P) % ED_P


def _ed_add(a, b):
    p = ED_P
    x1, y1, z1, t1 = a
    x2, y2, z2, t2 = b
    e1 = (y1 - x1) * (y2 - x2) % p
    e2 = (y1 + x1) * (y2 + x2) % p
    c = t1 * 2 * ED_D * t2 % p
    d = z1 * 2 * z2 % p
    e, f, g, h = e2 - e1, d - c, d + c, e2 + e1
    return (e * f % p, g * h % p, f * g % p, e * h % p)


def _ed_mul(s, point):
    q = (0, 1, 1, 0)
    while s > 0:
        if s & 1:
            q = _ed_add(q, point)
        point = _ed_add(point, point)
        s >>= 1
    return q


def _ed_encode(point):
    zi = pow(point[2], ED_P - 2, ED_P)
    x, y = point[0] * zi % ED_P, point[1] * zi % ED_P
    return (y | ((x & 1) << 255)).to_bytes(32, "little")


def _ed_base():
    p = ED_P
    y = 4 * pow(5, p - 2, p) % p
    x2 = (y * y - 1) * pow(ED_D * y * y + 1, p - 2, p) % p
    x = pow(x2, (p + 3) // 8, p)
    if (x * x - x2) % p:
        x = x * pow(2, (p - 1) // 4, p) % p
    if x & 1:
        x = p - x
    return (x, y, 1, x * y % p)


@lru_cache(maxsize=64)
def ed25519_public(seed):
    """RFC 8032 section 5.1.5."""
    h = hashlib.sha512(seed).digest()
    a = (int.from_bytes(h[:32], "little") & ((1 << 254) - 8)) | (1 << 254)
    return _ed_encode(_ed_mul(a, _ed_base()))


@lru_cache(maxsize=64)
def ed25519_sign(seed, message):
    """RFC 8032 section 5.1.6."""
    h = hashlib.sha512(seed).digest()
    a = (int.from_bytes(h[:32], "little") & ((1 << 254) - 8)) | (1 << 254)
    public = _ed_encode(_ed_mul(a, _ed_base()))
    r = int.from_bytes(hashlib.sha512(h[32:] + message).digest(), "little") % ED_L
    big_r = _ed_encode(_ed_mul(r, _ed_base()))
    k = int.from_bytes(hashlib.sha512(big_r + public + message).digest(), "little") % ED_L
    return big_r + ((r + k * a) % ED_L).to_bytes(32, "little")


@lru_cache(maxsize=64)
def x25519(scalar, u):
    """RFC 7748 section 5: the Montgomery ladder for any u."""
    p = 2 ** 255 - 19
    k = bytearray(scalar)
    k[0] &= 248; k[31] &= 127; k[31] |= 64
    k = int.from_bytes(k, "little")
    x1 = int.from_bytes(u, "little") & ((1 << 255) - 1)
    x2, z2, x3, z3, swap = 1, 0, x1, 1, 0
    for t in reversed(range(255)):
        bit = (k >> t) & 1
        swap ^= bit
        if swap:
            x2, x3, z2, z3 = x3, x2, z3, z2
        swap = bit
        a, b = (x2 + z2) % p, (x2 - z2) % p
        aa, bb = a * a % p, b * b % p
        e = (aa - bb) % p
        c, d = (x3 + z3) % p, (x3 - z3) % p
        da, cb = d * a % p, c * b % p
        x3, z3 = (da + cb) ** 2 % p, x1 * (da - cb) ** 2 % p
        x2, z2 = aa * bb % p, e * (aa + 121665 * e) % p
    if swap:
        x2, x3, z2, z3 = x3, x2, z3, z2
    return (x2 * pow(z2, p - 2, p) % p).to_bytes(32, "little")


def _w244_collections_views_bounds(i):
    """collections-views-bounds"""
    import itertools
    v = i % 5 + 3
    xs = list(range(1, v + 1))
    return (sum(xs[2:]) + sum(itertools.dropwhile(lambda x: x < 3, xs)) + sum(xs[1:v:2]) + sum(xs[:2])
            + sum(itertools.takewhile(lambda x: x < 3, xs)))


def _w244_collections_views_transform(i):
    """collections-views-transform"""
    import itertools
    v = i % 5 + 3
    xs = list(range(1, v + 1))
    return (sum(itertools.accumulate(xs, initial=0)) + sum(x for x in xs if x % 2 == 0) + sum(xs) + 10
            + sum(x * x for x in xs) + (v - 1) + (1 + sum(xs)))


def _w244_collections_views_reversed(i):
    """collections-views-reversed"""
    v = i % 5 + 3
    xs = list(range(1, v + 1))
    r = xs[::-1]
    return sum(r) + r[0] + r[-1] + r[1] + 2 * sum(xs) + v


def _w244_collections_combinatorics(i):
    """collections-combinatorics"""
    import itertools
    v = i % 5 + 3
    xs = list(range(1, v + 1))
    return (len(list(itertools.product([1, 2], xs))) + len(list(itertools.combinations_with_replacement(xs, 2)))
            + len(list(itertools.permutations([1, 2, 3, 4][:i % 2 + 3]))))


def _w244_collections_set_union(i):
    """collections-set-union"""
    v = i % 5 + 3
    xs = [v, 1, v, 4, 2, 4]
    ys = [2, v, 9]
    left = set(xs); right = set(ys)
    b = lambda c: 1 if c else 0

    return (sum(left) + len(left) + b(4 in left) + sum(left | right) + b(9 in (left | right))
            + sum(left & right) + len(left & right) + b(2 in (left & right)))


def _w244_collections_set_difference(i):
    """collections-set-difference"""
    v = i % 5 + 3
    xs = [v, 1, v, 4, 2, 4]
    ys = [2, v, 9]
    left = set(xs); right = set(ys)
    b = lambda c: 1 if c else 0

    return (sum(left - right) + len(left - right) + b(2 in (left - right))
            + sum(left ^ right) + len(left ^ right) + b(9 in (left ^ right)))


def _w244_collections_set_edits(i):
    """collections-set-edits"""
    v = i % 5 + 3
    xs = [v, 1, v, 4, 2, 4]
    ys = [2, v, 9]
    left = set(xs); right = set(ys)
    b = lambda c: 1 if c else 0

    return sum(left | {7}) + len(left | {7}) + sum(left - {1}) + len(left - {1}) + sum(left) + len(left)


def _w244_collections_sorted(i):
    """collections-sorted"""
    v = i % 5 + 3
    xs = [v, 1, v, 4, 2, 4]
    up = sorted(xs); down = sorted(xs, key=lambda x: -x)
    return sum(xs) + up[1] + up[-1] + len(xs) + sum(xs) + down[0] + down[1] + down[-1]


def _w244_collections_ordering_search(i):
    """collections-ordering-search"""
    import bisect
    v = i % 5 + 3
    s = [1, 2, 4, 4, 8]
    return 3 + (1 if v < 4 else 2) + [1, 2, 4, v + 10].index(4) + bisect.bisect_left(s, v) + bisect.bisect_right(s, v)


def _w244_collections_heap_deque(i):
    """collections-heap-deque"""
    import heapq
    v = i % 5 + 3
    xs = [v, 1, v, 4, 2, 4]
    h = xs + [0]
    heapq.heapify(h)
    heapq.heappop(h); heapq.heappop(h)
    return len(h) + h[0] + sum(xs) + xs[2] + len(xs[1:])


def _w244_collections_frequencies(i):
    """collections-frequencies"""
    from collections import Counter
    v = i % 5 + 3
    xs = [v, 1, v, 4, 2, 4]
    c = Counter(xs)
    w = sum(k * a for k, a in c.items())
    return w + w + len(c) + sum(c.values()) + 2


def _w244_digest_blake2b(i):
    """digest-blake2b"""
    x = str(i % 251 + 1).encode()
    m = b"a" + x + b"b"
    key = b"key" + x
    first = lambda h: int(h[0], 16)

    return first(hashlib.blake2b(m, digest_size=32).hexdigest()) + first(hashlib.blake2b(m, key=key).hexdigest())


def _w244_digest_blake2s(i):
    """digest-blake2s"""
    x = str(i % 251 + 1).encode()
    m = b"a" + x + b"b"
    key = b"key" + x
    first = lambda h: int(h[0], 16)

    return first(hashlib.blake2s(m, digest_size=32).hexdigest()) + first(hashlib.blake2s(m, key=key).hexdigest())


def _w244_digest_legacy_blocks(i):
    """digest-legacy-blocks"""
    x = str(i % 251 + 1).encode()
    m = b"a" + x + b"b"
    key = b"key" + x
    first = lambda h: int(h[0], 16)

    size = i % 100 + 1
    # Merkle-Damgard padding: 0x80, zeros, an 8-byte length; blocks of 64 bytes.
    blocks = ((2 * size + 9 + 63) // 64) % 256
    return first(hashlib.md5(m).hexdigest()) + first(hashlib.sha1(m).hexdigest()) + blocks


def _w244_digest_sha2_small(i):
    """digest-sha2-small"""
    x = str(i % 251 + 1).encode()
    m = b"a" + x + b"b"
    key = b"key" + x
    first = lambda h: int(h[0], 16)

    return first(hashlib.sha224(m).hexdigest()) + first(hashlib.sha256(m).hexdigest()) + first(hashlib.sha224(m[:-1]).hexdigest())


def _w244_digest_sha2_wide(i):
    """digest-sha2-wide"""
    x = str(i % 251 + 1).encode()
    m = b"a" + x + b"b"
    key = b"key" + x
    first = lambda h: int(h[0], 16)

    return (first(hashlib.sha384(m).hexdigest()) + first(hashlib.sha512(m).hexdigest()) + first(sha512_256_hex(m)))


def _w244_digest_sha3_small(i):
    """digest-sha3-small"""
    x = str(i % 251 + 1).encode()
    m = b"a" + x + b"b"
    key = b"key" + x
    first = lambda h: int(h[0], 16)

    return first(hashlib.sha3_224(m).hexdigest()) + first(hashlib.sha3_256(m).hexdigest())


def _w244_digest_sha3_wide(i):
    """digest-sha3-wide"""
    x = str(i % 251 + 1).encode()
    m = b"a" + x + b"b"
    key = b"key" + x
    first = lambda h: int(h[0], 16)

    return first(hashlib.sha3_384(m).hexdigest()) + first(hashlib.sha3_512(m).hexdigest())


def _w244_digest_shake(i):
    """digest-shake"""
    x = str(i % 251 + 1).encode()
    m = b"a" + x + b"b"
    key = b"key" + x
    first = lambda h: int(h[0], 16)

    return first(hashlib.shake_128(m).hexdigest(16)) + first(hashlib.shake_256(m).hexdigest(16))


def _w244_digest_read(i):
    """digest-read"""
    data = bytes(k % 256 for k in range(i % 100 + 1))
    return hashlib.sha256(b"p" + data).digest()[0] + hashlib.sha256(data).digest()[0] + hashlib.sha256(b"q" + data).digest()[0]


def _w244_regex_pattern_queries(i):
    """regex-pattern-queries"""
    v = i % 9 + 1
    x = str(v)
    text = "k" + x + "=" + x + x + " name=n" + x
    pair = re.compile(r"(?P<key>[a-z]+)(?P<id>\d)?=(?P<num>\d+)")
    found = list(pair.finditer(text))
    m = found[0]

    return 1 + len(found) + pair.groups + len(pair.groupindex) + (len(found) + 1) + len(pair.sub("#", text))


def _w244_regex_observation(i):
    """regex-observation"""
    v = i % 9 + 1
    x = str(v)
    text = "k" + x + "=" + x + x + " name=n" + x
    pair = re.compile(r"(?P<key>[a-z]+)(?P<id>\d)?=(?P<num>\d+)")
    found = list(pair.finditer(text))
    m = found[0]

    p = pair.match(text)
    return 1 + len(m.group(0)) + 1 + p.end() + int(m.group("id"))


def _w244_regex_match_groups(i):
    """regex-match-groups"""
    v = i % 9 + 1
    x = str(v)
    text = "k" + x + "=" + x + x + " name=n" + x
    pair = re.compile(r"(?P<key>[a-z]+)(?P<id>\d)?=(?P<num>\d+)")
    found = list(pair.finditer(text))
    m = found[0]

    return (len(m.group(0)) + len(m.group("num")) + int(m.group("num")) + m.end() + pair.groups + m.start("key") + m.end(1)
            + int(m.group("id")))


def _w244_regex_first_match(i):
    """regex-first-match"""
    v = i % 9 + 1
    x = str(v)
    text = "=" + x + " k" + x + "=" + x + x + " name=n" + x
    pair = re.compile(r"(?P<key>[a-z]+)(?P<id>\d)?=(?P<num>\d+)")
    found = list(pair.finditer(text))
    m = found[0]

    return pair.groups + m.end("num") + int(m.group("num")) + int(m.group("id")) + m.end()


def _w244_regex_prefix_match(i):
    """regex-prefix-match"""
    v = i % 9 + 1
    x = str(v)
    text = "k" + x + "=" + x + x + " name=n" + x
    pair = re.compile(r"(?P<key>[a-z]+)(?P<id>\d)?=(?P<num>\d+)")
    found = list(pair.finditer(text))
    m = found[0]

    p = pair.match(text)
    return (p.start(3) + pair.groups + p.end("key") + int(p.group("num")) + int(p.group("id")) + p.end() + len(p.group(0))
            + len(p.group("num")))


def _w244_regex_entire_match(i):
    """regex-entire-match"""
    v = i % 9 + 1
    x = str(v)
    text = "k" + x + "=" + x + x + " name=n" + x
    pair = re.compile(r"(?P<key>[a-z]+)(?P<id>\d)?=(?P<num>\d+)")
    found = list(pair.finditer(text))
    m = found[0]
    whole = re.fullmatch(r"(?P<key>k)(?P<id>\d)=(?P<num>\d+) name=(?P<rest>\w+)", text)

    return (whole.start(4) + 4 + whole.end("rest") + int(whole.group("num")) + int(whole.group("id")) + whole.end()
            + len(whole.group(0)) + len(whole.group("rest")))


def _w244_regex_replace_forms(i):
    """regex-replace-forms"""
    v = i % 9 + 1
    x = str(v)
    text = "k" + x + "=" + x + x + " name=n" + x
    pair = re.compile(r"(?P<key>[a-z]+)(?P<id>\d)?=(?P<num>\d+)")
    found = list(pair.finditer(text))
    m = found[0]

    return len(m.expand(r"\g<num>-\1")) + len(found) + 1 + len(re.findall(r"\d", text))


def _w244_toml_table_scalars(i):
    """toml-table-scalars"""
    v = i % 9 + 1
    x = str(v)
    # port = v, ratio = v.5 (truncated), on, two tags, ports [1, v], host "h<v>", the same value twice.
    return v + int(float(x + ".5")) + 1 + 2 + v + len("h" + x) + 1


def _w244_toml_table_lists(i):
    """toml-table-lists"""
    v = i % 9 + 1
    return 2 + 3 + v + 2 + 2 + 2 + 1


def _w244_toml_temporal(i):
    """toml-temporal"""
    v = i % 9 + 1
    when = datetime.datetime(2026, v, 1, 7, 30, tzinfo=datetime.timezone(datetime.timedelta(hours=5, minutes=30)))
    day = datetime.date(2026, 1, v)
    at = datetime.time(7, 30 + v)
    local = datetime.datetime(2026, 1, 1, 0, 0, v)
    return len(when.isoformat()) + len(day.isoformat()) + len(at.isoformat()) + len(local.isoformat())


def _w244_toml_maybe_scalars(i):
    """toml-maybe-scalars"""
    v = i % 9 + 1
    return v + 0 + 1 + v + 1 + 2 + len("svc" + str(v)) + 1 + len("+05:30")


def _w244_toml_values_built(i):
    """toml-values-built"""
    v = i % 9 + 1
    return 2 + v + len(datetime.date(2000 + v, v, v).isoformat()) + len("-inf" if v % 2 == 0 else "inf")


def _w244_json_node_reads(i):
    """json-node-reads"""
    v = i % 9 + 1
    doc = {'id': v, 'name': 'n%d' % v, 'ok': True, 'price': float('%d.5' % v), 'none': None, 'tags': ['a', 'b'], 'inner': {'k': v}}
    price = len(repr(doc['price']))

    return v + len(doc['name']) + 1 + price + 2 + v + len(doc) + len(doc)


def _w244_json_node_shape(i):
    """json-node-shape"""
    v = i % 9 + 1
    doc = {'id': v, 'name': 'n%d' % v, 'ok': True, 'price': float('%d.5' % v), 'none': None, 'tags': ['a', 'b'], 'inner': {'k': v}}
    price = len(repr(doc['price']))

    return len(doc) + 2 + 1 + len('array') + len({**doc, 'id': 0, 'extra': 1}) + len(json.dumps(doc, indent=2))


def _w244_json_maybe_reader(i):
    """json-maybe-reader"""
    v = i % 9 + 1
    doc = {'id': v, 'name': 'n%d' % v, 'ok': True, 'price': float('%d.5' % v), 'none': None, 'tags': ['a', 'b'], 'inner': {'k': v}}
    price = len(repr(doc['price']))

    return v + v + 1 + 2 + v + 1 + price + 2 * v


def _w244_json_maybe_presence(i):
    """json-maybe-presence"""
    v = i % 9 + 1
    doc = {'id': v, 'name': 'n%d' % v, 'ok': True, 'price': float('%d.5' % v), 'none': None, 'tags': ['a', 'b'], 'inner': {'k': v}}
    price = len(repr(doc['price']))

    return 1 + 0 + len(doc['name']) + 1 + 2 + 3 + 1


def _w244_json_presence_reader(i):
    """json-presence-reader"""
    v = i % 9 + 1
    doc = {'id': v, 'name': 'n%d' % v, 'ok': True, 'price': float('%d.5' % v), 'none': None, 'tags': ['a', 'b'], 'inner': {'k': v}}
    price = len(repr(doc['price']))

    # RFC 6901: "~" is written "~0" and "/" is written "~1" inside a token.
    return 2 + 1 + 1 + len(doc['name']) + len('/a/%d' % v) + len(('a/b~%d' % v).replace('~', '~0').replace('/', '~1'))


def _w244_json_streaming(i):
    """json-streaming"""
    v = i % 9 + 1
    return v + 2 + len(json.dumps({'k': v, 't': [True]}, separators=(',', ':')) + '\n')


def _w244_plist_xml_source(i):
    """plist-xml-source"""
    v = i % 9 + 1
    x = str(v)
    text = ('<?xml version="1.0"?>\n<plist version="1.0"><dict>'
            '<key>name</key><string>App &amp; ' + x + '</string><key>on</key><true/><key>ratio</key><real>' + x + '.5</real>'
            '<key>blob</key><data>AQID</data><key>when</key><date>2026-01-0' + x + 'T00:00:00Z</date>'
            '<key>tags</key><array><string>a</string><string>b' + x + '</string></array>'
            '<key>ports</key><array><integer>1</integer><integer>' + x + '</integer></array>'
            '<key>items</key><array><dict><key>id</key><integer>' + x + '</integer></dict></array>'
            '<key>inner</key><dict><key>k</key><string>v' + x + '</string></dict></dict></plist>')
    name_text = html.unescape('App &amp; ' + x)

    plist_open = text.index('<plist')
    return (text.index('<dict>') + text.index('<', 56) + (text.index('>', plist_open) + 1) + plist_open
            + len('ab' + x) + len(html.unescape('a&amp;b' + x)) + 3 + len('bad'))


def _w244_plist_xml_text(i):
    """plist-xml-text"""
    v = i % 9 + 1
    x = str(v)
    return len(' '.join((' a \n b' + x + ' ').split())) + len((' a' + x + ' ').strip()) + 1 + 1 + 2 + v


def _w244_plist_dictionary_reads(i):
    """plist-dictionary-reads"""
    v = i % 9 + 1
    x = str(v)
    text = ('<?xml version="1.0"?>\n<plist version="1.0"><dict>'
            '<key>name</key><string>App &amp; ' + x + '</string><key>on</key><true/><key>ratio</key><real>' + x + '.5</real>'
            '<key>blob</key><data>AQID</data><key>when</key><date>2026-01-0' + x + 'T00:00:00Z</date>'
            '<key>tags</key><array><string>a</string><string>b' + x + '</string></array>'
            '<key>ports</key><array><integer>1</integer><integer>' + x + '</integer></array>'
            '<key>items</key><array><dict><key>id</key><integer>' + x + '</integer></dict></array>'
            '<key>inner</key><dict><key>k</key><string>v' + x + '</string></dict></dict></plist>')
    name_text = html.unescape('App &amp; ' + x)

    return len(name_text) + 1 + v + len(base64.b64decode('AQID')) + 1 + v + v + len('v' + x)


def _w244_plist_dictionary_maybe(i):
    """plist-dictionary-maybe"""
    v = i % 9 + 1
    x = str(v)
    text = ('<?xml version="1.0"?>\n<plist version="1.0"><dict>'
            '<key>name</key><string>App &amp; ' + x + '</string><key>on</key><true/><key>ratio</key><real>' + x + '.5</real>'
            '<key>blob</key><data>AQID</data><key>when</key><date>2026-01-0' + x + 'T00:00:00Z</date>'
            '<key>tags</key><array><string>a</string><string>b' + x + '</string></array>'
            '<key>ports</key><array><integer>1</integer><integer>' + x + '</integer></array>'
            '<key>items</key><array><dict><key>id</key><integer>' + x + '</integer></dict></array>'
            '<key>inner</key><dict><key>k</key><string>v' + x + '</string></dict></dict></plist>')
    name_text = html.unescape('App &amp; ' + x)

    return 2 + 1 + 1 + len(base64.b64decode('AQID')) + 1 + v


def _w244_plist_maybe_more(i):
    """plist-maybe-more"""
    v = i % 9 + 1
    x = str(v)
    text = ('<?xml version="1.0"?>\n<plist version="1.0"><dict>'
            '<key>name</key><string>App &amp; ' + x + '</string><key>on</key><true/><key>ratio</key><real>' + x + '.5</real>'
            '<key>blob</key><data>AQID</data><key>when</key><date>2026-01-0' + x + 'T00:00:00Z</date>'
            '<key>tags</key><array><string>a</string><string>b' + x + '</string></array>'
            '<key>ports</key><array><integer>1</integer><integer>' + x + '</integer></array>'
            '<key>items</key><array><dict><key>id</key><integer>' + x + '</integer></dict></array>'
            '<key>inner</key><dict><key>k</key><string>v' + x + '</string></dict></dict></plist>')
    name_text = html.unescape('App &amp; ' + x)

    return 1 + v + len(name_text) + 2


def _w244_xml_events(i):
    """xml-events"""
    v = i % 9 + 1
    x = str(v)
    doc = ('<?xml version="1.0" encoding="UTF-8"?><root xmlns=""><item xmlns="" id="' + x + '&lt;&amp;">t' + x
           + '<!--c' + x + '--></item><?pi d' + x + '?>tail</root>')
    # Events: root, item, text, comment, end item, pi, text, end root; kind 0 is a start tag.
    return 8 + 0 + len('root') + len('t' + x) + doc.index('<item') + (1 if i % 2 == 0 else 0) + (0 if i % 3 == 0 else 2)


def _w244_xml_node_build(i):
    """xml-node-build"""
    v = i % 9 + 1
    x = str(v)
    return (len('k="' + html.escape('"' + x) + '"') + len(html.escape('a<b' + x, quote=False)) + len('<?go ' + x + '?>')
            + len('<!--' + x + '-->'))


def _w244_xml_element(i):
    """xml-element"""
    v = i % 9 + 1
    x = str(v)
    item = '<item xmlns="" id="' + x + '&lt;&amp;">t' + x + '<!--c' + x + '--></item>'
    root = '<root xmlns="">' + item + '<?pi d' + x + '?>tail</root>'
    doc = '<?xml version="1.0" encoding="UTF-8"?>' + root
    return len(doc) + len(root) + len(item) + 2 + 3 + 1 + len('t' + x) + len('t' + x)


def _w244_uuid_named(i):
    """uuid-named"""
    v = i % 9 + 1
    x = str(v)
    dns = uuid.NAMESPACE_DNS
    rank = lambda a, b: 1 if a.bytes < b.bytes else 2 if a.bytes == b.bytes else 3
    u8 = uuid.UUID(int=(v << 80) | (8 << 76) | (v << 64) | (2 << 62) | v)

    u3 = uuid.uuid3(dns, "n" + x); u5 = uuid.uuid5(dns, "n" + x)
    return u3.bytes[0] + 1 + int(str(u5)[0], 16) + rank(u3, u5) + rank(u5, u3)


def _w244_uuid_custom(i):
    """uuid-custom"""
    v = i % 9 + 1
    x = str(v)
    dns = uuid.NAMESPACE_DNS
    rank = lambda a, b: 1 if a.bytes < b.bytes else 2 if a.bytes == b.bytes else 3
    u8 = uuid.UUID(int=(v << 80) | (8 << 76) | (v << 64) | (2 << 62) | v)

    return u8.bytes[15] + 36 + 2 + rank(u8, dns) + 1 + int(str(u8)[7], 16)


def _w244_uuid_text(i):
    """uuid-text"""
    v = i % 9 + 1
    x = str(v)
    dns = uuid.NAMESPACE_DNS
    rank = lambda a, b: 1 if a.bytes < b.bytes else 2 if a.bytes == b.bytes else 3
    u8 = uuid.UUID(int=(v << 80) | (8 << 76) | (v << 64) | (2 << 62) | v)

    text = uuid.UUID("6ba7b81" + x + "-9dad-11d1-80b4-00c04fd430c8")
    return u8.version + (1 if u8.variant == uuid.RFC_4122 else 0) + rank(dns, u8) + 1 + rank(text, dns)


def _w244_url_parts_read(i):
    """url-parts-read"""
    v = i % 9 + 1
    x = str(v)
    origin = "https://user@example.org:8443"
    # RFC 3986 section 6.2.2: lower-case host, remove dot segments.
    return (len("user@example.org:8443") + len("example.org") + len(str(ipaddress.ip_address("10.0.0." + x))) + 1
            + len(origin + posixpath.normpath("/a/b/../c") + "?q=" + x + "#top"))


def _w244_url_parts_edit(i):
    """url-parts-edit"""
    from urllib.parse import urljoin
    v = i % 9 + 1
    x = str(v)
    origin = "https://user@example.org:8443"
    raw = "https://user@example.org:8443/a/b/../c?q=" + x + "#top"
    q = "k=" + x + "&on=true&k=2"
    return (len(origin + "/a/b/../c/d/e" + x + "?q=" + x + "#top") + len(origin + "/a/b/../c?q=" + x + "&" + q + "#top")
            + len(origin + "/a/b/../c?" + q + "#top") + len(urljoin(raw, "../z" + x)) + len("a%20b/" + x))


def _w244_url_query(i):
    """url-query"""
    from urllib.parse import urlencode
    v = i % 9 + 1
    x = str(v)
    pairs = [("k", x), ("on", "true"), ("k", "2")]
    return 1 + 2 + v + 1 + 0 + len(urlencode(pairs))


def _w244_url_form(i):
    """url-form"""
    from urllib.parse import urlencode, unquote_to_bytes
    v = i % 9 + 1
    x = str(v)
    form = urlencode([("k", x + " &"), ("on", "true")])
    return 2 + len(form) + len(form.encode()) + len(unquote_to_bytes("a%20b%2" + x))


def _w244_db_value_capture_scalars(i):
    """db-value-capture-scalars"""
    v = i % 9 + 1
    return (1 if v % 2 == 0 else 0) + (v * 1000003) % 1000 + (2 if v % 2 == 0 else 7)


def _w244_db_value_capture_text(i):
    """db-value-capture-text"""
    v = i % 9 + 1
    x = str(v)
    # Widths are the payload sizes in bytes: UTF-8 for text, none for NULL.
    return len(x + ".25") + len("\u00e9" + x) + 10 * len(("\u00e9" + x).encode()) + 2 * v + 100 * 3 + 1000 * 0


def _w244_db_value_capture_protocol(i):
    """db-value-capture-protocol"""
    v = i % 9 + 1
    return 0 + 8 + 2 + 16 + 4 + 2 + v


def _w244_db_value_print(i):
    """db-value-print"""
    v = i % 9 + 1
    x = str(v)
    return (1 if v % 2 == 1 else 0) + 7 * v + (4 if v % 3 == 0 else 1) + len(x + ".5") + len("t" + x) + v + v + 2 * len(x)


def _w244_db_value_kinds(i):
    """db-value-kinds"""
    v = i % 9 + 1
    none = i % 3 == 0
    return (0 if none else 1) + 2 + 1 + (0 if none else 3) + (0 if none else 4) + 4 + 3 + 5 + v


def _w244_db_value_readers(i):
    """db-value-readers"""
    v = i % 9 + 1
    x = str(v)
    return ((1 if v % 2 == 0 else 0) + v + 6 + len(x + ".75") + (9 if v > 5 else 3) + 11 * v + (100 if i % 4 == 0 else v)
            + len("s" + x))


def _w244_db_value_native(i):
    """db-value-native"""
    v = i % 9 + 1
    code = i % 10
    return (code if code < 7 else 50) + 5 + v


def _w244_db_column_reader_typed(i):
    """db-column-reader-typed"""
    v = i % 9 + 1
    x = str(v)
    # flag of 1 is true; 2 and 3 are refused.
    flag = 20 if i % 3 == 0 else 30
    return (1 if v % 2 == 0 else 0) + v + 5 + 3 * v + len("ab" + x) + len(x + ".5") + 2 + flag


def _w244_db_page_reader(i):
    """db-page-reader"""
    v = i % 9 + 1
    return v + len("t" + str(v)) + (7 + v) + 0 + 2


def _w244_db_maybe_reader(i):
    """db-maybe-reader"""
    v = i % 9 + 1
    x = str(v)
    b = 1 if v % 2 == 0 else 0
    f = i % 2
    g = 6 if v > 4 else 2

    return b + v + 5 + len(x + ".5") + f + g + 13 * v + 13 * v + 100 + 1 + 3


def _w244_db_maybe_presence(i):
    """db-maybe-presence"""
    v = i % 9 + 1
    x = str(v)
    b = 1 if v % 2 == 0 else 0
    f = i % 2
    g = 6 if v > 4 else 2

    return len("t" + x) + 2 + 1 + 2 + 3 + 5 + 2 + len(x + ".5") + 1


def _w244_db_presence_reader(i):
    """db-presence-reader"""
    v = i % 9 + 1
    x = str(v)
    b = 1 if v % 2 == 0 else 0
    f = i % 2
    g = 6 if v > 4 else 2

    return g + 13 * v + 1000 + len("t" + x) + b + v + len(x + ".5") + f


def _w244_db_statement_values(i):
    """db-statement-values"""
    v = i % 9 + 1
    x = str(v)
    b = 1 if v % 2 == 0 else 0
    f = i % 2
    g = 6 if v > 4 else 2

    snap = 10 if i % 4 == 0 else 2
    return g + 0 + 8 + snap + len("SELECT ?" + x) + 0 + 2 + v


def _w244_sqlite_session_execute(i):
    """sqlite-session-execute"""
    v = i % 9 + 1
    return 2 + 10 * len([x for x in (v, 2) if x > i % 3]) + v


def _w244_sqlite_session_rows(i):
    """sqlite-session-rows"""
    v = i % 9 + 1
    xs = [k for k in range(1, 11 + v) if k > v % 3][::-1]
    return len(xs) + xs[0] + len('n%d' % xs[-1]) + len([k for k in range(1, 11 + v) if k <= v])


def _w244_sqlite_session_prepare(i):
    """sqlite-session-prepare"""
    v = i % 9 + 1
    xs = sorted(x for x in (v, v + 5, 20) if x > v)
    return 1 + len(xs) + xs[0] + 1 + 2


def _w244_sqlite_transaction_control(i):
    """sqlite-transaction-control"""
    v = i % 9 + 1
    return v + 1


def _w244_sqlite_transaction_work(i):
    """sqlite-transaction-work"""
    v = i % 9 + 1
    return 1 + 2 + sum(x for x in (v, 2, 3, 10) if x > 1)


def _w244_sqlite_blob_session(i):
    """sqlite-blob-session"""
    v = i % 9 + 1
    blob = bytearray(v + 4)
    blob[1:3] = bytes([v, 7])
    return blob[0] + blob[1] + blob[2] + len(blob)


def _w244_sqlite_script(i):
    """sqlite-script"""
    v = i % 9 + 1
    return 3 + 10 * 2 + 100 * len([x for x in (v, 2) if x < 3])


def _w244_sqlite_backup(i):
    """sqlite-backup"""
    v = i % 9 + 1
    # Page 1 holds the schema; each empty table takes one root page.
    pages = 1 + v
    return v + pages + 10 * pages + v + 1


def _w244_sqlite_failures(i):
    """sqlite-failures"""
    v = i % 9 + 1
    return 1 + v + (v + 3) + 1 + 10 * 1 + 4


def _w244_sqlite_prepared_transaction(i):
    """sqlite-prepared-transaction"""
    v = i % 9 + 1
    return v + 7 + 100 * (2 if i % 2 == 0 else 0)


def _w244_sqlite_acquisition(i):
    """sqlite-acquisition"""
    v = i % 9 + 1
    return v + 20


def _w244_db_pool_state_lease(i):
    """db-pool-state-lease"""
    v = i % 9 + 1
    minimum = i % 2 + 1
    # warm keeps `minimum` physical connections: one is leased, the rest idle; the lease returns to idle.
    return 1 + 10 * (minimum - 1) + 100 * minimum + 3 + v


def _w244_db_pool_state_retire(i):
    """db-pool-state-retire"""
    v = i % 9 + 1
    serial = 2 if i % 2 == 0 else 1
    return 10 * serial + 1 + 2 + 4 + v


def _w244_db_pooled_connection(i):
    """db-pooled-connection"""
    v = i % 9 + 1
    return 2 + min(v, 3) + 1 + 2 + 10 * 2


def _w244_db_pooled_transaction(i):
    """db-pooled-transaction"""
    v = i % 9 + 1
    return 1 + v + 5 + 2


def _w244_db_pool_prepared(i):
    """db-pool-prepared"""
    v = i % 9 + 1
    return 2 + min(v, 2) + 0 + 1 + 2


def _w244_db_physical(i):
    """db-physical"""
    v = i % 9 + 1
    return v + 7 + v


def _w244_db_query_forms(i):
    """db-query-forms"""
    v = i % 9 + 1
    return 3 * v + 2 * v + 100 + (v + 2 * v + 3 * v) + 10 * 1 + v + 2


def _w244_locale_subtag_shapes(i):
    """locale-subtag-shapes"""
    pool = ['en', 'Latn', 'DE', '1996', 'yue', 'abc12', 'x', 'u1', 't0', '419', 'rozaj', 'a1b2c3d4e', 'ca', 'ZH', '9', 'h0']
    p = len(pool)
    picks = [pool[i % p], pool[(i + 5) % p], pool[(i * 7 + 3) % p]]
    al = lambda s: s.isascii() and s.isalpha()
    dg = lambda s: s.isascii() and s.isdigit()
    an = lambda s: s.isascii() and s.isalnum()

    def bits(s):
        # RFC 5646 section 2.1 grammar.
        b = 1 if 2 <= len(s) <= 8 and al(s) else 0
        b += 2 if len(s) == 4 and al(s) else 0
        b += 4 if (len(s) == 2 and al(s)) or (len(s) == 3 and dg(s)) else 0
        b += 8 if ((5 <= len(s) <= 8) or (len(s) == 4 and s[0].isdigit())) and an(s) else 0
        b += 16 if len(s) == 3 and al(s) else 0
        b += 32 if 2 <= len(s) <= 8 and an(s) else 0
        return b + ord(s.lower()[:1].upper())
    return sum(bits(s) for s in picks)


def _w244_locale_subtag_keys(i):
    """locale-subtag-keys"""
    pool = ['en', 'Latn', 'DE', '1996', 'yue', 'abc12', 'x', 'u1', 't0', '419', 'rozaj', 'a1b2c3d4e', 'ca', 'ZH', '9', 'h0']
    p = len(pool)
    picks = [pool[i % p], pool[(i + 5) % p], pool[(i * 7 + 3) % p]]
    al = lambda s: s.isascii() and s.isalpha()
    dg = lambda s: s.isascii() and s.isdigit()
    an = lambda s: s.isascii() and s.isalnum()

    x = str(i % 9 + 1)
    def bits(s):
        b = 1 if 1 <= len(s) <= 8 and an(s) else 0
        b += 2 if len(s) == 1 and an(s) else 0
        b += 4 if len(s) == 2 and an(s[0]) and al(s[1]) else 0
        b += 8 if len(s) == 2 and al(s[0]) and dg(s[1]) else 0
        return b
    return sum(bits(s) for s in picks) + len("ca-islamic-civil" + x) + 2 + 10 + 3


def _w244_locale_extensions(i):
    """locale-extensions"""
    tags = ['de-DE-u-attr-ca-buddhist-nu-latn-t-en-h0-hybrid-x-priv', 'sr-Latn-RS-u-nu-latn', 'en-US-u-co-phonebk-x-a-b', 'zh-Hant-TW-t-ja', 'fr-CA']
    text = tags[i % 5]
    parts = text.split("-")
    base, rest = [], []
    for k, s in enumerate(parts):
        if len(s) == 1:
            rest = parts[k:]
            break
        base.append(s)
    ext = {}
    key = None
    for s in rest:
        if len(s) == 1:
            key = s
            ext[key] = []
            if s == "x":
                break
        else:
            ext[key].append(s)
    u = ext.get("u", [])
    attrs = [s for s in u[:next((k for k, s in enumerate(u) if len(s) == 2), len(u))]]
    kw = []
    for s in u[len(attrs):]:
        if len(s) == 2:
            kw.append([s])
        else:
            kw[-1].append(s)
    suffix = ("-" + "-".join(rest)) if rest else ""
    return (len(attrs) + 10 * len(kw) + (len("-".join(kw[0])) if kw else 0) + 100 * len(ext.get("t", []))
            + len("-".join(base)) + len(suffix) + len(text))


def _w244_locale_fields(i):
    """locale-fields"""
    def canon(t):
        out = []
        after = False
        for k, s in enumerate(t.split("-")):
            if after or len(s) == 1:
                after = True
                out.append(s.lower())
            elif k > 0 and len(s) == 4:
                out.append(s.title())
            elif k > 0 and len(s) == 2:
                out.append(s.upper())
            else:
                out.append(s.lower())
        return "-".join(out)
    tags = ["SR-latn-rs", "en-us", "zh-HANT-tw-x-p", "de", "pt-BR-u-nu-arab"]
    c = canon(tags[i % 5])
    o = canon(tags[(i + 1) % 5])
    parts = c.split("-")
    base = []
    for s in parts:
        if len(s) == 1:
            break
        base.append(s)
    lang = base[0]
    script = next((s for s in base[1:] if len(s) == 4), "")
    region = next((s for s in base[1:] if len(s) == 2 or (len(s) == 3 and s.isdigit())), "")
    return (len(lang) + 10 * len(script) + 100 * len(region) + len("-".join(base)) + (1000 if c < o else 2000)
            + len(canon(tags[(i + 2) % 5])) + len("YYYY-MM-DD") + 10000 * len(parts))


def _w244_locale_filtered(i):
    """locale-filtered"""
    available = ["en-US", "en-GB", "de", "de-CH", "fr-CA", "sr-Latn-RS"]
    ranges = ["en", "de-ch", "fr", "sr-latn", "*", "zh"]
    req = [ranges[i % 6], ranges[(i + 2) % 6]]
    out = []
    for r in req:
        for a in available:
            if (r == "*" or a.lower() == r or a.lower().startswith(r + "-")) and a not in out:
                out.append(a)
    def lookup(r):
        parts = r.split("-")
        while parts:
            cand = "-".join(parts)
            for a in available:
                if a.lower() == cand:
                    return a
            parts.pop()
            if parts and len(parts[-1]) == 1:
                parts.pop()
        return "de"
    looked = lookup(["en-us-x-a", "de-ch-1901", "fr-fr", "zh-hant"][i % 4])
    return len(out) + 10 * len(out) + (len(out[0]) + 2 * len(out[-1]) if out else 0) + 100 * len(looked)


def _w244_locale_number_print(i):
    """locale-number-print"""
    # CLDR 46 number symbols: group, decimal, the space before a percent sign.
    SYMBOLS = {"en": (",", ".", ""), "de": (".", ",", "\u00a0"), "fr": ("\u202f", ",", "\u00a0")}
    code = lambda text: sum(ord(c) for c in text)
    def grouped(n, loc):
        g, d, p = SYMBOLS[loc]
        text = "{:,}".format(abs(n)).replace(",", g)
        return ("-" if n < 0 else "") + text
    def localized_decimal(text, loc):
        g, d, p = SYMBOLS[loc]
        whole, _, frac = text.partition(".")
        return grouped(int(whole), loc) + (d + frac if frac else "")

    v = i % 9 + 1
    loc = ["en", "de", "fr"][i % 3]
    g, d, p = SYMBOLS[loc]
    pct = "1" + str(v)
    special = ["NaN", "\u221e", "-\u221e"][i % 3]
    return (code(grouped(1234567 * v, loc)) + code(grouped(-98765432 * v, loc)) + code(localized_decimal("1234.5" if v % 2 == 0 else "0.25", loc))
            + code(pct + p + "%") + code(special) + (1234567 * v) % 1000 + len("2.75"))


def _w244_locale_number_data(i):
    """locale-number-data"""
    v = i % 9 + 1
    code = lambda text: sum(ord(c) for c in text)
    # CLDR 46: en percent "#,##0%", de "#,##0\u00a0%" (kind 2); patterns keep {minus} and {percent}
    # tokens that template() replaces with the symbols; ar-EG writes arab digits.
    suffix = "%" if i % 2 == 0 else "\u00a0%"
    return code(suffix.replace("%", "{percent}")) + code("{minus}") + 3 + code(suffix) + 1 + code("".join(chr(0x660 + int(d)) for d in "12" + str(v)))


def _w244_locale_number_magnitude(i):
    """locale-number-magnitude"""
    import decimal
    D = decimal.Decimal
    v = i % 9 + 1
    value = D("12.%d%d5" % (i % 3, v)).scaleb(i % 3 - 1)
    q = value.quantize(D("0.01"), rounding=decimal.ROUND_HALF_EVEN)
    whole, _, frac = format(q, "f").partition(".")
    frac = frac.rstrip("0") or "0"
    ascii_text = whole + "." + frac
    raw = len(str(4560 * v)) - 1 - 2
    return (raw + 10 * (value.adjusted() + 2) + 100 * len(whole) + len(frac) + 1000 * len(ascii_text) + 10000 * (q.adjusted() + 2)
            + int(whole))


def _w244_locale_number_printing(i):
    """locale-number-printing"""
    # CLDR 46 number symbols: group, decimal, the space before a percent sign.
    SYMBOLS = {"en": (",", ".", ""), "de": (".", ",", "\u00a0"), "fr": ("\u202f", ",", "\u00a0")}
    code = lambda text: sum(ord(c) for c in text)
    def grouped(n, loc):
        g, d, p = SYMBOLS[loc]
        text = "{:,}".format(abs(n)).replace(",", g)
        return ("-" if n < 0 else "") + text
    def localized_decimal(text, loc):
        g, d, p = SYMBOLS[loc]
        whole, _, frac = text.partition(".")
        return grouped(int(whole), loc) + (d + frac if frac else "")

    import decimal
    v = i % 9 + 1
    loc = ["en", "de"][i % 2]
    g, d, p = SYMBOLS[loc]
    value = decimal.Decimal(12345 * v).scaleb(-2)
    # A percent printing takes the value as already scaled.
    q = value.quantize(decimal.Decimal("0.1"), rounding=decimal.ROUND_HALF_EVEN)
    text = format(q, "f").rstrip("0").rstrip(".")
    printed = localized_decimal(text, loc) + (p + "%" if i % 3 == 0 else "")
    return code(printed) + 3 + 10 * (i % 3 + 1) + 100 * 2 + 1000 * 4


def _w244_locale_number_zeroes(i):
    """locale-number-zeroes"""
    count = i % 17
    return count + (1 if count > 0 else 5)


def _w244_locale_case_currency(i):
    """locale-case-currency"""
    # CLDR 46 number symbols: group, decimal, the space before a percent sign.
    SYMBOLS = {"en": (",", ".", ""), "de": (".", ",", "\u00a0"), "fr": ("\u202f", ",", "\u00a0")}
    code = lambda text: sum(ord(c) for c in text)
    def grouped(n, loc):
        g, d, p = SYMBOLS[loc]
        text = "{:,}".format(abs(n)).replace(",", g)
        return ("-" if n < 0 else "") + text
    def localized_decimal(text, loc):
        g, d, p = SYMBOLS[loc]
        whole, _, frac = text.partition(".")
        return grouped(int(whole), loc) + (d + frac if frac else "")

    v = i % 9 + 1
    x = str(v)
    def tr_lower(s):
        return s.replace("I", "\u0131").replace("\u0130", "i").lower()
    def tr_title(s):
        return " ".join(("\u0130" if w[0] == "i" else w[0].upper()) + tr_lower(w[1:]) for w in s.split(" "))
    loc, cur = ["en USD", "de EUR", "fr EUR", "en JPY"][i % 4].split(" ")
    amount = x + ("234.57" if cur != "JPY" else "235")
    number = localized_decimal(amount, loc)
    # CLDR 46: en "\u00a4#,##0.00", de and fr "#,##0.00\u00a0\u00a4"; symbols $, \u20ac, \u00a5.
    symbol = {"USD": "$", "EUR": "\u20ac", "JPY": "\u00a5"}[cur]
    text = symbol + number if loc == "en" else number + "\u00a0" + symbol
    return code(tr_lower("ISTANBUL " + x + "\u0130")) + code(tr_title("istanbul il\u00e7e" + x)) + (1 if cur < "GBP" else 2) + code(text)


def _w244_locale_gregorian_names(i):
    """locale-gregorian-names"""
    code = lambda text: sum(ord(c) for c in text)
    loc = ["en", "de", "fr"][i % 3]
    MONTHS = {"en": "January February March April May June July August September October November December",
              "de": "Januar Februar M\u00e4rz April Mai Juni Juli August September Oktober November Dezember",
              "fr": "janvier f\u00e9vrier mars avril mai juin juillet ao\u00fbt septembre octobre novembre d\u00e9cembre"}
    DAYS = {"en": "Monday Tuesday Wednesday Thursday Friday Saturday Sunday",
            "de": "Montag Dienstag Mittwoch Donnerstag Freitag Samstag Sonntag",
            "fr": "lundi mardi mercredi jeudi vendredi samedi dimanche"}

    v = i % 9 + 1
    era = {"en": "AD", "de": "n. Chr.", "fr": "ap. J.-C."}[loc]
    return (code(MONTHS[loc].split()[i % 12]) + code(DAYS[loc].split()[i % 7]) + code(era) + code("AM" if i % 24 < 12 else "PM")
            + code("%02d" % v) + 1 + 1)


def _w244_locale_localized_times(i):
    """locale-localized-times"""
    code = lambda text: sum(ord(c) for c in text)
    loc = ["en", "de", "fr"][i % 3]
    MONTHS = {"en": "January February March April May June July August September October November December",
              "de": "Januar Februar M\u00e4rz April Mai Juni Juli August September Oktober November Dezember",
              "fr": "janvier f\u00e9vrier mars avril mai juin juillet ao\u00fbt septembre octobre novembre d\u00e9cembre"}
    DAYS = {"en": "Monday Tuesday Wednesday Thursday Friday Saturday Sunday",
            "de": "Montag Dienstag Mittwoch Donnerstag Freitag Samstag Sonntag",
            "fr": "lundi mardi mercredi jeudi vendredi samedi dimanche"}

    import datetime as dt
    def short_time(h, m):
        if loc == "en":
            return "%d:%02d\u202f%s" % (h % 12 or 12, m, "AM" if h < 12 else "PM")
        return "%02d:%02d" % (h, m)
    def short_date(d):
        return {"en": "%d/%d/%02d" % (d.month, d.day, d.year % 100), "de": "%02d.%02d.%02d" % (d.day, d.month, d.year % 100),
                "fr": "%02d/%02d/%d" % (d.day, d.month, d.year)}[loc]
    def short_both(d, h, m):
        return short_date(d) + (" " if loc == "fr" else ", ") + short_time(h, m)
    def last_sunday(year, month):
        d = dt.date(year, month, 31)
        return d - dt.timedelta(days=(d.weekday() + 1) % 7)
    def berlin(utc):
        # EU rule: summer time from the last Sunday of March to the last Sunday of October, 01:00 UTC.
        start = dt.datetime.combine(last_sunday(utc.year, 3), dt.time(1))
        end = dt.datetime.combine(last_sunday(utc.year, 10), dt.time(1))
        summer = start <= utc < end
        return utc + dt.timedelta(hours=2 if summer else 1), summer

    h, m = i % 24, i * 7 % 60
    local = dt.date(2026, i % 12 + 1, i % 28 + 1)
    utc = dt.datetime(1970, 1, 1) + dt.timedelta(seconds=1767600000 + i * 1234567)
    plus3 = utc + dt.timedelta(hours=3)
    there, summer = berlin(utc)
    names = {"en": ("Central European Standard Time", "Central European Summer Time"),
             "de": ("Mitteleurop\u00e4ische Normalzeit", "Mitteleurop\u00e4ische Sommerzeit"),
             "fr": ("heure normale d\u2019Europe centrale", "heure d\u2019\u00e9t\u00e9 d\u2019Europe centrale")}[loc]
    combination = "{1} {0}" if loc == "fr" else "{1}, {0}"
    return (code(short_time(h, m)) + code(short_both(local, h, m)) + code(short_both(plus3.date(), plus3.hour, plus3.minute))
            + code(short_both(there.date(), there.hour, there.minute)) + code(names[1 if summer else 0]) + code(combination))


def _w244_locale_gmt_offset(i):
    """locale-gmt-offset"""
    code = lambda text: sum(ord(c) for c in text)
    loc = ["en", "de", "fr"][i % 3]
    MONTHS = {"en": "January February March April May June July August September October November December",
              "de": "Januar Februar M\u00e4rz April Mai Juni Juli August September Oktober November Dezember",
              "fr": "janvier f\u00e9vrier mars avril mai juin juillet ao\u00fbt septembre octobre novembre d\u00e9cembre"}
    DAYS = {"en": "Monday Tuesday Wednesday Thursday Friday Saturday Sunday",
            "de": "Montag Dienstag Mittwoch Donnerstag Freitag Samstag Sonntag",
            "fr": "lundi mardi mercredi jeudi vendredi samedi dimanche"}

    seconds = (i % 52 - 24) * 1800
    gmt = "UTC" if loc == "fr" else "GMT"
    # CLDR 46 hourFormat: fr writes U+2212 for minus, en and de a hyphen-minus.
    sign = ("\u2212" if loc == "fr" else "-") if seconds < 0 else "+"
    h, rest = divmod(abs(seconds), 3600)
    m = rest // 60
    if seconds == 0:
        text = gmt
    elif i % 2 == 0:
        text = "%s%s%02d:%02d" % (gmt, sign, h, m)
    else:
        text = "%s%s%d" % (gmt, sign, h) + (":%02d" % m if m else "")
    return code(text)


def _w244_locale_cldr_gregorian(i):
    """locale-cldr-gregorian"""
    tag = ["en", "de", "fr", "ru", "ja", "zz-Unknown"][i % 6]
    known = tag != "zz-Unknown"
    gregorian = (1 + 2 + 4 if known else 100) + 8
    n = i % 30 + 1
    # CLDR plural rules: ru cardinal one / few / many for integers; en ordinal one / two / few / other.
    if n % 10 == 1 and n % 100 != 11:
        ru = 1
    elif 2 <= n % 10 <= 4 and not 12 <= n % 100 <= 14:
        ru = 3
    else:
        ru = 4
    en = {1: 1, 2: 2, 3: 3}.get(n % 10, 5) if not 11 <= n % 100 <= 13 else 5
    # IANA registry: i-klingon -> tlh, zh-min-nan -> nan, art-lojban -> jbo; en-US is no whole tag.
    preferred = {"i-klingon": "tlh", "zh-min-nan": "nan", "en-US": "", "art-lojban": "jbo"}[["i-klingon", "zh-min-nan", "en-US", "art-lojban"][i % 4]]
    return gregorian + (16 if known else 0) + 32 * ru + 1000 * en + 10000 * len(preferred)


def _w244_locale_cldr_numbers(i):
    """locale-cldr-numbers"""
    # IANA: i-klingon and zh-min-nan are grandfathered; yue, cmn and zsm are extlangs of zh, zh and ms.
    grand = 1 if i % 4 in (0, 2) else 0
    prefix = {"yue": "zh", "cmn": "zh", "abc": "", "zsm": "ms"}[["yue", "cmn", "abc", "zsm"][i % 4]]
    known = i % 4 != 3
    letter = ["A", "$", "AB", "z"][i % 4]
    return grand + 2 * len(prefix) + (30 if known else 300) + (400 if len(letter) == 1 and letter.isalpha() else 0) + 1000 + 2000


def _w244_locale_preferred(i):
    """locale-preferred"""
    lists = [["de-DE", "fr", "und"], ["pt-BR"], ["und"], ["und"]]
    values = lists[i % 4]
    if i % 5 == 0:
        values = ["sr-Latn-RS", "en"]
    # LANGUAGE de_DE:fr:de_DE.UTF-8:C -> de-DE, fr, und (the duplicate dropped); C and POSIX are und.
    return len(values) + 10 * len(values[0]) + 100 * len(values)


def _w244_crypto_aead_text(i):
    """crypto-aead-text"""
    v = i % 9 + 1
    text = "note " + str(v) * 2
    # An AEAD ciphertext is the plaintext length plus a 16-byte tag; a changed AAD fails authentication.
    return (1 if i % 2 == 0 else 2) + len(text) + 10 * (len(text) + 16) + len(text) + 100


def _w244_crypto_xchacha(i):
    """crypto-xchacha"""
    v = i % 9 + 1
    size = i % 40 + 1
    return size + 16 + (size - 1) + 10 * len("w" + str(v)) + 1000


def _w244_crypto_ed25519_x25519(i):
    """crypto-ed25519-x25519"""
    s = i % 16
    seed = bytes((k * 11 + s) % 256 for k in range(32))
    other = bytes((k * 13 + s + 1) % 256 for k in range(32))
    nine = (9).to_bytes(32, "little")
    shared = x25519(seed, x25519(other, nine))
    # RFC 8410: SPKI is 12 header bytes and the 32 raw bytes; PKCS#8 carries the seed at offset 16.
    return 1 + 0 + 4 + 10 * 44 + 1000 * ed25519_public(seed)[31] + 100000 * shared[0]


def _w244_crypto_ecdsa_keys(i):
    """crypto-ecdsa-keys"""
    # RFC 7518 section 3.4: an ES256 JWS signature is R || S, 64 bytes.
    return len(pem_der('-----BEGIN PRIVATE KEY-----\nMIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgooq/y5Wqru57uZ/f\nLtDqQBGy63m5uDllm9k87Q2FqQuhRANCAAQaMDVadj3Njz0QDGkIC6mwMvsIrcKE\ngNpBpZ4p9lZl/U7IZOjaGwl/oQS67X/YHf5rcfdxvcb70003NsmoexTL\n-----END PRIVATE KEY-----\n')) + 1000 * 64 + 1 + 0 + 4


def _w244_crypto_jwt_keys(i):
    """crypto-jwt-keys"""
    s = i % 16
    message = ("h.c" + str(s)).encode()
    digest = [hashlib.sha256, hashlib.sha384, hashlib.sha512][i % 3]
    tag = hmac.new(b"0123456789abcdef" * 4, message, digest).digest()
    seed = bytes((k + s) % 256 for k in range(32))
    return len(tag) + tag[0] + 1000 * ed25519_sign(seed, message)[0] + 1 + 2 + len("HS256") * 100000


def _w244_crypto_rsa_keys(i):
    """crypto-rsa-keys"""
    return 1 + 0 + 4 + 10 * 2048 + 100000 * len(pem_der('-----BEGIN PRIVATE KEY-----\nMIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQDMrGJ6RyrG8131\ninCEJcrj0Z9FsI39S7Voe/05v0u44dwX4RngiHMyoPHfqSoDZ8l9KlOBV1oMNJd3\n1WeLCtgSZyzFWnBAFuK89tOpkHejlYbLXyzYpsZKrNkxdHfa3HtkPY5eb1TsHzzm\nbcDxiiuW+bRHXpvYp6Txz1+47SmVHh+Oa6hwYZ8EWIEqUvx6ffhFgxbgm+x5iYaZ\nTm8odDJZGWIGpGLpRH++6cR5oRRK5KvlZWyXzqGfGTMmjSQI3Jy8ty5j2tEFwzUB\nJhfimcPAExbFLLeEjgWCCw5iOEwAJI2a1Z3BcwcfYIao1yPU+jb9YXXu4tuwDnRW\nBbEsKYqdAgMBAAECggEAE+X4hwHPP1teuaX1bWuNL5vsG6bgDw2iu98+DbSy5G0U\n7D9muCUb6gKqsT9thbo75Q7N4oe/Ceu9BBn7wJFT02ozeJZjzl9LOOrYRiZ08zLw\n0WnV3tRL9+uZDA1SL7V6y5KOelDhlTkuPwrRlMoFld597+hbR3N4/kdWe5CXF9eS\n4BYgGSIXWFXNRjpO51wnwxFuiJDmbbjO6s1UJqg4l+28YNqqfiJ/Ta4MBRoYbxwj\nm8T2RPuiH9YWKkDKUFP7u6EiFV2OaF14o6Ydj14ZEcs8dDMBTptM+ajVHyA/UwNE\n99St2Xa+2VacJIiESUHcCiI6HV//2TSlYkY+H5GiMwKBgQDnNdaDE4Ak+1EP3w0E\n3jXDpKQM/AbUgsTXZ1WveAHpOQ4ZFvin3KWlHKlntiNZxsIcEXSrsNeakYhVduO/\nR2nrnIu1QA6JMNc2MaF7rmTBx3q5sr4ZLlCumdwSvdgjjnaj/7oP5tZWISYrH1jg\nhDocQETIEFxmNLZbOq4EpHSn7wKBgQDiniwmqIKODB3wN9LKACobToRKXC147k0W\nEaNKRFSAxFexYLE9x4iEXM7jmR4pXI5K9wOJHbFK776hNAhvIxxs4JJsUqQXN+hR\nnVMFX3kxoKvhbIj0ndqtRe29v4WRChgld+zRnCZup4DfUXURuT/7ZkEr93jE3fvN\nsbT+XK9KMwKBgHITZx5O7cfRU6KAC71eIarAvcxGQNxzFMmui2FC1baLVwBcDB1O\nSVAYvyCThgeSgE0285iObSIvMdPxD9ddVc5Q88FetI1N0rFDdYg/eCj/0fZzUUW9\nS0OKWEHK9yw76x9pWhTkcNYYDuvx0Hkd5EnyD72iDOTEl0CVyCw0M/ZzAoGBAIzv\nfdnU9szMsFnS/d7u10EARbnRNd59padR9NGdSUiD013XGqOnUjmvVmOpuF5U15G7\nqZE+zCIkk9PBfnVPyr/Tsnx3sOFXvb30L8upor/AguTBETPAcF1Onhph3/lh4755\npOh/Sk+LENTR3hLVuPrqjyF+2Lbmr1Evcba3gbSNAoGBAKmsLbGeaYD9CvvhrXVC\ncq5kbLDwJf/DRxuS1ypr9u+dnQEb8N1jldsCUcra3QxG+FLbXiq5yA609SElAQFa\nWksPKFh1Yc/PV1Ng6Y5JiWAQWBDSKf/K1+7V9Tzwl/KL5P/s9v4mdYE0u+JGqkp3\nmVFsewgdVbka4ezKH1dacziu\n-----END PRIVATE KEY-----\n')) + len("r" + str(i)) * 8


def _w244_crypto_rsa_pss_oaep(i):
    """crypto-rsa-pss-oaep"""
    # RSA-2048: signatures and OAEP ciphertexts are 256 bytes.
    return 1 + 2 + 10 * 256 + 4 + 10000 * 256 + 8 + 10000000 * i


def _w244_crypto_key_format(i):
    """crypto-key-format"""
    which = i % 2
    privates = ['-----BEGIN PRIVATE KEY-----\nMIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgooq/y5Wqru57uZ/f\nLtDqQBGy63m5uDllm9k87Q2FqQuhRANCAAQaMDVadj3Njz0QDGkIC6mwMvsIrcKE\ngNpBpZ4p9lZl/U7IZOjaGwl/oQS67X/YHf5rcfdxvcb70003NsmoexTL\n-----END PRIVATE KEY-----\n', '-----BEGIN PRIVATE KEY-----\nMIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQDMrGJ6RyrG8131\ninCEJcrj0Z9FsI39S7Voe/05v0u44dwX4RngiHMyoPHfqSoDZ8l9KlOBV1oMNJd3\n1WeLCtgSZyzFWnBAFuK89tOpkHejlYbLXyzYpsZKrNkxdHfa3HtkPY5eb1TsHzzm\nbcDxiiuW+bRHXpvYp6Txz1+47SmVHh+Oa6hwYZ8EWIEqUvx6ffhFgxbgm+x5iYaZ\nTm8odDJZGWIGpGLpRH++6cR5oRRK5KvlZWyXzqGfGTMmjSQI3Jy8ty5j2tEFwzUB\nJhfimcPAExbFLLeEjgWCCw5iOEwAJI2a1Z3BcwcfYIao1yPU+jb9YXXu4tuwDnRW\nBbEsKYqdAgMBAAECggEAE+X4hwHPP1teuaX1bWuNL5vsG6bgDw2iu98+DbSy5G0U\n7D9muCUb6gKqsT9thbo75Q7N4oe/Ceu9BBn7wJFT02ozeJZjzl9LOOrYRiZ08zLw\n0WnV3tRL9+uZDA1SL7V6y5KOelDhlTkuPwrRlMoFld597+hbR3N4/kdWe5CXF9eS\n4BYgGSIXWFXNRjpO51wnwxFuiJDmbbjO6s1UJqg4l+28YNqqfiJ/Ta4MBRoYbxwj\nm8T2RPuiH9YWKkDKUFP7u6EiFV2OaF14o6Ydj14ZEcs8dDMBTptM+ajVHyA/UwNE\n99St2Xa+2VacJIiESUHcCiI6HV//2TSlYkY+H5GiMwKBgQDnNdaDE4Ak+1EP3w0E\n3jXDpKQM/AbUgsTXZ1WveAHpOQ4ZFvin3KWlHKlntiNZxsIcEXSrsNeakYhVduO/\nR2nrnIu1QA6JMNc2MaF7rmTBx3q5sr4ZLlCumdwSvdgjjnaj/7oP5tZWISYrH1jg\nhDocQETIEFxmNLZbOq4EpHSn7wKBgQDiniwmqIKODB3wN9LKACobToRKXC147k0W\nEaNKRFSAxFexYLE9x4iEXM7jmR4pXI5K9wOJHbFK776hNAhvIxxs4JJsUqQXN+hR\nnVMFX3kxoKvhbIj0ndqtRe29v4WRChgld+zRnCZup4DfUXURuT/7ZkEr93jE3fvN\nsbT+XK9KMwKBgHITZx5O7cfRU6KAC71eIarAvcxGQNxzFMmui2FC1baLVwBcDB1O\nSVAYvyCThgeSgE0285iObSIvMdPxD9ddVc5Q88FetI1N0rFDdYg/eCj/0fZzUUW9\nS0OKWEHK9yw76x9pWhTkcNYYDuvx0Hkd5EnyD72iDOTEl0CVyCw0M/ZzAoGBAIzv\nfdnU9szMsFnS/d7u10EARbnRNd59padR9NGdSUiD013XGqOnUjmvVmOpuF5U15G7\nqZE+zCIkk9PBfnVPyr/Tsnx3sOFXvb30L8upor/AguTBETPAcF1Onhph3/lh4755\npOh/Sk+LENTR3hLVuPrqjyF+2Lbmr1Evcba3gbSNAoGBAKmsLbGeaYD9CvvhrXVC\ncq5kbLDwJf/DRxuS1ypr9u+dnQEb8N1jldsCUcra3QxG+FLbXiq5yA609SElAQFa\nWksPKFh1Yc/PV1Ng6Y5JiWAQWBDSKf/K1+7V9Tzwl/KL5P/s9v4mdYE0u+JGqkp3\nmVFsewgdVbka4ezKH1dacziu\n-----END PRIVATE KEY-----\n']
    publics = ['-----BEGIN PUBLIC KEY-----\nMFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEGjA1WnY9zY89EAxpCAupsDL7CK3C\nhIDaQaWeKfZWZf1OyGTo2hsJf6EEuu1/2B3+a3H3cb3G+9NNNzbJqHsUyw==\n-----END PUBLIC KEY-----\n', '-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAzKxiekcqxvNd9YpwhCXK\n49GfRbCN/Uu1aHv9Ob9LuOHcF+EZ4IhzMqDx36kqA2fJfSpTgVdaDDSXd9VniwrY\nEmcsxVpwQBbivPbTqZB3o5WGy18s2KbGSqzZMXR32tx7ZD2OXm9U7B885m3A8Yor\nlvm0R16b2Kek8c9fuO0plR4fjmuocGGfBFiBKlL8en34RYMW4JvseYmGmU5vKHQy\nWRliBqRi6UR/vunEeaEUSuSr5WVsl86hnxkzJo0kCNycvLcuY9rRBcM1ASYX4pnD\nwBMWxSy3hI4FggsOYjhMACSNmtWdwXMHH2CGqNcj1Po2/WF17uLbsA50VgWxLCmK\nnQIDAQAB\n-----END PUBLIC KEY-----\n']
    def pem(kind, der):
        # RFC 7468: base64 in lines of 64 characters between the boundaries.
        body = base64.b64encode(der).decode()
        lines = [body[k:k + 64] for k in range(0, len(body), 64)]
        return "-----BEGIN %s-----\n%s\n-----END %s-----\n" % (kind, "\n".join(lines), kind)
    # A PEM RSA key has no signature scheme, so it signs and verifies nothing.
    return ((1 if which == 0 else 3) + 2 * (i % 4 + 1) + 10 * len(pem("PRIVATE KEY", pem_der(privates[which])))
            + 100000 * len(pem("PUBLIC KEY", pem_der(publics[which]))))


def _w244_crypto_hmac_secret(i):
    """crypto-hmac-secret"""
    v = i % 9 + 1
    key = ("k" + str(v)).encode()
    tag = hmac.new(key, ("part" + str(v)).encode() + bytes([i % 256]), hashlib.sha256).hexdigest()
    params = "m=19456,t=2,p=1" if i % 2 == 0 else "m=%d,t=%d,p=1" % (1024 * v, v)
    return int(tag[0], 16) + 16 + 0 + 64 + 128 * len(key) + 1000 * 3 + 10000 * len(params)


def _w244_crypto_host_nonce(i):
    """crypto-host-nonce"""
    s = i % 16
    seed = bytes((k * 3 + s) % 256 for k in range(32))
    size = i % 24 + 1
    return (ed25519_public(seed)[0] + (0 if i % 3 == 0 else 256) + 1000 * size + 100000 * ((i + size - 1) % 256)
            + (0 if i % 2 == 0 else 512))


def _w244_crypto_x509_values(i):
    """crypto-x509-values"""
    v = i % 9 + 1
    return 2 + 10 * 2 + 100 * v + 1000 * (2 if i % 3 == 0 else 1) + 10000 * len("1.3.6.1.5.5.7.3." + str(v)) + (100000 if i % 2 == 0 else 0)


def _w244_crypto_x509_extensions(i):
    """crypto-x509-extensions"""
    leaf = i % 2 == 0
    # Leaf: subjectAltName (localhost, 127.0.0.1), basicConstraints CA:FALSE (critical), keyUsage digitalSignature (critical),
    # extKeyUsage serverAuth, subject and authority key identifiers. CA: basicConstraints CA:TRUE (critical),
    # keyUsage keyCertSign and cRLSign (critical), subject key identifier. Both keys are P-256 (91-byte SPKI).
    if leaf:
        return 6 + 10 * 2 + 100 + 0 + 1000 * 1 + 10000 * 1 + 100000 * 91 + 100000000 * 1
    return 3 + 10 * 2 + 100 + 200 + 1000 * 2 + 0 + 100000 * 91 + 0


def _w244_crypto_x509_chain(i):
    """crypto-x509-chain"""
    # localhost is the leaf's DNS name; example.org is not, so the name check fails.
    path = 2 if i % 2 == 0 else 100
    return path + 1000 * (i % 4 + 1) + 10 * 2


def _w244_fs_path_forms(i):
    """fs-path-forms"""
    import posixpath
    v = i % 9 + 1
    x = str(v)
    p = ["/srv/app/./logs/../data/f" + x + ".tar.gz", "rel/a/b" + x + ".txt", "/a/b/c" + x, "../up/x" + x + ".md"][i % 4]
    # A path drops "." segments when made; ".." stays until normalized.
    p = p.replace("/./", "/")
    name = p.rsplit("/", 1)[-1]
    stem, dot, ext = name.rpartition(".")
    if not dot:
        stem = name
    replaced = p[:len(p) - len(name)] + stem + ".bak"
    return ((1 if p.startswith("/") else 0) + 10 * len(p + "/k" + x) + 100 * len(replaced) + 1000 * len(posixpath.normpath(p))
            + (10000 if p < "/srv/app" else 20000) + 100000 * len("data/q" + x) + 1000000 * len(stem) + 10000000)


def _w244_fs_stat_values(i):
    """fs-stat-values"""
    v = i % 9 + 1
    bits = (i * 37) % 512
    return (1000 + i) + (2000 + v) + 3000 + (4000 + i % 5) + 10000 * (bits >> 6) + 100000 * ((bits >> 3) & 7) + 1000000 * (bits & 7)


def _w244_fs_file_bytes(i):
    """fs-file-bytes"""
    data = bytes((k + i) % 256 for k in range(i % 50 + 10))
    return data[3] + 1000 * 4 + 10000 * data[8] + 10000000 * 8


def _w244_fs_file_text(i):
    """fs-file-text"""
    x = str(i % 9 + 1)
    content = "alpha" + x + "\n" + "beta-" + x + x + "\n" + "gamma\n"
    return len("alpha" + x) + 10 * 2 + 100 * len(content) + 1000 * 3 + 10000 * len("z" + x)


def _w244_fs_entries(i):
    """fs-entries"""
    x = str(i % 9 + 1)
    return 0 + 2 + 0 + 8 + 16 * len("data" + x)


def _w244_fs_tree_links(i):
    """fs-tree-links"""
    x = str(i % 9 + 1)
    # Descendants of t: u, u/f.txt, h.txt, s.
    return len("f.txt") + 10 * 4 + 100 * 1 + 1000 * len("abc" + x) + 100000 * 2 + 0


def _w244_fs_temp_file(i):
    """fs-temp-file"""
    x = str(i % 9 + 1)
    # Read at the end of what was written: nothing.
    return len("head" + x) + len("line\n") + 2 + 100 * 0 + 1000


def _w244_fs_temp_finish(i):
    """fs-temp-finish"""
    x = str(i % 9 + 1)
    sizes = len("keep" + x) + 10 * len("new!" + x + x) + 100 * 3 + 1000 * len("text " + x)
    # Replacing consumes (closes) the temporary file.
    return 1 + 2 + 0 + 8 + 16 * sizes


def _w244_fs_root_stat(i):
    """fs-root-stat"""
    v = i % 9 + 1
    return 2 + 10 * ord("0123456789"[i % 5]) + 1000 * 10 + 10000 * v + 100000 * 7


def _w244_mime_part_build(i):
    """mime-part-build"""
    v = i % 9 + 1
    return 2 + 10 * v + 100 * 1 + 1000 + 10000 + 100000 * 1


def _w244_mime_parse(i):
    """mime-parse"""
    x = str(i % 9 + 1)
    # The root and its two parts; AQID is three bytes.
    return 3 + 10 * 1 + 100 * 2 + 1000 * 3 + 10000 + 20000 + 100000 * len("inline; filename=p" + x + ".png")


def _w244_mime_validate(i):
    """mime-validate"""
    x = str(i % 9 + 1)
    odd = i % 2 == 1
    decoded = base64.b64decode("aGk=") if odd else ("h\u00e9" + x).encode()
    # A text body fits text/*, a binary body any non-multipart type, a multipart body only multipart/* with its boundary.
    return (2 if odd else 1) + (8 if odd else 4) + (32 if odd else 16) + 64 + 1000 * len("t" + x) + 10000 * len(decoded)


def _w244_mime_parameters(i):
    """mime-parameters"""
    v = i % 9 + 1
    x = str(v)
    # RFC 2045: values that are not tokens are quoted; tspecials and space are not token characters.
    written = 'charset=utf-8; name="a b%s"; q=%s' % (x, x)
    token = {"a": 1, " ": 0, "(": 0, "~": 1, "/": 0, "Z": 1}[["a", " ", "(", "~", "/", "Z"][i % 6]]
    limited = 2 if i % 3 == 2 else 9
    return (1 if i % 2 == 0 else 0) + 10 * len(written) + 1000 * token + 10000 * v + 100000 * limited


def _w244_mime_scanning(i):
    """mime-scanning"""
    x = str(i % 9 + 1)
    import email.header
    decoded = "".join(part.decode(cs) if isinstance(part, bytes) else part for part, cs in
                      email.header.decode_header("=?utf-8?B?%s?= =?utf-8?Q?_w%s?=" % (["0J/RgNC40LLQtdGC", "aGVsbG8="][i % 2], x)))
    return (len("text") + 10 * len("html") + 100 * 2 + 1000 * len("attachment") + 10000 * 2 + 100000 * len("tok" + x)
            + 1000000 * len(decoded) + 10000000 * len("a=b" + x))


def _w244_mime_types_map(i):
    """mime-types-map"""
    v = i % 9 + 1
    name = ["a.okk", "b.ok" + str(v), "c.png", "d.pdf"][i % 4]
    # Own entries first (extensions compared case-insensitively), then the fallback; unknown is application/octet-stream.
    if name.endswith(".okk") or name.endswith(".ok" + str(v)):
        t = "text/x-okkama"
    elif name.endswith(".png"):
        t = "image/x-own"
    else:
        t = "application/pdf" if i % 2 == 0 else "???"
    # An unknown name is UnknownMediaType (counted as 3).
    return len(t) + 100 * 2 + 1000 * 2 + 10000 * 3 + 1000000 * v


def _w244_mime_system_types(i):
    """mime-system-types"""
    x = str(i % 9 + 1)
    t = ["text/x-a" + x, "text/x-a" + x, "image/x-b", "???"][i % 4]
    return len(t) + 100 * 2


def _w244_logging_writers(i):
    """logging-writers"""
    rank = i % 5
    db = i % 2 == 0
    # app.db needs error (rank 3), the rest info (rank 1); a broadcast fails when one destination is full.
    passes = rank >= (3 if db else 1)
    kept = (1 if passes else 0) + 1
    return (kept + 10 * (1 if passes else 0) + 100 + 1000 + 10000 * (1 if i % 2 == 0 else 0)
            + (100000 if rank >= 3 else 0) + 1000000 + 10000000 * 3)


def _w244_logging_names_fields(i):
    """logging-names-fields"""
    x = str(i % 9 + 1)
    db = i % 2 == 0
    rank = i % 5
    name = "app." + ("db" if db else "web")
    return ((1 if name < "app.m" else 2) + 10 * len(name + ".q" + x) + 100 * 3 + 1000 * 2
            + (10000 if (rank >= 0 if db else rank >= 2) else 0) + (100000 if rank >= 3 else 0))


def _w244_logging_lines(i):
    """logging-lines"""
    import datetime as dt
    v = i % 9 + 1
    x = str(v)
    rank = i % 5
    names = ["DEBUG", "INFO", "WARNING", "ERROR", "CRITICAL"]
    when = dt.datetime(1970, 1, 1) + dt.timedelta(seconds=1700000000 + i, hours=1)
    name = "app." + ("db" if i % 2 == 0 else "web")
    fields = json.dumps({"password": "s" + x, "id": v}, separators=(",", ":"))
    line = '%s+01:00 level=%s name="%s" process=42 message="m%s" fields=%s' % (when.strftime("%Y-%m-%dT%H:%M:%S"), names[rank], name, x, fields)
    return (len(line) + 1000 + (10000 if rank < 2 else 20000) + 100000 * (1 if rank == 3 else 0) + 1000000 * len("svc.c" + x) + 10000000)


def _w244_logging_logger_rotation(i):
    """logging-logger-rotation"""
    v = i % 9 + 1
    debug = 1 if i % 3 == 0 else 0
    # The suffix is the RFC 3339 time with ":" written "-", then "-" and the index.
    suffix = "2023-11-14T22-13-20Z-%d" % v
    return debug + 1 + 1 + 10 * 0 + (100 if i * 600 >= 3600 else 0) + 1000 * len(suffix)


def _w244_logging_files(i):
    """logging-files"""
    x = str(i % 9 + 1)
    return len("one " + x + "\n") + len("two\n")


def _w244_logging_rotating_file(i):
    """logging-rotating-file"""
    x = str(i % 9 + 1)
    return len("r" + x + "\n") + len("s\n")


def _w244_os_environment_read(i):
    """os-environment-read"""
    v = i % 9 + 1
    x = str(v)
    # Booleans read true or false; "yes" is refused (InvalidEnvironmentValue).
    flag = {0: 100000000, 1: 0, 2: 300000000, 3: 0}[i % 4]
    return (len("/home/u" + x) + (100 if i % 2 == 0 else 0) + 1000 * len("n" + x) + (8000 + v) * 10000 + flag
            + 1000000000 + 10)


def _w244_os_search_path(i):
    """os-search-path"""
    v = i % 9 + 1
    count = i % 5 + 1
    last = len("/opt/p%d/bin%d" % (count - 1, v))
    return count + 10 * count + 100 * len("/opt/p0/bin%d" % v) + 1000 * last + 10000 * last + 1000000 + 10000000


def _w244_os_process_pipe(i):
    """os-process-pipe"""
    v = i % 9 + 1
    return 1 + 2 + 4 + 10 * len("cat %d\n" % v) + 0 + 100


def _w244_os_process_readers(i):
    """os-process-readers"""
    v = i % 9 + 1
    return len("ab%d!\nA" % v) + 0 + 1000 + 10000


def _w244_os_process_streams(i):
    """os-process-streams"""
    v = i % 9 + 1
    x = str(v)
    return 1 + 10 * (len("one" + x) + len("two")) + 100 + 1000 * len("xyz" + x) + 10000 * v + 1000000 * len("e" + x + "\n") + 10000000


def _w244_os_process_misc(i):
    """os-process-misc"""
    v = i % 9 + 1
    return 0 + 10 * len("r%d\n" % v) + 100


WORK244 = {
    'collections-views-bounds': _w244_collections_views_bounds,
    'collections-views-transform': _w244_collections_views_transform,
    'collections-views-reversed': _w244_collections_views_reversed,
    'collections-combinatorics': _w244_collections_combinatorics,
    'collections-set-union': _w244_collections_set_union,
    'collections-set-difference': _w244_collections_set_difference,
    'collections-set-edits': _w244_collections_set_edits,
    'collections-sorted': _w244_collections_sorted,
    'collections-ordering-search': _w244_collections_ordering_search,
    'collections-heap-deque': _w244_collections_heap_deque,
    'collections-frequencies': _w244_collections_frequencies,
    'digest-blake2b': _w244_digest_blake2b,
    'digest-blake2s': _w244_digest_blake2s,
    'digest-legacy-blocks': _w244_digest_legacy_blocks,
    'digest-sha2-small': _w244_digest_sha2_small,
    'digest-sha2-wide': _w244_digest_sha2_wide,
    'digest-sha3-small': _w244_digest_sha3_small,
    'digest-sha3-wide': _w244_digest_sha3_wide,
    'digest-shake': _w244_digest_shake,
    'digest-read': _w244_digest_read,
    'regex-pattern-queries': _w244_regex_pattern_queries,
    'regex-observation': _w244_regex_observation,
    'regex-match-groups': _w244_regex_match_groups,
    'regex-first-match': _w244_regex_first_match,
    'regex-prefix-match': _w244_regex_prefix_match,
    'regex-entire-match': _w244_regex_entire_match,
    'regex-replace-forms': _w244_regex_replace_forms,
    'toml-table-scalars': _w244_toml_table_scalars,
    'toml-table-lists': _w244_toml_table_lists,
    'toml-temporal': _w244_toml_temporal,
    'toml-maybe-scalars': _w244_toml_maybe_scalars,
    'toml-values-built': _w244_toml_values_built,
    'json-node-reads': _w244_json_node_reads,
    'json-node-shape': _w244_json_node_shape,
    'json-maybe-reader': _w244_json_maybe_reader,
    'json-maybe-presence': _w244_json_maybe_presence,
    'json-presence-reader': _w244_json_presence_reader,
    'json-streaming': _w244_json_streaming,
    'plist-xml-source': _w244_plist_xml_source,
    'plist-xml-text': _w244_plist_xml_text,
    'plist-dictionary-reads': _w244_plist_dictionary_reads,
    'plist-dictionary-maybe': _w244_plist_dictionary_maybe,
    'plist-maybe-more': _w244_plist_maybe_more,
    'xml-events': _w244_xml_events,
    'xml-node-build': _w244_xml_node_build,
    'xml-element': _w244_xml_element,
    'uuid-named': _w244_uuid_named,
    'uuid-custom': _w244_uuid_custom,
    'uuid-text': _w244_uuid_text,
    'url-parts-read': _w244_url_parts_read,
    'url-parts-edit': _w244_url_parts_edit,
    'url-query': _w244_url_query,
    'url-form': _w244_url_form,
    'db-value-capture-scalars': _w244_db_value_capture_scalars,
    'db-value-capture-text': _w244_db_value_capture_text,
    'db-value-capture-protocol': _w244_db_value_capture_protocol,
    'db-value-print': _w244_db_value_print,
    'db-value-kinds': _w244_db_value_kinds,
    'db-value-readers': _w244_db_value_readers,
    'db-value-native': _w244_db_value_native,
    'db-column-reader-typed': _w244_db_column_reader_typed,
    'db-page-reader': _w244_db_page_reader,
    'db-maybe-reader': _w244_db_maybe_reader,
    'db-maybe-presence': _w244_db_maybe_presence,
    'db-presence-reader': _w244_db_presence_reader,
    'db-statement-values': _w244_db_statement_values,
    'sqlite-session-execute': _w244_sqlite_session_execute,
    'sqlite-session-rows': _w244_sqlite_session_rows,
    'sqlite-session-prepare': _w244_sqlite_session_prepare,
    'sqlite-transaction-control': _w244_sqlite_transaction_control,
    'sqlite-transaction-work': _w244_sqlite_transaction_work,
    'sqlite-blob-session': _w244_sqlite_blob_session,
    'sqlite-script': _w244_sqlite_script,
    'sqlite-backup': _w244_sqlite_backup,
    'sqlite-failures': _w244_sqlite_failures,
    'sqlite-prepared-transaction': _w244_sqlite_prepared_transaction,
    'sqlite-acquisition': _w244_sqlite_acquisition,
    'db-pool-state-lease': _w244_db_pool_state_lease,
    'db-pool-state-retire': _w244_db_pool_state_retire,
    'db-pooled-connection': _w244_db_pooled_connection,
    'db-pooled-transaction': _w244_db_pooled_transaction,
    'db-pool-prepared': _w244_db_pool_prepared,
    'db-physical': _w244_db_physical,
    'db-query-forms': _w244_db_query_forms,
    'locale-subtag-shapes': _w244_locale_subtag_shapes,
    'locale-subtag-keys': _w244_locale_subtag_keys,
    'locale-extensions': _w244_locale_extensions,
    'locale-fields': _w244_locale_fields,
    'locale-filtered': _w244_locale_filtered,
    'locale-number-print': _w244_locale_number_print,
    'locale-number-data': _w244_locale_number_data,
    'locale-number-magnitude': _w244_locale_number_magnitude,
    'locale-number-printing': _w244_locale_number_printing,
    'locale-number-zeroes': _w244_locale_number_zeroes,
    'locale-case-currency': _w244_locale_case_currency,
    'locale-gregorian-names': _w244_locale_gregorian_names,
    'locale-localized-times': _w244_locale_localized_times,
    'locale-gmt-offset': _w244_locale_gmt_offset,
    'locale-cldr-gregorian': _w244_locale_cldr_gregorian,
    'locale-cldr-numbers': _w244_locale_cldr_numbers,
    'locale-preferred': _w244_locale_preferred,
    'crypto-aead-text': _w244_crypto_aead_text,
    'crypto-xchacha': _w244_crypto_xchacha,
    'crypto-ed25519-x25519': _w244_crypto_ed25519_x25519,
    'crypto-ecdsa-keys': _w244_crypto_ecdsa_keys,
    'crypto-jwt-keys': _w244_crypto_jwt_keys,
    'crypto-rsa-keys': _w244_crypto_rsa_keys,
    'crypto-rsa-pss-oaep': _w244_crypto_rsa_pss_oaep,
    'crypto-key-format': _w244_crypto_key_format,
    'crypto-hmac-secret': _w244_crypto_hmac_secret,
    'crypto-host-nonce': _w244_crypto_host_nonce,
    'crypto-x509-values': _w244_crypto_x509_values,
    'crypto-x509-extensions': _w244_crypto_x509_extensions,
    'crypto-x509-chain': _w244_crypto_x509_chain,
    'fs-path-forms': _w244_fs_path_forms,
    'fs-stat-values': _w244_fs_stat_values,
    'fs-file-bytes': _w244_fs_file_bytes,
    'fs-file-text': _w244_fs_file_text,
    'fs-entries': _w244_fs_entries,
    'fs-tree-links': _w244_fs_tree_links,
    'fs-temp-file': _w244_fs_temp_file,
    'fs-temp-finish': _w244_fs_temp_finish,
    'fs-root-stat': _w244_fs_root_stat,
    'mime-part-build': _w244_mime_part_build,
    'mime-parse': _w244_mime_parse,
    'mime-validate': _w244_mime_validate,
    'mime-parameters': _w244_mime_parameters,
    'mime-scanning': _w244_mime_scanning,
    'mime-types-map': _w244_mime_types_map,
    'mime-system-types': _w244_mime_system_types,
    'logging-writers': _w244_logging_writers,
    'logging-names-fields': _w244_logging_names_fields,
    'logging-lines': _w244_logging_lines,
    'logging-logger-rotation': _w244_logging_logger_rotation,
    'logging-files': _w244_logging_files,
    'logging-rotating-file': _w244_logging_rotating_file,
    'os-environment-read': _w244_os_environment_read,
    'os-search-path': _w244_os_search_path,
    'os-process-pipe': _w244_os_process_pipe,
    'os-process-readers': _w244_os_process_readers,
    'os-process-streams': _w244_os_process_streams,
    'os-process-misc': _w244_os_process_misc,
}
WORK244_PROGRAMS = frozenset(WORK244)


def work244_contribution(name, i):
    return rem(WORK244[name](i), 7)
# END #244 WORK

# BEGIN #244 TAIL WORK — independent Python arithmetic / stdlib oracles.
# All inputs use v=i%23+1 and x=str(v). No expected value comes from VM output.
# Encodings/checksums use Python codecs, zlib and binascii.crc_hqx; fractions,
# calendar/datetime, statistics and difflib supply independent reference models.
# Random distribution scenarios inject U=1/8. Box-Muller then gives
# Z=sqrt(-ln(7/8)); Marsaglia-Tsang accepts the first candidate at shapes 2/3.
# Gamma, beta and von Mises expressions below are the resulting mathematical
# values; integer sampling uses an explicit finite selection, PCG its published
# 64/32 sequence (pcg_integer16 above). No operating-system randomness is used.
# INI serializes quoted names/values; debug-dumper-work counts the literal
# documented delimiters of Probe/@int/%string:int plus each decimal value.
# time-zone-work uses Berlin in January 2024 (CET, +3600 seconds, no DST);
# fixed-clock cases derive all dates from the Unix epoch. No host zone is read.
# i18n-mo-membership uses a hand-built little-endian GNU MO revision-0 layout:
# two records, metadata and key; its one-byte translation is ASCII 65+v.
# Continuous results are reduced to coarse integers away from rounding borders.
# Pearson correlation of an exactly fitted line is rounded before reduction,
# because IEEE floating-point evaluation can produce a value just below 1.

TAIL244_ORACLES = {
    'numbers-bigint-arithmetic': lambda i, v, x: (v*7+1)-6+(v*7+1)*6+(v*7+1)//6+(v*7+1)%6+(v*7+1).bit_length()+1,
    'numbers-bigint-powers': lambda i, v, x: math.gcd(v,6)+math.lcm(v,6)+v**3+v**2,
    'numbers-rational-arithmetic': lambda i, v, x: sum(r.numerator+r.denominator+len(str(r.numerator)+"/"+str(r.denominator)) for r in (fractions.Fraction(v,3)+fractions.Fraction(2,5),fractions.Fraction(v,3)-fractions.Fraction(2,5),fractions.Fraction(v,3)*fractions.Fraction(2,5),fractions.Fraction(v,3)/fractions.Fraction(2,5)))+int(fractions.Fraction(v,3)<fractions.Fraction(2,5)),
    'math-decimal-arithmetic': lambda i, v, x: (4*v-1)+(4*v+1)+(4*v+1)+2*v+1,
    'math-aggregate-work': lambda i, v, x: 2*(v+7)+12*v+int(math.hypot(v,3,4))+v*v+5,
    'time-date-work': lambda i, v, x: 2*datetime.date(2024,1,v).toordinal()+v+63,
    'time-weekday': lambda i, v, x: datetime.date(2024,1,v).isoweekday(),
    'time-clock-work': lambda i, v, x: (2*v+1)*10**9+22,
    'time-duration-work': lambda i, v, x: (v-30)*10**9+len(x)+2,
    'time-local-work': lambda i, v, x: v*3600*10**9+v+41,
    'time-format-work': lambda i, v, x: 2*v+19,
    'time-zone-work': lambda i, v, x: 3600*10**9+6+v,
    'time-fixed-today': lambda i, v, x: (v-1)*86400*10**9+v,
    'strings-parts-work': lambda i, v, x: 144+2*sum(map(ord,x)),
    'strings-number-work': lambda i, v, x: 2*v+fractions.Fraction(v,3).numerator+int(v%2==0),
    'numbers-float-sign': lambda i, v, x: v if v<12 else 0,
    'checksum-state-work': lambda i, v, x: __import__("binascii").crc_hqx(bytes([v,255,0,v+1]),65535)+zlib.crc32(bytes([v,255,0,v+1]))+zlib.adler32(bytes([v,255,0,v+1])),
    'encoding-base-decode': lambda i, v, x: 2*(v+383),
    'encoding-byte-text': lambda i, v, x: v+2*len(x)+233+sum(map(ord,x)),
    'encoding-utf-work': lambda i, v, x: sum(map(ord,"é😀"+x))+2*len("é😀"+x)+sum(("é😀"+x).encode("utf-32-be")),
    'encoding-pem-cursor': lambda i, v, x: v+256,
    'unicode-numeric-value': lambda i, v, x: v+(3 if v%2==0 else 4),
    'text-layout-new': lambda i, v, x: 16+len(x),
    'config-settings-scalars': lambda i, v, x: 2*v+len(x)+2,
    'config-settings-layers': lambda i, v, x: v+4*len(x)+3,
    'config-key-index': lambda i, v, x: v+1,
    'config-overlay-work': lambda i, v, x: 2*len(x)+1+v,
    'config-value-work': lambda i, v, x: 6*len(x)+11,
    'config-document-work': lambda i, v, x: 2*len(x),
    'config-ini-write': lambda i, v, x: 4*len(x)+19,
    'debug-dumper-work': lambda i, v, x: 103+6*len(x),
    'io-reading-work': lambda i, v, x: 2*v+3,
    'io-buffer-writing': lambda i, v, x: 3*v+11+2*sum(map(ord,x)),
    'io-transfer-work': lambda i, v, x: len(x)+1+120+sum(map(ord,x)),
    'io-input-work': lambda i, v, x: v+9,
    'io-async-reading': lambda i, v, x: v+4,
    'io-async-writing': lambda i, v, x: v+4,
    'io-timed-reading': lambda i, v, x: v,
    'archive-metadata-work': lambda i, v, x: 2*v+2,
    'archive-budget-work': lambda i, v, x: 194-2*v+2*len(x),
    'archive-stream-read': lambda i, v, x: len(x)+3,
    'archive-stream-write': lambda i, v, x: len(x)+3,
    'archive-tar-stream': lambda i, v, x: len(x)+3,
    'archive-zip-compression': lambda i, v, x: v%10+9,
    'compression-native-write': lambda i, v, x: v+256,
    'compression-native-read': lambda i, v, x: v+256,
    'compression-output-work': lambda i, v, x: v+256,
    'compression-failure-work': lambda i, v, x: v,
    'net-failure-work': lambda i, v, x: 2*v+2,
    'net-ip-work': lambda i, v, x: 513+v,
    'net-interface-work': lambda i, v, x: len("10.0.0."+x+"/24")+(v//4)*4+len("10.0.0."+x+":"+str(8000+v))+1,
    'net-suffix-work': lambda i, v, x: 5+v,
    'random-continuous-work': lambda i, v, x: int((v+1)*1000)+int((v+2*math.sqrt(-math.log(0.875)))*1000)+int(-math.log(0.875)/2*1000)+int(math.exp(v/10+0.5*math.sqrt(-math.log(0.875)))*1000),
    'random-gamma-beta': lambda i, v, x: int(v*(5/3)*(1+math.sqrt(-math.log(0.875))/math.sqrt(15))**3*1000)+int(1000/(1+((8/3)*(1+math.sqrt(-math.log(0.875))/math.sqrt(24))**3)/((5/3)*(1+math.sqrt(-math.log(0.875))/math.sqrt(15))**3))),
    'random-tail-distributions': lambda i, v, x: int(v/math.sqrt(0.875)*1000)+int(v*math.sqrt(-math.log(0.875))*1000)+int(((v/10-math.acos((1/(0.25+math.sqrt(1.0625))+math.cos(math.pi/8))/(1+math.cos(math.pi/8)/(0.25+math.sqrt(1.0625)))))%(2*math.pi))*1000),
    'random-choices-work': lambda i, v, x: v+v%3+10*(v+1),
    'random-samples-work': lambda i, v, x: (v+[2,1,0][v%3])*2+20*(v+[k for k in [2,1,0] if k!=[2,1,0][v%3]][v%2])+100*(v+[k for k in [2,1,0] if k!=[2,1,0][v%3]][1-v%2]),
    'random-list-removal': lambda i, v, x: 3*v+3,
    'random-pcg-integer': lambda i, v, x: pcg_integer16(v),
    'message-header-work': lambda i, v, x: len(x)+4+sum(("X-Count: "+x+"\r\nx-count: next\r\n").encode()),
    'cli-argument-items': lambda i, v, x: 2*v+1,
    'cli-invocation-values': lambda i, v, x: len(x)+v+7,
    'cli-command-help': lambda i, v, x: len("Usage: tool"+x+"\nA tool\n")+len("complete -c 'tool"+x+"' -f\n"),
    'csv-stream-row': lambda i, v, x: len(x)+3,
    'template-write-work': lambda i, v, x: sum(("A"+x+"B"+x+"<p>"+x+"</p>").encode()),
    'template-sections-work': lambda i, v, x: sum((("yes" if v%2==0 else "")+x+"é"+x).encode())+30-len(x),
    'template-program-parts': lambda i, v, x: 5+2*len(x),
    'template-library-names': lambda i, v, x: 2*len(x)+2,
    'i18n-catalog-entries': lambda i, v, x: len(x)+2,
    'i18n-fallback-plural': lambda i, v, x: (3 if v==1 else 4)+1,
    'i18n-plural-operands': lambda i, v, x: v+58+int(2<=v%10<=4),
    'i18n-gettext-charset': lambda i, v, x: 1+len(x)+sum(("é"+x).encode("utf-8" if v%2==0 else "latin1")),
    'i18n-gettext-metadata': lambda i, v, x: 6+len(x),
    'i18n-gettext-rule': lambda i, v, x: int(v!=1),
    'i18n-po-build': lambda i, v, x: len('# '+x+'\nmsgid "key"\nmsgstr "'+x+'"\n'),
    'i18n-mo-membership': lambda i, v, x: 65+v+int(v%2==0),
    'statistics-sample-work': lambda i, v, x: 8+int(statistics.mean([v,v+1,v+2,v+4]))+int(statistics.geometric_mean([v,v+1,v+2,v+4]))+int(statistics.harmonic_mean([v,v+1,v+2,v+4]))+2*v+3+sum(map(int,statistics.quantiles([v,v+1,v+2,v+4],n=4))),
    'statistics-spread-work': lambda i, v, x: int(statistics.variance([v,v+1,v+4])*1000)+int(statistics.stdev([v,v+1,v+4])*1000)+int(statistics.pvariance([v,v+1,v+4])*1000)+int(statistics.pstdev([v,v+1,v+4])*1000),
    'statistics-pairs-work': lambda i, v, x: v+5,
    'statistics-modes-work': lambda i, v, x: 4*v+4,
    'calendar-month-items': lambda i, v, x: v%20+1+calendar.monthrange(2024,v%12+1)[1]+calendar.Calendar().monthdatescalendar(2024,v%12+1)[0][0].day+calendar.Calendar().monthdatescalendar(2024,v%12+1)[-1][-1].day+v+2,
    'calendar-iso-week': lambda i, v, x: datetime.date.fromisocalendar(2024,v,1).toordinal()+datetime.date.fromisocalendar(2024,v,7).toordinal()+1+int(calendar.isleap(2024+v)),
    'csv-record-values': lambda i, v, x: len(x)+2*v+2,
    'csv-record-next': lambda i, v, x: len(x),
    'csv-field-render': lambda i, v, x: 4*len(x)+24,
    'csv-dialect-work': lambda i, v, x: 4*len(x)+27,
    'diff-render-work': lambda i, v, x: len("".join(difflib.unified_diff(["start\n","old"+x+"\n","end\n"],["start\n","new"+x+"\n","end\n"],fromfile="left",tofile="right",n=1)))+len("".join(difflib.context_diff(["start\n","old"+x+"\n","end\n"],["start\n","new"+x+"\n","end\n"],fromfile="left",tofile="right",n=1))),
}
# END #244 TAIL WORK

# BEGIN #244 LAST — four actions no batch reached (hand-derived).
LAST244_ORACLES = {
    # "k<v>=<v><v>": the first \d+ is the one digit v; Number(v) + length 1.
    'regex-first-text': lambda i: ((i % 9 + 1) + 1) % 7,
    # Two sends then two next(): some(i%5), some(i%3).
    'channel-next': lambda i: (i % 5 + i % 3) % 7,
    # "data-<digit>" and "hidden": six characters each.
    'html-attribute': lambda i: (6 + 6) % 7,
    # The worker bound as given.
    'parallel-workers': lambda i: (i % 4 + 1) % 7,
}
# END #244 LAST
