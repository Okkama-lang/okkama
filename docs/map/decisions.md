# Decisions and rulings (do not relitigate; extend when a decision is made)

The newest entries are at the top of each section. Each one gives its date, who decided, and where the details
are. Working rules (testing, CI, roles) are in LLM-GUIDE §1 and are not repeated here. Seeded 2026-11-20 from
the session's memory; older rulings live in the commits and tickets named.

## Direction (owner)

- **2026-11-23. #1016 no native provider twins in runtime-core (owner).**
  `okkama-runtime-core::native::execute` provides exactly the natives Sparrow's
  `host/mod.rs` `classify` routes to `ORIGIN` (minus those `natives.rs` answers
  itself: stdout/stderr and `runtime/bytes`); every other native is refused as
  "not provided by runtime-core". A twin of a Sparrow provider is not kept "just
  in case": live behaviour is fixed in the host. Guard:
  `native_origin::tests::origin_routes_match_the_runtime_core_providers`.

- **2026-11-22. #946 worker reuse (supplied implementation task).** Keep the
  public `parallel/map` and `spawn` APIs. A hidden map helper gives a copied
  bound action a pool for one `results()` call; existing batches, FutureScope
  and AllFutures retain ordering and failure/cancellation. Extend spawn's
  Request/ProcessGroup/OKW2 transport with a Unix duplex control socket, not
  the #938 test scheduler (which also uses fresh processes). Every request
  creates a fresh Runtime/heap/JIT/thread/host context and reconstructs its
  receiver from declared fields, excluding synthetic once slots. OS/native
  process-global state can persist within the run; pools never cross calls.
  Keep one-shot spawn outside map, without a reusable host transport, or in
  programs containing native process launches (descendant cleanup). Embedded
  execution never acquires a worker command. Scope settlement and shielded
  pool close kill/reap active and idle workers before publishing results.

- **2026-11-22. #957 local Cell rule (owner design and amendment).**
  A Cell-bearing value cannot be erased into any/roles or bind a generic
  parameter; Cell itself and builtin collections preserve visibility.
  Cell-retaining closures and bound actions may only be locally bound,
  captured by another frame-bound action, or called synchronously. Arbitrary
  synchronous callbacks are not non-escaping contracts. Enum payloads obey
  the field restriction. Further implementation amendment, authorized by the
  task's counterexample policy: `once let` is hidden field storage and cannot
  hold Cell, even through a pure-looking action-field call. One lexical
  body walk plus existing coercion/instantiation checks; no fixed point or
  public effect types. Source: #957 owner comments and amended task design.

- **2026-11-22. Security refresh #960/#963/#962 (owner task).** Keep one
  advisory failure policy for main/fuzz lockfiles. cargo-fuzz lacks a locked
  option, so its child Cargo build/check/metadata commands are wrapped with
  --locked. Coverage references identify the inspected source revision; runtime
  records separately identify binary provenance and never turn external guard
  stops or excluded scenarios into PASS. Document channel payload matching only
  within its verified scope; budgets and native groups are not OS isolation.

- **2026-11-22. #949 HTTP/2 session startup (supplied ticket).**
  `FutureGroup.start` is eager and `close` seals the loops group. Hold the
  reader and idle loop on a local Event, register/start the writer last, then
  release the Event. The writer must own output cleanup before a reader can
  queue GOAWAY and immediately see EOF. Preserve protocol faults, timeouts,
  cancellation and the public API; do not catch `FinishedFutureGroup`.

- **2026-11-22. Format native #13, CSV (owner performance rule).**
  Accept the pure default-grammar scalar/span parser and the indexed streaming
  byte scanner: all measured sizes pass 1.5x (worst median 1.326x, worst final
  run against best baseline 1.337x). Retain all eight natives for owner retirement.
  Keep custom-dialect behavior and public API unchanged; use hidden helpers.
  The byte scanner is maintained in csv.ok and mechanically copied into
  csv/stream.ok, avoiding a new public helper API or a dependency cycle;
  `tools/csv/sync-scanner.py --check` enforces identical copies.
  [Numbers, semantic inventory and verification](../csv-13/README.md).

- **2026-11-22. #943 fixed JUnit wire format (performance task).**
  Serialize its constant XML vocabulary directly in testing/report instead
  of constructing and repeatedly validating an XML tree. Keep user text
  escaping in the existing JUnitReportText/EscapedXml mechanisms and escape
  the shared suite/classname once per render. Keep the public API, exact
  output (including xmlns on each element), and document size bound.
  Verified by 2100 differential reports and eight killed test mutations.

