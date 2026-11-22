# CSV native migration (#13), 2026-11-22

Implemented in `feature/13-csv`, baseline `2046083434eda07103f3c7aedd5bd58208c7da1b`.
The supplied `okkama-sqlite` binary is unchanged; no Rust build, language or
public std API change. All eight native declarations and implementations remain
available for the owner to retire. Production std no longer calls them.

## Inventory and semantics

`std/runtime/csv_state.ok` declares `CsvState` and eight operations implemented
by `Csv` in `okkama-rs/crates/okkama-runtime-core/src/native/formats.rs`:

| Native | Operation / std callers before migration |
|---|---|
| `csv_state_create` | Bounded state; CsvScanned, CsvAsyncRows, CsvStream constructors |
| `csv_state_feed` | Append requested bytes; all three readers |
| `csv_state_advance` | need / row / eof / stable failure; all three readers |
| `csv_state_row` | Take announced row and reset status to need; all three |
| `csv_state_offset` | Error coordinate; all three |
| `csv_state_request` | min(4096, remaining fed-byte budget + 1), at least 1; all three |
| `csv_state_closed` | CsvStream.closed() |
| `csv_state_close` | Clear buffers, set closed and offset 0; CsvScanned cleanup, CsvStream.close() |

The native scanner only understands comma, double quote, CRLF. `CsvRows` used
it for the default dialect. Other dialects already used `CsvPrefix` in Okkama;
that path remains unchanged. `CsvInput` collects bounded bytes, strictly decodes
UTF-8, then uses CsvRows. `CsvRecords` wraps CsvRows for text and CsvAsyncRows for
AsyncReading. CsvStream supports Reading, AsyncReading and text. Searching all
std imports finds only `csv/input` and `csv/stream` as external callers of csv.
No CSV native participates in serialization: CsvString, CsvField, CsvRecord,
CsvDialect and their print helpers already write in Okkama.

- Delimiter and quote are distinct single Unicode scalars, neither CR nor LF.
  Only LF or CRLF is a valid dialect line ending; bare CR is never an ending.
  Stream APIs have no dialect option and retain strict CRLF. Inside quotes,
  CR, LF and CRLF are literal data. Doubled quotes are the only escaping.
- Quotes may open only at field start. After a closing quote only a doubled
  quote, separator, ending or EOF is valid. Spaces are data, never trimmed.
- Empty input has no rows. An ending alone has one empty field. Final ending
  is optional; a trailing delimiter adds an empty field. Open quote or pending
  CR at EOF is syntax failure. BOM, NUL and other controls are ordinary data.
- Input and decoded field budgets count Unicode scalars. Decoded doubled
  quotes count once. Rows and columns are bounded, zero limits meaningful,
  negative constructor limits raise `InvalidCsvLimits.invalid`. Byte limits
  count UTF-8 bytes. Stream native limits >= 2^62 clamp to 2^62-1; preserved.
- `CsvRows` checks total input length before scanning and validates the whole
  document before exposing any row. Repeated cursors use its memoized rows.
  Default grammar checks columns when finishing a field, syntax at the next
  scalar after CR, then row count after LF. At EOF columns precede rows.
  Custom grammar retains its pre-existing independent error precedence.
- `InvalidCsv.at(n)` uses the original zero-based scalar coordinate, including
  EOF. `InvalidUtf8.at(n)` uses the first byte of the malformed scalar, including
  incomplete final sequences. No normalization or replacement characters.
  UTF-8 rejects stray continuations, overlongs, surrogates and > U+10FFFF.
- Stream precedence follows the native: exhausted byte budget; invalid lead;
  incomplete scalar (fed-byte overflow before EOF/UTF8); scalar byte budget;
  malformed continuation/range; input budget; grammar and decoded-field budget.
- Public error texts stay `InvalidCsv.at(n)`, `InvalidUtf8.at(n)`,
  `CsvLimitExceeded.kind(CsvLimit.input|field|rows|columns)`,
  `CsvByteLimitExceeded.exceeded`, `EndOfCsv.reached`, `ClosedCsvStream.closed`,
  `AsyncCsvOrigin.walked`. CsvRecords maps a byte failure to CsvLimit.input,
  while CsvStream uses CsvByteLimitExceeded. CsvInput preserves its separate
  byte/decode/input validation order. Existing serializer errors are unchanged.
- Stream rows may escape before later failure. EOF and parser errors repeat
  until close. close is idempotent, clears parser storage and never closes the
  borrowed source. CsvStream closes on source failure/oversized read, preserving
  the original error (`CSV origin returned more bytes than requested`).
  CsvAsyncRows preserves its existing source-failure behavior, without adding
  closure. Only an empty source chunk means EOF, never a short nonempty chunk.
- UTF-8, doubled quotes and CRLF may split at any byte boundary. The scanner
  retains the current row and at most 4099 input bytes, not previous rows.
  Private native protocol misuse errors remain in the untouched natives:
  `CSV limits must be nonnegative` / `Invalid CSV state operation`.

