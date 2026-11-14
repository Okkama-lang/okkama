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
| `Role`, `Coerce(RoleWrap/RoleNarrow)`, role call | Witness inventory, `role.wrap/narrow`, `call.role` | C3: role/evidence and role-field cases |
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
