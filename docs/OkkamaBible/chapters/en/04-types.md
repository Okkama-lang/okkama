# Part II. Values

# Chapter 4. Types at a Glance

Every value in an Okkama program has a type, and the compiler knows it before
the program runs. This chapter surveys the whole type system in one place —
what kinds of types there are, where they are written, how the compiler finds
the ones you do not write, and where the deliberate gaps are. The following
chapters take each kind in turn.

## 4.1 What a type is for

A type is a promise about a value: which operations it supports and what
they mean. Pierce's textbook definition is that a type system is "a tractable
syntactic method for proving the absence of certain program behaviors by
classifying phrases according to the kinds of values they compute"
[@pierce2002]. The important words are *absence* and *tractable*: a type
checker cannot prove that a program is right, but it can prove cheaply that
certain mistakes cannot happen — adding a string to a number, calling an
action an object does not have, forgetting that a value may be absent.

Okkama uses its type system for three jobs:

- **To reject meaningless programs** before they run — the examples of Chapter 3
  were all rejected this way.
- **To document intent.** A signature such as `share(people is int) is rat`
  tells a reader what goes in and what comes out without reading the body.

## 4.2 The inventory

The table lists every kind of type a program can write.

| Kind | Spelling | Examples | Chapter |
|---|---|---|---|
| Exact integer | `int` | `42`, `-7`, `9223372036854775808` | 5 |
| Octet | `byte` | `byte(255)` | 5 |
| Exact rational | `rat` | `1 / 3`, `rat(2, 6)` | 5 |
| Binary floating point | `float` | `3.14`, `0.1` | 5 |
| Truth value | `bool` | `true`, `false` | 9 |
| Text | `string` | `"Hello"` | 6 |
| One character | `char` | `'x'` | 6 |
| Array | `@T` | `@int`, `@@string` | 8 |
| Map | `%K:V` | `%string:int` | 8 |
| Object | its name | `Bottle`, `Percent` | 12 |
| Role | its name | `Price`, `Shape` | 14 |
| Enum | its name | `Unknown`, `Maybe<int>` | 16 |
| Applied generic | `Name<T>` | `Maybe<int>`, `List<string>` | 17 |
| Successor | `^T` | `^Bottle` | 13 |
| Anonymous action | `(T1, T2) is R` | `(int) is int` | 18 |
| Future | `Future<T>` | `Future<string>` | 20 |
| Top type | `any` | `any`, `@any` | 4.6 |

There are no pointer types, no reference-versus-value distinction, no
nullable variants of other types, no unsigned integers apart from `byte`, no
fixed-width integer zoo (`i8`…`u64`), and no `void` value — an action that
returns nothing is declared `is void`, and its "result" cannot be stored.

## 4.3 Where types are written

Types are written at *boundaries*: places where a value enters a name that
other code will rely on.

- `let name is T = …` — a local binding with a declared type;
- `has field is T` — a field;
- parameters and results of constructors and actions;
- `catch e is T` — what a handler accepts;
- the elements and keys of collection types.

Inside a body, most types are inferred. The program below writes types only
where they say something — the declared `rat`, the array of roles — and lets
the compiler find the rest.

{{example:04-types}}

Three details are worth noticing. `count` has no annotation, and its type is
`int` because `3` is an `int`; inference in Okkama is *local and exact* — a
binding gets exactly the type of its initialiser, never a wider one. `1 /
count` is a `rat`, because dividing integers is exact. And `shapes` holds two
different objects behind one role: `@Shape` is an array whose elements all
perform `Shape`, so the `reduce` may call `area()` on each without knowing
which object it is.

## 4.4 Nominal, not structural

Two type systems can disagree about when two types are "the same". In a
*structural* system — TypeScript, Go's interfaces, OCaml's objects — a type
is compatible with another if it has the right shape: the same fields or
methods. In a *nominal* system — Java's classes, Rust's traits, Okkama — it is
compatible only if a declaration says so [@cardelli1985; @pierce2002].

