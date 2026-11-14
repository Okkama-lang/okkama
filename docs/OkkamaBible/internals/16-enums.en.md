# Chapter 16 — archived implementation passages

Replaced mixed paragraphs are preserved in full for context.

## 16.6 How enums run

described 0.5.x

## 16.6 How enums run

The two kinds of enum compile differently, and the difference shows in what
they cost.

A **payload-free enum** is a value of its own category in OIR. A case is
`enum.case`, and comparison is `enum.eq`:

```text
func @Probe.code(%0: Probe, %1: Status) -> int [method Probe] {
^b0:
    %2: Status = enum.case Status.ready
    %3: bool = enum.eq %1, %2
    cond %3, ^b1, ^b2
^b1:
    %4: int = const 1
    br ^b3(%4)
^b2:
    %5: int = const 2
    br ^b3(%5)
^b3(%6: int):
    ret %6
}
```

The source was `match s { case Status.ready { 1; } case Status.failed,
Status.done { 2; } }`. The last case is not tested at all: the match is
exhaustive, so whatever is not `ready` must be `failed` or `done`. On
Sparrow an enum value is an immediate 64-bit word, with the enum's number
in the high half and the case's in the low half. Building one allocates
nothing, and comparing two compares two words. On the JVM each case is a
singleton object of the runtime class `okkama.rt.Case`, held in a static field
of a class generated for the enum. Equality is identity, and map keys sort by
the case number.

A **variant** is a type of its own in OIR, and each of its cases is an
object. For

```okkama
enum Reading = (empty, value(item is int), pair(left is int, right is int));
```

OIR declares

```text
variant $variant_Reading_0 = (empty: $variant_Reading_0.empty, value: $variant_Reading_0.value, pair: $variant_Reading_0.pair)

object $variant_Reading_0.empty {
}

object $variant_Reading_0.value {
    field item: int
}

object $variant_Reading_0.pair {
    field left: int
    field right: int
}
```

A case holds only its own payload, so `Reading.pair(1, 2)` is one allocation
with two fields. Four instructions work on variants: `variant.new` builds a
case, `variant.tag` gives its number, `variant.get` reads a field of one
case, and `variant.of` views a case object as the whole variant (the
generated `text()` of an error enum uses it). A `match` that adds up a
`Reading` compiles to

```text
func @Probe.sum(%0: Probe, %1: $variant_Reading_0) -> int [method Probe] {
^b0:
    %2: int = variant.tag %1
    %3: int = const 0
    %4: bool = int.eq %2, %3
    cond %4, ^b1, ^b2
^b1:
    %5: int = const 0
    br ^b5(%5)
^b2:
    %6: int = const 1
    %7: bool = int.eq %2, %6
    cond %7, ^b3, ^b4
^b3:
    %8: int = variant.get %1, 1.0
    br ^b5(%8)
^b4:
    %9: int = variant.get %1, 2.0
    %10: int = variant.get %1, 2.1
    %11: int = int.add %9, %10
    br ^b5(%11)
^b5(%12: int):
    ret %12
}
```

`variant.get` names the case it expects (`2.1` is the second field of case
2) and traps with "Variant payload of an inactive case" when another case is
active. After a `match` has tested the tag the trap cannot fire, and the
check is one comparison.

Each engine keeps a case as one of its own objects. On Sparrow the
cases of a variant get consecutive object descriptors, so `variant.tag` is
the descriptor in the object's header minus that of the first case: a load,
a shift and a subtraction, with no field for the tag. The IR engine finds
the case from the object's declaration. The JVM generates an abstract class
for the variant, which holds the tag, and a subclass for each case with its
payload in ordinary fields.

Until 0.4.2 a variant was a single generated object with a slot for every
field of every case. Each slot was a one-element array, empty when its case
was not active, so `Reading.pair(1, 2)` built three arrays and an object, and
every read went through an accessor that checked the tag (issue #290). Below
is a loop of ten million steps (one million on the IR engine) on an Apple M2
Max. Each step builds one value and takes it apart: a case of a payload-free
enum, an object with two fields, a `value`/`pair` of `Reading`, or a case of
`Wide`, a variant of thirteen cases with two fields each. The times are the
wall time of `okkama run`, compilation included, best of three. The JVM runs
a jar built beforehand, so its time includes the start of the JVM (about
0.4 s) but not the build.

| | payload-free enum | object with two fields | `Reading` | `Wide` |
|---|---:|---:|---:|---:|
| Sparrow, before #290 | 0.44 s | 0.73 s | 2.41 s | 12.64 s |
| Sparrow, 0.4.2 | 0.41 s | 0.73 s | 0.69 s | 0.87 s |
| IR engine, before #290 | 1.71 s | 2.25 s | 5.59 s | 12.50 s |
| IR engine, 0.4.2 | 1.59 s | 2.35 s | 2.52 s | 2.53 s |
| JVM, before #290 | 0.40 s | 0.40 s | 0.74 s | 0.98 s |
| JVM, 0.4.2 | 0.41 s | 0.40 s | 0.52 s | 0.57 s |

A variant now costs about what an object with the same data costs, on every
engine. On Sparrow it is even a little cheaper, because the loop over
objects also calls `sum()`. The old cost grew with the fields of all cases
together: `Wide` built 26 arrays for every value, and ten million values on
Sparrow allocated 260 million arrays and 117 MB. Now they allocate no array
and 22 MB. Nothing a program can observe changed. Equality, `text()`,
catching an error enum and the report of an uncaught one behave as before,
and the conformance suite kept its expected outputs.

## 16.9 What this design costs

described 0.5.x

- a forgotten case is a compile error, not a run-time surprise;
- a value cannot be read through the wrong case;
- equality is exact, and two enums can never be compared by accident;
- a payload-free enum costs no allocation on Sparrow and a pointer
  comparison on the JVM.


## 16.1 A closed set of names

described 0.5.x

The order of a declaration is not entirely invisible. A map keeps its keys
sorted, and enum keys sort by their position in the declaration, on every
engine:

## 16.7 Enums elsewhere

described 0.5.x

## 16.7 Enums elsewhere

## 16.8 Limits

described 0.5.x

## 16.8 Limits