- **2026-11-22. #940 finding 1: cancellation during timeout timer cleanup.**
  For a directly selected origin, `Collection.winner` with `consumed[0]`
  preserves the irreversible receive commit until the wrapper publishes its
  outcome, including `finishing` while it awaits the cancelled timer. Check
  the `SelectionItem` intrinsic before applying this protection. Keep waiting
  for timer cleanup; do not consume the origin twice or change cancellation
  precedence for ordinary futures. Deterministic Rust regressions cover both
  `Within` and `Before`, with an ordinary-future control and a mutation check.

- **2026-11-22. Response content fields (#942, owner task).** Both content
  overloads use the existing ContentFields policy, preserving compatible media
  types and explicit framing and deriving known byte lengths. Open content once
  and reuse the opening. Public signatures and raw-body overloads stay unchanged.
  A supplied application/json field requires JsonBody, not a plain TextBody.

- **2026-11-22. Format native #13, diff (measured implementation decision).**
  Under the owner's one-module instruction, keep the existing public API and
  retain `runtime/diff::diff_script` (ID 141). A behavior-equivalent pure Okkama
  Hunt–Szymanski port adds 20.88 ms/call at 512 repeated lines (30.43×); the
  absolute cost exceeds the migration budget. Preserve the experiment outside
  std, keep all native registrations/counts, and add mutation-confirmed tests.
  This native is LCSidx, not Myers. [Numbers and evidence](../diff-13/README.md).

- **2026-11-22. Std integers (#511).** The owner authorizes the whole supplied
  review for this release, replacing its planning/report workflow with an
  implementation summary and excluding CHANGELOG/Git changes. Ordinary std
  integer quantities use int, including BigRational components and optional
  readers; domain objects and BigInt remain. Keep existing explicitly bounded
  readers, format/driver constraints and every power budget. One int-based
  BoundedPower beside BigInt serves both parsers and BigInt.power; exact rat
  handles rational normalization. [Audit, decisions and evidence](../std-int-511/README.md).

- **2026-11-22. Documentation Phase D (#515).** The owner authorized the
  entire supplied ticket without approval or language changes. Publishing
  consumes the same compiler model and Phase B declaration identities:
  man(7) section 3ok, static relative-link HTML, explicit JSON schema v1.
  Full-std export uses one compiler graph; optional install/packages generate
  pages from std. Site integration is a portable artifact directory within
  the requested worktree. Choices and checks: [Phase D record](../std-docs-phase-d-515.md).

- **2026-11-22. #329 runtime implementation and conditional gate.** Under the
  owner's runtime-first task, optimize a single ordinary async-destroy entry
  inside `region_close_until`; share `region_action` with the general close
  for owner/keep/pin decisions, and retain the ordinary shielded Future and GC
  roots in `rt_region_settle`. No ownership model, lowering or language change.
  The final measured destroy is still 1.884× close, so the owner's explicit
  stop after step 3 applies: no std guard conversions. Evidence and validation:
  [#329 record](../async-destroy-329.md).


- **2026-11-22. Documentation Phase C (#514).** The entire supplied ticket is
  authorized, without approval or language/API changes. Implementation choices
  under #512 §§12, 14, 16, 18: compile tagged Okkama examples by default; run
  only explicit opt-ins with expected output in a bounded, capability-restricted
  child, otherwise compile only; count actual public declarations and enforce
  only recorded completed std waves. Details and limits are implementation
  choices in the [Phase C record](../std-docs-phase-c-514.md).

- **2026-11-22. Atomic channel selection (#509).** The owner authorized the
  supplied review as the whole scope, replacing Phase A approval with an
  implementation decision record. `SelectedReceiving<T>` is a persistent,
  homogeneous receiver selector in Sparrow; it is not a race of receives.
  One frame registers on distinct underlying channels. A receive commit wins
  over subsequent cancellation, including direct `Within`/`Before` composition;
  cleanup removes all losing registrations. Rotate ready ties after the winning
  index, drain closed buffers, fail only after all sources are exhausted, reject
  empty/duplicate configurations and concurrent requests with typed errors.
  API, state machine and verification: [#509 record](../channel-selection-509.md).

- **2026-11-22. Documentation Phase B (#513).** The owner made the entire ticket
  the scope, without a new approval round or language changes. Open choices
  use the simplest behavior consistent with #512 §§11, 13, 16, 18; implementation
  choices and checks: [Phase B record](../std-docs-phase-b-513.md).

- **2026-11-20. One engine, no selector.** `okkama run` has no `--engine`, and `OKKAMA_ENGINE` and
  `OCS_RUST_ENGINE` are ignored (a4b8ad285). Only `bench/micro/run.py` still passes the flag, and only to old
  toolchains that know it.
- **2026-11-20. Tweed is built on Tomato's pipeline**, keeping every Tomato optimization (persistent
  collections, reduce/map fast paths, JIT). Only the ownership tracking is replaced, in phases (#903). Tomato's
  async bugs are fixed on the way: #587, #797, #817, #845, #846. The board was cleaned the same day:
  - 95 Champ/OIR-2 tickets were closed as not planned;
  - the 0.6.0 milestone was renamed to "Tweed";
  - 30 Tomato tickets were kept: the OH-* ownership roadmap #523 and the JIT, GC and startup items.
- **2026-11-20. The next engine is named Tweed** (Sparrow's next version, after Tomato; "Champ" is retired:
  the static-ownership attempt). Tweed is the next release's engine. Its ownership has one implementation:
  lowering holds the rules, one elaboration pass (static where proven locally, otherwise dynamic), a runtime
  owner tree with O(1) transfers, and one checker in debug and CI only. **Target:** the ownership overhead
  should be at least on par with Python, which it is 4–5× slower than today. "10× Tomato" is not the goal.
  Phase 0 is measurement (Tomato's ownership tax, the share that can be static, a prototype of the owner tree).
- **2026-11-20. Ownership gets one implementation.** Five layers re-derive the same rule. The owner wants to
  throw that out and adopt a proven model after a survey of Swift, Rust, Koka/Lean, Nim, Mojo, Python, Lua and
  Go, even if that means a rewrite. **Decided** the same day: Tweed (above). Don't extend the old own and don't
  chase deep blocker chains in it.
- **2026-11-20. Development instrumentation is a must-have.** It is the `okkama-devtrace` crate, switched on by
  `OKKAMA_DEV`; release builds compile it out (`--cfg okkama_release`).
- **2026-11-20. Code maps.** This directory is read before every task and updated after it.
- **2026-11-20. More code runs on Champ in the Wave CI (#894).** This covers t/, bench/micro (moved from
  okkama-docs; results stay there), examples/, and examples/bible (a copy of the Bible branch's examples). On
  Sparrow a failure fails the job; on Champ failures are only reported.
- **2026-11-20. Immutable containers inherit summaries (#529).** A container made only from summarized parts
  reaches nothing they do not, so it is summarized at birth; summaries are all undone together (`freed`), so an
  inherited one is as valid as its parts'. Machine code reads the same headers the helpers read and calls them only
  when the answer could be yes.
- **2026-11-20. Static resources reuse the ownership census (#909 2b).** The escape question has one
  implementation (`ir/ownership.rs`); `ir/statics.rs` adds only type, region and dominance conditions. A pinned
  resource is invisible to every walk and is destroyed once by `resource.leave`/`resource.unwind`.
- **2026-11-20. Resource identity lives in the object (#528, owner's choice: +16 bytes).** An object whose
  descriptor has a destroy of its own (and is not builtin) carries serial and owner in two hidden words after its
  fields; `Heap::object_words` is the one size rule for heap and JIT. Frames and destroy-less carriers keep the
  identity map. The +8 packed layout was measured and rejected (a region-id table costs back the speed).
- **2026-11-20. Conformance on Champ uses the `gate` profile** (#894 context, wave/76).
- **2026-11-20. The Full verifier is not optimized further.** It stays a CI gate. If it hits its limits, raise
  the limits first (#887).
- **2026-11-16. Claude writes the code.** Fable is the architect and reviewer for async, and since 2026-11-20
  is consulted only for a new model rule, in one compact message.
- **2026-11-15. Champ is Tomato minus runtime ownership.** Every Tomato optimization is mandatory in Champ
  (#826). Performance is measured only once Champ is production-ready.
- **The parity reference is Tomato built from the same tree** (#794), never an installed or released binary.
- **The JVM backend was removed** (#585), and the IR engine before it (#584).
- **2026-11-21. Inlining lives in the JIT, not in OIR** (#541, `docs/inliner-541.md`). Only field accessors are
  inlined; an inlined entry still polls the budget. Inlining a callee that can fail waits for an inline-frame
  table, so that traces keep the callee's frame.

## Std benchmark action inventory (#244, 2026-11-22)

Implementation choices under the owner's generator-only task:

- Read the current checkout's API via production/test-pruned AST JSON, excluding
  runtime and hidden declarations. Include object constructors, role signatures
  and payload enum constructors; exclude caps and non-callable enum constants.
- Use `module:Owner.action`, `Owner.new` for object construction and `Enum.case`
  for payload construction. Group overloads by target while displaying every
  distinct signature; one benchmark does not establish all-overload coverage.
- Count only explicitly called actions in the measured loop, including supporting
  construction there, not fixture setup before timing or implementation internals.
- Scaffolds preserve micro-v2 shape but remain unregistered until a person writes
  representative inputs and independent oracles. No generated checksum guesses.


### m–z measurement expansion (owner task, 2026-11-22)

Use bounded varying inputs and independent mathematical/content oracles, including
an explicit PCG32 implementation from the public reproducibility contract. Measure
pure network/terminal values without opening resources. Count explicit role calls
separately from concrete implementations and declare directly measured support
modules. Skip effectful-only APIs and modules without callable targets. All new
programs preserve protocol v2 and validate measured and warmup checksums at small
and larger N; correctness checks establish no performance result. Evidence and
scope: [m–z record](../../bench/micro/REVIEW-244-MZ.md).

## The Okkama Bible: source of truth (owner, 2026-11-22)

- The Bible is checked against the source code and the behaviour of the current toolchain only; the
  specification in okkama-docs may be stale and is not consulted. Where the book and the code disagree, the
  book is fixed; where the code looks wrong or the intent is unclear, a ticket goes to the milestone
  "Bible findings", to be assigned to releases later.
- The Bible lives in the main repository (`docs/OkkamaBible`) from 2026-11-22; it no longer stays on an
  unpushed branch.

## URL transformation semantics (#133, 2026-11-22)

Implementation choices under the owner's supplied review of 2026-11-21 and
implementation instruction of 2026-11-22:

- Keep existing parsing, `Parts`, canonical equality, resolution and normalization.
  Add methods directly on `UrlParts`, returning validated `UrlParts` rather than
  unchecked successors. The existing constructor gains empty/absent defaults.
- `path_replaced` accepts raw encoded URI/IRI path text, including valid escapes;
  the existing canonicalizer handles Unicode and rejects malformed escapes and
  delimiters. `joined` accepts only plain text segments; percent is data even in
  `%HH`, whereas base-path escapes are preserved. Slash inside a segment is `%2F`.
- Joining appends, never resolves or normalizes dots. No arguments is identity.
  One existing trailing slash is reused; all other slashes and empty segments
  remain. Empty paths begin with `/` if there is an authority, otherwise remain
  relative. Escape colon in the first relative segment; reject an undeclared
  leading `//` through existing structural validation.
- Query replacement takes `QueryParameters`, preserving its ordering, assignment
  distinctions and RFC data encoding. Empty parameters supply `?`; an explicit
  `query_removed()` removes the component. No form-urlencoded plus semantics.
- Port replacement takes `Maybe<int>`: some(0..65535) requires a host, none removes
  the explicit port. No scheme defaults; preserve userinfo and bracketed IPv6.
- Retain existing URL/path/segment bounds and `QueryParameters`' configured
  parameter bound. URL percent-encoding does not promise filesystem safety.

## Ownership model rulings (OIR-2): historical — OIR-2 and Champ were removed on 2026-11-20 (#911)

- **2026-11-20, L13** (6d466cae8). A close's envelope covers the destroy failures that settling any of the
  function's Futures may run (outcome and unstarted inputs). Own adds them to every scope of such a function.
- **2026-11-20, #847, Fable.**
  - A bound Future leaves its binding's scope by `cache.rehome`, and B9 does not carry "bound" across an edge
    that leaves the binding's scope.
  - Codegen admits a static split of an object that holds a resource without its own destroy; the loader
    reconstructs it (#732).
- **2026-11-20, #847, parked** (4a6c0faee, branch feature/847-schedule). A value bound from enclosing scopes
  that can hold service entries makes its owner dynamic (loader: SPB_SCHEDULE). It is not merged and has no
  test.
- **#848, Fable.** `own.accept` and `own.bind` put entries in the bound journal. An admissible `scope.split`
  never selects a bound entry. The loader tracks "admitted" apart from "carries".
- **#870, Fable.** The outcome of a shared await stays in the Future's cache. The value is a view, and its
  binding takes nothing.
- **PR-B1, #784.** A binding keeps the duty, and storing or offering a bound Future is an alias. Own never puts
  a bound Future in an admissible selection.
- **#732.** The loader reconstructs static selections after generation. Codegen's count is only a draft.
- **D1, async destroy.** Ruling PR-K is still only proposed; async destroy is unsupported on Champ.

- **2026-11-22. #329: static async-destroy guards (owner: option (a)).** A frame-owned guard with an own async
  destroy, alone in its scope's region of an async body, is a static resource: `resource.pin` after the
  creation, `resource.settle` / `resource.settle_unwind` (ISA 161/162, SPB 16 additive) at the exits instead of
  `region.track` + `region.adopt` + `region.settle`. The runtime closes the identity once (a pin holder may take
  it), runs the destroy SHIELDED | OBSERVED like `rt_region_settle`, and parks a still-running destroy per
  awaiting frame (`Resources::static_settling`, a GC root; the heap does not move). A guard sharing its region
  stays dynamic. Guard benchmark (2M calls, mode hoisted out of the loop): destroy 0.333 → 0.186 s against a
  deferred close of 0.181 s.


## data_dump unsupported values (#965, owner decision, 2026-11-22)

Keep data_dump primitive: no object dumper, reflection or user-defined actions.
Use the existing NativeFailure contract (core.debug / unsupported_type, native
symbol as operation); reuse data_kind/data_type metadata and point to
`debug/dumper::Dumper`. Sparrow already retains NativeFailure.message in its
report and uncaught metadata. Only the dump argument bridge gets category-only
future/failure values and fieldless object metadata; other inspection behavior
is unchanged. The current Bible example uses an enum and remains valid.


## Overloads with defaults (#993, lead decision, 2026-11-22)

A candidate that takes the call's arguments without filling trailing defaults
beats one that needs them: `f(a)` is chosen over `f(a, b = 1)` for `f(1)`, and a
shorthand constructor over a full one with defaulted trailing parameters for
`Q(a: 5)`. This is the C# and Kotlin rule. Two candidates that both need
defaults still tie. Before 0.6.4 every such pair was a call-site ambiguity, so
no valid program changes meaning. Enforced in `analysis/check/call.rs`
(`no_defaults_first`, next to `fixed_first` for variadics).

## A rebind is a successor of the same object (#984, owner ruling, 2026-11-22)

«Successor должен быть от объекта, а не от конструктора… Правило важнее.» A
rebind `p = …` is valid only when the value is a successor of the object bound
to `p` (`p = p.move()`). A fresh object, another local or any other value is
refused, whether or not the type has successor actions.

## defer on early exits (#997, lead decision, 2026-11-23)

`return`, `next` and `last` run the defers of every scope they leave, inner
scopes first and, within a scope, in reverse registration order, each once.
`throw` is unchanged: obligations are legitimately dropped on it. The flow
check (`analysis/flow.rs`, `run_defers_from`) reports a defer's own error once
however many exits run it.

## SQLite refusals, Unicode versions, process refusals (#1006, owner decisions, 2026-11-23)

- **SQLite: text + precise kind.** A refusal's DbFailure message is SQLite's
  own (`sqlite3_errmsg`; the provider's own refusals keep "SQLite operation
  rejected"), the code stays `SQLITE_<extended code>`. Plain SQLITE_ERROR (1)
  is classified by that message, following PostgreSQL's classes for the same
  failure: a statement that does not compile (syntax error, unknown or
  duplicate name, wrong argument count; PG class 42, including 42P01
  undefined_table) is `syntax`; a limit (PG 54) `resource`; integer overflow
  (22003) `data`; the rest (`SQL logic error`, transaction state) `unexpected`.
  Extended codes: MISSING_COLLSEQ `syntax`, FULL and IOERR_NOMEM `resource`,
  NOLFS `unsupported`. Messages may quote the token a statement failed at and
  schema names, never bound values or the path (`std/db/error.ok`).
- **Unicode versions per surface** (std docs state each, tests pin each):
  character properties 16.0.0 (`unicode.dat`), normalization 18.0.0, display
  clusters 17.0.0 with widths from the 16.0.0 properties (not unicode-width,
  which only rustyline uses), regex properties 16.0.0, CLDR/ICU/UCA as in
  `std/locale`.
- **Async refusals.** An async native's refusal is a Future that has already
  failed, decided by `signature().is_async` (process, unix, network, TLS,
  UDP alike); no hand-kept list.
- **Child pipes** rely on Tokio's `into_owned_fd` clearing O_NONBLOCK (tokio
  1.54.1, `convert_to_blocking_file`), pinned by a test.
- Dead natives `random_sample_accepted`, `random_integer_from_sample`,
  `random_float_from_sample`, `signal_wait` removed (IDs 386–388, 632 retired).

## Name< in an expression (#1004, lead decision, 2026-11-23)

`Name<` opens type arguments only when what follows parses as type arguments
closed by `>`; otherwise `<` compares, so a capitalised binding compares
(`let Count = 2; Count < 3`). `A < B > c` stays a type, as before: read as
comparisons it would order a bool, which is never valid. Bible appendix A.

## Floats past the range saturate (#986, lead decision, 2026-11-23)

The runtime never produces an infinite float from text: `float_parse` gives
±f64::MAX for a literal past the range (std's `Decimal.float` relies on a value
above 1e308 for 1e400), binary32 encoding refuses a value it cannot hold, and
`exp` past the range is refused.

## Reserved system type names (#987, 2026-11-23)

A role or an enum named `Error`, `Maybe` or `Presence` gets "… is the name of the
system role/enum" instead of "already declared". An object named `Maybe` or
`Presence` still shadows the system enum: whether to reserve these names for
every declaration is open (#1005, owner decision).

## Sparrow host net and data hotfixes (#1011, #1012, fixer decisions, 2026-11-23)

- **`test.case` and an uncatchable failure.** The Bible (ch. 15) promises that
  a failed check or a *thrown error* fails one sub-case and the rows after it
  still run; the parser's desugaring catches `Error`, and that already holds. A
  panic or a runtime error is uncatchable (ch. 10): it fails the sub-case it
  happened in and ends the test, the later rows are not run and not reported
  (`okkama-cli/tests/cli.rs` "nested"). Code unchanged; ch. 15 (en, ru) now
  says so.
- **No implicit dual stack.** An IPv6 TCP listener or UDP socket sets
  `IPV6_V6ONLY` to `!dual_stack` explicitly, whatever the system default; with
  `dual_stack: false` a bind to `::ffff:a.b.c.d` is refused.
- **Listen backlog.** `tcp_bound` binds without listening; `tcp_start_listening`
  calls `listen(backlog)` (the system may round or cap it). No peer is queued
  before the program starts listening.
- **DNS failure codes.** `EAI_NONAME`/`EAI_NODATA` stay `not_found`
  (`UnknownHost`); `EAI_AGAIN` is `unavailable` "System resolver is temporarily
  unavailable"; anything else is `unexpected` with the resolver's text. std maps
  every code but `not_found`/`resource_exhausted` to `DnsFailure.because(message)`.
- **Legacy natives stay.** The kind-code SQLite path and `db_capture*` have no
  std caller but are declared in `runtime/database` and `runtime/db`; std loses
  nothing (owner rule), so they are kept and documented, and the floating text
  of the old path is refused (`SQLITE_FLOAT_RANGE`) instead of becoming 0.0.

## One Duration constructor (#1018, owner decision on #1005, 2026-11-23)

- `Duration(seconds is rat)` is the only constructor of `time/duration`
  Duration: exact seconds, a whole number of nanoseconds or `InvalidDuration`,
  no rounding, no implicit int → rat. `Duration(value:, unit:)`, the seven unit
  constructors and `DurationUnit` are gone; do not bring back unit factories or
  wrapper objects (owner). Other units are arithmetic at the call site:
  `(5 * 60).rat()`, `rat(250, 1000)`, `30.rat()`.
- Migration wrote units out as that arithmetic, keeping the unit visible
  (`(15 * 60).rat()`, not `900.rat()`); `value * rat(1, 1000)` where the value
  was already a rat. The printed form `Duration(origin: 2)` (test failure text)
  is the field, not a constructor call, and stays.

## One overload identity (#1005, owner decision 2026-11-23)

- An overload is name + parameter count + resolved parameter types + variadic
  shape, for actions, role signatures, fakes and constructors; a shorthand
  constructor's types are its fields' (declared or inferred), so the check runs
  after field inference. Names, results, async, hidden, successor do not count.
- Reported once at the later declaration with a note at the first, as module
  duplicates already were (no separate note kind in diagnostics).
- The six system type names are reserved for every nominal declaration and type
  parameter; binding names stay free.

## dce after fold (#1006, 2026-11-23)

- A separate, switchable pass (`dce` in OKKAMA_IR_PASSES), not a widened `undef`:
  it drops only unread `const` (and `undef`), renumbering values as `undef` does.
  Listing tests that check lowering use the values they inspect.
