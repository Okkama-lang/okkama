# Chapter 17 — archived implementation passages

Replaced mixed paragraphs are preserved in full for context.

## 17.5 Parameters used as map keys

described 0.5.x

`Tally<string>` is fine. `Tally<@int>` would need arrays as keys, and the
compiler rejects it at the place that asks for it, naming the parameter that
made it a key. The property propagates: in `Wrap<U> { has inner is
Tally<U>; }`, `U` is a key parameter too, because it becomes `Tally`'s `T`. The
check runs after every body is typed, so it sees uses deep inside actions. An
engine that met a non-key under a key parameter anyway, through a path the
analysis missed, would fail at run time rather than compare keys by identity
(#242).

## 17.7 How generics run: one copy per specialization

described 0.5.x

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

described 0.5.x

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

## 17.11 Okkama among other languages

described 0.5.x

| | What can be generic | How it runs | Bounds | Variance | Type arguments |
|---|---|---|---|---|---|
| Java | classes, interfaces, methods | erasure, casts, boxed primitives | `extends`, several with `&` | at the use site, wildcards | inferred for methods, `<>` |
| C# | classes, interfaces, methods, delegates | reified; shared for references, JIT-specialized for value types | `where`, several | declared on interfaces and delegates | inferred for methods |
| Go | functions, types | GC-shape stenciling with dictionaries | interfaces with type sets | none | inferred for functions |
| Rust | functions, types, traits | monomorphization | trait bounds, several | inferred; matters for lifetimes | inferred |
| Haskell | functions, types, classes | dictionary passing, uniform values | class constraints, several | none (no subtyping) | inferred (Hindley–Milner) |
| OCaml | functions, types, modules (functors) | uniform values, one copy | through modules and signatures | declared (`+'a`, `-'a`) | inferred (Hindley–Milner) |
| Okkama | objects, roles, enums | whole-program monomorphization | one role, F-bounded allowed | none | inferred at construction, exact |

## 17.12 Limits

described 0.5.x

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
- **Role plainness is per declaration.** One implementation of `Cursor` that
  holds a resource makes the compiler track every `Cursor<T>` in regions
  (Chapter 11), not only the instances that can reach one (#268).

## 17.13 What this design costs

described 0.5.x

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


## Chapter 17. Generics

described 0.5.x

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

## Chapter 17. Generics

described 0.5.x

This chapter covers what can be generic, the one rule that inference follows,
bounds, parameters used as map keys, and why a specialization is a type of its
own. It then shows what the compiler does with generic code and compares the
result with Java, C#, Go, Rust, Haskell and OCaml.

## 17.1 Parameters on objects, roles and enums

described 0.5.x

The cost is an allocation per call, since `Least(3, 9, 1, 4)` builds an object
before `value()` reads it: one small heap object, cheap next to the work of
most algorithms. The gain is that inference has one home. Every generic
argument in Okkama is inferred at a constructor call, by one procedure, with
one set of error messages.

## 17.9 A worked example: the best N

described 0.5.x

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

## 17.3 Inference at construction

described 0.5.x

`Same()` has no arguments, so on its own it could not be specialized. In an
array declared `@Echo<string>` each element is expected to be an
`Echo<string>`, and `Same<T> does Echo<T>` matches that only with
`T = string`. A builtin value can be evidence for a role parameter too:
arrays are `Iterating<T>` and `Sequencing<T>`, so `Sorted([5, 3, 8])` in
Section 17.10 gets `T = int` from the array's elements.

## 17.4 Bounds

described 0.5.x

- **One bound per parameter.** There is no `T: Comparing<T> & Hashing`. A
  parameter that needs two capabilities is better served by a role that
  composes them.
- **No category bounds.** There is no `T: number`. Numbers in Okkama do not
  share a supertype, and arithmetic between categories needs explicit
  conversion. Generic arithmetic is written with the operator roles of
  Section 14.8, `Adding<T, R>` and its siblings, which an object implements per
  operand type.
- **No bounds on role parameters** yet, no defaults, no variance annotations.

## 17.10 Generics in the standard library

described 0.5.x

## 17.10 Generics in the standard library
