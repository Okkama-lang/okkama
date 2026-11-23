# State (update at the end of every task)

## Release 0.6.4 (2026-11-23, release/064)

- [x] Hotfixes from reading 0.6.3 end to end (milestone "Hotfixes TBD", board project 1): #973–#1004
  fixed, each with a unit test and a mutation check, blast radius measured over std, t/, examples and
  conformance; CI waves 1–7 on release/064.
- [x] Deferred: #1000 (macOS TLS flake, not reproducible), #1005 (owner decision: overload names,
  object named Maybe), #1006 (refactors, Unicode version contract, formatter width and std fmt-clean).
- [x] wave/rel064 green (it caught t/gen against #984 and a clippy lint, fixed in b09f9dfa8); release CI
  green; micro A/B 0.6.3 → 0.6.4 interleaved: −0.4% over 51 reliable pairs, all within noise
  (okkama-docs reports/2026-11-23-v0.6.4/micro.html); version 0.6.4.
- [ ] Tag v0.6.4 (owner pushes), Packages and GitHub Release, install-pgo, man site, cleanup.
- [x] #1016 (feature/1016-twins): runtime-core native twins removed (network, tls, sqlite, db, postgres,
  host_crypto, process, input, system, stdio, buffer, encoding, numbers, bytes, jre; regex, fs, json, idna,
  net, text, misc, formats, host cut to their live parts), ~14.9k lines; `native::execute` refuses
  unrouted natives; a sparrow-core test keeps the ORIGIN routes and runtime-core providers equal.

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
