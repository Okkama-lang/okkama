# Chapter 8. Collections

Okkama has three built-in collections — arrays, maps and ranges — and the
standard library adds lists, sets, deques, heaps and lazy views on top of
them. All of them are immutable. That single fact decides most of this
chapter: how collections are built, how they are changed, what they cost and
what is still missing.

## 8.1 Arrays

An array is an immutable, ordered sequence of values of one type, written
`@T`. Literals use brackets, elements are indexed from zero, and every
operation that "changes" an array returns a new one.

{{example:08-collections}}

The first lines of the program show the builtin actions of arrays: `size()`,
`first()`, `last()`, `slice(start, end)`, `join(separator)` for arrays of
strings, `ordering()` to sort, and `item(index)` — or the indexing operator
`array[index]`. An index outside the array is a runtime failure, unless the
expression supplies a fallback: `primes[9] else 0` is `0`. Like
`slice` on strings, nothing is clamped silently.

Ranges expand inside array literals: `[9, 0, 2..=4, 1]` is the array
`9 0 2 3 4 1`. `a..b` excludes its end and `a..=b` includes it; bounds are
integers and the start may not exceed the end.

`ordering()` sorts an array whose elements can be compared. The comment in
the example records a wrinkle of the current compiler: the builtin
recognises numbers and strings as comparable only when the `Comparing` role
from the `compare` module is in scope (issue #233).

### Typed identity

An array remembers its element type. Once it crosses a declared boundary —
`let names is @string = ...`, a parameter, a field — it is tagged with that
type, and even an empty result of `slice` or `keys` keeps it. An untyped
empty literal `[]` has no evidence of its type, so it may cross any
compatible boundary once, and that boundary fixes its type. This is why empty
collections sometimes need an annotation (Chapter 4).

### Equality

Arrays compare deeply: two arrays are equal when they have the same length
and equal elements in the same order. Maps compare by keys and values,
ranges by their bounds and kind, objects by type and *all* fields. Comparing
values of different categories is an error, not `false` — the same rule as
`1 == 1.0` in Chapter 5.

## 8.2 Functional operations

Three operations — `map`, `filter` and `reduce` — work on arrays, ranges,
maps and any object that performs the `Iterating` role. They are written with
a dedicated block syntax:

```okkama
let squares = (1..=5).map() -> n { n * n; };
let odd = squares.filter() -> n { n % 2 == 1; };
let total = squares.reduce(0) -> sum, n { sum + n; };
```

The block after `->` names its bindings and computes one value. For a map,
`map` and `filter` bind `key, value`, and `reduce` binds `accumulator, key,
value`. The operations are **eager**: each produces a complete array (or, for
`filter` on a map, a map). They visit elements in order — maps in sorted key
order — exactly once, and the first failure inside a block stops the
operation.

Why only three, and why a special syntax? `map`, `filter` and `reduce` are the
classical trio from which most collection algorithms are built, and they cover
the overwhelming majority of loops that produce a value. A dedicated syntax
makes the block a part of the call rather than an ordinary argument: it may
use the surrounding names, it cannot be stored or passed on, and the compiler
can lower it to a plain loop without allocating a closure. Lazy views —
`Taken`, `Filtered`, `Chunked` and friends — live in
`collections/views` for the cases where materialising every step would be
wasteful (Chapter 22).

The operations cannot exit early: there is no `last` inside a functional
block, so `reduce(false) -> found, x { found || x == y; }` visits every element
even after it has found one. Early-exit operations — `any`, `all`, `find` —
are among the open language proposals (issue #150).

## 8.3 Maps

A map `%K:V` associates keys with values. Map literals use brackets with
`key: value` pairs, and the keys are *expressions*: `["apple": 5]` maps the
string `"apple"`; `[apple: 5]` would use the value of a variable named
`apple`. There is no implicit string-key shorthand, so a typo cannot quietly
turn a variable into a string or the other way round.

Keys must be scalar values — integers, strings, booleans and the other
scalar categories, except `byte` and `any` — and the map keeps them **in sorted
order**. `keys()` and
`values()` return arrays in that order, and every functional operation visits
entries in it. Iteration order therefore never depends on hashing, insertion
history or the memory layout of a run: the same program prints the same output
every time, on every engine. Python made its dictionaries keep insertion order
in version 3.7 for the same reason — predictability — and many test suites
in languages with unordered maps contain workarounds for nondeterministic
output.

A lookup of a missing key is a runtime failure unless it has a fallback:
`stock["kiwi"] else 0`. `has(key)` asks without looking up, and a `Maybe` is
one `if` away when an action wants to return absence (Chapter 7).

## 8.4 Ranges

A range is a value: `1..5` and `1..=5` can be stored, passed and compared.
They are the source of most counted loops (`for 0..n -> i`), expand inside
array literals, and accept the functional operations, producing arrays.
Chained ranges such as `1..5..9` are rejected, and a range whose start is
greater than its end is an error rather than an empty range — a reversed
range is more often a bug than an intention.

## 8.5 Building and changing collections

Immutability raises the practical question: how do you build a collection
element by element, or change one entry of a map?

The language's answer today has three parts.

1. **Build in one step when possible.** `map`, `filter` and range expansion
   build complete arrays in one pass. Most collections in real programs are
   transformations of other collections, and these are linear.
2. **Accumulate with `reduce` or a library collection.** The standard library
   has persistent collections — `List` (a linked list with constant-time
   prefix), `Deque`, `OrderedSet`, `Heap` — whose "updates" return new
   versions that share structure with the old ones.
3. **Rebind a local that holds a successor-capable object.** A local may be
   rebound (`counter = counter.increment();`) only when its type is an object
   with successor actions; `let n = 1; n = 2;` is rejected. Scalar and
   collection locals cannot be rebound at all, which is why accumulation goes
   through `reduce` or library objects.

What is missing is the most common operation of all: appending to an array,
and setting one key of a map. Without them, code that genuinely builds an
array step by step — a parser collecting tokens, a server registering
connections — has to copy the whole array at each step. The standard library
does exactly that in about forty places, with an idiom like

```okkama
let following = (0..old.size() + 1).map() -> index {
    if index < old.size() { old[index]; } else { item; }
};
```

which copies `old` to add one element. Figure 8.1 measures the consequence.

{{figure:append-cost|Figure 8.1. Building an array of n integers: one map() stays flat; appending one element at a time by copying is quadratic (issue #145). Data: data/append-cost.json.}}

One `map()` builds 16,000 elements in the time it takes to start the process;
appending them one at a time by copying takes 3.7 seconds, and doubling the
size roughly quadruples it. This is the clearest case in the language of the
rule from Chapter 1: removing mutation must not remove efficiency, and the cure
is an operation, not a mutable entity.

The cure is well known. Persistent vectors and hash maps — Bagwell's hash
array mapped tries [@bagwell2001], as used by Clojure [@hickey2008], and the
structures in Okasaki's book [@okasaki1998] — give immutable collections
effectively constant-time append and update by sharing all but a logarithmic
path of their structure between versions. Issues #145 (array append and
concatenation) and #151 (map update) propose bringing these operations into
the language, named like the standard library's existing successors:
`array.addition(x)`, `map.addition(key, value)`, `map.difference(key)` — the
word `with`, common in other languages for this, is reserved in Okkama.

## 8.6 Iterating user collections

`for` walks arrays, ranges, maps' keys through `keys()`, and any object that
performs the standard `Iterating<T>` role from `collections/iteration`:

```okkama
role Cursor<T> {
    available() is bool;
    item() is T;
    successor() is Cursor<T>;
}

role Iterating<T> {
    cursor() is Cursor<T>;
}
```

A cursor is immutable: `successor()` returns the next cursor and leaves the
current one where it was, so two loops over the same collection never
interfere, and a cursor can be kept to resume later. Exhaustion is an
ordinary `false` from `available()` — no sentinel, no null, no exception. The
role is nominal: a collection must declare `does Iterating<T>`; having the
right method names is not enough.

## 8.7 Limits

- **No append or concatenation for arrays**, so incremental building copies
  (issue #145, Figure 8.1).
- **No map update**: changing one entry rebuilds the map (issue #151).
- **Map keys must be scalars.** Objects cannot be keys and there is no hashing
  contract, so sets and maps keyed by objects are linear in the library
  (issue #212).
- **Few array builtins.** `contains`, `index_of`, `reversed` and their kin are
  written with `reduce` today (issue #211).
- **No early exit from functional operations** (issue #150).
- **`ordering()` needs `compare.Comparing` in scope** (issue #233).

Most of these are the same gap seen from different sides: an immutable
language needs efficient persistent operations, and Okkama has the
immutability already and the operations not yet.
