# Decisions and rulings (do not relitigate; extend when a decision is made)

The newest entries are at the top of each section. Each one gives its date, who decided, and where the details
are. Working rules (testing, CI, roles) are in LLM-GUIDE §1 and are not repeated here. Seeded 2026-11-20 from
the session's memory; older rulings live in the commits and tickets named.

## Direction (owner)

- **2026-11-27. #1086 release A/B on two builds per version (lead decision).** Each version is built with
  `toolchain.sh` at LAYOUT_SEED 1 and 2 and runs one campaign per binary, A1 B1 B2 A2 (order balance and binary
  layout in the same four campaigns). The binaries' spread widens the band instead of being reported apart: a
  change smaller than twice it is the runtime's layout, not the code.
- **2026-11-27. #1085 micro harness warms the closing clock path, not `finish` (lead decision).** The span ends at
  the clock read in `Measurement.elapsed_since`; `start()` calls it once before reading the start instant. A quiet
  warm call of `finish` was measured worse (0.76/0.87 ms left against 0.38/0.51), so `finish` keeps one meaning,
  report. The rest (`finish`'s own lazy compile without inlining, ~0.4 ms; the async path after the loop, ~0.5 ms)
  is below 0.13 ns/iteration at the release N; small-N cells are judged by slope.
- **2026-11-27. #1025 micro toolchains are release builds (lead finding).** A micro A/B toolchain is built as
  the packages are (`--cfg okkama_release -C force-frame-pointers=yes`, `-p okkama-cli -p sparrow-cli -p
  blade-cli`). With devtrace compiled in, async programs run 2–5% slower (gates in the Future, await and
  allocation helpers); that, not a commit, was 0.6.4's async "slowdown". Judge a small-N cell by its slope
  over two N (the timed interval holds a fixed ~1.3 ms, #1085), never by one median.

- **2026-11-26. #933 JIT code placement (lead task, implementer measurement).** Keep one page per
  function (entries page-aligned, in-page offsets independent of compile order, now a tested rule in
  `code_memory`); no 16/64 KiB granule, cache-line padding or index-derived pages: on Apple silicon (macOS
  and the okkama-dev container) none of them moved array-plus, float or map-replace beyond run noise, on
  release/065 and on v0.6.1. `SPARROW_CORE_CODE_SHIFT` (bytes, 0–65536, refused when malformed) is the
  layout perturbation; the micro A/B uses it through `run.py --placements`, every placement in every round.
  The v0.6.1 array-plus gap is not JIT placement (it follows the AOT runtime binary's layout); no
  instruction-count mode, because the Docker VM has no PMU.
- **2026-11-26. Closure captures (#1077, lead decision).** An anonymous action
  captures exactly the names its body uses, a nested action's uses included,
  in the order of first use; the enclosing action captures a name for a nested
  one at the nested action's position. Nothing iterates a hash map to decide
  captures; a closure written in a default value sees no caller locals (#149).
- **2026-11-26. Every resource has an owner before the function leaves (#1073,
  lead decision; the owner gave none).** The ownership linearity check
  requires each resource a function makes (an object with its own `destroy`,
  from `obj.new`, a call or `await.take`) to be tracked, adopted or transferred
  by a region, or pinned, on every path to an exit, unless it is the value the
  function returns or throws; `region.leave/settle … keep` frees it again. A
  resource outside any region is an error, and lowering must satisfy the rule
  (a `return` through nested scopes re-tracks the kept value in each next
  region). `await` leaves the outcome with its Future; containers, roles and
  Futures are left to their regions. A `region.enter` of a region that may be
  open is reported at that enter, naming it.

- **2026-11-26. #1029 entry-result report (implementation choice under the
  owner's six-ticket task).** Preserve the bare runtime message used for
  results beyond i64: `main() must return an int in 0..255, got VALUE`, newline,
  exit 1, in both CLIs for every out-of-range integer. Remove the small-value
  CLI prefixes; keep Runtime::run_entry's i64 API and ordinary failures intact.

- **2026-11-26. Test/tool audit (owner, #1067/#1051/#1052/#1055).** Boolean
  switches are OFF when unset, empty, or ASCII-case-insensitive 0/false/off/no,
  otherwise ON, including previously default-ON proof/trace switches. NO_COLOR
  uses nonempty presence. Micro rounds aggregate geometrically, requiring every
  cell VALID; binary versions are checked against declared local revisions before
  measurement. Fuzzing rejects ordinary compile errors but crashes on typed
  compiler invariant failures, codegen failure and encoding failure.

- **2026-11-23. #1006 formatter: header comments, width, std fmt-clean (owner).**
  A block comment between a declaration's parameter list (or result) and its `{`/`;` stays on the header
  line; a `//` comment there opens the body (a bodiless header prints it after its `;`); a list ends at its
  real `)`. Lines over `WIDTH` break at the outermost list, operator chain or call chain on them until
  they fit; the layout with the least overflow wins (no break that gains nothing). A body under a header
  that broke stays one level in from the statement. `okkama fmt --check` over std is a gate step (gate.sh,
  CI `std` job, wave Corpora); generators run their output through the formatter (`tools/okkama_fmt.py`).

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
  character properties 17.0.0 (`unicode.dat`), normalization 18.0.0, display
  clusters 17.0.0 with widths from the same 17.0.0 properties (not
  unicode-width, which only rustyline uses), regex properties 17.0.0,
  CLDR/ICU/UCA as in `std/locale`. #1019 (2026-11-23): clusters and widths
  share one version; `unicode.dat` comes from ICU4C 78
  (`tools/runtime-data/gen-unicode.c`, replacing the Perl 5.42 generator that
  stopped at 16.0), names its version in a `V` line, and a runtime-core test
  compares it with unicode-segmentation's `UNICODE_VERSION`. Name corrections
  (NameAliases.txt) still replace the Name, as Unicode::UCD did.
- **Async refusals.** An async native's refusal is a Future that has already
  failed, decided by `signature().is_async` (process, unix, network, TLS,
  UDP alike); no hand-kept list.
- **Child pipes** rely on Tokio's `into_owned_fd` clearing O_NONBLOCK (tokio
  1.54.1, `convert_to_blocking_file`), pinned by a test.
- Dead natives `random_sample_accepted`, `random_integer_from_sample`,
  `random_float_from_sample`, `signal_wait` removed (IDs 386–388, 632 retired).
  #1020: fs `chown`, `opendir`/`readdir`/`rewinddir`/`closedir` (and
  `DirectoryHandle`), `lock_shared`/`lock_exclusive`/`unlock`,
  `file_state_create`, `environment_get`/`_set`/`_remove`/`_all` and
  `pg_page_rows` removed (IDs 149–152, 167–169, 171, 213–217, 616 retired;
  555 natives). Programs cannot change the environment: os/environment is a
  snapshot, so host::set_environment went too. The legacy SQLite and
  db_capture natives stay.
  #1022: fs `chdir`, `read`, `seek`, `truncate` removed (IDs 161, 163, 165,
  211 retired; 551 natives). Programs have no `chdir`: the run's cwd starts
  as the process's and only a worker's directory (`host::set_cwd`) changes
  it, inside the run's session. A native must have a caller in std or, by
  symbol outside tests, in sparrow-core, okkama-runtime-core or okkama-cli;
  `tools/engines/native-callers.py` checks it (gate, Wave) and its `KEPT`
  list names the 13 legacy SQLite/db_capture natives kept without one.

## A failed TLS handshake is the peer's (#1000, #1021, 2026-11-23)

The HTTP server's handshake deadline races `HttpServerHandshake.secured()`,
never the handshake itself: secured() turns a failed handshake into "no
connection", so a handshake that fails just as a drain cancels it cannot
become a secondary cleanup failure of the drain's cancellation (the runtime
keeps a failure that settled before the cancellation arrived, #663, and a
cancelled `Within` reports its origin's failure). Fixed in std, not in the
runtime: a cancelled operation that fails is a real cleanup failure in
general; only the server knows a failed handshake is nobody's failure.
#1023 makes the peer the boundary for every such race, not one site at a time:
the server runs each peer's `run()` through `HttpServerPeerWork.settled()`,
which absorbs the peer's `Cancelled` with whatever secondary failures its
connection attached (idle and header reads, body reads under their idle
timeout, the handler, writes, the upgrade, cleanups). run() already answers or
drops every other error; a non-cancellation failure of a peer still reaches
the server.

## A TLS record header is checked before its body (#1024, 2026-11-23)

`Tls::feed_record` (sparrow-core native_tls, client and server alike) feeds the
engine one whole record at a time, so it used to wait for as many bytes as any
five-byte header announced: "GET /" read as a record of 8239 bytes, and a plain
HTTP client held a server's handshake until the headers timeout. A header that
cannot start a TLS record (content type outside 20..=24, major version not 3,
body over 2^14 + 2048) now goes to rustls at once, which answers with an alert.

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

## Length limit: pre-check, not catch_unwind (#1033, 2026-11-26)

- A direct helper of machine code that can build a value past the header length
  (2^32 − 1 bytes or elements) checks `heap::passes_length_limit` first and fails
  the program, as `+` and `join` do; `heap::length` stays the panic of last resort,
  caught only by `rt_exec` and `rt_native`. No `catch_unwind` per direct helper.
  Tests lower the limit per thread (`heap::MAX_LENGTH`, test builds only).
## A cut trace keeps its tail (#1034, 2026-11-26)

- A failure trace past `TRACE_LIMIT` keeps the innermost `TRACE_LIMIT − TRACE_TAIL`
  activations and a ring of the 4096 outermost; the omitted ones are between the
  two. The renderers keep their `Fn(&[Site])` shape: the joined trace is rendered
  and one line after it counts the middle, for the root and for every cause.
## Argon2 memory is the provider's (#1026, 2026-11-26)

- okkama-native never calls argon2's allocating entry points (`hash_password_into`,
  `PasswordHasher`, `PasswordVerifier`): they free the password-derived blocks
  unzeroed even with `zeroize`. `argon2_into` owns and zeroes them; PHC making and
  checking mirror the crate's code over it (tests compare with the crate).
- A crate whose `zeroize` feature a wrapper does not forward gets a direct
  dependency only to enable it (`ed25519` for `KeypairBytes`).
## Every new opcode bumps SPB (#1061, 2026-11-26)

- `codec::VERSION` changes with every new instruction, operation-enum code or
  operand list, not only with encoding changes: a runtime that cannot run a file
  refuses it by version. A test pins (VERSION, instruction-set fingerprint); when
  it fails, bump the version and record the new pair.
- SPB 17 was Champ's format (retired, #911) and is skipped: SPB 16 → 18.
  The assembly text version stays equal to the SPB version.

## Model lookups the printer uses are fallible (#1064, 2026-11-26)

- `Role::slot_name` returns `Option`, like the other lookups by index (`Module::role`, fields, enum cases): the
  printer must print a damaged module (#1003), so it never indexes; only code after `ir::verify` may unwrap.
## `^C` passes for `C` only on unwind edges (#1063, 2026-11-26)

- The cleanup relaxation of OIR edge types (`^C` and `C` compatible) is for `block.unwind` targets only. The
  handler of `catch` and the `found` target of lookups also fill an implicit last parameter but are normal
  paths: a successor keeps its type there and its obligation is followed into the parameter.
## `blade new` writes only what it creates (#1039, 2026-11-26)

- An existing `NAME` (any file or directory) is refused, like `cargo new`; an existing empty directory too.
  The name must pass `manifest::check_package_name`; the default output has one definition,
  `project::default_output`.
## A shift does not close type arguments (#1032, 2026-11-26)

- `Name<…>` (#1004) is a type only when its closing `>` is a whole `>` token or the second half of a
  `>>`/`>>>` that a nested type closes too; a type closed by the first half of a shift would leave the
  other `>` comparing a type value, so the comparison reading wins (`Count < a >> 2` = `Count < (a >> 2)`).
  `>=` is not treated so: neither reading of `Count < a >= 2` is valid.
## undef stubs are checked by the SPB verifier (#1069, 2026-11-26)

- A read of an `undef` stub is refused by the verifier, not checked by Sparrow: the JIT trusts the verifier's
  initialization proof ("Registers start unbound; the verifier proves defined use", jit.rs), and a check at
  every dereference would tax every program. The rule covers the compiler's two shapes, a constructor field
  bound on some arms and a `for` in value position: a `bool` guard false wherever the stub may remain, read by
  `br`. `obj.new`/`obj.init` may store stubs (a constructor whose `self` escapes early, #271, needs it: std/http
  breaks otherwise), so the heap is outside the rule.
## The register check refuses early writes (#1043, 2026-11-26)

- An instruction's result and a terminator's edge value (the found value of `arr.lookup`/`map.lookup`, the
  value `catch` gives its handler: the target's parameters beyond the edge's arguments) may be written before
  the operands are read, so `codegen::sparrow::check` refuses either in the register of an operand of the same
  instruction or terminator. This is the allocator's conservative rule (#1007), now checked independently of
  it; the JIT happens to write the found value after the edge moves, and the rule does not rely on that.
- `TermKind::targets`/`targets_mut` and `operands`/`operands_mut` come from one macro each, as `InstKind`'s
  operands (#1006).
- The current IR is OIR-2 in names and in its text (`oir 2 high`); "OIR-1 §N" stays where it cites a section
  of the OIR-1 specification (`okkama-docs/docs/compiler/OIR-1.md`), whose numbering the code still follows.
## One adopt for an awaited result; strict `--passes` (#1044, 2026-11-26)

- The result of `await.take`, tracked as it is taken, then adopted by its binding: the adopt replaces the track
  (`FnBuilder::adopt_tracked_await`), and the take-extraction pass emits neither track nor adopt when the
  binding adopts right after (temporary or bound Future, #999). Track then adopt of one value in one region
  equals the adopt (`region_add` with `own` takes every identity the track would follow). Call results bound
  by `let` (`call`, `region.track`, `region.adopt`) are left as they are: not in #1044's scope.
- `okkama ir --passes` refuses a name that selects nothing (usage error, as `--after`), through
  `lowering::unknown_passes`, the one list check; lowering warns once for `OKKAMA_IR_PASSES` or a list
  selected through the library and names which.
## Blade rebuilds by content, locks local paths from the project (#1040, 2026-11-26)

- `blade run` decides by a stamp beside the output (`OUTPUT.blade-stamp`): SHA-256 of the input set (`.ok`
  files outside the output's directory and hidden directories, `blade.toml`, `blade.lock`; each by path and
  bytes) and of the output the build wrote. No modification times: a deleted file or an older copy must
  rebuild, and hashing a project's sources each run is cheap next to a build.
- A `path` dependency is `path+<dir relative to the project root>` in `blade.lock` (canonical directories, `/`
  separators); an absolute lock is not translated (pre-beta, no shims): `blade update NAME` rewrites it.
- `blade install` is removed (pre-beta); local directories are declared by hand in `[dependencies]`.

## `unreachable` compiles as a trap (#1072, 2026-11-27)

- `unreachable` stays in SPB (opcode 145, verifier and codegen unchanged): Sparrow compiles it as
  `trap unreachable` would, a program error "Unreachable code reached" that a handler catches, so a verified
  function holding it runs. Dropping it from the instruction set would be an SPB version bump for nothing.

## Closures of any width (#1074, 2026-11-27)

- Supported rather than refused: a verifier limit would have been the only width limit of SPB (static calls
  take any number of parameters), and an Okkama lambda of 9 parameters would have failed in the verifier at
  compile time. Rust's calls of compiled code (`jit::invoke`, used by the closure helper, `run_function_with`
  and intrinsic calls) take up to 8 words through named signatures; a wider one goes through `Jit::spread`, a
  Cranelift adapter per arity that loads the words and calls the code. It holds no value across the call and
  is not in `ranges`, so the collector passes over its frame as over a Rust one. Machine code calls a wide
  closure directly, as a narrow one (the `args + captures > 8` cut is gone).

## A stub in an object field is checked where it is read (#1076, 2026-11-27)

- The verifier keeps letting `obj.new`/`obj.init` store an `undef` stub (early `self`, #271); Sparrow checks
  the reads instead. `Program::stub_fields` (okkama-sparrow/src/verify.rs) lists per object type the fields
  some `obj.new`/`obj.init` may store a stub in: flow-insensitive over `undef`, `move`, edge and handler moves,
  so a superset of what the initialization analysis lets through; per type, since `obj.next` copies fields
  within one type and `variant.of` keeps the descriptor. `Loaded::new` computes it once (functions without
  `undef` are skipped: 10 ms over 1245 corpus programs, 0.7% of their verification).
- Only reads of those fields pay: `obj.get`, `variant.get`, and a #541 accessor of such a field stays a call.
  A session runtime checks every field (`Loaded::check_every_field`), since an extension may store a stub in
  a published field whose readers are compiled already.
- The stub of a `bool` field reads as `true`; any other stub fails the program ("Field `x` of `T` is read
  before it is bound", catchable). The compiler's construction check (`ObjPending`, #271) reads an unbound
  `$pending` as "still under construction" and relies on that; it is the one compiled read of a stub.
- Rust helpers that walk fields take a stub (`word::unbound(repr)`) as unbound: equality refuses it (as
  before), test output prints `null` (as before), the collector and ownership walks skip non-pointers (as
  before); map keys refuse it, `data_items` refuses to box it, `data_kind`/`data_dump` read it as undefined.
