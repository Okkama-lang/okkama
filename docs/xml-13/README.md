# XML native migration (#13), 2026-11-22

Branch `feature/13-xml`, baseline `b6b10bdb7`. The eleven `runtime/xml`
natives (IDs 658–668) are retired. The tokenizer is now Okkama
(`std/xml/internal/events.ok`) behind unchanged `xml/parse` and `plist/parse`.

## Inventory

`std/runtime/xml.ok` declared `XmlEventsState` and eleven operations,
implemented by `okkama-runtime-core/src/native/xml.rs` (origin only, no
sparrow-core provider): `xml_events` (tokenize within depth and event limits),
`xml_events_error`, `xml_events_offset`, `xml_event_count`, `xml_event_kind`,
`xml_event_name`, `xml_event_text`, `xml_attribute_count`,
`xml_attribute_name`, `xml_attribute_value` and `xml_event_offset`. The only
std callers were `xml/parse` (`ParsedXml.document`, every operation except
`xml_event_offset`) and `plist/parse` (`PlistXmlEvents`, the XML form of
`ParsedPlist`: no attribute calls). Serialization (`xml/write`,
`plist/write`, `XmlDocument.xml()`) never used the natives.

## Semantics, kept as the native had them

- **Well formedness only, no validation.** A DOCTYPE is skipped while
  quotes are tracked. A `[` outside quotes is "a DTD internal subset is not
  supported" at the `[`. A DOCTYPE after the root element is refused. No
  external entity is fetched.
- **References.** The only entities are `lt gt amp quot apos` and character
  references.
  - `&#…;` and `&#x…;` parse as Rust's `u32` does: one optional `+`, digits of
    the base, hex in either case.
  - `&#X…;` is not hex.
  - The value must be a character XML 1.0 allows. Surrogates, `#0`, U+FFFE
    and anything above U+10FFFF are refused.
  - A reference's text runs to the next `;` in the same run.
- **Characters.** Text, CDATA and attribute values must be XML 1.0
  characters, reported as `U+XXXX` with at least four uppercase digits.
  Comments and instruction data are not checked.
- **Line ends** become LF in text and CDATA: a CRLF pair or a lone CR.
  `&#13;` stays a CR.
- **Attribute values.** Tabs, LF and CR become spaces (no further
  normalization). `<` inside a value is refused. Duplicate names are refused
  at the second name, after its value is decoded.
- **Text events.** Adjacent text and CDATA join into one event: comments and
  instructions split them. Whitespace outside the root is dropped; other text
  there is refused at its first non-blank character. `]]>` in text is
  refused at the start of the text.
- **Comments and instructions** count only inside the root element.
  - A comment's content may not contain `--`; `<!-- a --->` is accepted with
    content ` a -`.
  - An instruction's data loses its leading blanks.
  - The XML declaration (target `xml` in any case) is allowed only at the
    very start, after an optional BOM. Elsewhere it is "an XML declaration
    that is not at the start", at the end of the target.
- **Names** start with an ASCII letter, `_`, `:` or any non-ASCII character,
  and continue with those, ASCII digits, `-` and `.`. Namespaces are resolved
  by `xml/parse` above the tokenizer, unchanged.
- **Limits.** The depth limit is checked after the start tag is read, at its
  end. The event limit is checked as each event is added, at the current
  position; joined text does not count.
- **Errors** are character offsets, BOM included, and keep the native's
  messages and order of checks.
- **Also refused**, each with its own message: a mismatched or unmatched end
  tag, a second root, an element still open at EOF, a document without a root
  element.

## Measurements

Release build, public API, three runs each (`bench/run.sh`). "Native" is
`okkama-13b` with the std of `b6b10bdb7`; "Okkama" is this branch's binary
and std. Raw output: `measurements.txt`.

| Program | Native | Okkama | Ratio (median) |
|---|---|---|---|
| xml small: one item, attribute + entity, `document()` + two reads | 13.38 / 14.03 / 13.83 µs | 13.91 / 13.89 / 13.14 µs | 1.00× |
| xml medium: RSS, 20 items, namespace, CDATA, comments (~5 KB) | 983.5 / 915.2 / 1012.5 µs | 1003.9 / 968.6 / 973.9 µs | 0.99× |
| xml large: 2000 records, 70 003 events (615 KB) | 191.5 / 174.7 / 190.6 ms | 207.2 / 207.8 / 202.2 ms | 1.09× |
| plist small: 1 entry, 5 keys | 33.32 / 32.56 / 34.06 µs | 36.18 / 34.84 / 35.20 µs | 1.06× |
| plist medium: 40 entries (~250 nodes) | 1070.5 / 1048.4 / 1055.3 µs | 1105.0 / 1100.1 / 1049.2 µs | 1.04× |
| plist large: 600 entries (~3600 nodes) | 22.01 / 21.12 / 22.41 ms | 25.02 / 24.63 / 22.85 ms | 1.12× |

On the large document, in one-off runs:

| Step | Time |
|---|---|
| Native tokenizer | ~4 ms |
| Native: reading three fields per event across the native boundary | ~23 ms |
| Okkama tokenizer | ~33 ms |
| Building the tree (`xml/parse` frames, the same code on both sides) | ~190 ms |

## Differential

`generate.py SEED COUNT OUT [BAD]` writes documents built from every
construct, mutated copies, 89 hand-written edges (each also with tight
limits) and limit cases (5 000 siblings, nesting at and past 256).
`differential.ok FILE` compares the event streams: kind, name, text, every
attribute, the offset of each event, or the reason and offset of the refusal.
It needs a binary that still has the natives (`okkama-13b`) and
`std/runtime/xml.ok` from `b6b10bdb7` next to this std.

Results are in `differential.txt`:

| Set | Documents | Well formed |
|---|---|---|
| Seeds 13, 14, 15 (BAD 0.05) | 3 × 6 000 | |
| Seeds 21, 22 (BAD 0.01) | 2 × 6 000 | |
| The three benchmark documents | 3 | |
| The documents of the std tests | 75 | |
| **Total** | **30 078** | **14 294** |

There are **0 differences**. An injected bug (`&apos;` not recognised) gave
264.

## Tests and mutations

`std/xml/internal/events.ok` has two tests:
- documents become events: references, line ends, attribute normalization,
  comments, instructions, prolog, joined text, names, BOM offsets;
- 40 refusals with exact offsets and messages, plus the depth and event
  limits.

`mutate.py BIN` applies 21 mutations. 20 are caught (`mutations.txt`). The
missed one (the `u32` bound on reference arithmetic) is equivalent: any value
above it fails `allowed()` anyway. The same holds for the surrogate filter,
which was removed from the code as redundant.

`xml/parse`, `xml/node`, `xml/name`, `xml/write`, `plist/parse`,
`plist/value` and `plist/write` std tests pass. `bench/micro/check.py`
passes for xml-attribute, xml-escape, xml-name, plist-parse, plist-integer
and plist-write. The Bible example 22-xml passes.
