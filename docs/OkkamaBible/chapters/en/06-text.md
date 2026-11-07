# Chapter 6. Text

Text looks like the simplest kind of data and is one of the hardest. A
programmer reading `"Привет".length()` expects one answer; a C programmer
counting bytes gets 12, a Java programmer counting UTF-16 units gets 6, a
Swift programmer counting what users see as characters also gets 6 — and for
`"👍🏽"` the same three programmers get 8, 4 and 1. Each answer is correct
for some definition of "length", and each language has chosen one definition
and let the others leak through its APIs. This chapter explains which
definition Okkama chose, why, and what that choice costs.

## 6.1 Three layers of text

Modern text has three layers, and most confusion about strings comes from
mixing them [@unicode16].

| Layer | Unit | Example: "é" written as e + combining accent |
|---|---|---|
| Bytes | an octet of an encoding | 3 bytes in UTF-8 (`65 CC 81`) |
| Code points | a Unicode scalar value | 2 code points (`U+0065 U+0301`) |
| Grapheme clusters | what a reader sees as one character | 1 cluster |

*Bytes* are what files and networks carry. Their meaning depends on an
encoding; the dominant one today is UTF-8, which encodes each code point in
one to four bytes and keeps ASCII unchanged [@rfc3629]. *Code points* are the
numbers the Unicode standard assigns to abstract characters — about 155,000 of
them in version 16.0. *Grapheme clusters* are sequences of code points that a
reader perceives as one character: a letter with combining accents, an emoji
with a skin-tone modifier, a flag made of two regional indicators. Their
boundaries are defined by rules that change between Unicode versions
[@uax29].

A language must pick one layer as the meaning of "a character of a string".

- **C, Go and Rust** index bytes. This is fast and honest about memory, but
  `s[0..3]` on non-ASCII text can cut a character in half, and Rust makes
  such slices a runtime panic.
- **Java, JavaScript and C#** index UTF-16 code units, an accident of history:
  they were designed when Unicode fitted in 16 bits. Characters outside the
  Basic Multilingual Plane, including most emoji, take two units, and
  `length` counts both.
- **Swift** indexes grapheme clusters. This matches what users see, but
  indexing is no longer a number — it is an opaque position that must be
  walked to — and the answer depends on the Unicode version of the runtime.
- **Python 3 and Okkama** index code points.

## 6.2 Okkama's choice: immutable code points

An Okkama `string` is an immutable sequence of Unicode scalar values. Its
`length()` is the number of code points; `slice(start, end)` takes code
points `start..end-1`; `split`, `replace` and the rest work on code points.
Strings are values: they are never modified, and two strings with the same
code points are equal.

{{example:06-text}}

The first line shows the difference between layers: six characters, twelve
bytes, because each Cyrillic letter takes two bytes in UTF-8. `slice(0, 3)`
gives the first three letters, never a broken byte sequence.

**Why code points.** They are the smallest unit that is stable — the same
text has the same code points in every encoding and every Unicode version —
and that cannot be cut into invalid pieces: every slice of a string is again
valid text. Bytes fail the second test, UTF-16 units fail both, and grapheme
clusters fail the first, because their rules change between versions. Code
points are not what a reader calls a character in every case, and
Section 6.8 shows the price; but they are the largest unit an operation can
rely on without consulting tables that change every year.