## Implementation decisions

`CsvDefaultPrefix` scans a single array of scalar values and retains source
spans; completed fields are sliced once and doubled quotes decoded once.
It remains pure, so `CsvRows.records()` can keep `once let`: the compiler
correctly refuses a memoized computation that uses an Atomic-backed scanner.
No UTF-8 encode/decode round trip is needed for in-memory parsing.

`CsvScanner` indexes byte chunks, validates UTF-8 inline, materializes only
completed spans/fields with the retained UTF primitive, and preserves its
state across reads. No ordinary-character strings are allocated. Coordinate
updates are batched; ordinary fields skip fragment accumulation. The first
stream prototype was ~1.6x on medium/large input. Removing per-byte updates of
three counters and fusing field finalization brought all sizes below 1.5x;
[tuning samples](tuning.json) preserve those experiments.

Both implementations and all helpers are hidden. Because hidden declarations
cannot be imported and csv/stream already depends on csv's public error types,
exporting a shared helper or importing the stream back into csv would change
the API or introduce a cycle. The byte scanner has one maintained source block
in csv.ok, copied mechanically into csv/stream.ok by
`tools/csv/sync-scanner.py`; `--check` verifies identity. This explicit generated
copy keeps the public API unchanged. There is no new public scanner module.

## Measurements and acceptance

All values below are microseconds per complete operation, including the same
public-API setup and row consumption. Fixture construction and UTF-8 fixture
encoding are outside the timed interval. Each batch has an explicit warmup and
checksum. Every row has ten fields (quoted separators, doubled quotes, an
embedded LF, and non-ASCII text). Sizes: 1, 100 and 10000 rows; N/WARMUP:
10000/1000, 200/20, 3/1. Stream chunks repeat 1, 2, 7, 4093 bytes, bounded by
request(), deliberately splitting syntax and UTF-8. The expected checksum is
13 * rows * N (and independently 13 * rows * WARMUP).

Baseline was measured before changing std. Final samples were taken after
formatting and restoring all mutations. [Baseline](baseline.json),
[final](final.json) retain the exact micro-v2 output, including elapsed ns.

| Path / rows | Native, three runs (µs/call) | Okkama, three runs (µs/call) | Median ratio | Median delta (µs) |
|---|---|---|---|---|
| parse-small | 2.807, 2.784, 2.815 | 1.910, 1.760, 1.884 | 0.671× | -0.923 |
| parse-medium | 192.774, 194.432, 194.438 | 138.259, 137.542, 133.443 | 0.707× | -56.890 |
| parse-large | 22211.945, 20714.708, 20704.986 | 18797.639, 17937.597, 17766.764 | 0.866× | -2777.111 |
| stream-small | 3.825, 3.881, 4.173 | 4.471, 4.459, 4.856 | 1.152× | +0.590 |
| stream-medium | 195.280, 207.707, 195.509 | 243.489, 260.989, 259.308 | 1.326× | +63.799 |
| stream-large | 20227.236, 20142.625, 20173.417 | 25768.486, 25641.847, 24182.139 | 1.271× | +5468.430 |

Additional manifest programs, measured in the confirmation A/B run:

| Path / rows | Native, three runs (µs/call) | Okkama, three runs (µs/call) | Median ratio | Median delta (µs) |
|---|---|---|---|---|
| csv-input | 4.528, 4.317, 4.435 | 2.803, 2.647, 2.502 | 0.597× | -1.788 |
| csv-stream | 5.350, 5.378, 5.401 | 5.485, 5.487, 5.483 | 1.020× | +0.107 |
| csv-round-trip | 5.654, 5.444, 5.485 | 3.660, 3.915, 3.656 | 0.667× | -1.825 |

**Accepted under the owner's 1.5x rule:** all six size/path combinations pass,
including comparing the slowest final run with the fastest native run (worst
1.337x, medium stream). Largest absolute regression is +5.468 ms per 10000-row
stream operation at the medians, within its 30.260 ms budget; in-memory parsing
improves at every size. No appeal to nanosecond noise is needed. A separate
[confirmation A/B](confirmation.json), using baseline modules materialized by
read-only `git show`, repeats all sizes and includes the three manifest programs;
its worst median ratio is 1.362x. Serialization itself was never native.

## Differential, tests and mutations

[Generator](generate.py), seed 130622: **6625 distinct parameter sets** including
valid input, insertion/truncation corruption, six dialect configurations
(comma/double quote/CRLF; semicolon/apostrophe/LF; TAB/double quote/CRLF;
emoji/section sign/LF; NUL/é/CRLF; pipe/backslash/LF), random independent limits,
invalid UTF-8, random chunk plans, and 25 long-boundary cases at 1/2/3/7/4096-byte
reads. All three dialect options vary. Existing tests also reject invalid
dialect configurations.

