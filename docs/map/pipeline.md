# The pipeline (source → OIR → SPB → Sparrow)

There is one engine, Sparrow; Tweed is its next version (#903). Phase contracts are in `docs/phases.md`. Paths
are relative to `okkama-rs/crates/`. `OKKAMA_DEV=lower` covers the compiler stages and `runtime`, `jit` and
`gc` cover execution (§9 of LLM-GUIDE).

| # | Stage | Entry point (file) | Decides | Checks | devtrace |
|---|---|---|---|---|---|
| 1 | parse | `syntax::parse` (okkama-compiler/src/syntax) | AST | syntax errors | — |
| 2 | imports, analysis | `lowering::modules::load_graph`, `analysis::analyze_mode` (okkama-compiler/src/analysis) | names, types, roles, caps, errors (THIR) | the language's static rules (semantic errors) | — |
| 3 | lowering | `lowering::lower_linked*` → `lower_modules` (okkama-compiler/src/lowering) | OIR-1: typed SSA with unwind targets and **cleanup regions** (`region.enter/adopt/track/transfer/leave/unwind/settle`), the ownership rules of the Bible | refuses what it cannot lower | lower (`ir lowered`) |
| 4 | IR passes | `lower_modules`: `statics`, `regions` (plainness, unused regions), `tail-calls`, `fold`, `hoist`, `undef`, `jumps` (okkama-compiler/src/ir) | the same meaning with fewer operations | `ir::verify` after them, then the ownership linearity check in debug builds or with `OKKAMA_VERIFY_OWNERSHIP=1` (#910); `OKKAMA_IR_PASSES=none` turns them off | lower (`ir after passes`, pass timers) |
| 5 | Sparrow codegen | `codegen::sparrow::generate` (okkama-compiler/src/codegen/sparrow) | SPB 16: registers, instructions, handlers | register allocation check (debug, gate) | — (#912) |
| 6 | SPB load | `okkama_sparrow::decode`, `Program::verify`, `sparrow-core` contracts | a verified program | types, budgets, intrinsic contracts | — |
| 7 | JIT | `sparrow-core/src/jit.rs` (Cranelift, whole function on first call) | machine code, stack maps, helper calls | Cranelift verifier in debug | jit |
| 8 | run time | `sparrow-core`: `failures.rs` (identities, regions, resource walks, destroy), `tasks.rs` (async frames, `rt_region_settle`), `heap.rs` (GC), `drivers.rs`, `intrinsics.rs` | when each resource is destroyed (reverse serial order), cancellation and settling | at-most-once destroy, `already_closed`, budgets | runtime, gc |

A failure names its stage: `syntax error`, `semantic error`, a lowering error, `internal compiler error: lowering
produced invalid IR` (`OKKAMA_DUMP_INVALID_IR=FILE` writes it), an SPB verify error, or a run-time failure with its
trace.

Where one ownership rule lives in several rows, rules.md lists them. The aim is lowering plus the runtime, with
one elaboration pass (#909) and one debug-only checker (#910).
