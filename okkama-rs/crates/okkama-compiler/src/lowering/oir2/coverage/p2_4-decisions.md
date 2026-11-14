# P2.4 C1: предложения интеграционных contracts для review Claude

Статус: **proposed, needs Claude review**, без реализации producer/IR/runtime.
Binding decisions начала плана соблюдены; ниже конкретизируются оставленные C1 варианты.
Они не объявляют открытый callable или skip готовым. Review исходного плана не заменяет audit этих deltas.
База API: `contracts.rs`, `own::{NativeSummaries,ExternalBoundary,Substitution}`, `verify::ExternalContracts`,
`own/summaries/{adapters,infer,annotate}.rs`, P1NativeSummaries и runtime ABI table текущего worktree 569.
#565 protocol reference — commit 4 `c7acb033`; дальнейшие snapshots из checkout 565 не считаются принятой базой.

| Decision | Owner | Audited verifier/contract step |
| --- | --- | --- |
| O1 registered open callable contract | C2, before C3 readiness | **Да**, IR delta, ± fixtures, independent audit |
| D1 full-graph synchronous opaque cleanup descriptor | C2/C6 | **Да**, trusted catalogue and verifier boundary |
| B1 nine exact native-any bridges | C6/C7 | **Да**, each registered signature/summary and backend binding |
| N1 missing-provider data/handle adapter | C7 | **Да**, checked adapter registration, no general ABI widening |
| S1 skip transport and test intrinsic | C8, before skip readiness | **Да**, failure/summary/codec/runtime delta |
| F1 ordinary fakes and test roots | C8 | **Нет** нового IR delta; existing verifier gates обязательны |
| I1 inventory and manifest cross-reference | C1 | **Нет**, parsing/consistency tests only |

## O1. Registered conservative open callable ABI

### Registry identity и граница доверия

Ключ: concrete structural `Type::Fn` (params/results, sync), execution profile и версия контракта.
Module-local TypeId используется только как lookup handle внутри данного Module, не как global identity.
`ExternalBoundary::Callable(TypeId)` остаётся точкой own lookup; registry создаётся до elaboration,
используя ту же specialization/declaration inventory, что и producer. Никакого копирования Mopen из IR.
Known singleton/finite origins продолжают использовать body inference/adapter join; registry нужен только open.
Unknown/mismatched intrinsic, nonconcrete fn type, неподтверждённый sync graph — диагностика.
Registry не выводит отсутствие async destroy из синхронности сигнатуры.

Нужно расширить `verify::ExternalContracts` независимым lookup callable contract по concrete fn type
и profile: предлагаемый метод `callable_summary(TypeId, ExecutionMode) -> Option<&Summary>`.
Trusted descriptors/services получают аналогичные exact lookups по type/contract identity.
Сейчас trait проверяет только native/intrinsic.
Independent verifier сверяет зарегистрированную схему с effective summary каждого open `fn.call`,
descriptor cleanup и generated entry ABI; запись summary самим producer не является сертификатом.
Source action type не получает ownership annotations; hidden ABI собственный у Own.

### Точный предлагаемый Summary

Нумерация summary parameters: 0 — callable environment, 1..N — пользовательские arguments.
Public fn type исключает environment и service operands. Canonical entry:
`callable, args..., deps..., input_packet -> value?, actual_packet, actual_deps`, unwind только `failure`.
Порядок service operands/слотов наследует Own explicit ABI и B62; это не новая ручная High ABI.
Before dispatch adapter принимает предложенный пакет; после dispatch ни один входной token не остаётся бесхозным.

- `entry_kind=CallBoundary`.
- Parameter 0: `Borrow`, `retention=None`, `deps=[]`, `destination=None`: вызов не потребляет captures.
  Result alias при этом отдельно может указывать на полный environment graph(0).
- Plain arguments: `Borrow`, no retained ownership, empty deps, destination None.
- Potentially managed arguments 1..N: `Take`, `retention=OwnerGraph(i)`, `deps=[i]`, destination Result
  как консервативная **возможность** удержания; при void result destination None.
  Take относится только к действительно предложенному fresh packet. Bound borrowed graph остаётся во
  внешнем journal; `consume value`/пустой offered packet не позволяют забрать token внешнего владельца.
  Audited step явно проверяет это соответствие и вводит adaptation для bound input:
  canonical consume entry получает value + borrowed deps + пустую offered часть этого аргумента.
  Consume SSA representation не является consume внешнего resource token; owner journal не меняется.
  Runtime adapter возвращает фактическую retained часть; unused fresh inputs закрывает до normal return.
