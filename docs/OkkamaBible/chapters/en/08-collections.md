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

`ordering()` sorts an array whose elements can be compared. It compares
them through the standard role `Comparing` of the `compare` module, so the
program imports that role even to sort numbers or strings, as the example
does. Without the import the compiler names it: "compares elements through
the role Comparing, which this program does not load: add
`import "compare" take Comparing;`" (issue #233).

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

## 8.7 Counts and positions: what happens at the edges

Array actions take two kinds of number, and they behave differently at the
edge of the array. A **count** says how many: asking for more than there is
gives what there is. A **position** names an element: a position outside
the array is a failure.

{{example:08-edges}}

| Action | Argument | Beyond the end | Negative |
|---|---|---|---|
| `take(n)` | count | the whole array | fails |
| `drop(n)` | count | `[]` | fails |
| `slice(start, end)` | positions | fails | fails; so does `start > end` |
| `item(i)`, `xs[i]` | position | fails; `xs[i] else v` gives `v` | fails; `xs[i] else v` gives `v` |
| `replaced(i, v)`, `removed_at(i)` | position | fails | fails |
| `first()`, `last()` | — | fail on an empty array | — |

Such a failure is not a thrown error a caller can catch; it is a runtime
failure that stops the program with its location, like a division by zero
(Chapter 10). `try? xs.slice(1, 9)` does not turn it into `Maybe.none`. The
asymmetry is deliberate. A count is a request — "at most three" — and
answering it with fewer is the useful answer. A position is a claim — "the
element at five exists" — and a false claim is a bug to surface, not a value
to invent. When a position may legitimately be missing, the program says
so with `else`.

## 8.8 Sorting

`ordering()` returns the array sorted in ascending order, and it is the only
sort the language has. It orders elements through the standard role
`Comparing<T>` from `compare`, so the program must import that role even to
sort integers, and an object sorts when it performs `Comparing` (Chapter
14). The sort is stable: elements that compare equal keep their original
order, so sorting by one key after another composes. Strings sort by code
point, as `order()` compares them (Chapter 6), which puts `"B"` before
`"a"`.

There is no descending variant and no sort by a key function. Descending
order is `xs.ordering().reversed()`, and sorting by a key is an object that
performs `Comparing` on that key — a small object with a name, in place of
an anonymous comparator. This is the razor's trade again: one operation and
one role instead of a family of sort methods.

The program below sorts runners by time, then by club, then slowest
first, and finally by a ranking that does not know every club.

{{example:08-sorting}}

- **A key is an object.** `ByTime` and `ByClub` each wrap a runner and
  compare one field. Sorting by a key is three steps: wrap, `ordering()`,
  unwrap. The wrapper has a name, which an anonymous comparator does not,
  and the name says what the order is.
- **Stability composes.** Bo and Cy both ran 39 minutes, and the sort by
  time keeps them in their input order. Sorting that result by club keeps
  the time order inside each club: Cy before Ada in North. To sort by
  several keys, sort by the least significant one first.
- **Reversal is not a stable descending sort.** `reversed()` turns the
  whole array around, ties included: slowest first puts Cy before Bo. When
  the order of equal elements matters, the descending key is written as an
  object of its own, whose `order` compares the other way.
- **A comparison may fail, and the sort stops.** `ByRank` throws for a
  club it has no rank for, and the first such comparison ends the sort with
  that error. There is no half-sorted array to clean up: `ordering()`
  returns a new array or fails, and `runners` is what it was.

## 8.9 A worked example: counting words

Maps, `reduce` and `ordering()` are enough for the classic exercise of
counting words, without a mutable counter anywhere.

{{example:08-words}}

The `reduce` threads a map through the words. Each step returns the map with
one entry set: `seen + [word: n]` adds or replaces the entry, and the old
map stays as it was. The empty map needs a declared type
(`let empty is %string:int = [:];`), because `[:]` alone says nothing about
its keys and values (Chapter 4). Iterating `counts.keys()` gives the words
in sorted order, `and` before `the`, whatever order they arrived in, so the
report is identical on every run and every machine. That determinism is a
property of the map, not of the program.

## 8.10 A worked example: grouping and joining

Reports are where collections earn their keep: rows arrive in one order and
must be grouped, totalled and joined with other data. Here are five sales,
grouped by customer and then by city and month.

{{example:08-report}}

- **Grouping is a `reduce` into a map of arrays.** Each step looks up the
  customer's earlier sales, `groups[sale.buyer()] else []`, and returns the
  map with that entry replaced by the longer array. The fallback says what a
  first sale means, an empty history, at the one place it matters. Nothing is
  mutated: every step yields a new map, and the arrays inside it keep the
  sales in the order they arrived.
- **A join is a lookup with a fallback.** `cities[sale.buyer()] else "?"`
  joins each sale with its customer's city. `cy` has no city, and the program
  decides here, in the expression, that such a sale goes to `"?"`. In SQL
  the same question is the choice between an inner and an outer join; here it
  is one `else`, written next to the lookup it qualifies.
