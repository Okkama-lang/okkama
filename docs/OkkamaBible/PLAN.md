# The Okkama Bible — plan

Working plan of the book (not part of the published text). A page is counted
as 2,500 characters of text, code included. The book has no page limit: at
least 600 pages per language, and more is better — it is a Bible.
`build.py --stats` prints the current size of every chapter.

## Principles

- **Two languages.** `chapters/en` and `chapters/ru` carry the same chapters
  with the same file names; the Russian text is a translation of meaning, not
  of word order.
- **True to the implementation.** Every statement about Okkama is checked
  against the compiler (`okkama-rs`), the conformance suite or a run. Tables of
  builtins, std modules and instructions are generated from the code, never
  typed by hand. Where the older Okkama Book disagrees, the code wins.
- **Every example runs.** Programs live in `examples/NN-name.ok`; a chapter
  includes one with `{{example:NN-name}}`, and `build.py --check` runs each
  on the IR engine and on the Sparrow core and compares stdout and exit code
  with `examples/NN-name.expected`.
- **Every claim about design is sourced.** Arguments cite published work by
  key, `[@hoare1981]`; `references.md` lists every key with authors, title,
  venue, year and a stable link. No key is cited that is not in the list, and
  no work is listed that the author has not checked exists.
- **Figures are generated.** `figures/src/*.py` write SVG into `figures/`
  from data in `data/` (benchmarks, compiler measurements) or from code
  (diagrams); `{{figure:name|caption}}` places one. Mermaid blocks are allowed
  for simple flow diagrams.
- **Honest.** Each part ends with what Okkama does badly or not yet, linked to
  its issue.
- **Two formats.** The finished book ships as Markdown (`OkkamaBible.md`,
  `OkkamaBible-RU.md`) and as PDF in both languages, with highlighted code.
  Okkama code is coloured from the compiler's own lexer
  (`okkama tokens --format json`), so highlighting never disagrees with the
  language; the PDF toolchain runs in a container, not on the host.

## Outline and page budget

| # | Chapter | Pages (at least) |
|---|---|---:|
| | **Part I. The razor** | |
| 1 | Entities must not be multiplied: Ockham, parsimony and software | 14 |
| 2 | A short history of objects: Simula, Smalltalk, Java, Elegant Objects | 16 |
| 3 | The Okkama manifesto, rule by rule, and what each rule costs | 18 |
| | **Part II. Values** | |
| 4 | Types at a glance: categories, exactness, no implicit conversion | 12 |
| 5 | Numbers: int, byte, rat, float, BigInt promotion, IEEE 754 | 20 |
| 6 | Text: strings, characters, Unicode, bytes | 14 |
| 7 | Absence without null: Maybe, Presence, else | 14 |
| 8 | Collections: arrays, maps, ranges, functional operations | 16 |
| | **Part III. Behaviour** | |
| 9 | Expressions and control flow | 14 |
| 10 | Failure: throw, panic, try/catch, causes, confirm | 22 |
| 11 | defer, destroy, regions: deterministic resources | 20 |
| | **Part IV. Objects** | |
| 12 | Objects, fields and constructors | 18 |
| 13 | Actions, successors and immutability | 18 |
| 14 | Roles: nominal contracts and composition | 18 |
| 15 | Fakes, tests and properties | 14 |
| 16 | Enums and pattern matching | 14 |
| 17 | Generics | 14 |
| 18 | Anonymous actions and closures | 10 |
| | **Part V. Programs** | |
| 19 | Modules, imports, packages and blade | 14 |
| 20 | Async, futures and structured concurrency | 22 |
| 21 | Processes, natives and capabilities | 14 |
| 22 | The standard library and its style (the json reference) | 24 |
| | **Part VI. Under the hood** | |
| 23 | The compiler: lexer to OIR | 20 |
| 24 | Three engines: IR interpreter, JVM, Sparrow | 16 |
| 25 | The Sparrow core: JIT, precise GC, stack maps | 20 |
| 26 | Safety by construction: verification, budgets, fuzzing, conformance | 18 |
| 27 | Performance: measurements and what they mean | 16 |
| | **Part VII. Practice and judgement** | |
| 28 | Building a CLI tool | 12 |
| 29 | Building a web service | 16 |
| 30 | Patterns and anti-patterns | 14 |
| 31 | Okkama among languages: Java, Kotlin, Go, Rust, Haskell, OCaml | 16 |
| 32 | Weaknesses, open problems and the road ahead | 14 |
| | **Appendices** | |
| A | Grammar | 8 |
| B | Builtin behaviour (generated) | 6 |
| C | Standard library index (generated) | 12 |
| D | Glossary | 6 |
| E | References | 10 |
| | **Total** | **≈ 552** |

The page counts are a floor, not a ceiling: a chapter is written to its
substance and grows with it. The planned floors add up to about 552 pages, so
chapters are expected to exceed them until the whole book passes 600 pages per
language.

## Notes for unwritten chapters

- Chapter 19 (modules): module constants — `const NAME = value;`, computed by
  the compiler, public unless `hidden const`, arrays and maps built once per
  run; why constants are not a way to share objects (#153).
- Chapter 20 (async): `async destroy`, awaited at scope exit (#154);
  structured await — cancelling a task cancels the future it awaits unless
  another task awaits it, and waits for it to settle (#147); why one child
  needs no group (compare Trio nurseries, Kotlin structured concurrency).

## Progress

Updated with each chapter: `build.py --stats`.

- Chapter 10 (failure): 14.9 pages (EN) of its floor of 22. Written: the
  two channels, history, error enums, throw, try/catch, causes, confirm,
  try?/try!, host failures, a module boundary (`10-config`), budgets,
  failures across tasks (`10-tasks`), the uncaught report, designing an
  error vocabulary, testing failures, a comparison table, costs, and
  resumption with Common Lisp restarts as the road not taken. Also how the engines carry a failure.
  To add: a case study of the std HTTP client's failures; more on
  failures inside cleanup and their secondaries (with Chapter 11). Engines disagree on the uncaught
  report (#270).
- Chapter 11 (resources): 7.7 pages (EN) of its floor of 20. Written:
  memory versus resources, finalizers, the lineage (RAII, `with`, `defer`),
  `defer`, `destroy` and its order, ownership by identity, cleanup on
  failure and in constructors, `async destroy`, resources and tasks
  (`11-tasks`), the `Closing` pattern (`11-closing`), regions and the IR
  they lower to, the cost of a walk (#9), limits, costs. To add: resources
  in maps and closures; channels; a larger worked example (a connection
  pool); how each engine implements regions (the core's region arrays and
  identity table, the IR engine's owner cells, the JVM's `Region`).
