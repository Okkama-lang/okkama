# Chapter 17. Generics

A box that holds an `int` and a box that holds a `string` do the same work. So
do a sorted list of prices and a sorted list of names, a channel of jobs and a
channel of results. Without type parameters a language makes you pick one of
three bad options: write the same object once per type, give up the type and
store `any`, or check every element when it comes back out. Generics are the
fourth option. You write the object once, over a type `T`, and the compiler
makes sure each use of it has one concrete `T`.

{{example:17-generics}}

- `Pair<K, V>` has two type parameters. `swapped()` returns a `Pair<V, K>`, a
  different specialization of the same declaration.
- `Least<T: Comparing<T>>` is an algorithm written as an object. Its `T` must
  be able to order itself, and the compiler checks that before the
  constructor runs.
- No call names a type. `Pair(key: "answer", value: 42)` is a
  `Pair<string, int>` and `Least(3, 9, 1, 4)` is a `Least<int>`: the compiler
  works out the arguments from the values.
- `Least(2.5, -0.5)` is a third specialization, `Least<float>`. The type arguments distinguish these types.

This chapter covers what can be generic, inference, bounds, map-key parameters and specialization identity, then compares these language rules with Java, C#, Go, Rust, Haskell and OCaml.

## 17.1 Parameters on objects, roles and enums

Three kinds of declaration take type parameters: objects, roles and enums.

```okkama
object Box<T> {
    has value is T;
    Box(:value);
    get() is T { value; }
}

role Echo<T> {
    echo(value is T) is T;
}

enum Slot<T> = (vacant, full(item is T));
```

The parameters follow the declaration's name in angle brackets. They are type
names, so they are capitalised, and they must be unique and differ from the
declaration's own name. A parameter is visible everywhere inside its
declaration: in fields, constructors (full and shorthand), actions, successor
actions and `destroy` of an object, in the signatures and the `fake` block of a
role, in the payloads of an enum. Outside it the parameter does not exist; a
caller sees `Box<int>`, never `T`.

A use of a generic type gives exactly as many arguments as the declaration has
parameters, and a type without parameters takes none:

```text
Generic type "Pair" expects 2 type argument(s), got 1
Type "Coin" is not generic and cannot accept type arguments
```

A parameter stands for one concrete type. It cannot take arguments of its own,
so `T<int>` is not a type; Okkama has no higher-kinded parameters.

The enums `Maybe<T>` and `Presence<T>` of Chapter 7 are generic enums built into
the compiler. A program declares its own the same way, as `Slot<T>` above. A
case without a payload does not mention `T`, so where nothing else fixes the
type it is written with its specialization: `Slot<int>.vacant`, like
`Maybe<Shape>.none`.

### No generic actions

The list leaves something out: an action cannot take type parameters of its
own. There is no `first<T>(items is @T) is T`. This follows from the object
model of Chapter 3. Okkama has no free functions and no static methods, so a
generic function would need an object to live on, and that object would need
its own `T` anyway. The language goes the other way and makes the algorithm
the object. `Least<T>` in the example is what Java would write as
`static <T extends Comparable<T>> T least(T first, T... rest)`. It is a value
that knows its inputs and computes its answer on request, the style that
*Elegant Objects* recommends in place of static methods and utility classes
[@bugayenko2016].

`Least(3, 9, 1, 4)` constructs an object before `value()` reads it, so the algorithm pays for constructing that value. Inference has one home: the constructor call, with one set of error messages.

### Where the idea comes from

Cardelli and Wegner's survey of polymorphism separates two kinds [@cardelli1985].
*Inclusion* polymorphism is what roles give: one value is usable as several
types, because it belongs to all of them. *Parametric* polymorphism is what
this chapter is about: one piece of code works uniformly over a type that is
supplied as a parameter. Both came into programming languages in the 1970s.
CLU let a cluster, its abstract data type, take type parameters, and checked
at compile time that the arguments offered the operations the cluster needed
[@liskov1977]. ML inferred polymorphic types for whole functions without any
annotations [@milner1978]. Object-oriented languages took parameters much
later and with more compromises. Java added generics in version 5, in 2004,
on top of a virtual machine that knew nothing about them [@bracha1998], and C#
added them in version 2.0, in 2005, together with a runtime that did
[@kennedy2001]. Go, which had shipped without generics for a decade, added
them in version 1.18 in 2022 [@griesemer2020; @scales2022].

