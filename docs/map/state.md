# State (update at the end of every task)

## Release 0.6.5 (release/065)

- [x] #1064 (feature/1064): `ir::Role::slot_name` returns `Option`; the printer prints `<invalid>` for a slot
  the role lacks (`call.role`, `does`), the verifier's messages fall back to `#N`.
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

- [x] #1061 (feature/1061): `codec::VERSION` = 18, `sparrow 18` in every `.spa` and test
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
