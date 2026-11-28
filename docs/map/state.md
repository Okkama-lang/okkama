# State (update at the end of every task)

## Release 0.6.7 (release/067, from main 418da2b86): performance

**Release 0.6.7, 2026-11-28 (owner on vacation, authorized):** every v0.6.7 ticket closed (remainders POSTPONED:
#1182–#1186). Gate = CI + Wave green on 59acebe33 (local gate refused by the test-protocol hook). Micro A/B v0.6.6 →
release/067: −30.2% over 66 lang cells, none slower than +3% (okkama-docs/benchmarks/reports/2026-11-28-v0.6.7/micro.html).
Showcase vs v0.6.6: cold −27%, steady −41% (report.html there). Version 0.6.7, CHANGELOG [0.6.7], Bible rebuilt; then PR
release/067 → main, tag v0.6.7 on the merge commit, packages, install-pgo, /man/, playground (SPB 19), develop.

**Owner on vacation 2026-11-28 → ~2026-12-12; release alone in /loop (memory vacation-release-0-6-7).** Authorized:
merge PR release/067 → main, tag v0.6.7, GitHub Release, design decisions with Fable, close/postpone myself. Plan:
#1177, #1176 done → #1152 (Fable's design in hand: structure index known at birth) → #1151 (Fable design pending; naive sync explicit roots measured 02 +122%, 09 +36%) → #1178 done → #1151 → #1152 → #1150/#1153 → release process (micro A/B vs v0.6.6,
showcase, Bible, man, CHANGELOG, version, PR, merge, tag, release, install-pgo, cleanup). Line: worktree
okkama-work/p-1110 (feature/1110) → release/067 + wave/067-*.

Baseline: micro A/B v0.6.5→v0.6.6 (+0.5%, layout noise) and the showcase ladder report
(okkama-docs/benchmarks/reports/2026-11-27-v0.6.6/); per-ticket micro A/B against the v0.6.6 toolchains
(okkama-work/micro-tc/v0.6.6-s{1,2}). Board v0.6.7: Perf 1–10 (#1105–#1114), Perf 11–18 from the showcase
(#1146–#1153), book findings #1138–#1145. Perf tickets: lead + Fable; small low-risk chores may go to Opus agents.

- [x] #1178 floats: `small_eligible` admits Kind::Float and float add/sub/mul/div/neg/abs/sqrt/cmp and float constants
  (not `Convert`, whose fast path reads an int tag); 03-float 10.44 → 5.20 ms. Test okkama-cli small_ints
  `float_functions_get_small_int_versions_with_the_general_versions_results` (restart with live floats, a float
  overflow's trace, on = off). Remaining 03 gap: per-op finiteness checks (fsub/fcmp/branch after each op).
- [x] #1176 string slices: `Heap::text_chain` (SHARED and the storage is a BUFFER) guards `concat_text`/`concat_parts`;
  `text_buffer_write` asserts BUFFER; `substring` shares via `Heap::allocate_text_slice` when ≥ `SLICE_MIN` (64) and
  2×piece ≥ storage (`text_storage_bytes`: flat length or buffer room), never a view of a view. Tests
  `ops::strings_1176_tests` (5; mutations of the sharing and of the guard caught); micro cell `string-rest`.
  Ladder ±1.4%.
- [x] #1177 string chains: lowering `string_concat` flattens `a + b + c …` (as `collection_operator` does) into
  `InstKind::StrConcatN` / SPB `str.concat_n` (164, SPB 19); `ops::concat_parts` (one allocation; a part ≥ 256 B or
  SHARED/APPENDED/PREPENDED → pairwise `concat_text`; APPENDED when all but the last reach 256 B), `rt_concat_n`
  (JIT spills the parts, interpreter gathers them). Tests `ops::strings_1177_tests` (4, mutation caught), lowering
  `a_chain_of_string_plus_is_one_concat_of_its_parts_in_order`, mnemonics. 05-strings −16%. Owner chose the
  compiler over the ticket's JIT-only plan: consecutive concats are rare (1 pair in ~20 per program).
- [x] #1171 builders in the heap: `heap::BUILDER` cell (length, capacity, buffer: an ARRAY cell of length 0, a leaf
  the builder traces up to its length), inline push in the JIT, `Heap::builder_finish` in place when the cell fits
  (same class, large ≤ 1/8 room) else copy, spare buffers until the next collection (`Heap::spare_buffers`),
  `builder.sized` (SPB 19, opcode 163; `map` over range/array/dictionary via `Lowerer::source_count`, capped at
  4M); Builder payload and `payload_mut_unchecked` gone. Tests `heap::builders_1171_tests` (7, mutations caught),
  dispatch_537, mnemonics. Steady 10-app −14%, 11 −14%, 08 −8%, 05 −3%, 06 ±0; 08 footprint 28.4 → 21.1 MB.
  `filter` sized by its source rejected (06 −4…−6%, 10-app +5%).
- [~] #1150 collections: int sort by tagged words (`sort_natural` fast path, test `ints_sort_in_value_order_small_or_big`);
  large builder buffers are raw allocations resized by realloc (`Heap::builder_large`, `allocate_large_buffer`,
  `resize_large_buffer`, `take_large_buffer` → `large`, freed by the sweep when unreachable); 06-collections
  10.9 → 6.67 ms (target ≤ 6), 50 collections per 20 repetitions (40 before #1171 counted builder memory, 80 after;
  room given back by a shrink is uncounted).
  Left: GC share (~25%) and the closures' JIT code.
- [~] #1151 roots (Fable's design): vendored `cranelift-frontend` 0.137.0 (`vendor/cranelift-frontend`, OKKAMA.md) with
  `FunctionBuilder::set_stack_map_reloads`; jit.rs `reload_policy` (default: no reloads in synchronous functions with a
  loop; `SPARROW_CORE_RELOADS=always|never`). 07 −16%, 10 −7%, 06 −2.5%, 09 +4.8% (bisected: lambda#3's closure-calling
  loop), 08 unchanged (12.6 ms; target ≤ 10 not met: Fable's next lever is sealing an instruction's blocks early, so
  loop-invariant values stop going through loop-header parameters). Naive sync explicit roots: 02 +122%, rejected.
  Earlier: breakdown on the ticket; a per-(role, slot) code table measured no gain (+0.4%), not kept; the
  lever is the frontend's spills around calls (#1147's deferred own root slots).
- [~] #1152 ownership, Fable's design (structure index known at birth): step 1 done — `Resources::positions`,
  `array_structure`, `inherit_slice_structure` (from `ops::slice_array`, copied slices), `may_hold_identity`; test
  `structure_tests::a_copied_slice_inherits_its_parents_index_inside_its_range` (range mutation caught); 11 −7%,
  indexes built 1002 → 2. Next: the INERT header bit (bit 30) and a table-driven array scan.
  Earlier: breakdown on the ticket (GC ~40%, region transfer walk ~8%); 2× growth threshold measured
  (−4%, +6…21 MB footprint), not kept; first repetition ≤ 2× steady met by #1147. Steady ≤ 4 ms needs a model design.
- [~] #1153 memory: breakdown on the ticket (builder Vec doubling 4.2 MB on 08-roles → gone with #1171: 28.4 → 21.1 MB;
  collection floor #920; compile worker +2.6…4.4 MB); macOS `time -l` RSS of the `sparrow` binary reads ~1 MB (odd).
- [x] #1149 strings: known-ASCII allocation (`allocate_text_with`, `text_new_of`), char split, `match_starts` (direct
  search ≤ 256 bytes), one-pass replace (`allocate_text_filled`), `Runtime.scratch_{offsets,ranges,words}`,
  `heap::copy_text`; tests `ops::strings_1149_tests` (5, mutations caught). 05-strings 12.3 → 9.05 ms. Views for
  split pieces rejected (Fable: −1%, retention); latent: `concat_text` treats any SHARED string as a buffer — a
  guard (`text_chain`: backing has BUFFER) is needed before anyone builds flat-backed views.
- [x] #1174 marking: generic `trace_object<P>`, leaf kinds unqueued in `mark_and_queue`; test
  `heap::tests::leaves_are_marked_without_tracing_and_containers_still_trace_1174` (mutation caught); corpus under
  GC stress unchanged (457/457).
- [~] #1147 first-run compile: step 1 done (A5 no-std frontend, A1 exits by handler target + shared cold return
  from the second exit, A4 no check after int add/sub/mul without budget/polling); reverted: A2 (no check after
  rt_compile: regalloc loss on 09-closures), A3 (direct branch edges: 07-objects +32%); A6 (context reuse) no gain.
  Step 2 done: `emit_unit`/`codegen`/`install` split (2a, same code), `jit/background.rs` Pool (queue, steal,
  wait), `compile_ordinary`, `speculate` (depth 2, 64 pending), `take_pending` (stale rule), `background_allowed`;
  default 1 worker (2 were no faster, +3 MB more peak footprint). Tests: background_units_install_as_a_synchronous_run_compiles_them (stale
  mutation caught), a_runtime_with_units_in_flight_drops_cleanly; corpus identical 0/2 threads, also GC stress.
  Tests: ready_callees_tests::handlers_with_one_target_share_one_failure_exit,
  budgets SQUARE (mutations caught). Step 2 (background codegen, Fable's design on the ticket) next.
- [x] #1175 safepoint not-due check: `Runtime::safepoint` inline + `collect_at_safepoint` cold; `Heap.node_counters`
  (`pmap::memory_cell`, Heap !Send by the pointer); test `heap::tests::map_nodes_of_this_thread_bring_a_collection_due_1175`
  (mutation: node growth ignored, caught). Steady: 10-app −6.2%, 05 −7.2%, 06 −7.1%, 11 −2.8%, 08 −2%.
- [x] #1148 bigint add/sub: vendor/num-bigint (0.4.8 + `|` carries, 4-digit add step; OKKAMA.md), workspace
  `exclude = ["vendor"]`, `[patch.crates-io]`; test `heap::tests::bigint_addition_matches_a_reference_1148`
  (mutations caught); micro cell int-big-add + oracle. 04-bigint 205 → 118 ms (an ADCS asm variant: 104 ms,
  not taken: upstream wants Rust-native, num-bigint#314). Upstream PR: owner yes.
- [x] #1114 GC: steps 1–2 done (design Fable on the ticket): `OWNER_BIG`/`OWNER_PAYLOAD`, `CELL_WORDS`,
  `Liveness` (granule 4 KiB, cap 2^20), `Marker`, `Reserved { head, bytes, page, threaded }`, `thread()`,
  `dead_pages_unread`; poison after `release_empty_pages`. Tests `heap::tests::*_1114` (3, mutations: owner
  allocation, granule shift, poison skip, threading short — all caught). Corpus under GC stress vs base: 364/364.
  Step 3 (bump lanes) rejected by its gate: 10-app +0.8…+5.6% vs step 2 (cycles +5.6%), 05-strings +2…6%;
  reverted (numbers on the ticket). Burst's +1.1 MB allocated with owner classes is
  unexplained (not whole pages; a separate empty-page allowance did not change it).
- [x] #1146 closed: no start-up regression (A/B v0.6.0/v0.6.6/now); breakdown on the ticket; pre-main → #1173.
- [x] #1103 DB schema → std RandomState (Opus agent, feature/1103, cherry-picked): benchmarks in the ticket
  comment (scratchpad hbench), FastHasher `write` debug_assert (no text keys), integer writes mixed directly;
  tests `cargo test -p sparrow-core --lib 1103` (4, mutation-checked). Audit: no other FastMap with external keys.
- [x] #1113 small-int function versions (feature/1110; toolchains micro-tc/f1113a tagged, f1113b untagged):
  `jit::{small_eligible, Version, GENERAL_VERSION}`, `Jit::{general, general_ids, small_ints}`, Emitter
  `small`/`entry_args`/`restart`; `compile` makes the small version as the code, `rt_compile(f | GENERAL_VERSION)`
  the general one at the first restart. Test `okkama-cli/tests/small_ints.rs` (exact results at the boundaries,
  `% 0` trace, on = off; mutations: no overflow check, no entry guard, no range check — all caught). Sweep
  scratchpad sweep1113.py: 364/364. Micro baseline −60%, int-small −40%; 01-arith 10.8 ms. Not done: mid-function
  transfer (restart from the entry instead), versions of functions with calls or allocations.
- [x] #1112 OIR `inline` + `scalars` (feature/1110 line; design Fable; owner: accept the step-count shift):
  `ir/inline.rs` (`inline_leaves`, `inline_branched`, `has_cycle`, `INLINE_INSTRUCTIONS` 24 /
  `INLINE_BRANCHED` 64 / `CALLER_INSTRUCTIONS` 400, `Attr::Inlined` + verifier `grants`), `ir/scalars.rs`
  (candidates incl. block parameters; never split parameters an edge fills itself or of unwind targets),
  `cfg::renumber_values`; passes after tail-calls, PASSES has 10 names. Tests `tests/scalar_replacement.rs` (8,
  mutation-checked; the unwind-target rule has no unit reproducer, the examples corpus covers it). Corpus sweep
  script scratchpad sweep1112.py (examples+t/ with passes all vs without, verifier on): 364/364. Micro successor
  −76%, constructor −58%, for-loop −66%; 07-objects 8.4 → 5.4 ms; 10-app unchanged. Not done: in-place reuse
  (Fable: adds nothing after scalar replacement), materialisation of escaping candidates.
- [x] #1111 closed with its cheap parts (owner, 2026-11-27: the full builder redesign is #1171, POSTPONED): fast
  small-int text (fa3bd1836, ebebaf4e8 the length limit), empty builder → shared empty array (test
  `an_empty_builder_finishes_as_the_shared_empty_array`). 10-app 53.5 → 52.5 / 53.6 → 53.6 ms (two seeds),
  06-collections −1.5% / +0.5%: noise. Fable's design and the cost estimate are on #1111 and #1171.
- [x] #1110 closure entries (feature/1110; worktree okkama-work/p-1110, target t-1110; toolchains micro-tc/f1110a
  base, f1110b stage 1, f1110c both): closure layout `[Raw entry] ++ params` (`heap::CLOSURE_ENTRY` 8,
  `CLOSURE_CAPTURES` 16, `Heap::capture`), `CallTarget::Code`, the call site's entry fast path, `compile_entry`
  with `captures`, `Jit::{closure_ids, closure_entries, closure_requests}`, `Emit::closure_entry`,
  `CLOSURE_ENTRY_INSTRUCTIONS` 400. Stage 1 alone (only capture-less closures) was +2% on 09-closures: captured
  closures paid the entry test; stage 2 made it −23%. Tests `interp::tests::closure_entries_call_with_a_trailing_
  closure_past_the_register_arguments` (10 params), the inline-closure tests under GC stress; mutations (argument
  order, capture offset) crash. Agents merged on this line: #1139 #1141 #1142 #1144 #1138.
- [x] #1109 binary diet, closed by the owner with its first parts ("not a top priority"; the rest in #1170,
  POSTPONED): strip + symbols archive in tools/package.sh (771a3f470), core tables re-exported from runtime-core
  (8b52c1f24; −16 KB only: LTO had merged the copies), opt-level "s" for rustyline, pulldown-cmark, toml,
  libsqlite3-sys, brotli, brotli-decompressor, regex-syntax (−0.80 MB on Linux; SQLite alone −0.75 MB). The micro
  ABBA at N=4M does not fit std cells of milliseconds per iteration (the B campaigns failed): probe with --n 4000
  --runs 3 instead. Toolchains micro-tc/f1109{a,s,u}-s*. Slim build: no, it is #1154.