Okkama's generics are parametric polymorphism in CLU's sense, restricted to
objects, roles and enums, with ML's idea that a programmer should rarely have
to write type arguments, but without ML's global inference.

## 17.2 One `T`, one type

The whole model rests on one rule: **within one specialization, `T` is one
exact type.** Every occurrence of `T` must resolve to the same type, and
nothing is widened to make them agree.

{{example:17-exact-rejected}}

`1` is an `int` and `2.0` is a `float`. In Okkama these are different
categories that never convert implicitly (Chapter 5), so `Least(1, 2.0)` has
no `T`. The compiler does not look for a common supertype, does not invent a
union `int | float`, and does not fall back to `any`. The program fails before
anything runs, and the fix is an explicit conversion: `Least(1.float(), 2.0)`.
Version 0.4.2 applies the same rule to collection literals: `[1, 2.0]` is
rejected unless an explicit `@any` is expected (Chapter 8).

Other languages answer the same question differently. Java's inference
computes a least upper bound. For `Arrays.asList(1, 2.0)` it finds an
intersection type of the form `Number & Comparable<…>`, a type that no
programmer wrote and whose values can barely be used: `Number` has no
arithmetic, so the list cannot be summed without casts [@gosling2023]. Kotlin
infers a similar intersection type. TypeScript infers the union
`(string | number)[]` for `[1, "two"]`. Each answer
compiles, and each pushes the problem to the place where the values are
used.

Okkama keeps the problem where it was created. A type argument is evidence of
what the values are, and an inferred `T` that no value has would be a guess.
The rule also keeps every other part of the type system simple: once inference
succeeds, `T` is an ordinary concrete type, and the body of `Least<int>`
type-checks exactly as if it had been written for `int`.

## 17.3 Inference at construction

`Box(value: 7)` names a declaration, not a type. The specialization is worked
out at the call, from two sources.

**The arguments.** Each constructor parameter whose type mentions a parameter
gives one *equation*: `value is T` with the argument `7` gives `T = int`. The
equations go into structure: a parameter `items is @T` with an `@string`
argument gives `T = string`, a parameter `%K:V` with a `%string:int` gives two
equations, a parameter `Box<T>` with a `Box<float>` argument gives `T = float`,
and a variadic tail `rest is ...T` gives one equation per value. A shorthand
constructor `Box(:value)` uses the field's type in the same way. If two
equations disagree, the error names both answers:

```text
Generic parameter "T" for object "Least" cannot be both int and float
```

**The expected type.** When the call stands where a type is already declared,
in a typed binding, a field, an argument for a typed parameter, a result or
`return`, that type is evidence too. This is what makes an empty collection
usable:

{{example:17-evidence-rejected}}

`[]` has no elements, so it says nothing about `T`, and without other evidence
the compiler stops. A declared target fixes it:
`let empty is Bucket<int> = Bucket(values: []);`. The declared type supplies
evidence that is *missing*. It never overrides evidence that is present: in
`let wrong is Box<int> = Box(value: "x");` the argument and the target
disagree, and that is an error, not a conversion.

The expected type reaches into nested constructions, but only through
parameters it already fixes:

```okkama
let x is Box<Empty<int>> = Box(item: Empty());
```

The target gives the outer `T = Empty<int>`. The inner `Empty()` stands where an
`Empty<int>` is expected, so it gets its `int` from there. The compiler does
not look ahead to later statements, does not try overloads to see which one
type-checks, and does not search for a type that would make a program compile.
Inference uses only what is visible at the call.

### Roles as evidence

Roles take part in both directions. A role-typed target supplies the arguments
of an object that implements the role:

{{example:17-identity}}

`Same()` has no arguments, so on its own it could not be specialized. In an
array declared `@Echo<string>` each element is expected to be an
`Echo<string>`, and `Same<T> does Echo<T>` matches that only with
`T = string`. A builtin value can be evidence for a role parameter too:
arrays are `Iterating<T>` and `Sequencing<T>`, so `Sorted([5, 3, 8])` in
Section 17.9 gets `T = int` from the array's elements.

When one equation gives a role and another gives an object that implements
it, the role stays. A `List<Json>` that has a `JsonBool` prepended is still a
`List<Json>`, not a `List<JsonBool>`. Inference never looks for a common role
of two unrelated objects, however: if one argument is a `Square` and another a
`Circle`, the program must say `Shape` somewhere.

