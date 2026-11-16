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
  on Sparrow, the only engine (the IR engine was removed in 0.6.0, #584),
  and compares stdout and exit code
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
- **The language, not its organs (owner, 2026-11-14).** The Bible describes
  the language: semantics, observable behaviour, costs as the language defines
  them, and guidance. Implementation internals do not belong here: compiler
  phases, OIR, engines, Sparrow, the JIT, GC, stack maps, SPB, regions as a
  mechanism, and which engine does what. They date the book with every
  release, and they get a book of their own later (working title, from the
  owner's joke: «Библия сатаниста»). Passages taken out of existing chapters
  are kept in `internals/` for that book, never deleted.
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
| 11 | defer and destroy: deterministic resources | 20 |
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
| | **Part VI. Costs** | |
| 23 | What things cost: the language's cost model, measured from the outside | 16 |
| | **Part VII. Practice and judgement** | |
| 24 | Building a CLI tool | 12 |
| 25 | Building a web service | 16 |
| 26 | Patterns and anti-patterns | 14 |
| 27 | Okkama among languages: Java, Kotlin, Go, Rust, Haskell, OCaml | 16 |
| 28 | Weaknesses, open problems and the road ahead | 14 |
| | **Appendices** | |
| A | Grammar | 8 |
| B | Builtin behaviour (generated) | 6 |
| C | Standard library index (generated) | 12 |
| D | Glossary | 6 |
| E | References | 10 |
| | **Total** | **≈ 478** |

The former Part VI (the compiler, the engines, Sparrow, safety by
construction) moved to the separate internals book on 2026-11-14.

The page counts are a floor, not a ceiling: a chapter is written to its
substance and grows with it. The planned floors add up to about 552 pages, so
chapters are expected to exceed them until the whole book passes 600 pages per
language.

## Language changes to reflect (0.6.0)

- Chapter 15 (fakes): `fake.storage` was removed in 0.6.0 (#583). Done
  2026-11-15: a fake's changing state is a field holding an `Atomic` (a Cell
  cannot be a field); `15-library-tests` and 15.3/15.5 rewritten, and the
  example passes on 0.5.8 (same pattern as the 0.6.0 conformance case
  fakes.typed-state); recheck with the 0.6.0 toolchain.
- Since 2026-11-15 `build.py --check` runs every example on Sparrow only,
  with `--engine sparrow`, which both the 0.5.8 and the 0.6.0 toolchains
  accept. The IR engine was removed in 0.6.0 (#584, owner's decision on
  #574), and the JVM backend too (#585).

## Notes for unwritten chapters

- Chapter 25 (Sparrow GC), from 0.5.5–0.5.6:
  - The collection trigger counts reused cells: a sweep keeps one free chain
    per page, and a list that runs dry takes the next chain and counts its
    bytes (#522). Before, machine code popped cells uncounted and the heap
    grew every cycle once #503 stopped charging builders: 10-app 13 → 25 →
    12.5 MB.
  - Surplus empty pages are freed after a sweep (#435). A program after a
    burst ran 1.95 → 0.70 s; resident memory did not drop, because mimalloc
    keeps the pages.
  - Objects are marked when queued (#438): marking −23–34% on 11-ownership.
  - The floor of 1 MiB stays: 8 MiB made sha256 52% slower (#544). A
    sweep-time recycling of Failure payloads did not help.
  - A deadline heap for timers measured parity and was not adopted (#441).
  - One waiter is woken without a vector (#440); the reactor's woken buffer
    is reused (#442).
- Chapter 27 (performance), from 0.5.5:
  - The release report is the micro-benchmark A/B (protocol v2, Docker,
    oracles, VALID/INCONCLUSIVE, no baseline subtraction); the
    cross-language ladder is on demand.
  - The micro suite once rotted, when all 72 programs used removed std
    names.
  - Cases where a measurement changed a decision: #544 (a trade of time
    for memory, accepted), #441 (rejected at parity), #435 (CPU, not RSS).

- Chapter 18 (closures): in a program with a `destroy` anywhere, every
  function value is treated as possibly owning a resource, so closures are
  tracked by regions (#292); 09-closures with `os/environment` imported runs
  6× slower on the JVM. Also for Chapter 11's limits, with #285 (tail calls
  lost to empty regions, fixed in 0.4.1).
- Chapter 7 (absence): 7.4 pages (EN), 7.6 (RU) of its floor of 14
  (2026-11-15). Written: what null meant, one construct per meaning (bare
  `none` does not parse either), `Maybe` and its four exits, where a `Maybe`
  comes from (searches, indexes and keys with fallbacks, readers, `try?`,
  streams; `07-sources`), `else` in detail (lazy, typed, three fallback kinds,
  chains, `confirm`), `Maybe` as a value (equality, fields, not a map key,
  owns what it holds), `Presence`, absence at the edges (env, config, json
  `maybe`/`presence`, SQL `NULL`, Codd's marks), a worked settings example
  (`07-settings`), the null object and sentinels, a comparison table with Go,
  costs, limits. To add: `Maybe` in generic code (a cache `Index<K, V>`);
  measured cost of a `Maybe` on the hot path against a sentinel; an
  absence-heavy case study from std (the HTTP client's optional headers).
- Chapter 20 (async): `async destroy`, awaited at scope exit (#154);
  structured await — cancelling a task cancels the future it awaits unless
  another task awaits it, and waits for it to settle (#147); why one child
  needs no group (compare Trio nurseries, Kotlin structured concurrency).

## Progress

- 2026-11-15, evening: 5.11 "What exactness costs, in digits" (`05-growth`:
  harmonic numbers as rat and float, a year of daily interest exact or
  booked to the cent); a pass over the 70 closed issues the chapters cite
  (`build.py --issues`): #190 (ch. 1), #264 → #329 (ch. 11), #121, #196,
  #199, #204 (ch. 20; `20-stream` now iterates the receiver itself). Found
  while writing: #743 (`rat.numerator()`/`denominator()` return strings).
  Many "To add" notes below are already written (14.8 iteration, 16.6
  protocol, 12.11 domain model, 11.11 pool, 20.12 pool and pipeline).
- 2026-11-16: 27.8–27.10 Python, Swift, Erlang; 5.12 floating point in
  practice (`05-kahan`, checked against Python); 3.13 the weaknesses each
  rule removes (CWE classes, titles checked at MITRE); 11.11 replacing a
  resource (`11-rotation`: rebinding keeps old values, a fold releases);
  2.11 values come back (Evans, Bloch, records); 8.8 sorting worked through
  (`08-sorting`); 28.1 the gap the book found (#743).
- 2026-11-16, later: 4.9 three refusals; 13.14 async successors (found
  #754); 9.10 expressions in other languages; 12.11 when construction fails
  (`12-unbound`, `12-unbound-rejected`, `12-partial`: the D-035 split
  between the static and the path check, fields bound in blocks, release
  order; C++, Java/OBJ11-J, Kotlin `lateinit`, Swift, Rust). Chapter 12 is
  9.8 pages.
- 2026-11-16: 6.11 patterns without catastrophe (`06-patterns`: named
  groups, literal replacement, typed errors, `(a+)+$`; Kleene, Thompson,
  Cox; Python 3.9 `re` measured at 0.6 s for 22 a's, doubling per a;
  Stack Overflow 2016 and Cloudflare 2019 outages, CWE-1333).
- 2026-11-16: 14.9 async actions in a role (`14-async-role`: a desk, a
  table and a fallback decorator; `14-async-shape-rejected`,
  `14-async-caller-rejected`; Swift SE-0296, Kotlin, Rust 1.75, C#).
- 2026-11-16: 15.8 waiting without waiting (`15-backoff-tests`: a
  recording `Timing`, `Stub`, `RecordedCalls`, mutation-checked; why
  counting waits is right here); found #756 (test report prints object
  identities).
- 2026-11-16: 27.11–27.13 Clojure (values and identities, open protocols,
  `with-open`; Hickey's HOPL history), C# (the ideas added one by one, from
  the official version history up to C# 15's unions), TypeScript (structural
  and deliberately unsound; `strictNullChecks`); port and conclusion
  renumbered to 27.14–27.15.
- 2026-11-16: 28.1 "What writing this book found" (#743, #754, #756); 28.4
  a synchronous answer to an async role (Swift's direction); 28.5 the path
  check of unbound fields and the regex dialect as accepted costs.

- 2026-11-15 (idle work during the 0.6.0 rebuild): worked examples or
  comparison sections added to chapters 1–6, 8, 9, 11, 14–16 (harbour,
  units after the Mars Climate Orbiter report, slug, grouping and joining,
  binary search, commit or roll back, the rules in one transfer; type
  systems, collections and testing compared; histories of roles and
  enums). Chapter 28 written (4.3 pages EN): what closed while the book was
  written, open language issues (#286–#289), library and tool gaps, open
  questions (data-object shorthand, narrowing `any`, a non-finite type,
  `Maybe` combinators, errors in signatures, #148, generic actions), costs
  accepted on purpose. Stale passages fixed: #214 and #233 (ch. 4, 8),
  BigInt after #189 (ch. 5). Chapter 27 first version (4.0 pages EN): a
  matrix of the rules across Java, Kotlin, Go, Rust, Haskell, OCaml and
  Okkama, one section per language, what the comparison shows. Still
  unwritten: chapter 23 (cost model, after Champ is production-ready).
- Editorial pass (2026-11-14): applied “The language, not its organs” to
  all 21 remaining existing chapter pairs (1–19, 21–22); Chapter 20 was
  used as the reference and left unchanged. Removed and rewritten source
  passages are preserved verbatim in 19 new EN/RU pairs under `internals/`,
  with source section headings and `described 0.5.x`. Chapter 11 is now
  “defer and destroy: deterministic resources”. Renumbered Chapters 5,
  10–12, 14, 16–17 and updated chapter cross-references in both languages.
  Removed figure references: `numbers-cost`, `slice-cost`, `append-cost`;
  no example files or figure files were deleted. `17-grow-rejected` remains
  as a language-limit example. Verified paired section structure and asset
  references, verbatim archive coverage, absence of old engine-chapter
  references, and successful `python3 build.py`; `--check` was not run.

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

  Redone for 0.5.3 (2026-11-11): 19.7 describes Blade after #347–#351, #353
  (packages as repositories, minimal version selection, sync by locked
  commit, local changes kept); 19.4 and 19.11 the diagnostics of #338. Still
  no real project example: installing runs git, which the cage forbids.

- Chapter 26 (patterns and anti-patterns), 2026-11-14: 12.8 pages (EN and
  RU) of its floor of 14. Written:
  - patterns and what a language does to them (Alexander, GoF, Norvig;
    antipatterns after Koenig and Brown);
  - composition instead of flags (`26-flags`), Composite (`26-composite`),
    Strategy as a role or an action value (`26-strategy`);
  - successor pipelines (`26-pipeline`), Builder as named options
    (`26-options`, Bloch), typestate (`26-typestate`, `-rejected`), Memento
    as a list of values (`26-undo`);
  - named refusals (`26-refusals`), renamed at a layer boundary (→ 10.12);
  - every dependency a role with its fake (`26-signup-tests`); decide, then
    do (`26-decide`, Bernhardt's functional core);
  - resources in scopes (`26-section`), absence decided at the edge
    (`26-edge`), one owner for shared state (`26-owner`,
    `26-shared-rejected`);
  - the anti-patterns the compiler refuses and the ones it allows;
  - a worked refactoring (`26-before`, `26-after`), the GoF catalogue read
    in Okkama, and costs.

  Fixed 2026-11-14: the chapter repeated each example's output in a manual
  `text` block after `{{example}}`, which already appends it.

  Left for the floor: 26.13 (costs) and 26.1 are thin; candidates are a
  pattern for tests (tables of cases) and the expression problem in
  practice. Found while writing: #594 (an enum case list rejects a trailing
  comma).

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

8.4 pages (EN) of its floor of 14 (2026-11-11: 21.5.1 process workers).

**Written** (21.1–21.9):
- natives and providers (`21-unregistered`);
- the native ABI, async natives;
- handles (`21-handle`: idempotent close, `already_closed`);
- authority as values (`21-directory`; FileRoot, no chdir; Miller, Dennis–Van Horn);
- 21.4.1 the environment and the clock (`21-authority`: snapshots, `Timing`, a fixed clock, clock domains; where std still makes its own `SystemClock()`, #352);
- running programs (Command without a shell, InheritedCommand, Process lifecycle);
- native groups and embedding budgets (#269 MemoryScope);
- the command line keeps the user's authority (Deno, WASI, JEP 411);
- limits, costs.

- 21.5.1 process workers: the same program run again, values only, failures
  as text, cancellation as a kill (#359), output passed on whole;
  multiprocessing and Erlang.

**To add:**
- a worked example of an embedder: `run_limited` with groups and budgets, Rust side;
- the provider side: how a native is registered and kept equal on three engines (conformance of natives);
- TLS and PostgreSQL as handles;
- a history of FFIs (JNI, ctypes, Rust FFI) and why Okkama has none for users.

Examples cannot fork or reach the network in the cage, so process and network code stays in plain code blocks.

## Chapter 24 notes (2026-11-15)

8.9 pages (EN and RU) of its floor of 14. The tool is `tally` (examples/tally/tool.ok, a module). The run example is `24-tally` over tally/sample.txt; the tests are `24-tally-tests`, with a Given implementation of Sources. Sources: McIlroy 1978, POSIX Utility Conventions, CLI Guidelines.

Written:
- what a tool owes its user (five rules);
- the command line as data (cli/command);
- the tool, deciding then printing (Outcome, Shell);
- failures at the edge (codes 0, 1, 2; bugs are not caught);
- tests without a terminal;
- whole or streamed input;
- configuration and the environment;
- shipping (help, completion, build/exec);
- costs.

Added: subcommands (`24-subcommands`), a streaming filter with grep's exit status (`24-filter`), and flag > environment > default precedence (`24-settings`). Found while writing: #637, a branch command refuses its own --help.

Added 2026-11-15:
- output for people and programs (`24-output`: DetectedTerminalCapabilities, NO_COLOR, JSON);
- changing files safely (`24-rewrite`: a Change value, --dry-run, sibling write + move).

Left out on purpose: interruption (Ctrl-C). The std has no public signal API for synchronous tools (only the internal runtime/signal used by web/app), and the book makes no unverified claim about Ctrl-C.

To add for the floor:
- a longer case study: grow `tally` into a two-subcommand tool end to end;
- or move material from 24.3 into a guided walkthrough.

## Chapter 25 notes (2026-11-15)

First draft, 4.0 pages (EN and RU) of 16. The service is a notes JSON API (examples/notes/api.ok): NoteStore role, MemoryStore over Atomic<Shelf> with compare_and_replace, NotesApi controller, NotesService routes. Tests are `25-notes-tests` with WebTestClient, no network. Sources: RFC 9110, RFC 9457.

Written:
- what a service is made of;
- the service, endpoints, failures as statuses (problem details);
- shared state (Atomic plus a whole-value shelf);
- tests without a network;
- running (listen, graceful shutdown on SIGTERM/SIGINT);
- costs.

Second pass (2026-11-15), 9.9 pages:
- arounds (`25-guarded-tests`: BearerAuth on a scope, an own Maintenance around);
- keyset pagination (`?after=&limit=`, limit + 1, a `next` link; offset vs keyset);
- an exception scope (`StoreRefusals`: `StoreRefusal.duplicate` from the store becomes 409);
- the store role became `async`, so a database can do it; honest note that the CAS loop never retries while `added` has no suspension point (Section 20.3);
- PostgreSQL behind NoteStore (`notes/postgres`: a pool, `$n` parameters, UNIQUE + SQLSTATE 23505 → the role's refusal, keyset in SQL); compiled through `25-postgres-wiring`, never connected in the sandbox;
- a concurrency test (four spawned creations, ids 1..4).

To add for the floor:
- the same suite over PostgresStore in CI (a role contract suite), only if the book's check gets a database;
- a session/login walkthrough with web/session (blocked: WebTestClient sends no headers on GET/DELETE and keeps no cookies, #650);
- the server-perf notes, only with measured numbers.
