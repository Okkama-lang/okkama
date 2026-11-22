# Appendices

# Appendix A. Grammar

This appendix gives the syntax that the 0.6.2 parser accepts, written in EBNF:
`=` defines a production, `|` separates alternatives, `[ … ]` is optional,
`{ … }` repeats zero or more times, and `"…"` is a terminal. Names in
CAPITALS are lexical tokens (A.1). A program that matches the grammar can still
be rejected by the checker. The chapters give those rules; where the parser
accepts a form the checker always refuses, a note says so.

## A.1 Lexical structure

**Source text and whitespace.** Spaces, tabs, CR and LF separate tokens.
Outside literals and comments only ASCII is allowed; any other character is a
lexical error.

```text
line_comment  = "//" { any character except LF } ;
block_comment = "/*" { any character } "*/" ;      (* does not nest: the first "*/" closes it *)
```

**Words.** Identifiers and keywords share one shape. The parser enforces
case by position, with three classes.

```text
WORD       = ( letter | "_" ) { letter | digit | "_" } ;   (* ASCII only *)
LOWER_NAME = ( "a"…"z" | "_" ) { letter | digit | "_" } ;  (* fields, parameters, actions, enum cases, aliases, module segments *)
TYPE_NAME  = "A"…"Z" { letter | digit | "_" } ;            (* objects, roles, enums, caps, type parameters, constructors *)
BINDING    = WORD that is not a keyword ;                  (* let, for, catch, const and cap-field names: any case *)
```

- A member name after `.` and an action name may be a keyword: `test.ok()`,
  `xs.last()`, an action named `has(…)` or `is(…)`. A removed word may not.
- An action may not be named `with` or start with `with_`. The parser refuses
  such a name both in a declaration and after `.`.
- `<` after a type name opens type arguments only when the name contains a
  lower-case letter (`List<int>`). `LIMIT < x` is a comparison.

**Keywords.** These words cannot be used as names (except in the member and
action positions above):

```text
as async await case catch confirm const defer destroy does else enum
false fake for has if import is last let loop match module native
next object panic return role self spawn take test throw true try
```

**Contextual words** are ordinary identifiers that mean something only in one
position:

| Word | Meaning |
|---|---|
| `hidden` | before `object`, `role`, `enum` or `const` at module scope; before an action in an object |
| `cap` | starts a cap when another identifier follows |
| `once` | only directly before `let` |
| `void` | result marker after `is` |
| `main` | the only action allowed at module scope |
| `_` | the remainder case in `case _` |

Words from other languages get a dedicated message instead of a generic one:
`class`, `struct`, `interface`, `trait` and `protocol` before a type name;
`extends`, `inherits` and `implements` after an object header; `static` and
`view` in an object; `while`, `until` and `do` as statements; `x += 1`, `x++`
and `x--`.

**Reserved (removed) syntax.** The lexer still recognises these spellings, and
the parser rejects every one of them with a message:

| Spelling | Message |
|---|---|
| `null`, `nil`, `none` | Null/none/nil values are not part of Okkama v1.0 syntax. |
| `isa`, `super` | Inheritance is not part of Okkama v1.0 syntax. |
| `abstract`, `protected`, `with`, `finally` | "…" is reserved and not part of Okkama v1.0 syntax. (`finally` after a `try`: finally is not supported.) |
| `?` | Single ? is not supported; use Maybe<T> or Presence<T> for optional types. |
| `??` | ?? / !! selection was removed; use a match with compact cases… |
| `?.` | Optional chaining is not part of Okkama v1.0 syntax. |
| `**` | Exponent operator is not part of Okkama v1.0 syntax. |

`none` is still accepted as a member name, so `Maybe.none` works.

**Literals.**

