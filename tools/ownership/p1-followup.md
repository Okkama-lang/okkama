# #593 follow-up: P1 backend fixes and own diagnoses

Baseline: `bd193116`, branch `feature/593-oracle-two-toolchains`. No changes to
`okkama-oir/src/own/` or `okkama-oir/src/verify/`. No commits, network, CLI program runs,
Docker, conformance/stress suites, CHANGELOG changes or cargo fmt.

## Execution tracker

- [x] D19 `deps.of`: defined zero-word location, transitive proof retention, golden and execution test.
- [x] Closed P1 native/intrinsic contracts, builtin layouts, runtime adapters and isolated tests.
- [x] A: exact High collection inventory and proposed own/verifier behavior.
- [x] M: per-function counters and explanation of the rejected budget charge.
- [x] Required commands and refreshed per-fixture stage report. P1 remains red as recorded below.

## 1. D19 and backend contracts

`deps.of` is SPB opcode 40: one non-linear Deps result and one language-value anchor.
The result remains a real SSA definition, with a dependency edge to its anchor. The existing
dependency closure keeps that anchor and its proof roots live through downstream uses.
The structural/load gate rejects a missing anchor dependency. The JIT emits no storage or helper call:
`service_layout(Deps).words == 0`. The producer still advances the abstract PC and relocates roots normally.

Golden: `okkama-compiler/src/codegen/sparrow_oir2/fixtures/deps.spb17`.
Tests: `oir2_pipeline_deps_golden` (including missing-retention negative), `p1_pipeline_deps_run`
(real source, verified Low, SPB codec/load, JIT, exit 7 and `app.R 1` trace).

`spb17::p1::P1Contracts` constructs trusted signatures from the finite host catalog and checked
object descriptors, independently of artifact function declarations. Both compiler `compile_low`
and runtime decode/load use it. Unknown symbols and changed retention/signatures are rejected.
The OIR native catalog is independently checked again against the elaborated signature: language
parameters, erased deps parameters, and the explicit packet positions are validated.

| Entry | Machine contract | Failures |
|---|---|---|
| Atomic<int|string>.new_value | owned, `(T, packet) -> Atomic<T>` | none |
| Atomic<int|string>.value | observation, `(Atomic<T>, deps) -> T` | Thrown |
| Atomic<int|string>.replace | observation, `(Atomic<T>, T, deps) -> void` | Thrown |
| Atomic<int|string>.close | observation, `(Atomic<T>, deps) -> void` | none |
| Error.text | observation, `(Error, deps) -> string` | none |
| utf8_encode | owned, `(string, packet) -> bytes` | Native |
| stdout_write | owned, `(bytes, packet) -> int` | Native |

Atomic storage is `[T, closed: bool]`, without resource identity or automatic destroy.
Close is idempotent and clears T to UNBOUND. Closed value/replace construct the actual
`AtomicClosed.closed` enum payload before exposing Thrown; replacement does not store on failure.
Error references and byte arrays are traced language references. Enum labels and case-object payload
layouts supply Error.text formatting; the SPB object codec carries `error_cases` metadata. Existing
SPB goldens gained the empty metadata field. Enum errors use a tag; payload errors use their case fields.
Native providers reuse Sparrow UTF-8, stdout capture, output limits and panic isolation. A private
four-string NativeFailure carrier gives host failures the Native kind declared by the contract.
Fatal/exhaustion statuses stay terminal. No such carrier receives a resource serial.

SPB opcode 41 (`invoke.observe`) preserves observation results and explicit failure edges, without
creating or consuming a packet. Observation ABI arity counts only language words, excluding zero-word deps.
The forced-GC observation test covers both normal and explicit unwind edges, including failure tracing.
Existing owning calls continue to use CallOwn/CallPost. The runtime native adapter handles transport
from the actual entry ABI, independently of the operation name.

Atomic.replace remains an observation for this run, as requested. To implement OIR-2 §7.2, change
its compiler summary to CallBoundary and its trusted SPB catalog entry to Owned, then update ABI tests.
That adds the empty packet transport for plain T and selects invoke.call.own. The cell operation and
pre-store closed check need no change. No resource transfer behavior is inferred from its current observation status.

## 2. Diagnosis A: actual Stdout path

The supplied description mentions byte/char/int arrays. The measured 25-fixture path also requires
`array<str>` because Stdout.writeln receives variadic strings. It uses the native UTF-8 encoder,
not a source UTF-8 loop. No char-array or int-array operations occur on this path.

