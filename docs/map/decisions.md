# Decisions and rulings (do not relitigate; extend when a decision is made)

The newest entries are at the top of each section. Each one gives its date, who decided, and where the details
are. Working rules (testing, CI, roles) are in LLM-GUIDE §1 and are not repeated here. Seeded 2026-11-20 from
the session's memory; older rulings live in the commits and tickets named.

## Direction (owner)

- **2026-11-27. Trunk-based branches (owner).** «main — источник релиза, туда льем только через пулл-реквесты,
  прямые слияния веток отвергаем; develop — общая песочница для всех изменений, на релиз не влияет». Release
  branches are cut from `main`, features leave and rejoin the release branch, a merge into `develop` is a nightly
  build (`okkama-nightly-YYYY-MM-DD`), the release ends with a pull request into `main` and the tag on its merge
  commit. A ruleset on `main` enforces it; LLM-GUIDE §10 has the steps.
- **2026-11-27. #1100 debug tests before the merge (lead decision).** The Wave and CI run `cargo test` of
  okkama-compiler and okkama-cli in the dev profile (debug assertions, the ownership linearity check) besides the
  gate profile; the nightly's workspace debug run stays. Debug info is line tables only on the runners (disk).
- **2026-11-27. #1101 okkama-cli tests start okkama through `tests/tree` (lead decision).** `tree::okkama()`
  sets the tree's std; no test relies on the binary's own std lookup, and `cli.rs` refuses a file that names
  `CARGO_BIN_EXE_okkama` itself. A test of another std sets `OKKAMA_STDLIB_DIR` again on that command.
- **2026-11-27. #1095 one switch implementation in okkama-devtrace (lead decision).** The environment-switch rule
  lives once, in `okkama_devtrace::env_switch`: the lowest crate the six readers share, with no dependencies, and
  not compiled out in a release (only its gates are). No new crate; okkama-runtime-core, whose only switch is a
  test's, takes it as a dev-dependency. Old paths stay as re-exports, so no call site changes.
- **2026-11-27. #1097 a runtime that cannot be built is a load error (lead decision).** Everything before `main`
  runs (decode, verify, intrinsic contracts, JIT setup such as `SPARROW_CORE_CODE_SHIFT`) is reported with the
  launcher's prefix (`okkama:`/`sparrow:`), exit 1 as before; only a failure of the run itself keeps the runtime
  report's formatting. Before, setup failures printed without a prefix under `run` and `exec`.
- **2026-11-27. #1096 the update report covers the lock (lead decision).** A lock change that `pkg/` already has
  is the pkg line's text plus ` in blade.lock`; a package already reported for `pkg/` gets no second line.
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
- **2026-11-27. A bound async action's Future is tracked like a direct call's (#1102, coordinator's task, lead
  design).** `call.fn` of an async action value returns a lazy Future that owns the call's fresh arguments
  (#587), so lowering tracks it in the scope. An immediately taken one follows #932 (`awaited_temporary` accepts
  `call.fn`). The bound action's closure, when it is defined in the function, decides driver or not. An unresolved
  one (a parameter) counts as a driver, so the track follows the take: the conservative form, which costs a
  skipped add of a spent Future.