```text
INT       = DIGITS                                       (* decimal, unbounded *)
          | ( "0x" | "0X" ) HEX { HEX | "_" }            (* "_" only between digits *)
          | ( "0b" | "0B" ) BIN { BIN | "_" } ;          (* based literals at most 2^63 - 1 *)
DIGITS    = digit { digit | "_" } ;                      (* "_" only between two digits *)
FLOAT     = DIGITS "." DIGITS [ EXPONENT ]
          | DIGITS EXPONENT ;                            (* 1e5, 1.5E-3, 2e+10; no ".5", no "1." *)
EXPONENT  = ( "e" | "E" ) [ "+" | "-" ] DIGITS ;
STRING    = '"' { char | ESCAPE } '"'                    (* may span lines; newlines kept *)
          | "```" { any character } "```"                (* raw: no escapes, CR kept; first ``` closes *)
          | "endl" ;                                     (* exactly "\n" *)
CHAR      = "'" ( char | ESCAPE ) "'" ;                  (* exactly one character, no newline *)
ESCAPE    = "\n" | "\r" | "\t" | "\\" | '\"' (string) | "\'" (char)
          | "\u{" HEX [HEX] [HEX] [HEX] [HEX] [HEX] "}" ;  (* a Unicode scalar value *)
BOOL      = "true" | "false" ;
```

There are no octal literals (`0o17` is the lexical error "octal literals are not supported", #968) and no `\0` escape. Writing `1.` leaves the dot for
member access, which is why `1..3` is a range.

**Operators and punctuation.** Operators are matched by longest match.

```text
+  -  *  /  %  &  &&  |  ||  ^  ~  !  <<  >>  >>>
==  !=  <  <=  >  >=  <-  =  ->
.  ..  ..=  ...  ,  ;  :  ::  @  #
(  )  {  }  [  ]
```

When the type parser closes generic arguments, it splits `>>`, `>>>` and `>=`,
so `Box<Box<int>>` parses. `#` appears only to report the removed action
directives.

## A.2 Modules and declarations

```text
program     = module_decl { import_decl } { module_item } EOF ;
module_decl = "module" LOWER_NAME { "/" LOWER_NAME } [ version ] ";" ;
version     = "v" INT "." INT "." INT ;                  (* written v1.2.3 *)
import_decl = "import" STRING [ "as" LOWER_NAME ] [ "take" name { "," name } ] ";" ;
```

