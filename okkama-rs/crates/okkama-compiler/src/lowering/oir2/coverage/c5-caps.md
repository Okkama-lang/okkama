# C5 — capabilities, High only (#569 / #643)

Base: d374c8ff. This record covers the source producer only. Own elaboration, runtime lifetime
observations, #593 and Claude review remain separate gates. Every p2_4 status stays `planned`.

## Implementation and decisions

- [x] Reserve nominal cap declarations after concrete object identities, before resolving signatures
      and recursive field types. Each declaration records the concrete owner type and its ordered
      computed-field function references. Internal and external caps use the same resolved THIR ABI.
- [x] Reuse C2's specializer and C2a's `CapField` discovery. Concrete owner identity and source cap
      index distinguish generic instances. No second specialization path or residual generic is added.
- [x] Declare each computed field as an independent function with owner/environment parameter 0,
      Unelaborated ABI and its own Action root. Ordinary return, failure, defer, functional-block and
      nested-closure lowering applies. Owner-field access becomes `obj.get` on that parameter;
      external `Owner::field` uses the same resolved field index without source-name re-resolution.
- [x] `CapSelect` evaluates and takes its owner once, then emits `cap.make owner, ty concrete-cap`
      with no packet. It retains the complete owner SSA value, rather than a projection of fields
      read by the view. All owner fields, including hidden resources and their transitive graphs,
      remain in the concrete object declaration. Ownership elaboration will assign retention/deps.
- [x] `CapRead` emits a separate `call.cap cap, field_ref(declaration, index)()` on every read.
      The cap operand uses Borrow; explicit arguments, packets, deps and summary are absent in High.
      The computed target's owner-prefix ABI is parameter 0. There is no eager computation or cache.
      Calls carry the same conservative effect envelope and H5 unwind as ordinary summary-free calls.
- [x] Rebinding or taking a successor of the source does not replace an existing cap's SSA owner.
      New caps use the new owner. Caps pass through ordinary parameters, results, immutable fields
      and C2 generic transport without an adapter or changed nominal identity.
- [x] C3a/C4 finite-callable admission follows action results from computed fields. Unknown explicit
      cap inputs poison callable fields through their complete owner type graph; this does not
      authorize arbitrary open calls or supply O1. Closed action fields and returned closures work.
- [x] Frontend checking still precedes lowering. No changes to cap access, equality, construction,
      self spelling or owner-to-cap coercion rules are made.

## Source goldens and focused evidence

Exact source inventory: [fixtures/p2_4/caps](../fixtures/p2_4/caps/).
Stored expectations: [goldens/p2_4/caps](../goldens/p2_4/caps/).

| Golden | Contract |
| --- | --- |
| cap-hidden | Fresh Owner with hidden Res/destroy, cap returned from Factory, computed text reads only label |
| cap-call | Fresh owner becomes cap, computed-field call with exact field reference |
| cap-parameter | Ordinary method receives concrete cap parameter and invokes its computed field |
| cap-recompute | One acquisition, two distinct call.cap instructions on the same cap; body calls Probe.read |
| cap-external-internal | Separate internal/external declarations, external OwnerField reads private owner field |
| cap-generic | Distinct Owner<int>/Owner<string> owner types, cap identities and computed functions |
| cap-snapshot | Existing cap survives source rebind through successor; later cap uses the new owner |
| cap-field | Concrete cap stored in an ordinary object field and read by a method |
| cap-generic-transport | Generic Box stores, returns and echoes Owner<int>::View |
| cap-body | Computed body uses reduce, return and defer; second field returns a captured closure |

Every golden passes verify_high before printing, after parsing, and when loading stored text.
The helper checks normalized round-trip, repeat-lowering determinism, exact file inventory,
200-line size limit and 120-column stored text. The ten stored goldens span 62–184 lines;
generic symbols wrap only at whitespace outside quoted identifiers. Focused tests check full hidden-resource layout,
owner ABI, old/new SSA operands and two independent computed calls. A paired finite/open callable
field test checks that cap transport does not bypass the pending O1 restriction.

The recomputation test is structural High evidence; no runtime counter or stdout observation is
claimed. The frontend requires a local snapshot before a nested closure can use a cap-owner field;
the fixture follows that resolved THIR rule. `defer` uses its expression syntax.

## Comparison with #565 commit 4 inputs

Read-only comparison against feature/565-own protocol-high `cap-hidden`, `cap-call`, `cap-parameter`:

| Commit 4 input | Producer witness | Comparison |
| --- | --- | --- |
| cap-hidden | cap-hidden | Same complete Owner(hidden Res, label), cap.make on the whole fresh owner, cap result exit |
| cap-call | cap-call, cap-hidden | Same cap.make + call.cap field_ref, computed owner parameter 0, Borrow and no High packet |
| cap-parameter | cap-parameter | Same concrete cap parameter and summary-free call.cap; no caller-side owner extraction |

Source differences are explicit: symbols are qualified; real constructors, lexical binds, scopes,
source locations and D11 records are retained. The resource has an empty synchronous destroy and the
source Factory has an ordinary receiver. The small cap-call owner is plain and its computed result
reads n + 1; resource/full-owner retention is covered separately by cap-hidden. cap-parameter uses
Owner(n) rather than the manual resource R. These change neither cap op/ABI nor retention rules.
No Own fixture, summary, adapter or cleanup-order certificate is generated by this High-only task.

## Coverage and remaining gates

All C5 rows in p2_4.tsv point here with status `planned`. The matrix records the actual High paths.
Full conformance inputs, corpus observations (hidden resource retention, duplicate alias cleanup,
successor cleanup, original snapshot checkpoint), Own integration and runtime execution stay gated
on the corresponding follow-up work. They are not certified by these small source fixtures.

## Verification

All commands run from `okkama-rs/`, offline with four Cargo jobs. The final compiler run uses
stored goldens without an update variable.

| Command | Result |
| --- | --- |
| `cargo check --offline -j 4 --workspace --all-targets` | PASS, exit 0 |
| `cargo clippy --offline -j 4 --workspace --all-targets -- -D warnings` | PASS, exit 0, no warnings |
| `cargo test --offline -j 4 -p okkama-compiler --lib oir2` | PASS, 173 tests, 118 filtered, exit 0 |
| `cargo test --offline -j 4 -p okkama-oir` | PASS, 178 unit + 29 integration, 0 doctests, exit 0 |

Initial source-fixture syntax/scope errors and long concrete-symbol lines were corrected before
the final stored-golden run. Protected verifier, own and contracts files were not changed.
`git diff --check`, all modified/new Rust/Okkama/High line widths and the exact scope check pass.
All 18 C5 rows link here; all 162 p2_4 rows retain status `planned`. The earlier #568 builder
record `coverage/c5.md` is preserved. No docs outside coverage, CHANGELOG, formatter, additional
suite, standalone program, Docker, network or commit is part of this work. HEAD stays d374c8ff.