- Void: `results=[]`. Plain result: одна relation с пустыми aliases/deps/fresh.
- Managed result: `aliases=[OwnerGraph(0), OwnerGraph(1), ..., OwnerGraph(N)]`,
  `external_deps=[0,1,...,N]`, `provenance=UnboundResult`, плюс runtime-fresh permission, описанный ниже.
  Это разрешённый верхний предел, не утверждение, что каждый arg присутствует в каждом результате.
  Фактический descriptor/result deps может быть точнее; capture-derived borrow не становится fresh.
- `contract.results` — точный concrete return type (либо пусто для void).
- `failures=[Thrown,Program,Panic,Native]`, `suspends=false`, `async_destroy=false`, `publication=Atomic`.
  Cancel не имитируется в synchronous C3; async boundary ждёт #565-c5/#570.
  Test execution contract дополнительно допускает S1 TestSkip через helper calls.
- `protocol=Closure`, registered publish/dynamic transitions, checks owner/live_dependencies/snapshot/transferable,
  roots operands. Unwind после принятия settle inputs; dynamic-check unwind возвращает его unaccepted packet
  в актуальный caller state для cleanup. Эти разные outcomes нельзя объединить или потерять packet.

Destination Result здесь должен трактоваться как may-retain upper bound, с фактическим packet на возврате.
Текущая `TransferDestination` не кодирует альтернативы; audited O1 обязан явно определить и проверить эту
семантику (включая take-with-void cleanup), а не молча перенести её из ограниченного fixture.
Неиспользованный fresh input может завершиться до возврата, без включения в result packet.

### Fresh и capture-derived result: необходимая IR delta

Текущий `ResultRelation.fresh: Vec<CreationSiteId>` перечисляет статические creation sites.
У open callee их невозможно перечислить. Mopen из `closure-open` имеет fresh=[], alias parameter(1),
external_deps=[1]; он описывает только returned argument и недостаточен для общего action type.
`SummarySelection::OwnerGraph(0)` уже выражает capture-derived retention; новый capture-selector не нужен.

Предлагается явное поле relation `runtime_fresh: bool` (text `runtime_fresh=true`, default false).
Допуск true — **только** у зарегистрированного open/runtime entry, связан с checked result descriptor,
actual returned packet и Closure protocol. Смысл: разрешён неизвестный набор новых identities результата,
каждая зарегистрирована до публикации и фактически присутствует в returned packet.
Это не wildcard CreationSiteId, не разрешение произвольного IR объявить себя trusted и не plain certificate.
Статическое fresh остаётся для известных тел; runtime-fresh не добавляет identity самому callable value.
Capture-derived return сохраняет actual deps полного callable owner graph; actual packet содержит только
передаваемую owned часть, а чужие borrowed captures не изымаются. Смешанный result объединяет fresh packet
и borrowed deps без двойного destroy. Bound resource argument требует отдельного регрессионного evidence.

Необходимые изменения описаны, но не выполняются C1:

1. `contracts.rs`: поле relation, явная семантика may-retain result upper bound и permissions для external entries.
2. Text parse/print, summary normalization/join/SCC, visitors/remapping, builder и tests для нового поля.
3. Own analysis/driver: symbolic runtime creation origins вместо выдуманных sites; actual packet/deps
   admission и result exits; adapter не забирает environment captures и bound arguments.
4. Verifier summary/effects/retention/linearity: external callable lookup, descriptor identity и shape,
   conservation actual packet, normal/unwind slots, forbidden forged runtime-fresh, result alias bounds.
5. Backend adapter: register-before-publish, same failure transport, exact resolution of entry version.