A module segment may be a keyword-shaped lower-case word (`module test/more`).
Imports must come before the first declaration ("Imports must appear before
module declarations."). A standard-library path such as
`std.io.stdout.Stdout()` is an ordinary expression (A.4), not an import form.

```text
module_item = [ "hidden" ] ( object_decl | role_decl | enum_decl | const_decl )
            | external_cap
            | test_decl
            | native_decl
            | main_decl ;
const_decl  = "const" BINDING [ "is" type ] "=" expression ";" ;
main_decl   = [ "async" ] "main" "(" ")" "is" result block ;
```

The only action allowed at module scope is `main`, and it takes no parameters:
"Only main() may be declared at module scope.", "main() cannot declare
parameters." (rejected by the checker: a result other than `int`).

### Objects

```text
object_decl   = "object" TYPE_NAME [ generic_params ] [ "does" type { "," type } ] object_body ;
object_body   = "{" { object_member } "}" ;
object_member = has_decl | constructor | action_decl | "hidden" action_decl
              | cap_decl | destroy_decl | test_decl ;
has_decl      = "has" LOWER_NAME { "," LOWER_NAME } ( "is" type [ "=" expression ] | "=" expression ) ";" ;
destroy_decl  = [ "async" ] "destroy" block ;
```

- An object header takes no parameters: "Object headers do not accept
  parameters in v1.0."
- `hidden` may stand only before an action. Before a constructor, a field, a
  cap, `destroy` or a test it is a syntax error. `hidden(` is an action named
  `hidden`, and `has(` is an action named `has`.
- A field without a type takes it from its initializer, at compile time:
  `has x = 1;` is `has x is int = 1;`. With neither a type nor an initializer
  (`has x;`) the checker asks for the type, and an initializer that settles
  no type (`[]`, `[:]`) needs it written.

```text
constructor = TYPE_NAME parameter_list block                      (* full *)
            | TYPE_NAME "(" ")" ";"                               (* zero-field shorthand *)
            | TYPE_NAME "(" ":" LOWER_NAME { "," ":" LOWER_NAME } [ "," ] ")" ( ";" | block ) ;
```

The constructor's name must match the object: "Constructor name must match
enclosing object." A shorthand cannot mix `:field` entries with typed
parameters. `Name() {}` and `Name();` are different forms. Full constructors
take parameter defaults like actions.

```text
action_decl    = [ "async" ] LOWER_NAME parameter_list "is" result block ;
result         = "void" | "^" type | type ;
parameter_list = "(" [ param_group { "," param_group } [ "," ] ] ")" ;
param_group    = LOWER_NAME { "," LOWER_NAME } "is" type          (* names share one type *)
               | parameter ;
parameter      = LOWER_NAME "is" [ "..." ] type [ "=" expression ] ;
```

- The result is mandatory: "Action requires an explicit result: "is <type>"
  or "is void"."
- `is ^T` declares a successor action.
- One variadic parameter is allowed, and it must be last. A variadic
  parameter cannot have a default, and a group of names cannot be variadic.
- In a group, only the last name takes the default: in `f(x, y is int = 5)`,
  `x` is required.
- Defaults are rejected by the checker in role signatures, enum payloads,
  natives (syntax error), and actions with a variadic parameter.

### Caps

```text
cap_decl     = "cap" TYPE_NAME "{" { cap_field } "}" ;             (* inside an object *)
external_cap = "cap" name "::" TYPE_NAME "{" { cap_field } "}" ;   (* at module scope *)
cap_field    = BINDING "is" type block ;                           (* no "^", no parameters *)
```

Inside an external cap, `Owner::field` reads a private field of the owner.

### Roles, fakes and enums

```text
role_decl   = "role" TYPE_NAME [ generic_params ] "{" { role_member } "}" ;
role_member = signature | fake_decl | test_decl ;
signature   = [ "async" ] LOWER_NAME parameter_list "is" result ";" ;
fake_decl   = "fake" object_body ;
```

Role type parameters cannot have bounds: "Generic bounds are currently
supported only on object declarations." A cap in a role or a fake is an
error. A fake body uses the object-member grammar without the
constructor-name check.

```text
enum_decl = "enum" TYPE_NAME [ generic_params ] [ "is" TYPE_NAME ] "="
            "(" [ enum_case { "," enum_case } [ "," ] ] ")" ";" ;
enum_case = LOWER_NAME [ parameter_list ] ;                        (* payload fields *)
```

- Enum type parameters cannot have bounds (syntax error).
- An upper-case case name is a syntax error: "Expect enum case name."
- An enum with no cases, `enum E = ();`, parses, and the checker rejects it:
  "Enum "E" has no cases; an enum needs at least one" (#966).
- `is Error` makes an error enum (rejected by the checker: type parameters on
  an error enum, and any role other than `Error`).

### Tests and natives

```text
test_decl   = [ "async" ] "test" STRING block ;
native_decl = "native" "object" TYPE_NAME ";"
            | "native" [ "async" ] LOWER_NAME parameter_list "is" result ";" ;
```

- A test may stand at module scope, in an object or in a role.
- In production parsing (`okkama ast --mode production`), test and fake bodies
  are skipped as balanced braces.
- Natives are allowed only at module scope. They take no defaults, no
  variadic parameter and no body, and a native object takes no type
  parameters.

### Generic parameters

```text
generic_params = "<" type_param { "," type_param } [ "," ] ">" ;
type_param     = TYPE_NAME [ ":" type ] ;                          (* bound: objects only *)
```

A parameter may not repeat or shadow the declaration's own name.

## A.3 Statements

```text
block     = "{" { statement } "}" ;
statement = let_stmt | once_let | rebind | self_bind | successor_bind
          | if_stmt | match_stmt | for_stmt | loop_stmt | try_stmt
          | next_stmt | last_stmt | return_stmt | throw_stmt
          | defer_stmt | confirm_stmt | block | expr_stmt ;
```

The final statement of a block may omit its `;` when it is an expression
statement or a compact `if`. `if`, `match`, `loop` and `try` statements take
one optional `;`.

```text
let_stmt       = "let" BINDING [ "is" type ] "=" expression ";" ;
once_let       = "once" let_stmt ;                  (* checker: only at top level of an object's action *)
rebind         = LOWER_NAME "=" expression ";" ;    (* constructor field bind, or local rebind *)
self_bind      = "self" "." LOWER_NAME "=" expression ";" ;     (* checker: constructors only *)
successor_bind = "^" LOWER_NAME "=" expression ";" ;            (* checker: successor actions only *)
```

`other.field = value;` is a syntax error: "Only "self.field" may be assigned;
object fields are not mutable." A local's name may be upper-case (`let X = 1;`).

```text
if_stmt     = "if" expression block [ "else" ( if_stmt | block ) ]
            | compact_if ;                          (* compact form is an expression statement *)
compact_if  = "if" expression ":" expression "else" ":" expression ;
```

The two forms do not mix: `if c: a else { … }` and `if c { … } else: b` are
errors. A compact branch is one expression, never a block. A fallback inside a
branch needs parentheses: `if c: (x else y) else: z`.

```text
match_stmt = "match" expression "{" { match_arm } "}" ;
match_arm  = "case" "_" case_body
           | "case" pattern { "," pattern } case_body
           | "else" block ;
case_body  = block | ":" [ "throw" ] expression ";" ;
pattern    = expression ;     (* a value, Enum.case, or Enum.case(binding | "_", …) *)
```

- `case _` and `else` both name the remainder, and a match has at most one of
  them.
- `else` always takes a block; `else:` is a syntax error.
- Patterns are parsed as expressions. The checker accepts values and variant
  patterns whose bindings are names or `_`. Nested patterns and ranges are
  rejected.

```text
for_stmt    = "for" [ "await" ] source "->" for_binding { "," for_binding } block ;
for_binding = BINDING ;                            (* one name *)
loop_stmt   = "loop" block ;
next_stmt   = "next" [ "if" expression ] ";" ;     (* never a value *)
last_stmt   = "last" [ expression ] [ "if" expression ] ";" ;
```

- An arrow at the top level of the `for` source belongs to the `for`, so a
  functional call used as the source needs parentheses.
- A `for` without a source is an error: use `loop`.
- A binding is one name. `x[64]` is a syntax error: batching belongs to the
  source (a source that does `Batching<T>` binds one `@T` per step).

```text
return_stmt  = "return" [ expression ] [ "if" expression ] ";" ;
throw_stmt   = "throw" [ expression ] [ "if" expression ] ";" ;   (* bare: rethrow *)
defer_stmt   = "defer" expression ";" ;                           (* an expression, not a block *)
confirm_stmt = "confirm" expression [ "else" ( "throw" expression | "panic" expression ) ] ";" ;
expr_stmt    = expression [ "if" expression ] ";" ;
```

- Right after `return`, `throw` or `last`, `if c;` is the bare form with a
  condition, while `if c { … } else { … }` is a value.
- In a `confirm` condition, an index's `else` belongs to the `confirm`; in
  `(xs[i] else 0)` it is a fallback.
- `panic;` is a syntax error: use `throw;`.

```text
try_stmt     = "try" block catch_clause { catch_clause } ;
catch_clause = "catch" BINDING "is" type { "," type } block
             | "catch" pattern { "," pattern } block ;   (* Enum, Enum.case, Enum.case(a, _), ns.Enum.case *)
```

A lone lower-case name without `is` is an error ("Expect "is" after catch
error name."); there is no catch-all form, and `finally` is not supported.

## A.4 Expressions

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

Precedence from loosest to tightest. Every binary level is left-associative
except ranges, which do not chain ("Range expressions cannot be chained.").

| Level | Operators |
|---|---|
| fallback | `else` (right side is a whole expression) |
| or | `\|\|` |
| and | `&&` |
| stream write | `<-` |
| equality | `==` `!=` |
| range | `..` `..=` (one per expression) |
| comparison | `<` `<=` `>` `>=` |
| bitwise or | `\|` |
| bitwise xor | `^` |
| bitwise and | `&` |
| shift | `<<` `>>` `>>>` |
| additive | `+` `-` |
| multiplicative | `*` `/` `%` |
| prefix | `!` `-` `~` `await` `spawn` `panic` |
| postfix | `::name`, `.name`, call and functional block, `[i] else f`, `...` |

These levels have some consequences:

- Comparisons are looser than bitwise operators, so `a & 3 == 1` is
  `(a & 3) == 1`.
- `1..n + 1` is `1..(n + 1)`.
- `await x + 1` is `(await x) + 1`, and `!a.b()` is `!(a.b())`.
- `try? e` and `try! e` cover the operand up to an `else`: `try? f() else 0`
  is `(try? f()) else 0`.
- An `else` followed by `{` or `:` never starts a fallback.

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

- A `(` starts an anonymous action when it is followed by `)` or by
  `name is` / `a, b is`; otherwise it is a grouping.
- Anonymous actions cannot be `async`, variadic or successors, and their
  parameters take no defaults (a syntax error, #958).
- A range inside an array literal is spread: `[0, 1..=3, 9]` holds five
  numbers.
- Casts such as `int(x)`, `float(n)` and `byte(n)` are ordinary calls;
  `@int(…)` is the array cast.

```text
if_expr    = "if" expression block "else" ( if_expr | block ) | compact_if ;
match_expr = "match" expression "{" { match_arm } "}" ;
loop_expr  = "loop" block ;                       (* value from "last value;" *)
try_expr   = "try" block catch_clause { catch_clause }
           | ( "try?" | "try!" ) binary ;         (* no space before ? or ! *)
```

An `if` used as a value must have an `else`: "If expression requires else
branch."

**Functional blocks.** A member call of one of six operations may be followed
by bindings and a block:

```text
functional = "->" LOWER_NAME { "," LOWER_NAME } block ;
(* receiver.map() | .filter() | .any() | .all() | .find() take no arguments;
   receiver.reduce(initial) takes exactly one; named arguments are refused *)
```

Any other operation name before `->` is an error: `Unsupported functional
operation "each".`

**Unpack and spread.** Postfix `...` unpacks an array or a variadic
binding: `let all = values...;`, `f(first, rest...)`, `for values... -> v`.
(rejected by the checker: elsewhere, for example inside an array literal.)

**Spawn.** `spawn` needs a call operand. `spawn(` is an ordinary call of an
action named `spawn`.

**Test helpers.** Inside test code, two forms have their own syntax:

```text
test_throws = "test" "." "throws" "(" type ")" block ;        (* value: the caught error *)
test_case   = "test" "." "case" "(" expression ")" block ;    (* a named sub-case *)
```

## A.5 Types

```text
type        = "@" type                                          (* array *)
            | "%" type ":" type                                 (* map *)
            | [ "async" ] "(" [ type { "," type } ] ")" "is" ( "void" | type )   (* action type *)
            | named_type ;
named_type  = name { "." name } [ "<" type { "," type } ">" ] [ "::" name ] ;
result      = "void" | "^" type | type ;                        (* after "is" in a declaration *)
```

- `^T` exists only as a declaration result, never inside another type.
- `void` is only a result marker: "void is only allowed as an action result
  marker."
- Type arguments take no trailing comma.
- `ns.Name` names a type through an import namespace. `Owner::Cap` and
  `Box<int>::Tag` name cap types.
- The built-in names are ordinary identifiers that the checker resolves:
  `int`, `float`, `rat`, `byte`, `bool`, `string`, `char`, `any`, and the
  generics `Future<T>`, `Maybe<T>` and `Presence<T>`.
- Bounds (`T: Comparing<T>`) belong to generic parameters (A.2).

| Example | Meaning |
|---|---|
| `@string`, `@@int` | array of strings, array of arrays |
| `%string:@int` | map from string to array of int |
| `(int, int) is int` | synchronous action type |
| `async (int) is string` | async action type (a bound async action) |
| `Box<Maybe<int>>` | nested arguments (`>>` is split) |
| `Customer::BusinessCard` | cap type |

