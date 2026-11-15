# #569 / #640 C2a — callable discovery and concrete types

Scope: producer preparation only, following the accepted C1 decisions. C2b owns the audited O1
contract delta. C3+ operations remain explicitly Unsupported. No callable summary, descriptor,
ownership decision, new IR form or verifier rule is emitted here.

## Types and discovery

`specialize::prepare` returns `Prepared { analysis, callables }`. Action parameters/results recursively
pass through the existing specializer, including nested actions and nominal/collection types inside
signatures. Cap owner arguments use the same concrete object instances. `Declarations` interns the
existing `Type::Fn(FnType { params, result, is_async: false })`; void is `result: None`. Async action
types remain Unsupported. Ordinary application signatures are still checked in full.

The FIFO specialization worklist includes root bodies, requested bound-action methods and cap computed
fields. Closures are reserved and queued before their bodies are visited. Their identity is
`BodyKey { root: concrete enclosing BodyOwner, path: Vec<usize> }`: each path element is a closure
ordinal within its immediately enclosing lexical body. Sibling closures, two generic instantiations
and recursively discovered closures cannot collide through `BodyOwner::Closure` or equal spans.

Every closure job carries its substitution, requesting chain, lexical receiver and capture layout.
The existing depth 256 / count 2^18 limits also cover closure jobs. Root identities preserve concrete
object/member indices, including `CapField { object, cap, field }` and `Test(index)`. The explicit
nested-body traversal does not change THIR `each_child`, which intentionally excludes closure bodies.
After the queue drains, specialized closure bodies are reattached from deepest paths upward; both the
sidecar and embedded THIR contain the concrete bodies. Later consumers need no second specializer.

## Layout

`DiscoveredBody` records the lexical receiver type separately from actual snapshots, preserving
receiver authority without capturing an unused receiver. Its ordered `captures: Vec<Capture>` contains:

1. `Snapshot::Receiver`, when `thir::uses_receiver` requires it; no THIR local target.
2. THIR captures in their original order, tagged `Snapshot::Parameter(outer)` or `Snapshot::Local(outer)`,
   each with its concrete type and `target: Some(inner)` mapping into the closure local table.

Explicit parameters remain in `body.params`, after the eventual capture prefix. A nested closure
snapshots the local/parameter of its immediate enclosing body; the outer layout retains that local's
own origin. Bound inventory entries record the concrete receiver type and exact method or role/slot
identity. Cap bodies record their concrete owner receiver. These are snapshot/layout facts for the
existing own protocol, not ownership, retention, destroy order or resource classification decisions.

## Open-call inventory for O1

`Inventory { test_mode, bodies, bound, open_calls }` belongs only to the producer. An `OpenCallable`
contains one full concrete structural `Ty::Action` and `sites: Vec<OpenCallSite>`. Each site contains
`{ body: BodyKey, module, span, ordinal }`. The ordinal identifies CallValue discovery within a lexical
body; it is not an execution schedule. Types and sites use deterministic first-discovery order;
repeated signatures share an entry. A separate inventory instance is built for each execution profile.

All THIR `CallValue` sites are inventoried, including sites inside nested bodies, repeated sites and
nested action signatures. This is a conservative registration input; it does not classify dispatch as
known/open, erase finite-target precision or authorize any call. Existing `TypeId`s can be resolved
through `Declarations.types` when C2b builds the trusted registry. No `CallContract`, summary, adapter,
runtime descriptor or trusted contract registration is created in C2a.

`Registry::build_mode` preserves the input mode and root count. Test identities/source order and
`Callee::Test` remain intact. Production parsing/pruning produces no test roots, closure discoveries
or open-call sites from test bodies. The lowering entry still rejects C3+ callable/test/cap constructs
with source Unsupported diagnostics after discovery.

## Evidence and status

- [x] Concrete generic action parameters/results, nested/nominal signatures, action generic arguments and void.
- [x] Separate closure jobs, concrete context, recursion deduplication, sibling keys and depth/count limit tests.
- [x] Local, parameter and receiver snapshot tests; ordered combined layout and unused receiver check.
- [x] Generic bound-action target and generic cap computed-field body discovery, including nested closure.
- [x] Production/test isolation, preserved test identity and Callee::Test.
- [x] Open-call structural deduplication, site identity and deterministic specialization.
- [x] Three producer inventory goldens: generic-callables, nested-callables, recursive-closures.
- [x] C2-owned manifest annotations updated without claiming full acceptance.
- [x] All four required command gates passed; results below.

The three C2-owned rows remain `planned`: `L01.gram.types.void-parameter-type`,
`L03.action-types.parameter-variance`, `L03.action-types.result-mismatch`. Their full frontend/source
acceptance is not inferred from successful producer preparation. All other rows retain their status.
The golden files are producer inventories, not High/Own/runtime acceptance artifacts.

## Command results

All commands ran from `okkama-rs/`, offline with four build jobs:

| Command | Result |
| --- | --- |
| `cargo check --workspace --all-targets --offline -j 4` | PASS |
| `cargo clippy --workspace --all-targets --offline -j 4 -- -D warnings` | PASS, no warnings |
| `cargo test -p okkama-compiler --lib oir2 --offline -j 4` | PASS: 156 passed, 0 failed, 118 filtered |
| `cargo test -p okkama-oir --offline -j 4` | PASS: 178 unit + 29 integration tests; 0 doctests |

The 14 added compiler tests include the three golden comparisons. Repeated preparation compares
ordered producer inventories; the golden comparisons also run without the update environment variable.
The initial test run exposed invalid source fixtures (generic constructor spelling, required bound-action
type boundary, production pruning and a cap closure reading a field unavailable to its frontend scope).
The fixtures now use the existing frontend contracts. The previous ordered-sets Unsupported diagnostic
is preserved. Strict clippy found a needless Box replacement allocation; reattachment now replaces the
existing contents. Final checks above ran after those corrections.

`git diff --check` and added-code line-length checks pass. No files in `okkama-oir` changed. No network,
Docker, CLI programs, other test suites, cargo fmt, CHANGELOG changes or commits were used. HEAD remains
`9f8066f3` on `feature/569-lower-callables`. C2b's O1 contract implementation and later IR lowering remain
separate audited steps; they are not claimed complete by C2a.
