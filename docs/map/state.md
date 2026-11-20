# State (update at the end of every task)

Last update: 2026-11-20, after wave/78. The working branch of 0.6.0 is `release/0.6.0`; develop is behind it.

## Branches and worktrees (`../okkama-work`)

| Worktree | Branch | What | Status |
|---|---|---|---|
| w4 | release/0.6.0 | merge point (fa45220ba = wave/78) | merges only after a green Wave CI |
| dev | feature/devtrace | okkama-devtrace (wave/79) and these maps | CI pending |
| sch | feature/847-schedule | **parked** fix for SPB_SCHEDULE: an offered binding with service entries makes its owner dynamic (4a6c0faee) | no test; frozen pending the ownership decision |
| cp | feature/754-cancel-docs | old | unknown: check before use |
| r060 | wave/18 | old | unknown: check before use |

## Champ, in numbers (Wave CI, run 44735863424, release before wave/77)

- Conformance: 1022/1046 after wave/77, 0 timeouts.
- Remaining blockers:
  - async destroy D1 (6);
  - channels Q2 (7);
  - TCP race SPB_SCHEDULE → FRESH_PACKET (5);
  - own budget on SQLite (2);
  - smaller items.
- Corpora:

  | Corpus | Champ fails |
  |---|---|
  | t/ | 78 of 141 files |
  | Bible examples | 90 of 240 |
  | examples/ | 29 of 69 |
  | micro | 4 of 75 |

  Sparrow passes all of them, including 50 729 t/ checks.
- Failure buckets, filed as sub-issues of #894:

  | Ticket | Bucket | Cases |
  |---|---|---|
  | #895 | diagnostics printed as Debug | 61 |
  | #896 | closure identity (wrong output) | 2 |
  | #897 | runtime memory budget | 42 |
  | #898 | own budget | ~30 |
  | #899 | SPB generation rejects its own output | — |
  | #900 | Atomic of objects or arrays | ~20 |
  | #901 | verifier and lowering rejections | — |
  | #902 | unsupported builtins | — |

- First devtrace reading (a 50-line race, debug build): verify_load takes 21.5 s of about 34 s, own's analysis of
  main takes 1.87M work units, and the native-contract catalogue takes 1.7 s.

## Tweed (decided 2026-11-20): phase 0 measurements

Setup: Tomato v0.5.8 (Linux toolchain `okkama-work/micro-tc/v0.5.8`, `--engine sparrow`: the default there is
still IR) and CPython 3.13 in the same `okkama-dev` container, 1 CPU. Micro-v2 protocol, oracles checked.
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

## Next

1. Merge wave/79 (devtrace + maps) after a green CI.
2. Tweed phase 1 on Tomato's pipeline (owner to confirm the base): per-instance managed bit and summaries.
3. Meanwhile: work through the bulk buckets that do not depend on the ownership model (#895 diagnostics,
   #900 Atomic, #902 builtins, #896 closure identity).