Обязательные + cases: plain/void, returned fresh argument, returned bound argument, own fresh Res,
capture-derived Res/view, mixed fresh+capture graph, repeated call, late argument failure, dispatch/body/cleanup
failure, finite known join без fallback. − mutations: omitted packet/deps, fake site, missing capture graph,
wrong signature/protocol/profile, self-certified descriptor, reused consumed edge input, false no-async proof.
**Отдельный reviewed/audited step до C3 open-call readiness**, как требует решение Claude №3.

## D1. Full-graph opaque descriptor

Использовать existing `DescriptorContract { managed, retention, cleanup, async_destroy, summary }`.
Для open fn/any `managed=Runtime`, retention полного допустимого графа через OwnerGraph/Descriptor,
cleanup — точный trusted intrinsic, summary из registry. Closed plain — только независимый full-graph proof.
Graph включает hidden object fields, role witness targets, nested collections, cap owner, closure captures,
а в Tests — также fake implementers. `async_destroy=false` требует доказательства на всём допустимом графе.

Existing descriptor fields достаточны для synchronous layout; **новый descriptor opcode не предлагается**.
Нужны audited registration/validation rules и backend traversal, включая O1 unknown result graph.
Пока proof отсутствует или hidden cleanup async — source Unsupported/diagnostic до #565-c5, не fake sync proof.
Once использует тот же граф и по-прежнему отвергает любой reachable resource; body purity сохраняется frontend.

## B1. Полный trusted opaque bridge allowlist из runtime ABI

Источник: `okkama-runtime-abi/src/table.rs`, сверка `std/runtime/{debug,format}.ok`.
Ровно **9** записей с `any` в параметре или результате, включая вложенные `@any`; иных таких ABI rows нет.
Wire IDs сохраняются. Compiler remap выполняется только после exact canonical symbol + signature match.
Предлагаемые intrinsic IDs — `okkama.opaque.<wire-id>`; numeric suffix — явная конечная таблица, не fallback.

| Wire ID | Canonical provider symbol | Exact source signature |
| --- | --- | --- |
| 140 | `runtime/debug@1::data_dump/1` | `(any) -> string` |
| 153 | `runtime/format@1::sprintf_values/2` | `(string, @any) -> string` |
| 529 | `runtime/debug@1::data_kind/1` | `(any) -> string` |
| 530 | `runtime/debug@1::data_type/1` | `(any) -> string` |
| 531 | `runtime/debug@1::data_size/1` | `(any) -> int` |
| 532 | `runtime/debug@1::data_leaf/1` | `(any) -> string` |
| 533 | `runtime/debug@1::data_names/1` | `(any) -> @string` |
| 534 | `runtime/debug@1::data_keys/1` | `(any) -> @any` |
| 535 | `runtime/debug@1::data_items/1` | `(any) -> @any` |

High: exact `call.intrinsic @registered_bridge`, bridge metadata binds provider symbol/wire ID/backend function.
Общий `call.native` продолжает запрещать any и language object/role/action/cap/Future graphs рекурсивно.
Неизвестное пользовательское native с any диагностируется; совпадение короткого имени не даёт bridge.

Все bridges сохраняют исходную call boundary native: fresh admission в callee, bound inputs borrowed.
Не превращать их автоматически в observation ради оптимизации cleanup timing.
Summary concrete parameters/types; `CallBoundary`, atomic publication, sync, failures Program/Panic/Native
консервативно плюс точные typed failure descriptors из реализации, если bridge их создаёт.
No async graph без D1 proof. Bridge не вызывает пользовательский code для скрытой распаковки или string().
Native/format implementation и diagnostic formatting должны быть сверены C7; список видов failure — upper bound,
а фактическая категория/runtime message сохраняется от provider.

- 140, 529, 530, 531, 532, 533: inputs не удерживаются после normal return; managed resource identities
  результата отсутствуют, aliases/external_deps пусты; string/@string могут иметь GC allocation.
- 153: traverse `@any`, но результат string не удерживает ownership graph входа; aliases/deps/fresh identities пусты.
- 534: map keys разрешённых scalar key types boxed в @any; ownership identities не создаются и не удерживаются.
  Exact key policy проверяется по действующему frontend/key contract, не считается arbitrary managed projection.
