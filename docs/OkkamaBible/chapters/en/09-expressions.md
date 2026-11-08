# Chapter 9. Expressions and Control Flow

Okkama's control flow fits in one sentence. It has two conditionals, `if` and
`match`, plus a compact two-way choice, `?? !!`. It has two loops: `for` over a
source and an unbounded `loop`. It has three exits — `next`, `last` and
`return` — and a postfix condition. There is no `while`, no `do`, no `switch`
with fall-through, no `goto`, no labels, no `+=` and no `++`. Most of what is
left are expressions: they produce values.

This chapter covers those constructs, the operators, and the order in which an
expression is evaluated. Other chapters cover the rest:

- failure (`throw`, `try`) — Chapter 10;
- cleanup (`defer`) — Chapter 11;
- patterns over enums — Chapter 16;
- `for await` — Chapter 20.

The program below uses nearly all of it.

{{example:09-expressions}}

## 9.1 Blocks have values

The value of a block is the value of its last expression, so an action needs
no `return` to produce its result. `letter` above is a single `if` chain:
whichever branch runs supplies the string. The semicolon after the last
expression is optional. `{ "A" }` and `{ "A"; }` are the same block, and the
formatter keeps whichever spelling the author chose.

The idea is older than most languages in use today:

- McCarthy's LISP introduced the conditional *expression* [@mccarthy1960].
- Landin's "The Next 700 Programming Languages" argued for building a language
  from expressions, with a statement as the special case of an expression
  whose value nobody uses [@landin1966].
- ML, Haskell, Scala, Kotlin and Rust followed that line.

C and Java followed ALGOL 60 instead. In them `if` is a statement, and since
programs still need a conditional value, both languages added a second
construct for it, `cond ? a : b`. Two spellings of one idea are exactly the
multiplication Chapter 1 argues against.

`if`, `match`, `loop` and `try` (Chapter 10) produce values, and so does every
call. A bare block `{ … }` does not. It is a statement that opens a scope, and
`let v = { … };` is a syntax error. A scope that has to produce a value
becomes an action, which gives it a name, a type and a test.

## 9.2 Names are bound once

`let` binds a name to a value, with an optional declared type:

```okkama
let limit = 60;                 // inferred: int
let names is @string = [];      // declared: a typed boundary (Chapter 4)
```

A name cannot be declared twice in the same scope. A nested block may shadow
it, and the outer meaning returns when the block ends. A name also cannot be
pointed at a new value:

{{example:09-rebind-rejected}}

The rule comes from the manifesto (Chapter 3): a name means what it was bound
to, so a reader never has to search forward for the assignment that changed
it. Scalars and collections are values, and there is no such thing as "the
same total, now bigger". The loop above has three idiomatic replacements:

- **`reduce`**, when the loop computes one value from a collection:
  `(1..=3).reduce(0) -> sum, n { sum + n; }` (Chapter 8).
- **A successor object**, when the changing state is a thing in the program's
  domain. A local whose type declares a successor action *may* be rebound to
  its successor (section 9.5 and Chapter 13).
- **A `Cell`**, the one explicit mutable container, which is allowed only in
  synchronous code (Chapter 13).

Without reassignment, compound assignment has nothing to abbreviate. So `+=`,
`-=` and the rest do not exist, and neither do `++` and `--`.

## 9.3 Operators

Operators, loosest first:

| Operators | Meaning |
|---|---|
| `?? … !!` | compact choice (9.4) |
| `else` | fallback for a `Maybe`, a missing index or key |
| `\|\|` | boolean or |
| `&&` | boolean and |
| `<-` | write to a stream |
| `==  !=` | equality |
| `..  ..=` | range |
| `<  <=  >  >=` | ordering |
| `\|`  `^`  `&` | bitwise or, exclusive or, and (in this order) |
| `<<  >>  >>>` | shifts |
| `+  -` | addition, concatenation, collection `+` and `-` (Chapter 8) |
| `*  /  %` | multiplication, division, remainder |
| `!  -  ~` | not, negation, bitwise complement |
| postfix | calls, member access, indexing, unpacking `...` |

The table departs from C in two places:

- **Bitwise operators bind tighter than comparisons.** `flags & mask != 0`
  means `(flags & mask) != 0`, which is what the programmer meant. In C the
  same text means `flags & (mask != 0)`. Ritchie traced the C rule to B, where
  `&` also served as logical *and*, and wrote that in hindsight moving `&`
  above `==` would have been better [@ritchie1993].
