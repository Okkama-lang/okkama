# time_format native migration (#13), 2026-11-22

Branch `feature/13-time`, baseline `89320c60c`. The five
`runtime/time_format` natives (IDs 460–464) are retired. `time/format`
writes and reads its patterns in Okkama (`TimePattern`, hidden) behind its
unchanged API. `runtime/time_zone`, which shares `time.rs`, stays.

## Inventory

`std/runtime/time_format.ok` declared five natives, all implemented by
`okkama-runtime-core/src/native/time.rs` (origin only, no sparrow-core
provider):

| Native | What it did |
|---|---|
| `time_pattern_invalid_offset(pattern, local, parsing)` | Checked a pattern. |
| `time_format_date` | Wrote a date. |
| `time_format_local` | Wrote a local date-time. |
| `time_parse_invalid_offset(origin, pattern, local)` | Found where a text stops matching. |
| `time_parse_parts` | Gave the fields a text holds. |

The only caller was `time/format` (`FormattedDate`,
`FormattedLocalDateTime`). `http/date` uses it through
`FormattedLocalDateTime`.

## Semantics, kept as the native had them

- **Directives.** `%Y %m %d %H %M %S %f` have fixed widths 4/2/2/2/2/2/9.
  `%%` is a percent sign. Every other character is a literal Unicode
  scalar, and offsets count scalars.
- **Pattern errors**, in this order:
  1. a bad directive, or a `%` ending the pattern, is refused at its `%`.
     Tokenization comes first, so `%Y %H %q` in a date pattern fails at the
     `%q`;
  2. a time directive in a date pattern is refused at the first one;
  3. when reading, a missing required field is refused at the pattern's
     length. Required are Y, m and d, plus H, M and S for a local date-time.
- **Writing.** Fields are zero-padded to their width and never cut. A
  negative value would keep its `-` inside the width; `Date` and
  `TimeOfDay` never give one.
- **Reading.**
  - Literals must match character by character; a field needs exactly its
    width of ASCII digits (`０` is refused).
  - A repeated field must repeat its value, or the text is refused at the
    repeat.
  - The text must end where the pattern does.
  - Each refusal is at the first offset that does not fit.
  - Nanoseconds default to 0. Field values go to `Date` and `TimeOfDay`,
    which refuse impossible ones as before.
- **Order of checks.** `string()` of a reading object refused a bad pattern
  before reading the text. It now reads first, but the reading checks the
  pattern first itself. Any pattern that `string()` refuses, the reading
  refuses too, at the same offset, so the outcome is the same.

## Implementation

`TimePattern` splits the pattern at `%`. The checks, the exact reading and
the slow writing are pure recursion, so `parsed()` keeps its `once let`
(#491).

Fast paths, each checked against the exact path:

- **Writing.** A pattern without `%%` (or with `%%` marked as U+0001,
  unless the pattern holds U+0001 itself) is written by replacing each valid
  directive with its padded field. A `%` left over means a bad directive:
  the exact check then names its offset.
- **Reading a pattern with more than eight directives.** Fixed widths place
  every field. Each field is read where it stands, the pattern is written
  again, and the result is compared with the text. Any difference goes to
  the exact reading. Below that size the exact reading is faster.

## Measurements

Release build, public API, 20 000 iterations per point, three runs each
(`bench/run.sh`).

- "Native": `okkama-13c` with the std of `89320c60c`.
- "Okkama": this branch's binary and std.
- Raw output: `measurements.txt`.

Patterns:
- small: `%Y-%m-%d`, a date;
- medium: `%Y-%m-%dT%H:%M:%S.%f`, a local date-time;
- large: a 160-character sentence with `%%` and every field twice.

| Point | Native | Okkama | Ratio (median) |
|---|---|---|---|
| format small | 0.74 / 0.74 / 0.73 µs | 0.54 / 0.54 / 0.54 µs | 0.73× |
| format medium | 1.34 / 1.34 / 1.23 µs | 1.04 / 1.04 / 1.11 µs | 0.78× |
| format large | 2.62 / 2.60 / 2.43 µs | 2.21 / 2.12 / 2.15 µs | 0.83× |
| parse small | 1.63 / 1.61 / 1.51 µs | 1.50 / 1.58 / 1.52 µs | 0.94× |
| parse medium | 2.77 / 2.76 / 2.55 µs | 3.14 / 3.15 / 3.37 µs | 1.14× |
| parse large | 5.94 / 5.93 / 5.89 µs | 7.16 / 7.14 / 7.20 µs | 1.21× |

## Differential

`generate.py SEED COUNT OUT` writes records of three kinds, all with random
fields:

- format inputs and parse inputs;
- patterns mixing literals (including Unicode), every directive, `%%`, bad
  directives and a trailing `%`, with repeated and missing fields;
- texts written by the pattern, often mutated.

`differential.ok FILE` computes each outcome twice:
- as the previous `time/format` did, by calling the natives as it called
  them;
- through the public API of this `time/format`.

It compares the text, the date-time, or the failure kind and offset. It
needs a binary that still has the natives (`okkama-13c`) and
`std/runtime/time_format.ok` from `89320c60c` next to this std.

Seeds 13, 14 and 15 give 18 000 inputs:

| Outcome | Count |
|---|---|
| texts written | 7 213 |
| pattern refusals | 5 280 |
| reads | 3 336 |
| text refusals | 2 158 |
| `Date` refusals | 13 |

The 31 inputs of the std tests were run too. There are **0 differences**
(`differential.txt`). An injected bug (an unknown directive taken as `%Y`)
gave 257.

## Tests and mutations

`std/time/format.ok` gains three tests:
- padding and `%%`, including U+0001 in a pattern;
- pattern refusals with offsets;
- readings and text refusals with offsets, including the long-pattern path.

`mutate.py BIN` applies 14 mutations, and all are caught
(`mutations.txt`). `http/date`, `time/date` and `time/local_datetime` std
tests pass. `examples/time` (4) passes, and `bench/micro/check.py` passes
for date-format.
