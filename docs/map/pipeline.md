# The Champ pipeline (OIR-2 → SPB 17 → Champ)

The driver is `okkama-compiler/src/lowering/oir2/pipeline.rs`. Its `compile_high_with` and `compile_coro_stages`
functions run every stage in order. A failure reads `OIR-2 <phase>: <message>`, and the phase names the row
below. `OKKAMA_DEV=stages` times each stage, and adding `,dump` writes each function after high, own, coro and
low. Paths are relative to `okkama-rs/crates/`.

| # | Phase in errors | Entry point (file) | Decides | Checks | devtrace |
|---|---|---|---|---|---|
| 1 | `imports`, `analysis` | `lowering::modules::load_graph`, `analysis::analyze_mode` (okkama-compiler/src/analysis) | names, types, roles, errors (THIR) | the language's static rules (semantic errors) | front |
| 2 | (lowering) `unsupported in OIR-2 lowering: …`, exit 4 | `lower_to_oir2` (okkama-compiler/src/lowering/oir2/) | OIR-2 High: lexical scopes, bindings, defers, async ops | refuses constructs it cannot lower | lower |
| 3 | `contracts` | `P1NativeSummaries::for_module` (lowering/oir2/native_summaries.rs) | contracts of natives and intrinsics | ABI of natives (NativeSummaryError) | stages (about 1.7 s on a small program in debug) |
| 4 | `verify_high` | `okkama_oir::verify_high_with` (okkama-oir/src/verify) | — | High well-formedness | verify |
| 5 | `own_elaborate`, `own_rewrite` | `own::elaborate_ownership` (own/mod.rs), `own::rewrite_subset` (own/driver.rs) | **who owns what**: per-function flow analysis (own/analyze.rs), emission of splits, joins, admits, binds, scope begin and end, Future duties, dynamic steps (own/driver/*.rs), inventories and effects (own/scopes.rs) | its own budget: 50M work, 256 MB (`Incomplete`) | own |
| 6 | `contracts_own`, `verify_own` | `verify_own_with` | — | ownership linearity again (verify/linearity, precise_linearity) | verify |
| 7 | `coro`, `verify_coro` | `coro::lower_coro` (okkama-oir/src/coro) | async frames, suspension brackets | B9 bound Futures and others (verify/coro.rs, coro_flow.rs) | coro, verify |
| 8 | `low`, `verify_low` | `low::lower_cleanup` (okkama-oir/src/low) | cleanup plans, executors, layouts | C-2 and others | low, verify |
| 9 | `layout` | `layouts()` / `sparrow_oir2::objects` (pipeline.rs) | object layouts | missing runtime overrides | — |
| 10 | `compile_low` (codegen) `GenerateError("…")` | `sparrow_oir2::compile_verified_low` → `generate_with_allocation` (okkama-compiler/src/codegen/sparrow_oir2/mod.rs, cleanup.rs) | SPB 17 code, proofs and evidence (static selections, closes, envelopes) | refuses what it cannot encode | codegen |
| 11 | `compile_low` `GenerateError("RULE: …")` with an UPPER_CASE rule | `s::verify_load` (okkama-sparrow/src/spb17/load.rs, automaton.rs, services.rs, evidence.rs, suspension.rs) | — | **re-derives ownership from the bytecode** (the dominant compile cost) and checks the proofs. Its per-function check names are `native inputs` … `automaton`, `allocation` | loader |
| 12 | `encode` | `s::encode` | SPB 17 bytes | — | — |
| 13 | run time | sparrow-core: `champ/` (rt_* helpers: cleanup, futures, groups, streams), `jit.rs`, `heap.rs`; Tomato's regions in `failures.rs` | destroys, settles, cancels | `already_closed`, budgets | runtime, jit, gc |

Build modes: a `champ-abi` build of okkama-cli runs only SPB 17, with `OKKAMA_PIPELINE=oir2`. Structural
verification is the default, and Full verification needs `OKKAMA_VERIFY_OWNERSHIP=1` (a CI gate; performance
frozen). The conformance adapter builds Champ with `--profile gate`.

Where one rule lives in several of these rows, rules.md lists them.
