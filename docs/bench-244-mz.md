# #244: std micro-benchmarks, m–z (2026-11-22)

Scope: this worktree only, supplied `release/062` context; no Git operations,
compiler/std changes, builds, Docker, clippy or CHANGELOG changes.

All 57 programs were created through `actions.py --scaffold MODULE:Object.action`,
then completed by hand and registered only with independent Python oracles.
The temporary `.oracle.py` stubs and unfinished scaffold files were removed.
Only explicit timed calls count; fixture constructors stay outside measurement.
Checksums below validate semantics, not performance or resistance to optimization.

## Counts

| Inventory | Before | After |
|---|---:|---:|
| Language programs | 64 | 64 |
| std/builtin programs | 60 | 117 |
| All programs | 124 | 181 |
| Measured public action identifiers | 132 | 264 |
| Modules with declared measured actions | 55 | 117 |
| Non-runtime modules | 435 | 435 |
| Public action identifiers | 6652 | 6652 |

57 previously unmeasured in-scope modules now have measurements. Five additional
supporting modules are explicitly measured: `collections/iteration`, `fs/path`,
`io/buffer`, `io/decorators` and `io/text`. These explain the module increase of 62.

## Programs and independent checks

Every row passed N=17 and N=31 with W=3, plus N=10031 with W=101.
The larger run crosses decimal digit widths, carries, port/options/input wrap
boundaries and fractional-second trimming cases. Values are equal VM/Python sums.

| Program | Measures | N=10031 | W=101 |
|---|---|---:|---:|
| `aggregate-extremes` | Min/max of three varying integers | 28512 | 282 |
| `message-header` | Header-name validation and header value construction/reading | 39045 | 193 |
| `mime-disposition` | Content-Disposition parsing and canonical rendering | 49076 | 294 |
| `mime-media-type` | Media type parsing and parameter lookup | 39045 | 193 |
| `mime-text-body` | UTF-8 MIME text-body encoding | 29014 | 92 |
| `mime-transfer` | Base64 MIME transfer encoding of a byte snapshot | 30110 | 314 |
| `mime-extension` | Built-in media-type lookup by a synthetic path extension (no file access) | 15047 | 152 |
| `mime-encoded-word` | RFC 2047 UTF-8 encoded-word rendering | 3000 | 303 |
| `dns-fixed-record` | Static DNS record construction and hostname observation (no resolver I/O) | 19053 | 61 |
| `host-endpoint` | Hostname/port endpoint construction and rendering | 19053 | 61 |
| `network-failure` | Portable network failure construction and diagnostic extraction | 49076 | 294 |
| `network-interface` | Synthetic interface snapshot validation and observation | 30099 | 310 |
| `ip-classification` | Built-in special-purpose IPv4 classification (TEST-NET-1) | 20062 | 202 |
| `multicast-options` | Multicast option construction and observation (no socket) | 29935 | 299 |
| `public-suffix` | Built-in Public Suffix List registrable-domain lookup | 5921 | 496 |
| `connection-endpoints` | Generic connection endpoint value construction and observation | 30093 | 303 |
| `tcp-options` | TCP listening-option validation and backlog observation (no listener) | 30042 | 297 |
| `tls-details` | Synthetic TLS negotiated-detail construction and protocol observation (no TLS I/O) | 5921 | 496 |
| `udp-datagram` | Synthetic UDP packet construction and payload observation (no socket) | 29854 | 297 |
| `unix-address` | Unix socket address validation/rendering (no socket or filesystem access) | 59004 | 395 |
| `bigint-add` | BigInt addition beyond machine integer width | 30093 | 309 |
| `bigrational-normalize` | Exact rational gcd normalization | 30087 | 295 |
| `shortest-float` | Shortest binary64 decimal conversion and coefficient observation | 30104 | 304 |
| `operator-adding` | Adding role dispatch to BigInt addition | 30093 | 303 |
| `plist-parse` | XML plist parsing and typed dictionary integer reading | 30093 | 297 |
| `plist-integer` | Large plist integer value construction and reading | 30093 | 300 |
| `plist-write` | XML plist serialization of a varying string | 29014 | 92 |
| `pcg-bytes` | Seeded PCG32 byte generation | 29624 | 301 |
| `seeded-uniform` | Seeded uniform integer sampling over a PCG32 source | 30063 | 260 |
| `regex-capture` | Regex capture extraction from a varying numeric match | 39045 | 193 |
| `regex-range` | Half-open regex range validation and length observation | 27777 | 276 |
| `regex-parts` | Lazy regex splitting and cursor traversal | 49076 | 294 |
| `sizing-role` | Sizing role dispatch over a varying-size array | 26960 | 268 |
| `text-template` | Text-template construction and substitution | 9652 | 590 |
| `html-template` | HTML-template rendering with escaping | 58890 | 395 |
| `html-template-library` | Named HTML-template library construction and lookup | 58890 | 395 |
| `template-json-value` | JSON scalar projection for template data | 39045 | 193 |
| `text-template-library` | Named text-template library construction and lookup | 29014 | 92 |
| `template-binding` | Template binding construction and typed-path lookup | 39045 | 193 |
| `template-path` | Typed template-path construction and joining | 58890 | 395 |
| `template-stream` | Template streaming into an in-memory byte buffer | 9652 | 590 |
| `template-sections` | Conditional template-section rendering | 34029 | 142 |
| `template-tree` | Template syntax-tree parsing and literal observation | 29014 | 92 |
| `terminal-size` | Terminal size validation and observation (no terminal) | 29787 | 297 |
| `terminal-failure` | Terminal failure construction and diagnostic extraction | 29014 | 92 |
| `terminal-color` | Indexed terminal-color code generation (no terminal output) | 8484 | 61 |
| `display-width` | Unicode display width with wide/combining characters | 9652 | 590 |
| `instant-shift` | Explicit instant arithmetic (no current-time read) | 30093 | 305 |
| `time-of-day` | Civil time validation and fractional-second formatting | 49028 | 490 |
| `duration-add` | Exact duration addition and nanosecond observation | 30093 | 305 |
| `iso-text` | ISO calendar-date parsing | 30093 | 300 |
| `local-datetime-shift` | Civil date-time arithmetic across day boundaries | 29085 | 294 |
| `rfc3339-parse` | RFC 3339 timestamp parsing into an explicit epoch duration | 30597 | 308 |
| `fixed-zone` | Fixed-zone offset construction and observation (no host zone/clock) | 30100 | 303 |
| `url-parts` | URL parsing and canonical serialization | 19053 | 61 |
| `uuid5-name` | Deterministic namespace/name UUID5 hashing | 29913 | 298 |
| `xml-name` | Namespaced XML-name construction and lexical rendering | 29014 | 92 |

