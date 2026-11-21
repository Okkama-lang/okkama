# Format natives: migration matrix (#13)

The owner's review of #13 (v0.7.0) is the spec: each format or encoding native
is rewritten in Okkama, kept native, removed or deferred, on evidence. The
acceptance target for a rewrite is at most 1.5× the native time on its
benchmark, judged in absolute numbers. Crypto, TLS, X.509 and PEM stay native
(decision of 2026-11-07). `config/ini` was rewritten in v0.4.1 (973940c5) and
is not revisited.

The inventory below is from the repository at the start of the work
(`okkama-runtime-abi/src/table.rs`, the `runtime/*.ok` declarations and a search
of `std/` outside `std/runtime` for each native's callers). "Sparrow" means the
native has its own provider in `sparrow-core/src/host`; "origin" means it is
bridged to `okkama-runtime-core` through `host/native_origin.rs`, with values
converted on each call. Sparrow is the only engine (the IR engine and the JVM
are gone), so "all backends" means Sparrow plus the origin implementation.

Dispositions:
- **remove**: no std caller is left; the native goes from the ABI table,
  runtime-core and sparrow-core, and its ID is retired.
- **rewrite**: an Okkama implementation is a candidate; it is measured against
  the native first and accepted only within the target.
- **retain**: kept native, for the reason given.
- **defer**: belongs to the v0.7.0 std redesign of its module; measured then,
  so that no API due for replacement is ported.

| Module | Natives (used) | Callers in std | Implementation | Disposition | Reason, risk | Tests needed |
|---|---|---|---|---|---|---|
| template | 10 (1) | template/compile, engine, json, sections use `template_output_size` | origin (`formats.rs`) | **remove** the 9 `template_compile_*`/`template_node_*`; **retain** `template_output_size` | The compiler natives are dead since the scanner moved to Okkama (#178). `template_output_size` measures UTF-8 bytes without encoding: a primitive, retained. | runtime/template tests of the removed natives go with them |
| json | 50 (41) | json | Sparrow (`native_json`) and origin (`json.rs`) | **remove** the 9 unused (`json_member_name`, `json_quote`, `json_number_text`, `json_output*`); **retain** the rest | The parser and node store are the hot path of every JSON user; a port is far beyond 1.5×. The output natives were replaced by Okkama writing. | json std tests |
| bigint | 13 (2) | numbers/bigint (`bigint_valid`, `bigint_bits`) | Sparrow (`native_numbers`) and origin (`numbers.rs`) | **remove** the 11 arithmetic natives; **retain** `bigint_valid`, `bigint_bits` | Arithmetic moved to builtins (#511). | numbers/bigint std tests; provider tests that call the removed natives |
| uuid | 8 (8) | uuid/value, uuid/generation | origin (`value.rs`) | **rewritten**: `uuid_valid`, `uuid_bytes`, `uuid_string`, `uuid_version`, `uuid_variant`, `uuid_custom`, `uuid_versioned`; **retain** `uuid_named` | Fixed 16-byte inputs on `encoding/hex` and bit operations. `uuid_versioned` only sets the version and variant bits of given bytes (the entropy comes from `random/source`). `uuid_named` stays: through `digest/legacy` (MD5/SHA-1 in Okkama) a named UUID costs 8.5 µs (v3) and 23 µs (v5) against 0.45 µs native. | done: canonical text, bit-prefix and UUID 8 layout tests in uuid/*; differential over 12 000 values |
| url_text | 1 (1) | url/value (`url_dot_segments`) | origin | **rewritten**, native removed | RFC 3986 §5.2.4 on short paths; string work. A path without dot segments is returned as it is. | done: §5.2.4 cases and limits in url/value; differential over 4 000 generated paths |
| layout | 4 (4) | text/layout, debug/dumper, terminal/screen, archive, compare, compression | origin | **rewrite** (measure) | Text wrapping and indentation; moderate inputs; widths depend on `unicode_character` display widths, which stay native. | differential tests with Unicode widths |
| diff | 1 (1) | diff | origin | **rewrite** (measure) | Myers diff; risk on large inputs (O(ND) must hold). | differential test against the native script on generated sequences |
| time_format | 5 (5) | time/format | origin (`time.rs`) | **defer** | Pattern formatting and parsing of the time module, redesigned in v0.7.0. | — |
| hex, base32, base64, base85, quoted_printable | 15 (15) | encoding/* (and mail, http through them) | Sparrow (`native_text`, `native_encoding`) for hex and QP; origin for base32/64/85 | **rewrite** (measure, hex first) | Per-byte loops: the native is a tight Rust loop, so a port may exceed 1.5× on large inputs; measured before deciding. | differential tests including invalid input offsets |
| hpack | 1 (1) | http/internal/hpack_primitives | origin | **retain** | Bit-level Huffman decoding on the HTTP/2 hot path. | — |
| toml | 16 (16) | toml/parse | origin (`markup.rs`) | **defer** | Measured in 2026-11 at 2–5× native for a port; the toml module is redesigned in v0.7.0. | — |
| csv_state | 8 (8) | csv, csv/stream | origin (`formats.rs`) | **defer** | Streaming parser state; csv is redesigned in v0.7.0; a port is expected beyond 1.5×. | — |
| html | 12 (12) | html/events, html/entities | origin (`markup.rs`) | **defer** | HTML tokenizer; expected beyond 1.5×; html is redesigned in v0.7.0. | — |
| xml | 11 (11) | xml/parse, plist/parse | origin (`xml.rs`) | **defer** | XML tokenizer; same as html. | — |
| utf, single_byte, bytes, binary, number_text, decimal, buffer | 51 (all) | foundation modules | Sparrow | **retain** | Primitives the Okkama codecs are built on (#13 builds on `encoding/bytes` and `io/input`). | — |
| checksum, compression | 8 (8) | checksum/*, compression/* | origin | **retain** | CRC tables in tight loops; zlib/brotli codecs. | — |
| unicode_character, unicode_normalization, idna, public_suffix, cldr, collation | 16 (16) | unicode/*, net/*, locale/* | origin | **retain** | Data tables (Unicode, PSL, CLDR) kept by the host. | — |
| statistics, math_aggregate, random_distribution, format | 18 (18) | statistics, math/*, random/*, numbers/float | origin | **retain** | Float math through libm (`sqrt`, `log`, …) and `sprintf` formatting. | — |
| regex | 20 | regex | Sparrow | **retain** | A regex engine. | — |
| crypto, host_crypto, secret, pem, x509, tls_transport | 43 | crypto/*, net/tls | Sparrow and origin | **retain** | Owner's decision of 2026-11-07. | — |
| ini | — | config/ini | Okkama | done in v0.4.1 | — | — |

## Results

Each module's work is a separate change with its numbers. Rows above are
updated as modules land.

| Module | Change | Native | Okkama | Decision |
|---|---|---|---|---|
| template | 9 dead natives removed | — | — | removed |
| json | 9 dead natives removed (`json_member_name`, `json_quote`, `json_number_text`, `json_output*`) | — | — | removed |
| bigint | 11 dead arithmetic natives removed | — | — | removed |
| url_text | `UrlDotSegments` in Okkama (a persistent list as the stack, a dot-free path returned as it is); `url_dot_segments` and the `runtime/url_text` module removed | plain 0.36 µs, RFC example 0.33 µs, 100 segments 3.4 µs, 1 000 segments 32 µs | 0.055 µs, 0.71 µs, 5.6 µs, 52 µs | accepted: within 1.6× from 100 segments, +0.4 µs on a short dotted path; `resolved()` costs about 11 µs either way |
| uuid | `UuidText`, `UuidValue` (string, version, variant), `Uuid8.bytes()` and `Uuid4s.sample()` in Okkama; 7 natives removed, `uuid_named` retained | parse 0.44 µs, `string()` 0.73 µs, `version()` 0.16 µs, `variant()` 0.23 µs, `Uuid8.string()` 1.0 µs, `Uuid4s.sample()` 0.70 µs | 0.63 µs, 0.44 µs, 0.006 µs, 0.007 µs, 0.61 µs, 0.57 µs | accepted: parsing alone 1.4×, every other operation faster; parse and `string()` together 1.05 µs against 1.16 µs |
