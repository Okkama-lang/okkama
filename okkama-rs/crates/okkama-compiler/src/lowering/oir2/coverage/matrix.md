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
| Range-expanded / large literal | Linear `builder.push/append/append.range`, `finish`, failure `abandon` | C5: `builders/`; migrated C4 `range-expansion`, `large-literal` |
| `For`, array/range/map/varargs/Iterating/Batching | Parent headers, scoped iterations, D11 state and exits | C6: `traversal/`; [scope and fixture evidence](c6.md) |
| `Functional(map/filter/reduce/any/all/find)` | Enclosing action CFG, builders, short circuit | C7: early-exit, accumulator, reduce-builder |
| Imported source calls, plain `once let` | Reachable concrete source bodies, checked cache plumbing | C8: all four L09 collections; once negatives |
| Closures / bound actions / any / caps | Explicit source-span Unsupported until #569/#565-c4 | C9 integration: any rows, ordered-sets |
| Async body / await / future | Explicit source-span Unsupported until #570/#565-c5 | C9 integration boundary; no async case in this inventory |
| Full runtime/harness case | #593 comparison using executable backend | C9 acceptance for all runtime rows |

Families describe the tested contract, while the owning commit is the last producer
feature needed by the actual source (for example array formatting through `map` is C7).
C5 has no standalone original case: builder coverage comes from dedicated producer
fixtures and C7 accumulator cases. General `any` is C9 integration owned by #569.

C5 implementation and §4.2 evidence are tracked in [c5.md](c5.md). The former
precommit blocker is a positive regression and canonical High fixture. Structural
producer coverage does not change any conformance row from `planned`.
C6 traversal evidence and its remaining integration gates are tracked in [c6.md](c6.md).

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

## C4 implementation and High evidence

C4 implements concrete arrays, maps and ranges in the independent producer.
It does not run ownership elaboration or change the verifier/own implementation.
All 143 statuses, including the 56 C4 rows, remain `planned`. `producer=` and
`C4-partial=` identify structural evidence; they do not accept the original
conformance files or their runtime output. Existing dependency classifications
remain in force, including `any`, functional formatting and the #593/backend gates.

- [x] Empty, typed, nested and role-bearing literals; concrete generic collections.
- [x] Ordered evaluation of elements and map key/value pairs, including duplicate keys.
- [x] Range expansion after operand evaluation; inclusive/exclusive validation.
- [x] Large literals use `array.new` directly; no streaming builder is emitted.
- [x] Indexed reads, lazy lookup/fallback, sizes and array first/last/item.
- [x] Slice, take/drop, reversed, contains/index_of and string-array join.
- [x] Concat/merge/difference, replaced/removed_at, map has/keys/values.
- [x] First-boundary retags and typed empty results, including operator operands.
- [x] Int, float, rat, bool, char, string and enum key categories; byte keys retain
      the frontend rejection. The untyped `map<never,never>` sentinel is permitted
      only as an empty map, without relaxing concrete key constraints.
- [x] H5 unwind and D11 result exits; deterministic verification and codec round trips.
- [x] 37 High fixtures and four frontend diagnostic fixtures in
      `fixtures/p2_3/collections/` and `goldens/p2_3/collections/`.

### Operations and exact relations

High emits operations with Unelaborated ABI and no packets/deps. The existing
public `Retention` contracts and codec express the relations below; expanding
`Projection` is unnecessary. No new runtime intrinsic or substitute OwnerGraph
selection was introduced.

| Source / resolved form | High form | Relation in public contracts |
|---|---|---|
| `[a,b]`, `[k:v]` | `array.new`, `map.new` | `Union`, `Entries`, in evaluation order |
| `start..end`, `start..=end` | `range.new`, `range.len` | Scalar; failing range validation keeps unwind |
| Range part in array | C5 ordered `builder.push/append.range`, `finish` | Existing buffer/element contract |
| `xs[i]`, `item`, successful array lookup | `array.get` / `array.lookup` found edge | `Index` |
| `m[k]`, successful map lookup | `map.get` / `map.lookup` found edge | `MapEntry` / internal `Map` |
| `m[k] else fallback` | Lookup found/missing branches, D11 join | Found `Map`; fallback evaluated only on missing |
| `first/last` | Array lookup, missing `trap empty` | `Index`; trap retains H5 unwind |
| Slice/take/drop | `array.slice`, scalar count validation/clamp CFG | `Slice` |
| `m.keys()[i]`, `m.values()[i]` | `map.key_at/value_at` | `MapPosition(index,key)` / internal `MapIndex` |
| `m.keys()`, `m.values()` | `map.keys/values` | `Keys`, `Values` |
| `removed_at`, `replaced` | `array.remove/replace` | `ArrayRemove` / internal `Remove`; `ArrayReplace` composes old Remove + new value |
| `+`, `-` | `array.concat/difference`, `map.merge/difference` | `Union`, `MapMerge`, `Difference`; displaced obligations stay with their owner |
| Nested getters | `obj.get` and calls to concrete getter bodies | Exact `Field` edges compose paths; existing `SummarySelection::Path` is the call-summary representation |
| Retag | `array.retag`, `map.retag` | `Value`, preserving the resolved concrete tag |
| Reversed | Counted CFG, `array.get/new/concat` | `Index` + `Union` |
| Contains/index_of | Short-circuit CFG, get/compare, concrete Maybe cases | `Index`; scalar result / concrete variant |
| Size/has/join | `array.len`, `map.len/has`, `array.join` | Scalar result; join observes string elements |

