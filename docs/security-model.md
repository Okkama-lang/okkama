# Okkama trust model

State of Sparrow 0.6.3. The guarantees below were checked against implementation and tests; this is not a
proof that arbitrary input is harmless. Budgets and native permissions are separate controls. Source/test
paths are relative to `okkama-rs/crates/` unless they start with `tools/`, `.github/` or `okkama-rs/`. Update
this document when a boundary changes.

## How to run untrusted input today

| Input | Boundary |
|---|---|
| Source | `okkama build x.ok -o x.spb` compiles without running `main`; compilation itself needs OS resource limits (§1). |
| Program | `okkama run x.ok` compiles and executes on Sparrow; `sparrow x.spb` executes the built artifact (§§2–3). |
| Bytecode | `sparrow check x.spb` checks decoding, verification and runtime intrinsic contracts without executing the entry (§2). |
| Embedding | Prefer the byte-input `sparrow_core::run_limited` boundary with explicit `budget::Limits`, plus OS isolation for hostile code (§5). |

## 1. Source

**Checked boundaries.**
- `build` lowers source and writes SPB; `run` lowers, encodes and executes on Sparrow. Compilation does not
  execute the program's `main` (`okkama-cli/src/main.rs`). Regressions: `okkama-cli/tests/cli.rs`,
  `build_dispatches_file_and_stdin_to_sparrow_and_writes_bytecode`;
  `every_run_form_executes_on_sparrow_whatever_the_environment`.
- Default parser limits are nesting 64, tree depth 128 and 2²⁰ nodes per file
  (`okkama-compiler/src/syntax/mod.rs`). Regressions in `okkama-compiler/tests/parser_robustness.rs`:
  `deep_nesting_is_reported_not_overflowed`; `long_chains_are_reported_not_overflowed`;
  `program_size_is_limited`.
- The lexer terminates on any text (`okkama-compiler/src/syntax/lexer/mod.rs`;
  `okkama-compiler/src/syntax/lexer/tests.rs`, `lexer_terminates_on_arbitrary_bytes`). Trees are freed without
  recursion (`okkama-compiler/tests/parser_robustness.rs`, `deep_expressions_drop_without_recursion`,
  `optional_and_choice_nodes_drop_without_recursion`).