Okkama is nominal for objects and roles. `Triangle` performs `Shape` because
it says `does Shape`; an object with an action `area() is rat` that does not
say so is not a `Shape`. Chapter 3 gave the reason: a shared method name is
not a shared contract. Nominal roles cost a word in each declaration and buy
the certainty that every implementation of a role was written *as* an
implementation of it, by someone who could read the role's documentation.

Between objects there is no subtyping at all. `Square` is not a subtype of
`Triangle`, and no object is a subtype of another object; the only relation
is "performs a role". This removes the variance puzzles that make generic
code in languages with class hierarchies hard to write — whether a list of
squares is a list of shapes — at the price of writing `@Shape` where a
hierarchy would have inferred it.

## 4.5 Inference: local and exact

Okkama infers the types of local bindings, of the results of expressions,
and of generic type arguments, but it does so only from what is directly
visible, and it never widens.

- `let x = expression;` gives `x` the type of `expression`.
- An array literal's element type is the type of its elements: `[1, 2, 3]`
  is `@int`. A literal with mixed elements such as `[1, "two"]` has no
  element type, and it never silently becomes `@any`: it compiles only where
  a declared `@any` boundary expects it (`let mixed is @any = [1, "two"];`),
  and anywhere else the compiler rejects it and names both types.
- A generic object's type arguments are inferred from its constructor's
  arguments when they determine them; otherwise they must be written:
  `let empty is List<string> = EmptyList();`.

This is deliberately weaker than the Hindley–Milner inference of ML and
Haskell, which finds the most general type of a whole program without any
annotations [@milner1978]. Global inference is powerful, but its error
messages can point far from the mistake, and its results depend on code the
reader is not looking at. Okkama annotates every declaration — parameters,
results, fields — and infers inside bodies, so that a type is always found
within a few lines of where it is used.

Inference has one known rough edge: an empty literal needs a declared type,
because it contains no evidence. Until 0.4.1 anonymous actions had a second
one: they could not declare their result type, so some generic calls needed
an annotation on the binding that received them. Since issue #214 an
anonymous action may state it, `(n is int) is int { n * 2; }`.

## 4.6 The top type `any`

`any` is the one type that every value can enter. It exists for the places
where heterogeneity is the point: a database row of mixed columns, a log
record with fields of different kinds, a generic transport.

A value that enters `any` becomes an *opaque box*: it remembers what it is,
but the program cannot look inside. Arithmetic, comparison, member access,
indexing and `match` on an `any` are all rejected, and nothing converts an
`any` back to a concrete type implicitly.

{{example:04-any-rejected}}

This makes `any` a *boundary*, not an escape from typing. In TypeScript `any`
switches the checker off for everything it touches; in Okkama it switches
*access* off. Code may pass, store and return boxed values, and only the
trusted native layer — which receives them unboxed — or a future explicit
narrowing construct can open them.

## 4.7 Equality and order

`==` asks whether two values are the same value, and the answer depends only
on what the values are, never on where they live. Okkama has no reference
equality: there is no "same object" test, because two objects with equal
fields are interchangeable everywhere a program can observe them.

{{example:04-equality}}

- **Objects compare structurally.** Two `Square(side: 2)` are equal; Chapter
  12 shows how this extends to nested objects, and why it lets objects serve
  as map keys.
- **Collections compare element by element.** Arrays are equal when they
  have the same length and equal elements in order; maps when they have the
  same keys with equal values.
- **Values behind a role** are equal when they are the same kind of object
  with equal fields. A `Square` and a `Circle` behind `Shape` are simply
  unequal: the comparison is allowed, because both are `Shape` values, and
  its answer is `false`.
- **Numbers.** `int`, `byte` and `rat` are exact, so `1 / 2 == 2 / 4` is
  true: both are the rational one half. A `float` is equal only to the same
  binary64 value, so `0.1 + 0.2 == 0.3` is false (Chapter 5).

Comparing values of two different concrete types is not `false` but a
compile-time error. A program that asks whether an `int` equals a `float`, or
an `Invoice` equals a `Customer`, has made a mistake that no run can make
meaningful, and the compiler says so:

{{example:04-compare-rejected}}

