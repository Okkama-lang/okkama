# P2.4 C1: inventory и интеграционная матрица (#569 / #639)

Статус: C1 подготовлен для Claude. Все evidence gates в `p2_4.tsv` остаются `planned`.
Ни один перечисленный ниже будущий source golden ещё не объявляется существующим или прошедшим.
Имена `p2_4/<name>` — обязательства следующих C2–C9, а не ссылки на готовые fixtures.
Producer в C1 не меняется. Binding decisions взяты из начала `OIR-2-P2.4-plan.md`.

## Границы inventory

TSV: `id`, `path` относительно repository root, одна ведущая `family`, owning `commit` #569,
`dependencies`, `status`, исходные `mode/outcome/exit`, `inherited`, конкретный `subcase`.
`-` означает отсутствие exit expectation или inherited ID; `none` — отсутствие внешнего gate.
Взаимодействующие семейства описаны в subcase; ведущая family не означает, что остальные не нужны.
Исходный `case.json` остаётся нормативным для sources/support, stdout, stage/category и environment.
`error`, `failure`, `reject` и `success` сохранены раздельно; success с exit 1 не является pass suite.
Для negatives требуется intended frontend category/stage, не случайный `Unsupported`.

Включены все группы §7.3, 22 первоначальные inherited строки §7.2, corpus §7.4 с наблюдениями,
а также найденные прямые применения новых форм вне перечисленных групп: opaque overload signatures,
`Atomic<bool>`, `Atomic<@float>`, `Atomic<Maybe<Tree>>`, CAS, closure в time.zones и cap через path alias.
Общие уже поддержанные stdout/UTF bridges и `Atomic<int/string>` как счётчик/лог не создают новую строку
только по факту импорта. Функциональное `items.any() -> ...` остаётся CFG #568, не opaque any.
Пользовательский object с именем `Cell<T>` в generic-successor cases не является std Cell.
Общая native ABI перечисляется по L06 native contracts; полный std runtime catalogue относится к C7.

Ровно 22 строки имеют `inherited=<старый ID>`. Их owning commits #568 и статусы в `p2_3.tsv` сохранены.
Дополнительно выявлен `L05.batching.preferred-over-cursor`: реальный Both содержит `Atomic<bool>`.
Его новая C7 зависимость добавлена в `p2_3.tsv`; теперь ссылок #569 там 23: исходные 22 плюс эта находка.
Все 23 направлены на `p2_4.tsv#<id>`. Это расширение inventory, не заявление о покрытии #568.

Полные mixed inputs сохраняют #570/#565-c5, включая async main без await. Sync extractions — отдельные
ID с `#sync`; они не закрывают родительский input. `anonymous.async-rejected` отвергается парсером,
`native.decl-valid-forms` и `gram.role.members` остаются parse-only, а skipped async tests в production
не требуют async lowering. Runtime gates требуют #593 и backend, даже когда compiler High станет готов.
#565 commit 4 — общая база всех High→Own проверок, не незавершённая внешняя dependency каждой TSV строки.

## Счётчики C1

Всего 162 строки: 124 полных conformance cases + 4 extracted subcases, 34 corpus case/subcase entries.
Все 162 статуса — planned; пересечения dependencies считаются независимо.

| Family | Rows | Commit | Rows |
| --- | --- | --- | --- |
| closure | 45 | C2 | 3 |
| bound action | 12 | C3 | 41 |
| cap | 21 | C4 | 11 |
| any | 35 | C5 | 18 |
| native | 8 | C6 | 37 |
| cell/atomic | 16 | C7 | 22 |
| test profile | 22 | C8 | 22 |
| once | 3 | C9 | 8 |

Dependencies: #565-c5 — 15; #570 — 15; #593 — 100; backend — 100; none — 62.
Commit C9 означает full-input integration acceptance; implementation owner указан в subcase.

## THIR → High → будущий source golden

Общее: concrete signatures, ABI unelaborated, snapshot packet=None, Action root на каждое тело,
D11 result demand, D8 prepare/exit, H5 unwind и реальные locations. Own вставляет packets/deps/adapters.
Каждый source golden требует High verify, print/parse, детерминизм, затем Own и independent verify.