**Why immutable.** Every argument of Chapter 3 applies, and text adds one
more: an immutable string can be shared between objects, threads and the
garbage collector without copying, and a substring can share its parent's
storage. Java made strings immutable in 1995 for the same reasons, and
`StringBuilder` for the cases where text is assembled piece by piece
[@gosling2023]; Okkama has the first and still lacks the second (issue #139).

## 6.3 Writing strings

String literals are written in double quotes and support a small set of
escapes:

| Escape | Meaning |
|---|---|
| `\n`, `\r`, `\t` | newline, carriage return, tab |
| `\"`, `\\` | a quote, a backslash |
| `\u{H…}` | the Unicode scalar value with 1 to 6 hexadecimal digits, e.g. `\u{e9}` for é, `\u{1b}` for ESC |

An unknown escape or a lone backslash at the end is a lexical error, not a
literal backslash — a typo cannot silently change the text.

For text that contains quotes and backslashes — JSON, regular expressions,
Windows paths, SQL — Okkama has *raw* literals fenced by three backticks:

```okkama
let document = ```{"name": "Ada", "path": "C:\new"}```;
```

Everything between the fences is kept exactly: no escapes, no interpolation,
no trimming of indentation. The first closing fence ends the literal, so a
raw literal cannot contain three backticks itself.

There is no string interpolation. Text is assembled with `+`, which accepts
only strings, so every number must be converted explicitly with `.string()`.
The rule is the same as for numbers (Chapter 5): `"n = " + 1` is an error, not
an implicit conversion. It costs some typing in output code and removes a
whole class of formatting surprises; the standard library's `strings/format`
offers positional formatting for longer templates.

## 6.4 What a string can do

The builtin actions of `string` are few; the table is generated from the
compiler. Searching, prefixes and case conversion live in the standard
library today, and Chapter 32 discusses moving the most common ones into the
language (issue #210).

| Action | Result | Note |
|---|---|---|
| `length()` | `int` | number of code points |
| `empty()` | `bool` | `length() == 0` |
| `slice(start, end)` | `string` | code points `start..end-1`; out-of-range indexes fail, never clamp |
| `split(separator)` | `@string` | literal separator; keeps empty fields; `""` splits into code points |
| `replace(target, replacement)` | `string` | every non-overlapping occurrence, left to right |
| `replace_first(target, replacement)` | `string` | the first occurrence |
| `trim()` | `string` | removes leading and trailing whitespace |
| `chomp()` | `string` | removes one trailing line ending |
| `string()` | `string` | the string itself |
| `order(other)` | `Order` | code point order |

Two design decisions recur in this table. Operations are *literal*: `split`
and `replace` never interpret their argument as a regular expression, so
`"a.b".split(".")` splits on dots; patterns go through the `regex` module,
explicitly. And operations do not guess: a `slice` past the end fails rather
than returning a shorter string, because a caller who asked for ten characters
and silently got seven has a bug that will surface far away.

## 6.5 Equality and normalisation

Two strings are equal when they contain the same code points. That is not
always what a reader means. "é" can be written as one code point (U+00E9) or
as "e" followed by a combining acute accent (U+0065 U+0301); both look the
same, and the example above shows that Okkama — like Python, Java and Rust —
considers them different strings until they are *normalised*.

Unicode defines four normalisation forms [@uax15]. NFC composes sequences
into precomposed characters where possible; NFD decomposes them; NFKC and
NFKD additionally replace *compatibility* characters — ligatures such as "ﬃ",
full-width letters, superscripts — by their plain equivalents. The standard
library's `unicode/normalization` implements all four for Unicode 16.0.0.

Okkama does not normalise implicitly, and the reason is the razor once more.
Implicit normalisation would make equality depend on tables that change with
Unicode versions, would make `length()` of a string differ from the length of
the text it was built from, and would hide a decision that belongs to the
program: an identifier comparison wants NFKC, a text editor wants to keep what
the user typed. The rule is: normalise at the boundary where text enters the
program, in the form the program needs, and compare normalised strings
afterwards — the same "parse, don't validate" principle as for numbers
[@king2019].

## 6.6 Text and bytes

Text crosses into bytes only through explicit encoders and decoders, and only
there:

```okkama
let bytes is @byte = Utf8Encoded(word).bytes();
let back is string = Utf8Decoded(bytes).string();
```

`encoding/utf` provides UTF-8, UTF-16 and UTF-32 (the last two with an
explicit byte order — serialised data never inherits the machine's), and
`encoding/single_byte` provides ASCII and Latin-1. Decoding is *strict*:
invalid input is an error naming the offset, never a silent U+FFFD
replacement character. A program that wants replacement — a log viewer, for
example — asks for it.

This separation is the lesson of two decades of encoding bugs. Python 2 let
byte strings and text mix implicitly, and code worked until it met its first
non-ASCII user; Python 3 separated them at the cost of a famously painful
migration. Okkama starts on the far side of that migration: there is no
implicit conversion in either direction, and a function's signature says
whether it takes text (`string`) or data (`@byte`).

## 6.7 Characters

`char` is a single code point, written in single quotes: `'x'`, `'Ж'`,
`'\u{1F44D}'`. A `char` literal must contain exactly one code point after
escapes, so `'é'` written with a combining accent is rejected. Characters
compare and order like strings.

That is almost all they do today. A `char` cannot be turned into a string or
into its code point, and a string cannot be taken apart into `char`s — `slice`
returns one-character *strings* — so most code, including the standard
library, works with one-character strings instead (issue #231). This is a
case where the razor has not finished its work: the language has two
half-entities where one whole one would do. Chapter 32 returns to it.

## 6.8 What text costs

Code-point indexing has a cost that byte indexing does not: finding the 1000th
code point of a UTF-8 string means walking the first 999, because code points
have different byte lengths. The Sparrow core keeps a fast path — a string
known to be ASCII has one byte per code point, so `length()` and `slice()` are
constant-time — and walks otherwise.

The walk is cheap once and expensive in a loop. A program that visits every
character of a string with `slice(i, i + 1)` does a walk per visit, so its
total cost is quadratic in the length of non-ASCII text. Figure 6.1 measures
exactly that program on the 0.4.1 core.

{{figure:slice-cost|Figure 6.1. Visiting each character with slice(i, i + 1): constant per character for ASCII, quadratic for Cyrillic text (issue #52). Data: data/slice-cost.json.}}

For 32,000 ASCII characters the program takes 0.15 s, almost all of it
process start; for 32,000 Cyrillic letters it takes 2.6 s, and doubling the
text quadruples the time. The remedy belongs to the language and runtime,
not to every program: a cursor that walks a string once (issue #52 plans it),
or builtin character iteration (issue #210). Until then, programs that scan
long non-ASCII text should split it, encode it to bytes, or use the
`regex` module, all of which walk once.

The standard library audit of November 2026 found the same pattern —
character-by-character slicing — in about forty modules, from MIME header
parsing to template compilation, and filed an issue for each hot one. It is
the best example in this book of a cost that a design choice moved from the
language to its users, and of why the choice has to be paid for with the
right operation.

## 6.9 Limits

- **No searching or case builtins.** `contains`, `starts_with`, `ends_with`,
  `find`, `lower` and `upper` are not builtins yet (issue #210).
- **Characters are half an entity.** `char` cannot become text or a code
  point (issue #231).
- **No string builder.** Assembling long text from many pieces with `+` is
  quadratic; the library has no native builder yet (issue #139).
- **Per-character loops are quadratic on non-ASCII text** until a cursor
  exists (issue #52).
- **Booleans print as `1` and `0`.** `true.string()` is `"1"`, a choice
  inherited from the first implementation that surprises readers of output
  (Chapter 9 discusses it).
