# Приложения

# Приложение A. Грамматика

Это приложение описывает синтаксис, который принимает парсер версии 0.6.2, в
нотации EBNF: `=` определяет правило, `|` разделяет варианты, `[ … ]` —
необязательная часть, `{ … }` — повторение ноль и более раз, `"…"` —
терминал. Имена ПРОПИСНЫМИ буквами — лексические токены (A.1). Программа,
подходящая под грамматику, ещё может быть отвергнута проверкой типов и правил;
эти правила изложены в главах, а там, где парсер принимает форму, которую
проверка всегда отвергает, стоит пометка.

## A.1 Лексическая структура

**Текст и пробелы.** Пробелы, табуляции, CR и LF разделяют токены. Вне
литералов и комментариев допустим только ASCII; любой другой символ —
лексическая ошибка.

```text
line_comment  = "//" { any character except LF } ;
block_comment = "/*" { any character } "*/" ;      (* does not nest: the first "*/" closes it *)
```

**Слова.** Идентификаторы и ключевые слова имеют одну форму; регистр парсер
проверяет по позиции.

```text
WORD       = ( letter | "_" ) { letter | digit | "_" } ;   (* ASCII only *)
LOWER_NAME = ( "a"…"z" | "_" ) { letter | digit | "_" } ;  (* fields, parameters, actions, enum cases, aliases, module segments *)
TYPE_NAME  = "A"…"Z" { letter | digit | "_" } ;            (* objects, roles, enums, caps, type parameters, constructors *)
BINDING    = WORD that is not a keyword ;                  (* let, for, catch, const and cap-field names: any case *)
```

- Имя члена после `.` и имя действия может быть ключевым словом: `test.ok()`,
  `xs.last()`, действие `has(…)` или `is(…)`. Удалённое слово — не может.
- Действие не может называться `with` или начинаться с `with_`: парсер
  отвергает такое имя и в объявлении, и после `.`.
- `<` после имени типа открывает аргументы типа, только если в имени есть
  строчная буква (`List<int>`); `LIMIT < x` — сравнение.

**Ключевые слова.** Эти слова не могут быть именами (кроме позиций члена и
действия, описанных выше):

```text
as async await case catch confirm const defer destroy does else enum
false fake for has if import is last let loop match module native
next object panic return role self spawn take test throw true try
```

**Контекстные слова** — обычные идентификаторы, которые что-то значат только
в одной позиции:

| Слово | Значение |
|---|---|
| `hidden` | перед `object`, `role`, `enum` или `const` на уровне модуля; перед действием в объекте |
| `cap` | начинает cap, если за ним идёт ещё один идентификатор |
| `once` | только непосредственно перед `let` |
| `void` | маркер результата после `is` |
| `main` | единственное действие, допустимое на уровне модуля |
| `_` | остаток в `case _` |

Слова из других языков получают отдельное сообщение вместо общего: `class`,
`struct`, `interface`, `trait` и `protocol` перед именем типа; `extends`,
`inherits` и `implements` после заголовка объекта; `static` и `view` в
объекте; `while`, `until` и `do` как инструкции; `x += 1`, `x++` и `x--`.

**Зарезервированный (удалённый) синтаксис.** Лексер по-прежнему распознаёт эти
написания, и парсер отвергает каждое с сообщением:

| Написание | Сообщение |
|---|---|
| `null`, `nil`, `none` | Null/none/nil values are not part of Okkama v1.0 syntax. |
| `isa`, `super` | Inheritance is not part of Okkama v1.0 syntax. |
| `abstract`, `protected`, `with`, `finally` | "…" is reserved and not part of Okkama v1.0 syntax. (`finally` после `try`: finally is not supported.) |
| `?` | Single ? is not supported; use Maybe<T> or Presence<T> for optional types. |
| `??` | ?? / !! selection was removed; use a match with compact cases… |
| `?.` | Optional chaining is not part of Okkama v1.0 syntax. |
| `**` | Exponent operator is not part of Okkama v1.0 syntax. |

Как имя члена `none` по-прежнему допустимо, поэтому `Maybe.none` работает.

**Литералы.**

```text
INT       = DIGITS                                       (* decimal, unbounded *)
          | ( "0x" | "0X" ) HEX { HEX | "_" }            (* "_" only between digits *)
          | ( "0b" | "0B" ) BIN { BIN | "_" } ;          (* based literals at most 2^63 - 1 *)
DIGITS    = digit { digit | "_" } ;                      (* "_" only between two digits *)
FLOAT     = DIGITS "." DIGITS [ EXPONENT ]
          | DIGITS EXPONENT ;                            (* 1e5, 1.5E-3, 2e+10; no ".5", no "1." *)
EXPONENT  = ( "e" | "E" ) [ "+" | "-" ] DIGITS ;
STRING    = '"' { char | ESCAPE } '"'                    (* may span lines; newlines kept *)
          | "```