- **Writing to a stream has its own operator**, `<-`. As a result `<<` is
  always a shift.

Every operator requires operands of the right category and converts nothing:

- `&&`, `||`, `!` and every condition take a `bool`. That includes the
  conditions of `if`, `next if`, `confirm` and `??`. `if count { … }` is an
  error, not a test for zero.
- `==` and `!=` compare values of one category. `1 == 1.0` is rejected
  (Chapter 5), and so is comparing an `int` with a range.
- `<` and the other orderings need the same numeric category on both sides.
  `1 < 2.5` is rejected; write `1.float() < 2.5`.
- `+` adds numbers, joins strings and concatenates collections, and never
  mixes them: `"n = " + 1` is an error (Chapter 6).
- `/` on two integers gives an exact `rat`. There is no `**`; powers are
  written with `pow` (Chapter 5).

## 9.4 Choosing

### `if`

The condition of `if` is a `bool`, and the braces are mandatory: there is no
`if (c) statement;`. The brace-less form is behind one of the best-known
security bugs of the last decade. In 2014 Apple's TLS code had two consecutive
`goto fail;` lines under one unbraced `if`. The second line always ran, so the
signature on the server's key exchange was never verified [@langley2014]. With
mandatory braces, indentation cannot misrepresent which lines a condition
guards.

As an expression, `if` must have an `else`:

{{example:09-if-rejected}}

Kotlin and Rust reject this too. If the arm that is not written were allowed,
it would need a value, and Okkama has no such value to supply (Chapter 7).

