# C3a — anonymous creation and known invocation (#569 / #641)

High-only producer evidence, on `feature/569-lower-callables`, base `e8ef7534`.
The own checkout was read only: commit-4 inputs are in
`565/okkama-rs/crates/okkama-oir/src/own/tests/protocol-high/closure-*.oir`,
not `protocol-goldens/closure-*.high.oir`. No Own rewrite of these new sources or
runtime/oracle acceptance is claimed. All 41 C3-owned TSV rows remain `planned`.

## Implemented and checked

- [x] C2a lexical body keys reserve distinct lambda functions before their helpers/bodies.
- [x] `fn.closure` snapshots the current SSA values, receiver first when used, then C2a captures.
- [x] Capture prefix appears in the lambda signature and `[lambda captures N owner ...]`,
  but not in the public `fn` type. Lexical owner authority survives without a receiver capture.
- [x] Each evaluation remains a separate `fn.closure` evaluation; bodies are not action identities.
- [x] Known invocation emits `fn.call` with Borrow arguments/environment, `summary=None`,
  `packet=None`, empty deps/descriptors/type arguments, unelaborated ABI and an H5 effect envelope.
- [x] A conservative, monotone producer gate tracks finite origins across SSA edges/backedges,
  direct-call inputs/results, known lambda inputs/results, captured callables and lazy defer bindings.
  It emits no summary, entry specialization or ownership adapter; those remain Own's responsibility.
  Unknown projections/inputs poison joins. Bottom/unseeded cycles never establish a known callable.
- [x] Open invocation reports `Unsupported` at the actual invocation span, pending audited O1.
  No identity-only Mopen, descriptor or fake summary was introduced.
- [x] Every lambda has an Action scope and its own return boundary, failures, catches and D11 exits.
  Lambda defer helpers use the enclosing lexical key, so different Closure owners cannot collide.
  Receiver authority and lazy binding reads remain available inside lambda defers.
- [x] Frontend must-use proof runs before lowering. Captured successors use `succ.take` at creation,
  then ordinary capture parameter/binding types; there is no `succ.erase` or frontend proof bypass.
- [x] `val.eq`/`val.ne` preserve action-handle equality; aliases share the SSA value. Two calls of
  one factory evaluate the same lambda body twice. Equality through arrays is also preserved.
- [x] Existing Unsupported tests were narrowed to remaining unsupported features; anonymous
  actions inside functional CFG now have separate callable boundaries.

## Small stored goldens

Sources: [fixtures/p2_4/anonymous](../fixtures/p2_4/anonymous/).
Expected High: [goldens/p2_4/anonymous](../goldens/p2_4/anonymous/).
The inventory is exact, with 20 independent sources, each under 125 printed High lines.

| Golden | Evidence |
| --- | --- |
| closure-empty | No captures, ordinary known call |
| closure-known | Bound resource snapshot and known call |
| closure-fresh | Escaping closure of a locally created resource, preserving lexical bind |
| closure-borrowed | Outer resource borrowed through an inner branch and closure result exit |
| closure-receiver | Whole receiver capture and explicit private-field owner authority |
| closure-nested | Separate outer/inner functions and snapshot prefixes |
| closure-loop | Creation inside iteration, current iteration value captured |
| closure-resource-result | Known lambda creates and returns a fresh resource through D11 |
| closure-failure | Failure stays inside lambda until invocation, caller catches it |
| closure-return | Explicit return exits lambda; enclosing main continues |
| closure-equality | Alias equality, distinct evaluations of one body, aggregate equality |
| closure-defer | Replacement return belongs to the lambda's own boundary |
| closure-snapshot | Closure operand is pre-rebind SSA; rebind has a distinct SSA operand |
| closure-parameter | Escaping snapshot of an explicit parameter |
| closure-known-phi | Same resource-capturing callable on both arms |
| closure-known-join | Finite set of two anonymous targets, no open contract |
| closure-known-loop | Resource-capturing callable stays known through loop-carried environment |
| closure-known-parameter | Direct known caller supplies an action parameter |
| closure-successor | Frontend carried-use proof and explicit successor capture conversion |
| closure-captured-callable | A lambda calls a captured known action |