- **A composite key is a value object.** `Bucket` has only scalar fields, so
  it can key a map (Section 8.3), and two buckets with the same city and month
  are the same key. The map keeps its keys in order field by field: city
  first, then month. Strings compare by code point (Section 4.7), so `"?"`
  comes before `"Lima"` and `"Oslo"`.
- **The output is the same on every run.** Both loops walk `keys()`, which
  are sorted, so neither report depends on the order of the input or on
  hashing. Shuffle `sales` and the program prints the same eight lines.

In SQL this is `GROUP BY` and `JOIN`, and the join itself goes back to
Codd's relational model [@codd1970]. The operations are the same; what the
program adds is that each one is a typed expression whose result can be
named, tested and passed on.

## 8.11 Beyond arrays and maps: the library's collections

Arrays, maps and ranges are built in. Everything else is a library object,
and the library's collections follow the same rule as the built-in ones:
nothing changes, and every operation that adds or removes gives a new
collection.

{{example:08-library}}

- **A `Deque` is the queue of a breadth-first search.** `first()` reads the
  front, `tail()` drops it, and `suffix(n)` adds at the back. Each returns a
  new deque, and adding at either end costs O(1) amortized. The search moves
  forward by rebinding `search` to its successor (Section 13.5), and nothing
  else in it changes.
- **An `OrderedSet` remembers what was reached.** `membership` answers in
  the time of its kind, and `addition` gives a new set. The order of
  insertion is kept, so the traversal of a set never depends on hashing.
  `KeyedSet` keeps map keys in key order instead.
- **A `Heap` hands out the least value by its own order.** `Delivery` does
  `Comparing<Delivery>` by the hours left, so `top()` is the most urgent
  delivery. `addition` gives a new heap with one more value, and
  `pending.top()` is still Astana after `later` was made from it.
- **Old versions stay valid and share their parts.** `pending` and `later`
  are both usable, and the second shares the tree nodes of the first. This
  is the persistence of Section 8.5, extended to structures the language
  does not build in.

The library also has `List` (a persistent linked list), `Frequencies`
(counts in first-occurrence order), `Sorted` and its binary-search
projections, lazy views (`Mapped`, `Filtered`, `Chunked` and others), and
combinatorial cursors. Each is an object that does some of `Iterating`,
`Sizing` and `Sequencing`, so `for`, `reduce` and the other operations of
Section 8.2 accept it as they accept an array.

One wart shows in the example. `addition` on a set returns the role
`Set<T>`, and the role cannot also promise `Iterating<T>`, because roles do
not compose yet. A set that has gone through `addition` can answer
`membership` and `size`, but `for` cannot walk it without a cast that the
language does not have. The example keeps the order in an array of its own.
The fix belongs to the language (role composition) or to the library (an
`addition` that returns the concrete set). It is listed with the other gaps
in Chapter 28.

## 8.12 Okkama among other languages

| | Default collections | Map iteration order | `map`, `filter` |
|---|---|---|---|
| Python | mutable `list` and `dict` | insertion order, guaranteed since 3.7 [@pystdtypes] | lazy iterators [@pyfunctions] |
| Java | mutable; `List.of` and `List.copyOf` give unmodifiable lists [@javalist] | depends on the class: unspecified for `HashMap`, sorted for `TreeMap` | lazy streams [@javastream] |
| Go | mutable slices and maps | not specified, and not the same from one iteration to the next [@gospec] | loops |
| Rust | `Vec`, `HashMap`, changed through `mut` | arbitrary for `HashMap` [@rusthashmap], sorted for `BTreeMap` | lazy iterators [@klabnik2023] |
| Clojure | persistent, immutable, sharing structure [@hickey2008] | unordered `hash-map`, ordered `sorted-map` [@clojureds] | lazy sequences |
| Okkama | immutable arrays and maps | always sorted by key | eager; lazy views in `collections/views` |

Two columns show the choices.

- **Order.** Go goes furthest in the other direction: the specification
  promises nothing about map order, so a program that depends on it is
  wrong. Python chose insertion order. Okkama chooses sorted order for every
  map, so the order follows from the keys alone: two maps with the same
  entries iterate the same way, however they were built (Section 8.3).
- **Laziness.** Python, Java, Rust and Clojure make `map` and `filter` lazy:
  nothing runs until the result is consumed, which saves work on long
  pipelines and makes the moment of a failure harder to see. Okkama's are
  eager, so a failure inside a block happens at the call that contains it.
  When laziness pays, the program asks for it by name with a view
  (Section 8.2).

Clojure is the closest relative. Its collections are immutable values that
share structure between versions, as Okkama's persistent library
collections do (Section 8.5); the difference is that in Okkama there is no
mutable collection beside them to fall back on.

## 8.13 Limits

- **What still copies.** The `map()` idiom and `a + b` of two large arrays cost the full result.
- **A large map’s first update costs a copy.** Subsequent updates can be cheaper; do not assume every update has the same cost.
- **`ordering()` needs `import "compare" take Comparing;`**, even for
  numbers and strings; the compiler names the missing import (issue #233).
