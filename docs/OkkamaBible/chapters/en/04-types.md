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
- **To make the runtime fast.** The Sparrow core chooses a machine
  representation for every register from its type — a raw 64-bit integer, a
  double, a pointer the garbage collector traces — which is only safe because
  the types are checked (Chapter 25).

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

Inference has known rough edges. Anonymous actions cannot declare their
result type, so some generic calls need an annotation on the binding that
receives them (issue #214), and empty literals need a declared type because
they contain no evidence.

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

## 4.7 What the compiler checks, and what remains for run time

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

Some checks can only happen while the program runs, because they depend on
values: an index within an array's bounds, a key present in a map, a
conversion such as `byte(256)` in range, a division by a non-zero number, a
float result that stays finite. These are *runtime failures* (Chapter 10):
precise, located errors that stop the program — or, in the case of typed
errors, that a caller may catch.

The boundary between the two lists has moved over time. The first Okkama
implementation, an interpreter in Perl, checked most types while running;
the Rust compiler checks most of them statically, and the remaining dynamic
checks are concentrated where values, not types, decide.

## 4.8 Limits

- **No union or intersection types.** A value is of one type; a choice
  between types is expressed by a role or by an enum with cases.
- **No type-level computation.** Generics take types as parameters, but there
  are no type functions, associated types or dependent types (Chapter 17).
- **Annotation of empty and anonymous things.** Empty collections and some
  anonymous actions need declared types (issue #214).
- **No explicit narrowing of `any` yet.** Opaque values can only be passed
  on (Chapter 32).

None of these is an accident. Each would add a kind of entity; the razor asks
for the work each would do, and so far the answer has been "it saves an
annotation" — which is exactly the kind of convenience Chapter 1 promised to
weigh, not to grant.
