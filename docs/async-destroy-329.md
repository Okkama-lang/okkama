# Async destroy cost gate (#329, 2026-11-22)

Scope: the supplied ticket and comments, the supplied 2,000,000-call benchmark,
and the owner's runtime-first instruction on the fresh release/062 worktree
`l-codex2`. No language, compiler, std API, CHANGELOG or git changes.

## Execution tracker

- [x] Read the ticket, benchmark, runtime guide, code maps and Book 20.3.
- [x] Instrument helper counts and inclusive timings using devtrace.
- [x] Measure the reference built from this tree, five runs of each mode.
- [x] Implement the single-object fast path using the existing ownership rule.
- [x] Add runtime and Okkama lifecycle coverage and verify each new test with a mutation.
- [x] Run targeted ownership and Future regression tests after restoring mutations.
- [x] Measure the final release, five runs of each mode: gate **not met**, stop after step 3; no std conversions.
- [x] Update code maps and remove the disposable target and measurement binaries.

## Cause and implementation

On the reference, **both modes create 4,000,001 Futures and call `run_body`
4,000,001 times**. Destroy adds 4,000,000 `region_add` calls, 4,000,000
`region_close_until` calls, 12,000,000 `owner_of` calls, 4,000,000 owner writes,
4,000,000 region pushes and 6,000,000 separate membership checks. Counts cover
one complete invocation of the benchmark, including its main function.

Separate timing runs (counts disabled) report 102.388 ms in `region_add` and
122.330 ms in `region_close_until`, versus 109.574 ms in `future_new` for destroy
and 95.076 ms in `future_new` for close. These are **inclusive diagnostic
intervals with probe overhead**, not estimates of uninstrumented ns/call.
Nested `run_body` and `rt_region_settle` timers must not be added together.
The measured region intervals total roughly twice the destroy-mode Future
creation interval, consistent with the supplied sampling profile.

The runtime changes are:

1. `region_close_until` recognizes exactly one ordinary object with an async
   destroy, no intrinsic runtime state and no possible kept reference graph.
   It skips private-region draining, graph walks and sorting. All other cases
   retain the general path.
2. `region_action` is the **one shared selector** for owner, keep and pin rules,
   called by the fast path and the general iterator. Both use `destroy_one`.
   No alternative owner representation or ownership rule was introduced.
3. `region_push` checks membership and finds its free inline slot in one scan;
   overflow insertion uses the existing membership set. `region_add` no longer
   performs the same membership check before calling `region_push`.
4. Normal `rt_region_settle` exits borrow the single keep word instead of
   allocating a `Vec`. Failure payload lists retain the general implementation.

The region retains the selected receiver until its destroy Future has been
created. Pending destroys still live in `RegionValue::settling`. The existing
`rt_region_settle` path continues to root receiver/region/frame/failures,
run the ordinary Future with SHIELDED and OBSERVED, await suspension and link
cleanup failures. Defers still run before object destruction, and multiple
objects still close in reverse creation order.

Instrumentation is opt-in: `OKKAMA_DEV=cleanup-count` or `cleanup-time` with
`OKKAMA_DEV_OUT=...`. The two categories are separate so locking for detailed
counts does not contaminate the timer intervals. Normal timing runs unset both
and unset `SPARROW_CORE_STATS`. Packaged builds with `okkama_release` compile
these hooks out, like the existing devtrace hooks.

## Measurements

Final back-to-back wall measurements, seconds (2,000,000 iterations each):

| Run | Before destroy | Before close | After destroy | After close |
|---|---:|---:|---:|---:|
| 1 | 0.389999 | 0.191396 | 0.371612 | 0.186307 |
| 2 | 0.391631 | 0.181312 | 0.374651 | 0.184719 |
| 3 | 0.387794 | 0.193087 | 0.349346 | 0.189728 |
| 4 | 0.363817 | 0.191489 | 0.357442 | 0.205580 |
| 5 | 0.386504 | 0.193631 | 0.356179 | 0.200989 |
| **Median** | **0.387794** | **0.191489** | **0.357442** | **0.189728** |

Destroy improves **7.8% by median**, from 193.9 to 178.7 amortized ns/call.
Close is 95.7 before and 94.9 after. The candidate destroy/close ratio is
**1.884**, versus 2.025 before. Even the fastest destroy run remains above
the slowest close run. **The ticket's precondition is not met. No guards were
converted, and no std-module tests or service loads are applicable to a
conversion in this run.** All eleven candidates remain deferred under #329.

Raw timings, hashes, diagnostics, allocation counts and mutation statuses are
under `docs/measurements/async-destroy-329-*.json`.
The reference source for the wall timings is the original runtime, before the
new diagnostic hooks; the separate reference diagnostics use the same runtime
with hooks added. The final candidate retains the hooks, disabled for wall
measurements. This avoids treating an installed binary as the reference.

The runner `tools/profile/async-destroy-329.py` warms both modes, then runs five
interleaved pairs (alternating which mode starts). It checks exit status and
exactly `2000000` on stdout. Its metric is whole-process wall time, including
frontend and JIT, divided by 2,000,000 only when reporting amortized ns/call.
There are no parallel builds during the measured samples. Numbers are from
this session; the ticket's installed release/062 numbers are context only.

## Verification

All commands below run in `okkama-rs` with:

```sh
export CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-329
```

Successful final checks after all mutations were restored:

```sh
cargo test -p sparrow-core --offline -j 4 --lib single_async_close
cargo test -p sparrow-core --offline -j 4 --lib inline_identity_tests
cargo test -p okkama-cli --offline -j 4 --test tweed_ownership single_async_destroy_lifecycle
cargo test -p okkama-cli --offline -j 4 --test tweed_ownership input_cleanup_suspends_shielded_before_publication
cargo test -p okkama-cli --offline -j 4 --test sparrow_future_outcomes
```

Results: **7 + 8 Rust tests and 1 + 1 + 21 CLI integration tests pass**.
The new seven Rust tests cover creation order and at-most-once destruction,
kept graphs and foreign owners, pins, primary/secondary destroy failures,
suspended shielded cleanup, GC during suspension, GC before Future allocation,
and duplicate insertion in inline and overflow regions. GC is tested with
explicit ordinary collections, without a GC-stress build.

Each new test was mutation-checked (exit 101 with a test assertion failure,
then source restored):

| Mutation | Failing test filter / target |
|---|---|
| Skip creation-order sort | `--lib single_async_close_keeps_reverse` |
| Let the fast path ignore keep graphs | `--lib single_async_close_preserves` |
| Ignore pin handoff | `--lib single_async_close_hands` |
| Drop secondary failure linking | `--lib single_async_close_links` |
| Remove SHIELDED | `--lib single_async_close_shields` |
| Admit duplicate overflow entries | `--lib single_async_close_region_insertion` |
| Clear the receiver slot before Future allocation | `--lib single_async_close_roots` |
| Remove SHIELDED in the Okkama lifecycle scenario | `--test tweed_ownership single_async_destroy_lifecycle` |

The mutation commands use the same cargo package/options as the corresponding
successful commands above, with the listed narrower filter. Raw statuses and
filters are recorded in `measurements/async-destroy-329-mutations.json`.

Release build command (same flags/target directory for before and after):

```sh
cargo build -p okkama-cli --release --offline -j 4
```

From the worktree root, the benchmark runner is:

```sh
python3 tools/profile/async-destroy-329.py target-329/release/okkama --label after --out docs/measurements/async-destroy-329-timing.json
```

It executes the required commands with the explicit worktree std directory:

```sh
OKKAMA_STDLIB_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/std target-329/release/okkama run bench/ownership/guard-329.ok destroy
OKKAMA_STDLIB_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/std target-329/release/okkama run bench/ownership/guard-329.ok close
```

The code maps were regenerated with `python3 tools/map/gen.py` from the worktree
root. The disposable `target-329` directory and measurement binaries were removed.

No broad suites, clippy, formatting, Docker, service loads or GC-stress builds
were run. No std modules were changed.

## Remaining cost and open questions

The final count run confirms the fast path is selected **4,000,000 times**
(two passes per destroy: select it, then finish the closed entry).
All 2,000,000 destroy bodies complete inline; there are no suspended destroys
in this benchmark. The remaining overhead is not a scheduler round trip.
The region still performs **two admissions, two close passes, six owner reads,
two owner writes and three temporary root pushes per guard**. Standalone
`region_contains` calls disappear, but membership is still checked by the
merged insertion scan; this is not elimination of membership semantics.

After timings are 85.134 ms in admission and 92.590 ms in close, with 98.266 ms
in Future creation (close: 97.603 ms), in separate instrumented runs. Heap stats
are unchanged: 831,321,416 bytes allocated; 2,000,006 runtime arrays using
96,000,088 bytes (one 48-byte region per iteration plus startup arrays), and
4,000,001 async frames using 624,000,224 bytes. Both versions collect 195 times.
The optimization reduces CPU work without eliminating those region allocations.
Stats-enabled run durations are not wall benchmark samples.

The final OIR dump (`measurements/async-destroy-329-oir.txt`) still has adjacent
`region.track` and `region.adopt` for the guard; the static resource pass reports
`not static (builtin or async destroy)`. A remaining design question is whether
that existing ownership elaboration can safely coalesce admission and/or extend
its static resource representation to awaited destruction. This run does not
establish that such a change would reach parity; it would need its own proof and
measurements. New ownership rules, eliding shielding and a second ownership
implementation are not acceptable ways to satisfy the gate.

The gate remains open on the existing ticket. The owner's conditional stop after
step 3 applies; std conversion, per-guard lifecycle tests and conversion-load
comparisons have not been started.

## Step 4 (2026-11-22): static async-destroy guards — parity

The owner chose option (a): the compiler may change how it emits regions for a frame-owned guard. Instead of
merging `region.track`/`region.adopt`, the existing static-resource pass (#909 2b) now also takes a guard with
an own async destroy that is alone in its scope's region of an async body: `resource.pin` after the creation,
`resource.settle` at the exits and `resource.settle_unwind` on the failure paths. `rt_resource_settle` closes
the identity once and awaits the destroy shielded and observed, exactly as `rt_region_settle` does for a
region, so cancellation, failure linking and suspension behave as before (conformance L07/L08 async-destroy
cases, t/lang/awaited, Bible 11 and 20, all with `OKKAMA_VERIFY_OWNERSHIP=1`).

`bench/ownership/guard-329.ok` compared its mode string inside the loop, which costs the destroy mode a full
string comparison per call (the close mode fails on the length). With the mode hoisted (the benchmark is
updated), release build, 14 interleaved runs, median:

| Build | destroy | close | ratio |
|---|---:|---:|---:|
| release/062 before | 0.333 s | 0.198 s | 1.68 |
| step 4 | 0.186 s | 0.181 s | 1.03 |

The remaining 2.5 ns per call is the guard's identity (an object with a destroy is registered once). The
precondition of the ticket holds; the guard conversions follow.