- 535: array elements/map values/object fields возвращаются как @any; conservative relation aliases OwnerGraph(0),
  deps=[0], provenance UnboundResult, **без нового resource token**. Свежая array/box allocation не равна
  resource creation. Fresh входная owned часть, достижимая через результат, переносится фактически;
  bound owner остаётся borrowed. Source regressions нужны для owner-dependent projected views и hidden graph.

Каждый bridge получает ± signature, nested graph, retention/failure tests и independent verifier audit.
B1 не требует расширения native opcode; descriptor/result integration использует D1/O1 reviewed deltas.
Нельзя закрыть B1 только проверкой canonical signature: ABI catalogue пока не содержит ownership/effect facts.

## N1. Checked adapter для отсутствующего provider

Разделить compiler declaration registration, trusted transport contract и runtime provider availability.
`module@major::name/arity` + concrete signature + nominal handle identity регистрируются до dispatch.
Для unknown provider допустим **только** проверенный conservative adapter data/handle ABI:
scalar data, рекурсивные collections с разрешёнными keys и data/handles, void, nominal native handles.
Recursive object/role/fn/cap/Future/any transport отвергается; exact B1 — отдельная allowlist.
Async declaration может парситься, но invocation ждёт #570. Unknown intrinsic всегда ошибка регистрации.

Предлагается точный intrinsic adapter `okkama.native.resolve.<canonical-key>` с registry entry на signature,
а не попытка пометить произвольный extern как trusted. Имя — display key; авторитет даёт запись registry.
Entry `CallBoundary`, borrow data/handle args, no language resource retention, empty aliases/deps/fresh,
exact result types, sync, atomic, conservative Program/Panic/Native failures; отсутствие provider — Native
resolution failure с оригинальным source location/message. No automatic Okkama destroy у handle.
Adapter является универсальным кодом только для проверенного transport schema, не wildcard ownership summary.

Invocation runtime-resolves canonical symbol. При отсутствии provider fail **после** оценки предшествующих
source effects/arguments: `L06.native.missing-provider` печатает `before`, затем resolution failure.
Нельзя отбраковывать программу frontend лишь из-за отсутствия host binding или придумывать provider.
Наличие source declaration с запрещённым ABI, отсутствие trusted contract и missing provider — разные ошибки.
Если provider найден, adapter проверяет wire/signature/handle registry и возвращает/закрывает его native
allocations по host ABI; handle регистрируется до публикации, незавершённая native allocation убирается на failure.
Language wrapper с destroy остаётся ordinary object; adapter не создаёт для handle language token.

Нового native opcode не требуется. **Нужен audited contract/catalogue step**: conservative adapter должен
приниматься independent verifier по зарегистрированной transport schema; self-declared summary не принимается.
Backend binding/provider tests и #593 — внешний gate. C1 описывает N1 и не меняет runtime resolution.

## S1. Skip как uncatchable internal failure outcome

Existing FailureKind содержит Thrown/Program/Panic/Native/Cancelled; отдельного skip marker в OIR-2 нет.
Не предлагать masquerade под Panic/Program с магической строкой: это теряет классификацию и cleanup priority.
Предлагается явный **TestSkip** non-thrown kind с plain reason string, только trusted runtime constructor.
Marker живёт в существующем failure service/carrier transport, без пользовательского Error/ownership deps.
GC сохраняет reason/causes/secondaries; marker не содержит resource graph.

High: `call.intrinsic @okkama.test.skip(reason)` с registered concrete `(str)->void`, затем `unreachable`.
Summary `CallBoundary`, Borrow reason, no results/retention, atomic, sync; failures=[TestSkip].
Дополнительно нужен explicit registered `normal_returns=false` в CallContract (default true) и
ContractRegistry outcome validation: normal outcome отсутствует, normal CFG continuation запрещён.
Предлагаемая structural delta: `OpContract.normal: Option<OutcomeContract>`; ordinary ops получают Some,
skip — None. Builder/editor/edge schemas/codec и independent verifier должны согласованно это поддержать.
Нельзя считать `unreachable` доказательством отсутствующего normal outcome без контракта callee.
На Own entry adapter нет normal result/packet publication; принятые fresh inputs settled до unwind.
Tests-only registry разрешает вызов из ordinary helper, сохраняя current-test dynamic context.
Production registry не регистрирует intrinsic и не принимает reachable TestSkip effect.

