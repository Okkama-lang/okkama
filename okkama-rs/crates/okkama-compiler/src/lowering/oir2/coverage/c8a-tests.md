# C8a (#646): test profile without skip, High only

C6 remains blocked on the descriptor_cleanup contract from feature/565-own.
This step implements F1 using existing IR forms. S1 is not implemented.
All p2_4.tsv rows remain planned: source High evidence is not full conformance,
Own, backend, runner, #593 or async runtime acceptance.

## Forms and boundaries

- [x] Existing desugar_test_items produces ordinary fake objects, concrete
  constructors/default bodies, successor fields and role witnesses. No fake.new,
  fake.storage or hidden storage owner is emitted. Each constructor evaluates
  defaults anew. Existing Cell/Atomic language rules are unchanged; this step
  adds no storage protocol or storage catalogue entry.
- [x] BodyOwner::Test(index) declares a deterministic $testN function with
  signature () -> void and its own Action root. There is no receiver or object
  authority. Frontend registry order and qualified module/object/role names
  determine Module.tests, independently of body discovery order.
- [x] Test mode emits Feature::Test and Some(tests), including Some([]).
  Production mode emits None and no test feature, bodies, fake declarations or
  fake witnesses. C2a registry rebuild continues to preserve the selected mode.
- [x] Callee::Test lowers the exact four existing registered core operations:
  test.ok, test.is, test.isnt and test.fail in ops/core.rs and ContractRegistry.
  Their entry kind is ObservationAdmissionStaysCaller, operands are Observe,
  and failed assertions record case state with normal continuation. Arguments
  evaluate left to right using existing successor conversions. A failing argument
  still follows the ordinary H5 unwind route. The existing Program failure
  envelope (including a missing current-case context) is preserved as well.
  Helpers use the runner's dynamic current-case context and do not acquire a separate test identity.
- [x] async test retains FunctionAttr::Async, Feature::Async and separate High
  creation/body envelopes. Its parameterless void body has no input or returned
  language graph; creation returns future<void>. The body envelope permits
  suspension and failure. This is an async identity/body fixture without await,
  not a synchronous encoding of async execution. Await, async ordinary actions,
  Future ownership, scheduling and async cleanup remain #570/#565-c5 gates.
- [x] test.skip is explicit Unsupported with its source span and pending audited
  S1 contract. No marker, summary or fake transport is invented. close/text/bytes
  remain explicit Unsupported outside this four-core-assertion slice; their
  typed diagnostic contracts are a later C8 step.

## Entry and isolation contract

The runner selects Module.tests in test mode, never Module.entry/main. It invokes
only each listed () -> void test function in table order. An async attribute
requires the future runner to start and await the case before proceeding.
Some([]) is an empty-root-suite error, not a successful zero-case run and not a
fallback to main. None is a production module and uses the normal entry.

Before every case, the runner installs a fresh assertion/report context and a
fresh invocation/ownership root. It executes the case, waits for all required
cleanup, records pass/fail, then discards that context before the next case.
A failed assertion records failure and continues the case; a thrown/program/
panic/native failure exits through cleanup and fails that case. The next case
still runs. Status is 1 if any case failed, otherwise 0. Skip is unsupported here.

Producer evidence establishes distinct roots and fresh constructor/default calls
in each case, with state in ordinary successor fields. It does not execute or
certify the runner, runtime cleanup, Future protocol or process exit status.
Imported ordinary helper bodies remain available; tests from imports are not
added to the root suite. main may remain declared, but is not in the test table.

## Evidence

Six stored goldens live in goldens/p2_4/tests, with five source fixtures in
fixtures/p2_4/tests; production reuses the exact fake-successor source.
Every source module is independently verified in the correct execution mode
before printing, after parsing and as stored text. A second independently
verified lowering must print identical text. Stored lines are at most 120 columns;
goldens are limited to 220 lines. No verifier budget is increased.

| Golden | Evidence |
| --- | --- |
| fake-successor | Ordinary fake/ctor/default/witness; successors append call arguments to @int |
| async | Parameterless async test identity, two summaries, core assertions |
| assertion-failure | All four assertions, helper assertion, normal continuation after failure, H5 argument failure |
| isolation | Two case roots construct independent fake state, defaults evaluated in each constructor call |
| empty | Some([]) with a separately declared main, preserving empty-suite distinction |
| production | Same module as fake-successor, with tests/fake stripped and ordinary main retained |

Additional regressions check qualified source order, generic Echo<int> fake
witnesses, successor assertion conversions, imported helpers without imported
test roots, production verification rejection of test operations, and explicit
Unsupported for skip and unregistered typed assertion families.

## Verification

Final verification commands run from okkama-rs/, offline with four jobs.
No own/verifier/contracts source, CHANGELOG or documentation outside coverage
is changed. No formatter, other suite, standalone binary, Docker, network or
commit is used.

- [x] cargo check --offline -j 4 --workspace --all-targets: exit 0.
- [x] cargo clippy --offline -j 4 --workspace --all-targets -- -D warnings:
  exit 0, no warnings.
- [x] cargo test --offline -j 4 -p okkama-compiler --lib oir2:
  exit 0; 180 passed, 0 failed, 0 ignored, 118 filtered out. Final run uses stored
  goldens without OKKAMA_OIR2_C8A_WRITE_GOLDENS. The generation run also passed.
- [x] cargo test --offline -j 4 -p okkama-oir: exit 0; 178 unit tests and
  29 integration tests (4 + 2 + 13 + 10), 0 doc tests, no failures.
- [x] git diff --check, source/golden width and scope checks pass. Six goldens
  have 18–147 lines. All 162 manifest rows remain planned; the 22 C8 rows link
  this record with explicit evidence boundaries.
- [x] Protected own/verify/contracts sources and HEAD (9b9b09fb) are unchanged.

Initial fixture checks caught a forbidden role rebind, malformed byte syntax,
and mismatched assertion operand types; the source fixtures were corrected.
The catalogue check also caught the incorrect expectation of an empty failure
set: assertions retain the existing Program failure envelope. Generated fake
signatures are wrapped outside quoted identifiers for readable stored text.
No verifier or contract relaxation was used to obtain passing goldens.

C8a High scope is complete. Full C8, S1 skip audit, typed diagnostic assertions,
Own/runner/backend/#593 evidence and async execution remain separate work.