`oir2_c3a_anonymous_source_goldens` verifies High before printing, after parsing,
and for every stored golden, checks normalized round-trip and canonicalized stored text,
and lowers each source twice for determinism. The long defer registration is wrapped
at an argument boundary in stored text to keep every line within 120 columns. `oir2_c3a_golden_inventory_and_snapshot_operands`
checks the exact source/golden inventory and the pre-rebind capture operand directly.
Additional focused tests cover generic/nested signatures, known lambda parameters,
defer captures/receiver/failure, intended must-use negatives and alias invocation.
Open negatives include an uncalled callable parameter, a field projection and a mixed known/open join.

## Commit-4 protocol comparison

“Same” refers to op/ABI/retention, allowing source names/IDs, lexical bindings/scopes,
constructors, D11 and effect metadata. It never means byte-for-byte module identity or Own acceptance.
The broader C1 matrix remains the full C2–C9 target; this table records actual C3a sources.

| Source golden | Commit-4 High input | Same? | Exact reason |
| --- | --- | --- | --- |
| closure-empty | closure-empty | yes | Empty `fn.closure`, capture-free lambda ABI, summary-free `fn.call`; empty retained capture graph |
| closure-known | closure-known | yes | Bound R capture is borrowed, capture-prefix ABI and retained graph unchanged |
| closure-borrowed | closure-borrowed | no, whole input | Borrowed arm has the same snapshot ABI/retention; source additionally joins an empty lambda in the other arm, giving a finite two-origin set |
| closure-fresh | closure-fresh | no, retention boundary | Source must bind local R before snapshot; manual input offers unbound fresh R directly. Source snapshot borrows R, and its local graph escapes by normal result exit |
| closure-known-phi | closure-known-phi | yes | Both arms forward the same resource-capturing action; no open summary or dynamic fallback is requested |
| closure-known-join | closure-known-join | no, target kind | C3a joins two lambdas. Manual input joins lambda and `fn.bind`; bound actions remain C4 |
| closure-known-loop | closure-known-loop | no, exact CFG | Same resource capture/known call ABI; source carries the lexical environment through range-for, manual fixture explicitly forwards closure/index through a different backedge |

Other C3a goldens have no matching commit-4 closure protocol input; they are additional source regressions.
`closure-loop` creates a new value per iteration, unlike manual `closure-known-loop`, which creates once.
`closure-open`/`dynamic-closure` are deliberately not produced: O1 remains a separate audited branch step.
The fresh/borrowed/join/loop source differences require their own future Own goldens.

## Coverage and boundaries

C3-owned TSV subcases link to this evidence and identify remaining frontend/full-input or
Own/backend/#593 gates. Their status stays `planned`; partial source fixtures do not close full inputs.
Captured-successor negatives remain intended frontend errors. This high-only step does not close
C3's Own integration, O1 open invocation, C4 bound actions or the full #569 roadmap.

## Verification

All commands ran from `okkama-rs/`, offline with `-j 4`.

| Command | Final result |
| --- | --- |
| `cargo check --workspace --all-targets --offline -j 4` | PASS |
| `cargo clippy --workspace --all-targets --offline -j 4 -- -D warnings` | PASS, no warnings |
| `cargo test -p okkama-compiler --lib oir2 --offline -j 4` | PASS: 161 tests, 118 filtered |
| `cargo test -p okkama-oir --offline -j 4` | PASS: 178 unit + 29 integration; 0 doctests |

The final compiler run used stored goldens without an update variable. Focused C3a checks also
passed independently: 5 tests, including all 20 source High goldens. Initial failures identified
obsolete Unsupported expectations, a non-expression source block, successor entry/binding types,
and a case-sensitive negative assertion. Those were corrected and reverified. Strict clippy's
single-element-loop warning was fixed by retaining the async negative as a single source check.

`git diff --check`, code/fixture/golden line widths and scope restrictions pass. All 162 manifest
statuses remain `planned`, and all 41 C3 rows link to this evidence. No `cargo fmt`, network, Docker,
CLI execution or commits were used. No changes to `okkama-oir`, its verifier/own/contracts, CHANGELOG
or documentation outside coverage. HEAD remains `e8ef7534`; Claude review remains external.