- **2026-11-27. Closure and cap-field results are tracked like call results (#1099, coordinator's task, lead
  design).** `BodyLowerer::emit` tracks the result of `call.fn` and `call.cap` in its scope as it does for
  `call`/`call.role`/`await.take` (a Future result excepted: its rules are #932/#587).
  `lowering::untrack_borrowed_results`, run once the module is whole, drops that track where the ownership census
  reports no creation (the census, not the type, decides: the same notion as the check, #1093), so borrowed
  results stay untracked as before. The census is computed once and handed to the statics pass, which it does
  not invalidate (the census reads no region operation); `okkama check` skips the step.
- **2026-11-27. The linearity check takes freshness and argument ownership from the model (#1093, lead
  decision applying #553/#555).** A call result is a resource the function must own only where the ownership
  census reports a creation site (`ownership::analyze`; call resolution shared via `ownership::resolved_call`),
  never by type alone. A call whose callee adopts the parameter (its body has `region.adopt` of it, as kept after
  the param-adopt elision) owns the fresh argument from the call on, including the call's unwind edge. A branch
  moves a resource's ownership state into the block parameter it is passed to (coordinator, from the book
  review). The `statics` pass removes the regions it emptied itself instead of relying on `regions` after it.
  A `LoweringError::Internal` message is bare text; the kind is named once by whoever prints it (Display adds
  "internal compiler error: "). A discarded `call.fn`/`call.cap` result that is a new resource is a real leak
  in lowering, refused by the check (#1099), not a false positive.
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
  otherwise ON. Correction (9d7749f4e): the default-ON switches keep their default — OKKAMA_TRACE,
  SPARROW_CORE_POINTER_FREE and SPARROW_CORE_INT_RANGE are ON when unset (`enabled_or(name, true)`),
  and 0/false/off/no (or empty) turns them off. NO_COLOR
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

## Blade's stamp covers the toolchain (#1080, 2026-11-27, no owner comment: own design)

- The toolchain is what the `okkama` Blade runs says in `okkama env` (version line, `binary`, `std`); Blade
  does not resolve std itself (`OKKAMA_STDLIB_DIR`, install root, repository ancestors stay in one place,
  okkama-cli `stdlib_dir`). The stamp records the version line and SHA-256 of the binary and of the std tree
  (every non-hidden file by tree path and digest), by content only: the same std or binary at another path
  reuses the output (an SPB holds no std paths).
- Stamp format `blade-stamp 2` as the first line; up to date only on an exact match, so a stamp of another
  format (#1040's `inputs`/`output` alone) rebuilds once. No compatibility reading of old stamps (pre-beta).
- Digests cached per file in `OUTPUT.blade-cache`, keyed by (path, size, mtime) plus on Unix (dev, inode,
  ctime): ctime cannot be set back, so a `cp -p` or a restored mtime still rereads. The key decides only
  whether to reread, never whether to build; a file younger than 2 s is not remembered (racy-clean, as git).
  Measured: uncached hashing ~29 ms per no-op run (22.6 MB binary, 7 MB std), cached ~1 ms, so the cache is
  needed; per output beside the stamp (no global cache dir).
- `okkama env` (~3.7 ms to start) runs while Blade fingerprints the project, and the std the cache last saw
  is listed on a thread meanwhile, used only if `okkama env` names the same directory. The remaining +~4 ms
  per no-op run is the price of asking `okkama` itself; caching its answer would duplicate std resolution.

## Blade's stamp covers import directories and build variables (#1087, 2026-11-27, own design)

- `okkama env` stays the one place: it names each `OKKAMA_IMPORT_DIRS` directory (`import`, search order, the
  same split as the loader through `import_dirs`) and each set variable of `lowering::BUILD_ENVIRONMENT`
  (`build NAME="VALUE"`, the raw value Debug-quoted so it is one line). Blade does not know the variable names.
- Build variables are those that change the SPB: `OKKAMA_IR_PASSES`, `OKKAMA_OWNERSHIP_PROTOTYPE`. Checks,
  reports, profiles and dumps (`OKKAMA_VERIFY_OWNERSHIP`, `OKKAMA_CHECK_REGISTERS`, `OKKAMA_PROFILE_REGISTERS`,
  `OKKAMA_OWNERSHIP_REPORT`, `OKKAMA_DUMP_INVALID_IR`) and runtime or diagnostic switches outside the compiler
  (`OKKAMA_TRACE`, `OKKAMA_DEV*`, `OKKAMA_ARRAYS`, `OKKAMA_MAPS`, `OKKAMA_ICU_LIBDIR`) are not: a check may fail a
  build but never changes what a passing one writes. `build_environment_tests` fails on an unclassified literal.
- An import directory counts by content and position (`import` lines in order, `missing` for an absent one);
  with no import directory and no build variable the stamp text is #1080's, so outputs stamped by #1080 stay
  valid. `okkama env` runs in the project root, where `okkama build` runs, and Blade joins relative paths to it.
- Cache format `blade-cache 2`: `tree` lines for every tree listed (std, then imports), all listed on one thread
  while `okkama env` starts.

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

## Fuzz harness contracts (#1088, #1089, #1050, lead, 2026-11-27)

- The module of a runtime intrinsic is user input: any source may say `module async/barrier`. Lowering checks
  the shape the runtime needs where it lowers the intrinsic (`lowering/intrinsics.rs`, `Site::refuse`) and
  reports a semantic error at its declaration. The runtime keeps checking layouts at load (`sparrow-core`
  `contracts.rs`). Nothing in analysis duplicates the check.
- Unlinked OIR is valid OIR: `lower_program` keeps imports unresolved (§2.1), the verifier accepts them, and
  codegen refuses them. A harness that runs code links first (`fuzz/fuzz_targets/support/linking.rs`). Only
  `lower_linked*` output is a program.
- The fuzz coverage replay runs with the same ASan options as `cargo fuzz run` (`detect_odr_violation=0`).
  The SQLite `aiOff` report is a false positive: two identical internal-linkage tables at one address. It is
  not a duplicate link.
- The exec target links against the repository's std tree (#1091, 2026-11-27): one parse per module and
  process, plain std names only, budgets unchanged, no host natives (`Limits.natives = Some([])`).
- An intrinsic's thrown error is the object of its case's own payload (#290), and its fields are exactly
  what the runtime raises, in count and kinds (#1090, 2026-11-27). The `[$tag, payload arrays]` layout is
  not a thrown error any more.
- Every tracked `test_*.py` runs in CI through `tools/test-python.sh` (#1092, 2026-11-27). A tool test
  needs no toolchain, Docker or network.
- A native whose canonical symbol (`module@major::name/arity`) is in the ABI table must have the table's
  signature, or lowering refuses it at the declaration (#1098, 2026-11-27). A symbol outside the table
  compiles, and the runtime refuses it, as the SPB verifier does.
- A fuzz finding's input is kept in `okkama-rs/fuzz/regressions/<target>/`. `run.sh` adds it to the corpus,
  and a test replays the target's logic on it.

## The playground's compiler in wasm (#1078, 2026-11-27, owner-approved ticket; design by the implementer)

- **okkama-runtime-core has a default feature `native`**: the modules `native`, `host` and `data` and every
  dependency only they use (okkama-native with aws-lc and getrandom, libc, the C zstd, Brotli, miniz,
  unicode-normalization/-segmentation, md-5, sha1). okkama-compiler depends on it with `default-features =
  false`: it uses only `Value`, `IntValue`, `heap` (arrays, map keys), `ops`, `integer` and `float` (constant
  folding and value types), and no compiler code reached the gated modules. Cutting the dependency instead
  would have moved the value types out of the crate the runtime shares them from (two copies of one rule).
  Every runtime (sparrow-core, okkama-cli) keeps the default, so a native build unifies to the same features and
  compiles the same crate: corpus SPB byte-identical (std -t 499, t/ 51, examples 264, okkama-rs/examples 13,
  Bible 181, conformance 541: 1549 built programs), check/fmt output identical on all 2220 files.
- **`okkama_compiler::commands`** is the one place that renders what check, fmt, tokens, ast, ir and build
  report (`Finding`: file, span, severity, kind, message and the printed text; `Report`: findings and the whole
  text), including the missing-docs gate, the two-profile check of #649, the merge of their findings, pass
  selection (`passes_named`, `passes_after`), the parse of a found import (`commands::module`) and the editor's
  two-pass inspection (`commands::inspect`). okkama-cli, its OCS adapter and okkama-wasm call it; the CLI keeps
  only files, flags and exit codes. Nothing in it reads a file or the environment: sources come from the
  caller, imports from the `ModuleLoader` it passes (a factory taking the test mode).
- **The playground's loader** (`okkama-wasm` `Loader`) follows the CLI's search order without Blade packages:
  a std file looks in std, then beside itself; any other file beside itself, then in the import directories,
  then in std; a module never imports itself. Files are the bundled std under `std/` plus what the page sets
  (`setFiles`). The CLI's `FileLoader` stays in the CLI (it needs the file system and blade.toml).
- **The std is embedded** by `okkama-wasm/build.rs` (`include_str!` of every `std/**/*.ok`, unchanged, so
  positions in std match the CLI): 499 files, 5.97 MB of the 9.6 MB wasm.
- **API**: each export takes one JSON request and returns one JSON response. Commands answer `{ok, code,
  stdout, stderr, diagnostics}` with `stdout`/`stderr`/`code` exactly the CLI's; a diagnostic is `{file,
  severity, kind, message, text, span: {start, end} (bytes) | null, range: {start, end: {line, character}}
  (zero-based, UTF-16) | null}`. Requests: `{source, path = "main.ok", tests?}` for check/ir/spa, plus
  `passes`/`after` (ir), `format` (tokens: text|json; ast: tree|json), `mode`, `spans`, `pruneTests` (ast);
  `{source, path, line, character, trigger?}` for hover → `{hover: {markdown, range} | null}`, definition →
  `{definition: {path, range} | null}`, signature → `{signature: {label, parameters, active} | null}`,
  complete → `{items: [{label, kind, detail, documentation, insert, snippet}]}` (kind: method, field,
  variable, object, role, enum, enumMember, function, keyword, module, argument, folder), in the order to show
  them. The IDE calls answer from the last inspection of the file that parsed (since #1128: the last analysed one,
  `ide::Answering`), as the language server does
  (recorded by each IDE call; the server records one per change). `version()` → `{version, stdModules,
  stdBytes, passes}`, `passes` the IR passes in order (`lowering::PASSES`, #1118). `lastPanic()` → `{panic:
  {message, location: "file:line:column" | null} | null}`: the last call's panic (each call clears it; the
  hook is installed by the first call), read from the trapped instance before the page starts a new one.
- **`std.…` completion reads a `StdTree`** (#1115, 2026-11-27): `key`, `modules`, `stamp`, `entries`, `source`
  (the stored text the stamp describes) and `unsaved` (an editor's text, applied on top of the cached index and
  never cached). `ide::DiskStd` is the language server's std directory (stamp: each file's length and mtime;
  native only); okkama-wasm's `BundledStd` is the embedded std, a constant stamp, with the page's files under
  `std/` as unsaved texts. The wasm sorts the items as the editor shows the server's (`sortText` = deep, label).
  The parity test compares them with `okkama lsp`'s, item by item.
- **The compiler's only host calls are in `host.rs`** (#1116, 2026-11-27): `host::Clock` (the
  `OKKAMA_PROFILE_REGISTERS` timings; zero on wasm32) and `host::dev_output` (`OKKAMA_DUMP_INVALID_IR`,
  `OKKAMA_OWNERSHIP_REPORT`; dropped on wasm32). `ide/std_disk.rs` is native only. File identity is the
  loader's (`ModuleLoader::identity`, default the name; the CLI canonicalizes). okkama-devtrace treats wasm32
  like `okkama_release`. `host::tests` greps every other source file (comments and the trailing test module
  excluded) for file-system, clock and process calls.
- **How diagnostics name files** (#1119, 2026-11-27): a file the user named keeps the user's spelling; a file
  found through an import is named relative to the current directory when it is under it, and by its canonical
  path otherwise (`source::display_name(found, base)`, lexical). The CLI's `FileLoader` names loaded modules so
  (`LoadedModule::id` stays canonical; `commands::module(id, name, …)`), and run-time traces use the same
  function. The language server keeps whole names (they are URIs) and names a relayed import error from the
  checked file's directory. `okkama man` keeps whole names in its loader (the export records them) and names
  its warnings by the rule. The playground's names are the page's, relative to its root, its current directory.
  The parity test runs the CLI in the repository root on relative paths against a page-like workspace (`std/`,
  root-relative names), unnormalized, with imported files' diagnostics in the sample.
- **The playground page is published with the release** (2026-11-27), like the /man/ reference: the site repo
  (okkama-site, `playground/`) is rebuilt per release by `playground/tools/build.sh CHECKOUT [WASM_BINDGEN]`
  (wasm, wasm-bindgen of Cargo.lock's version, `pkg/manifest.json`, `examples.json`).
- **Parity is a test of the CLI binary** (`okkama-cli/tests/playground.rs`): `okkama check`, `fmt` and `ir`
  against `Workspace::check`/`fmt`/`ir`, and `spa` against the disassembly of `okkama build`'s SPB, on t/, the Bible's rejected examples, six std modules and the fixtures of
  `tests/playground/` (a warning, a module shadowing std, an imported fake), byte for byte with the exit code.

## Sparrow's interpreter, synchronous part (#1120, 2026-11-27; design Fable, `okkama-docs/runtime/sparrow-interpreter-1079.md`; implementer's choices where it leaves room)

- **The helpers are called as they are; `rt_exec` is not split.** The interpreter calls the `extern "C"` helpers
  (`ops::rt_exec`, `failures::rt_failure_op`, `rt_native`, `rt_intrinsic`, `rt_future_new`, …) directly, with the
  instruction's uses gathered from its registers into a stack buffer in visit order (the plan keeps the use list
  per instruction, as the JIT keeps `operands`). `execute` and `Operands` are untouched: no second access path,
  no second monomorphisation, the JIT's `rt_exec` source byte for byte as before (the design's split was for a
  generic register-file access; a copy of a few words per generic instruction costs less than the risk).
- **The mode lives in `Runtime`, not in `Loaded`.** Synchronous roots read every register by kind and do not
  depend on the pointer-free proofs; #1122 moves the choice to load time when async frame layouts need the
  proofs off.
- **Refusal at the moment of need.** A failure at an instruction an active handler covers, a `catch`, and an
  async body (`Runtime::resume`, the suspending instructions) stop the run (status FATAL) with "`symbol:pc`: … is
  not supported by the interpreter yet (#1121/#1122)". A program with handlers that never fails runs. No
  refusal at load.
- **Regions and resources already run.** The `failure_or_region` family (regions, `resource.unwind`,
  `fail.*`, `raise`, `panic`, `role.narrow`) goes through `rt_failure_op`, `resource.pin`/`leave` through
  their helpers: they need no handler, and a destroy is the seam's re-entry. #1121 keeps handlers, `catch`,
  unwinding through them and the differential test.
- **Own branches only where the JIT has no helper.** Every `program::generic` instruction runs `rt_exec`, also
  where the JIT has an inline fast path (the PLAIN reference); the 48 own branches follow the emitter (small
  integers inline, else `rt_int`/`rt_int_order`; a non-finite float through `rt_float`; allocation from the free
  list, else `rt_allocate`; `rt_created` where the type can reach a destroy; the stub check of #1076). A call of
  an accessor or of a small callee is an activation: the JIT's #541 and #926 shortcuts change no step, failure
  or trace.
- **Limits.** The register stack takes at most the run's stack bytes (1 GiB; under a memory budget the capped
  stack, `stack_capped` making it "out of memory"); a re-entry from Rust checks the Rust stack against
  `Ctx.stack_limit`, as the JIT's prologue checks SP. No re-entry counter yet: the design gives it to the async part (#1122).
- **Shared code moved out of `jit.rs`.** `program.rs`: `generic`, `failure_or_region`, `suspends`,
  `dispatch_table`, `role_slot_is_async`; `texts.rs`: the four emitter texts. Moves only, no change in the JIT.

## Sparrow's interpreter: handlers and the differential test (#1121, 2026-11-27; design Fable; implementer's choices)

- **Unwinding mirrors `check`.** For each activation from the innermost: record (`rt_trace`, keyed by its
  register file), then take the first handler whose range covers the instruction (ranges end before `end`, as
  the JIT reads them); FATAL and EXHAUSTED pass every handler. The handler's edge is a step when it goes back,
  its moves read before they write, the failure goes into the handler register last; `ctx.failure` keeps the
  failure, as in the JIT.
- **One observation API, not a second runner.** `sparrow_core::observe(Engine, bytes, arguments, limits)` is
  `run_described` with the engine chosen in the call (`Runtime::use_engine`, before anything runs) and both
  streams captured; the environment switch stays the way to choose it for a process.
- **Standard error has a sink.** `okkama_runtime_core::stdout::write_error` is the one way `stderr_write` writes;
  inside `capture_streams` it appends to the capture, otherwise it flushes standard output first, as before.
- **Coverage is dynamic.** A test-only recorder in the interpreter's loop collects the mnemonics a run executes;
  the coverage test runs every `.spa` with an entry of the SparrowBook, sparrow-core's tests, `tests/mnemonics`
  and sparrow-cli's handler programs on both engines and fails while a synchronous mnemonic is not executed.
  Templates that do not assemble on their own are skipped (their tests complete them). `fut.spawn` counts as
  async: spawning runs a body (#1122).
- **The differential samples are fixed lists**, not the whole corpora (CI time, debug profile included): t/
  base, op and lang; sixteen examples whose output depends on the program alone; every single-file conformance
  case expecting a run-time failure that builds (at least 50). Whole corpora stay #1123's (`sparrow-interp`
  configurations).

## The interpreter in the test runs (#1123, 2026-11-27; design Fable §6.2–6.3; implementer's choices)

- **One exceptions file for every runner.** `okkama-rs/tools/engines/interp-exceptions.txt`, read by
  `differ.py`, `tools/t/run.py` and (through `differ.py ocs`) the Wave and the gate. Selectors: `refusal:` (the
  interpreter's own refusal), `both:` (both outputs contain it, statuses equal), `either:`, `jit:`, `path:`; each
  line names a ticket or §6.3, or the file does not load. The fuzz `exec` differential mode keeps the same four
  rules in Rust (`linking::differ`), with no file reader in the fuzz target.
- **A memory budget is `either`, not `both`.** The interpreter's register stack is allocated memory and the JIT's
  machine stack is not, so a deep recursion under the fuzz budgets stops on steps under the JIT and on memory
  under the interpreter (found by the differential test). §6.3 lists the moment of memory exhaustion as a known
  divergence; the exception follows it, for either side.
- **Report-only first.** The Wave's `sparrow-interp` matrix branch has `continue-on-error`, the gate's step
  records `informative`; both become blocking after the first green run (design decision 11). The JIT's
  branch is unchanged: `SPARROW_CORE_INTERPRET` is empty there, which the switch rule reads as off.
- **The interpreter's t/ run compares with the JIT in one invocation** (`--configs sparrow,sparrow-interp,
  sparrow-interp-gc-stress`): t/run.py already requires one TAP stream of all configurations. The GC-stress
  interpreter gets four times the timeout (#1120's 105 s file); ten since #1122's follow-up: on the Wave's debug
  adapter t/gen/persistence/04 needs 121 s locally (JIT under stress 57 s) and ran out of 240 s on CI at 520 of
  618 checks; with slow runners (x1.86, #843) 600 s. Chosen over a subset: the branch is blocking and should
  see the whole suite; only a hang waits the full timeout.
- **An extension of an interpreter's session loads as the session did (#1122 follow-up).** `Runtime::extend`
  drops the pointer-free proofs of the new `Loaded` when the runtime has an interpreter, before the prefix
  checks: otherwise every published function's metadata and frame layout differ and every extension is refused.
- **The seeded JIT defect needs no reference build.** The mutated build's interpreter does not run the emitter's
  code, so `okgen.py --engines sparrow,sparrow-interp` on that one build finds it; the existing shared-helper
  mutant keeps its unmutated reference.

## Sparrow without the JIT and the host, wasm32-clean (#1124, 2026-11-27; design Fable §7, §4.2; implementer's choices where it leaves room)

- **Features, as the design's §7.1.** sparrow-core `default = ["jit", "host"]`; okkama-runtime-core's `native`
  (still the default) is `providers + host + zstd + brotli + crypto`; okkama-native `aws-lc` (default). Every
  binary and every other crate's tests build the defaults, so `cargo tree -e features` of okkama-cli and
  sparrow-cli changes only by the new feature names of these three crates (every external crate's version and
  features identical). `jit` or `host` on wasm32 is a `compile_error!` that names `--no-default-features`.
- **sparrow-core depends on runtime-core `providers + crypto` and on okkama-native without `aws-lc`
  unconditionally; `host` adds the rest.** RustCrypto digests, HMAC, HKDF, AEAD, ECC and argon2 stay in the
  no-host build (sparrow-core's own `native_crypto`/`native_secret`/digest handles use them); what the playground
  allows is the profile's question (#1125, owner decision 6 of the design), not the build's. zstd (C) and Brotli
  are left out without the host; their formats are then refused as `unsupported` ("zstd is not available in this
  build"), not as unknown.
- **What `host` gates in sparrow-core** is the design's list, files and sockets, plus `native_glob`, `path_syntax`
  (path natives use the host's path rules) and the worker modules. `native_net` (numeric IP parse/format) and
  `native_postgres` (DataRow pages, no socket) are pure and stay: the design listed them with the host, but they
  touch no OS. `native_db`, `native_input` (stdin through `std::io`), `native_io` (the request machinery) stay.
  runtime-core's `native::idna` (public suffixes) serves only `x509`'s name checks and goes with it.
- **The bridge routes only what runtime-core provides.** `native_origin::handles` (the catch-all for synchronous
  natives) also requires `okkama_runtime_core::native::provides` without the host, so a native neither side has
  is unsupported (`--native-capabilities`, `natives::supported`) instead of routed to the bridge's refusal; with
  the host the condition is constant true. `origin_routes_match_the_runtime_core_providers` holds in both builds.
- **`Engine::Jit` exists only with `jit`** (#1121's `observe`/`Runtime::use_engine`): a build without the JIT
  cannot be asked for it.
- **Without `jit` every runtime interprets** (`Runtime::assemble`: `!cfg!(feature = "jit") ||
  SPARROW_CORE_INTERPRET`), `invoke`/`resume` call the interpreter directly, the collector's JIT branch is
  `unreachable!`, `frame_pointer!` is 0. The JIT path's `invoke`/`resume`/`*_cold` bodies are unchanged under
  `cfg(feature = "jit")`.
- **Process workers without the host behave as a runtime without a worker command** (`worker_absent.rs`; replaced
  by the emulation of #1127, below): `bind`
  copies the action (`Runtime::closure_copy`, shared with `worker.rs`), a pool hands the plain action back, closing
  one is done, `spawn` refuses with `texts::NO_WORKER_COMMAND` (the text `worker.rs` gives without a command). The
  `WorkerBinding`/`WorkerPool` states exist only with the host.
- **The VM thread** stays natively without the host (deep recursion needs its 1 GiB); only the host session
  (`okkama_runtime_core::host::enter`) is the host's. On wasm32 the run takes the caller's stack.
- **`clock`.** `clock::Instant` is `std::time::Instant` natively (timers, statistics, JIT-path code unchanged)
  and a virtual timeline on wasm32: `now = installed source + skipped`, `skip_to(deadline)` never goes back, the
  wall clock is the source's wall clock at installation plus the timeline's progress. Without an installed source
  time stands still except for skips (a deterministic run). The embedder (#1125, sparrow-wasm) installs
  `Source { monotonic, wall }` as plain function pointers: sparrow-core has no wasm-bindgen. The time natives
  (`wall_clock`, `monotonic_clock`, `clock_domain`'s process number) read `clock`.
- **`io.rs`: one portable part, three waits.** The woken queue, requests, wakers and the firing of due timers are
  shared by `wait_external`; the wait is `wait_tokio` (host, the old body unchanged), `wait_sleeping` (native
  without the host: a condition variable the waker notifies holding its lock, woken by the earliest timer and the
  interrupt look) or `skipping` (wasm32: a fired waker needs no wait; else skip to the earliest timer; requests
  alone cannot progress there, so false and "no task can make progress").
- **`sort_or_stop` under `panic = "abort"`** is a stable bottom-up merge sort over two copies whose comparison may
  stop it, the slice taking the result only when the sort completes; the unwinding version stays the native one.
  Both are tested natively (`sort_tests` run the same properties on each).
- **Budget arithmetic in `i64`**, not `isize` (`Runtime::memory_used`, `would_exceed`, `over_budget`,
  `rt_allocate`, `budget::Meter::used`, `Watch::grow`): identical on 64-bit targets.
- **`catch_unwind`** (`rt_native`, `rt_exec`'s panic guard) stays: under `panic = "abort"` a panic is a trap, and
  the page gets its text from a panic hook (#1125).
- **Recursion over value depth (audit, design §7.2).** Unbounded by value depth: `Runtime::is_key`/`key` and
  `Key::cmp_other`/`data_bytes`/its drop (`ops.rs`, nested value-object map keys). Bounded: the regex parser
  (nesting ≤ 256), `data_dump` (depth 64), test-report `show` (`SHOWN_DEPTH`); iterative: `equal`, JSON, marking.
  v1 keeps the trap (design); the keys' rewrite is a ticket.
- **Tests without the JIT.** Tests that need an async body or a failure handler carry
  `cfg_attr(not(feature = "jit"), ignore = "#1122 …"/"#1121, #1122 …")`; the JIT's own tests (`jit/`,
  `ready_callees`, compile limits) are `cfg(feature = "jit")`; `interp_tests::engines` compares the interpreter
  with itself under GC stress when there is no JIT.
## The polling of a runtime is fixed at its first run (#1129, 2026-11-27, lead decision)

- **Read at the same moment, not recompile.** Whether code polls the fuel cell is fixed when a runtime first runs
  (`Budget::polled`, set in `start_budget`): the JIT compiles it into every function, the interpreter and the
  sorting intrinsics read `Runtime::polled`. A later run whose budgets would turn polling on or off is refused
  with a message; the values of the budgets may change. Recompiling was rejected: compiled code embeds published
  addresses (ready callees, dispatch tables), and dropping every function between a session's cells would lose
  what the session compiled for an embedding case no command line reaches. Before the first run `polled()`
  answers from the budgets as they are (a session's destroys at close, direct compiles in tests).
- **`toolchain.sh` fetches first.** A networked `cargo fetch --locked` on the registry volume runs before the
  offline build; with every crate present it passes without network. A failure names its step.

## Sparrow's interpreter: async bodies (#1122, 2026-11-27; design Fable §2.4, §6.4; implementer's choices)

- **An async activation is the base of its entry.** `interp::resume` pushes an activation whose registers are the
  frame's `REGISTERS` (no register-stack chunk); a suspension writes `RESUME = pc + 1`, leaves and answers
  pending; a resumption starts at `RESUME - 1`. Await-like instructions occur only in the async function itself, so
  no synchronous activation is ever above a suspension. Each resumption is a step (the JIT polls in every entry of
  the body).
- **Pointer-free off by mode.** `Loaded::without_pointer_free` runs when the runtime is assembled for the
  interpreter, and `Runtime::use_engine(Interpreter)` re-lays the heap's descriptors before anything ran.
- **Re-entry counter.** `Interp::depth` counts entries from Rust (`invoke`, `resume`); past `REENTRIES` (200 on
  wasm32, unbounded natively, where `Ctx.stack_limit` decides) the entry is a stack overflow.
- **§6.4 fixed in the JIT.** `rt_trace`'s rule is one record per activation; the JIT's `check` now passes an async
  body's heap frame (stable across resumptions) instead of its frame pointer; synchronous and inlined code keep the
  frame pointer (#926). Ticket filed for the record.
- **The interpreter's branches block.** With async running and every difference explained, the Wave's
  `sparrow-interp` matrix branch loses `continue-on-error` and the gate's interpreter step fails the gate.

## The playground's runtime (#1125, 2026-11-27; design Fable §5, §8 item 6; implementer's choices; owner's §9 answers taken as recommended, awaiting the owner)

- **Owner's decisions provisional.** §9 #4 (profile text "Native X is not available in the playground"), #6 (hashes
  and HMAC in; ECC, AEAD, RSA, X.509 out — ECC's RustCrypto code is still linked through okkama-native, the profile
  only names what has no provider), #7 (two modules, 2.0 MB gzip), #8 (workers refused:
  `Limits::permits_workers`, "Process workers are not available in the playground"), #9 (real clocks through
  `performance.now`/`Date.now`, real randomness through getrandom's `wasm_js`), #10 (5·10⁷ steps, 256 MiB
  `MemoryScope::Runtime`, 1 MiB output) are implemented as recommended and await the owner.
- **No `okkama run --profile playground` (#5).** The parity test calls the two crates' Rust functions (rlib), so the
  CLI surface was not needed; not built.
- **The profile is data beside `PURE`.** `Profile {name, groups, allowed, refused, budgets}`, checked first in
  `Limits::permits`; `allowed` names natives of refused groups the playground offers: `stdin_*` (the buffer) and
  natives that compute although their module sits in a group (`input_state`, net `ip_*`, postgres `pg_page*`,
  `timezone_local_*`, `system_platform/architecture/byte_order`). The consistency test (build without default
  features) requires offers ⇔ `natives::supported`. Reclassifying those natives in `Reach::classify` was left
  out: the companion catalogue of the compiler (#569 C7) agrees with the groups by test, so it is a compiler
  change; the profile does not depend on it.
- **CLDR out of the module.** Measured 2.80 MB gzip with the tables in; the five CLDR files are 0.94 MB of gzip
  that does not compress further. okkama-runtime-core feature `cldr` (in `native`) embeds them; without it
  `native::provide_cldr(file, gzip)` installs a family (checked: gzip, UTF-8, three-part records), and a table of a
  missing family fails `core.cldr`/`unavailable`. `requirements` lists all five files for any program declaring a
  CLDR native: which family it reads shows only when it runs. tzdb, Unicode, IDNA and PSL stay in.
- **Paths stay under `host`.** `host/path_syntax.rs` depends on `path_regex.rs` (fancy-regex) and serves the fs,
  glob, temporary and watch providers: not cheap to move, and the playground refuses fs anyway.
- **Output as pieces.** `stdout::with_sink` hands every write of both streams to a closure on the VM thread (after
  `capture`, before buffering); `run_embedded` marks each piece with nanoseconds since the run's start on
  `clock::Instant` and joins consecutive writes of one stream at the same moment. sparrow-wasm turns pieces into
  text holding back a character split between writes of one stream. Since #1134 the writes can stream instead (below).
- **Uncaught across modules.** sparrow-wasm describes the failure as JSON (`category`, `errorType`, `message`,
  stacks as runs `[function, pc, count]`, offsets, causes); okkama-wasm reads it back (`Uncaught::category_named`)
  and renders with the module of its last `build`. `main_outcome` is the CLI's 0..255 rule and report newline.
- **stdin.** `Heap::set_stdin` puts a `Cursor` in `Stdin`; `Stdin::read` reads it instead of the process's; after
  it, end of input. The playground always passes a buffer (empty without the field).
- **Streamed output (#1134).** `run(spb, request, sink?)`: with a sink (a JS function) every write is handed to it
  while the program runs, as one chunk's JSON text, and the answer's `output` is `[]`; without one the output comes
  back in the answer as before. One chunk per write, not batched: a run is one synchronous call, so neither the
  module nor the worker's JS can flush on a timer, and a chunk held back would be lost when the page stops the run
  (`worker.terminate()`); the output budget (1 MiB) bounds the number. sparrow-core: `Embedding::pieces`
  (`PieceSink`, called on the VM thread) instead of collecting. On wasm32 the sink sits in a thread-local for the
  call (`JsValue` is not `Send`); a sink that throws loses that chunk (`catch`), the run goes on. Natively
  `sparrow_wasm::run_streaming(spb, request, emit)` takes a closure and returns the answer as a `Value`.
- **Batches at the budget poll (#1136).** A streamed run joins a stream's consecutive writes into one chunk while
  they fall within a window of the first (default 16 ms of the run's timeline, `window` in the request; 0 is a chunk
  per write, kept for a page that wants every write) and within 64 KiB; the chunk goes when the next write does not
  join it, at the run's next budget poll (`stdout::tick`, called from `rt_poll`; the playground always polls, it has
  a step budget) and at the end. So no tail waits while the program computes: a poll comes every 65 536 steps.
  Measured in wasm (Node 26, release build, best of three):

  | slice (steps) | window | 60 000 lines: chunks | run | max wait | print-then-compute: "started" arrives |
  |---|---|---|---|---|---|
  | any | per write | 120 000 | 158–163 ms | 1.7–2.3 ms | 0.1–0.2 ms |
  | 4 096 | 16 ms | 162 | 51 ms | 0.9 ms | 0.2 ms |
  | 16 384 | 16 ms | 41 | 48 ms | 1.7 ms | 0.4 ms |
  | 65 536 | 4 ms | 21 | 48 ms | 4.3 ms | 1.4 ms |
  | 65 536 | 16 / 50 ms | 11 | 50 ms | 5.4–6.0 ms | 1.5 ms |

  The interpreter runs about 50 million steps a second there (the design guessed 4–10 million), so the default slice
  already bounds the wait to a few milliseconds: `SLICE_PLAYGROUND = SLICE` (65 536), now a field of the profile and
  of `Limits` (`Limits::slice`) so it can be tuned; 16 ms windows (one frame). A run that writes per write costs three
  times the time in messages alone. On Stop the page loses at most what was written since the last poll. The poll's
  tick is a thread-local look in `rt_poll`, which runs only under budgets (micro-benchmarks have none: no A/B).
- **A user's stop has its own text (#1136, wording awaiting the owner).** `version().profile.stopped` =
  `"Execution stopped\n"` for the Stop button; `interrupted` stays the watchdog's, as `--timeout`'s in the CLI.
- **The page's texts come from the runtime (#1134).** `version().profile.interrupted` is the line `okkama run`
  prints when interrupted (`Exhausted::Interrupted`), written by the page on Stop and on its watchdog;
  `profile.exhausted.{steps,memory,output}` are the budget lines (a run's own report carries them too).
- **Reachable natives (#1135).** `requirements` names the refused natives a run can reach (`sparrow_core::reach`)
  from the entry, or the tests of a test build; `declared` keeps the old list. The walk over-approximates: direct
  calls and closures, constants' inits, cap fields called, intrinsic helpers and objects, built objects (`obj.new`,
  `variant.new`, builtins) with their destroy, and a built object's implementation of a role method once that
  method can be called (a reached `call.role` of that role and slot, or `reach::RUNTIME_CALLS`: `Error.text` for an
  uncaught report, `size`/`item`/`order` from an intrinsic). A test counts the runtime's role-dispatch sites so a
  new one cannot slip past the list. CLDR `files` follow the reachable natives too. Measured: the Bible's refused
  examples 8→4, 16→12, 23→22 refused natives (others equal), the smoke test's file program 8→4; over examples/
  (315 programs) every run that stopped at a refusal (47) stopped at a named native.

- **Module tests in the playground (#1127).** `test(spb, request, sink?)` runs an SPB built with `tests: true` as
  `okkama run -t` does (`sparrow_core::run_tests_embedded`: the same `test_profile`, each test on its own runtime,
  here on the interpreter with `Limits::playground()` and the request's standard input given afresh to each test).
  The human report goes through the same receiver as the tests' output, through a `LineWriter` (the report is
  written in fragments; one chunk per line, not per fragment). Exit as the CLI's: 0 all passed, 1 a test failed, 4
  with `report` `okkama: …` when the tests cannot run. Not done in the playground: `okkama run -t`'s
  documentation examples (`ok - doctest …`, each a program of its own), name filters (the CLI filters the module
  before codegen), workers (`-j`). Parity: the Bible's 13 `-tests` examples and 25 std modules' tests (json, csv,
  diff, toml, plist, strings, crypto/hmac, crypto/kdf, digests, uuid, checksums …) byte for byte, doctest lines
  left out of the CLI's side.
- **Hashes, HMAC, HKDF, UUIDs, checksums (#1127, owner's §9 #6 still provisional).** Already in the profile since
  #1125 (`crypto`, `host_crypto`, `uuid`, `checksum` are pure, RustCrypto in wasm); now checked: a test that every
  native of those modules is offered, their std modules' tests in the parity test, a digests program
  (`okkama-cli/tests/playground/digests/main.ok`) compared with the CLI and run in wasm by the Node smoke test with
  the known values (SHA-256 "abc", HMAC-SHA256, HKDF, UUID 5 of example.com, CRC-32, Adler-32).
- **CLDR on demand** (design §8 item 8, first point) was done in #1125 (`provide`, `files`).
- **Left for the owner:** workers' emulation (§9 #8) and the resumable scheduler (§9 #3).

- **Workers emulated in the instance (#1127, owner's decision 8: emulation; decision 3: time stays
  fast-forward).** Without the host (`worker_emulated.rs`, and with it when `Runtime::emulated_workers`: the
  playground's embedding natively, `run_embedded`/`run_tests_embedded`), `worker.spawn` runs the worker at the spawn
  on a runtime of its own: a clone of the parent's `Loaded` (no second load), the parent's engine and budgets
  (each worker its own steps, memory and output, the same interrupt flag), empty standard input. Receiver fields
  and arguments cross by the process codec, moved to `worker_wire.rs` (so `spawn cannot transfer …` and every
  decoding rule are the same), the result back the same way; a failure comes back as the worker's report text;
  its stdout and stderr are captured and handed over when the parent takes the result, before it. The Future's
  driver waits one scheduler round at the spawn (as a process worker's reply comes after the parent goes on), so
  the parent's own writes after a spawn come first; a cancelled worker reports nothing. Nondeterminism of real
  workers resolved as one valid order: workers finish at their spawn, their outputs reach the parent in the order
  their drivers run (spawn order). Pools reuse nothing (every spawn is a fresh runtime). `system_cpu_count` is 1
  without the host (one processor runs the emulated workers in turn), offered by the profile, so `ParallelMapped`
  with its default bound runs. `permits_workers` no longer refuses under the profile; its text went. Differences
  from the CLI, recorded: a CLI worker runs without budgets (the playground's does, so a spinning worker ends
  with the budget's text as the parent's failure), a worker's stderr is written through `write_error` (ordered
  with stdout; the CLI writes the child's stderr directly). No Bible example uses workers (chapter 20 says its
  examples do not); the parity uses `okkama-cli/tests/playground/workers/` (two workers, output, failure, bound
  action, parallel map, a compile-time transfer refusal), all equal to the CLI's process workers.

### JSON shapes of the playground's runtime (sparrow-wasm; #1125, #1134, #1135, #1127)

- `run(spb, request, sink?)` → request `{stdin?: text, arguments?: [text], window?: ms}` (`window`, #1136: a
  stream's writes within it of the first are one chunk; 0 a chunk per write; default 16); answer
  `{exit: 0..255, output: [chunk], report: text | null, uncaught: Uncaught | null}`, or `{error}` for a request
  that is not JSON. A program that does not load: `exit` 1, `report` `"okkama: …\n"`. `report` is null when there
  is nothing to print (exit 0..255 from main), else the stderr text `okkama run` prints before the stack.
- chunk `{stream: "stdout" | "stderr", text, t}`: `t` nanoseconds since the run started on the run's timeline
  (wasm32: virtual, sleeps skipped). Without a sink, writes of one stream at the same `t` are joined.
- Uncaught `{category: "uncaught-error" | "runtime" | "panic" | "exhausted", errorType: text | null, message,
  trace: [[function, pc, count]], traceOmitted, traceOffset, causeTraces: [[offset, [[function, pc, count]],
  omitted]]}`: stacks as runs of equal consecutive sites. okkama-wasm `failureText({report, uncaught, cwd?})` →
  `{ok: true, text}`.
- `test(spb, request, sink?)` → request `{stdin?: text, seed?: 0..2^53-1, window?: ms}`; answer `{exit: 0 | 1 | 4, output:
  [chunk], report: text | null}` (`report` only with exit 4), or `{error}` for a request that is not JSON or a bad
  seed. With a sink, `output` is `[]`.
- `requirements(spb)` → `{refused: [symbol], declared: [symbol], files: [name]}` (symbols sorted), or `{error}`.
- `provide(name, gzipBytes)` → `{ok: true}` or `{error}`.
- `version()` → `{version, profile: {name, groups: [name], steps, memory, output, interrupted, stopped, exhausted:
  {steps, memory, output}}}`.
- `lastPanic()` → `{panic: {message, location} | null}`.

## Map keys walk their depth on their own stack (#1130, 2026-11-27, from the #1124 audit)

- **What is iterative.** `Runtime::is_key`/`key` (their object arms, `is_object_key`/`object_key`, `#[cold]`),
  `Key::cmp_other`'s object arm (`cmp_fields`: the open levels as slice pairs), `owned_bytes` and `data_bytes`
  (`owned_object_bytes` bottom-up, since each level divides by its own owners; `object_data_bytes` a plain sum)
  and the drop of nested keys. The scalar and string arms, `Key::cmp`'s inline fast path and every map entry
  layout are unchanged.
- **The drop.** `Key::Object` holds `Fields(Rc<[Key]>)` (same size as before) whose `Drop` returns at once when
  no field is an object key; otherwise the last owner moves each nested level's `Rc` out of its parent (leaving an
  empty slice) onto a list before the parent is freed, so no drop goes more than one level down.
- **Same results** are a test, not an argument: 300 keys of every kind, objects three deep and partly shared,
  compared pairwise and sized against the recursive definitions kept in the test (`ops/key_depth_tests.rs`); a
  100 000-deep key is checked, built, compared, sized and dropped on a 1 MiB thread, in both builds.

## Sparrow on 32-bit targets (#1131, 2026-11-27)

- **How it runs.** The suite of sparrow-core without default features is built for `i686-unknown-linux-musl`
  (static, linked by rust-lld with the target's self-contained musl, no C toolchain) and run on a 32-bit Linux:
  locally a `linux/386` alpine container (Docker's emulation on the arm64 Mac), on the Wave's x86_64 runner
  directly (`wasm` part). Locally it ran by module filters and one integration file at a time.
- **`STACK_BYTES` is 1 GiB on 64-bit targets and 128 MiB on 32-bit ones**: a 1 GiB reservation per VM thread left
  no room for a second run's thread in a 32-bit address space ("VM thread: Resource temporarily unavailable"). The
  interpreter's register stack takes the same bound (so 128 MiB on wasm32).
- **Tests.** `length_tests`' past-4 GiB case exists only on 64-bit targets (a 32-bit `usize` has no such length).
- **A session on the interpreter** (`extend`) drops the pointer-free proofs of the extension's functions as
  `assemble`/`use_engine` drop its own (#1122); the published-metadata check refused every extension before. Found by
  this run; it fails natively without default features too.

## Pure natives are pure in both classifications (#1133, 2026-11-27)

- `input_state`, `net` (`ip_*` only) and `postgres` (`pg_page*` only) are pure modules in `budget::PURE` and the
  companion catalogue's `PURE`; `timezone_local_*` falls to `time_zone`'s pure; `system_platform`,
  `system_architecture`, `system_byte_order` are pure by name (`budget::SYSTEM_CONSTANTS`, the catalogue's
  `effect`), the rest of `runtime/system` stays Env (host name, CPUs, storage) or Random (`secure_*`). Effect counts
  of the catalogue: pure 275, stdio 3, host 264. The playground's `allowed` keeps only `stdin_*`.

## One rule for the inspection an editor's answer uses (#1128, 2026-11-27)

- **`Inspection::analysed`**: parsed, std paths desugared, the import graph loaded and analysis ran.
  **`ide::Answering`** keeps the last analysed inspection and its text; the language server records every
  inspection (each change) and okkama-wasm every IDE call's, and both answer hover, completion, definition and
  signature from it. A std path or import that does not resolve no longer costs the language server every symbol:
  the earlier answers stay, the new diagnostics are published. The outline is syntax: the current text's when it
  parses, else the answering one's.
- **Left as it is:** the playground inspects only at IDE calls, so a text that was analysed between two calls and
  never sent is not seen; with the rule above that changes an answer only when such a text differs in its symbols
  from both neighbours.
- `docs/OkkamaBible/build.py --check` no longer rewrites `examples/` out of the output: since #1119 the CLI names
  imported files relative to the current directory (249 examples, unchanged).

## Float fast paths without safepoints (#1105, 2026-11-27; owner approved lifting #537's caution for these conversions; audit Fable)

- The inline int→float and float→int conversions call no safepoint. A conversion adds nothing to the collection
  counter (`Heap::needs_collection`), so its safepoint only collected for its neighbours' work, and the same code
  without a conversion never had one (Fable's review: not every allocation path is a safepoint, e.g.
  `charge_external`, map-node growth and `[Allocates]`-only helpers, so "the allocation slow path is a safepoint"
  is not the argument). #537's "do not remove required safepoints" stays for anything that allocates or polls.
- `check`'s failure blocks are cold everywhere since #1107 stage 2 (first only inside slow paths, `Emit::cold`,
  removed then): cold is not inherited, and they sat in the hot layout between the fast path and its continuation. After the range proof float→int uses `fcvt_to_sint_sat` (a bare `fcvtzs`).
- Float `+ − × ÷` on finite operands: a non-finite result always fails (`scalar::float_binary`), so the JIT's slow
  path never returns. After its `check` the continuation is `trap(user 1)` (`FLOAT_SLOW_PATH_RETURNED`): reaching it
  is a broken invariant, not a program failure. Finiteness is `fcmp ord (x − x), (x − x)` (`Ordered`: one condition on x86-64).

## Scalar operations inline, shared path as the oracle (#1166 / #1106 stage 1, 2026-11-27; lead)

- Byte `& | ^ ~` are always inline (a byte is a raw u8 word). Byte shifts inline when the int count is
  tagged-small in 0..7; any other count goes to a cold slow path that always fails (`fail_generic`, trap 2 after it).
- `int.ushr`, `int.abs`, int `bits` rotations, reversal and swap are inline for tagged-small operands whose result is
  tagged-small again (`tag_if_small`); otherwise the shared path, so BigInt results and failure texts stay exact.
  `sign` of an int or float and `sqrt` of x ≥ 0 are inline; rat `sign` stays generic.
- Tests compare the inline build with `jit.plain` (every such instruction through rt_exec) on edge values, and
  assert the fast inputs never reach rt_exec (`exec <mnemonic>` stats).

## Shared empty and fieldless values (#1168 / #1106 stage 3, 2026-11-27; owner approved sharing; rules from Fable's review)

- One instance per runtime of: the empty array of each representation (and the packed byte array), the empty map
  of each (key Repr, value Repr), and the fieldless value of each shareable object type. The heap keeps them as
  roots (`Canonical`), made on first use; the JIT bakes their addresses as constants.
- Substitute only at the construction instructions (`obj.new`/`variant.new` with no fields, `arr.new []`,
  `map.new []`); never a map builder (`mbuilder.put` changes it) and never inside `allocate_payload`.
- Shareable type = no fields, no destroy, not builtin, not `held` (Atomic/Event cells are written in place and
  keyed by address), no traced layout slot, and not throwable: a rethrow is recognised by the thrown value's
  address (`failure_link`), so `throw Oops{}` inside a handler must be a fresh value.
- A session extension that turns a published descriptor unshareable is refused (`held` is recomputed on extend).
- `map.len` stays a checked helper (#537); the remaining cost of map-empty is that call.

## Cold blocks: rare paths only (#1107 stage 2, 2026-11-27; lead, from the audit's item B)

- Cold: `check`'s failure blocks, the allocation's free-list refill, compile on first call, array index outside,
  the BigInt paths of int arithmetic, comparison, bit operations and not/neg, the atomic intrinsics' closed path,
  and the failing slow paths of #1105/#1106. int-big did not move (+0.1%), so BigInt paths may stay cold.
- Not cold, because real data takes them often: equal-length string comparison, `StrLen` of non-ASCII text,
  structural `val.cmp` of objects.

## Explicit roots for synchronous functions: rejected (#1107 stage 1, 2026-11-27; lead, by the owner's rule on #1107)

- Fable's reconstruction: the audit's proxy numbers were `SPARROW_X_NORELOAD` + `SPARROW_X_COLD`. The cold half is in
  (stage 2: recursion −11.9%, method-call −8.1%).
- Prototype (Fable's scheme, first step): sync functions on `Roots` with words spilled before every call and
  reloaded after it, nothing stored at definitions, inlining off (`SPARROW_CORE_SYNC_ROOTS`, A/B in one binary,
  two layout seeds): baseline 2.13 → 2.00 ns (−6%), compare +12%, closure-call +17%, constructor +19%, role-call
  +50%, method-call +54%, recursion +65%; geomean +16% over 12 cells.
- The remaining gain on scalar loops (−6%) does not pay for the losses on calls, and the full scheme (store
  skipping, inlining with per-callee word maps, a live-across set per call) is a large change. By the owner's rule
  ("if explicit roots create excessive complexity or regressions, retain the existing stack-map mechanism"),
  synchronous functions keep Cranelift's stack maps. The patch is kept in
  okkama-docs/runtime/perf-audit-2026-11-27/1107-sync-roots-prototype.patch.

## Shared failure exits; stack maps spliced, not sorted (#1108, 2026-11-27; lead, owner approved the direction)

- `check` keeps its hot path (status load and branch); a failure goes through a two-instruction cold trampoline that
  passes the pc to one exit per (function, inlined activation, handler). The exit reloads the status, records the
  activation at that pc (`trace`), then goes to the handler or out. Handlers are chosen at compile time by pc, so
  pcs share an exit only when their handler (or its absence) is the same.
- Async bodies with explicit roots keep an exit per call: their stack maps report different words at each call.
- A function's code range overlaps no other's, so its maps are one sorted batch spliced in at their place; code
  addresses need not grow from function to function (`register_code`, tested with addresses in random order).

## An internal compiler error has one kind in check and build (#1143, 2026-11-27; lead — the ticket left it to the owner, no owner comment)

- `okkama check` and `okkama build` both say `internal compiler error: <text>` for a lowering invariant failure
  (`SemanticError::internal`, `Finding::semantic`): it is a compiler bug, not a semantic error of the program.
  Other kinds still differ between the two commands ("analysis limitation" vs "lowering error"); left as they are.

## Oracle tooling (#1144, 2026-11-27; an Opus agent's choices, accepted by the lead)

- The fuzz differential target embeds `okkama-rs/tools/engines/interp-exceptions.txt` (`include_str!`) and applies
  its `jit:`, `both:` and `either:` selectors as `differ.py` does; `path:` never applies to a fuzz input.
- The dead refusal text is gone everywhere; a run without the JIT is explained only by `path:` lines, and a
  `refusal:` line is rejected as an unknown selector.
- `differ.py run` exits 1 when a program differs and when nothing built; some programs unbuilt still pass.
- The interpreter test harness turns on emulated workers, so workers.spa's spawns run in-process.

## Playground positions (#1139, 2026-11-27; agent, accepted)

- Hover and definition take zero-based line/character in UTF-16 units against the text of the inspection that
  answers (the last analysed), as `okkama lsp` does; completion and signature help against the current source.

## Block parameters in the linearity check (#1138, 2026-11-27; agent, accepted by the lead)

- A branch moves every argument's ownership state into its parameter reading the state before the branch (all
  sources cleared, then all destinations set, so swaps keep their resources). A destination that may hold a FREE
  resource the branch does not pass on is refused: "%P takes a new value at the branch from block X to block Y
  while the one it holds may be in no region". One shape stays refused as on release/065 (a dominating value
  passed to the same parameter twice); lowering does not produce it.

## Worker failures (#1141, 2026-11-27; agent, accepted)

- A worker's run error crosses without its "RuntimeError: " prefix (`worker_wire::failure_text`), so the parent's
  report has it once; other report names (a panic's "Panic: …") are kept.

## Closure entries (#1110, 2026-11-27; design Fable, owner approved the direction)

- A closure's first word after the header is its entry (0: the general path); captures follow from offset 16. The
  layout declares the entry Raw, so tracing and the ownership walks skip it.
- Entry ABI `(ctx, a1..aA, closure)`: code of a function taking the arguments alone ignores the trailing closure
  (C calling conventions, caller cleanup), so its ordinary code is its entry; for c ≥ 1 captures a second
  compilation of the body reads them from the closure in its prologue. Its frames are the function's (traces,
  stack maps registered as any code).
- The entries a compiled function's `fn.closure` names are compiled right after it (`compile` drains
  `closure_requests`); the closure reads its slot when it is made, so no helper patches closures. Closures made
  by the interpreter, `rt_closure` or `closure_copy` keep 0. Not for async functions, natives or bodies over 400
  instructions.

## First-run compile (#1147, 2026-11-28; design Fable)

- Mixed mode (interpret first, JIT later) rejected: most compile time is in once-called functions with long loops,
  which need OSR. Cheap Cranelift settings rejected (single_pass, opt_level none: steady losses).
- Step 1 hygiene kept where measured: no-std frontend, exits by handler target, no status check after int
  add/sub/mul without budget/polling. Not kept: dropping the check after rt_compile (regalloc loss on a hot closure),
  direct branch edges (07-objects +32%: block layout), compile-context reuse (no gain). Lesson: measure the whole
  ladder, not only the programs a change aims at.
- Step 2 (done): background codegen with demand-driven speculation of static callees: emission stays on the VM
  thread, workers only run Context::compile, installation (definition order) only at a demand; a unit is stale when
  a lazy callee was published after its emission (then recompiled synchronously, keeping the synchronous code).
  Default on with 1 worker (2 measured no faster, +3 MB more peak footprint each); synchronous under stats, profile,
  dump, memory budget, polling, sessions.

## Safepoint fast path (#1175, 2026-11-28; owner put it in 0.6.7)

- The not-due check is inline at every allocating helper; the collection is a cold out-of-line function.
- The heap keeps a pointer to its thread's map-node counter cell (`pmap::memory_cell`): the heap never leaves its
  thread (the pointer makes it !Send; nothing needed it to be Send), and the cell has no destructor.

## One instruction for a chain of string `+` (#1177, 2026-11-28; owner: in the compiler, not the JIT)

- The ticket's JIT fusion of consecutive `str.concat` does nothing: the parts are computed between the concats
  (const, int→text, calls), 1 consecutive pair in ~20 per program. Fusing across them needs the parts kept as GC
  roots, which the SSA values of a lowering give for free: `a + b + c …` lowers to its parts, then one
  `str.concat_n` (two parts stay `str.concat`). Right-nested chains flatten too (associative; order kept).
- `concat_parts`: any part ≥ `TEXT_CHAIN_MIN` or in a text-buffer chain → the pairwise `concat_text` sequence
  (amortized appends unchanged); otherwise one flat allocation, APPENDED when all but the last part reach 256 B.
- Past 4 GiB the failure (pairwise text) comes after every part is evaluated, not between them: accepted.
- Opcode 164 shares SPB 19 with `builder.sized`: 19 is unreleased, so no second bump.

## Builders in the heap (#1171, 2026-11-28; owner: back into 0.6.7; design Fable on #1111)

- A builder is `heap::BUILDER` `[header|repr][length][capacity][buffer]`; the buffer is an ARRAY cell whose header
  says length 0 (a leaf: marked, not traced); the builder traces slots below its length. Builders stay linear in
  the IR, but the cell keeps one identity anyway: a push never changes the builder word.
- Not a BUFFER array (#246) as the builder itself: the header has no room for an exact capacity (6 bits, powers of
  two), and an exact capacity is what lets `builder.sized` finish in place.
- `finish` in place when `class_of(length + 1) == class_of(capacity + 1)`, or for large cells when at most an eighth
  is left; otherwise copy (memory of retained arrays stays exact). Spare buffers (≤ 16, ≤ 512 elements) live until
  the next collection, which clears the list before marking: they are no roots.
- `builder.sized` is a capacity hint, not a contract: a non-small or negative count is no hint, at most 4M
  elements up front. Only `map` uses it; `filter` sized by its source cost 10-app +5% (filters that keep little).
- Walkers treat BUILDER as they treated the Builder payload (may reach; no children; never summarized).

## 0.6.7 scope after the mid-release showcase (2026-11-28; owner)

- Showcase mid-0.6.7 against v0.6.6: cold −23%, steady −32% (okkama-docs/benchmarks/reports/2026-11-28-v0.6.7-mid).
- Background codegen keeps one worker by default; its ~3 MB of peak footprint is accepted (owner: a little memory
  for this speed-up is fine, the JVM takes far more).
- Back into 0.6.7: #1171 (builders as a heap kind; the rest of #1150 and part of #1153), #1151 (own root slots in
  the JIT instead of the frontend's spills), #1152 (steady ≤ 4 ms through a walk of the ownership graph on a move
  into a region; after #1151), #1176 and #1177 (from Hotfixes TBD). Designs by Fable, full-ladder A/B per step.
- 03-float (steady 2.7× Go and HotSpot) goes to #1178 in POSTPONED; not started.

## Vendored num-bigint (#1148, 2026-11-28; owner: vendor + upstream PR)

- num-bigint 0.4.8 lives in okkama-rs/vendor/num-bigint with a Rust-native change (`|` carries, 4-digit add
  step), not inline assembly, so it can go upstream as is; not a workspace member. Drop it when upstream has it.

## Sweep by page (#1114, 2026-11-28; owner: measured prototype; design Fable)

- Payloads and big integers live on owner pages; general pages never hold Rust state. Owner pages are swept
  eagerly (cleanup timing unchanged); only general pages may be taken back unread.
- Page liveness is a byte per 4 KiB granule over the pages' span, set by `mark_and_queue` (the only writer of
  MARK). Pages are not aligned: a granule shared with a neighbour or a large object only makes a page look live.
- Lazy sweep rejected (unmarking and counting live is the walk; owner pages must stay eager); a per-cell side
  bitmap deferred with the marking follow-up.
- Owner (2026-11-28): a little more memory for this speed is acceptable, unless something is plainly wrong.
- Bump lanes (step 3) rejected with numbers: allocating from a bump region before the free cells of partly live
  pages cost cycles (10-app +5.6%, 05-strings up to +6%) for +0.8% instructions; the free-list pop stays.

## Keys from outside the process take a keyed hash (#1103, 2026-11-28; owner: research first)

- Text or bytes that come from outside (DB column names, JSON members, headers) key std's RandomState
  (SipHash-1-3); FastHasher (unkeyed multiplicative) is for engine-assigned integers only and asserts so in debug.
- foldhash was 2–5× faster but claims no HashDoS resistance; ahash is not in the lock file; a seeded FastHasher
  keeps seed-independent collisions (zero padding, a bit-63/bit-4 differential). Security wins: the cost is
  nanoseconds once per result set.

## Small-int function versions (#1113, 2026-11-28; owner: a guarded prototype; #1107's spill prerequisite rejected)

- Only functions without effects get one: the instructions are int add/sub/mul/mod/cmp, bool not/cmp, moves,
  constants that fit small, jump/branch/return; registers are ints, raw words and references only passed on.
- The fallback restarts the whole call in the general version with the entry's original (tagged) arguments,
  instead of a transfer at the same pc: no state to reconstruct, and nothing observable repeats.
- The general version is compiled at the first restart (a slot per function), so a function that stays small
  costs one compilation, smaller than before.
- Inside, ints are untagged; add/sub check the range by `(v << 1) >> 1 == v`, mul also the i64 overflow, mod
  restarts on divisors 0 and −1. Tagged small ints (the first variant) gave baseline −46%, untagged −60%.
- Off while the runtime polls (budgets): a restart would count the entry and loop steps again.

## OIR inlining and scalar replacement (#1112, 2026-11-27; owner revisited "inlining lives in the JIT"; design Fable)

- `inline` takes direct callees that cannot fail: no loop in their blocks, no unwind, terminators br/cond/ret,
  instructions from a fixed set (const, int add/sub/mul/neg, comparisons, bool not, obj.new/obj.next of plain
  objects, obj.get, succ.take), at most 24 instructions in one block or 64 over several; callers stop past 400.
  Failure traces stay identical (only FAILED records an activation). The JIT inliner stays.
- Owner, 2026-11-27: `--max-steps` may count differently (an inlined callee's entry step is gone); no step opcode.
- The verifier grants an inlined callee's rights (`obj.new` of a constructor, `obj.next` of a successor, `obj.get`
  of its object) through `Attr::Inlined` on the caller; source encapsulation was checked by the analyser.
- `scalars` replaces plain objects whose uses are field reads, receivers of candidate successors and branch
  arguments of candidate block parameters; a parameter an edge fills itself, or of an unwind target, is never
  split. No in-place reuse (a missed alias would corrupt silently; Fable: no gain after scalar replacement).