- [x] #1169 flaky listen-backlog test (feature/1169 0ba0eef5e): macOS reads SO_LISTENQLIMIT instead of counting
  peers (8 of 8 completed under parallel load); an Opus agent's first fix (1 s patience) failed under load.
- [x] #1108 shared failure exits + map registration (feature/1108; worktree okkama-work/p-1108, target t-1108;
  toolchains micro-tc/f1108m-s1 maps only, f1108x-s{1,2} both): `Emit::exits` keyed (function, inline fail block,
  handler index), `failure_exit`, `record_and_handle`; async bodies with explicit roots keep an exit per call.
  `register_code` (sorted splice of a function's maps, ordered insert of its range). Tests
  `register_code_tests`, `ready_callees_tests::calls_share_one_failure_exit` (one `trace` per handler context);
  `ready_static_target…` expects +2 branches (the compile path shares the call's exit). 10-app: code −22%, CLIF
  −14%, compile −10% (target −25% not reached); map registration not measurable at 34 functions; micro +0.3%.
  Compile profile script: scratchpad compile-profile.sh (SPARROW_CORE_PROFILE_COMPILE, sums of the marks).
- [x] #1107 stage 2 cold blocks (feature/1107; worktree okkama-work/p-1107, target t-1107; toolchains
  micro-tc/f1107c-s{1,2}): `check`'s failure blocks always cold (the `Emit::cold` flag of #1105 removed), cold
  allocation refill, compile-on-first-call, array index outside, int arith/compare/bitwise/not-neg slow paths,
  atomic intrinsic slow path. Not cold (frequent on real data): equal-length string compare, non-ASCII `StrLen`,
  structural `val.cmp`. Micro recursion −11.9%, method-call −8.1%, int-big +0.1%. Stage 1 (explicit roots for
  sync functions) prototyped on Fable's design and rejected with numbers (decisions.md); wave 067-a fixes in
  29bc8987a (dead `closure` helper removed, helpers test array literal, clippy).
- [x] #1168 (#1106 stage 3) shared instances (design and safety review: Fable): `heap::Canonical` (empty arrays per
  repr incl. ARRAY_BYTES, empty maps per (key, value) Repr, fieldless objects per shareable descriptor; marked in
  `collect_inner`, cleared in `Drop`), `Heap::{set_shareable, empty_array, empty_map, shared_object}`,
  `runtime::shareable_objects` (no fields, no destroy, not builtin, not held, no traced slot, no Error witness),
  `extend` refuses "the extension changes published object sharing". Substituted at construction only: JIT
  (`ObjNew`/`VariantNew`/`arr.new []`/`map.new []` as constants), interpreter, `rt_array` count 0, ops `MapNew`
  without entries. Tests `interp::tests::{shareable_types_and_shared_instances, shared_empty_values_stay_empty}`;
  mutations (no rooting, no Error exclusion) caught. Micro map-empty 39.37 → 7.33 ns, match −8.7%. Also: the
  no-JIT build's dead-code warning on `closure_words` (#1167) silenced.
- [x] #1167 (#1106 stage 2) inline array literals ≤ 16 (`INLINE_ARRAY_LITERAL`, `Emit::array_literal`, byte arrays
  PACKED eight to a word) and closures (`Heap::closure_words`, the `closure_new` layout; rt_closure's checks are a
  guard, else the helper). Tests `interp::tests::inline_array_literals_match_the_interpreter` (int with a BigInt,
  str, byte × sizes 0–17, JIT with GC stress too), `inline_closures_keep_their_captures_under_collection`;
  mutations (byte packing, a skipped capture) caught. Micro closure-create 14.32 → 5.49 ns, array-literal 12.42 →
  8.22 ns (the cell also pays two `%` and an index). Toolchains micro-tc/f1167-s{1,2}.
- [x] #1166 (#1106 stage 1) scalar inlining (feature/1106; worktree okkama-work/p-1106, target t-1106; toolchains
  micro-tc/f1166-s{1,2}): byte `& | ^ ~ << >> >>>`, `int.ushr`, `int.abs`, `sign` (int, float), `float.sqrt`,
  `bits` actions inline (`Emit::{small_count, tag_if_small, tagged_sign, slow_generic, fail_generic, bits}`, trap
  `GENERIC_SLOW_PATH_RETURNED` = 2). Tests `scalar_inline_tests.rs` (inline vs `jit.plain` on edge values, fast
  inputs never reach rt_exec; mutations clz offset and abs overflow caught). Micro byte 10.18 → 2.72 ns (−73%).
  Design of all three stages on #1106 (sub-issues #1166–#1168).
