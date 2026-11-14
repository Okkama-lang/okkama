# Chapter 22 — archived implementation passages

Replaced mixed paragraphs are preserved in full for context.

## 22.2 The reference: `json`

described 0.5.x

Writing is the same object built in code: `Json(JsonField("id", 4711), …)`
holds its members in order, and `string()` asks the runtime for the compact
text in one call; `pretty(2)` indents it. Numbers are exact, never `float`:
a JSON number reads back as an `int`, a `BigInt` or a `Decimal`.

## 22.5 Readable and efficient

described 0.5.x

- *No quadratic growth.* Collections are built with builders, `reduce`
  accumulators or the persistent arrays and maps, never by copying in a
  loop. The runtime makes `acc + [x]` inside `reduce` grow in place
  (section 8.5), but a module does not rely on a reader knowing that.
- *Measured.* A hot path has a micro-benchmark, and a change is judged on
  the release build, not on a debug or gate profile whose code layout
  exaggerates small differences.
- *Natives only where measured,* or for what Okkama cannot do itself:
  cryptography, TLS, X.509 and PEM stay native; a parser written in Okkama
  stays in Okkama unless a measurement says otherwise.
- *No fixed cost per call.* A table, a pattern or a template is parsed
  once, when its object is built, not every time an action uses it.

The two principles meet in the runtime underneath. The 0.5.7 release, for
instance, changed no public API of `std`, yet it
made `join`, ranges in array literals, map literals and the future
collections of `async/collect` several times faster on large inputs: the
library's shape stayed put while its cost moved. A style that keeps the
surface small is what lets that happen without breaking the programs above
it.

## 22.8 Tests

described 0.5.x

The library is tested where it is written. Every public object has at
least one inline `test` next to it (chapter 15), and the std tests run on
both engines, the IR engine and Sparrow, in every gate. A module is not
changed until tests pin what it does now: the review of 0.5.4 wrote such
*characterization* tests first and changed code second, so that a
simplification which altered behaviour showed up as a failing test, not as
a surprise in a program.

## 22.8 Tests

described 0.5.x

Finally, the modules people use directly have examples: 70 small programs
under `examples/`, one per task, each with the output it must print, run on
both engines by `examples/check.py`. A test checks that the library is
right; an example checks that it is usable, and that the way the
documentation says to use it still works.

## 22.12 okkama man — full source example

described 0.5.x

```text
$ okkama man json
NAME
    json - JSON values: one immutable Json, parsed or built in code, read
    without casts and written by the runtime in one call.

SYNOPSIS
    let order = ParsedJson(text).root();
    order.int("id");                                // 4711
    order.maybe("note").text() else "";             // "" when absent or null
    …
```