Восьмеричных литералов нет (`0o17` — лексическая ошибка «octal literals are not supported», #968), как
нет и экранирования `\0`. После `1.` точка остаётся для доступа к члену,
поэтому `1..3` — диапазон.

**Операторы и пунктуация.** Операторы распознаются по самому длинному
совпадению.

```text
+  -  *  /  %  &  &&  |  ||  ^  ~  !  <<  >>  >>>
==  !=  <  <=  >  >=  <-  =  ->
.  ..  ..=  ...  ,  ;  :  ::  @  #
(  )  {  }  [  ]
```

Закрывая аргументы обобщённого типа, парсер разделяет `>>`, `>>>` и `>=`,
поэтому `Box<Box<int>>` разбирается. `#` встречается только ради сообщения об
удалённых директивах действий.

## A.2 Модули и объявления

```text
program     = module_decl { import_decl } { module_item } EOF ;
module_decl = "module" LOWER_NAME { "/" LOWER_NAME } [ version ] ";" ;
version     = "v" INT "." INT "." INT ;                  (* written v1.2.3 *)
import_decl = "import" STRING [ "as" LOWER_NAME ] [ "take" name { "," name } ] ";" ;
```

Сегмент модуля может совпадать по форме с ключевым словом, если он в нижнем
регистре (`module test/more`). Импорты идут до первого объявления («Imports
must appear before module declarations.»). Путь стандартной библиотеки вида
`std.io.stdout.Stdout()` — обычное выражение (A.4), а не форма импорта.

```text
module_item = [ "hidden" ] ( object_decl | role_decl | enum_decl | const_decl )
            | external_cap
            | test_decl
            | native_decl
            | main_decl ;
const_decl  = "const" BINDING [ "is" type ] "=" expression ";" ;
main_decl   = [ "async" ] "main" "(" ")" "is" result block ;
```

На уровне модуля разрешено только действие `main`, и оно не принимает
параметров: «Only main() may be declared at module scope.», «main() cannot
declare parameters.» (проверка отвергает результат, отличный от `int`).

### Объекты

```text
object_decl   = "object" TYPE_NAME [ generic_params ] [ "does" type { "," type } ] object_body ;
object_body   = "{" { object_member } "}" ;
object_member = has_decl | constructor | action_decl | "hidden" action_decl
              | cap_decl | destroy_decl | test_decl ;
has_decl      = "has" LOWER_NAME { "," LOWER_NAME } ( "is" type [ "=" expression ] | "=" expression ) ";" ;
destroy_decl  = [ "async" ] "destroy" block ;
```

- Заголовок объекта не принимает параметров: «Object headers do not accept
  parameters in v1.0.»
- `hidden` допустим только перед действием; перед конструктором, полем, cap,
  `destroy` или тестом это синтаксическая ошибка. `hidden(` — действие с
  именем `hidden`, `has(` — действие с именем `has`.
- Поле без типа получает его от инициализатора во время компиляции:
  `has x = 1;` — это `has x is int = 1;`. Без типа и без инициализатора
  (`has x;`) проверка требует тип, а инициализатор, который тип не задаёт
  (`[]`, `[:]`), требует написать его явно.

```text
constructor = TYPE_NAME parameter_list block                      (* full *)
            | TYPE_NAME "(" ")" ";"                               (* zero-field shorthand *)
            | TYPE_NAME "(" ":" LOWER_NAME { "," ":" LOWER_NAME } [ "," ] ")" ( ";" | block ) ;
```

Имя конструктора совпадает с объектом: «Constructor name must match enclosing
object.» Сокращённая форма не смешивает записи `:field` с типизированными
параметрами. `Name() {}` и `Name();` — разные формы. Полные конструкторы
принимают значения параметров по умолчанию, как действия.

```text
action_decl    = [ "async" ] LOWER_NAME parameter_list "is" result block ;
result         = "void" | "^" type | type ;
parameter_list = "(" [ param_group { "," param_group } [ "," ] ] ")" ;
param_group    = LOWER_NAME { "," LOWER_NAME } "is" type          (* names share one type *)
               | parameter ;
parameter      = LOWER_NAME "is" [ "..." ] type [ "=" expression ] ;
```

- Результат обязателен: «Action requires an explicit result: "is <type>" or
  "is void".»
- `is ^T` объявляет действие-преемник.
- Вариативный параметр допустим один, и он последний. У него не бывает
  значения по умолчанию, и группа имён не может быть вариативной.
- В группе значение по умолчанию получает только последнее имя: в
  `f(x, y is int = 5)` параметр `x` обязателен.
- Значения по умолчанию проверка отвергает в сигнатурах ролей, данных
  вариантов перечислений и в действиях с вариативным параметром; в натив-
  объявлениях это синтаксическая ошибка.

### Cap

```text
cap_decl     = "cap" TYPE_NAME "{" { cap_field } "}" ;             (* inside an object *)
external_cap = "cap" name "::" TYPE_NAME "{" { cap_field } "}" ;   (* at module scope *)
cap_field    = BINDING "is" type block ;                           (* no "^", no parameters *)
```

Внутри внешнего cap `Owner::field` читает приватное поле владельца.

### Роли, фейки и перечисления

```text
role_decl   = "role" TYPE_NAME [ generic_params ] "{" { role_member } "}" ;
role_member = signature | fake_decl | test_decl ;
signature   = [ "async" ] LOWER_NAME parameter_list "is" result ";" ;
fake_decl   = "fake" object_body ;
```

У параметров типа роли не бывает ограничений: «Generic bounds are currently
supported only on object declarations.» Cap в роли или фейке — ошибка. Тело
фейка использует грамматику членов объекта без проверки имени конструктора.

```text
enum_decl = "enum" TYPE_NAME [ generic_params ] [ "is" TYPE_NAME ] "="
            "(" [ enum_case { "," enum_case } [ "," ] ] ")" ";" ;
enum_case = LOWER_NAME [ parameter_list ] ;                        (* payload fields *)
```

- У параметров типа перечисления не бывает ограничений (синтаксическая
  ошибка).
- Вариант с прописной буквы — синтаксическая ошибка: «Expect enum case name.»
- Перечисление без вариантов, `enum E = ();`, разбирается, и проверка его
  отвергает: «Enum "E" has no cases; an enum needs at least one» (#966).
- `is Error` делает перечисление ошибок (проверка отвергает параметры типа у
  перечисления ошибок и любую роль, кроме `Error`).

### Тесты и нативы

```text
test_decl   = [ "async" ] "test" STRING block ;
native_decl = "native" "object" TYPE_NAME ";"
            | "native" [ "async" ] LOWER_NAME parameter_list "is" result ";" ;
```

- Тест может стоять на уровне модуля, в объекте или в роли.
- При разборе в режиме production (`okkama ast --mode production`) тела тестов
  и фейков пропускаются как сбалансированные скобки.
- Нативы допустимы только на уровне модуля; у них нет значений по умолчанию,
  вариативного параметра и тела, а у натив-объекта — параметров типа.

### Параметры обобщённых типов

```text
generic_params = "<" type_param { "," type_param } [ "," ] ">" ;
type_param     = TYPE_NAME [ ":" type ] ;                          (* bound: objects only *)
```

Параметр не может повторяться или совпадать с именем самого объявления.

## A.3 Инструкции

```text
block     = "{" { statement } "}" ;
statement = let_stmt | once_let | rebind | self_bind | successor_bind
          | if_stmt | match_stmt | for_stmt | loop_stmt | try_stmt
          | next_stmt | last_stmt | return_stmt | throw_stmt
          | defer_stmt | confirm_stmt | block | expr_stmt ;
```

Последняя инструкция блока может опустить `;`, если это инструкция-выражение
или компактный `if`. Инструкции `if`, `match`, `loop` и `try` принимают одну
необязательную `;`.

```text
let_stmt       = "let" BINDING [ "is" type ] "=" expression ";" ;
once_let       = "once" let_stmt ;                  (* checker: only at top level of an object's action *)
rebind         = LOWER_NAME "=" expression ";" ;    (* constructor field bind, or local rebind *)
self_bind      = "self" "." LOWER_NAME "=" expression ";" ;     (* checker: constructors only *)
successor_bind = "^" LOWER_NAME "=" expression ";" ;            (* checker: successor actions only *)
```

`other.field = value;` — синтаксическая ошибка: «Only "self.field" may be
assigned; object fields are not mutable.» Имя локальной привязки может быть
прописным (`let X = 1;`).

```text
if_stmt     = "if" expression block [ "else" ( if_stmt | block ) ]
            | compact_if ;                          (* compact form is an expression statement *)
compact_if  = "if" expression ":" expression "else" ":" expression ;
```

Формы не смешиваются: `if c: a else { … }` и `if c { … } else: b` — ошибки.
Ветка компактной формы — одно выражение, никогда не блок; запасное значение
внутри ветки берётся в скобки: `if c: (x else y) else: z`.

```text
match_stmt = "match" expression "{" { match_arm } "}" ;
match_arm  = "case" "_" case_body
           | "case" pattern { "," pattern } case_body
           | "else" block ;
case_body  = block | ":" [ "throw" ] expression ";" ;
pattern    = expression ;     (* a value, Enum.case, or Enum.case(binding | "_", …) *)
```

- `case _` и `else` оба означают остаток, и у `match` не больше одного из них.
- `else` всегда принимает блок; `else:` — синтаксическая ошибка.
- Образцы разбираются как выражения; проверка принимает значения и образцы
  вариантов, привязки которых — имена или `_`. Вложенные образцы и диапазоны
  отвергаются.

```text
for_stmt    = "for" [ "await" ] source "->" for_binding { "," for_binding } block ;
for_binding = BINDING ;                            (* one name *)
loop_stmt   = "loop" block ;
next_stmt   = "next" [ "if" expression ] ";" ;     (* never a value *)
last_stmt   = "last" [ expression ] [ "if" expression ] ";" ;
```

- Стрелка на верхнем уровне источника `for` принадлежит самому `for`, поэтому
  функциональный вызов в роли источника берётся в скобки.
- `for` без источника — ошибка: пишите `loop`.
- Привязка — одно имя. `x[64]` — синтаксическая ошибка: пакеты — дело
  источника (источник, который `does Batching<T>`, привязывает по одному `@T`
  за шаг).

```text
return_stmt  = "return" [ expression ] [ "if" expression ] ";" ;
throw_stmt   = "throw" [ expression ] [ "if" expression ] ";" ;   (* bare: rethrow *)
defer_stmt   = "defer" expression ";" ;                           (* an expression, not a block *)
confirm_stmt = "confirm" expression [ "else" ( "throw" expression | "panic" expression ) ] ";" ;
expr_stmt    = expression [ "if" expression ] ";" ;
```

- Сразу после `return`, `throw` или `last` запись `if c;` — голая форма с
  условием, а `if c { … } else { … }` — значение.
- В условии `confirm` `else` у индекса принадлежит `confirm`; в
  `(xs[i] else 0)` это запасное значение.
- `panic;` — синтаксическая ошибка: пишите `throw;`.

```text
try_stmt     = "try" block catch_clause { catch_clause } ;
catch_clause = "catch" BINDING "is" type { "," type } block
             | "catch" pattern { "," pattern } block ;   (* Enum, Enum.case, Enum.case(a, _), ns.Enum.case *)
```

Одиночное имя в нижнем регистре без `is` — ошибка («Expect "is" after catch
error name.»); формы «поймать всё» нет, `finally` не поддерживается.

## A.4 Выражения

```text
expression = binary [ "else" ( "throw" expression | expression ) ] ;   (* fallback, loosest *)
binary     = unary { binary_op unary } ;                               (* by the table below *)
unary      = ( "!" | "-" | "~" ) unary
           | "await" unary
           | "spawn" unary                       (* operand must be a call *)
           | "panic" expression
           | postfix ;
postfix    = primary { "::" name | "." name | call [ functional ]
                     | "[" expression "]" [ "else" expression ] | "..." } ;
call       = "(" [ argument { "," argument } [ "," ] ] ")" ;
argument   = [ name ":" ] expression ;
```

Приоритеты от самого слабого к самому сильному. Все бинарные уровни
левоассоциативны, кроме диапазонов, которые не сцепляются («Range expressions
cannot be chained.»).

| Уровень | Операторы |
|---|---|
| запасное значение | `else` (справа — целое выражение) |
| или | `\|\|` |
| и | `&&` |
| запись в поток | `<-` |
| равенство | `==` `!=` |
| диапазон | `..` `..=` (один на выражение) |
| сравнение | `<` `<=` `>` `>=` |
| побитовое или | `\|` |
| побитовое исключающее или | `^` |
| побитовое и | `&` |
| сдвиг | `<<` `>>` `>>>` |
| аддитивные | `+` `-` |
| мультипликативные | `*` `/` `%` |
| префиксные | `!` `-` `~` `await` `spawn` `panic` |
| постфиксные | `::name`, `.name`, вызов и функциональный блок, `[i] else f`, `...` |

Следствия:

- сравнения слабее побитовых операторов: `a & 3 == 1` — это `(a & 3) == 1`;
- `1..n + 1` — это `1..(n + 1)`;
- `await x + 1` — это `(await x) + 1`, а `!a.b()` — это `!(a.b())`;
- `try? e` и `try! e` захватывают операнд до `else`: `try? f() else 0` — это
  `(try? f()) else 0`;
- `else`, за которым идёт `{` или `:`, никогда не начинает запасное значение.

```text
primary = literal | "self" | name
        | generic_type                            (* Maybe<int>.none; Box<int>(…) is rejected by the checker: "A type is not a value" *)
        | "(" expression ")"
        | anon_action
        | array_literal | map_literal
        | array_cast
        | if_expr | match_expr | loop_expr | try_expr ;
literal = INT | FLOAT | STRING | CHAR | "true" | "false" ;
```

```text
anon_action   = "(" [ param_group { "," param_group } ] ")" "is" ( "void" | type ) block ;
array_literal = "[" "]" | "[" expression { "," expression } [ "," ] "]" ;
map_literal   = "[" ":" "]" | "[" expression ":" expression { "," expression ":" expression } [ "," ] "]" ;
array_cast    = "@" ( "int" | "float" | "byte" | "string" ) call ;
```

- `(` начинает анонимное действие, если за ней идёт `)` либо `name is` /
  `a, b is`; иначе это группировка.
- Анонимное действие не может быть `async`, вариативным или преемником, и
  у его параметров нет умолчаний (синтаксическая ошибка, #958).
- Диапазон внутри литерала массива разворачивается: в `[0, 1..=3, 9]` пять
  чисел.
- Приведения `int(x)`, `float(n)`, `byte(n)` — обычные вызовы; `@int(…)` —
  приведение массива.

```text
if_expr    = "if" expression block "else" ( if_expr | block ) | compact_if ;
match_expr = "match" expression "{" { match_arm } "}" ;
loop_expr  = "loop" block ;                       (* value from "last value;" *)
try_expr   = "try" block catch_clause { catch_clause }
           | ( "try?" | "try!" ) binary ;         (* no space before ? or ! *)
```

`if`, использованный как значение, обязан иметь `else`: «If expression
requires else branch.»

**Функциональные блоки.** За вызовом одной из шести операций могут идти
привязки и блок:

```text
functional = "->" LOWER_NAME { "," LOWER_NAME } block ;
(* receiver.map() | .filter() | .any() | .all() | .find() take no arguments;
   receiver.reduce(initial) takes exactly one; named arguments are refused *)
```

Любое другое имя операции перед `->` — ошибка: `Unsupported functional
operation "each".`

**Распаковка.** Постфиксное `...` распаковывает массив или вариативную
привязку: `let all = values...;`, `f(first, rest...)`, `for values... -> v`
(в других местах, например внутри литерала массива, проверка отвергает).

**Spawn.** `spawn` требует операнд-вызов; `spawn(` — обычный вызов действия с
именем `spawn`.

**Помощники тестов.** В тестовом коде у двух форм свой синтаксис:

```text
test_throws = "test" "." "throws" "(" type ")" block ;        (* value: the caught error *)
test_case   = "test" "." "case" "(" expression ")" block ;    (* a named sub-case *)
```

## A.5 Типы

```text
type        = "@" type                                          (* array *)
            | "%" type ":" type                                 (* map *)
            | [ "async" ] "(" [ type { "," type } ] ")" "is" ( "void" | type )   (* action type *)
            | named_type ;
named_type  = name { "." name } [ "<" type { "," type } ">" ] [ "::" name ] ;
result      = "void" | "^" type | type ;                        (* after "is" in a declaration *)
```

- `^T` бывает только результатом объявления, но не частью другого типа.
- `void` — только маркер результата: «void is only allowed as an action result
  marker.»
- В аргументах типа нет завершающей запятой.
- `ns.Name` называет тип через пространство имён импорта; `Owner::Cap` и
  `Box<int>::Tag` — типы cap.
- Встроенные имена — обычные идентификаторы, которые разрешает проверка:
  `int`, `float`, `rat`, `byte`, `bool`, `string`, `char`, `any` и обобщённые
  `Future<T>`, `Maybe<T>`, `Presence<T>`.
- Ограничения (`T: Comparing<T>`) пишутся у параметров обобщённого типа (A.2).

| Пример | Значение |
|---|---|
| `@string`, `@@int` | массив строк, массив массивов |
| `%string:@int` | словарь из string в массив int |
| `(int, int) is int` | тип синхронного действия |
| `async (int) is string` | тип async-действия (связанного async-действия) |
| `Box<Maybe<int>>` | вложенные аргументы (`>>` разделяется) |
| `Customer::BusinessCard` | тип cap |
