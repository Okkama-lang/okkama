# HTML tokenizer migration (#13), 2026-11-22

The public `html/events` and `html/entities` API now uses Okkama. All twelve
native declarations, implementations and ABI entries remain untouched for the
owner to retire. No language, runtime, public std signature or CHANGELOG change.

Implementation and pure checks are complete. Three `web/app` integration tests
remain **blocked by the execution environment**, not green: binding a TCP
listener returns `permission_denied`. No Docker, cargo, clippy, commits, index
writes or remote mutations were used. See `tests.txt` for the complete results.

## Inventory and compatibility contract

Sources: `std/runtime/html.ok`, `std/html/events.ok`, `std/html/entities.ok`,
`okkama-rs/crates/okkama-runtime-core/src/native/markup.rs` at
`b6b10bdb7fc1ad299dcf71b349ed3ed2ced292f3`. This is the branch's initial HEAD;
`93a353d25` (CSV), `b6b10bdb7` (TOML), and the `947508635` / `26469c74b` stats
were inspected as the migration models, together with the migration matrix,
std principles, LLM guide and code maps.

| Native | Previous std use | Native observation |
|---|---|---|
| `html_events` | HtmlEvents constructor; UnescapedHtml.string | Eager immutable registry handle for all lexical events |
| `html_event_count` | HtmlEvents.cursor | Number of events |
| `html_event_kind` | HtmlEventCursor.item | 0 start, 1 end, 2 text, 3 comment, 4 declaration, 5 PI |
| `html_event_name` | start/end projection | ASCII-lowercase name |
| `html_event_value` | text/comment/declaration; UnescapedHtml | Exact payload |
| `html_event_target` | PI projection | Target, case preserved |
| `html_event_data` | PI projection | Data, without leading/trailing whitespace |
| `html_event_self_closing` | start projection | Final slash before optional whitespace |
| `html_attribute_count` | start projection | Includes duplicate names |
| `html_attribute_name` | attribute projection | ASCII-lowercase name |
| `html_attribute_valued` | attribute projection | Presence of `=`, independent of empty value |
| `html_attribute_value` | attribute projection | Attribute-context reference decoding |

Previously only `html/events` and `html/entities` called these natives. All
calls outside `std/runtime` are now gone. `EscapedHtml` was already Okkama and
is unchanged. Its direct callers are `template/engine`, `http/static` and
`http/xmlrpc`. The complete transitive caller set is in `callers.json`:
`template/html`, `template/html_library`, `template/sections`, `web/app`,
`web/around`, `web/reply`, `web/routing`, `web/session`, `web/testing`, plus
the direct modules and both HTML modules. There are no other `html/*.ok` files.

This is a lexical tokenizer, **not the WHATWG browser tree builder**. The
native's exact deterministic recovery, rather than browser behavior, is the
migration contract:

- Ordinary text runs to the next `<`. Invalid markup is text. Adjacent text
  pieces coalesce into one event, even across malformed tags or empty PIs.
  NUL, CRLF, non-ASCII characters and combining sequences are preserved.
- A start tag begins with an ASCII letter; its name continues to Unicode
  whitespace, `/` or `>`. Names lowercase ASCII only, preserving e.g. `İ`,
  `Ä` and emoji. The first unquoted `>` ends the tag. Both quote types are
  honored throughout the body, including malformed attribute syntax. An
  unmatched quote/no closing `>` makes the attempted tag literal text.
- A trailing slash followed only by Unicode whitespace marks self-closing
  and is removed before attribute scanning, even from an unquoted value.
  Attribute names end at whitespace or `=`; slash and quote can be name data.
  Empty names stop attribute scanning. Quoted values end at their own quote;
  unquoted values at whitespace. Duplicates and original order are retained;
  bare boolean attributes have `valued=false,value=""`, `a=` has `valued=true`.
- End tags seek the first `>` without quote handling. Leading whitespace is
  skipped; a valid ASCII-letter name is read; remaining body text is ignored.