Order is narrower than equality. `<`, `<=`, `>` and `>=` apply to numbers and
to objects that perform the standard role `Comparing<T>`, whose
`order(other)` they call (Chapter 14). Text has no operators: `order(other)`
and `less(other)` compare strings by code point, so `"Zebra"` comes before
`"apple"`, which is what a program that sorts identifiers wants and what a
program that sorts names for people does not; a locale-aware order is a
library's job, not an operator's (Chapter 6). Booleans, characters, arrays
and maps have no order at all, and asking for one is a compile-time error.

## 4.8 Conversions are explicit

Okkama never converts a value from one type to another behind the
programmer's back. Every conversion is an action with a name, and the name
says what is lost.

| From → to | Written | What happens |
|---|---|---|
| `int` → `rat` | `n.rat()` | exact |
| `int` → `float` | `n.float()` | the nearest binary64 value |
| `int` → `byte` | `n.byte()`, `byte(n)` | fails outside 0..255 |
| `byte` → `int` | `b.int()` | exact |
| `float` → `int` | `x.int()` | drops the fraction toward zero; fails outside the signed 64-bit range |
| `float` → `rat` | `x.rat()` | exact: `2.5.rat()` is `5/2` |
| `rat` → `int` | `q.int()` | drops the fraction toward zero |
| `rat` → `float` | `q.float()` | the nearest binary64 value |
| `int` → `char` | `n.char()` | the character with that code point; fails for a surrogate or a value beyond 0x10FFFF |
| `char` → `int` | `c.code()` | its code point |
| any value → `string` | `v.string()` | its canonical text |

Two operations change type by design rather than by conversion, and both are
exact. Dividing integers gives a rational, `7 / 2` is `7/2`; and arithmetic
on bytes gives an integer, `byte(250) + byte(10)` is `260`, because a byte is
an octet to store and send, not a number that wraps. Mixing kinds in one
operation is a compile-time error too: `1 + 1.0` is rejected with "Operator +
cannot be applied to int and float". The cost is a call where C or JavaScript would convert
silently; the gain is that every place a value changes its kind is written
down, with the name of what it does.

## 4.9 What the compiler checks, and what remains for run time

The Okkama compiler checks, before anything runs:

- that every type name refers to a declared type with the right number of
  generic arguments;
- that operators receive operands of compatible categories (`int` with
  `float` is an error, Chapter 5);
- that every action call names an action the receiver's type has, with
  arguments of the right types;
- that conditions are `bool`, successors are used, fields are not accessed
  from outside, and objects that claim a role implement all of it;
- that a `match` used as a value covers every case of its enum, so that it
  always produces one (a non-exhaustive `match` used as a statement is
  allowed, and the editor flags it with a lint).

Three short programs show what such a refusal looks like. The first
multiplies a count by a price:

{{example:04-mix-rejected}}

The fix is a conversion the reader can see, `items.float() * price` or a
`rat` price, and which one is right is the program's decision, not the
compiler's (Chapter 5). The second holds a square behind the role it
performs and asks for something the role does not promise:

{{example:04-role-call-rejected}}

A role reference offers the role's actions and nothing else, even when the
object behind it has more; reaching `side()` means holding a `Square`, or
adding `side()` to the role (Section 14.3). The third claims a role and
forgets half of it:

{{example:04-incomplete-rejected}}

Each message names the place, the types and the missing piece. None of these
programs runs at all: the error is found where it was written, not on the
first input that reaches it.

Some checks can only happen while the program runs, because they depend on
values: an index within an array's bounds, a key present in a map, a
conversion such as `byte(256)` in range, a division by a non-zero number, a
float result that stays finite. These are *runtime failures* (Chapter 10):
precise, located errors that stop the program — or, in the case of typed
errors, that a caller may catch.

Types are checked before execution; the remaining dynamic checks concern values, such as bounds and divisors.

## 4.10 A worked example: units the compiler checks

On 23 September 1999 the Mars Climate Orbiter was lost as it entered orbit
around Mars. The investigation board found the root cause in a ground
program, `SM_FORCES`, whose output file was specified in newton-seconds and
was written in pound-force seconds. The navigation software read the numbers
as newton-seconds, underestimated the effect of the thruster firings by a
factor of 4.45, and computed a trajectory that took the spacecraft far too
close to the planet [@nasa1999mco]. Both programs were correct about their
own numbers. The error lived at the boundary between them, where a number
crossed without its unit.

