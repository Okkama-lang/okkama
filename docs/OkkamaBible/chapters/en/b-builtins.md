# Appendix B. Builtin Actions

The values of the built-in types answer a fixed set of actions without any
import. This appendix lists them as the compiler of version 0.6.2 knows them
(its table of builtin signatures). "Same" in a parameter list means a value
of the receiver's own type; `T`, `K` and `V` are an array's element type and
a map's key and value types. Two actions need a standard role in scope,
because their result is that role's type: `order(other)` returns the `Order`
of the module `compare` (import `Order`), and an array's `cursor()` returns
the `Cursor<T>` of `collections/iteration` (import `Cursor`). Without the
import the compiler reports the action as missing (issue #952).

## B.1 Numbers

| Action | `int` | `byte` | `rat` | `float` |
|---|---|---|---|---|
| `abs()` | `int` | — | `rat` | `float` |
| `sign()` | `int` | — | `int` | `int` |
| `pow(n)` | `int`; `n` is a non-negative `int` | — | `rat`; `n` is an `int`, negative inverts | `float`; `n` is a `float` |
| `sqrt()` | `float` | — | — | `float` |
| `min(same)`, `max(same)` | `int` | — | — | `float` |
| `clamp(low, high)` | `int` | — | — | `float` |
| `less`, `less_or_equal`, `greater`, `greater_or_equal` (same) | `bool` | `bool` | `bool` | `bool` |
| `order(same)` | `Order` | `Order` | `Order` | `Order` |
| `numerator()`, `denominator()` | — | — | `int` | — |
| `int()` | `int` | `int` | truncates | truncates |
| `byte()` | fails outside 0..255 | `byte` | truncates, then checks | truncates, then checks |
| `float()` | nearest | nearest | nearest | `float` |
| `rat()` | exact | exact | `rat` | the shortest decimal, exactly |
| `char()` | the code point | — | — | — |
| `string()` | canonical text | canonical text | `1/3`, or `3` | shortest round-trip text |

`float` has eleven more, each giving a `float`: `floor()`, `ceil()`,
`round()` (half away from zero), `trunc()`, `exp()`, `ln()`, `log2()`,
`log10()`, `sin()`, `cos()` and `tan()`, and `atan2(other)`, the angle of
the point `(self, other)`. `finite()` and `nan()` answer a `bool`; for an
Okkama float they are always `true` and `false` (Section 5.5). Every float
result must be finite, or the operation fails.

`int` and `byte` share seven bit actions over a fixed window, 64 bits for
an `int` and 8 for a `byte` (Section 5.3): `bit_count()`,
`leading_zeros()` and `trailing_zeros()` give an `int`; `rotate_left(n)`,
`rotate_right(n)`, `reverse_bits()` and `swap_bytes()` give a value of the
receiver's type.

Operators on numbers: `+`, `-`, `*` and `%` keep the category, two `byte`s
give an `int`, and `/` on two `int`s gives a `rat`. `==`, `!=`, `<`, `<=`,
`>` and `>=` compare two values of one category. `&`, `|`, `^`, `~`, `<<`,
`>>` and `>>>` work on `int` and `byte`. Mixing categories is a compile-time
error everywhere.

## B.2 `bool` and `char`

| Receiver | Actions |
|---|---|
| `bool` | `string()` gives `"true"` or `"false"` |
| `char` | `code()` gives the code point as an `int`; `order(char)` gives an `Order`; `string()` gives a one-character string |

`bool` takes `!`, `&&` and `||`; `char` has no `less` and no `<` (issue
#953).

## B.3 `string`

| Action | Result | Meaning |
|---|---|---|
| `length()` | `int` | code points |
| `empty()` | `bool` | `length() == 0` |
| `slice(start, end)` | `string` | code points `start..end-1`; fails out of range |
| `split(separator)` | `@string` | literal; keeps empty fields; `""` gives the code points |
| `replace(target, replacement)` | `string` | every non-overlapping occurrence |
| `replace_first(target, replacement)` | `string` | the first occurrence |
| `trim()`, `trim_start()`, `trim_end()` | `string` | whitespace at both or one edge |
| `chomp()` | `string` | one trailing line ending |
| `contains(part)`, `starts_with(part)`, `ends_with(part)` | `bool` | literal |
| `find(part)`, `find_last(part)` | `Maybe<int>` | code-point index |
| `lower()`, `upper()` | `string` | Unicode default case mapping |
| `repeat(count)` | `string` | fails for a negative count or a result over 1 GiB |
| `characters()` | `@char` | the code points |
| `less(other)`, `less_or_equal(other)` | `bool` | code-point order |
| `order(other)` | `Order` | code-point order |
| `string()` | `string` | itself |

`+` joins two strings; no other type joins a string.

## B.4 Arrays `@T`

| Action | Result | Meaning |
|---|---|---|
| `size()` | `int` | |
| `item(i)`, `xs[i]` | `T` | fails out of range; `xs[i] else v` gives `v` |
| `first()`, `last()` | `T` | fail on an empty array |
| `slice(start, end)` | `@T` | positions; fails out of range |
| `take(n)`, `drop(n)` | `@T` | at most `n`; a negative `n` fails |
| `reversed()` | `@T` | |
| `replaced(i, value)`, `removed_at(i)` | `@T` | a copy changed at one position |
| `contains(value)` | `bool` | by `==` |
| `index_of(value)` | `Maybe<int>` | by `==` |
| `join(separator)` | `string` | arrays of `string` |
| `ordering()` | `@T` | stable ascending sort through `Comparing` (import it) |
| `cursor()` | `Cursor<T>` | the first position (import `Cursor`) |

The functional operations `map`, `filter`, `reduce`, `any`, `all` and
`find` are written with a block, `xs.map() -> x { … }` (Section 8.2), and
are the only names the parser accepts after `->` in that position. `+`
concatenates two arrays of one element type, `-` removes every element equal
to one on the right (`[1, 2, 1] - [1]` is `[2]`), and ranges expand inside
an array literal: `[0, 2..=4]`. An array cast converts every element:
`@float(xs...)`, `@string(1, 2.5)`, `@byte(255, 3)` and `@int(…)` take
positional values and spreads, and fail on the first element that does not
convert.

## B.5 Maps `%K:V`

| Action | Result | Meaning |
|---|---|---|
| `size()` | `int` | |
| `has(key)` | `bool` | without looking up the value |
| `keys()` | `@K` | in key order |
| `values()` | `@V` | in key order |
| `m[key]` | `V` | fails for a missing key; `m[key] else v` gives `v` |

`map`, `filter` and `reduce` take `key, value` (`reduce` an accumulator
first), and `filter` gives a map. `m + ["k": v]` sets entries and
`m - ["k": v]` removes them by key; the values on the right are ignored.

## B.6 `Future<T>`

| Action | Result | Meaning |
|---|---|---|
| `done()` | `bool` | whether it has settled |
| `cancel()` | `Future<T>` | asks it to stop; using the result discharges the must-use rule |
| `cancelled()` | `bool` | whether it was cancelled |

The value is reached only through `await` (Chapter 20).

## B.7 Conversion functions

Five conversions are also written as functions: `int(x)`, `byte(x)`,
`float(x)`, `string(x)` and `char(n)` mean `x.int()`, `x.byte()`,
`x.float()`, `x.string()` and `n.char()`, and the functions also accept a
value that already has the target type (`string("s")`, `char('a')`), where
the method does not exist. `rat(numerator, denominator)`
takes two integers and makes the reduced fraction; a single argument is an
error, and `x.rat()` converts one value.
