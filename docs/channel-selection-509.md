# Atomic channel selection (#509)

Scope: the owner's supplied engineering review, authorized on 2026-11-22.
Phase A is this decision record, without an approval round. Sparrow (Tweed)
and its GC-stress configuration are the execution targets; IR and JVM execution
were removed. No syntax or general future-selection abstraction is added.

## Design decisions

1. `SelectedReceiving<T>(receivers: @Receiving<T>)` is persistent and provides
   `async item() is SelectedItem<T>`. `SelectedItem.source()` is the zero-based
   input index; `value()` delivers the selected payload. The outcome is a typed
   message holder, with the source needed to route the message.
2. States: idle -> scanning -> parked -> committed -> idle; scanning or parked
   may instead terminate by cancellation or exhaustion. One async frame is
   registered in every open channel, rather than one task per alternative.
3. A ready scan removes exactly one buffered value (refilling from the first
   blocked sender), or accepts exactly one blocked sender. An arriving sender
   transfers its message to the parked frame and marks it completed. This is
   the receive commit; no safepoint or user code intervenes in claim/transfer.
4. Cancellation before commit makes the frame ineligible and removes every
   registration when the cancellation driver runs. Commit before cancellation
   wins: completion is handled before the cancellation flag, as in `Receiving`.
   An already observed all-closed refusal likewise precedes a later cancellation;
   cancellation requested first makes the frame ineligible for that refusal.
   Cleanup is synchronous and cannot interleave with another channel operation.
5. Closed buffers remain receivable. Closed, drained channels are ineligible.
   Closing one alternative leaves other registrations parked. All closed and
   drained throws `ChannelClosed`; repeated close is idempotent.
6. Ready ties use cyclic input order, initially zero, then `(winner + 1) % n`.
   Registered receivers obey channel FIFO. An arriving sender wins according
   to scheduler event order; unrelated readiness events have no ordering promise.
7. Duplicate underlying channels, including distinct endpoint aliases, throw
   `DuplicateChannelReceiver` during construction. Empty input throws
   `EmptyChannelSelection`. One receiver is supported as the degenerate case.
8. A second running `item()` on the same selector throws
   `ConcurrentChannelSelection`. A cold future reserves nothing. A committed
   request remains active until its driver publishes the result. Cancelling
   a rejected request does not affect the active request.
9. Compiler intrinsic registration follows `Receiving.item`, with load-time
   type/layout contracts. The result is an ordinary typed object. Receiver
   fields retain endpoints; compiler and runtime channel-carrier analysis also
   includes receivers reachable solely through selection, for resource ownership.
10. `Within` and `Before` own timeout scheduling and await cancellation cleanup.
    Selection introduces no timer. Their timeout boundary cancels the origin
    using the same commit precedence as explicit cancellation.

## Execution tracker

- [x] Read ticket, review, repository rules, std principles and runtime paths.
- [x] Write inline async and direct Rust driver tests before implementation.
- [x] Register and implement the constructor and atomic selection driver.
- [x] Verify cancellation, closure, fairness and overlapping FIFO behavior.
- [x] Verify ownership, GC roots and bounded obsolete registrations.
- [x] Verify timeout composition, examples and public declaration inventory.
- [x] Measure registration, cleanup, latency, memory and throughput locally.
- [x] Update code maps and record exact validation commands/results.

The single receive path stays unchanged. The sender's direct-handoff path gets
one selector-flag branch; unrelated channel operations never scan alternatives.
Construction, registration and cleanup cost O(n) plus FIFO queue removal cost
on the selected channels. Queue removal must preserve other waiters' order.


## Test matrix and evidence

Tests were added before runtime implementation. The first filtered Rust test
build failed because `Selection` and its state variant did not exist. The
initial timeout-boundary regression also failed with a committed message lost;
the inline fake-clock version returned `TimedOut` on the pre-fix release binary.

- Inline `async/channel` tests: one and several buffered sources, refill,
  unbuffered rendezvous before and after registration, no losing receive,
  cold/suspended/repeated cancellation, commit precedence, close during wait,
  closed buffers/exhaustion, two/three-source fairness over many rounds,
  occasional sources, duplicates/endpoint aliases, concurrent requests,
  one-source selection and string/float payloads, `Within` and `Before` cleanup.
