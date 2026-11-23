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

**Why immutable.** A string passed to another object keeps its code points. To assemble text piece by piece, use `TextBuffer` from `strings/buffer`: each `write` appends without copying all preceding text, and `string()` returns the finished immutable result [@gosling2023].

{{example:06-buffer}}

A `TextBuffer` is a `WritingText`, so it goes wherever text is written: a
report that prints to standard output in production prints into a buffer in
a test.

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

The builtin actions of `string` cover measuring, cutting, searching,
trimming, case and repetition. Until 0.4.1 searching and case lived in the
standard library, which worked around their absence in about forty modules;
the most common ones are now builtins (issue #210).

| Action | Result | Note |
|---|---|---|
| `length()` | `int` | number of code points |
| `empty()` | `bool` | `length() == 0` |
| `slice(start, end)` | `string` | code points `start..end-1`; out-of-range indexes fail, never clamp |
| `split(separator)` | `@string` | literal separator; keeps empty fields; `""` splits into code points |
| `replace(target, replacement)` | `string` | every non-overlapping occurrence, left to right |
| `replace_first(target, replacement)` | `string` | the first occurrence |
| `trim()` | `string` | removes leading and trailing whitespace |
| `trim_start()`, `trim_end()` | `string` | removes it from one edge |
| `chomp()` | `string` | removes one trailing line ending |
| `contains(part)` | `bool` | literal search |
| `starts_with(part)`, `ends_with(part)` | `bool` | literal prefix and suffix |
| `find(part)`, `find_last(part)` | `Maybe<int>` | code point index of the first or last occurrence; `""` is found at the start, respectively the end |
| `lower()`, `upper()` | `string` | full Unicode case mapping, no locale: `"ß".upper()` is `"SS"` |
| `repeat(count)` | `string` | a negative count, or a result over 1 GiB, fails |
| `characters()` | `@char` | the code points, in one walk |
| `string()` | `string` | the string itself |
| `order(other)` | `Order` | code point order; `Order` comes from the std module `compare` |
| `less(other)`, `less_or_equal(other)` | `bool` | the same order: `order(other) == Order.less`, `!= Order.greater` (until 0.4.2 they compared lengths) |

Two design decisions recur in this table. Operations are *literal*: `split`,
`replace` and `contains` never interpret their argument as a regular
expression, so `"a.b".split(".")` splits on dots; patterns go through the
`regex` module, explicitly. And operations do not guess: a `slice` past the
end fails rather than returning a shorter string, because a caller who asked
for ten characters and silently got seven has a bug that will surface far
away; a search that finds nothing says so with `Maybe.none`, not with `-1`.

Case conversion is the one place where "no guessing" needs a qualification.
`lower()` and `upper()` apply the Unicode default case mapping, which is the
same everywhere and needs no locale — but Turkish, Azeri and Lithuanian have
their own rules (in Turkish, `"I".lower()` should be the dotless `"ı"`). Those
rules live in the library, where the locale is an argument the reader can see:
`LocalizedCase(text: "istanbul", locale: Locale("tr")).upper()` from
`locale/case` is `"İSTANBUL"`, and its `title()` capitalises words (issue #261).
The builtins stay locale-free, so `"I".lower()` is `"i"` on every machine.

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
library's `unicode/normalization` implements all four for Unicode 18.0.0.

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
compare with `==` and have `order(other)`, by code point; unlike strings they
have no `less`, and no character has `<`.

A `char` converts in both directions: `letter.code()` is its code point as an
`int`, `letter.string()` (or `string(letter)`) its text, and `char(1046)`
makes one from a code point — a number that is not a Unicode scalar value
(above `0x10FFFF`, or a surrogate) is a runtime error, not a replacement
character. `text.characters()` takes a string apart into its `@char`, so a
loop over the characters of a string is `for text.characters() -> letter`.

Until 0.4.1 none of this existed: a `char` could be written and compared and
nothing else, and code — the standard library included — worked with
one-character strings (issue #231). The razor had left two half-entities
where one whole one would do; the cure was to finish the entity rather than
to remove it, because a code point and a one-code-point string are different
things — one is a number with a name, the other a sequence.

## 6.8 What text costs

Code-point indexing has a cost: code points have different byte lengths in UTF-8, so an arbitrary position can require a walk. ASCII positions are constant-time. A sequential scan of non-ASCII text is linear; arbitrary indexing can cost up to a linear walk.

`characters()` walks a string once and returns its code points. Use it when a loop needs characters rather than positions.

The standard library audit of November 2026 found the same pattern —
character-by-character slicing — in about forty modules, from MIME header
parsing to template compilation, and filed an issue for each hot one. It is
the best example in this book of a cost that a design choice moved from the
language to its users, and of why the choice has to be paid for with the
right operation. Version 0.6.2 paid it: about two hundred such sites now call
`contains`, `starts_with`, `ends_with` and `find` (issue #260). The slicing
that remains is a parser's cursor moving through its input, which is what
slicing is for.

## 6.9 Laying out text

Without interpolation, longer text is assembled from small objects that each
know one transformation and answer `string()`. Two modules carry them:
`strings/format` fills a pattern, and `text/layout` shapes paragraphs.

{{example:06-layout}}

- **`Formatted(pattern, values...)`** fills each `{}` slot, left to right,
  with the next value's own `string()`. Other braces are literal, so `{x}`
  stays `{x}`. A pattern whose slots do not match its values is the error
  `InvalidFormat`, not a blank or a leftover `{}`.
- **`Wrapped(origin:, width:)`** collapses runs of whitespace between words,
  removes it at both ends, and packs whole words into lines no wider than
  `width`. A word is never split; one longer than the width keeps a line of
  its own.
- **`Indented(origin:, prefix:)`** puts the prefix before every line that is
  not blank and keeps the line endings; **`Dedented(origin:)`** removes the
  longest spaces-and-tabs prefix that every non-blank line shares.
- **`Shortened(origin:, width:, placeholder:)`** keeps the longest
  whole-word prefix that still fits together with the placeholder.

Widths count code points, consistently with `length()`: a width of 36 is 36
Unicode scalar values, which is what a terminal shows for text without wide
or combining characters, and an honest approximation otherwise. Each object
is a value, so a layout is built by nesting — `Indented(origin:
Wrapped(...).string(), prefix: "  ")` — rather than by calling a chain of
methods on a mutable buffer.

## 6.10 A worked example: a slug from a title

A *slug* is the part of a web address made from a title: `creme-brulee` for
"Crème brûlée". Making one touches every layer of this chapter, and every
step is a decision that the program states.

{{example:06-slug}}

- **Case first.** `lower()` applies the Unicode default case mapping
  (Section 6.4). It needs no locale and gives the same result everywhere.
- **Spelling rules are explicit.** The German sharp s has no decomposition,
  so no Unicode algorithm turns `ß` into `ss`. The slug does it with a
  literal `replace`, and the comment says why. German readers would also
  expect `ä` to become `ae`; this slug does not do that, and a program for
  German titles would add one more `replace` in the same place.
- **Decompose, then drop the marks.** NFKD splits `è` into `e` and a
  combining grave accent, and it folds the ligature `ﬁ` into `f` and `i`
  (Section 6.5). The combining marks are recognised by their Unicode general
  category, *nonspacing mark*, through `unicode/character`, and removed.
  Nothing here guesses: the tables are Unicode's own, 18.0.0 for the
  decomposition and 17.0.0 for the categories.
- **Everything else separates words.** ASCII letters and digits are kept,
  and any other character becomes a space. `split(" ")` keeps the empty
  fields between repeated spaces (Section 6.4), so `filter` drops them
  before `join("-")`.
- **The edge is visible.** A title in a script without a Latin
  decomposition, such as `日本語`, gives an empty slug. Transliteration from
  Japanese to Latin letters is a separate problem with its own conventions,
  and a real program would fall back to an identifier. The example prints
  `[]` rather than hiding the case.

None of these steps is hidden in the string type or in a default of the
library. When the slug comes out wrong for some title, the missing rule is
missing from the program text, where a reader can find it and add it.

## 6.11 Patterns without catastrophe

Section 6.4 kept patterns out of the string type: `split(".")` splits on
dots. Patterns live in the `regex` module, and a program asks for one by
name.

{{example:06-patterns}}

- **A match reads like a record.** `each.text("event")` is the text of a
  named group, and `each.int("day")` reads the same group as a number, so
  `"03"` becomes `3` without a separate parse. A group that took no part in
  the match says so. It does not pretend to be empty text.
- **The common questions are asked of the pattern.** `found`, `first`,
  `all`, `parts` and `replaced` each take the whole text. `replaced` is
  literal: a `$` in the replacement is just a dollar sign. Expanding
  `${name}` is a separate, explicit policy.
- **A malformed pattern is a typed error with a position.** The missing
  `)` is reported at offset 13, the end of the expression, counted in code
  points like everything else in this chapter. The pattern compiles once,
  at its first use, and remembers either the program or the error.
- **`(a+)+$` answers at once.** That line is the reason for the dialect.

Regular expressions began as mathematics. Kleene described the "regular
events" a finite automaton can recognise [@kleene1956]. Thompson turned the
notation into a search command for the QED editor, and his algorithm
compiles a pattern into a nondeterministic automaton and runs every state
at once, in time linear in the text [@thompson1968]. The tools that came
later, Perl first and then the libraries of Java, Python, Ruby and
JavaScript, chose backtracking instead: they try one alternative, and on
failure go back and try the next. Backtracking makes backreferences and
lookaround easy. It also makes some patterns take exponential time. Russ
Cox measured a 29-character input that took Perl over sixty seconds and a
Thompson automaton about twenty microseconds [@cox2007].

The pattern in the example is the textbook case. On this machine,
Python 3.9's `re` takes 0.6 seconds for twenty-two `a`s and a `b`, and
every further `a` doubles the time. The example's forty would take about
two days. The failure has a CWE number, CWE-1333, Inefficient Regular
Expression Complexity [@cwe], and a history of outages.

- **Stack Overflow, July 2016.** A post with about 20,000 whitespace
  characters in a row met a pattern that trimmed trailing whitespace. Its
  backtracking was only quadratic, but quadratic in 20,000 took the site
  down for 34 minutes. The fix replaced the pattern with a substring
  function [@stackexchange2016].
- **Cloudflare, July 2019.** One firewall rule containing `.*(?:.*=.*)` put
  the CPUs that serve HTTP at nearly 100% across the network for 27
  minutes. The remedy announced afterwards was a move to RE2 or Rust's
  `regex`, two engines with a linear-time guarantee [@cloudflare2019].

Okkama's dialect makes the same choice those engines made. It has
literals, classes, groups, alternation and the usual quantifiers, but no
backreferences, lookaround, conditionals or recursion, so a bounded
Thompson automaton can run every pattern in linear time. Its limits are
explicit, typed failures: 4,096 code points of expression, 256 nested
groups, 1,024 captures and 16,384 states. Its Unicode data is pinned to
version 17.0, the same as the string type's. A program that needs a
backreference writes the check as code after the match. The cost of that
is a few lines. The cost of a catastrophic pattern is an outage.

## 6.12 Limits

- **Text assembly.** Extending a chain with `text + piece` costs the appended text rather than a copy of everything so far. A `TextBuffer` makes the intention to build one result explicit.
- **Case conversion in the builtins has no locale.** `lower()` and `upper()`
  follow the Unicode default; Turkish, Azeri and Lithuanian rules are a
  library call, `LocalizedCase` (issue #261).