- Comments consume `<!--` through the first `-->`, or EOF. No reference
  decoding or trimming occurs. `<!...>` is a declaration through the first
  `>`/EOF, with trimmed contents; there is no special doctype grammar or CDATA
  state. Thus CDATA syntax is a declaration payload.
- PIs prefer a later `?>` over an earlier `>`; otherwise end at `>`/EOF. Trim
  the body, split target from data at its first Unicode whitespace and trim
  leading data whitespace. An empty body produces ordinary decoded text.
- Only non-self-closing `script` and `style` enter raw text. `textarea` and
  `title` use the ordinary lexical rules. Raw content is copied unchanged
  until `</`, optional Unicode whitespace, the matching case-insensitive name
  and a whitespace, slash or `>` delimiter. The match itself does not require
  a later `>`. Normal end-tag parsing then resumes. Native Unicode per-scalar
  case comparison also matches long s and dotless i in these ASCII raw names;
  a resulting non-ASCII initial end-tag letter may still recover as text.
- Unicode whitespace is U+0009–000D, 0020, 0085, 00A0, 1680, 2000–200A, 2028,
  2029, 202F, 205F, 3000. U+001C and NUL are not whitespace.
- No implicit input, nesting, attribute, event or text-size limits; no syntax
  error class or source coordinates are exposed. Native event and attribute
  indices are zero-based. Recovery never throws a parser error.
- All 2,231 pinned named spellings are case-sensitive; longest match wins,
  maximum 32 name characters. Semicolonless matches in attributes are refused
  before ASCII alphanumeric or `=`. Numeric references use decimal or `x`/`X`
  hex, ASCII digits, optional semicolon, arbitrary leading zeros, U+FFFD for
  zero, surrogates or overflow, and the 27 HTML C1 substitutions. Digitless and
  unknown references remain literal. Decoding is one pass, never recursive.
- `UnescapedHtml` preserves literal markup by protecting `<` before using the
  same scanner. Empty source returns empty. Repeated observations agree.
- Document identity and cursor equality are preserved: distinct HtmlEvents
  snapshots compare unequal, same-document cursors at the same position compare
  equal, and event payloads compare structurally. A private captured action
  preserves snapshot identity without a native handle. Cursor item/successor
  past EOF still throw `IterationEnded.ended`; invalid unvalued HtmlAttribute
  still throws `InvalidArgument.because("an unvalued HTML attribute must have an empty value")`.

Native failures remain unchanged because the native layer remains unchanged:
`core.html / invalid_input`; invalid event index has operation
`runtime/html@1::html_event/2`, message `HTML event index is outside the stream`.
A wrong event field says `HTML event has no {name|value|target|data|self_closing}`;
wrong attribute event says `HTML event has no attributes`; bad attribute index
says `HTML attribute index is outside the tag`. Those last failures name the
actual native operation. There are no failure position fields beyond the
indices passed to those unchanged substrate calls.

## Implementation and tuning

`HtmlReader` is hidden in `html/events`: indexed `@char` Unicode scalars, span
slicing, persistent array append, direct construction of the public event and
attribute objects. Source is bound at construction and parsed by `cursor()`;
there is no I/O and no registry-owned handle left. A hidden visitor in
`html/entities` reads the single decoded text event. No new public helper or
module API is needed.

The entity map is a hidden constant, built once per process, generated directly
from the pinned `.dat`. `table.py` verifies all entries and the lexical/length
assumption used to stop named-reference searches at a delimiter. SHA-256:
`dd052ec076d43ce16cd2195d13552ae196b2294cd9f0ead3a59b17d9833916f7`.

Measured fixes (the intermediate raw series are retained as `tuning*.json`):

1. Array spread recopied the growing decoded-fragment list. Replacing it with
   persistent `+` append removed quadratic growth: large entities 4.43–4.51 s
   became 39–40 ms. Text is joined once; no per-character string concatenation.