- [x] #1105 Perf 1 float fast paths (feature/1105; worktree okkama-work/p-1105, target t-1105; toolchains
  micro-tc/f1105-s{1,2}): `rt_scalar_safepoint` removed, float `+ − × ÷` finiteness by `x − x`, cold diverging slow
  path with `trap(user 1)` after `check`, cold `check` blocks inside slow paths (`Emit::cold`), `fcvt_to_sint_sat`
  after the range proof (Fable's review); test `float_arithmetic_fails_on_non_finite_results_and_handlers_take_
  the_failure`. Micro float 9.09 → 3.40 ns (−63%); full suite before the review fixes −1.9% (56 cells), short A/B
  after them (10 cells) within ±1.7%; 03-float steady 13.6 → 10.6 ms in Docker (−22%). Bench.run: 2016 → 1800 code
  bytes, stack-map words 31 → 16. rat cells move ±5% between the two seeds of one binary: the rat "+5%" of the
  0.6.6 A/B is layout. Fable's side question (a loop of caught float failures with no safepoint under
  --max-memory) has no source form: `catch error is Error` does not catch the RuntimeError (same on 0.6.6).

## Release 0.6.6 (release/066)

- [x] Release commit 0.6.6 (2026-11-27): version 0.6.6, CHANGELOG [0.6.6], fuzz/Cargo.lock brought up to the wasm
  dependencies (it lacked js-sys since #1124), Bible rebuilt (md/HTML/PDF, EN/RU); the owner confirmed the design's
  §9 answers («дада»). No release-wide micro A/B (per-change A/Bs in the CHANGELOG). Published with the release: the
  playground on the website (okkama-site playground/, rebuilt from the release commit).

- [x] #1127 workers (owner's decision 8; feature/1127w from release/066 9d42172fd, not committed; worktree
  okkama-work/hf-1127w, targets t-1127w and t-1127w-nd): `worker_emulated.rs` (replaces `worker_absent.rs`),
  `worker_wire.rs` (codec moved out of `worker.rs`), `Runtime::emulated_workers`, `State::WorkerBinding` without
  the host (`WorkerPoolRef`), `Loaded`/`FunctionInfo` Clone, `system_cpu_count` = 1 without the host (profile
  allows it), `permits_workers` without the profile refusal. Tests: `embedding_tests::workers_are_emulated_in_the_
  same_instance` (both builds), `one_processor_without_the_host`; playground_run
  `the_playground_emulates_workers_as_the_cli_runs_them` over `tests/playground/workers/` (6 programs, equal to
  the CLI's process workers); smoke: parallel map and worker output in wasm. sparrow_wasm_bg.wasm 3,846,641 bytes,
  1,887,848 gzip.
- [x] #1136 (feature/1136 from release/066 554a3f99c, not committed; worktree okkama-work/hf-1136, targets t-1136
  and t-1136-nd): `stdout::{with_tick, tick}`, `rt_poll` ticks; `Batching {Write, Window}`, `BATCH_BYTES`,
  `Receiver` (replaces `piece_sink`); `Limits::slice`/`Profile::slice`/`SLICE_PLAYGROUND` (= SLICE after
  measuring); sparrow-wasm `window` request field (`WINDOW_MS` 16), `version().profile.stopped` (`STOPPED`,
  wording awaiting the owner); tests in embedding_tests (poll hands over, stream and size bounds), stdout,
  sparrow-wasm; smoke: 6000 lines batched vs per write, the stopped worker prints `stopped`. Measurement in
  decisions.md. sparrow_wasm_bg.wasm 3,829,703 bytes, 1,881,648 gzip.
- [x] #1127 (parts without owner decisions; feature/1127 from release/066 acea75ebb, not committed; worktree
  okkama-work/hf-1127, targets t-1127 and t-1127-nd): `sparrow_core::run_tests_embedded`, `EmbeddedTests`,
  `TestSetup` (engine, stdin) threaded through `test_profile`, `piece_sink`; sparrow-wasm `test`/`test_streaming`
  (+ wasm `test(spb, request, sink?)`); `budget::tests::the_playground_offers_digests_macs_uuids_and_checksums`;
  playground_run: `the_playground_runs_tests_as_okkama_run_t` (38 suites: 13 Bible + 25 std, all equal),
  `the_playground_hashes_as_the_cli_does` (`tests/playground/digests/main.ok`); smoke.mjs: tests and digests in
  wasm. sparrow_wasm_bg.wasm 3,826,377 bytes, 1,879,943 gzip (+16.7 KB). CLDR on demand: done in #1125. Workers
  emulation (§9 #8) and the resumable scheduler (§9 #3) wait for the owner.
- [x] #1133 and #1128 (feature/1133 from release/066 d7e6c5d79, not committed; worktree okkama-work/hf-1133,
  target t-1133). #1133: `budget.rs` (`Reach::classify`, `PURE`, `SYSTEM_CONSTANTS`, PLAYGROUND `allowed` = stdin
  only) and `okkama-runtime-abi/src/catalogue.rs` (PURE/HOST lists, `effect`, counts 275/3/264); five family tests
  (`pure_family`), green in default and no-default builds with the companion test and the profile-consistency test;
  mutation (net back in Net) fails three tests. #1128: `ide::Answering` + `Inspection::analysed`, used by
  `lsp.rs` and okkama-wasm; unit test `answers_come_from_the_last_analysed_inspection` (mutation: record parsed
  fails it); `tests/playground.rs` completes `bottle.` after `std.coll` (failed before: the LSP offered nothing);
  `cli.rs` lsp_* green; Bible `build.py --check` without the replace: 249 examples, 0 failures (gate build of the tree).
- [x] #1134 + #1135 Playground streaming and reachable natives (feature/1134 from release/066 d7e6c5d79, not
  committed; worktree okkama-work/hf-1134, targets t-1134 and t-1134-nd): `Embedding::pieces`/`PieceSink`;
  sparrow-wasm `run(spb, request, sink?)`, `run_streaming`, `Decoder`, `version().profile.{interrupted,exhausted}`;
  `sparrow-core/src/reach.rs` (`functions`, `RUNTIME_CALLS`), `ProgramNatives {declared, reachable}`,
  `requirements` → `{refused, declared, files}`; playground_run streams and checks over examples/ that a refusal
  hit is named (315 programs, 47 stopped, all named); smoke.mjs: sink, stopped worker, texts, declared/refused
  (file program 8→4). sparrow_wasm_bg.wasm 3,773,197 bytes, 1,863,281 gzip (+8.3 KB; budget 2,000,000).
- [x] #1125 Playground runtime (feature/1125 from release/066, rebased on f5e49d602, not committed; worktree
  okkama-work/hf-1125, targets t-1125 and t-1125-nd): crate `sparrow-wasm` (run/requirements/provide/version/
  lastPanic, clocks from the page); `budget::Profile`/`PLAYGROUND`/`Limits::playground()`/`permits_workers`;
  `stdout::with_sink`; `run_embedded`, `program_natives`, `refused_natives`, `Heap::set_stdin`; `native/secure.rs`;
  runtime-core feature `cldr` + `provide_cldr`, `gunzip_checked`; `Uncaught`/`main_outcome` in runtime-core
  `failure.rs`; `okkama_compiler::commands::{compile, trace}`; okkama-wasm `build`/`builtSpb`/`failureText`;
  `okkama-cli/tests/playground_run.rs` (160 compared, 8 failing, 10 refused, all equal); `tools/playground/
  smoke.mjs`; CI wasm job and Wave `wasm` part. Size: sparrow_wasm_bg.wasm 3,749,216 bytes, 1,854,972 gzip (budget 2,000,000; okkama_wasm_bg.wasm 2,437,086 gzip). Owner's §9 #4, #6–#10
  provisional (decisions.md); no `--profile playground` flag.
- [x] #1122 follow-up (feature/1122b from release/066 9bbd43bce, not committed; worktree okkama-work/hf-1122b,
  target t-1122b): release/066 red after #1122. `Runtime::extend` loads an interpreter session's extension
  without the pointer-free proofs (3 session tests of the no-default suite; new
  `session_tests::a_session_on_the_interpreter_extends_as_on_the_jit`, fails with the fix removed); t/run.py
  `sparrow-interp-gc-stress` timeout ×10 (persistence/04 121 s locally on debug, CI ran out of 240 s).
- [x] #1130 and #1131 (feature/1130 from release/066 9bbd43bce, not committed; worktree okkama-work/hf-1130,
  target t-1130). #1130: `ops.rs` key walks on explicit stacks (`is_object_key`, `object_key`, `cmp_fields`,
  `owned_object_bytes`, `object_data_bytes`, `Fields` + `take_apart` for the drop; all out of line), tests
  `ops/key_depth_tests.rs` (100 000-deep key on a 1 MiB thread, default and no-default builds; 300 keys against
  the recursive definitions); mutations caught: compare order, no drop walk, recursive build (stack overflow).
  Micro A/B (toolchain.sh at seeds 1 and 2, release/066 9bbd43bce vs candidate, A1 B1 B2 A2, n = 25 000 and
  100 000, 5 runs, 14 map cells): round 1 +1.5% (map-long-key +4%, the new walks inlined into `cmp_other`); with
  them out of line, round 2 −0.6% geomean, slope −0.9%, every cell within ±7% and within the protocol's noise
  (map-miss +3–4% in both rounds, inside its band; worth a look in the release A/B). Toolchains and results in
  okkama-work/micro-tc/1130-{base,cand}{1,2}, 1130-results (round 2) and 1130-results-round1.
  #1131: i686-unknown-linux-musl build of sparrow-core's no-default suite (rust-lld, host target added with
  rustup), run in `linux/386` alpine containers by module filter and per integration file: every lib module and
  all 13 files pass after the fixes (`budget` 155 s emulated in the gate profile). Found: 1 GiB VM stacks exhaust
  the 32-bit address space (`STACK_BYTES` 128 MiB on 32-bit), `length_tests` 64-bit only, and a #1122 regression
  in the no-JIT build (a session on the interpreter refused every extension: pointer-free proofs). Wave `wasm`
  part runs the i686 suite on the runner (gate profile).
- [x] #1122 Interpreter 3 (feature/1122 from release/066 29d0bf49e, not committed; worktree okkama-work/hf-1122,
  target t-1122): `interp::resume` (registers in the Future's frame, `RESUME`, a step per resumption), await,
  await.take, region/resource settle and settle_unwind, `rt_complete`; `Loaded::without_pointer_free` +
  `Heap::relayout` for the interpreter; `Interp::depth`/`reentries` (`REENTRIES` 200 on wasm32); the #1122
  `#[ignore]`s of #1124 removed (26 tests; all pass on the no-default-features build); `tests/mnemonics/async.spa`,
  coverage test now all 142 mnemonics. §6.4 confirmed: the JIT recorded an async body twice after a suspended
  `region.settle_unwind`; fixed (`Emitter::check` passes the heap frame), ticket #1132. Also fixed: #1129 broke
  `jit::ready_callees_tests::lazy_compilation_failure…` (it dropped a memory budget between runs; now a large
  one). Tests: `interp::tests` 28, 9 mutations caught. Corpus (release, cage): t/ 142 identical on the JIT and
  both interp configs (no skips); examples 263 identical (differ.py now runs a directory's programs serially:
  examples/fs share files); bench/micro check 749/773 on the interpreter, the 24 cage failures the JIT's;
  conformance on the interpreter (okkama-dev) 1075/1075. Micro A/B async cells (toolchain.sh seed 1, A B B A,
  n=10⁵): geomean +0.3%, all within ±2.7%. The async exception line is gone; the Wave's sparrow-interp branch and
  the gate's interpreter step are blocking. Toolchains okkama-work/micro-tc/1122-{base,cand}.
- [x] #1129 (feature/1129 from release/066 cd4548694, not committed; worktree okkama-work/hf-1129, target t-1129):
  `Budget::polled` fixed in `start_budget`, `Runtime::polled()` read by `jit::compile`, `interp::step`,
  `intrinsics::sort_natural`; refusal text `polling_fixed`; test `interp::tests::the_polling_of_a_runtime_is_fixed…`
  (three budget pairs, JIT = interpreter; 2 mutations caught); sparrow-core `--test budget` and `runtime::` green.
  JIT code unchanged when budgets do not change between runs (no A/B). `bench/micro/toolchain.sh`: fetch step
  (exit 3) and build step (exit 4); end-to-end build checked.
- [x] #1123 Interpreter 4 (feature/1123 from release/066 32f74e98b, not committed; worktree okkama-work/hf-1123,
  target t-1123): `okkama-rs/tools/engines/{differ.py, interp-exceptions.txt, test_differ.py}`; t/run.py configs
  `sparrow-interp`, `sparrow-interp-gc-stress` (×4 timeout, exceptions → "skipped" with reason); wave.yml
  conformance matrix `[sparrow, sparrow-interp]` (interp `continue-on-error`, `--adapter-env
  SPARROW_CORE_INTERPRET=1 --timeout-scale 4`, `differ.py ocs`, corpora with the interp configs and `differ.py
  run examples`; actionlint clean, not yet run on GitHub); gate.sh step "interpreter" (informative); okgen
  `--engines sparrow,sparrow-interp`; mutants.sh JIT mutant `sparrow-jit` (+ `MUTANTS_ONLY/TARGET/OUT`); fuzz
  exec `OKKAMA_FUZZ_DIFFERENTIAL=1` (`linking::differ`, test in okkama-compiler `fuzz_lowering`). Local runs:
  conformance on the interpreter (okkama-dev, gate-like) 986/1075 PASS, 89 async refusals explained, 0
  unexplained; t/ 142 files on sparrow + both interp configs: all ok, 3 async skipped (213 s, cage); differ.py
  examples: 211 same, 52 async excepted, 0 differ; okgen 30 seeds clean: 0 disagreements; mutants.sh
  sparrow-jit: 8/30 seeds disagree, minimized to 1 statement. Found by the fuzz differential test: a deep
  recursion stops on steps under the JIT and on memory under the interpreter (§6.3, `either:` exception).
  Docker volumes okkama-gate-target-hf-1123{,-mutants}; results in okkama-work/ocs-1123.
- [x] #1121 Interpreter 2 (feature/1121 from release/066 c828a2d62, not committed; worktree okkama-work/hf-1121,
  target t-1121): handlers and `catch` in `interp.rs` (`unwind` returns whether a handler caught; `fail!` in the
  loop resumes there; `jump_then!` shared by catch and the lookups); `sparrow_core::{Engine, Observed, observe}`,
  `Runtime::use_engine`; okkama-runtime-core `stdout::{capture_streams, write_error}` (natives.rs stderr goes
  through it); `okkama-cli/tests/differential.rs` (t/ 49, 16 examples, conformance run failures 58 that build,
  step parity on 3 programs × 5 budgets); `sparrow-core/tests/mnemonics/*.spa` (6 programs) and
  `interp::tests::every_synchronous_mnemonic_runs_on_both_engines` (dynamic coverage, test-only recorder in the
  loop; async mnemonics and `fut.spawn` wait for #1122). Tests: `interp::tests` 24, differential 4,
  `stdout::tests::capture_streams…` 1; 11 mutations caught. Corpus (release, cage, JIT/interp/interp+GC):
  t/ 139/142, examples 211/263, conformance run failures 55/58 same, the rest async refusals, 0 differ;
  bench/micro check 621/773 on the interpreter (128 async, 24 cage-only on both). JIT path untouched (no A/B).
- [x] #1124 Interpreter 5 (feature/1124 from release/066 c828a2d62, not committed; worktree okkama-work/hf-1124,
  target t-1124; baseline worktree okkama-work/hf-1124-base + target t-1124-base, both to remove): sparrow-core
  features `jit` and `host` (default), okkama-runtime-core `native` = `providers + host + zstd + brotli +
  crypto`, okkama-native `aws-lc`; `clock.rs` (std::time natively, virtual timeline on wasm32: `install`,
  `skip_to`), `io.rs` split (portable part + `wait_tokio` / `wait_sleeping` / `skipping`), `worker_absent.rs`,
  stoppable merge sort under `panic = "abort"`, budget math in i64; decisions.md #1124 has every gate and the
  recursion audit. `cargo check -p sparrow-core --no-default-features --target wasm32-unknown-unknown` clean (no
  tokio, cranelift, libc, aws-lc, rusqlite, zstd, brotli in its tree: 130 crates); `cargo check` clean (lib and
  tests) for default, none, `jit`, `host`; runtime-core providers/providers+crypto/host/zstd/default clean;
  okkama-compiler + okkama-wasm wasm32 still clean. Tests without defaults, run locally by module filter: lib, every module
  (all modules; 7 async tests ignored #1122), integration all 13 files (19 ignored #1121/#1122, compile limits
  and ready_callees JIT-only); `timer_wait` on the Tokio and the sleeping wait (`--features jit`); mutations
  caught: no interrupt look in the sleeping wait, waker not notifying, unstable merge. Default build: `cargo tree
  -e features` of okkama-cli/sparrow-cli identical for every external crate (only the three crates' new feature
  names); corpus A/B of gate builds (release/066 c828a2d62 vs candidate, cage, scratchpad compare.py): t/ 142,
  examples 333, std -t 125 (every 4th module), conformance 1075 cases: exit, stdout, stderr identical, 0 flaky;
  SPB of 2121 files identical. Wave part `wasm` and a CI "Playground wasm" step added, not yet run on GitHub
  (clippy of the no-default combinations is first checked there). Cargo.lock gains js-sys (getrandom `wasm_js`
  on wasm32 only).
