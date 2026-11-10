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
- `Least(2.5, -0.5)` is a third specialization, `Least<float>`. Each one is
  compiled separately, so none of them pays for the others.

This chapter covers what can be generic, the one rule that inference follows,
bounds, parameters used as map keys, and why a specialization is a type of its
own. It then shows what the compiler does with generic code and compares the
result with Java, C#, Go, Rust, Haskell and OCaml.

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

The cost is an allocation per call, since `Least(3, 9, 1, 4)` builds an object
before `value()` reads it: one small heap object, cheap next to the work of
most algorithms. The gain is that inference has one home. Every generic
argument in Okkama is inferred at a constructor call, by one procedure, with
one set of error messages.

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
Section 17.10 gets `T = int` from the array's elements.

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
  Section 14.8, `Adding<T, R>` and its siblings, which an object implements per
  operand type.
- **No bounds on role parameters** yet, no defaults, no variance annotations.

## 17.5 Parameters used as map keys

A map key must be a scalar, an enum, a string or a value object (Chapter 8).
When a generic object uses its parameter as a key, as `%T:V` in a field, a
signature or a body, or as the key of a map literal, every specialization has
to respect that:

{{example:17-key-rejected}}

`Tally<string>` is fine. `Tally<@int>` would need arrays as keys, and the
compiler rejects it at the place that asks for it, naming the parameter that
made it a key. The property propagates: in `Wrap<U> { has inner is
Tally<U>; }`, `U` is a key parameter too, because it becomes `Tally`'s `T`. The
check runs after every body is typed, so it sees uses deep inside actions. An
engine that met a non-key under a key parameter anyway, through a path the
analysis missed, would fail at run time rather than compare keys by identity
(#242).

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

## 17.7 How generics run: one copy per specialization

The compiler turns generic code into ordinary code before any engine sees it.
Lowering (Chapter 23) walks the program from `main`. Each time it reaches a
generic declaration with concrete arguments, it requests that specialization:
a new object type with the parameters substituted, and new copies of the
actions that are actually called on it. Here is the OIR of a program that uses
`Box<T>` with two types:

```text
object Box<int> {
    field value: int
}

object Box<string> {
    field value: str
}

func @main() -> int [entry] {
^b0:
    %0: int = const 7
    %1: Box<int> = call @Box<int>.new#0(%0)
    %2: str = const "seven"
    %3: Box<string> = call @Box<string>.new#0(%2)
    %4: int = call @Box<int>.get(%1)
    %5: str = call @Box<string>.get(%3)
    %6: int = str.len %5
    %7: int = int.add %4, %6
    ret %7
}

func @Box<int>.get(%0: Box<int>) -> int [method Box<int>] {
^b0:
    %1: int = obj.get %0, value
    ret %1
}

func @Box<string>.get(%0: Box<string>) -> str [method Box<string>] {
^b0:
    %1: str = obj.get %0, value
    ret %1
}
```

No `T` survives. `Box<int>.get` is exactly the function that a hand-written
`IntBox.get` would be. The IR engine, Sparrow and the JVM back end
receive only concrete objects and concrete functions. On the JVM each
specialization becomes its own class (`box$Box$int$.class` and
`box$Box$string$.class` in the jar) whose field has its exact type, a
`BigInteger` in one and a `String` in the other, so nothing is cast when it is
read. This strategy is called *monomorphization*.

### What it costs

The price of monomorphization is code. Every specialization is a copy, and a
generic object pulls in the specializations of everything it uses. The
standard library's `Sorted<T: Comparing<T>>` shows the scale: a program that
sorts one array and reads its first element, then the same program with arrays
of more element types:

| Element types sorted | OIR functions | OIR objects | OIR lines |
|---|---:|---:|---:|
| `int` | 46 | 12 | 738 |
| `int`, `string` | 69 | 17 | 1063 |
| `int`, `string`, `float` | 92 | 22 | 1389 |
| `int`, `string`, `float`, `byte` | 115 | 27 | 1714 |

Each new element type adds 23 functions and 5 objects: `Sorted` itself, its
cursor, the bulk ordering that sorts it, and the adapter and cursor through
which a plain array becomes an `Iterating<T>`. The
growth is linear and bounded by what the program uses, because lowering
requests only reachable code. In practice the numbers are small. The
benchmark application `10-app` has one generic specialization, `Atomic<…>`,
among 22 objects and 85 functions, and the INI benchmark has 14 among 61
objects and 206 functions.

Monomorphization has one hard limit. A generic action that constructs a
*larger* specialization of its own object would need infinitely many copies:

{{example:17-grow-rejected}}

`Grow<int>.deeper` asks for `Grow<Box<int>>.deeper`, which asks for
`Grow<Box<Box<int>>>.deeper`, and so on. The depth of this *polymorphic
recursion* depends on a run-time value, so no finite set of copies covers it.
Lowering stops at a chain of 256 requests, or at 2^18 specializations in
total, and names the chain (H-15). Languages that compile generics to one
shared copy accept such programs; Okkama, like Rust and C++, rejects them.
The check happens in lowering, and since 0.4.2 `okkama check` lowers the
program too, so it reports this error like any other (#317).

### The alternatives

Monomorphization is one of four strategies in use.

**Erasure.** Java compiles `List<Integer>` and `List<String>` to one class
that holds `Object`s, and inserts casts where values come out. The design
came from GJ, whose stated aim was to add generics without changing the
virtual machine or breaking existing class files [@bracha1998]. The cost
shows in three places. Primitives must be boxed, since there is no
`List<int>`. A cast can fail at run time when unchecked conversions were
allowed at compile time. And `T` does not exist at run time, so `new T()` and
`instanceof List<String>` are impossible. Java's wildcards and inference later
grew complex enough that subtyping between generic types became undecidable
[@grigore2017], and that the combination of wildcards and `null` made the type
system unsound [@amin2016].

**Reification with run-time specialization.** C# keeps type arguments at run
time. The .NET runtime shares one compiled copy of a generic method among all
reference types, and the JIT compiler produces a specialized copy for each
value type, so a `List<int>` stores unboxed `int`s [@kennedy2001]. This is
monomorphization for the cases where data layout differs and sharing for the
rest, done lazily by the JIT.

**Dictionaries.** Haskell compiles a function with a class constraint, such
as `Ord a => [a] -> a`, to a function that takes an extra argument, a
dictionary of the `Ord` operations for the actual type [@wadlerblott1989]. One
copy of the code serves every type, at the price of an indirect call for each
operation and a uniform boxed representation of values. Go 1.18 mixes the two
approaches. Types with the same *GC shape* (roughly, the same layout for the
garbage collector) share one compiled copy, which also receives a dictionary
for what differs between them [@scales2022]. The formal model behind Go's
generics, Featherweight Go, gives a translation of generic programs into a
non-generic core by monomorphization [@griesemer2020].

**Monomorphization.** C++ templates make a copy per instantiation
[@stroustrup1994], and so does Rust for generic functions and types
[@klabnik2023]. Generic code then runs at the speed of hand-written code, and
the cost is paid in compile time and binary size.

Okkama chose monomorphization because its compiler sees the whole program. A
package is compiled from source together with its dependencies (Chapter 19),
so there is no separately compiled generic library that would have to work for
types it has never seen. Every engine then gets the simplest possible input.
The IR engine needs no dictionaries, the Sparrow JIT compiles each copy with
its exact register kinds (an `int` is a tagged word, a `float` the raw bits
of a double), and the JVM back end needs no casts.

## 17.8 A parameter or a role?

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

Monomorphization does not make the parameter version faster at the call. Inside
`Total<Square>`, `item.area()` is a call through the bound, and lowering
emits it as one:

```text
%10: Square = array.get %7, %9
%11: Shape = role.wrap %10
%12: int = call.role %11, Shape.area()
```

`role.wrap` costs nothing on Sparrow and the JVM, and `call.role` is a
lookup in a table per role slot followed by an indirect call (Section 14.7).
Measured on a loop of ten million `area()` calls (best of five on an Apple M2
Max, including compilation), `Total<Square>` and a `Total<Shape>` over a mix
of squares and strips take the same time: 0.375 s and 0.378 s on Sparrow,
and 1.21 s and 1.18 s for a million calls on the IR engine. What a
specialization changes is the data and what flows out: a `Total<Square>`
tells its caller that it holds squares, and a `Box<float>` keeps its field as
the raw bits of a double, where a field of type `any` would hold a box.

## 17.9 A worked example: the best N

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
  `Top<Bid>`. Each is compiled separately, so the comparison inside
  `offered` is the builtin integer order in the first, the code-point order of
  strings in the second, and `Bid.order` in the third.

A role field could not replace the parameter here. `Comparing<T>` takes the
type that an item is compared with, so a field `has items is @Comparing<…>`
would have to name that type and the object would work for one type only. The
parameter is what lets one declaration say "items that can be compared with
each other".

## 17.10 Generics in the standard library

The standard library is the largest user of generics: 195 generic object
declarations and 17 generic roles. Most of them fall into four groups.

- **Collection views and cursors.** `Taken<T>`, `Sliced<T>`, `Zipped<A, B>`,
  `Sorted<T: Comparing<T>>`, `SortedBy<T, K: Comparing<K>>` and their cursors
  implement `Iterating<T>` and `Sequencing<T>`, so they chain with each other
  and with plain arrays.
- **Containers with behaviour.** `Cell<T>` (Chapter 13), `Atomic<T>` and
  `Channel<T>` (Chapter 20).
- **Contracts.** `Comparing<T>`, the operator roles `Adding<T, R>` and its
  siblings, `Iterating<T>`, `Cursor<T>`, `List<T>`, `Set<T>`.
- **Settlements and cleanups** of asynchronous natives
  (`HttpProxySettlement<T>`), where the type parameter carries the result of
  a native call through the scheduler.

{{example:17-library}}

The example uses two of them. `Sorted` is specialized twice, for `int` and for
`string`, and each specialization gets its `T` from the array it receives.
`SortedBy` gets `T = Job` from the array and `K = int` from the key's declared
result. Every element type `T` has to implement `Comparing<T>`, and every key
type `K` has to implement `Comparing<K>`, and the compiler checks both at the
call.

## 17.11 Okkama among other languages

| | What can be generic | How it runs | Bounds | Variance | Type arguments |
|---|---|---|---|---|---|
| Java | classes, interfaces, methods | erasure, casts, boxed primitives | `extends`, several with `&` | at the use site, wildcards | inferred for methods, `<>` |
| C# | classes, interfaces, methods, delegates | reified; shared for references, JIT-specialized for value types | `where`, several | declared on interfaces and delegates | inferred for methods |
| Go | functions, types | GC-shape stenciling with dictionaries | interfaces with type sets | none | inferred for functions |
| Rust | functions, types, traits | monomorphization | trait bounds, several | inferred; matters for lifetimes | inferred |
| Haskell | functions, types, classes | dictionary passing, uniform values | class constraints, several | none (no subtyping) | inferred (Hindley–Milner) |
| OCaml | functions, types, modules (functors) | uniform values, one copy | through modules and signatures | declared (`+'a`, `-'a`) | inferred (Hindley–Milner) |
| Okkama | objects, roles, enums | whole-program monomorphization | one role, F-bounded allowed | none | inferred at construction, exact |

Okkama sits at the conservative end on almost every column. It infers less
than Haskell or OCaml, which find types for whole functions [@milner1978]. It
has fewer kinds of bound than C# or Rust, and no variance at all. What it gains
is predictability. Every generic argument is decided at one kind of place, a
constructor call, from the values in front of it. A reader can do the same
inference by looking at the call, and an error message names the two types
that disagree.

## 17.12 Limits

- **No generic actions.** An algorithm generic in its inputs is written as a
  generic object (Section 17.1).
- **No variance.** `Box<Square>` is not a `Box<Shape>`. Declare the target
  type at construction instead.
- **One bound, on objects only.** No multiple bounds, no bounds on role
  parameters, no category bounds such as `T: number`, no default arguments,
  no higher-kinded parameters.
- **No call-site specialization.** `Box<int>(…)` is not valid source; the
  specialization comes from the arguments or from a declared target.
- **Code size grows with specializations.** Each element type of `Sorted`
  adds 23 functions (Section 17.7).
- **Polymorphic recursion is rejected,** at compile time on every engine.
- **Fake diagnostics** name the generated `Role$fake` object instead of the
  `fake` block the programmer wrote (#276).
- **Role plainness is per declaration.** One implementation of `Cursor` that
  holds a resource makes the compiler track every `Cursor<T>` in regions
  (Chapter 11), not only the instances that can reach one (#268).

## 17.13 What this design costs

Exact identity makes some programs longer. Mixed numbers need explicit
conversions, empty collections need a declared type, and a function over
`T` becomes an object with a constructor. Whole-program monomorphization makes
compilation do more work and produces more code for programs that use one
generic object with many types. It also rules out polymorphic recursion, which
erased or dictionary-based implementations allow.

What the design buys is that generic code is not a separate world. After
inference, a `Least<int>` is checked, lowered and executed as a plain object
over `int`: no casts, no boxes, no dictionaries, and the same speed on every
engine as the code a programmer would have written by hand for that one type.
Inference has one rule that a reader can apply in their head, and when it
fails the message names the two types that disagree.