In Okkama a unit can be a type. Two objects with the same single field are
still two types, because the type system is nominal (Section 4.4):

{{example:04-units}}

- **The unit is in the type, not in a comment.** `NewtonSeconds` and
  `PoundSeconds` each hold one `rat`. Nothing about their shape tells them
  apart, and nothing has to: they are different declarations.
- **Conversion is an action with a name.** `newton_seconds()` is the only
  way from one to the other, and its factor is written once, exactly: one
  pound-force is 0.45359237 kg times 9.80665 m/s², which is 4.4482216152605
  newtons by definition. `rat` keeps the product exact until the result is
  shown (Chapter 5).
- **Arithmetic is offered, not inherited.** `plus` accepts only another
  `NewtonSeconds`. An object gets no `+` it did not declare, so impulse
  cannot be added to time or to a raw number by accident.

The mistake of 1999, written in Okkama, does not compile:

{{example:04-units-rejected}}

The compiler names the action, the argument it was given and the parameter
it expected. The error is found where the two kinds of number meet, before
anything runs, and the fix is a call that a reader can see.

The cost is a declaration per unit and an action per operation the program
needs. A program that does physics with dozens of units would want derived
units — newton-seconds as newtons times seconds — computed by the type
system, as in F#'s units of measure [@kennedy2010]; Okkama has no such feature, and each
combination is written by hand. For the few quantities that cross a boundary
between programs or teams, the declaration is cheap insurance.

## 4.11 Okkama among other languages

| | Java [@gosling2023] | Go [@gospec] | TypeScript [@tscompat] | Rust [@klabnik2023] | Python with hints [@pep484] | Haskell [@haskell2010] | Okkama |
|---|---|---|---|---|---|---|---|
| Checked | before running | before running | before running; types erased | before running | by a separate checker; nothing at run time | before running | before running |
| Compatibility | nominal | nominal types, structural interfaces | structural | nominal | nominal classes | nominal | nominal |
| Inference | local (`var`, generic calls) | local (`:=`) | local and contextual | inside function bodies | depends on the checker | whole program | inside bodies |
| Top type | `Object` | `any` | `any`, `unknown` | none (`dyn Any` to downcast) | `Any`, `object` | none | `any`, opaque |
| Absence | `null` in every reference type | `nil` for pointers, interfaces, maps, slices | `null`, `undefined` (strict checks optional) | `Option` | `None`, `Optional` | `Maybe` | `Maybe` |
| Implicit numeric conversion | widening | none | one number type | none | `int` to `float` | none | none |

Three rows place Okkama.

- **Compatibility and inference.** Okkama is nominal and infers inside
  bodies from what is written there, with every declaration annotated. Rust
  made the same choice: function signatures are written, bodies are
  inferred. Haskell infers whole programs; Python leaves typing to a tool
  that the language does not run [@pep484].
- **The top type.** Every language with a top type except Okkama has a way
  back from it. A Go `any` yields its value to a type assertion, a
  TypeScript `unknown` to narrowing, and a TypeScript `any` turns checking
  off for everything it touches. An Okkama `any` can be stored and passed
  on, and only the native layer opens it (Section 4.6).
- **Absence.** Java and Go let a missing value hide inside ordinary types;
  TypeScript makes that a compiler option, `strictNullChecks` [@tseveryday].
  Rust, Haskell and Okkama put it in a type of its own (Chapter 7).

## 4.12 Limits

- **No union or intersection types.** A value is of one type; a choice
  between types is expressed by a role or by an enum with cases.
- **No type-level computation.** Generics take types as parameters, but there
  are no type functions, associated types or dependent types (Chapter 17).
- **Annotation of empty collections.** An empty literal needs a declared
  type, because it carries no evidence of what it will hold.
- **No explicit narrowing of `any` yet.** Opaque values can only be passed
  on (Chapter 28).

None of these is an accident. Each would add a kind of entity; the razor asks
for the work each would do, and so far the answer has been "it saves an
annotation" — which is exactly the kind of convenience Chapter 1 promised to
weigh, not to grant.