- [x] #1120 Interpreter 1 (feature/1120 from release/066 39a627b67, not committed; worktree okkama-work/hf-1120,
  target t-1120): `sparrow-core/src/interp.rs` behind the seam `Runtime::invoke`/`Runtime::resume` (runtime.rs;
  the six entries of tasks.rs, intrinsics.rs, failures.rs ×2, runtime.rs ×2 go through it) and the root branch in
  `Runtime::safepoint`; `SPARROW_CORE_INTERPRET` read in `Runtime::assemble` through `env_switch`; devtrace
  category `interp`. 141 instructions: 74 `generic` → `rt_exec`, 12 `failure_or_region` → `rt_failure_op`, 48 own
  branches (142 / 75 / 55 since #1122, #1145), 7 refused (catch, 6 async). Moved out of jit.rs: `program::{generic, failure_or_region, suspends,
  dispatch_table, role_slot_is_async}`, `texts.rs`. Tests: `interp::tests` (19, in-process JIT/interpreter/GC
  stress comparison `engines`), 21 mutations of interp.rs/runtime.rs each caught. Corpus (release build, cage):
  t/ base+op+lang+gen 135 same / 7 refused / 0 differ, under GC stress the same (persistence/06 needs 105 s vs
  31 s); examples 133 same / 130 refused / 0 differ; SparrowBook examples and sparrow-core/sparrow-cli .spa: all
  without handlers/async identical; bench/micro check.py 560/773 pass on the interpreter (61 handler and 128
  async refusals, 24 cage failures shared with the JIT). Switch off: t/ 142 and examples 263 byte-identical to
  release/066, micro check failures the same set. Micro A/B, JIT path (okkama-dev, toolchain.sh builds of
  release/066 39a627b67 and the candidate at seeds 1 and 2, n=10⁵, 5 runs, A B B A per seed, plus a 25k/100k
  slope round on seed 1): 12 cells (map-index/-replace/-plus/-miss/-literal, string-concat/-length/-split,
  byte, byte-search, closure-call, recursion), geomean −0.2%, every cell within ±4% (string-length +5.5% on
  seed 1, +0.3% on seed 2: layout). Interpreter/JIT, marginal ns per iteration (slope 20k→100k, seed 1):
  baseline ×8.4, int-small ×8.5, float ×6.2, recursion ×19.2, method-call ×16.9, closure-call ×12.7, role-call
  ×16.4, array-index ×8.9, map-index ×2.0, string-concat ×2.1, string-length ×6.3; geomean ×7.9 (design target
  ≤10×). Toolchains okkama-work/micro-tc/1120-{base,cand}{,2}, results okkama-work/micro-tc/1120-results.
- [x] Playground follow-ups #1115–#1119 (feature/pg2 from release/066 342cc7322, not committed; worktree
  okkama-work/hf-pg2, target t-pg2): `std.…` completion over `ide::StdTree` (DiskStd for the LSP, BundledStd in
  okkama-wasm), parity with `okkama lsp` in tests/playground.rs (#1115); `compiler/src/host.rs` (Clock,
  dev_output) and `ModuleLoader::identity`, devtrace inert on wasm32, grep test `host::tests` (#1116); one caret
  in a successor's completion detail (#1117); `lastPanic()` and `version().passes` (#1118); imported files named
  relative to the current directory (`source::display_name`), LSP relay from the importer's directory, parity
  test on root-relative paths with imported diagnostics (#1119). No expected outputs changed (Bible, t/,
  conformance); SPB unchanged by the naming (checked on 19-shop from two directories).
- [ ] The playground page is built on the website (repo okkama-site, `playground/`), committed locally as
  e913d3f, not yet published; rebuilt per release with `playground/tools/build.sh CHECKOUT` (wasm-bindgen
  0.2.130 in okkama-work/tools-wasm). Uncommitted there: worker.js reads `lastPanic` after a trap, playground.js
  takes the passes from `version().passes` and maps the `folder` kind; `pkg/` rebuilt from hf-pg2.

- [x] #1078, Rust side (feature/1078, not committed; worktree okkama-work/hf-1078, target t-1078): the compiler
  builds for `wasm32-unknown-unknown` (`cargo check -p okkama-compiler --target wasm32-unknown-unknown`).
  okkama-runtime-core feature `native` (default) gates `native`, `host`, `data` and their dependencies; the
  compiler depends on it without default features. `okkama_compiler::commands` renders what check, fmt,
  tokens, ast, ir and build report (`Finding`, `Report`), used by okkama-cli (main.rs, doctest.rs gate, ocs.rs)
  and by the new crate `okkama-wasm` (cdylib + rlib; `Workspace` = loader over the embedded std and the page's
  files; JSON API, decisions.md #1078). Parity: `okkama-cli/tests/playground.rs` (130 files: t/ 51, Bible
  rejected 69, std 6, fixtures `tests/playground/` 4): check, fmt, ir (all passes and `--after fold`) identical,
  spa = disassembly of the CLI's SPB; eleven mutations of the wasm side (loader order for std and non-std
  files, import dirs, self-import, test-mode imports, profiles, stderr trim, fmt newline, warning code, `after`
  ignored, build profile) each fail it; two of commands.rs (merge dedup, errors-only) fail its unit tests. Corpus before/after (release builds, release/066 994aa8603 vs the candidate): SPB of 1549 built
  programs byte-identical (std -t 499, t/ 51, examples 264, okkama-rs/examples 13, Bible 181, conformance 541),
  check and fmt output identical on all 2220 files, tokens/ast/ir (8 variants) identical on 420 files. Release
  wasm 10.1 MB; after `wasm-bindgen --target web --remove-name-section`: 9.6 MB, gzip 2.40 MB, brotli 1.74 MB
  (std text 5.97 MB of it, code 3.5 MB). Node smoke test of the nodejs build: every export answers; check of
  std/http/server 0.33 s. wasm-bindgen-cli 0.2.130 installed into okkama-work/tools-wasm (not on PATH). Not yet:
  the site page, the CI job (proposed in the report), `std.…` path completion in the playground.

- [x] Release commit 0.6.5 (2026-11-27): version 0.6.5, CHANGELOG [0.6.5] with summary and compare links, Bible
  rebuilt (md/HTML/PDF, EN/RU), micro report `okkama-docs/benchmarks/reports/2026-11-27-v0.6.5/micro.html`
  (rc1 fa572a909 vs v0.6.4, −1.0% over 75, both built by toolchain.sh at seeds 1 and 2; the final A/B on
  026f0ea13 was cancelled by the owner so the performance audit could use the machine). First release through
  the trunk-based flow: release/065 → develop (nightly gate) → pull request into main → tag on the merge commit.
  Performance audit (Fable) in `okkama-docs/runtime/perf-audit-2026-11-27/` — ten tickets proposed for after 0.6.5.
- [x] #1102 (feature/1099 rebased on release/065 6e13e1d1e, not committed): a discarded Future of an ordinary async
  call runs nothing (lazy, Bible §20.1) and its scope releases the fresh arguments it owns (#587). A bound async
  action's (`call.fn`) Future was never tracked, so its argument leaked (`destroy 12`/`13` never ran). `emit`
  now tracks it, and `futures::awaited_temporary` takes `call.fn` (driver unless its `fn.closure` is in the function
  and names a non-driver). Corpus SPB vs #1099: 34 of 2279 differ (std net/mail/http/web/fs, their examples, 9 web
  micro programs). Docker std -t of the 12 changed modules: same pass/fail on both (web/routing and fs/root have
  1 failure each on both). Micro A/B (n=10⁴, 5 runs, quiet round A3 B3 B4 A4): web cells within −2.7…+2.6%
  (session-lifecycle +2.6% the largest); the first round's A2 was under host load (Spotlight). Toolchains:
  okkama-work/micro-tc/1102-cand against 1099-cand (= release/065 code).
- [x] #1100/#1101 (feature/1100, not committed): `wave.yml` part `debug` and `ci.yml` job `debug` ("Tests
  (debug)"): `CARGO_PROFILE_DEV_DEBUG=line-tables-only cargo test --locked --no-fail-fast -p okkama-compiler -p
  okkama-cli`, caches `wave-debug`/`tests-debug`; not yet run on GitHub (first run: time, disk, the linearity
  check's refusals). `okkama-cli/tests/tree/mod.rs` (`okkama()`, `OKKAMA`, `std_dir()`, `root()`) is the one way
  tests start okkama; `cli.rs` `tests_run_okkama_with_the_std_of_this_tree` guards it. Bible §19 lock section
  (EN/RU) has the `in blade.lock` line.

- [x] #1099 (feature/1099, not committed): `BodyLowerer::emit` tracks `call.fn`/`call.cap` results (not Futures);
  `lowering::untrack_borrowed_results` drops the track where the census reports no creation; the census is
  computed once and passed to `statics::make_static(module, census)`; skipped for `okkama check`. Release
  compile of std/http/server.ok -t: 460 ms on both (a second census cost +80 ms before sharing). Corpus SPB vs
  release/065 (2279 programs that build): 14 differ, all HTTP/web, 34 new tracks, every one after
  `ClientResponse::Exchange.history` (an `@ExchangeHop` field wrapped in a new `Sequencing` adapter per read).
  bench/micro check.py on 99 closure/cap programs: same as release/065 (10 cage-only fs/process failures).
  Micro A/B (okkama-dev, toolchain.sh builds of 57da2e6c2 and the candidate, n=10⁴, 5 runs, A1 B1 B2 A2), the
  two programs whose SPB changed: http-cache-response 37018/37369 → 36883/36732 ns, http-connection-policies
  98698/98955 → 98735/99769 ns: within noise. Toolchains in okkama-work/micro-tc/1099-{base,cand}. Tests: tweed_ownership (4, sync+async), statics
  `closure_and_cap_results_are_tracked_only_when_they_are_new`; #1093's census test now also drops the new track.
- [x] #1098 (feature/1098, not committed): `lowering/natives.rs::check_abi`, called from `lower_native`, refuses
  a native whose canonical symbol is in the ABI table but whose parameters, result or asynchrony differ
  (semantic error at the declaration); unknown symbols stay the runtime's. `okkama-runtime-abi`
  `Native::signature_text` is public for the message. Sweeps: 16656 std/runtime native mutants (14751 codegen
  failures without the check, 0 with it); fuzz run 45896662080's exec + analyze corpora (5581) linked against
  std: 0 codegen failures.
- [x] #1094/#1095/#1096/#1097 (feature/book, not committed): `okkama-devtrace/src/env_switch.rs` is the one
  switch implementation (pub; re-exported as `okkama_compiler::env_switch`, `sparrow_core::env_switch`; a `use` in
  both CLIs; a dev-dependency of okkama-runtime-core); the five copies are gone. devtrace `requested`: a blank
  `OKKAMA_DEV` category list is off. Default-on pins: `okkama-cli/tests/env_switches.rs` (`OKKAMA_TRACE` under
  run/exec), `sparrow-cli/tests/cli.rs`, `sparrow-core/src/program.rs` `switch_tests` (POINTER_FREE, INT_RANGE).
  Blade `report(root, installed, locked, new)` adds `… in blade.lock` lines for lock changes `pkg/` already has.
  SPB verifier: "a rotation needs a count"; `analysis_bytes` prints the computed bytes. `run_executable` returns a
  failed `configured_runtime` as `ExecutionError::Load`, `core_described` prefixes run_described's load error
  with `okkama:`.

- [x] #1093 (feature/dbg, not committed): `ir/linearity.rs` takes a call result as a resource only where the
  census reports a creation site (`Calls::of` runs `ownership::analyze` once per module, only when some function
  has a resource-typed candidate; resolution shared through `ownership::resolved_call`); a call whose callee
  adopts the parameter owns the fresh argument, also on its unwind edge (`Calls::adopted`); a branch moves an
  argument's ownership state into a resource-typed block parameter (`flow`); the dead `await.take` owner branch
  is gone. `ir/statics.rs` removes the regions it empties (`regions::elide_among`): with `OKKAMA_IR_PASSES=statics`
  37 corpus programs were refused "may still be open" before (a `resource.leave` before `region.leave R` failed
  to the outer handler; empty region, no leak; since #1058 ran the pass alone). `LoweringError::Internal`
  messages are bare, Display adds the prefix (CLI, `okkama check`, doctests print the kind once). #1073's two
  refusal tests now also make `Res.read` borrow its receiver (an adopting callee owns the argument). Corpus with
  `OKKAMA_VERIFY_OWNERSHIP=1`, release build, passes `all`/`none`/`statics`: std -t 499, t/ 49 + t/gen 93,
  examples 264 (69 intentional rejects), conformance 601 compiling cases (474 expected rejects): 0 refusals, 0
  ICE; SPB under `all` byte-identical to release/065 on 1382 files. Found: #1099 (a discarded `call.fn`/`call.cap`
  result that is a new resource is never destroyed; the check refuses it). Not run: the debug workspace suite.
- [x] #1090 + #1091 + #1092 (feature/fuzz2, not committed): `contracts.rs::thrown` checks that an error case's
  fields are exactly the raised payload (the legacy `$tag` layout, unemitted since #290, used to load; a
  crafted one read `x` as the tag, 0); an intrinsic's contract errors name the intrinsic; `intrinsics::raised`
  and runtime-core `raised_fields` removed. Fuzz `exec` links against the repository's std
  (`support/linking.rs`, `StdTree`, one parse per module and process, plain names only) and shares `run`
  with `tests/fuzz_lowering.rs`. `tools/test-python.sh` (9 test_*.py, ~17 s) in CI job "Tool tests" and the
  Wave's maps job. Tests: okkama-cli `--test intrinsic_contracts`, okkama-compiler `--test fuzz_lowering`.
  Not yet seen on Linux: the Python tests and the exec target with std (Fuzz dispatch).
- [x] #1088 + #1089 + #1050 (feature/fuzz, not committed; fuzz run 45890980218 on 08a63b30b):
  `lowering/intrinsics.rs` `Site::refuse` makes every shape the runtime needs a semantic error at the intrinsic
  (error enum and its first case, result object with the receiver's type-parameter count, helper field, cap field
  and action); `core::ArraySequence` missing stays internal. Swept 653 mutants of the eleven intrinsic modules
  (production and test mode): no internal error. Fuzz `exec` lowers through `support/linking.rs`
  (`lower_linked`, a loader with no modules) instead of unlinked `lower_program`. `tools/fuzz/coverage.sh`
  replays coverage with `detect_odr_violation=0` (cargo-fuzz run's default; SQLite FTS3/FTS5 `aiOff` false
  positive). Crash inputs in `okkama-rs/fuzz/regressions/<target>/`, copied into the corpus by `run.sh`,
  replayed by `tests/fuzz_lowering.rs`. Not verified on CI yet: a Fuzz dispatch on the pushed branch must
  show `coverage/spb.txt` and `coverage/exec.txt` with TOTAL.
- [x] #1086: `bench/micro/toolchain.sh WORKTREE OUT [LAYOUT_SEED]` (release cfg, rust-lld `--shuffle-sections=*=SEED`
  via rustc's gcc-ld wrapper; refuses seed 0 = random and a devtrace binary); `report.py` `binary_spread` per cell
  (per-binary geometric medians by `toolchain.binary_sha256`), band = 2 × max(series spreads, binary spreads),
  devtrace campaigns incompatible. Seeds 1 and 2 of 6d84de2fd: different binaries and symbol order, same checksums.
- [x] #1085: `bench/micro/support/harness.ok` — `Measurement.elapsed_since` holds the closing clock read; `start()`
  runs it once before the start instant. Constant in the interval (slope over N=25k/100k, 3 interleaved rounds,
  rc 6d84de2fd): baseline no-inline 1.25 → 0.38 ms, default 0.025 → 0.002; async-await default 1.40 → 0.51,
  no-inline 1.36 → 0.46. A quiet warm call of `finish` itself measured worse (0.76/0.87 ms) and was dropped.
  The async rest (~0.5 ms with inlining) is outside the harness.
- [x] #933 (feature/933, not committed): `code_memory` places every function on a page of its own at
  `shift` bytes (`CodeMemory::new(shift)`, `Region::written`, skipped pages untouched); `Jit::new` reads
  `SPARROW_CORE_CODE_SHIFT` through `env_switch::bytes` (refuses a malformed value), `Jit::with_code_shift`;
  devtrace event `jit` "code placed". `bench/micro/run.py --placements` (every placement in every round,
  `layout`/`layout_spread` per cell, placements part of compatibility), report.py shows the layout spread.
  Numbers (okkama-dev, n=10⁶, 5 runs): release/065 — moving JIT code (base +4/8/12/16/20/36 KiB, shift
  64/128/1024/2048, 64 KiB granule) keeps array-plus 58–59, float 8.8, map-replace 376–385 ns; v0.6.1 —
  array-plus 75.7–79.1 ns on every JIT placement vs 60 with `NO_INLINE=main`, unchanged by heap page
  alignment, malloc padding and stack shifts, gone after an unrelated Rust edit (62.9/63.8): the AOT binary's
  layout. `NO_INLINE=main` adds ~1.4 ms of lazy compile (`Measurement.finish`) to the timed interval. No PMU
  in the Docker VM. Proposed tickets: harness pre-compile of the finish path; link-order perturbation
  (lld `--shuffle-sections`) for the AOT layout lottery.
- [x] #1072 (feature/1072): `Emitter::instruction` (sparrow-core/src/jit.rs) compiles `unreachable` as a trap
  (`jit::UNREACHABLE`, "Unreachable code reached", a program error a handler catches); SparrowBook table text.
  No corpus SPB holds opcode 145 (1245 programs of std -t, examples, bench/micro; 552 of t/ and conformance).
- [x] #1074 (feature/1074): `Jit::spread` (sparrow-core/src/jit.rs), one Cranelift adapter per arity over 8,
  called by `jit::invoke`; `rt_call_closure` takes a vector past 8 words; `Invocation::capture_count` has no
  width limit; `closure_call` calls wide closures directly. No corpus program has a closure over more than 8
  parameters or a `call.fn` with 7+ arguments, so no emitted code changes; the closure helper's path for
  async and 3+ captures runs as before: std `run -t` 395 modules, examples 75, bench/micro check 773, t/ 142
  files (50972 checks) identical to release/065 (0269ef0e7) in the cage.
- [x] #1076 (feature/1076): `Program::stub_fields` (okkama-sparrow/src/verify.rs), `Loaded::may_hold_stub`
  (sparrow-core/src/program.rs), `Emitter::bound_field` for `obj.get`/`variant.get` (a #541 accessor of such a
  field stays a call); a `bool` stub reads `true` (`$pending`, #271), any other stub fails the program; map keys
  and the debug natives refuse or read the stub as unbound; a session runtime checks every field. Checked
  reads: 55 of 180696 field reads in bench/micro (2 of 774 programs), 407/58854 in examples, 203/97604 in std
  tests; load cost 10 ms over 1245 programs (0.7% of their verification). Blast radius vs release/065
  (0269ef0e7): conformance full-release 1075/1075 on both; std `run -t` 395 modules, examples 75 identical
  (cage and Docker); Bible examples, bench/micro check, t/ identical.
- [x] #1077 (feature/1077): closure captures in first-use order. `Checker::outer_view` gives a nested
  anonymous action the enclosing body's locals plus the names it has not captured yet (`Outer::inherited`);
  a use records a pending capture, which `Checker::resolve_captures` turns into a capture of the enclosing
  body after the nested one is checked. Before, `visible()` captured every outer name in HashMap order, so
  nested closures over-captured and their SPB varied run to run. Compile sweep (2382 files, std -t, t/ with
  t/gen, examples, conformance, Bible examples): exit codes as release/065; SPB differs in 22 files, all with
  nested closures (16 were already nondeterministic on release/065), their runs print the same; three branch
  sweeps byte-identical.
- [x] #1025 (no code): the 0.6.4 async slowdown is not a regression. The rc2 micro toolchain
  (`okkama-work/micro-tc/v0.6.4`, okkama b80691b5…, measured in r064c/d) was built without
  `--cfg okkama_release`, so devtrace is compiled in (`OKKAMA_DEV=runtime` writes `counters.json`; a
  release build writes nothing): its gates in `Runtime::future_new`/`run_body` (span + count),
  `rt_future_new`/`rt_await_take`/`rt_complete` and `Heap::allocate`/`allocate_class` run on every
  iteration. Same source 29c3dc84a, okkama-dev, marginal ns/iteration (slope between N=25 000 and 10⁵,
  5 interleaved rounds, two seeds): release cfg vs devtrace in — async-await 42.07/42.23 vs 44.09/44.31
  (+4.8%), spawn +2.5/+1.4%, event-wake +2.7/+2.0%, cancel +2.8/+2.2%; the rc2 binary itself +3.0…+4.1%
  on async-await, also when it runs SPB built by 7fb98e01f. 7fb98e01f → 29c3dc84a, both release cfg:
  async-await −0.6…+1.0%, spawn −1.3…+3.2% over six runs; #1009/#1011/#1013 points (44a4b151b, d1d2b17c5,
  486af1b02, 94eba7fb9) within −0.4…+1.6% on async-await. The hot loop takes no `rt_exec` (stats: helpers
  future_new, await_take, complete only), so #1009's `catch_unwind` is not on it. JIT placement spread on
  release/065 (`SPARROW_CORE_CODE_SHIFT` 0/64/1024): 1.1% async-await, 1.0% spawn. At N ≤ 10⁵ the timed
  interval carries a fixed 1.2–1.5 ms (#1085), which follows the SPB, so medians at one N are biased; compare
  slopes. Shipped 0.6.4 (packages.yml, ~/.local) has devtrace out.
- [x] #1071 (feature/1071): `ir::verify` checks cap declarations (`check_cap`: owner exists, distinct field
  names, each field a synchronous `[method Owner]` function of the owner alone returning a value),
  `worker.bind` (entry `fn(field types…, action params…)` with the action's result) and `worker.spawn` (the
  entry does not return a Future). Not reachable from lowering; compile sweep as release/065.
- [x] #1073 (feature/1073): `ir/linearity.rs` refuses a resource (object with its own destroy made by `obj.new`,
  a call or `await.take`) that may be in no region (track, adopt, transfer, pin; `keep` frees) when the function
  leaves, other than the value returned or thrown, or made again by a loop while the one before is free; and a
  `region.enter` while the region may be open, at that enter. Header: debug builds and `OKKAMA_VERIFY_OWNERSHIP=1`
  in any build but a packaged release. Its find: `exit_scopes` left nested regions with `keep` and no track in
  between, so a failing outer defer or close lost a returned resource; each next region now tracks it
  (`lowering/body/scopes.rs`). Compile sweep (2382 files) with `OKKAMA_VERIFY_OWNERSHIP=1`, passes on and off:
  exit codes as release/065; SPB differs in 78 files beyond #1077's nondeterminism (std http/db/net/web/mail,
  conformance L07/L11/L12/L13, examples), all from the added tracks.
- [x] #1075 + #1070 (feature/1075-1070): the hoist driver comment names the nearest outside dominator (#239);
  IR docs: `char` compare takes every operator, `float`/`rat` `%` keeps the dividend's sign, `rat.part` is
  `int`, `Move` lists `future.handoff`; verifier: a rotation without a count gets its own message; the
  linearity check names plain OPEN "open"; dead `IntBinaryOp::traps()` and IR `TrapCode::Unreachable`
  removed (SPB's `TrapCode::Unreachable` stays). The `map.keys` operand already prints once ("of non-map
  type", since #1043/#1044).
- [x] #1064 (feature/1064): `ir::Role::slot_name` returns `Option`; the printer prints `<invalid>` for a slot
  the role lacks (`call.role`, `does`), the verifier's messages fall back to `#N`.