2. Remove Cell wrapper allocations inside the scanner and repeated raw-mode
   name parsing. Direct local Atomic accumulators and raw-state selection at
   start-tag construction reduced large tokenization from 56–61 to 48–50 ms.
3. Use `origin.characters()` directly instead of mapping each char to an int;
   `.code()` is read only where a classification is needed. Large tokenization
   dropped to 40–43 ms.
4. Scan name/value/space spans with indexed loops instead of replacing an
   accumulator at every scalar. Return untouched spans without decoder setup.
   Large tokenization dropped to 37–39 ms.
5. Avoid lowercase assembly when the name contains no ASCII uppercase. Search
   the two fixed comment/PI terminators directly, with no object per character.
   Preserve document/cursor identity with a captured action and an immutable
   cursor. Final timings follow below.

These fixes change neither the language nor native code. Malformed quoted-tag
lookahead retains the native's rescan algorithm; repeated unterminated tag
candidates can still require quadratic lookahead. This is inherited behavior,
not a claim of linear time on every adversarial HTML string. Ordinary document
scanning and decoded-text accumulation are linear in their input/output size.

## Differential and tests

- `generate.py`, seed 131122: **7,205 inputs × 2 public paths = 14,410 comparisons,
  zero differences**. Both complete event payloads (ordered attributes, valued
  flags, self-closing flags, all six kinds) and unescaped text are JSON encoded.
  Every pinned entity spelling is exercised in text and attribute contexts.
  Numeric C1/scalar/overflow/leading-zero forms, malformed/truncated markup,
  raw names, duplicate and odd attributes, Unicode whitespace, emoji, NUL and
  CRLF are included, plus long text, 1,200 attributes and 2,000 nested tags.
- Native oracle is the unchanged original std projection, loaded from the
  fixed commit using read-only `git show`; candidate imports the current std.
  Both use the supplied binary and **the same absolute std directory**. Batches
  of 250 bound the native handle registry. Comparison uses LF boundaries only
  (Unicode line separators are legitimate JSON string data).
- 13 new inline tests: 11 in events, 2 in entities. They also pass against the
  original native projection (`characterization.txt`). Existing tests stay.
  The raw Unicode close test was corrected during development after both the
  native and candidate demonstrated the specified adjacent-text coalescing;
  no expectation was changed to excuse a candidate difference.
- All HTML and retained runtime/html tests pass: 14 + 8 + 2 = 24. Every
  transitive caller was attempted: **79 tests pass, 3 web/app tests are blocked**
  by `NetworkFailure.detailed(binding, permission_denied, "Cannot bind TCP listener")`.
  See `tests.json` and `tests.txt`; the runner exits nonzero for this limitation.
- Five manifest programs pass the independent `bench/micro/check.py` oracles
  for N=17 and N=31, warmup 3 (`micro-check.txt`).
- **23/23 mutations caught by the intended filtered inline test**, using scratch
  copies only. Syntax/semantic errors do not count as caught mutations.
  Every mutation, changed expression and failure output is in `mutations.json`:
  ASCII lowering; Unicode space; attribute ambiguity; self-closing slash;
  style raw mode; raw-close delimiter; Unicode raw-name matching; comment
  terminator; declaration trimming; PI target; quoted tag end; unquoted NUL;
  numeric zero; surrogate boundary; invalid-scalar replacement; C1 mapping;
  hexadecimal base; named-reference case; two-scalar entity; text flush;
  exhausted cursor; cursor snapshot identity; invented input limit.

## Reproduction

All commands run from `/Volumes/SamsungSSD/stuff/okkama-work/l-codex2`.
The scripts set `OKKAMA_STDLIB_DIR` themselves; direct commands use the export.
Scratch programs/corpus are outside the repository; only evidence is checked in.
Each standalone Okkama evidence run is bounded to 45 CPU seconds and 60 wall
seconds. Benchmarks run sequentially, never concurrently with each other.