- Imports use an explicit stack, limited to 1024 nested imports, 16 384 modules and 256 MiB of sources
  (`okkama-compiler/src/lowering/modules.rs`; `okkama-cli/tests/imports.rs`,
  `an_import_chain_past_the_depth_limit_is_a_diagnostic`, [#20]).
- Generic specialization is limited to depth 256 and 262 144 specializations; excess produces a diagnostic
  with the chain (`okkama-compiler/src/lowering/ctx.rs`; `okkama-compiler/tests/lowering.rs`,
  `endless_specialization_is_a_diagnostic_with_its_chain`, [#32]).
- Name conflicts identify both sources; `check` output is independent of file and hash-map order
  (`okkama-cli/src/main.rs`, `okkama-compiler/src/lowering/ctx.rs`; `okkama-cli/tests/names.rs`,
  `a_conflict_names_both_sources`, `the_order_of_inputs_changes_nothing`, [#33]).
- An independent analysis checks register allocation in debug builds and the release gate
  (`okkama-compiler/src/codegen/sparrow/check.rs`, `the_allocator_passes_and_each_corruption_is_caught`;
  `tools/gate/gate.sh`, [#30]). IR passes can be disabled with `OKKAMA_IR_PASSES=none` and are compared in
  that gate (`okkama-cli/tests/ir_passes.rs`, `every_pass_selection_prints_the_same`, [#29]).
- Nightly coverage-guided fuzzing covers lexer, parser, analysis, OIR verification, SPB decoding and execution
  (`tools/fuzz/run.sh`, `.github/workflows/fuzz.yml`; targets `lex`, `parse`, `analyze`, `ir_verify`, `spb`,
  `exec` in `okkama-rs/fuzz/fuzz_targets/`, [#34]).

**Not guaranteed.** These checks are not a total compiler CPU/RSS bound; large-function compilation and
analysis type depth need separate consideration ([#49], [#50]). Compilation reads imports and build writes its
output under the host user's permissions. Use an isolated process with bounded input, time, memory and
filesystem access for untrusted source.

## 2. Bytecode `.spb` on Sparrow

**Checked boundaries.**
- Decode limits input to 256 MiB and accounts decoded elements against 512 MiB before allocation
  (`okkama-sparrow/src/codec.rs`). Regression: `counts_are_bounded_by_the_input_and_the_decoded_size`.
  LEB128 numbers must be canonical (`okkama-sparrow/src/codec.rs`, `numbers_are_canonical_and_bounded`).
  Decode verifies the resulting program; malformed/truncated input has codec coverage in
  `okkama-sparrow/tests/codec.rs`, `decoding_verifies_the_program`, and
  `every_truncation_and_corrupted_byte_is_a_positioned_error` ([#24]).
- Structural/type verification checks register types and signatures, including runtime tables
  (`okkama-sparrow/src/verify.rs` and `okkama-sparrow/src/typing.rs`). Regressions:
  `okkama-sparrow/tests/verify.rs`, `rejects_false_register_types_and_signatures`;
  `rejects_runtime_tables_with_false_signatures` ([#16]).
- Verifier initialization analysis is limited to 64 MiB per function (`okkama-sparrow/src/verify.rs`,
  `MAX_ANALYSIS_BYTES`, [#24]). Decode-to-verifier coverage: `okkama-sparrow/tests/codec.rs`,
  `decoding_verifies_the_program` (no dedicated limit regression).
- Machine frames are limited to 256 KiB, within 1 MiB of stack headroom checked at each prologue
  (`sparrow-core/src/jit.rs`, `sparrow-core/src/runtime.rs`). Stack exhaustion reports `Stack overflow`, or
  memory-budget exhaustion when that budget caps the stack. Destroy/closure recursion coverage:
  `okkama-cli/tests/nested.rs`, `recursion_through_rust_ends_in_a_failure_and_the_process_goes_on` ([#25]).
- Runtime helper signatures are checked at compile time (`sparrow-core/src/jit.rs`;
  `okkama-cli/tests/helpers.rs`, `every_runtime_helper_runs`, [#23]). Exceptional edges preserve values read
  by handlers, including under collector stress (`okkama-cli/tests/handlers.rs`,
  `handlers_preserve_reports_under_collector_stress`, [#22]).
- The byte-input execution API decodes/verifies before constructing the runtime; runtime construction also
  checks intrinsic contracts (`sparrow-core/src/lib.rs`; `sparrow-core/src/runtime.rs`). `check` uses the
  same decoder and contracts without running the entry (`sparrow-core/src/lib.rs`). Regressions:
  `sparrow-cli/tests/cli.rs`, `check_does_not_execute_a_valid_infinite_loop`;
  `check_refuses_intrinsics_against_their_runtime_contract`.
- Channel endpoints require exact payload agreement through `Program::same_type`, including role
  implementations (`sparrow-core/src/contracts.rs`, `okkama-sparrow/src/lib.rs`). Regression:
  `okkama-cli/tests/intrinsic_contracts.rs`, `endpoints_of_one_channel_carry_one_payload_type` rejects
  even unused receiving intrinsics with a different reference payload type ([#69], [#375]). This does
  not establish every generic intrinsic substitution or the full selector matrix.
- JIT compilation estimates live-register/block cost and rejects an estimate above 256 MiB or the configured
  lower memory budget (`sparrow-core/src/jit.rs`). Regression: `sparrow-core/tests/compile_cost.rs`,
  `many_values_live_across_fallible_instructions_are_refused_before_compiling` ([#73]). This is an estimate,
  not an allocator or process RSS ceiling.

**Use today.** Verify bytecode before execution and retain an OS memory/time limit. Verification does not
authorize host effects or prove the JIT/native implementation free of defects. Do not treat low-level
caller-constructed `Program`/`Loaded` values as the checked byte-input embedding boundary.

## 3. Programs

**Checked boundaries.** `--max-steps`, `--max-memory`, `--max-output` and `--timeout` configure execution
controls; they are unset by default (`sparrow-core/src/budget.rs`). Steps charge function entries and loop
back-edges, not every instruction or unit of native work. Budget exhaustion is a special stop: program
handlers cannot catch it and ordinary destroy/defer cleanup is not promised (`sparrow-core/src/budget.rs`;
`sparrow-core/src/runtime.rs`). Regressions: `okkama-cli/tests/budgets.rs`, `every_budget_stops_sparrow`, and
`steps_are_counted_alike_and_running_out_cannot_be_caught` ([#21], [#71]).

Shared native value graphs are freed without recursion (`okkama-runtime-core/src/heap.rs`,
`deep_chains_are_released_without_recursion`, [#28]). SQLite cancellation handles reach only their own
connection: a closed connection's handle does not touch the next one on the same worker
(`sparrow-core/src/host/native_sqlite.rs`, `sqlite_interrupt_reaches_only_its_own_connection`, [#18]). X.509 path search has a 1024-step budget (`okkama-runtime-core/src/native/x509.rs`,
`ambiguous_issuers_end_in_a_defined_refusal_within_the_budget`, [#17]). Private RSA operations (key
generation, PSS/PKCS#1 v1.5 signing, OAEP decryption) use aws-lc's constant-time, blinded implementation
(`okkama-native/src/ops.rs`; `okkama-native/tests/vectors.rs`, `rsa_signatures_and_oaep_round_trip`,
`rsa_interoperates_with_openssl_including_x509_legacy_forms`, [#70]). These functional tests do not establish
timing behavior independently of the provider.

**Native cancellation limits.** Interruption is cooperative. The host flag is passed to interruptible native
operations and checked again after their return (`sparrow-core/src/natives.rs`). Sort comparisons and PBKDF2
have cancellation coverage, but a non-cooperative call such as Argon2 need not return promptly. The CLIs
install a watchdog which attempts process exit one second after the interrupt flag is raised
(`okkama-cli/src/main.rs`; `sparrow-cli/src/main.rs`). This is process termination, not successful native
cancellation or guaranteed cleanup. Regressions: `okkama-cli/tests/budgets.rs`,
`a_long_sort_stops_for_the_timeout`, and `long_host_operations_stop_for_the_timeout` ([#72], [#237]).
Embedding `run_limited` does not install that CLI watchdog: an interrupt flag is not a hard deadline for an
in-flight native call. Use an external process supervisor for that boundary.

**Host access.** Ordinary CLI execution uses unrestricted native permissions (§5). Resource flags do not
restrict files, network, processes or environment. Use an OS sandbox/container with appropriate mounts,
credentials, network policy and external limits; native groups alone are not OS isolation.

## 4. Packages

`blade` stages package/lock changes and recovers interrupted transactions (`blade/src/install.rs`;
`blade/tests/install.rs`, `an_interrupted_install_recovers_to_one_consistent_state`, [#39]). This does not
establish that a dependency is trustworthy. `.github/workflows/audit.yml` audits both `okkama-rs/Cargo.lock`
and `okkama-rs/fuzz/Cargo.lock` with `--deny warnings` and the same `okkama-rs/.cargo/audit.toml` policy
([#19], [#960]). An advisory snapshot cannot prove absence of defects. Release gate/publishing policy lives in
`.github/workflows/packages.yml` ([#26]).

## 5. Embedding `sparrow-core`

**Native policy.** `Limits::default().natives == None` permits all natives. `Some(vec![])` denies
classified host groups; pure computation and stdout/stderr remain allowed. Listing `NativeGroup::Clock`
permits Clock and denies other groups. An explicit restriction refuses unclassified natives.
Sync and async dispatch enforce this (`sparrow-core/src/natives.rs`, `sparrow-core/src/drivers.rs`).
Regressions: `sparrow-core/src/budget.rs`, `every_native_is_classified`, `the_preferred_languages_need_env`;
`sparrow-core/src/runtime.rs`, `a_native_outside_the_permitted_groups_fails_the_program`.
Classification is not proof of every provider's internal behavior ([#31], [#405]).

An explicitly restricted configuration:
```rust
use sparrow_core::budget::{Limits, MemoryScope};
let limits = Limits {
    natives: Some(vec![]),
    steps: Some(1_000_000),
    memory: Some(64 << 20),
    memory_scope: MemoryScope::Runtime,
    output: Some(64 << 10),
    ..Limits::default()
};
// Pass limits to sparrow_core::run_limited; add a host interrupt flag as needed.
```

This still permits computation and output, and is not a filesystem-root, network-destination or hostile-tenant
isolation mechanism. Keep OS isolation.

**Memory accounting.** `MemoryScope::Process` is the default. It requires a process-wide probe installed with
`set_memory_probe`; subsequent installations are ignored. Sparrow measures the probe's change from the
runtime's baseline, not total RSS. Other threads/runtimes can affect that reading. CLIs use mimalloc committed
bytes; `Counted` is available for an embedder's allocator (`sparrow-core/src/budget.rs`;
`sparrow-core/src/runtime.rs`; `okkama-runtime-core/src/budget.rs`). Regression:
`sparrow-core/tests/budget.rs`, `runaway_programs_stop_within_their_budgets`.

`MemoryScope::Runtime` instead reads `Heap::held_bytes`: heap pages, large objects and accounted Rust memory
owned by heap objects, without a process probe (`sparrow-core/src/runtime.rs`; `sparrow-core/src/heap.rs`).
Regression: `okkama-cli/tests/runtime_budgets.rs`, `runtimes_in_one_process_keep_their_own_memory_budgets`
([#269]). It excludes memory outside that accounting, including native-library allocations and other threads;
it is not a total per-tenant or process quota. Polling and selected allocation preflights do not bound every
native allocation. The narrower join preflight is covered by `okkama-cli/tests/native_budgets.rs`,
`a_join_past_the_memory_budget_is_stopped_before_it_is_built` ([#72]).

**Failure containment.** Native dispatch catches a provider panic and poisons that runtime
(`sparrow-core/src/runtime.rs`). Regression: `a_panicking_native_is_an_internal_error_and_poisons_the_runtime`
also checks that a fresh runtime works. This is not a promise to recover from arbitrary unsafe/FFI faults or
panics crossing another `extern "C"` helper ([#23], [#31]). Memory accounting and native permissions do not
make all host state runtime-local; environment/default-directory isolation remains a separate embedding
concern ([#40]). `run_worker` is the CLI worker entry, not the recommended untrusted-input embedding API.

**Parallel map workers (#946).** On Unix, `parallel/map` reuses worker processes only for one
`results()` call. Each item still gets a fresh Runtime, heap, language globals, host context and receiver
reconstructed from declared fields (never its `once` cache). Native process-wide caches/statics, allocator
state and foreign changes to the true process environment/directory are not reset between items. This is
not a security boundary between mutually untrusted items. Ordinary `spawn` retains one process per item;
programs containing native process launches also keep that path, so their descendants retain per-item
process-group teardown. Without a configured worker command, no process executor is installed. Both
transports kill/reap cancelled workers before settlement; the run closes/reaps idle workers too. Tests:
`sparrow-core/src/worker_process.rs` (`pool_tests`), `sparrow-core/src/worker.rs` (`wire_tests`), and
`okkama-cli/tests/parallel_map.rs`.


[#16]: https://github.com/Okkama-lang/okkama/issues/16
[#17]: https://github.com/Okkama-lang/okkama/issues/17
[#18]: https://github.com/Okkama-lang/okkama/issues/18
[#19]: https://github.com/Okkama-lang/okkama/issues/19
[#20]: https://github.com/Okkama-lang/okkama/issues/20
[#21]: https://github.com/Okkama-lang/okkama/issues/21
[#22]: https://github.com/Okkama-lang/okkama/issues/22
[#23]: https://github.com/Okkama-lang/okkama/issues/23
[#24]: https://github.com/Okkama-lang/okkama/issues/24
[#25]: https://github.com/Okkama-lang/okkama/issues/25
[#26]: https://github.com/Okkama-lang/okkama/issues/26
[#28]: https://github.com/Okkama-lang/okkama/issues/28
[#29]: https://github.com/Okkama-lang/okkama/issues/29
[#30]: https://github.com/Okkama-lang/okkama/issues/30
[#31]: https://github.com/Okkama-lang/okkama/issues/31
[#32]: https://github.com/Okkama-lang/okkama/issues/32
[#33]: https://github.com/Okkama-lang/okkama/issues/33
[#34]: https://github.com/Okkama-lang/okkama/issues/34
[#39]: https://github.com/Okkama-lang/okkama/issues/39
[#40]: https://github.com/Okkama-lang/okkama/issues/40
[#49]: https://github.com/Okkama-lang/okkama/issues/49
[#50]: https://github.com/Okkama-lang/okkama/issues/50
[#69]: https://github.com/Okkama-lang/okkama/issues/69
[#70]: https://github.com/Okkama-lang/okkama/issues/70
[#71]: https://github.com/Okkama-lang/okkama/issues/71
[#72]: https://github.com/Okkama-lang/okkama/issues/72
[#73]: https://github.com/Okkama-lang/okkama/issues/73
[#237]: https://github.com/Okkama-lang/okkama/issues/237
[#269]: https://github.com/Okkama-lang/okkama/issues/269
[#375]: https://github.com/Okkama-lang/okkama/issues/375
[#405]: https://github.com/Okkama-lang/okkama/issues/405
[#960]: https://github.com/Okkama-lang/okkama/issues/960