| THIR form | High contract | Planned source golden / owner |
| --- | --- | --- |
| `Closure` | `fn.closure @lambda, [captures], ty fn` | `p2_4/closure-{empty,self,nested,escape}` C2/C3 |
| `BoundAction` | `fn.bind receiver, @method, ty fn` | `p2_4/bound-{snapshot,fresh,pending}` C4 |
| `BoundRoleAction` | `fn.bind.role receiver, role R, slot N` | `p2_4/bound-role-{overload,generic}` C4 |
| `CallValue` | `fn.call callable(args)` | `p2_4/call-{known,join,loop,open,late-failure}` C3 |
| `CapSelect` | `cap.make owner, ty Cap` | `p2_4/cap-{hidden,external,generic,snapshot}` C5 |
| `CapRead` | `call.cap cap, field_ref(decl,index)()` | `p2_4/cap-{parameter,recompute,result-view}` C5 |
| `OwnerField` | `obj.get owner0, field` | `p2_4/cap-owner-field` C5 |
| `Coercion::Box` | `any.box value, ty any` | `p2_4/any-{plain,resource,boundary,nested}` C6 |
| `BodyOwner::Test` | `Module.tests`, `Feature::Test`, `() -> void` body | `p2_4/test-{roots,names,empty}` C8 |
| `Callee::Test` | `test.ok/is/isnt/fail`; typed intrinsics | `p2_4/test-{assertions,skip,successor}` C8 |
| fakes after desugaring | ordinary ctor/defaults/object/role.wrap | `p2_4/fake-{state,defaults,generic}` C8 |
| native callee/declaration | handle, extern symbol, `call.native` | `p2_4/native-{data,handle,missing}` C7 |
| trusted native with any | exact registered `call.intrinsic` bridge | `p2_4/opaque-{inspect,items,format}` C6/C7 |
| Cell/Atomic concrete calls | std Cell body + registered Atomic intrinsics | `p2_4/storage-{read,replace,cas}` C7 |

Closure body key — `(concrete enclosing body, lexical path)`, не один `BodyOwner::Closure`.
Receiver capture идёт первым только при `uses_receiver`, затем THIR locals; explicit parameters после captures.
Сохранить owner authority lambda; owner-scoped test её не получает. Capture prefix отсутствует в `fn` type.
Known targets используют inference/origin join; open target — registry contract из decision O1 ниже.
Bound targets берутся из resolved THIR identity/slot, без повторного поиска по имени/arity.
Cap computed field получает concrete owner в parameter 0, удержание cap — полный owner graph.
`any -> any` — identity; heterogeneous literals box каждый element, retag не заменяет boxing.
Fakes не создают `fake.new`/`fake.storage`. Test throws остаётся существующим try/catch desugaring.
Atomic placeholder bodies не lowerятся; read/replace/CAS/closed/close имеют точные contracts.
Once сохраняет #568 load/store/layout и frontend proof, дополняя full-graph classification (C6).

## Protocol mapping: все 29 пар #565 commit 4

Прочитаны `protocol-high/*.oir`, `protocol-goldens/*.oir`, `protocols.rs` и protocol manifest в checkout 565.
База commit 4: `c7acb033`; во время C1 HEAD checkout уже `fb222fc6`, а не указанный в плане `f24dd24e`.
High inputs относительно commit 4 не изменены; Own snapshots/stats имеют последующие исправления.
Mapping ниже сверен также с `git show c7acb033:.../protocol-goldens/<name>.oir`.
Ни HEAD 565, ни незакоммиченные verifier fixes не становятся dependency #569 без отдельного review.

`match` означает совпадение op/ABI/retention с соответствующим High input и Own protocol,
с точностью до IDs, имён, locations, обычных source binds/scopes и вспомогательных constructors.
Это не побайтовое совпадение полного printed module. `differ` означает существенную разницу входа,
контракта либо невозможность получить ручной input из поддержанного source; причина дана после таблицы.
Итого **16 match / 13 differ**. Ни одна пара сама по себе не является source/runtime acceptance.