```sh
export OKKAMA_STDLIB_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/std
HTML_BIN=/private/tmp/claude-501/-Volumes-SamsungSSD-stuff-okkama-new/36c16e3b-d3ea-4a70-8bd4-a8c12a2ab588/scratchpad/bin/okkama-13b
HTML_SCRATCH=/private/tmp/html-13

# Original baseline: executed BEFORE changing std (three runs of each case).
python3 docs/html-13/bench.py --okkama "$HTML_BIN" --scratch "$HTML_SCRATCH" --output docs/html-13/baseline.json
# To reproduce that baseline after the rewrite, add --native and use a new output.
python3 docs/html-13/check.py --okkama "$HTML_BIN" --scratch "$HTML_SCRATCH"
python3 docs/html-13/table.py
python3 docs/html-13/characterize.py --okkama "$HTML_BIN" --scratch "$HTML_SCRATCH"
python3 docs/html-13/mutate.py --okkama "$HTML_BIN" --scratch "$HTML_SCRATCH"
python3 docs/html-13/tests.py --okkama "$HTML_BIN"
"$HTML_BIN" fmt -w std/html/events.ok std/html/entities.ok docs/html-13/differential.ok
python3 bench/micro/check.py --okkama "$HTML_BIN" --stdlib "$OKKAMA_STDLIB_DIR" --only html-events,html-escape,html-unescape,html-template,html-template-library
python3 docs/html-13/bench.py --okkama "$HTML_BIN" --scratch "$HTML_SCRATCH" --output docs/html-13/final.json
# Longer batches to investigate the short-batch timing outlier, all samples kept:
python3 docs/html-13/bench.py --okkama "$HTML_BIN" --scratch "$HTML_SCRATCH" --native --steady --output docs/html-13/confirmation-native.json
python3 docs/html-13/bench.py --okkama "$HTML_BIN" --scratch "$HTML_SCRATCH" --steady --output docs/html-13/confirmation-okkama.json
python3 tools/map/gen.py
```

`bench.ok.in` uses the same warmed `run N WARMUP` / micro-v2 clock protocol as
`bench/micro/std/*.ok`. Its scalar checksums are independently asserted: 11
events or 26 decoded characters per repeated fixture. All input construction
and reporting are outside the measured interval. Tokenizer time includes
creation and full public cursor traversal. Entity time includes `string()` and
length observation. Warmup covers constant-table initialization/JIT. The large
fixture contains thousands of nested section/p/b groups, attributes, Unicode,
entities, boolean/quoted/unquoted values and comments; byte sizes are recorded.

## Timing results

### Initial baseline and final short batches

All times are **µs/call**; each cell lists all three runs in order.

| Case | UTF-8 bytes | N/warmup | Native, three runs | Okkama, three runs | Median ratio | Max final / min native |
|---|---:|---:|---|---|---:|---:|
| events-small | 153 | 2000/200 | 11.331 / 8.748 / 8.654 | 9.959 / 10.038 / 14.668 | 1.147× | 1.695× |
| events-medium | 5049 | 100/10 | 269.588 / 267.860 / 267.243 | 315.391 / 323.700 / 309.326 | 1.177× | 1.211× |
| events-large | 511938 | 2/1 | 27,006.145 / 27,276.979 / 27,080.896 | 35,093.979 / 35,359.938 / 34,990.312 | 1.296× | 1.309× |
| entities-small | 62 | 2000/200 | 4.173 / 4.050 / 4.164 | 4.432 / 4.430 / 4.650 | 1.064× | 1.148× |
| entities-medium | 5084 | 100/10 | 407.556 / 394.427 / 406.498 | 287.742 / 304.239 / 300.498 | 0.739× | 0.771× |
| entities-large | 511996 | 2/1 | 41,139.771 / 41,071.292 / 41,487.666 | 29,418.146 / 30,344.666 / 28,623.688 | 0.715× | 0.739× |
| html-escape | — | 2000/200 | 0.498 / 0.531 / 0.498 | 0.501 / 0.511 / 0.514 | 1.025× | 1.032× |
| html-unescape | — | 2000/200 | 1.325 / 1.326 / 1.333 | 1.778 / 1.964 / 1.837 | 1.385× | 1.482× |
| html-template | — | 2000/200 | 1.713 / 1.749 / 1.755 | 1.780 / 1.838 / 1.812 | 1.036× | 1.073× |
| html-template-library | — | 2000/200 | 0.855 / 0.847 / 0.861 | 0.865 / 0.879 / 0.862 | 1.011× | 1.037× |
| html-events | — | 2000/200 | 4.164 / 4.068 / 4.036 | 3.169 / 3.115 / 3.035 | 0.766× | 0.785× |