| High op | Actual operand/result types | Function | Sites across 25 modules |
|---|---|---|---:|
| array.new | string values -> array<str> | main | 64 |
| array.new | string values -> array<str> | app.Observer.destroy | 4 |
| array.len | array<str> -> int | io.stdout.Stdout.writeln#4 | 25 |
| array.get | array<str>, int -> str | io.stdout.Stdout.writeln#4 | 25 |
| array.len | array<byte> -> int | io.stdout.Stdout.write_bytes#6 | 25 |

In each linked module, writeln uses instruction i3 for array.len and i6 for array.get;
write_bytes uses i3 for array.len. Fixture-specific array.new sites and operand types are in
`p1-own-diagnostics.json`. The byte array itself is returned by runtime/utf@1::utf8_encode/1;
it is not constructed by High array.new.

Classification already gives these arrays `Plain` with a ClosedGraph certificate, no reachable
resources, destroy entries, service types, or unresolved descriptors. The failure is in the synchronous
producer dispatch/transfer handling, not a need to relabel all collections as managed.

Proposed #565 change, following OIR-2 §§3–4 and elaboration §7.2 Derived collections:

1. Recognize exactly ArrayNew, ArrayLen and ArrayGet for a complete plain element graph. Cover byte,
   char, int and string specializations; do not use this rule for residual or managed-element arrays.
2. ArrayNew keeps the collection operation, with `packet: None`, plain value facts and the union of
   the actual element deps. These fixtures have empty ownership deps. Preserve element positions and
   duplicate references; create no ownership token, journal entry, creation serial or collection destroy.
3. ArrayLen is an observation: no admission and no packet. Its integer result has empty deps;
   keep the input array rooted for the access.
4. ArrayGet is an observation: retain the selected element/full actual dependency evidence,
   without taking/admitting the array. Preserve the existing bounds failure and unwind routing.
   For these plain independent strings, the ownership dependency set is empty; GC reachability remains.
5. The transfer analysis and rewriting driver must agree. Add positive source/High-to-Own cases for
   all three ops and empty/duplicate arrays; negative residual and managed-element cases stay checked.
6. Own verifier expectations: same result/operand types and bounds-failure schema, valid Plain certificate
   for the complete element graph, SSA dominance, dependency/lifetime evidence on branch/loop transport,
   no fabricated linear packet or early scope close. Dropping array/get roots must fail backend root checks.
   A future managed collection implementation must preserve selection formulas and displaced journals;
   it cannot be certified by this plain specialization.

## 3. Diagnosis M: rejected dense estimate before the solver

Observed **10**, not 11, memory failures at this HEAD. All are `main` of the fixture named below.
The limit is `OwnOptions.memory_limit = 256 << 20 = 268435456` bytes in `own/mod.rs`,
and the returned limit is `OwnLimit::Memory`. It is not VerifyLimits.memory_bytes or the
50000000 work/step limit (the verifier happens to have the same 256 MiB default).

Let N = SSA values, I = instructions, Sf = scopes of this function, Sm = scopes of the whole module,
Q = 1 + N + 2I + Sf*N, B = blocks, E = semantic edges, K = bindings. In own/analyze.rs:

```text
facts = N*(2Q+8) + Sm*Q*4 + K*(N+2Q)
charge = (B+E)*facts*64
```

The graph includes conservative implicit unwind/failure-point edges. The analysis first charges Q*128,
and then attempts the dense charge. Every failure below is at that latter call, before FlowState
initialization and the first worklist evaluation. Thus neither std writeln/UTF-8 execution, join
explosion, nor missing widening causes these observed failures. The domain is already finite.

Columns: B/E = blocks/semantic edges, Q = indexed equations, W = accepted charged work,
pre = accepted charged bytes before the failed request, dense = rejected request, all byte counts exact.

| Fixture (`main`) | B/E | N | Q | W | pre bytes | dense bytes |
|---|---:|---:|---:|---:|---:|---:|
| scopes-nested-shadowing | 18/58 | 52 | 433 | 623 | 63272 | 378176000 |
| branches-wanted-match | 35/118 | 105 | 1505 | 1867 | 208520 | 4552349760 |
| call-chains | 20/78 | 69 | 492 | 694 | 73312 | 702589440 |
| rebind-history-defer | 18/57 | 58 | 339 | 529 | 51072 | 302188800 |
| successor-staging | 23/73 | 85 | 645 | 961 | 92484 | 1068840960 |
| loops-next-last | 19/45 | 51 | 462 | 609 | 65252 | 342921216 |
| loops-valued-nested | 30/93 | 82 | 821 | 1105 | 117516 | 1638981888 |
| defer-order | 28/82 | 76 | 521 | 765 | 77592 | 923915520 |
| unwind-nested | 22/62 | 74 | 549 | 851 | 78736 | 666688512 |
| try-resource-results | 30/91 | 92 | 895 | 1237 | 126616 | 1896598528 |