- [x] #1063 (feature/1063): `ir::verify` `check_target_with` relaxes `^C`/`C` only on `unwind` targets
  (catch handler and lookup `found` edges are normal paths). Compile sweep (3170 files, passes on; 2122 with
  OKKAMA_IR_PASSES=none): SPB identical to release/065 but the nondeterministic `L03.anonymous.lexical-scope`.
- [x] #1039 (feature/1039): `blade new` (blade/src/lib.rs `new_project`) refuses an existing NAME, a missing or
  extra argument and an invalid package name; creates with `create_dir`/`File::create_new`; output from
  `project::default_output` (shared with `Project::parse`); no `bin/`.
- [x] #1060 (feature/1060): okkama-cli tests describe one engine: docs of sparrow_scalars/variants/strings/
  objects/native/providers, traces, budgets, signals, strings, terminal_pty, jit_accessors; one-element engine
  loops and `{ let engine = "sparrow"; … }` blocks collapsed (signals `engines()`, budgets `run(engine, …)`,
  terminal_pty, cli.rs); `both()` → `expect()`; the pty driver takes no engine; cli.rs no longer sets
  `OCS_RUST_ENGINE` (engine_selection.rs keeps the test that stale engine variables are ignored).
- [x] #1040 (feature/1040): `blade run` rebuilds by `OUTPUT.blade-stamp` (blade/src/project.rs `inputs`,
  `fingerprint`, `stamp`, `up_to_date`; SHA-256 through `install::checksum_of`), never by mtime; lock spells
  `path` sources from the project (`Source::Path { dir, spelled }`, `install::relative`); one `report` for
  sync/update/add/remove against `pkg/.blade-installed` (files changed, source moved); sync reports
  "no longer required" only after a complete walk; `settled` names a busy blade; `blade install`,
  `add_source`, `Cli::execute` and the commented-out code gone. Not covered by the stamp: the okkama binary and
  its std (a new okkama with the same SPB version reuses the output) — covered since #1080.
- [x] #1080 (feature/1080, not committed): the stamp records the toolchain (`blade-stamp 2` header, then the
  `okkama env` version line, `binary sha256:…`, `std sha256:…`, inputs, output; blade/src/toolchain.rs
  `Probe::start`/`finish`, `Digests`, `Key`, `list`; project.rs `stamp`, `up_to_date`, `STAMP_FORMAT`, `CACHE`).
  Per-file digest cache `OUTPUT.blade-cache` (size, mtime, dev, inode, ctime; entries younger than 2 s not
  remembered). No-op `blade run` 8.4 → 12.3 ms (gate build, ~/.local okkama 0.6.4, 22.6 MB binary + 499 std
  files), ~3.7 ms of it `okkama env` start; uncached 41 ms. Not covered: `OKKAMA_IMPORT_DIRS` trees and
  compile-affecting environment (`OKKAMA_IR_PASSES` …) — covered since #1087.
- [x] #1087 (feature/1087, not committed): `okkama env` names `import    DIR` (okkama-cli `import_dirs`, shared
  with `FileLoader::from_env`) and `build     NAME="VALUE"` for set `lowering::BUILD_ENVIRONMENT`
  (`OKKAMA_IR_PASSES`, `OKKAMA_OWNERSHIP_PROTOTYPE`); guard `build_environment_tests` classifies every
  `"OKKAMA_…"` literal of okkama-compiler (build vs `NOT_BUILD`). Blade toolchain.rs: `Toolchain.imports/build`
  (stamp lines `import sha256:…`, `build …` after `std`), `Located`, `parse_env`, `Probe::start(okkama, root,
  cache)` (env runs in the root; relative paths joined to it), cache `blade-cache 2` with `tree` lines (every
  tree listed speculatively). No-op `blade run` without imports unchanged (14.4 vs 14.4 ms under load 10); with a
  499-file import tree +1.6 ms. Not covered (runtime-only, by decision): `OKKAMA_ARRAYS`, `OKKAMA_MAPS`,
  `OKKAMA_ICU_LIBDIR`, `OKKAMA_TRACE`, devtrace.