The oracles use Python content/format rules, arbitrary-precision arithmetic,
`fractions.Fraction`, base64 and `uuid.uuid5`. The PCG oracle implements the
published PCG-XSH-RR 64/32 sequence and seed initialization from `random/source`.
No expected checksum was captured or inferred from VM results. XML plist output
is specified independently as UTF-8 XML text, including its header and final LF.

## Deliberate exclusions

- `math/constants`, `math/rounding`: no public callable targets in the inventory.
- `net/tcp_racing`: its useful observation opens real connections; no race is run.
- `parallel/map`: explicitly excluded by the task.
- `terminal/mode`, `terminal/prompt`, `terminal/screen`, `terminal/session`:
  useful operations require terminal state, terminal I/O or resize observation.
- All other socket/resolver/handshake operations, `SystemMediaTypes` file reads,
  current-time operations and unseeded randomness remain unmeasured.
- Already measured modules were retained; supporting calls are declared in the
  manifest, without claiming transitive implementation or overload coverage.

## Commands and results

Run from `/Volumes/SamsungSSD/stuff/okkama-work/l-codex2`.
The executable used was `/Users/shootnix/.local/bin/okkama`.

```sh
python3 bench/micro/check.py --okkama /Users/shootnix/.local/bin/okkama --stdlib /Volumes/SamsungSSD/stuff/okkama-work/l-codex2/std --jobs 2 --only aggregate-extremes,message-header,mime-disposition,mime-media-type,mime-text-body,mime-transfer,mime-extension,mime-encoded-word,dns-fixed-record,host-endpoint,network-failure,network-interface,ip-classification,multicast-options,public-suffix,connection-endpoints,tcp-options,tls-details,udp-datagram,unix-address,bigint-add,bigrational-normalize,shortest-float,operator-adding,plist-parse,plist-integer,plist-write,pcg-bytes,seeded-uniform,regex-capture,regex-range,regex-parts,sizing-role,text-template,html-template,html-template-library,template-json-value,text-template-library,template-binding,template-path,template-stream,template-sections,template-tree,terminal-size,terminal-failure,terminal-color,display-width,instant-shift,time-of-day,duration-add,iso-text,local-datetime-shift,rfc3339-parse,fixed-zone,url-parts,uuid5-name,xml-name
```

Exit 0: `All programs match their oracles.`
`Programs=57, N=17,31, engine=sparrow`; both measured and warmup sums checked.

The initial check exposed three benchmark mistakes (XML literal quoting, a named
`Utf8Output` constructor argument and a section-local variable lookup). They were
corrected before the successful complete check; no std or oracle was altered to
accept an incorrect VM value.

For the larger comparison, a temporary Python driver used two workers and ran
the following exact argument shape for each of the 57 names:

```sh
OKKAMA_STDLIB_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/std OKKAMA_TRACE=off /Users/shootnix/.local/bin/okkama run --timeout 60 /Volumes/SamsungSSD/stuff/okkama-work/l-codex2/bench/micro/std/NAME.ok 10031 101
```

Each child had a 90-second outer timeout; nonzero exit or stderr failed the run.
`run.parse_sample(stdout, 10031, 101, name)` compared both sums to
`oracles.expected(name, N)`. Exit 0:
`All 57 larger-N and warmup checks match independent oracles.`
The complete checksums appear above. No timing samples or speedup claims were made.

```sh
python3 -m unittest discover -s bench/micro -p 'test_*.py'
python3 bench/micro/actions.py --okkama okkama --stdlib std
python3 bench/micro/actions.py --okkama okkama --stdlib std --check
python3 bench/micro/coverage.py --stdlib /Volumes/SamsungSSD/stuff/okkama-work/l-codex2/std
python3 tools/map/gen.py
python3 tools/map/gen.py --check
```

Results: 24 Python tests passed; action generation/currentness and module coverage
generation passed; code map generation/currentness passed. README counts updated.

The optional cage preflight could not start: the surrounding sandbox forbids
`/bin/ps`, which the cage requires for process-tree/RSS monitoring. The requested
bounded host correctness checks succeeded. No cage/isolation or timing claim is made.
