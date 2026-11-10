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
  on the IR engine and on Sparrow and compares stdout and exit code
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
| 25 | Sparrow: JIT, precise GC, stack maps | 20 |
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

- Chapter 18 (closures): in a program with a `destroy` anywhere, every
  function value is treated as possibly owning a resource, so closures are
  tracked by regions (#292); 09-closures with `os/environment` imported runs
  6× slower on the JVM. Also for Chapter 11's limits, with #285 (tail calls
  lost to empty regions, fixed in 0.4.1).
- Chapter 20 (async): `async destroy`, awaited at scope exit (#154);
  structured await — cancelling a task cancels the future it awaits unless
  another task awaits it, and waits for it to settle (#147); why one child
  needs no group (compare Trio nurseries, Kotlin structured concurrency).

## Progress

Updated with each chapter: `build.py --stats`.

- Chapter 20 (async): 10.9 pages (EN) of its floor of 22. Written: lazy
  futures (`20-lazy`), where futures came from (Baker–Hewitt, Multilisp,
  promises, F#, C#, JS, Rust; colour), the cooperative scheduler and wake
  order (`20-wake`, #340), `spawn` and process workers, structured await and
  cancellation (`20-cancel`, #147; Trio, Kotlin), channels and the state
  owner (`20-ledger`), fan-out and deadlines (`20-fanout`), events, locks,
  semaphores, barriers, atomics, tokens, resources across suspension and
  `async destroy` (`20-lease`, #154), `for await` (`20-stream`), lost
  failures (`20-lost`), how the engines run tasks, a comparison table,
  limits, costs. `--check` of `20-wake` and `20-cancel` on IR needs a build
  with #340. To add: a worked example (a crawler or a connection pool with a
  bounded channel and a deadline); measured costs of await, spawn and a
  channel round trip per engine (micro benchmarks); generics and async with
  Chapter 17; the history of structured concurrency at more length (Dijkstra's
  goto letter as Smith reads it, Erlang links and supervisors).
- Chapter 10 (failure): 14.9 pages (EN) of its floor of 22. Written: the
  two channels, history, error enums, throw, try/catch, causes, confirm,
  try?/try!, host failures, a module boundary (`10-config`), budgets,
  failures across tasks (`10-tasks`), the uncaught report, designing an
  error vocabulary, testing failures, a comparison table, costs, and
  resumption with Common Lisp restarts as the road not taken. Also how the engines carry a failure.
  To add: a case study of the std HTTP client's failures; more on
  failures inside cleanup and their secondaries (with Chapter 11). Engines disagree on the uncaught
  report (#270).
- Chapter 11 (resources): 8.2 pages (EN) of its floor of 20. Written:
  memory versus resources, finalizers, the lineage (RAII, `with`, `defer`),
  `defer`, `destroy` and its order, ownership by identity, cleanup on
  failure and in constructors, `async destroy`, resources and tasks
  (`11-tasks`), the `Closing` pattern (`11-closing`), resources in maps and
  closures (`11-containers`), regions and the IR they lower to, the cost of
  a walk (#9), limits, costs. To add: channels; a larger worked example (a connection
  pool); how each engine implements regions (Sparrow's region arrays and
  identity table, the IR engine's owner cells, the JVM's `Region`).
- Chapter 12 (objects): 8.9 pages (EN) of its floor of 18. Written:
  declaring an object, private and permanent fields, explicit constructors,
  shorthands, several constructors (variadic), validation in the
  constructor, `self` calls and `private:`, caps (`12-cap`), structural
  equality and objects as map keys, what a constructor compiles to (IR of
  `Point`), handing `self` out of a constructor (`12-escape`, `obj.init`;
  the missing run-time check is #271), how each engine stores an object,
  a comparison with Java records, Kotlin data classes and Rust structs,
  costs. To add: a worked domain model (money, an order and its lines);
  designing small objects (one noun per computation) with std examples;
  constructor failure and partial objects in depth.
- Chapter 13 (actions and successors): 7.7 pages (EN) of its floor of 18.
  Written: actions and results, `return` boundaries, `void`, parameters,
  defaults and named calls, overloads (ambiguity, #273), variadics,
  successors and `obj.next`, reads see the receiver, the must-use rule
  (#272 for its location), rebinding, failure, naming transitions (`with_*`
  rejected, CQS), composition and private successors, `Cell`, the cost of
  a successor on each engine, a comparison table, costs. To add: a worked
  state machine (an order's life) across several objects; successors
  through roles (`^Role`, with Chapter 14); measurements of successor
  loops against `Cell` and `reduce`; async successors.
- Chapter 14 (roles): 6.9 pages (EN) of its floor of 18. Written:
  declaring a role and conformance (exact parameters), nominal versus
  structural (Go, Liskov–Wing), restricted references and no downcast
  (capabilities, attenuation), narrowing between roles, successors through
  roles and typestate (`14-states`), decorators instead of inheritance, how
  each engine runs a role call, operators as roles (#215, `14-operators`;
  C++, Python, Haskell `Num`, Rust `std::ops`), a comparison table, costs
  (operators look only left). To add: generic
  roles (`Iterable<T>`, `Cursor<T>`) and the iteration contract; std roles
  (`Writing`, `WritingText`) and built-in values behind roles; async role
  actions; a larger composition case study from std (HTTP client layers);
  measurements of role calls against direct calls.
- Chapter 15 (fakes, tests, properties): 5.2 pages (EN) of its floor of 14.
  Written: test blocks and the report, the `test` binding, fakes versus
  mocks (Meszaros, Fowler, EO), writing a fake, state in a fake (fields,
  successors, `fake.storage`), properties with shrinking (QuickCheck), the
  std toolbox, current gaps (#275, #276, #277), costs. `build.py --check`
  runs `NAME-tests` examples with `-t`. To add: a worked test suite for a
  service with a clock and a repository fake; `RecordedCalls`, `Stub` and
  `ManualClock` examples; async tests in depth; snapshot tests.
- Chapter 16 (enums and pattern matching): 8.2 pages (EN) of its floor of
  14. Written: payload-free enums (no ordinal, order, actions or mixing; C,
  Pascal, Java, Go), declaration order as map-key order, variants (positional
  payloads, no accessors, generic inference, structural equality; Pascal
  variant records, HOPE, ML, Haskell, Rust, Java sealed/records/pattern
  switch), `match` and its exhaustiveness rules (Maranget), compact choices,
  enums versus roles (the expression problem: Reynolds, Cook, Wadler), how
  both kinds run (OIR, Sparrow's enum word, the JVM's `Case`, variants with
  an object per case since #290, measured before and after), a comparison
  table, limits (#286–#291), costs. To add: a worked protocol (messages as
  variants) and a state machine; error enums in depth with Chapter 10;
  `Maybe`/`Presence` as variants with Chapter 7; what changes when #287 and
  #288 land.

- 0.4.3 (2026-11-10): Chapters 8 and 6 follow the persistent collections of
  Sparrow (#246–#255). Chapter 8 gains `replaced` and `removed_at`, how `+`,
  slices, `replaced` and map versions share structure, why tries serve only
  `replaced` chains, and Figure 8.1 remeasured with four series
  (`okkama-docs/benchmarks/persistence/fig81`). Its limits now name the IR
  and JVM ports (#335, #336), the declined #248, and the first version of a
  large map. Chapter 6's limit on `+` says it shares on Sparrow.

- Chapter 19 (modules): 14.1 pages (EN), 14.4 (RU) of its floor of 14. Written:
  - the module as a file, versions and identity, `main()` that never runs on
    import, multi-file modules;
  - the three import forms, no wildcard, collisions, `std.` paths, naming;
  - lookup order, bounds, cycles, and breaking a cycle with a contract module;
  - no re-export, the dependency closure, nominal identity by module and
    major;
  - `hidden` and the contract/hidden-implementation pattern;
  - module constants;
  - packages and blade (honestly marked young, owner 2026-11-10: «blade у нас
    больная тема пока, там все в полусыром виде»);
  - whole-program builds into one SPB/JAR;
  - history (Parnas, Modula-2, ML), a comparison table, limits and costs.

  To redo when blade matures: 19.7, with a real project example. Diagnostics
  of same-named types: #338.

## Naming (owner, 2026-11-10)

The virtual machine is **Sparrow**. **Tomato** is the name of its current
version, as Tahoe is a version of macOS. Chapters call the machine Sparrow,
and name Tomato only where a version matters (for example, against the
previous Sparrow, `sparrow-origin`).

## Notes for the build

- `builtins_table()` (for Appendix B) matches `(Ty::X, "name")` only;
  alternatives such as `"less" | "less_or_equal" | "greater" |
  "greater_or_equal"` are missed and must be handled before the appendix is
  written.
- Chapter 17 (generics): 13.2 pages (EN) of its floor of 14. Written:
  parameters on objects, roles and enums; no generic actions (algorithms
  are objects); history (CLU, ML, GJ, .NET, Go); exact identity
  (`Least(1, 2.0)`); inference at construction (arguments, expected type,
  roles, several constructors, anonymous action results, no explicit
  specialization); F-bounded role bounds; key parameters (#242);
  specialization as identity, no variance, generic fakes; a table of
  diagnostics; monomorphization with OIR, its code size (`Sorted` ×1..4
  types) and polymorphic recursion (H-15, #317); erasure, reification,
  dictionaries; parameter or role (calls through a bound stay `call.role`,
  measured equal); a worked `Top<T>`; std generics; a comparison table;
  limits (#276, #268, #317). To add: generics and async (`Future<T>`,
  `Channel<T>`) with Chapter 20; a measurement of compile time against the
  number of specializations.
- Chapter 18 (closures): 10.2 pages (EN), floor of 10 reached. Written:
  syntax and types of anonymous actions, positional calls, functional blocks
  are not closures, captures as snapshots and captured rebinding rejected
  (#321), closures made in a loop, captured successors carried by closures
  and bound actions (#322, #324), exact types, equality by identity, action
  values as data, closures or objects, behaviour that waits needs a role
  (#323), the IR and each engine's representation with measured call costs
  and the 0.4.2 micro-benchmarks (#244), closures and regions (#292), a
  closure frame in a failure report (#270), a history, a comparison table,
  limits (no recursion through `let`, #318). To add: a worked example (an
  event bus; retry policies as closures versus objects); closures in std
  (`SortedBy` keys, `Cell` updates); closures crossing tasks (with Chapter
  20); the JVM's inlining on a megamorphic call site.


## Chapter 20 sources (checked 2026-11-10)

- halstead1985: Halstead, MULTILISP, TOPLAS 7(4): 501–538, doi 10.1145/4472.4478.
- liskov1988: Liskov & Shrira, Promises, PLDI '88, pp. 260–267, doi 10.1145/53990.54016.
- baker1977: Baker & Hewitt, The Incremental Garbage Collection of Processes, MIT AI WP-149, hdl 1721.1/41969.
- syme2011: Syme, Petricek & Lomov, The F# Asynchronous Programming Model, PADL 2011, LNCS 6539: 175–189, doi 10.1007/978-3-642-18378-2_15.
- nystrom2015: What Color is Your Function?, journal.stuffwithstuff.com/2015/02/01/what-color-is-your-function/.
- elizarov2018: Structured concurrency, medium.com/@elizarov/structured-concurrency-722d765aa952 (Sept 2018).
- smith2018 is already listed. Examples: scratch 20-lazy.ok verified on ir/sparrow; the conformance case L08 cancel.awaited-cleanup-settles is the model for structured await.

## Chapter 21 notes (2026-11-10)

5.6 pages (EN) of its floor of 14.

**Written** (21.1–21.9):
- natives and providers (`21-unregistered`);
- the native ABI, async natives;
- handles (`21-handle`: idempotent close, `already_closed`);
- authority as values (`21-directory`; FileRoot, no chdir; Miller, Dennis–Van Horn);
- running programs (Command without a shell, InheritedCommand, Process lifecycle);
- native groups and embedding budgets (#269 MemoryScope);
- the command line keeps the user's authority (Deno, WASI, JEP 411);
- limits, costs.

**To add:**
- process workers in depth: transferable values, failures as data, what a worker can reach;
- a worked example of an embedder: `run_limited` with groups and budgets, Rust side;
- the provider side: how a native is registered and kept equal on three engines (conformance of natives);
- std's other explicit-authority types (Environment, clocks for tests, `ManualClock` from ch.20);
- TLS and PostgreSQL as handles;
- a history of FFIs (JNI, ctypes, Rust FFI) and why Okkama has none for users.

Examples cannot fork or reach the network in the cage, so process and network code stays in plain code blocks.