- [x] #1032 (feature/1032): `Name<` trial parse rejects a type closed by the first half of a shift
  (`Parser::at_split_shift`) and undoes its token splits on backtracking (`splits`, `unsplit_since`). Compile
  sweep over std, t/, examples, conformance, Bible examples, bench/micro (3170 files): SPB byte-identical to
  release/065 except `L03.anonymous.lexical-scope`, whose closure capture order differs run to run on both
  binaries (codegen nondeterminism, ticket to file).
- [x] #1069 (feature/1069): the SPB verifier tracks `undef` stubs (`State` in okkama-sparrow/src/verify.rs):
  carried by moves, edge moves and `obj.new`/`obj.init` fields, read only on the `true` side of a `br` on a
  guard that is false where the stub may remain. Compile sweep over std, t/, examples, conformance, Bible
  examples, bench/micro (3170 files): SPB identical to release/065 but the nondeterministic
  `L03.anonymous.lexical-scope` (closure capture order varies run to run). Open: a stub stored in a field and read by
  `obj.get` (heap) is not tracked (ticket to file).
- [x] #1043 (feature/065-ir): `codegen::sparrow::check` refuses a result or an edge value in an operand's
  register (`early_write`); `TermKind` targets/operands from the macros `terminator_targets`/
  `terminator_operands`; IR text `oir 2 high`, OIR-2 naming; tests for #1007 (`FutureGroup` not plain in
  `param_adopts`, the linearity check's refusal of region block parameters, a catch value's register).
- [x] #1044 (feature/065-ir): `FnBuilder::adopt_tracked_await` (lowering/builder.rs, called from
  `BodyLowerer::adopt`) and `take_unique` (futures.rs): one adopt for an awaited result; `okkama ir --passes`
  refuses unknown names (`lowering::unknown_passes`); the warning names its origin; the conversion gap message
  is "`char` to `float` conversion". Compile sweep of 2382 files (std -t, t/ with t/gen, examples,
  conformance, Bible examples) against release/065: exit codes identical; SPB differs in 90 files with the passes
  on (1106 `region.track` fewer, no other opcode) and 102 with them off (1230); the rest are run-to-run closure
  order (`L03.anonymous.lexical-scope`, t/lang/closures, t/gen nesting), same opcodes. Not done: #1043's
  `codegen::Backend::from_name` without a caller; call results bound by `let` keep track then adopt.
- [x] #1033 (feature/1033): `heap::passes_length_limit` is the one pre-check of a header length;
  `builder_finish` and `int_text` return `Outcome`, so `rt_builder_finish`/`rt_int_text` fail the
  program (Fails effect; JIT checks the status after `int_text` too). Direct helpers with
  bounded sizes (`rt_array`, `rt_closure`: operand counts) need none; natives and `rt_exec`
  catch panics.

## Formatter, IDE, CLI and IDNA audit (2026-11-26, feature/065-ide)

- [x] #1036 point 1: only a bare lsp/repl starts the interactive command.
  `--help` alone uses the existing help page and exits 0; every other argument
  (including additional help flags) is named in a usage error, exit 2.
  Points 2–4 were already fixed in the documentation lane.
- [x] #1037: bodiless signatures retain their post-semicolon comment when
  header comments are pending; brackets inside comments do not delimit lists;
  object/enum/const name comments and destroy header comments keep their place.
  Trailing comments cannot be stolen across another construct on the same line.
  The #1006 block-comment/header and line-comment/body rules remain covered.
- [x] #1041: action result types in IDE completion/signatures use the same
  root-module-qualified names as member-table keys, including role actions.
  Tests cover both same-named objects, role receivers and repeated calls.
- [x] #1042: self is completed once in object scope and never in main;
  constant-use spans cover only the identifier; misplaced/stale doc comments
  are corrected. Explicit and implicit synopsis extraction skips runnable
  doctests (comma or whitespace metadata) and output fences, preserves examples,
  and removes an indented block's indentation. Std overlays read and parse once
  for their deep-index overview and declaration inventory.
- [x] #1029: both CLIs print the runtime's existing bare entry-range message
  for every integer outside 0..255, exit 1. The i64 runtime API is unchanged.
  Exact stdout/stderr/status tests cover run/exec and standalone Sparrow,
  0/42/255, 256/300/-1, both i64 boundaries and positive/negative 2^70.
- [x] #1031: regenerated idna.dat with gen-idna.pl, Perl 5.42.2 and
  Net::IDN::UTS46 2.591, entirely offline. The decompressed diff removes only
  `B L 110000 10FFFF` and `J U 110000 10FFFF`; all B/J ranges are ordered and
  end at or below 0x10FFFF. No manual data edit.

Validation: 88 distinct selected tests pass: compiler ide::tests (43),
syntax::format::tests (25; idempotence and token/comment preservation),
ide::std_paths::tests (4), eight documentation rendering/publishing tests,
runtime-core native::idna::tests (5), and three CLI regressions in the two
cli.rs integration files. New regression tests fail against the original
implementations; a separate renderer mutation confirms the synopsis failures,
and the restored renderer passes. `cargo check --offline -j 4` passes.
Generated maps refreshed with `python3 tools/map/gen.py`; `git diff --check`
passes. No remaining work in this six-ticket scope. No commits, CHANGELOG,
std source, parser, IR verifier/printer or Sparrow verification/typing changes;
no full test suites, Docker, cargo fmt or clippy.

- [x] #1034 (feature/1034): `Failure::record`/`Failure::sites` (runtime.rs): the head of
  `TRACE_LIMIT − TRACE_TAIL` innermost activations, then a ring of the `TRACE_TAIL` = 4096
  outermost; `Uncaught::cause_traces` carries each cause's omitted count; the count line
  says "in the middle" under the root and under causes.
- [x] #1026 (feature/1026): okkama-native `argon2_into` (zeroed `Vec<Block>` through
  `hash_password_into_with_memory`), `argon2_hash` mirrors `hash_password_with_salt`,
  `argon2-verify` mirrors `verify_password` over it; direct dependency `ed25519` with
  `zeroize` (Cargo.lock and fuzz/Cargo.lock).
- [x] #1062 (feature/1062): `typing::non_finite` walks `GlobalDecl.data` after `fits`;
  the global check refuses the first non-finite float; unit test `typing::data_tests`.
- [x] #1066 (feature/1066): `jit::compile_limit_text` (MiB / KiB / bytes) in the compile-cost
  refusal; unit test `compile_limit_tests`, integration `compile_cost.rs` at 256 KiB.
- [x] #1056 (feature/1056): `asm::Parser::error_after` (line of the consumed token) at the 14
  consume-then-fail sites of asm.rs, operand.rs and `instructions!`; `unknown_escape` prints an
  invisible escape by code point; unit tests `asm::line_tests`.
- [x] #1057 (feature/1057): `session::place` names a changed item by (fresh position, item);
  constants by their init's symbol (`fresh_globals`); test `tests/session.rs`.

- [x] #1061 (feature/1061): `codec::VERSION` = 18, `sparrow 19` in every `.spa` and test
  source (51 files); `codec::tests::the_instruction_set_is_pinned_to_the_version` hashes
  `Instruction::SIGNATURES` (test-only) and the `op_enum!` codes. SparrowBook rebuilt
  (build.py gained the six missing mnemonics 157–162). Branches still writing `sparrow 16`
  in new tests need the same rename when they land.

## Test/tool audit (2026-11-26, feature/065-tools)

Scope: #1048, #1058, #1059, #1053, #1051, #1052, #1067, #1050, #1055.
No commits or CHANGELOG edits; no Docker, fuzz campaign, full Rust suite,
clippy or cargo fmt. Only bounded, selected tests and dry tooling checks.

- [x] #1048: all seven listed compiler test files pin their observed pipeline
  with `with_passes("all", ...)`; all 107 tests pass with `OKKAMA_IR_PASSES=none`.
- [x] #1058: CLI differential tests enumerate `lowering::PASSES` plus all/none.
  Pin the std path and require the arithmetic oracle, so missing imports cannot
  count as equivalent execution. Arithmetic across every selection and the
  deliberate wrong-pass test pass. The existing failures fixture still checks
  cleanup, errors and 100000 tail calls; it was not executed locally because
  the task prohibits long recursion on the host.
- [x] #1059: the scope fixture contains FutureScope.outcome; absence fails, and
  both original SPB acceptance and crafted rejection are checked. Both tests pass.
- [x] #1053: setup-free group 0 splits every 40 checks; shared setup stays intact.
  Three Python regressions pass. Generated six affected families (31375 checks);
  the five setup-free families have no action exceeding 40 checks.
- [x] #1051: in-repo HTML template, geometric aggregation of every engine round,
  every cell required VALID, missing/invalid/incompatible rounds excluded,
  duplicate files and mixed version labels rejected. Seven tiny-fixture tests pass,
  including standalone HTML generation. Archived c/d rounds reproduce 55 reliable
  non-control pairs and +0.5631095% gross time, rather than keeping round d alone.
- [x] #1052: record actual --version, compare to the local revision's workspace
  package version, refuse mismatches before workloads; an explicit override stays
  visibly flagged and cannot be compared as release evidence. Missing local
  revisions are UNVERIFIED with a warning. Runner tests: 18 pass; --plan passes.
- [x] #1067: one boolean helper per consuming Rust crate, used by runtime,
  compiler, CLI and test switches; NO_COLOR remains nonempty presence. Document
  unset-OFF defaults, including POINTER_FREE, INT_RANGE and TRACE. Trace tests
  explicitly request tracing. Boolean matrices, stats subprocesses, prototype
  selection, and std terminal inline tests pass; std formatted with built okkama.
- [x] #1055: typed LoweringError::Internal for invariant failures, including OIR
  verification and linearity; analyze/exec share a rejection policy which panics
  on that kind, and exec expects successful codegen/encoding. Two compiler
  mutation tests (undefined return operand in sum.ok and missing resource close)
  reach the shared fuzz policy and panic; two rejection-policy tests pass.
- [x] #1050 diagnostics: coverage build/replay/report logs retained per target;
  stale reports removed, TOTAL required, failure tail printed, final run exits 3
  after checking all targets. Three dry regression tests pass, including the full
  driver with fake tools and failure of every coverage target. Explicit coverage
  build path and bounded build jobs;
  preserve the frame pointers required by Sparrow when cargo-fuzz sets RUSTFLAGS.
- [ ] #1050 historical spb/exec failure cause and real coverage replay: the cited
  job's stderr was discarded, so its cause cannot be recovered from the job log.
  Instrumented offline `cargo check` of all six targets passes, but does not link
  or replay them. The permitted dry checks cover failure handling; a real nightly
  coverage run is required to establish whether another build/replay fix is needed.

Historical #1052 evidence: all four `v0.6.3-...-r064{a,b,c,d}.json` files in
`okkama-docs/benchmarks/micro/results` declare `7d2b3403e` (Cargo.toml: 0.6.3),
but record `okkama 0.6.2` and SHA-256
`8b5484fec30729c3a9f22a1867b9083dbd261f01c1e90922faaed7d86ce7a440`.
No other archived micro campaign has that hash. The retained micro-tc directory
contains only v0.6.4; the baseline binary is unavailable. Thus the historical
0.6.3 → 0.6.4 identity is unverified, even though its numerical aggregation is
reproducible. No archive was rewritten and no replacement campaign was run.

Final validation: `cargo check --offline -j 4`, all selected Rust tests and 31
Python tests pass. `okkama fmt --check std/terminal/capability.ok` passes.
Generated maps are refreshed with `python3 tools/map/gen.py`; `git diff --check`
is clean. No full suites, campaigns or host long-recursion checks were run.

Mutation checks reject the old unpinned lowering, inactive scope fixture,
swallowed Internal errors, presence-only stats, oversized group 0, last-round
aggregation and unchecked binary version. Mutated sources are restored.


## Documentation review (2026-11-26, feature/065-docs)

- [x] #1027, #1028, #1030: source line docs, OIR-2 help and the IR-pass warning.
- [x] #1035: remaining Russian host comments translated after checking their code.
- [x] #1036 points 2–4: REPL declarations, Sparrow help spacing, OIR-2 bytecode docs.
- [ ] #1036 point 1: `main()` still dispatches lsp/repl before parsing trailing
  arguments. Fixing `--help` and rejecting extra arguments changes program behavior,
  which this documentation-only task explicitly forbids. Use `okkama help lsp`
  or `okkama help repl` for now; a behavioral patch is outside this task's authorization.
- [x] #1038, #1045: pruning/format-width descriptions and reserved-name docs.
- [x] #1065: string comparison, capability field syntax, complete map-key types,
  accurately named map test, OIR type spelling in array.remove errors; also remove
  the repeated map operand in map.keys/map.values errors as explicitly permitted.
- [x] #1068: remove obsolete JIT/PostgreSQL settings, correct the Rust OCS endpoint
  description, document worker output limits and developer switches with readers/defaults.
  The Python conformance adapter still reads OCS_RUST_ENGINE; its invocation stays valid.
