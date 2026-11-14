# Chapter 6 — archived implementation passages

Replaced mixed paragraphs are preserved in full for context.

## 6.2 Okkama's choice: immutable code points

described 0.5.x

**Why immutable.** Every argument of Chapter 3 applies, and text adds one
more: an immutable string can be shared between objects, threads and the
garbage collector without copying, and a substring can share its parent's
storage. Java made strings immutable in 1995 for the same reasons, and
`StringBuilder` for the cases where text is assembled piece by piece
[@gosling2023]. Okkama has both. The second is `TextBuffer` from
`strings/buffer` (issue #139, since 0.5.5): it keeps the text in the runtime,
each `write` appends without copying what is there, and `string()` hands out
the finished, immutable result.

## 6.8 What text costs

described 0.5.x

Code-point indexing has a cost that byte indexing does not: finding the 1000th
code point of a UTF-8 string means walking the first 999, because code points
have different byte lengths. Sparrow keeps a fast path — a string
known to be ASCII has one byte per code point, so `length()` and `slice()` are
constant-time — and walks otherwise.

The walk is cheap once and expensive in a loop. A program that visits every
character of a string with `slice(i, i + 1)` walked from the start at each
visit, so its total cost was quadratic in the length of non-ASCII text.
Figure 6.1 measures that program on Sparrow before and after the
fix.

{{figure:slice-cost|Figure 6.1. Visiting each Cyrillic character with slice(i, i + 1): quadratic before issue #52, linear after it. Data: data/slice-cost.json.}}

Before the fix, 32,000 Cyrillic letters took 2.6 s, and doubling the text
quadrupled the time; ASCII text was already constant per character. After
it, the same program takes 0.13 s — the process start. The fix keeps strings
indexed by code point and remembers, for the four most recently indexed long
strings, the last position reached (its code point and byte offset); every
lookup walks from the nearest of the start, the end and that position. A
sequential scan therefore walks the text once, and random access costs the
distance from the nearest known point. Every engine does the same (issue
#52).

`characters()` needs no cache at all: it walks the string once and returns
every code point. On the same 32,000 letters it also takes 0.13 s, and it is
the loop to write when the program wants characters rather than positions.

## 6.9 Limits

described 0.5.x

- **`+` shares on every engine, but says less than a buffer.** Since 0.4.3
  on Sparrow (issue #255) and 0.5.3 on the IR engine and the JVM (issues
  #335, #336), a chain `text + piece` writes into a text buffer the versions
  share: a long text assembled from many pieces costs a constant per piece,
  not a copy of everything so far. When a loop builds one result, a
  `TextBuffer` (issue #139) says so in the code instead of relying on that
  sharing.
- **Case conversion has no locale.** `lower()` and `upper()` follow the
  Unicode default; Turkish, Azeri and Lithuanian rules are missing
  (issue #261).
- **The standard library still has its workarounds.** About forty modules
  search, compare prefixes and walk characters by slicing; moving them to
  the builtins waits for the rewrite of the library (issue #260, postponed).