[Probe](differential.ok) compares **four public paths**: CsvRows, synchronous
CsvStream.row, async CsvStream.next, async CsvRecords.next. It compares complete
rows/serialized records and exact error texts, previously emitted rows, repeated
errors/EOF and close behavior. **26500 comparisons, zero differences.**
[Counts and matching SHA-256](differential.json): text 4251 successes, 1173 syntax
and 1201 limit failures; sync stream 1762 EOFs, 2382 syntax, 720 UTF8, 946 byte
and 815 other limit failures. These totals refer to cases, not individual rows.

All **27 inline tests** pass: csv 11, csv/stream 9, csv/input 7. New tests were
also run against the native implementation as characterization. Both CSV
examples match their expected output. The declaration-only runtime/csv_state
module has no inline tests (`run -t` correctly refuses an empty test suite).
All three micro programs match their independent oracles at N=17 and N=31,
WARMUP=3. Public AST declarations/signatures of csv and csv/stream were compared
against their original modules (constructor/action bodies and private state
excluded), with no differences. Native source/declarations remain unchanged; search finds no csv_state calls outside std/runtime.

[Mutation runner](mutate.py), [individual failures](mutations.json): all 13
mutants compile and are killed by behavioral assertions; source restored in
`finally`, then the full focused checks pass again.

| Mutation | Detecting behavior |
|---|---|
| default delimiter comma -> plus | exact field arrays and limits |
| default CR requires CR instead of LF | valid CRLF rows and scalar offsets |
| remove default quote undoubling | exact decoded quote/Unicode/control field |
| allow extra default final column | columns-before-rows at EOF and CRLF |
| discard incomplete UTF8 tail at feed | every byte cut and split UTF8 errors |
| stream CR requires CR instead of LF | CRLF and original scalar coordinates |
| reject exact byte bound (`>` -> `>=`) | exact 12-byte record budget |
| permit excess input scalar (`>=` -> `>`) | input error, repeated unchanged |
| permit excess decoded field | doubled quote / multibyte field budget |
| permit excess row | zero row budget after valid CRLF |
| permit excess final column | zero column budget at field completion |
| syntax offset + 1 | exact scalar offset after multibyte text |
| accept UTF8 surrogate range | InvalidUtf8.at(4), repeated unchanged |

## Reproduction commands

Run from this worktree; every Okkama invocation uses the absolute current std.
`check.py` writes only generated files outside the repository, reads the fixed
baseline using `git show` (no checkout/index/commit), and runs both versions
against the same binary and current non-CSV std dependencies.

```sh
export OKKAMA_STDLIB_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/std
CSV13_BIN=/private/tmp/claude-501/-Volumes-SamsungSSD-stuff-okkama-new/36c16e3b-d3ea-4a70-8bd4-a8c12a2ab588/scratchpad/bin/okkama-sqlite
python3 docs/csv-13/check.py --okkama "$CSV13_BIN" --scratch /private/tmp/csv13/reproduce --bench
python3 docs/csv-13/mutate.py --okkama "$CSV13_BIN" --scratch /private/tmp/csv13/final-mutations
"$CSV13_BIN" fmt -w std/csv.ok std/csv/stream.ok
"$CSV13_BIN" run -t std/csv.ok
"$CSV13_BIN" run -t std/csv/stream.ok
"$CSV13_BIN" run -t std/csv/input.ok
"$CSV13_BIN" run examples/csv/rows.ok
"$CSV13_BIN" run examples/bible/22-csv.ok
python3 bench/micro/check.py --okkama "$CSV13_BIN" --stdlib "$OKKAMA_STDLIB_DIR" --only csv-input,csv-stream
python3 bench/micro/check.py --okkama "$CSV13_BIN" --stdlib "$OKKAMA_STDLIB_DIR" --only csv-round-trip
python3 tools/csv/sync-scanner.py --check
python3 tools/map/gen.py
python3 tools/map/gen.py --check
```

Original pre-change timings used the six generated `parse-{small,medium,large}`
and `stream-{small,medium,large}.ok` programs under `/private/tmp/csv13`, with
`run N WARMUP` as listed above, three runs each. The checked-in template and
check.py recreate them. No cargo, Docker, clippy, network or whole-project test
suite was run. The existing cage failed before launch because this managed
sandbox denies `/bin/ps`; the replacement runner stays in the active sandbox
and bounds each child by 45 CPU seconds and 60 wall seconds.

## Completion tracker

- [x] Inventory and observable semantics.
- [x] Three pre-change baseline runs per size/path.
- [x] Pure hidden scalar parser and streaming byte state machine; public API intact.
- [x] Investigate and fix initial streaming slowdown; final and confirmation measurements.
- [x] At least 5000 generated cases, four paths, zero differences.
- [x] Characterization tests, all 13 mutations caught, focused tests/examples/oracles pass.
- [x] Retain all eight natives/declarations; eliminate their std callers.
- [x] Update migration matrix/results and code maps; produce unified patch without staging.

No known remaining work in the assigned migration scope. Native retirement
remains the owner's decision, explicitly outside this change.