- [x] #1054: generated index documents non-rule exclusions without changing extraction;
  current release branch, benchmark references/engine, rc2 numbers, gate steps/triggers,
  and debug conformance versus fat-LTO std/t binaries. Remove stale test-binary counts.
- [x] #1049: stale JSON/corpus/recovery comments corrected; corpus inventory verified
  as 499 + 13 files; parser rejection assertions compare complete intended messages.
  The unused corpus metadata fallback stays unchanged under the no-behavior-change rule.
- [x] Targeted integration files: parser_expressions 39/39, ir_verify 16/16.
- [x] `cargo check --offline -j 4`, `python3 tools/map/gen.py --check` and
  `git diff --check` pass; maps regenerated with `python3 tools/map/gen.py`.

No commits or CHANGELOG edits. No full suites, application binaries, Docker, fmt or clippy run.

## Release 0.6.4 (2026-11-23, release/064)

- [x] Hotfixes from reading 0.6.3 end to end (milestone "Hotfixes TBD", board project 1): #973–#1004
  fixed, each with a unit test and a mutation check, blast radius measured over std, t/, examples and
  conformance; CI waves 1–7 on release/064.
- [x] Deferred: #1000 (macOS TLS flake, not reproducible), #1005 (owner decision: overload names,
  object named Maybe), #1006 (refactors, Unicode version contract, formatter width and std fmt-clean).
- [x] wave/rel064 green (it caught t/gen against #984 and a clippy lint, fixed in b09f9dfa8); release CI
  green; initial rc1 micro A/B 0.6.3 → 0.6.4 interleaved: −0.4% over 51 reliable pairs, all within noise
  (candidate 7fb98e01f); superseded by rc2 29c3dc84a, +0.6% over 55 reliable pairs
  in the final release report (okkama-docs reports/2026-11-23-v0.6.4/micro.html).
- [ ] Tag v0.6.4 (owner pushes), Packages and GitHub Release, install-pgo, man site, cleanup.
- [x] #1016 (feature/1016-twins): runtime-core native twins removed (network, tls, sqlite, db, postgres,
  host_crypto, process, input, system, stdio, buffer, encoding, numbers, bytes, jre; regex, fs, json, idna,
  net, text, misc, formats, host cut to their live parts), ~14.9k lines; `native::execute` refuses
  unrouted natives; a sparrow-core test keeps the ORIGIN routes and runtime-core providers equal.

## Release 0.6.4 (2026-11-23)

- [x] Released from release/064: hotfixes #973–#1024 (milestone "Hotfixes TBD" stays open,
  findings after it carry no release, owner 2026-11-23); micro A/B +0.6% (55 reliable pairs),
  async 4–6% slower → #1025; Bible editions rebuilt (249 examples checked).

## #1005 one overload identity, reserved system type names (2026-11-23, release/064)

- [x] `analysis/registry.rs`: `same_overload` (name, count, resolved types, variadic; a pair
  with an error type is skipped) for object actions, fakes and role signatures;
  `check_ctor_overloads` after `infer_field_types` for full and shorthand constructors;
  one error at the later declaration, one note at the first (`first_declared`).
- [x] `reserved_type_name`: Error, Maybe, Presence, Future, NativeFailure, Cancelled for
  objects, roles, enums, native objects (`register_names`) and type parameters
  (`new_params`); `Future` always resolves to the system type.
- [x] Corpus audit: only Duration's unit constructors broke the rule (#1018).

## #1006 compiler leftovers (2026-11-23, release/064)

- [x] One dominator implementation (`cfg::dominators_by`); `operands`/`operands_mut`
  from one macro; unread AST fields dropped; constant uses recorded in
  `Registry::const_uses` for IDE hover/definition; codegen errors placed through
  `DebugInfo`; a `dce` pass drops unread constants after fold/hoist.
- [x] Runtime part (SQLite text and kind, Unicode versions, async natives, blocking,
  dead natives) with an agent; formatter header comments, width, std fmt-clean + gate.

- [ ] Runtime part (SQLite text and kind, Unicode versions, async natives, blocking,
  dead natives) with an agent.
- [x] Formatter (feature/1006-fmt): header comments stay on the header line (`//` opens the
  body); lines over WIDTH break at the outermost list/operator chain/call chain (`fit` in
  `syntax/format.rs`), roles after `does` and case patterns fill; generators of plurals, iana,
  hpack (and gregorian drift from #943) emit fmt-clean text; `fmt --check` over std in gate.sh,
  CI and wave. Blast radius over std, t/, examples, conformance, Bible examples (2291 files):
  no Internal, all idempotent; code lines over 100 columns not made of one long literal
  712 → 7, none with a break point (a string, 3 test names, 3 long `let … is Type`).
- [ ] `okkama fmt -w std` (lead; 251 std files change under the new formatter), then the gate step passes.

## #1018 one Duration constructor (2026-11-23, feature/1018-duration, part of #1005)

- [x] `std/time/duration.ok`: only `Duration(seconds is rat)` (whole nanoseconds or
  `InvalidDuration`, no rounding, no int → rat); `Duration(value:, unit:)`, the seven
  unit constructors and `DurationUnit` removed; module docs around the one constructor
  with an `okkama,run` example; inline tests: whole/fractional/1 ns, negative and zero,
  arithmetic and order, scaling, `DurationString` round trip, sub-nanosecond rejected.
- [x] Every call migrated (units written out: `(5 * 60).rat()`, `rat(250, 1000)`):
  std 63 files, examples, conformance (time.durations rewritten; two new reject
  cases `L09.time.duration-unit-constructor`, `L09.time.duration-int-seconds`),
  bench/micro (86 programs), Bible chapters and examples (EN, RU), OkkamaBook,
  okkama-cli/okkama-compiler test fixtures. Inventory, STD-ACTIONS, coverage regenerated.
- Blast radius, base `release/064` vs branch (debug): std `run -t` 499 modules, only
  time/duration differs (new tests); t/ 142 files 50972 checks pass on both; examples
  75/75 on both; conformance full-release 1071/1071 → 1073/1073, only
  `L09.time.durations` stdout differs (rewritten case); micro check 773 programs match
  oracles on both; Bible `--check` 249 examples.
- [ ] Next: #1005 enforcement (lead) finds no name-only constructor overloads in std.

## #1006 runtime leftovers (2026-11-23, feature/1006-runtime)

- [x] SQLite refusals carry `sqlite3_errmsg`; plain SQLITE_ERROR classified by
  message (syntax / resource / data / unexpected, as PostgreSQL's classes);
  extended codes FULL, IOERR_NOMEM, NOLFS, MISSING_COLLSEQ mapped.
- [x] Unicode versions documented per surface (properties 16.0.0,
  normalization 18.0.0, clusters 17.0.0, widths from 16.0.0 properties), std
  tests pin them; Bible 6 and the Book follow.
- [x] Async process natives refuse with a failed Future by
  `signature().is_async`; `blocking()` dropped (Tokio 1.54.1 `into_owned_fd`
  clears O_NONBLOCK, pinned by a test).
- [x] Dead natives removed: `random_sample_accepted`,
  `random_integer_from_sample`, `random_float_from_sample`, `signal_wait`
  (IDs 386–388, 632 retired; 569 natives).
- [x] #1019: `unicode.dat` regenerated at Unicode 17.0.0 from ICU4C 78
  (`gen-unicode.c`); widths, clusters, character properties and regex
  categories are all 17.0.0, normalization 18.0.0. U+1ADD alone is 0 columns,
  ORCA U+1FACD 2.
- [x] #1020: the natives without a std caller removed (555 natives).
- [x] #1022: fs `chdir`, `read`, `seek`, `truncate` removed (IDs 161, 163,
  165, 211 retired; 551 natives); the #40 test now shows a run reads the
  process cwd and has no `chdir`. `tools/engines/native-callers.py` (gate step
  "natives" with gen-abi --check, Wave `maps`) fails on a native without a
  caller; 13 legacy SQLite/db_capture natives are kept by name.
- [x] #1000, #1021: an HTTPS drain no longer exits 1 when a handshake fails
  just as the drain cancels it (`HttpServerHandshake` in http/server).
- [x] #1023: a peer the server cancels (close, drain) settles quietly
  (`HttpServerPeerWork` in http/server): a read, write, handler or cleanup of
  its connection that fails as the cancellation comes no longer fails `listen`.
- [x] #1024: a TLS record header that cannot be one (content type outside
  20..=24, major version not 3, body over 2^14 + 2048) goes to rustls at once,
  client and server: plain HTTP to an HTTPS port gets an alert at once
  instead of waiting out the headers timeout.
- [ ] Open: IDNA's UTS #46
  table (Net::IDN::UTS46) is older than 17.0; its X status keeps refusing
  17.0 characters.

## #1011, #1012 Sparrow host net, test reports and data (2026-11-23, feature/1011-host-net)

- [x] #1011: TLS `abort` wakes pending operations (no hang after a cancelled
  write); TCP `listen(backlog)` at `tcp_start_listening`, nothing queued before;
  `IPV6_V6ONLY = !dual_stack` for TCP and UDP; IPv4 `IP_MULTICAST_IF` by index;
  DNS `unavailable` for `EAI_AGAIN`, IPv6 literals before the name check, the
  five dropped resolver tests back; zone names by `if_nametoindex`; JUnit
  `errors` counts crashes; Busy/interruptible shared by TCP and TLS.
- [x] #1012: legacy SQLite floating text refused (`SQLITE_FLOAT_RANGE`);
  `Regex.all` linear (queries share the text's scalars: 16000 matches 44.3 s →
  0.54 s, debug); `Json.text(name)` and friends through the name index
  (20000 members 1310 → 62 ms, debug); JSON hard depth limit reports a real
  line/column; postgres page restored on any exit; `needs_tokio` skips
  Postgres; stale headers and SAFETY comments.
