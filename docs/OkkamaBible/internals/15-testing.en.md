# Chapter 15 — archived implementation passages

Replaced mixed paragraphs are preserved in full for context.

## 15.1 Tests live next to the code

described 0.5.x

Tests run in the same process, on the engine chosen with `--engine`.
`okkama run -t --filter TEXT` runs only the tests whose names contain
`TEXT`; with several `--filter`s, those that contain any of them. A test
may stand anywhere among an object's members, between hidden actions too;
it is still a test of the object. The standard library can
render a test report as human text, TAP, JSON or JUnit (`testing/report`),
but the command line offers no report formats, seeds or parallel workers
yet. They are planned with the rewritten standard library (#339).

## 15.6 Properties

described 0.5.x

- `IntegerGeneration(-100000, 100001)` produces integers from −100000 up
  to, but not including, 100001.
- `42` is the seed of the property's own PCG random stream. The same seed
  produces the same inputs on every engine and every run.
- `200` cases, with a size that grows from 0 to `1000` across them, so the
  early cases are small.
- When a case fails, up to `64` reductions are tried, and the smallest
  input that still fails is reported.

## 15.9 What this design costs

described 0.5.x

- tests sit beside the code and vanish from production;
- every role has a double, maintained with the role and checked by the
  compiler;
- a failed property comes back as a seed and a minimal input that
  reproduce it on any engine.

