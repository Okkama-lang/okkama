# The pipeline (source → OIR → SPB → Sparrow)

There is one engine, Sparrow; Tweed is its next version (#903). Phase contracts are in `docs/phases.md`. Paths
are relative to `okkama-rs/crates/`. `OKKAMA_DEV=lower` covers the compiler stages and `runtime`, `jit` and
`gc` cover execution (§9 of LLM-GUIDE).

| # | Stage | Entry point (file) | Decides | Checks | devtrace |
|---|---|---|---|---|---|
| 1 | parse | `syntax::parse` (okkama-compiler/src/syntax) | AST | syntax errors | — |
| 2 | imports, analysis | `lowering::modules::load_graph`, `analysis::analyze_mode` (okkama-compiler/src/analysis) | names, types, roles, caps, errors (THIR) | the language's static rules (semantic errors) | — |
| 3 | lowering | `lowering::lower_linked*` → `lower_modules` (okkama-compiler/src/lowering) | OIR-1: typed SSA with unwind targets and **cleanup regions** (`region.enter/adopt/track/transfer/leave/unwind/settle`), the ownership rules of the Bible | refuses what it cannot lower | lower (`ir lowered`) |
| 4 | IR passes | `lower_modules`: `statics`, `regions` (plainness, unused regions), `tail-calls`, `fold`, `hoist`, `dce`, `undef`, `jumps` (okkama-compiler/src/ir) | the same meaning with fewer operations | `ir::verify` after them, then the ownership linearity check in debug builds or with `OKKAMA_VERIFY_OWNERSHIP=1` (#910); `OKKAMA_IR_PASSES=none` turns them off | lower (`ir after passes`, pass timers) |
| 5 | Sparrow codegen | `codegen::sparrow::generate` (okkama-compiler/src/codegen/sparrow) | SPB 16: registers, instructions, handlers | register allocation check (debug, gate) | — (#912) |
| 6 | SPB load | `okkama_sparrow::decode`, `Program::verify`, `sparrow-core` contracts | a verified program | types, budgets, intrinsic contracts | — |
| 7 | JIT | `sparrow-core/src/jit.rs` (Cranelift, whole function on first call) | machine code, stack maps, helper calls | Cranelift verifier in debug | jit |
| 8 | run time | `sparrow-core`: `failures.rs` (identities, regions, resource walks, destroy), `tasks.rs` (async frames, `rt_region_settle`), `heap.rs` (GC), `drivers.rs`, `intrinsics.rs` | when each resource is destroyed (reverse serial order), cancellation and settling | at-most-once destroy, `already_closed`, budgets | runtime, gc; cleanup-count / cleanup-time (#329) |

A failure names its stage: `syntax error`, `semantic error`, a lowering error, `internal compiler error: lowering
produced invalid IR` (`OKKAMA_DUMP_INVALID_IR=FILE` writes it), an SPB verify error, or a run-time failure with its
trace.

Where one ownership rule lives in several rows, rules.md lists them. The aim is lowering plus the runtime, with
one elaboration pass (#909) and one debug-only checker (#910).

Documentation (#512/#513) is a separate read-only path: `doc::document` reuses
the module graph and declaration registry, then `doc::Documentation` resolves
public declaration links and warning spans. `doc::render` supplies man pages,
search results and IDE documentation. `ide::build` selects declarations using
analysis spans; std path completion uses the same public inventory and editor
overlays, resolving selected entries through LSP `completionItem/resolve`.
This path neither lowers bodies nor generates or executes code.


Documentation checks (#514) consume that same public model. `doc::examples`
classifies CommonMark blocks with source-byte maps; CLI `doctest.rs` compiles
selected examples using the ordinary resolver/lowering/SPB path before
`run -t` runs inline tests. Only explicit runnable metadata permits the
private child: verified bytecode, denied host capabilities, a cleared
environment, disposable directory, bounded capture and wall/RSS/allocation
limits. Unavailable isolation is an explicit compile-only result. Parent-side
instruction maps recover original comment locations for runtime failures.

`doc::coverage` counts public declarations, enum cases and computed cap fields
and owns the completed-wave list and resolver-based file identity policy.
IDE indexing reports `missing-docs` warnings (errors for completed std files);
CLI root check/IR/build/run gates apply the same coverage and identity rule.
Extraction, man and IDE documentation still never generate or execute code.

Channel selection (#509) follows the existing intrinsic path:
`lowering/intrinsics.rs` and `ir::INTRINSICS` register the selector constructor
and async `item`; `sparrow-core/contracts.rs` validates endpoint-array and
selected-result layouts and payload type agreement. `drivers.rs` performs one
atomic receive, queue registration and cleanup; `primitives.rs` updates parked
selectors on close. Channel carrier analysis in `ir/mod.rs` and `failures.rs`
includes endpoints used only through selection. Direct timeout composition
publishes an already committed selection before attempting cancellation and
preserves its winner while awaiting the cancelled timer's cleanup (#940).

Publishing (#515) remains on that read-only path. `doc::document_graph` exposes
all public models in one ordinary import graph for batch export.
`doc::publish` renders man(7)/3ok and standalone HTML with Phase B targets,
and serializes explicit JSON schema v1 (`doc/schema-v1.json`). CLI `man --all`
writes a sorted std catalogue. `tools/install-man.sh` stages generated man
pages for optional installation/packages, with a filename ownership manifest;
`make reference` builds the portable HTML catalogue for okkama-docs.


### Parallel map worker transport (#946)

`std/parallel/map.ok` keeps ordered batches and FutureScope; hidden
`ParallelMapWorkers.bound/close` opt a copied action into one run-local pool.
`okkama-compiler/src/lowering/intrinsics.rs` and `ir/mod.rs` register those
private intrinsics. Worker binding/entry lowering transfers declared receiver
fields, excluding synthetic once slots. `sparrow-core/contracts.rs` checks the
helper's receiver/action types; `worker.rs` retains the spawn codec and driver.
`worker_process.rs` shares Request cancellation, ProcessGroup ownership and
output bounds between one-shot and reusable Unix sessions. `lib.rs::serve_worker`
is the common CLI loop: a fresh Runtime for each request, a separate duplex
control socket, ordinary output pipes. `program.rs` conservatively disables reuse
when native process launches could leave descendants between items. The CLIs'
executable test path preserves their configured worker command; embedding tests
with no command remain process-free.
