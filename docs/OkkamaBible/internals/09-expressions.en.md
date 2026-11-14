# Chapter 9 — archived implementation passages

Replaced mixed paragraphs are preserved in full for context.

## 9.5 Loops

described 0.5.x

A `loop` without a `last` runs until something stops it. The step budget
(Chapter 26) counts every branch back to an earlier point of the program. So
`okkama run --max-steps N` stops any loop, on every engine, with the same
report.

## 9.7 Order of evaluation

described 0.5.x

Okkama has a second reason as well. Three engines run the same program — the
IR interpreter, the JVM and Sparrow — and the conformance suite
compares their outputs byte for byte. With an unspecified order, the expected
answer for a test would be a set of outputs instead of one.

## 9.3 Operators

described 0.5.x

- `&&`, `||`, `!` and every condition take a `bool`. That includes the
  conditions of `if`, `next if`, `confirm` and `??`. `if count { … }` is an
  error, not a test for zero.
- `==` and `!=` compare values of one category. `1 == 1.0` is rejected
  (Chapter 5), and so is comparing an `int` with a range.
- `<` and the other orderings need the same numeric category on both sides.
  `1 < 2.5` is rejected; write `1.float() < 2.5`.
- `+` adds numbers, joins strings and concatenates collections, and never
  mixes them: `"n = " + 1` is an error (Chapter 6).
- `/` on two integers gives an exact `rat`. There is no `**`; powers are
  written with `pow` (Chapter 5).
- An object takes part in arithmetic and ordering only through roles it
  declares: `a + b` is `a.plus(b)` when the type of `a` does `Adding`, and
  `a < b` asks `a.order(b)` through `Comparing` (§14.8). An object without
  those roles has no operators.