| Protocol golden | Source construct producing High input | Form |
| --- | --- | --- |
| any-borrowed | outer bound Res boxed inside nested block | match |
| any-fresh | direct `Res()` result boxed at typed any return | match |
| any-known-plain | `7` at any result boundary | match |
| bound-role | bound `role.label`, then indirect call | match |
| bound-snapshot | bind `owner.label`, rebind owner, call old reference | match |
| cap-call | bound resource owner → cap → computed label | match |
| cap-hidden | direct Owner(hidden Res, label) return through cap | match |
| cap-parameter | concrete cap parameter → computed label | match |
| closure-borrowed | nested block returns lambda capturing outer bound Res | match |
| closure-empty | empty-capture lambda returning int, then call | match |
| closure-fresh | return lambda capturing locally created Res | differ |
| closure-known | bound Res captured by known lambda, then call | match |
| closure-known-join | conditional lambda / bound-method callable join | match |
| closure-known-loop | loop invoking and transporting one known callable | differ |
| closure-known-phi | both conditional arms yield same captured callable | match |
| closure-open | callable parameter `(Res) is Res`, called with fresh Res | differ |
| descriptor-binding | concrete boundary with trusted type descriptor | differ |
| descriptor-plain-parameter | generic identity specialized with plain argument | differ |
| dynamic-any | opaque typed boundary retaining managed graph | differ |
| dynamic-atomic | managed Atomic operations through registered service | differ |
| dynamic-cell | std Cell operations over borrowed managed values | differ |
| dynamic-channel | channel publication/receive in #570 | differ |
| dynamic-closure | open fn.call through registered Closure adapter | differ |
| dynamic-group | FutureGroup/collector operations in #570 | differ |
| once-plain | `once let` of plain int inside object action | differ |
| role-call | bound Res wrapped as role, exact slot call (#568) | match |
| role-parameter | role parameter, exact slot call (#568) | match |
| role-plain | plain concrete implementer wrapped as role (#568) | match |
| service-schedule-empty | no source obligation for an empty service schedule | differ |

### Точные retention и различия

- `any-borrowed`: no offered packet; boxed graph retains external deps, inner close не уничтожает outer Res.
  `any-fresh`: direct unbound constructor result передаёт fresh packet через box/result exit.
  `any-known-plain`: known int graph, empty deps; не вводить dynamic Any только из-за типа any.
- `bound-role`: role.wrap + exact Readable slot 0; Own создаёт witness entry и callable ABI.
  `bound-snapshot`: Own ссылается на `read.__own_entry_1`, deps старого receiver переживают rebind.
- `cap-call`/`cap-parameter`: `call.cap field_ref(View,0)` применяет summary owner-as-parameter-0.
  В `cap-hidden` source временный Owner даёт packet cap; скрытый R остаётся в полном графе,
  хотя @label читает только str. Отдельный source fixture нужен и для bound owner без packet.
- `closure-borrowed`/`closure-known`: capture R берёт deps bound owner, не token и не новую identity.
  Empty closure сохраняет identity action value, но не получает resource destroy obligation.
  `closure-known-phi` не теряет known origin; join lambda/bound в `closure-known-join` конечен,
  не требует dynamic Closure. Реальные ветки могут добавить lexical scopes без смены retention.
- `closure-fresh`: ручной High не содержит `bind %r`, хотя declaration binding существует.
  Source `let r=Res(); () is int { r.read(); }` ОБЯЗАН сохранить bind. Поэтому snapshot operation
  заимствует r, а owned local часть переносится при result exit closure. Ручной Own передаёт packet
  прямо в fn.closure. Нужен отдельный source Own golden; удалять bind для совпадения запрещено.
- `closure-known-loop`: ручной High переносит `%current` и index по backedge, вызывает тот же closure
  в iteration, затем читает r. Source for обычно держит callable как invariant local; source loop с
  carried values добавляет реальные binds/result exits. Сохраняются finite origin/deps и нулевой Merge,
  но точный CFG/Own snapshot нельзя принять как source golden без отдельного lowering fixture.
- `closure-open`: ручной Mopen только alias parameter(1), deps=[1], fresh=[]; он возвращает аргумент.
  Он не выражает произвольный fresh или capture-derived result. Предлагаемый O1 добавляет runtime-fresh
  result relation, retention callable owner graph(0) и все допустимые argument graphs. Adapter сохраняет
  actual packet/deps и cleanup на обоих outcomes; нужны самостоятельные source goldens всех трёх форм.
- `descriptor-binding`: ручной @f(int,type) с explicit descriptor.binding не имеет языкового аналога.
  #569 выдаёт concrete type.of/registry facts в opaque/callable boundaries; не добавляет type-параметр
  в source action ради этого теста. `descriptor-plain-parameter` содержит residual @f<T> и dictionary;
  #569 fully specializes T=int и не выдаёт residual executable generics. Оба остаются отдельными IR tests.
- Все шесть `dynamic-*` вручную задают protocol/summary на @f(R)->int, тело которого лишь возвращает 7.
  `dynamic-any`/`dynamic-closure` в Own принимают selected packet через checked dynamic transition.
  Source использует реальные opaque/open entries с descriptors, а не синтетическую аннотацию @f.
  `dynamic-atomic`/`dynamic-cell` дают scope→scope transition с пустым packet и managed R parameter;
  это protocol machinery, а не High настоящих storage операций. Source golden должен проверять actual
  read/replace/CAS, borrowed deps и публикацию до/после equality, без токена в stored value.
  Cell следует std body над Atomic: family определяется зарегистрированным adapter, не именем метода.
  `dynamic-channel`/`dynamic-group` остаются #570/#565-c5 IR regressions, scheduling/runtime здесь не имитируется.
- `once-plain`: ручной High сразу store/load без cache guard. Source once имеет slot layout, miss/hit CFG,
  computes-once и publication только после успеха; #568 plumbing сохраняется. Plain closure/role/opaque
  certificate проверяется по полному графу, включая test witnesses; resource graph остаётся rejection.
- Role triples уже принадлежат #568: own сохраняет wrapper deps и exact witness adapters; #569 использует
  их, а не внедряет второй role pipeline. Закрытый plain implementer не отменяет declared role ABI.
- `service-schedule-empty`: искусственный `service.schedule` с пустыми rules проверяет service inventory.
  Producer не должен добавлять пустой schedule ради snapshot. Реальные duties появятся только из contracts.

## Следующие gates и обязательства

C2: discovery/type/registry и отдельный audited O1 contract step до готовности C3 open calls.
C3–C7: source goldens для таблицы, ± mutations по contracts, High→Own semantic verification.
C8: audited S1 transport step до skip readiness; production/test pair, state defaults, runner isolation.
C9: полные inputs, inherited gates, census и #593/backend evidence. C1 не закрывает acceptance #569.
Дополнительные corpus regressions §7.4 ещё предстоит создать: late arguments A d / d A; fresh/capture result;
role-bound borrow/take; cap result-view; opaque borrow после inner close; expired storage; cleanup/skip/next case.
