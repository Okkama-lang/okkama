# P2.3 producer coverage

The binding review decisions (renumbered C2–C9) govern this inventory. `p2_3.tsv`
contains 127 L05, four L09 and twelve ownership rows. Paths are repository-relative.
Comma-separated dependencies are cumulative; `none` means no external feature gate.
`#593,backend` records unavailable executable evidence, including the harness gate.
Every row remains `planned`: inventory is not conformance acceptance. Negative cases
retain their frontend rejection contract. Subcase selectors describe original checks;
no extracted fixture or acceptance of the whole source file is implied.

| Resolved THIR form | Concrete high operation / representation | Owning commit and golden / case target |
|---|---|---|
| `Object(id,args)`, `Callee::Ctor/Method` | Concrete object layout, `call`, `obj.get/new/init` | C2: `generic-object`, `generic-action`, `generic-defaults` |
| Generic field default / destroy body | Concrete default/destroy function and declaration link | C2: `generic-defaults`, `generic-destroy` |
| `Variant(id,args)`, `VariantNew/Tag/Get` | Concrete case layouts, `variant.new/tag/get` | C2: `generic-enum`; L05 variants and Maybe/Presence |
| Recursive generic references / calls | Reserved declaration/function identity, concrete direct call | C2: `generic-recursive`, `generic-recursion-dedup` |
| Bound / key parameter substitution | Checked concrete environment, diagnostic before high | C2: bound/key negative and cheap growth-limit tests |
| `Role`, `Coerce(RoleWrap/RoleNarrow)`, role call | Concrete role declarations, `Object.witnesses`, `role.wrap/narrow`, `call.role` | C3: ten role goldens and three diagnostic goldens in `goldens/p2_3/roles/` |
| `Successor(Object/Role)`, role result adapter | `role.wrap ^Object → ^Role`, `succ.take ^Role → Role`; one must-use obligation | C3: `role-successor`; canonical verifier B8 successor-role ± fixtures |
| `Array/Map/Range`, literals, collection builtins | `array.*`, `map.*`, `range.*`, lazy lookup CFG | C4: array/map/literal/typed-identity rows |
| Large/range-expanded literal | Linear `builder.*` / `mbuilder.*`, unwind/abandon | C5: builder producer fixtures; prerequisites of reduce-builder |
| `For`, array/range/map/Iterating/Batching sources | Loop CFG, iteration/cursor/batch scopes and result exits | C6: iterable/batching, ownership loops subcases |
| `Functional(map/filter/reduce/any/all/find)` | Enclosing action CFG, builders, short circuit | C7: early-exit, accumulator, reduce-builder |
| Imported source calls, plain `once let` | Reachable concrete source bodies, checked cache plumbing | C8: all four L09 collections; once negatives |
| Closures / bound actions / any / caps | Explicit source-span Unsupported until #569/#565-c4 | C9 integration: any rows, ordered-sets |
| Async body / await / future | Explicit source-span Unsupported until #570/#565-c5 | C9 integration boundary; no async case in this inventory |
| Full runtime/harness case | #593 comparison using executable backend | C9 acceptance for all runtime rows |

Families describe the tested contract, while the owning commit is the last producer
feature needed by the actual source (for example array formatting through `map` is C7).
C5 has no standalone original case: builder coverage comes from dedicated producer
fixtures and C7 accumulator cases. General `any` is C9 integration owned by #569.

C1 files: this table, `p2_3.tsv`, `tests.rs`, and the two-line module registration
at the end of the existing producer `tests.rs`. C2 tests use a separate module.

## C3 implementation and evidence

The §8 ruling permits successors of concrete roles. T1 accepts those roles and
continues to reject scalar/service successors and roles containing unbound type
parameters. The operand checker preserves the successor wrapper through
`role.wrap/narrow`; `succ.take` removes only must-use. The must-use component
tracks conversion/SSA aliases separately from ownership aliases, so wrap/narrow
retain one obligation while `exit.commit` still creates its own successor use.
No other verifier rules or ownership behavior are changed.

