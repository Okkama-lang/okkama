# State (update at the end of every task)

Last update: 2026-11-20, after wave/78. The working branch of 0.6.0 is `release/0.6.0`; develop is behind it.

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
9. #817: the root region and root closer for Futures that outlive their owners (wave/90). Next: Tomato bugs #845,
   #797, #769, #587, #846, then #907, #910, #912, #913.
