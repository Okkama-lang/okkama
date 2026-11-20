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

## Ownership redesign (open, owner's decision)

- Survey: scratchpad `ownership-survey.md` (session of 2026-11-20). Its recommendation is A.
  - A is a runtime owner tree, syntactic lowering and one `ElaborateOwnership` pass with drop flags.
    Uncertain cases stay dynamic.
  - Verification is one checker in debug and CI only; the loader trusts the bytecode.
  - B is A without the static pass and is phase 1. Perceus-style refcounting for destroy is rejected:
    it releases at the last reference, while Okkama releases at the owner scope.
- The decision document goes to the owner; nothing is rewritten before the decision.

## Next

1. Merge wave/79 (devtrace + maps) after a green CI.
2. Write the ownership decision document for the owner.
3. Until then: work through the bulk buckets that do not depend on the ownership model (#895 diagnostics,
   #900 Atomic, #902 builtins, #896 closure identity).