### Confirmation with longer batches

All times are **µs/call**; each cell lists all three runs in order.

| Case | UTF-8 bytes | N/warmup | Native, three runs | Okkama, three runs | Median ratio | Max final / min native |
|---|---:|---:|---|---|---:|---:|
| events-small | 153 | 10000/1000 | 9.402 / 9.509 / 9.567 | 9.822 / 9.926 / 9.996 | 1.044× | 1.063× |
| events-medium | 5049 | 300/30 | 288.048 / 283.307 / 283.002 | 323.830 / 321.096 / 311.885 | 1.133× | 1.144× |
| events-large | 511938 | 5/2 | 29,476.767 / 28,199.417 / 28,553.533 | 33,919.042 / 35,217.875 / 35,546.867 | 1.233× | 1.261× |
| entities-small | 62 | 10000/1000 | 4.213 / 4.197 / 4.269 | 4.374 / 4.150 / 4.220 | 1.002× | 1.042× |
| entities-medium | 5084 | 300/30 | 437.586 / 430.692 / 431.116 | 309.718 / 317.402 / 283.154 | 0.718× | 0.737× |
| entities-large | 511996 | 5/2 | 43,314.808 / 43,568.133 / 42,889.292 | 30,053.758 / 30,357.242 / 30,366.208 | 0.701× | 0.708× |
| html-escape | — | 10000/1000 | 0.356 / 0.343 / 0.351 | 0.329 / 0.312 / 0.338 | 0.937× | 0.986× |
| html-unescape | — | 10000/1000 | 1.316 / 1.362 / 1.353 | 1.616 / 1.630 / 1.530 | 1.195× | 1.239× |
| html-template | — | 10000/1000 | 1.645 / 1.684 / 1.706 | 1.608 / 1.641 / 1.592 | 0.955× | 0.997× |
| html-template-library | — | 10000/1000 | 0.763 / 0.832 / 0.811 | 0.764 / 0.784 / 0.739 | 0.941× | 1.028× |
| html-events | — | 10000/1000 | 4.297 / 4.275 / 4.271 | 2.842 / 2.940 / 3.064 | 0.688× | 0.718× |

The first final short-batch series contains a small-tokenizer outlier:
14.668 µs versus 9.959/10.038 µs in the other two runs. Its ratio to the best
initial native sample is 1.695×, which is **not** excused as nanosecond noise.
The cause of that isolated timing was not established. Both implementations
were therefore remeasured with longer identical batches, retaining every
sample and the original series. N/warmup increased from 2000/200 to 10000/1000,
100/10 to 300/30, and 2/1 to 5/2. No code changed between the short final and
confirmation series; checksums agree for both versions.

**Performance verdict: within the 1.5× rule in the confirmation series on every
size and every manifest HTML program**, including the conservative max-final /
min-native comparison (maximum 1.261×, large events). Median tokenizer ratios
are 1.044× / 1.133× / 1.233×; median entity ratios 1.002× / 0.718× / 0.701×.
Large tokenization adds 6.664 ms/call at the median; large entity decoding saves
12.958 ms/call. The original short-batch series and its outlier remain visible.
The Okkama implementation is selected; natives stay physically in place for the
owner. Integration validation is still limited by the three TCP-bind refusals.