- Direct driver tests: blocked sender refill, eager losing-queue removal,
  cancelled registration skipped before cleanup, overlapping-selector FIFO,
  close/send/cancel ordering, cold cancellation, concurrent pending and committed
  requests, malformed layouts and mismatched payload contracts. Seed `509`
  drives 512 bounded repeated interleavings with 2..8 alternatives.
- Ownership integration: fresh winners, intact losing messages, borrowed/shared
  payloads destroyed exactly once, unread payload cleanup, committed rendezvous
  after cancellation, and a resource-owning result across the timeout boundary.
- GC: 128 cycles, each with a collection while parked and after completion or
  cancellation; watched selector states disappear, live bytes return to the same
  baseline, and all channel queues are empty. Collections poison freed cells.
- Cleanup contains no safepoint, user call or scheduling point: cancellation
  cannot interleave halfway through cleanup. Both sides of this atomic interval
  are tested instead of asserting an impossible internal interleaving.
- The producer/consumer example and new OCS fixture run through a focused CLI
  integration test. The OCS fixture is available to the existing Sparrow and
  Sparrow GC-stress CI configurations. This checkout uses
  `SPARROW_CORE_GC_STRESS=1`, not a Cargo feature of that name.

## Measurement method

`channel_single_bench.ok` compares the unchanged ordinary handoff against the
candidate on the same tree. The control build temporarily replaces only the
selector flag branch with the original `complete_wait` call; the exact candidate
source is saved within `target-509` and restored before final verification.
Both builds use the requested offline release profile, 4 build jobs and fat LTO.
Each sample does 16,384 ordered receives; the first of seven samples is warmup.
The two binaries are then alternated. These are local micro measurements, not
an entire release performance gate.

`channel_select_bench.ok` measures public-API throughput for 2, 8 and 32
alternatives: all buffered sources ready, mostly idle sources, and capacity-zero
rendezvous. The direct Rust cost probe isolates registration and cancellation
cleanup and counts heap allocations. It uses 512 rounds per configuration and
no timing thresholds. A resource-free ready round allocates exactly one frame
and one result object (128 heap bytes here), independent of alternative count.
The final direct-driver probe (debug Rust, not release timing) reports:

| Alternatives | Registration, ns | Cancellation cleanup, ns | Heap allocations / ready round | Heap bytes / ready round |
|---|---:|---:|---:|---:|
| 2 | 568 | 1,278 | 2 | 128 |
| 8 | 1,011 | 1,689 | 2 | 128 |
| 32 | 2,465 | 3,090 | 2 | 128 |

These small timing samples are diagnostic and have no acceptance threshold.
Configuration channel references use 8 bytes per alternative. Receiver queues
retain 32 bytes of capacity per otherwise empty channel after first use; that
capacity is reused, and no obsolete registration remains.


## Changed files

- `std/async/channel.ok`: API, typed failures, inline semantic tests and fake clock.
- `std/async/channel_single_bench.ok`, `std/async/channel_select_bench.ok`: bounded performance probes (no public objects).
- `okkama-rs/crates/okkama-compiler/src/lowering/intrinsics.rs`, `src/ir/mod.rs`: registration and ownership carrier types; no syntax/checker changes.
- `okkama-rs/crates/sparrow-core/src/drivers.rs`: selection and committed-origin timeout precedence; the ordinary receive body is unchanged, and the sender handoff gains one flag branch.
- `okkama-rs/crates/sparrow-core/src/primitives.rs`, `src/tasks.rs`, `src/intrinsics.rs`: persistent state, waiter flag, closure updates and validated construction.
- `okkama-rs/crates/sparrow-core/src/contracts.rs`, `src/failures.rs`: load-time safety and payload ownership.
- `okkama-rs/crates/sparrow-core/src/selection_tests.rs`: direct driver, GC, interleaving and allocation tests.
- `okkama-rs/crates/okkama-cli/tests/channel_pins.rs`: ownership, example and fixture integration tests.
- `examples/async/channel_selection.ok`, `.expected`: independently closing producer merge.
- `conformance/cases/L08-async-and-concurrency/channel.atomic-selection/{main.ok,case.json}`: portable semantic fixture.
- `conformance/contracts/stdlib.json`: regenerated public inventory.
- `docs/channel-selection-509.md`, `docs/map/{state,decisions,pipeline,files}.md`: decision/progress records and regenerated code maps.