### Several constructors

An object may have several constructors (Chapter 12), and inference runs for
each one that fits the call's shape. Fixed and shorthand constructors are
tried before variadic ones, as in overload resolution. If two constructors fit
and infer different specializations, the call is ambiguous and the compiler
says so (`Ambiguous construction of "Pair": constructors infer different
generic arguments`) instead of picking one.

### Functions as evidence

An anonymous action may declare its result type, and that result is evidence
for a generic parameter. This is how the standard library's `SortedBy<T, K>`
learns its key type:

```okkama
let quickest = SortedBy(origin: jobs, key: (job is Job) is int { job.hours(); });
```

The key's type `(Job) is int` meets the parameter `key is (T) is K`, giving
`T = Job` and `K = int`. Before 0.4.1 an anonymous action could not declare its
result, and the same call needed `let quickest is SortedBy<Job, int> = …`
(#214).

### No explicit specialization

There is no syntax to specialize at the call: `Box<int>(value: 7)` is an error
(`A type is not a value`). The way to state a specialization is to declare the
type of the place that receives the value. That keeps one way of doing it, and
the type ends up on the binding, where a reader looks for it.

## 17.4 Bounds

A parameter may be required to implement a role:

```okkama
role Comparing<T> {
    order(other is T) is Order;
}

object Least<T: Comparing<T>> { … }
```

`Comparing` is the standard library's three-way order (`std/compare`).
`T: Comparing<T>` says that a `T` can be ordered against another `T`. The bound
mentions the parameter it constrains. This is *F-bounded* polymorphism, named
by Canning, Cook, Hill, Olthoff and Mitchell, who showed that ordinary bounded
quantification cannot express "a type comparable with itself" and that a
recursive bound can [@canning1989]. Java's `<T extends Comparable<T>>` is the
same idea, and its roots go back to CLU, one of the first languages built
around parameterized abstract types, where a `where` clause required a type
argument to provide the operations a module needed [@liskov1977].

`int`, `float`, `rat`, `byte`, `string` and `char` implement `Comparing` of
themselves, so `Least` accepts all of them. `bool` deliberately has no
natural order:

{{example:17-bound-rejected}}

The bound is checked after inference has found `T` and before the
constructor's body runs. It never takes part in inference: `T: Comparing<T>`
does not tell the compiler what `T` is, and an object whose `T` cannot be
inferred is not rescued by its bound. Nor does a bound convert anything. A
`T` either implements the role or the program is rejected.

A role-typed argument satisfies a bound when the role *narrows* to it, that
is, when it offers every action of the bound (Chapter 14). Given
`object Tagged<T: Named>`, a value of a role `Counted` that has `name()` and
`count()` satisfies `T: Named`. The specialization is then `Tagged<Counted>`,
so `get()` hands back a `Counted` with `count()` still available, not a
`Named`.

What bounds cannot do is as deliberate as what they can:

- **One bound per parameter.** There is no `T: Comparing<T> & Hashing`. A
  parameter that needs two capabilities is better served by a role that
  composes them.
- **No category bounds.** There is no `T: number`. Numbers in Okkama do not
  share a supertype, and arithmetic between categories needs explicit
  conversion. Generic arithmetic is written with the operator roles of
  Section 14.7, `Adding<T, R>` and its siblings, which an object implements per
  operand type.
- **No bounds on role parameters** yet, no defaults, no variance annotations.

## 17.5 Parameters used as map keys

A map key must be a scalar, an enum without payloads, a string or a value
object (Chapter 8).
When a generic object uses its parameter as a key, as `%T:V` in a field, a
signature or a body, or as the key of a map literal, every specialization has
to respect that:

{{example:17-key-rejected}}

`Tally<string>` is valid; `Tally<@int>` is rejected because arrays cannot be keys. In `Wrap<U> { has inner is Tally<U>; }`, `U` must satisfy the same key rule. Violating the key rule is never a request to compare keys by identity (#242).

## 17.6 A specialization is a type

`Box<int>` and `Box<float>` are different types, as different as `Point` and
`Money`. A `Box<float>` cannot go where a `Box<int>` is expected, and that
holds for fields, parameters, results, variadic tails, successors, typed
bindings, `catch` clauses and imports. Role conformance is substituted too:
`Same<T> does Echo<T>` makes `Same<int>` an `Echo<int>` and nothing else, so
assigning it to an `Echo<string>` is rejected
(`Generic parameter "T" for object "Same" cannot be both string and int`).

There is also no variance. `Square` implements `Shape`, but a `Box<Square>` is
not a `Box<Shape>`:

```text
Cannot initialize variable "shapes" of type Box<Shape> with Box<Square>
```

That is the safe answer for a type whose `T` appears in both directions, and
Okkama does not ask the programmer to say which direction a parameter is used
in. The common case still works because inference reads the target first:
`let direct is Box<Shape> = Box(value: Square(side: 3));` builds a
`Box<Shape>` from the start, wrapping the square as a `Shape`.

### Generic fakes

A role's `fake` block (Chapter 15) is written against the role's parameters,
and it must conform *symbolically*. A fake whose `echo` takes `int` does not
implement `echo(value is T)`, even though `Echo<int>` is the only
specialization the tests use. A fake has no constructor arguments to infer
from, so its specialization comes from the target:

```okkama
let echo is Echo<int> = Echo.fake();   // test mode only
```

Without a declared target the construction fails instead of guessing.

### Reading the errors

Every generic error comes from one of a few checks, and its message says which:

| Message | What happened | Usual fix |
|---|---|---|
| `Generic parameter "T" for object "X" cannot be both A and B` | two equations disagree, or a target contradicts an argument | convert one value explicitly, or correct the declared type |
| `Cannot infer generic parameter "T" for object "X"` | no argument gives evidence (empty collections, no arguments) and no target is declared | declare the type of the binding, field or parameter |
| `… has type A, which does not satisfy R<A>` | the inferred type does not implement the bound | implement the role, or wrap the value in an object that does |
| `Generic parameter "T" for object "X" is a map key: …` | a key parameter got a type that cannot be a key | use a scalar, an enum, a string or a value object |
| `Ambiguous construction of "X": constructors infer different generic arguments` | two constructors fit with different specializations | declare the target type |
| `Generic type "X" expects N type argument(s), got M` | a written type has the wrong arity | write every argument |
| `A type is not a value` | an attempt to specialize at the call, `Box<int>(…)` | move the type to the binding |
| `The program requests a chain of more than 256 generic specializations …` | polymorphic recursion | stop growing the type in the recursive call |

## 17.7 A parameter or a role?

Roles (Chapter 14) and type parameters both let one piece of code work with
many types, and a design often has to choose between them. Compare a total of
areas written both ways:

```okkama
// A role: one Totals object for any mix of shapes.
object Totals {
    has shapes is @Shape;
    Totals(:shapes);
    area() is int { shapes.reduce(0) -> sum, shape { sum + shape.area(); }; }
}

// A parameter: one specialization per element type.
object Total<T: Shape> {
    has items is @T;
    Total(:items);
    area() is int { items.reduce(0) -> sum, item { sum + item.area(); }; }
}
```

The role version accepts `[Square(side: 2), Strip(width: 3)]`. The parameter
version accepts that array only as `Total<Shape>`, and otherwise needs every
element to have the same type: `Total(items: [Square(side: 2)])` is a
`Total<Square>`. The difference shows when the object gives values back. A
`Total<Square>` that offers `largest() is T` returns a `Square`, with all of
its actions, while `Totals` can only return a `Shape`. A type parameter keeps
the information about *which* type it holds, and a role deliberately forgets
it.

So the rule of thumb is about what flows out. If the code only consumes
values through the role's actions, a role-typed field is simpler and admits
mixtures. If it hands values back, or builds new containers of them, or uses
them as map keys, a parameter preserves their exact type for the caller.

## 17.8 A worked example: the best N

A leaderboard, a list of the most expensive orders, the three slowest
requests: keeping the best `N` items of a stream is the same algorithm for
any type that can be ordered. Here it is once, for every such type:

{{example:17-top}}

The pieces of the chapter meet here.

- **The bound.** `Top<T: Comparing<T>>` needs `order()` and nothing else, so it
  works for `int`, `string` and any domain object that implements
  `Comparing` of itself. `Bid` does, with its own policy: a higher amount wins
  and equal amounts go by name.
- **The target supplies `T`.** An empty top has no items, so
  `Top(size: 3, items: [])` alone could not be specialized. Each start value
  is declared, `let scores is Top<int> = …`, and from there `reduce` carries
  the type: its accumulator is a `Top<int>`, and so is each `offered(…)`.
- **Immutability.** `offered` returns a new `Top`, so `scores` still holds the
  empty top after the loop. The successor-style value of Chapter 13 needs no
  special form here: the result type is the same specialization, `Top<T>`.
- **Three specializations.** The program uses `Top<int>`, `Top<string>` and
  `Top<Bid>`. The comparison inside `offered` follows its element type: the builtin integer order in the first, the code-point order of
  strings in the second, and `Bid.order` in the third.

A role field could not replace the parameter here. `Comparing<T>` takes the
type that an item is compared with, so a field `has items is @Comparing<…>`
would have to name that type and the object would work for one type only. The
parameter is what lets one declaration say "items that can be compared with
each other".

## 17.9 Generics in the standard library

The standard library is the largest user of generics: 185 generic object
declarations and 18 generic roles. Most of them fall into four groups.

- **Collection views and cursors.** `Taken<T>`, `Sliced<T>`, `Zipped<A, B>`,
  `Sorted<T: Comparing<T>>`, `SortedBy<T, K: Comparing<K>>` and their cursors
  implement `Iterating<T>` and `Sequencing<T>`, so they chain with each other
  and with plain arrays.
- **Containers with behaviour.** `Cell<T>` (Chapter 13), `Atomic<T>` and
  `Channel<T>` (Chapter 20).
- **Contracts.** `Comparing<T>`, the operator roles `Adding<T, R>` and its
  siblings, `Iterating<T>`, `Cursor<T>`, `List<T>`, `Set<T>`.
- **Settlements** of asynchronous steps (`FormSettlement<T>`,
  `RobotsSettlement<T>`), which hold the outcome of a step whatever its type.

{{example:17-library}}

The example uses two of them. `Sorted` is specialized twice, for `int` and for
`string`, and each specialization gets its `T` from the array it receives.
`SortedBy` gets `T = Job` from the array and `K = int` from the key's declared
result. Every element type `T` has to implement `Comparing<T>`, and every key
type `K` has to implement `Comparing<K>`, and the compiler checks both at the
call.

## 17.10 Okkama among other languages

| | What can be generic | Bounds | Variance | Type arguments |
|---|---|---|---|---|
| Java | classes, interfaces, methods | `extends`, several with `&` | at the use site, wildcards | inferred for methods, `<>` |
| C# | classes, interfaces, methods, delegates | `where`, several | declared on interfaces and delegates | inferred for methods |
| Go | functions, types | interfaces with type sets | none | inferred for functions |
| Rust | functions, types, traits | trait bounds, several | inferred; matters for lifetimes | inferred |
| Haskell | functions, types, classes | class constraints, several | none (no subtyping) | inferred (Hindley–Milner) |
| OCaml | functions, types, modules (functors) | through modules and signatures | declared (`+'a`, `-'a`) | inferred (Hindley–Milner) |
| Okkama | objects, roles, enums | one role, F-bounded allowed | none | inferred at construction, exact |

Okkama sits at the conservative end on almost every column. It infers less
than Haskell or OCaml, which find types for whole functions [@milner1978]. It
has fewer kinds of bound than C# or Rust, and no variance at all. What it gains
is predictability. Every generic argument is decided at one kind of place, a
constructor call, from the values in front of it. A reader can do the same
inference by looking at the call, and an error message names the two types
that disagree.

## 17.11 Limits

- **No generic actions.** An algorithm generic in its inputs is written as a
  generic object (Section 17.1).
- **No variance.** `Box<Square>` is not a `Box<Shape>`. Declare the target
  type at construction instead.
- **One bound, on objects only.** No multiple bounds, no bounds on role
  parameters, no category bounds such as `T: number`, no default arguments,
  no higher-kinded parameters.
- **No call-site specialization.** `Box<int>(…)` is not valid source; the
  specialization comes from the arguments or from a declared target.
- **Polymorphic recursion requiring unbounded specializations is rejected.**

{{example:17-grow-rejected}}

## 17.12 What this design costs

Exact identity makes some programs longer. Mixed numbers need explicit conversions, empty collections need a declared type, and an algorithm over `T` becomes an object with a constructor. Polymorphic recursion that requires an unbounded succession of distinct specializations is rejected.

Generic code follows the ordinary object rules: `Least<int>` works with `int`, and a parameter preserves its exact type. Inference has one rule a reader can apply, and a failed inference names the types that disagree.
