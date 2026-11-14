# Chapter 8 — archived implementation passages

Replaced mixed paragraphs are preserved in full for context.

## 8.2 Functional operations

described 0.5.x

Why only three, and why a special syntax? `map`, `filter` and `reduce` are the
classical trio from which most collection algorithms are built, and they cover
the overwhelming majority of loops that produce a value. A dedicated syntax
makes the block a part of the call rather than an ordinary argument: it may
use the surrounding names, it cannot be stored or passed on, and the compiler
can lower it to a plain loop without allocating a closure. Lazy views —
`Taken`, `Filtered`, `Chunked` and friends — live in
`collections/views` for the cases where materialising every step would be
wasteful (Chapter 22).

## 8.3 Maps

described 0.5.x

Keys are scalar values — integers, strings, booleans and the other scalar
categories, except `byte` and `any` — or value objects. A `rat` key is its
exact value: `rat(1, 2)` and `2 / 4` are one key (since 0.4.2). Value objects
are objects whose fields are all scalars, enums or such objects, with no
`destroy`. Because objects are
immutable, an object key is its fields: two equal cards are one key, exactly
as `==` says. The map keeps its keys **in sorted order** — objects field by
field in declaration order. `keys()` and
`values()` return arrays in that order, and every functional operation visits
entries in it. Iteration order therefore never depends on hashing, insertion
history or the memory layout of a run: the same program prints the same output
every time, on every engine. Python made its dictionaries keep insertion order
in version 3.7 for the same reason — predictability — and many test suites
in languages with unordered maps contain workarounds for nondeterministic
output.

## 8.5 Building and changing collections

described 0.5.x

The most common operations — appending to an array, setting or removing one
key of a map — are `+` and `-` (issue #145): `items + [item]`, `a + b + c`,
`prices + ["fig": 90]`, `prices - ["pear": 0]`. A chain is one allocation of
the result, never a series of intermediate arrays. Inside `reduce` the
accumulator also grows in place when nothing else sees it (issue #243): a
block that only returns `acc` or `acc + [item]` appends in amortized constant
time, the way a mutable vector would, because no one can observe the
intermediate arrays.

Elsewhere `+` used to copy. Until 0.4.3 the result was a fresh copy, so code
that built an array step by step through an object's fields or a shared cell
copied the whole array at each step.

Since 0.4.3, Sparrow gives these operations representations that share
structure between versions (issues #246–#255). Not a line of the programs
that use them changes.
- **Appends and prepends.** A chain of `items + [x]` or `[x] + items` on an
  array of 32 elements or more writes into a buffer the versions share: at
  its end, or before its start, when the version being extended ends (or
  starts) there. Every older version keeps its elements. A second version
  made from the same one copies, as before.
- **Slices.** `slice`, `take` and `drop` share their origin's storage when
  they keep at least half of it, so a slice keeps alive at most twice what it
  holds.
- **Replacements.** A chain of `replaced` on an array of 64 elements or more
  turns it into a 32-way trie, so a replacement copies one path from the
  root. A trie that is only read rewrites itself flat after two passes.
- **Maps.** A map of more than 64 entries turns into a persistent B+ tree the
  first time a version is made from it. Later versions share its nodes.
- **Strings.** Chains of `+` on strings share text buffers the same way
  (Chapter 6).

Arrays and maps that are built once and then only read stay flat, so reading
them costs what it did.

One idiom still copies on every engine, because it builds a new array with
`map` at each step. The standard library used it before `+` existed:

## 8.5 Building and changing collections

described 0.5.x

Since 0.5.3 the IR engine and the JVM keep chains the same way (issues #335,
#336). The rules are Tomato's, adapted to their memory: reference counts
with a cycle collector on the IR engine, the JVM's own collector and
`java.util.List` on the JVM. A step of an append chain at 100 000 elements
costs 200–400 ns on the IR engine and 45–70 ns on the JVM, where each was a
copy of the whole array. Figure 8.1 measures the difference on Sparrow.

{{figure:append-cost|Figure 8.1. Building an array of n integers on Sparrow: one map() stays flat; the map() idiom and items + [x] by copying (0.4.2) are quadratic; items + [x] over the shared buffers of 0.4.3 stays flat (issues #145, #246). Data: data/append-cost.json.}}

One `map()` builds 64,000 elements in the time it takes to start the process,
about 11 milliseconds. Appending them one at a time with `+` took 1.1 seconds
when every step copied, and doubling the size roughly quadrupled the time. On
0.4.3 it takes 15 milliseconds. The idiom, which builds a new array at each
step, still takes 0.85 seconds for 16,000 elements and 3.4 for 32,000.

## 8.5 Building and changing collections

described 0.5.x

The cure is well known. Persistent vectors and hash maps give immutable
collections effectively constant-time append and update, by sharing all but
a logarithmic path of their structure between versions. Examples are
Bagwell's hash array mapped tries [@bagwell2001], as used by Clojure
[@hickey2008], and the structures in Okasaki's book [@okasaki1998].

Okkama took the step in 0.4.3 without a new operation: `+` and `-` stayed,
and only their representation changed. It took the cure piecemeal rather
than wholesale.
- **Tries only where they pay.** Measured on Sparrow, a trie read in an index
  loop costs 45–75% more than a flat array, and programs read far more than
  they write. So tries serve only chains of `replaced`, and flatten
  themselves back once they are only read.
- **Buffers for appends.** Appends get a flat buffer with a frontier instead,
  which reads like any flat array.
- **An ordered tree for maps.** Maps get an ordered B+ tree rather than a
  hash trie, because Okkama's maps are ordered.

## 8.6 Iterating user collections

described 0.5.x

`for file.text() -> line { … }` asks for a batch, walks it
as an array and asks again; `next`, `last` and `return` work as in any loop,
and `map`, `filter`, `reduce` take such a source too. The reason is cost. A
cursor is an object and three role calls per item, about 0.7 µs on Tomato; a
batch is one call per batch and then the array loop, about 0.09 µs per line
of a file whose batches the runtime cuts in Rust (#465). A source that does
both roles is walked in batches.

## 8.7 Limits

described 0.5.x

- **What still copies.** Two things copy on every engine: the `map()` idiom,
  and `a + b` of two large arrays (issue #248 was declined). Chains of `+` on
  strings copy on the IR engine and the JVM, which keep `Rc<str>` and
  `java.lang.String` (issue #357, and the JVM's phase D of #336, on hold).
  On the IR engine only arrays and maps of plain values (numbers, strings,
  and collections of them) share their nodes, because its cycle collector
  counts every edge as one strong reference; arrays and maps that hold
  objects are copied as before. On the JVM, tries for chains of `replaced`
  are built but off by default (`OKKAMA_ARRAYS=trie`): the JIT needs some
  fifteen passes over a flattened trie to read it as fast as an `ArrayList`.
- **A large map's first version copies.** A map of more than 64 entries
  becomes persistent at the first version made from it: that version costs
  one copy, and the later ones a path. Maps of 64 entries or fewer are
  copied whole, which is cheaper at that size.
- **`ordering()` needs `compare.Comparing` in scope** (issue #233).

The first two are what is left of the gap that the milestone "Persistence
Operations" tracked. Okkama had the immutability and the operators. Since
0.4.3 Sparrow has the representations, and the other engines follow.