- [ ] Parked: stdout of a worker killed by `--test-timeout` — `stdout::write_through`
  is in runtime-core, the one-line call in `okkama-cli/src/main.rs` (worker
  side) waits for the lead (CLI was another fixer's area). SQLITE_ERROR kinds and
  `sqlite3_errmsg` text (std refusal texts, owner decision).
- Blast radius, base `release/064` vs branch (debug): conformance run/test 873
  cases 0 differences; std `run -t` 212 modules same (132 with tests, all pass);
  t/ 142 files 50972 checks pass on both; examples 75/75 on both.

## Release 0.6.3 (2026-11-22, release/063)

- [x] Bible findings #952–#971, #954, #956, #967; #957 (Cell stays synchronous, local rule); security
  #960–#963; #946 (parallel/map worker reuse); #951; #972 (linearity bound); version 0.6.3.
- [x] Micro A/B 0.6.2 → 0.6.3: within noise (−4.1% over 47 pairs; campaign order shifts a few percent);
  the map-literal regression of the first candidate fixed (790fd2c51, #[cold] error text).
- [x] v0.6.3 published (Packages on the tag, GitHub Release with 14 assets), install-pgo, man site on
  0.6.3, Bible PDF/HTML rebuilt (fb8c20446), worktrees and micro toolchains cleaned (baseline
  micro-tc/v0.6.3 kept).

## #946 run-local ParallelMapped worker reuse (2026-11-22, feature/946-pool)

- [x] Extend spawn's Tokio Request/ProcessGroup transport with reusable Unix
  sessions; keep the OKW2 codec and separate ordinary output from control frames.
- [x] Scope the hidden pool to one results() call; keep batching, ordering,
  FutureScope cancellation and awaited process cleanup. No public std/spawn API
  changes, no automatic process executor in embedding, no cross-call reuse.
- [x] Reconstruct each receiver in a fresh Runtime/heap/JIT/thread/host context;
  exclude synthetic once slots from worker binding/entry inputs. Retain one-shot
  spawn for programs with native process launches, preserving descendant cleanup.
- [x] Docker targeted runtime tests: 18 with filter `worker` plus the new intrinsic
  contract test; 2 CLI integration tests; 7 inline map tests via build --tests /
  sparrow --test. Targeted compiler lowering test passes. Reuse and once-slot
  mutation checks both fail as intended, then the restored sources pass.
- [x] Release benchmark, macOS arm64, N=100 / warmup=10, five samples each:
  median **9.494705 → 5.693915 ms/call**, **1.6675×**, **40.03% less**.
  Timed/warm checksums: 303/33 in all ten runs. Fresh VM/JIT per item remains.
- [x] Document process-global isolation limits, update maps and external design /
  report. No commits or CHANGELOG edits. Public declarations unchanged; std
  inventory regeneration does not apply.

The pool shares PID, native statics/caches and allocator/foreign process-global
state within one run; it is not fresh-process isolation or an untrusted-item
boundary. Non-Unix transport retains one-shot behavior. Implementation is left
uncommitted for the lead session; no unresolved implementation blocker.

## #957 local Cell rule implemented (2026-11-22)

- Local checker-only frame classification reuses closure capture pairs and
  covers bound object/role receivers, nested captures, branch/block tails,
  synchronous calls versus spawn/async entry, and every consuming operand.
- Coercion and resolved generic substitutions refuse Cell erasure; enum
  payloads mirror the field rule. Further audit amendment: hidden `once let`
  fields cannot hold a Cell returned through an action field.
- Focused tests: 82 pass. Base mutation: all 72 new-restriction assertions fail;
  10 positive controls pass. Old patch's 27 boundary cases are preserved.
- All 1,914 existing std/corpus files checked before/after: no changed
  diagnostics. 21 pre-existing ownership-linearity failures and intentional
  rejection fixtures remain; t/ passes 51/51 with its support import root.
- Five debug-check runs per binary/program: median 24-postgres-wiring
  2.1528 → 2.1826 s (+1.38%); 24-notes-tests
  4.0065 → 4.0279 s (+0.53%).
- Ten new conformance cases pass their check outcomes and schema validation.
  Bible §18.6 EN/RU updated; accepted/rejected examples pass in bounded Docker.
- No whole-program Cell pass, public action effects, runtime/VM changes,
  std API changes, commits or CHANGELOG edits. External spec proposal is in
  the task report because that repository is outside writable roots.

## Security release 0.6.3 (#960, #963, #962; 2026-11-22)

- [x] #960: fuzz lockfile regenerated from the current manifests; both
  lockfiles pass `cargo audit --deny warnings`. Audit CI checks both graphs;
  tools/fuzz/locked-cargo puts `--locked` on cargo-fuzz's child
  build/check/metadata calls (cargo-fuzz has no such option).
- [x] #963: the Security audit kit (outside the repo) is Sparrow-only; #382/#399
  point at okkama-cli; structural check 64/64; runs record source and binary
  revisions separately.
- [x] #962: docs/security-model.md checked against implementation and tests:
  exact channel payload scope, permissive native default, explicit
  restrictions, memory scopes, cooperative native cancellation.

## #965 data_dump diagnostics (2026-11-22, feature/965-data-dump)

- [x] Preserve primitive formatting; unsupported values raise the existing
  NativeFailure with core.debug / unsupported_type, kind/type and Dumper guidance.
- [x] Reuse inspection metadata. Sparrow's dump-only bridge preserves future and
  failure categories and does not traverse object fields; other inspection calls
  keep their existing conversion. Existing native-failure reporting retains the message.
- [x] Core regressions cover supported scalars/enums/collections, unsupported
  categories, nested arrays/maps and object keys. Sparrow regressions cover the
  displayed error, uncaught metadata, category bridging and supported arrays.
- [x] Targeted `data_dump` lib tests: runtime-core 2/2, sparrow-core 3/3.
  Restoring the old failure branch makes the negative test fail while the positive
  test passes. `cargo check -p sparrow-core --offline -j 4` passes (including core).
- [x] Update runtime/debug doc comments and code maps. Bible §19.2 already uses
  Order.less (an enum); §21.2 needs no correction for this diagnostic change.
- Scope limits: no full std/conformance suites or Bible builds under the owner's
  command restrictions; no commits, remote edits or CHANGELOG changes.

## The Okkama Bible (2026-11-22)

`docs/OkkamaBible` is on develop (fast-forward of feature/okkama-bible, owner 2026-11-22). Every chapter was
checked against the 0.6.2 source; appendices A–E and the PDFs (662 pages EN, 703 RU) are built. 246 examples
pass with `build.py --check` on the installed 0.6.2. Findings: milestone "Bible findings" (#952–#959,
#964–#968), not yet assigned to releases. The PDF toolchain is the `okkama-bible-pdf` image from
`docs/OkkamaBible/pdf/Dockerfile`.

Last update: 2026-11-20, after wave/78. The working branch of 0.6.0 is `release/0.6.0`; develop is behind it.

## #133 URL construction and transformation (2026-11-22, l-codex)

Scope: the supplied #133 review (2026-11-21), authorized for implementation by
the owner on 2026-11-22. Work stays in `l-codex`; no git or remote issue edits.

- [x] Inspect the existing component constructor, canonicalizer, query/percent
  encoding, HTTP client call sites, std principles and RFC resolution cases.
- [x] Add inline tests before implementation; the initial check fails for the
  missing transformation API. The original three `url/value` tests pass.
- [x] Add immutable `path_replaced(path:)`, `joined(segments...)`,
  `query_replaced(parameters:)`, `query_removed()` and `port_replaced(port:)`.
  Return ordinary validated `UrlParts`, using the canonical constructor; no
  successor bypass of validation, whole-URL reparse, builder or duplicate accessor.
  Add safe defaults to the existing structured constructor.
- [x] Verify raw paths versus plain segments, escaping, empty segments, relative
  forms, immutability, preservation, IPv4/IPv6/named hosts and userinfo, explicit
  ports and removal, query identity/order/duplicates, and length/segment bounds.
- [x] Verify round trips and preservation properties over 144 generated URLs;
  retain all 42 normal/abnormal RFC 3986 section 5.4 resolution examples inline.
- [x] Convert the three HTTP client examples to structural URL construction;
  GET demonstrates query replacement. Add a server-free `build_url` example
  with an embedded slash and repeated query names, and its expected output.
- [x] Update English/Russian Book documentation and `//` CommonMark API comments;
  format the touched Okkama files with the supplied binary.
- [x] Local verification: `url/value` 15 tests, `url/query` 3 tests,
  `url/percent` 5 tests pass; `check` passes for all touched modules/examples
  and `std/http/client.ok`. The construction example and existing
  `examples/url/parts.ok` run and match their `.expected` text.

Validation boundary: only the supplied Sparrow binary is available/authorized;
there is no second current backend (see decisions). Full conformance suites,
Docker, network runs and builds are excluded by the task. The server-backed
client examples are statically checked, not run. No language/runtime changes.

Ownership/priority: URL is a foundation area in `docs/std-principles.md` §8 and
stays in std; an external `ok-package` move is inappropriate. Recommend P2:
current client examples expose manual-construction ergonomics, but this review
found no concrete exploit requiring P1. No release version changes are needed;
CHANGELOG is excluded explicitly. Semantics are recorded in
[decisions.md](decisions.md) and the Book's URL section.

## Branches and worktrees (`../okkama-work`)

| Worktree | Branch | What | Status |
|---|---|---|---|
| w4 | release/0.6.0 | merge point (902eb0ef7 = wave/79) | merges only after a green Wave CI |
| dev | feature/911-ci (was feature/devtrace, merged as wave/79 = 902eb0ef7) | Tweed work | — |
| sch | feature/847-schedule | **parked** fix for SPB_SCHEDULE: an offered binding with service entries makes its owner dynamic (4a6c0faee) | no test; frozen pending the ownership decision |
| cp | feature/754-cancel-docs | old | unknown: check before use |
| r060 | wave/18 | old | unknown: check before use |

## Champ removed (#911, 2026-11-20)

The following are gone (branch feature/911-remove-champ, about 355k lines):
- the OIR-2 pipeline: the okkama-oir crate, `lowering/oir2` and `codegen/sparrow_oir2`;
- SPB 17: `okkama-sparrow/src/spb17`, whose shared `Repr`, `TraceSlot` and `OBJECT_KIND` moved to
  `okkama_sparrow::layout`;
- the Champ runtime and JIT (`sparrow-core/src/champ`, `jit/champ`) and every `champ-abi` cfg (feature removed);
- the Champ tests, the conformance adapter's champ engine with its blockers ranking, the verifier differential,
  the P1 two-toolchain oracle and the OIR-2 reference model.

The parked branch feature/847-schedule (worktree sch) is obsolete: remove it.

## Tweed (decided 2026-11-20): phase 0 measurements

Setup: Tomato v0.5.8 (Linux toolchain `okkama-work/micro-tc/v0.5.8`, `--engine sparrow`, which that old CLI still
takes: its default was IR) and CPython 3.13 in the same `okkama-dev` container, 1 CPU. Micro-v2 protocol, oracles checked.
Scripts: scratchpad `tweed0/` (session 36c16e3b).

- **Micro, ns/iteration, Tomato / Python.**

  | Bench | Tomato | Python |
  |---|---|---|
  | baseline | 4.2 | 22.8 |
  | constructor | 8.3 | 102 |
  | successor | 13.5 | 184 |
  | destroy | 95 | 208 |
  | destroy-observed | 84 | 184 |
  | defer | 11.7 | 97 |

  Tomato is faster everywhere. A resource still costs about 87 ns over a plain object, 11× the object itself.
- **11-ownership: Tomato 84 ms, Python 30 ms (2.8× slower).** The same program without `Vault.destroy` takes
  4.6 ms. **The ownership tax is about 95% of the run.**
- **Why.** devtrace counters, per run of 11-ownership:

  | Counter | Count |
  |---|---|
  | `rt_created` | ~100k |
  | `rt_region_add` | ~600k |
  | `rt_region_leave` | ~100k |
  | graph walks | 2.9k |
  | walk steps | 2.9M |

  Only about 400 objects have a destroy. Tomato decides "managed" **per type**: every `Record` has a
  `Ledger`-role field, and `Vault` is one implementation of that role. The decision should be **per
  instance**: 99.9% of the records hold a plain `Account`.
- **Tweed runtime, phase 1.**
  - A per-instance "reaches managed" bit computed at construction from the field values. Objects are immutable,
    so it never changes: O(fields).
  - Region add and transfer of a value without the bit cost one branch.
  - Walks descend only into fields with the bit set.
  - Late edges (Atomic, Future outcomes, builders, `obj.init`) keep a conservative path.
  - Expected on 11-ownership: about plain (≈5 ms) against Python's 30 ms.

## Board (2026-11-20, after the cleanup)

- **Milestone 21: "0.6.0 — Tweed".** 30 Tomato tickets are open, and the Tweed epic is **#903**.
- **#903's sub-issues:**

  | Ticket | Item | State |
  |---|---|---|
  | #904 | phase 0 | done |
  | #905 | per-instance managed bit | open |
  | #906 | walks follow managed fields only | open |
  | #907 | late edges | open |
  | #908 | ownership benchmark gate against Python | open |
  | #909 | phase 2 elaboration pass | open |
  | #910 | debug-only checker | open |
  | #911 | retire Champ | open |
  | #912 | devtrace for Tomato's pipeline | open |
  | #913 | naming | open |

  Also linked: #523 (the OH-* roadmap), and the Tomato async fixes #587, #769, #797, #817, #845 and #846.
- **Closed as not planned:** 95 Champ/OIR-2 tickets, among them the corpora buckets #895–#902.

## Next

1. #911 is done: step 1 (CI) in wave/80, step 2 (code) in wave/81.
2. #908 is done: in the container 11-ownership takes 100.5 ms on Tomato, 23.0 ms on Tweed and 28.2 ms on CPython.
   Command and table: bench/ownership/README.md.
3. #909, phase 2: elide region tracking of values that do not escape (a reduce accumulator's versions), plus
   the remaining region-add calls.
4. #914: a temporary child in a group reads a lent resource after its destroy (Tomato bug).
5. #909 step 2a merged (044ce1979 = release/0.6.0): the IDENTITY header bit; small closes sort on the stack.
6. #528 (OH-04), in-object identity: **+16 chosen by the owner (2026-11-20)**, the packed variant removed.
   Numbers: okkama-docs/runtime/identity-layout-528.md (merged: 0e29d87d0).
7. #909 2b: the `statics` pass (feature/909-phase2). micro destroy 80 → ~50 ns on the host; first cut takes one
   resource per region. Merged (90aa8d63b). Container gate with CPython and Go: bench/ownership/README.md.
8. #529: JIT fast paths for region adds and per-instance identities, inherited summaries. ownership.ok on the host
   21.7 → 9.5 ms (plain twin 4.0). Owner 2026-11-20: ownership is solved; continue 0.6.0 in my own order.
   Merged (bdf7cb4a0); #526 closed as not needed.
9. #817: the root region and root closer for Futures that outlive their owners (wave/90).
10. #845 and #797 merged (74f704764). #769 and #846 wait for the owner (std contract); #587 needs a channel and
   group handoff on the region runtime (large).
11. #907 (late edges), #912 (devtrace by site), #910 (linearity checker + an exit_scopes fix it found): wave/92.
12. #552: big async bodies keep their registers in their frame (fixture 2.8 s → 0.22 s).
13. Night of 2026-11-20/21: statics reach factories, async bodies and shared regions (oldest member); JIT
    numbers inline identities and destroys static resources directly; lazy regions (#530). 11-ownership on the
    host: 9.5 → 7.5 ms. Closed as checked or done: #482, #523–#529, #531–#533, #537, #540, #552, #797, #817,
    #845, #907, #910, #912, #913.
14. Waiting for the owner: #769 and #846 (std contracts), #790 (language), #538 (GC floor numbers). Big designs
    still open: #453, #457, #541. #588 (REPL) moved to 0.6.1 by the owner (2026-11-21).
15. #587 (owner's approval of 2026-11-21): Future INPUTS, completion-time release for async bodies, drivers keep
    theirs, channel packets owned by the channel's capabilities (feature/587-inputs).