Изменения separate reviewed step:

1. FailureKind/text codec/summary join/effect envelope: TestSkip и normal_returns marker.
   Propagate TestSkip по helper SCC до test root, не только из BodyOwner::Test.
2. Catch dispatch: только Thrown идёт в пользовательские Error patterns; TestSkip минует любой catch.
   Verifier rejects route в пользовательский catch и forged/user-created skip constructor.
3. Existing D8 unwind/defer/destroy/carrier очищает весь требуемый suffix, сохраняя skip marker.
   Cleanup failure делает case failed; skip не стирает прежнюю assertion failure.
   Не менять production failure precedence: для TestSkip явно определить, что любая реальная defer/destroy
   failure доминирует над skip; original skip остаётся diagnostic secondary при необходимости.
4. Test runner различает pass/fail/skip после cleanup, хранит assertion state отдельно от marker,
   выполняет следующий case и возвращает 1 при любом fail. Main не запускается; empty root suite — ошибка.
5. Independent verifier проверяет mode, signature, no-normal outcome, effect closure и failure edge slots.
   Backend transport/runner реализуется согласованно; отсутствие реализации остаётся внешним gate.

+ fixtures: direct/helper skip; skip через try/catch; defer/destroy перед следующим test;
skip после failed assertion; cleanup failure поверх skip; source order и main-not-called.
− fixtures: forged skip, production module, normal edge после skip, swallowed marker, dropped cleanup state,
неверный summary, превращение failed case в successful skip. Round-trip и semantic Own verification обязательны.
**Отдельный reviewed шаг с ± fixtures и независимым audit до C8 skip readiness** по решению Claude №3.

## F1. Fakes / test roots без нового storage protocol

`desugar_test_items` → ordinary object fields/defaults/ctors/successors и specialized role witnesses.
Ни fake.new, ни fake.storage не возвращаются. Test mode/root identities сохраняются при rebuild registry.
Owner-scoped tests — отдельные roots без implicit self/private authority; qualified names/source order сохраняются.
Module.tests=Some([]) отличается от production None. Assertions ok/is/isnt/fail используют existing core ops;
close/text/bytes — exact typed registered test intrinsics; throws — frontend desugaring.
Нового IR delta для F1 не предложено; registered assertion contracts и existing execution-mode verification
всё равно обязательны. Реальный runner и #593 evidence нужны для covered status.

## Состояние C1 и следующие решения

- [x] Прочитаны binding decisions, cases, corpus, ABI catalogue и 29 protocol pairs; описаны отличия базы 565.
- [x] Все TSV строки классифицированы; 22 первоначальные inherited rows имеют cross-reference.
- [x] Дополнительный Atomic<bool> dependency #568 записан явно; статус planned сохранён.
- [x] O1/D1/B1/N1/S1/F1 описаны как предложения с границами audit и concrete acceptance requirements.
- [x] Проверки C1: workspace check, strict clippy, разрешённый compiler lib oir2 suite.
- [x] Claude accepted C1 decisions O1/D1/B1/N1/S1/F1, as recorded in the C2a task instructions.
- [ ] O1/S1 audited implementation, source goldens и C2–C9 acceptance — будущая работа #569.

Проверки выполнены из `okkama-rs/`, offline, с четырьмя build jobs:

```text
cargo check --workspace --all-targets --offline -j 4
cargo clippy --workspace --all-targets --offline -j 4 -- -D warnings
cargo test -p okkama-compiler --lib oir2 --offline -j 4
```

Результаты: check PASS, clippy PASS; tests 142 passed, 0 failed, 0 ignored, 118 filtered out.
Первый clippy выявил лишний concat! в новом тесте; исправлено, повторная проверка прошла без warnings.
Четыре новых теста проверяют schema/paths/mode/outcome/exit и полный case inventory,
inherited/corpus cross-references, все 29 mappings и точные 9 opaque ABI rows.
Изменения остаются внутри coverage; test registration использует уже подключённый coverage/tests.rs.
Это inventory/build evidence, не acceptance будущих producer forms, новой версии Own или runtime/backend.
Ни source conformance programs, ни corpus runtime programs отдельно не исполнялись.