## Exact validation commands

Working directory `/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/okkama-rs`:

```sh
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-509 cargo check -p okkama-compiler --offline -j 4
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-509 cargo check -p sparrow-core --offline -j 4
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-509 cargo test -p sparrow-core --offline -j 4 --lib selection_ -- --nocapture
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-509 cargo test -p okkama-cli --offline -j 4 --test channel_pins
SPARROW_CORE_GC_STRESS=1 CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-509 cargo test -p okkama-cli --offline -j 4 --test channel_pins
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-509 cargo build -p okkama-cli --release --offline -j 4
```

Working directory `/Volumes/SamsungSSD/stuff/okkama-work/l-codex2`:

```sh
OKKAMA_STDLIB_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/std /Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-509/release/okkama run -t std/async/channel.ok
SPARROW_CORE_GC_STRESS=1 OKKAMA_STDLIB_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/std /Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-509/release/okkama run -t std/async/channel.ok
OKKAMA_STDLIB_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/std /Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-509/release/okkama run -t std/async/timeout.ok
OKKAMA_STDLIB_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/std /Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-509/release/okkama run -t std/async/collect.ok
OKKAMA_STDLIB_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/std /Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-509/release/okkama run -t std/async/channel_single_bench.ok
OKKAMA_STDLIB_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/std /Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-509/baseline-okkama run -t std/async/channel_single_bench.ok
OKKAMA_STDLIB_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/std /Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-509/release/okkama run -t std/async/channel_select_bench.ok
python3 tools/map/gen.py
```

Working directory `/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/conformance`:

```sh
python3 tools/inventory_std.py ../std --write
```

Only focused tests are run locally. No whole suites, Docker, git operations,
CHANGELOG changes or syntax extensions. `target-509` was removed after the
measurements and final verification. There are no remaining API questions;
all review choices are recorded above. The full existing CI gate is outside
this task's allowed local commands.


## Final results (2026-11-22)

- Both `cargo check` commands pass. The final release build succeeds without warnings.
- 13 direct Rust tests pass; 512 seed-509 rounds and 128 GC lifecycle cycles are included.
- All 15 `channel_pins` integration tests pass normally and under GC stress (5 new tests, 10 existing).
- All 14 inline channel tests pass normally and under GC stress (12 new tests, 2 existing).
- Existing timeout tests: 2 pass. Existing collect tests: 1 passes.
- Both bounded performance probes pass. The new example and OCS fixture pass through the CLI integration test.
- Public inventory: 502 modules, 4,232 declarations, 4,856 public actions.

Release single-channel A/B: five alternating pairs, 30 non-warmup samples per
configuration. The only control-source difference is the removed selector
branch. Candidate source was restored exactly before the final build and tests.

| Capacity | Control median, ns / receive | Candidate median | Change | Control range | Candidate range |
|---|---:|---:|---:|---:|---:|
| 1 | 239 | 231.5 | -3.1% | 179..295 | 176..269 |
| 0 | 241.5 | 246 | +1.9% | 191..261 | 197..301 |

There is no observed significant regression in these noisy local samples.
The ordinary receive source is unchanged; the direct sender handoff adds one
selector flag branch. Buffered sends with no waiting receiver pay no new check.

Release selection: median of three runs of 16,384 receives (ns / receive):

| Alternatives | All buffered sources ready | Mostly idle | Capacity-zero rendezvous |
|---|---:|---:|---:|
| 2 | 208 | 216 | 257 |
| 8 | 205 | 209 | 221 |
| 32 | 219 | 246 | 258 |

These are end-to-end loop costs, including sending/refilling and value checks,
not just the receive driver's instructions. Allocation counts, queue capacity,
GC reclamation and direct setup/cleanup measurements are recorded above.

All required semantics and applicable review phases are complete. Phase A is
replaced by the owner's requested decision record; Phase C covers Sparrow and
its GC-stress configuration. No separate engine remains to implement. No open
questions or implementation blockers remain. Full CI is not claimed as a local
run; the added tests and fixture participate in the existing CI paths.

Code maps regenerated: `files.md` updated, `rules.md` verified current. The
entire temporary `target-509` directory (including control binary and source
backup) was deleted and its absence verified. No actionable checklist remains.
