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

Five more builtins search and reshape (issue #211): `contains(value)` and
`index_of(value)` — a `Maybe<int>` — compare elements with `==`, so a value
object is found by its fields; `reversed()` turns an array around; and
`take(count)` and `drop(count)` keep, respectively remove, at most `count`
elements from the start. Unlike `slice`, these two are defined for any
count that is not negative — `[1, 2].take(5)` is `[1, 2]` — because "at most
`count`" is what their names promise; a negative count fails.

Two more give a copy changed at one index (issues #250, #251).
- `replaced(index, value)` is the array with `value` at `index`.
- `removed_at(index)` is the array without the element at `index`.

The receiver keeps its elements, as everywhere. An index outside the array
fails as indexing does. `value` meets the element type by the rules of a
parameter boundary: an array of a role takes an implementer, and `@any`
boxes. No other conversion applies.

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
compatible boundary once, and that boundary fixes its type. Inside a larger
expression its neighbours count as such evidence, at any depth and in either
order: `[[1], []]` and `[[], [1]]` are `@@int`, `["a": [1], "b": []]` is
`%string:@int`, and `[[1]] == [[]]` or `if c { [[]]; } else { [[2]]; }`
type-check. Only `never` parts are filled in this way. Two concrete types
still disagree (`[[1], ["s"]]`), and an expression that is empty all the way
down waits for a boundary. This is why empty collections sometimes need an
annotation (Chapter 4).

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

`map`, `filter` and `reduce` cover most loops that produce a value. Their blocks are part of the call: they may use surrounding names but cannot be stored or passed on. Lazy views — `Taken`, `Filtered`, `Chunked` and friends — live in `collections/views` when materialising every step would be wasteful (Chapter 22).

There is no `last` inside a functional block: `reduce` always visits every
element. Searches therefore have their own operations, which stop at the
first decisive item: `any` (is there one?), `all` (are they all?) and `find`
(a `Maybe` of the first match) — `xs.any() -> x { x.overdue(); }` where one
would otherwise write `reduce(false) -> found, x { found || x.overdue(); }`
and walk the whole array (issue #150). A search for an equal element needs
no block at all: `xs.contains(y)`.

## 8.3 Maps

A map `%K:V` associates keys with values. Map literals use brackets with
`key: value` pairs, and the keys are *expressions*: `["apple": 5]` maps the
string `"apple"`; `[apple: 5]` would use the value of a variable named
`apple`. There is no implicit string-key shorthand, so a typo cannot quietly
turn a variable into a string or the other way round.

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
every time. Python made its dictionaries keep insertion order
in version 3.7 for the same reason — predictability — and many test suites
in languages with unordered maps contain workarounds for nondeterministic
output.

A lookup of a missing key is a runtime failure unless it has a fallback:
`stock["kiwi"] else 0`. `has(key)` asks without looking up, and a `Maybe` is
one `if` away when an action wants to return absence (Chapter 7).

A generic object can key a map by its own type parameter:
`object Index<T> { has entries is %T:int; … }`. Such a `T` is a *key
parameter*, and the compiler holds every specialization to the key rule
above: `Index<string>` and `Index<Card>` compile, `Index<@int>` does not, and
neither does a `Wrap<U>` that passes its `U` on to an `Index<U>` and is given
an array (issue #242). The standard library's `KeyedSet<T>` and
`KeyedFrequencies<T>` are built this way — logarithmic membership and counts,
traversal in key order — beside `OrderedSet` and `Frequencies`, which accept
any `T` with equality, arrays included, and pay for it with a linear search.

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
   prefix), `Deque`, `OrderedSet`, `KeyedSet`, `Heap` — whose "updates" return new
   versions that share structure with the old ones.
3. **Rebind a local that holds a successor-capable object.** A local may be
   rebound (`counter = counter.increment();`) only when its type is an object
   with successor actions; `let n = 1; n = 2;` is rejected. Scalar and
   collection locals cannot be rebound at all, which is why accumulation goes
   through `reduce` — or, in synchronous code, through a `cell::Cell`, the one
   explicit mutable container, which the compiler keeps out of async actions
   and object fields (issue #152).

Use `+` and `-` to append array elements or set and remove map entries: `items + [item]`, `a + b + c`, `prices + ["fig": 90]`, `prices - ["pear": 0]`. A concatenation chain costs the size of its result. Inside `reduce`, a block that returns only `acc` or `acc + [item]` appends in amortized constant time; earlier observable values remain unchanged.

One idiom still copies: it builds a new array with `map` at each step. Repeating it to grow a collection gives quadratic total work:

```okkama
let following = (0..old.size() + 1).map() -> index {
    if index < old.size() { old[index]; } else { item; }
};
```

This is the clearest case in the language of the rule from Chapter 1:
removing mutation must not remove efficiency, and the cure is an operation,
not a mutable entity.

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

A source that is consumed as it is read — a file, a socket — performs
`Batching<T>` instead, from the same module:

```okkama
role Batching<T> {
    batch() is @T;      // the next batch; [] ends the source
}
```

`for file.text() -> line { … }` asks for a batch, walks it as an array and asks again. `next`, `last` and `return` work as in any loop; `map`, `filter` and `reduce` accept such sources too. Batches amortize the cost of requesting items over several values. A source that does both roles is walked in batches.

## 8.7 Limits

- **What still copies.** The `map()` idiom and `a + b` of two large arrays cost the full result.
- **A large map’s first update costs a copy.** Subsequent updates can be cheaper; do not assume every update has the same cost.
- **`ordering()` needs `compare.Comparing` in scope** (issue #233).
