# Decisions and rulings (do not relitigate; extend when a decision is made)

The newest entries are at the top of each section. Each one gives its date, who decided, and where the details
are. Working rules (testing, CI, roles) are in LLM-GUIDE §1 and are not repeated here. Seeded 2026-11-20 from
the session's memory; older rulings live in the commits and tickets named.

## Direction (owner)

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