The counters are deterministic charged allocation estimates, not process RSS or measured live heap.
`oir2_p1_own_diagnostics` uses lower_std, the real P1 catalog and the public per-function analysis API.
It records returned OwnStats and independently reconstructs the documented indexing/charge formula;
it does not patch own, change limits, run binaries, or certify an incomplete function.
Unsupported-op failures may return default stats; their zero counters must not be read as measured work.

Proposed #565 fix:

- Keep the 256 MiB and 50M-work defaults. Do not raise the default to cover a fictitious dense 4.55 GB
  matrix; a larger cap hides the accounting/model problem and still scales badly.
- Index resource equations only where resource/service/provenance evidence is possible. Plain SSA
  values still need availability/liveness handling, but do not need a fresh resource origin and one
  journal equation per lexical scope. Keep stable static origin/phi identities for managed values.
- Use only this function's scope set for per-function state dimensions. Unrelated std/action scopes
  must not multiply every state in main.
- Match accounting to the actual sparse FlowState representation: charge allocated map/set entries,
  shared bitset pages, vectors and retained states when created; include conservative container/clone
  capacity and scratch memory, and release scratch charges when freed. Separate cumulative allocations
  from peak live auxiliary memory. Do not simply delete the dense charge without replacement.
- Keep monotone joins/worklist and the work cap. Add deterministic just-at/just-below budget tests,
  a small main linked against many irrelevant std scopes, a loop with managed history, and these 10
  fixtures. Require exact unchanged results and no certificate on exhaustion. No widening is justified
  by the measured failures.

## 4. Verification and per-fixture outcome

All Cargo commands were run offline with -j 4 from okkama-rs. The Python test was also run from
okkama-rs, using ../tools/ownership/test_oracle_two.py (the repository-relative path is tools/ownership).

| Command | Result |
|---|---|
| cargo check --workspace --all-targets | PASS |
| cargo check -p okkama-cli --features champ-abi --all-targets | PASS |
| cargo clippy --workspace --all-targets -- -D warnings | PASS |
| cargo clippy --workspace --all-targets --features okkama-cli/champ-abi,sparrow-cli/champ-abi -- -D warnings | PASS |
| cargo test -p okkama-compiler --lib oir2 | PASS, 97 |
| cargo test -p sparrow-core --features champ-abi --lib champ | PASS, 80 |
| cargo test -p okkama-sparrow --lib spb17 | PASS, 40 |
| cargo test -p okkama-cli --features champ-abi --test p1_pipeline | FAIL 101, 5 passed / 25 failed |
| python3 -I ../tools/ownership/test_oracle_two.py | PASS, 24 |
| git diff --check | PASS |

The 5 passing P1 tests are inventory, scalar execution, unsupported-no-fallback, new deps execution,
and new Atomic execution. None is counted as one of the 25 source-fixture successes.
All 25 source fixtures pass High lowering/verification and still stop in own:
15 at A and 10 at M. Their downstream Low/SPB/JIT behavior remains unmeasured.

| Fixture | Stage | Reason |
|---|---|---|
| scopes-nested-shadowing | own | M: memory-limit |
| scopes-temporary-tail | own | A: array.new |
| branches-discarded-if-308 | own | A: array.new |
| branches-wanted-match | own | M: memory-limit |
| temporary-arguments | own | A: array.new |
| temporary-receivers | own | A: array.new |
| call-results | own | A: array.new |
| call-chains | own | M: memory-limit |
| rebind-history-defer | own | M: memory-limit |
| successor-staging | own | M: memory-limit |
| loops-next-last | own | M: memory-limit |
| loops-valued-nested | own | M: memory-limit |
| defer-order | own | M: memory-limit |
| unwind-late-argument | own | A: array.new |
| unwind-nested | own | M: memory-limit |
| destroy-failures | own | A: array.new |
| try-resource-results | own | M: memory-limit |
| partial-construction | own | A: array.new |
| secondary-failures | own | A: array.new |
| panic-unwind | own | A: array.new |
| direct-singleton | own | A: array.new |
| late-old-identity | own | A: array.new |
| mixed-merge | own | A: array.new |
| reserve-r-a | own | A: array.new |
| reserve-a-r | own | A: array.new |