- [x] Concrete role declarations and `does`; static and dynamic slot calls.
- [x] Exact role declaration and slot identities in `call.role`, with H5 unwind.
- [x] Permitted narrowing with success and runtime-failure-route goldens.
- [x] Concrete and successor result adapters, including `^Box → ^Package`.
- [x] C2 specialization of generic roles and bounds/evidence; distinct declarations
      and witness targets for `Read<int>` and `Read<string>`.
- [x] Closed witness inventory in the existing `Object.witnesses` representation:
      exact role `TypeId`, ordered slot `FuncId`s, concrete receiver and body.
- [x] Builtin bridge needed by the C3 golden: string → `ByteSource`, through the
      builtin `StringSource` constructor, `bytes` method and `str.utf8`.
- [x] Ten deterministic High goldens, verified before printing, after parsing and
      from stored text: `role-static`, `role-dynamic`, `role-two-implementations`,
      `role-narrow-success`, `role-narrow-failure`, `role-successor`, `role-generic`,
      `role-return`, `role-concrete-return-adapter`, `role-builtin`.
- [x] Three diagnostic goldens: unknown slot and forbidden narrowing at frontend;
      no reachable implementation at lowering (source span retained).
- [x] Canonical verifier ± fixtures for concrete successor roles, wrap/narrow/take,
      alias use, discarded wrappers, scalar/service/open-role successors and
      dropping a successor obligation by a plain-role conversion.

`role-concrete-return-adapter` deliberately exercises the resolved THIR boundary:
the frontend currently requires an ordinary implementation result to match the
role result exactly. Source-level concrete-to-role returns are also covered by
`role-return`; successor covariance is covered from source by `role-successor`.
The generic-evidence tests cover concrete object bounds, narrower role
arguments and role fields without claiming execution of the original L05 files.

Remaining boundaries: async/variadic role slots, general builtin adapters for
scalar Comparing and collection iteration/sequence are explicit Unsupported;
they are not needed by these C3 goldens. Closed-set ownership summary joins and
semantic acceptance remain with #565; executable/oracle evidence remains gated
by #593 and backend. Dynamic dispatch introduces no `own.dynamic` or fabricated
summary. All 143 manifest statuses remain `planned`, including all eight C3 rows.
The `producer=` links in those rows record structural coverage only.

Final C3 verification, run from `okkama-rs/` with `--offline -j 4`:

| Command | Result |
|---|---|
| `cargo check --workspace --all-targets` | exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0, no warnings |
| `cargo test -p okkama-compiler --lib oir2` | exit 0; 115 passed, 118 filtered |
| `cargo test -p okkama-oir` | exit 0; 178 unit + 29 integration passed; 0 doc tests |

The fixture inventory is ten C3 High goldens, three diagnostic goldens and
thirteen new canonical verifier fixtures (six valid, seven invalid). The
canonical suite also rechecks the existing discarded-commit successor negative.
No runtime conformance/oracle run or L05/L09 acceptance is claimed.

Dependency classification also records explicit `any` signatures in application
declarations, including unused `Taker.anything`: the producer checks these bodies.

## Inventory counts

| Family | Rows |
|---|---:|
| any | 8 |
| array | 18 |
| batching | 6 |
| boundary | 6 |
| choice | 12 |
| collections | 7 |
| functional | 1 |
| generic | 21 |
| iterable | 4 |
| literal | 10 |
| map | 12 |
| maybe | 4 |
| nominal | 3 |
| ownership-builders | 1 |
| ownership-collections | 3 |
| ownership-negative | 3 |
| ownership-roles-generics | 3 |
| ownership-traversal | 2 |
| reduce | 1 |
| std-collections | 4 |
| typed-identity | 3 |
| variants | 11 |

| Owning commit | Rows |
|---|---:|
| C2 | 35 |
| C3 | 8 |
| C4 | 56 |
| C5 | 0 |
| C6 | 13 |
| C7 | 13 |
| C8 | 8 |
| C9 | 10 |

| Dependency (non-exclusive) | Rows |
|---|---:|
| none | 48 |
| #565-audit | 10 |
| #565-c4 | 25 |
| #565-c5 | 0 |
| #569 | 22 |
| #570 | 0 |
| #593 | 82 |
| backend | 82 |