An `else if` chain works in every position: as a statement, as the tail of an
action (as in `letter`) and as a value after `let x =`. Every link has its
`else`, and all branches have one type (issue #257).

### `match`

`match` compares a subject with each case's values using `==`. A case may list
several values, and `else` takes the rest. There is no fall-through and no
`break`: each case is its own block, and cases that share a body list their
values in one `case`, as `case "A", "B"` does above.

A case value is compared, not tested. `case 1..5` compares an `int` with a
range and is rejected as such; a condition on a value belongs in `if`. A
`match` that produces a value needs an `else`, unless its cases cover every
possibility, which only enums can do (Chapter 16). All its branches must have
one type.

### `?? !!`

`cond ?? a !! b` is a typed two-way choice:

- the condition is a `bool`;
- both branches have one type;
- only the chosen branch is evaluated.

On an enum, the same operator chooses by case and binds the payload
(Chapter 7 used it on `Maybe`; Chapter 16 covers the rest):

```okkama
let greeting = users.find(1) ?? .some(name): "hello, " + name !! .none: "hello, stranger";
```

Choices cannot be nested — not inside a branch, not in the condition, not
inside parentheses:

```text
syntax error: Nested ?? / !! choices are not allowed; use a local variable or match.
```

A nested ternary is legal in C and Java and hard to read in both. Okkama turns
the usual style advice into a rule, and following it costs one named local.

`?? !!` is the one place where Okkama keeps two spellings of one idea, and it
does so on purpose. The enum form chooses by pattern in a single expression,
which otherwise would take a full `match`. The boolean form comes along
because a language with the enum form and without the boolean one would be
stranger than a language with both.

## 9.5 Loops

### `for`

`for source -> name { … }` runs its body once per element. The source can be:

- an array;
- a range;
- an object that performs `Iterating<T>` (Chapter 8).

A map is walked through `keys()` or `values()`, in sorted key order. A string
is not a source: code points are reached by index today (issue #210). There is
exactly one binding. An index comes from a range:
`for 0..names.size() -> i`.

### `loop`

`loop { … }` repeats until `last`, `return` or a failure leaves it. It is an
expression: `last value` leaves the loop with that value. Each iteration runs
in a fresh scope, and the tail value of the body is ignored. A bare `last`
gives no value, so a `loop` whose value is used must end with `last value`;
otherwise the compiler reports "Bare `last` cannot produce the value of this
loop".

{{example:09-dice}}

The die is the second replacement from section 9.2. It never changes:
`rolled()` returns its successor, and the local `die` is rebound to that
successor. The rebinding is allowed because `Die` declares a successor action
(`is ^Die`). The rebinding inside the loop body updates the `die` declared
outside it, and the loop's value is the number of rolls.

There is no `while`, because `loop` already covers it:

- `while c { body }` is `loop { last if !c; body }`;
- `do { body } while c` is `loop { body; last if !c; }`.

Knuth's survey of structured programming discussed the loop whose exit is in
the middle [@knuth1974]. Neither `while` nor `do` can express that loop without
duplicating code; `loop` with `last if` writes it directly, and the two
classic loops become special cases of it. One construct does the work of
three.

### `next`, `last` and the postfix condition

`next` starts the next iteration and `last` leaves the loop. Both take a
postfix condition, and `last` can also carry a value. In `last value if found;`
the condition is evaluated first, and the value only if the condition holds.
A postfix `if` also works on an expression statement,
`out.writeln(line) if verbose;`, and on `return` and `throw`:
`return found if done;`, `throw Empty.list if items.size() == 0;`
(issue #258). Right after `return`, `if c;` is the bare form with a
condition, while `return if a { 1 } else { 2 };` returns an `if` expression.

`next` and `last` act on the nearest `for` or `loop`; there are no labels. To
leave two loops at once, move the inner loop into an action and `return` from
it, or make the inner loop a `loop` whose value tells the outer one what to
do.

`next` and `last` are also rejected outside a loop and directly inside a
functional block. A `map` visits every element, and searches have their own
operations, `any`, `all` and `find` (Chapter 8). These checks happen at
compile time, even in code that never runs.

### Unbounded loops and budgets

A `loop` without a `last` runs until something stops it. The step budget
(Chapter 26) counts every branch back to an earlier point of the program. So
`okkama run --max-steps N` stops any loop, on every engine, with the same
report.

## 9.6 `return`

`return value;` leaves the action from any depth. Most actions never need it,
because the tail value is the result. `return` is for early exits: a search
that has found its item, like `first_below` in the first example, or a guard
with nothing more to do.

A functional block is part of its call, not a function of its own
(Chapter 8). So `return` inside a `map` or `filter` block leaves the
*enclosing action*, just as a return inside a Smalltalk block leaves its home
method, or a `return` inside a Kotlin inline lambda leaves the calling
function.

## 9.7 Order of evaluation

{{example:09-order}}

An expression is evaluated by these rules:

- the receiver first, then the arguments left to right, each exactly once,
  all before the call;
- the left operand, then the right one, for every binary operator. Only `&&`
  and `||` may skip their right operand, as soon as the answer is known;
- for `if`, `match` and `?? !!`, the condition or subject once, then one
  branch;
- for `else`, the fallback only when the value is missing;
- for `last value if c`, `c` first, and `value` only if `c` holds.

C leaves the order of operands and arguments unspecified. `f() + g()` may call
either function first, and a program whose output depends on the order is
legal and unpredictable. Java fixed the order as left to right [@gosling2023],
and Okkama does the same.

Okkama has a second reason as well. Three engines run the same program — the
IR interpreter, the JVM and the Sparrow core — and the conformance suite
compares their outputs byte for byte. With an unspecified order, the expected
answer for a test would be a set of outputs instead of one.

## 9.8 What is not there

| Elsewhere | In Okkama |
|---|---|
| `while (c) { … }` | `loop { last if !c; … }` |
| `do { … } while (c)` | `loop { …; last if !c; }` |
| `break`, `continue` | `last`, `next` |
| `break outer` (labels) | an action and `return`, or a loop value |
| `x = x + 1`, `x += 1`, `x++` | a new name, `reduce`, a successor or a `Cell` |
| `c ? a : b` | `if c { a } else { b }` or `c ?? a !! b` |
| `switch` with fall-through | `match`, several values per `case` |
| `goto` | none [@dijkstra1968] |
| `if (c) statement;` | braces always |

The compiler knows this table. `while`, `do`, `+=` and its kin, `++`, `--`,
`break` and `continue` are rejected with a message that names the Okkama form
(issue #256):

```text
syntax error at "while": Okkama has no `while`: write `loop { last if !(condition); ... }`.
syntax error at "=": Okkama has no compound assignment: a name is bound once; bind a new name, or use `reduce` or a successor.
semantic error: Undefined variable "break"; Okkama leaves a loop with `last`
```

## 9.9 Limits

- **Strings are not a `for` source.** Code points are reached by index
  (issue #210).
- **One binding per `for`.** An index comes from a range, not from a second
  binding.

Writing this chapter turned up three more: messages for `while`, `+=` and
`break` that did not name the Okkama form, `else if` that worked only where
`if` was a statement, and `return` and `throw` without a postfix condition.
All three were fixed in 0.4.1 (issues #256, #257, #258).