Concat/merge flatten both sides, difference only its left chain, matching the
current pipeline. Small persistent forms evaluate operands before the combined
operation. C5 streams large concat/merge chains in source order through buffer
updates; untagged empties retain the resolved concrete result type.
Map positional projections evaluate the map receiver before the index and
select the requested key/value side without retaining the other side.

### Golden inventory and audit staging

The exact 37-name source inventory is checked by `GOLDENS` in
`collection_tests.rs`. Every module is independently verified before printing,
after parsing, and as stored expected text; a second lowering is independently
verified and compared byte for byte.

| Requirement | Fixtures |
|---|---|
| Literals, tags, nesting, large construction | `literals`, `large-literal`, `empty-results`, `generic-collections`, `role-elements` |
| Evaluation order, duplicate map keys, expansion | `evaluation-order`, `range-expansion`, `scalar-keys` |
| Lazy hits/misses, resource fallback | `lookup-lazy` |
| H5 failures including catches | `bounds`, `bounds-{first,last,map,take,drop,remove,replace}` |
| Persistent operators and empty operands | `array-ops`, `map-ops`, `operators-empty-chain`, `resource-projections` |
| Derived search/shape CFG | `search-{contains,index}`, `shape-{take,drop,reversed}`, `resource-reversed` |
| Index/Slice/Map/MapIndex/Keys/Values/Remove/Path | Nine `projection-*` relation fixtures plus `projection-control` |

The relation fixtures each include fresh local resources and borrowed parameters;
duplicated array elements/map values are intentional. They cross block parameters
and return boundaries with D11 preparations. `projection-control` carries aliases
through a loop and branch/lookup joins, and produces empty selections from
resource-bearing sources. `projection-keys` and `projection-map-key-index` use
resource values with plain keys. Nested getters preserve exact field edges for
later Path summary inference. `resource-reversed` carries resource aggregates
through the primitive shape loop.

These are High inputs for the §7.3 audit. Own accept/reject pairs, selected/full
and external dependency proofs, stable view-key/token move checks, actual owner
invalidation and forged-key rejection remain with #565. Packets, view-key service
moves and computed deps are forbidden in High, so none are fabricated in these
fixtures. Hidden-owner cursor/cap views remain gated by C6/#569/#565.

`oir2_c4_frontend_negatives` checks stage and diagnostic text for wrong key and
wrong element types, byte keys, and the original
`tests/ownership-corpus/rejected/map-resource-key.ok`. The stored diagnostic
names are `wrong-key`, `wrong-element`, `byte-key`, `resource-key`.

### Remaining boundaries

Streaming builders are implemented in C5. C6 now implements application array,
range, resolved map, variadic, cursor and synchronous batch traversal; see [c6.md](c6.md).
The original P1 range golden text is preserved. Functional blocks remain
Unsupported (C7); ordering and general builtin iterable adapters remain outside C6. Any/closures/capability
and async dependencies remain #569/#570. General scalar conversions from string
and int-to-char are outside this slice and produce explicit Unsupported; typed
identity and numeric conversions supported by existing contracts are retained.

The verifier uses bounded relational exploration. Initial combined fixtures with
many sequential branching operations/catches reached its default memory budget
(`Incomplete`); operation/failure goldens are isolated to identify individual
contracts. No verifier limit or rule was changed, and no incomplete verification
is counted as a passing golden. Managed semantic and executable acceptance still
requires #565 audit fixes and #593 with the executable backend.

### Final C4 verification

Run from `okkama-rs/`, offline with four jobs; the final compiler test run used
stored fixtures without the update environment variable.

| Command | Result |
|---|---|
| `cargo check --offline -j 4 --workspace --all-targets` | exit 0 |
| `cargo clippy --offline -j 4 --workspace --all-targets -- -D warnings` | exit 0, no warnings |
| `cargo test --offline -j 4 -p okkama-compiler --lib oir2` | exit 0; 118 passed, 118 filtered |
| `cargo test --offline -j 4 -p okkama-oir` | exit 0; 178 unit + 29 integration passed; 0 doc tests |

`git diff --check` passes. New Rust/source fixture lines are at most 120 characters.
No verifier/own source changes, frontend changes, new intrinsics or git commit.
