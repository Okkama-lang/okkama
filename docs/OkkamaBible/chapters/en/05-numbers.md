# Chapter 5. Numbers

Numbers are where programming languages lie most often. A machine integer is
not an integer: it is a residue modulo 2⁶⁴ that pretends to be one until it
overflows. A floating-point number is not a real number: it is the nearest of
a finite set of binary fractions, and `0.1` is not among them. Most languages
expose these representations directly and leave the programmer to remember
where they differ from arithmetic. Okkama does the opposite: its default
numbers *are* the mathematical objects they look like, and the machine
representations are used only where the program asks for them.

## 5.1 Four categories

Okkama has four numeric categories, and they never convert into each other
silently.

| Category | What it is | Literal | Exact? |
|---|---|---|---|
| `int` | an integer of any size | `42`, `-7`, `0xFF`, `0b1010` | yes |
| `rat` | a fraction of two integers, always reduced | `1 / 3`, `rat(2, 6)` | yes |
| `float` | an IEEE 754 binary64 number, always finite | `3.14` | no |
| `byte` | an octet, `0..255` | `byte(255)` | yes |

Arithmetic takes two operands of the same category, with one exception that
never loses information: `/` on two integers gives a `rat`. So `+`, `-`, `*`
and `%` on two `int`s give an `int`, on two `rat`s a `rat`, on two `float`s a
`float`, and two `byte`s add up to an `int`. Mixing categories is an error —
`1 == 1.0`, `1.5 + 1`, even `1 + 1 / 2` — and the program converts
explicitly: `1.rat() + 1 / 2`. The rule has no exceptions by design: even the
step from `int` to `rat`, which loses nothing, is written down (issue #230
records the decision).

{{example:05-exact}}

Every line of this output differs from at least one mainstream language. In C,
Java and Go, `9223372036854775807 + 1` wraps to a negative number; in Okkama
it is the next integer. `30!` needs more than 64 bits and is exact. `1/3 +
2/3` is exactly `1`, and `1/10 + 2/10` is exactly `3/10`, while the same sum in
`float` shows the binary rounding every programmer meets eventually. `-7 % 3`
is `-1`: the remainder has the sign of the dividend, as in C and Java (Python
gives `2`). `int(-3.9)` truncates toward zero.

## 5.2 Exact integers

An Okkama `int` behaves as a mathematical integer: results do not wrap at a fixed-width boundary. Larger results take more time and memory as their digit count grows.

**Why.** Overflow is a correctness failure that type checking does not catch.
The loss of the first Ariane 5 in 1996 came from converting a value into a
16-bit integer too small for it [@lions1996]; countless security
vulnerabilities come from a size computation that wrapped around. Languages
that care about correctness have been moving away from silent wrapping: Python
decided in 2001 to unify its integer types into one arbitrary-precision type,
a change completed in Python 3 [@pep237], Scheme has long distinguished exact from inexact numbers in its
numeric tower [@r7rs2013], and Rust panics on overflow in debug builds. Okkama
goes all the way: there is no wrapping integer type to choose by mistake.

A product that cannot fit in the run’s memory budget is refused before computation. Powers are exact too: `(2).pow(100)` is `1267650600228229401496703205376`. An `int` cannot be raised to a negative power; `rat.pow` accepts one and inverts (issue #228).

**Cost.** Exact arithmetic must preserve results beyond fixed-width bounds. Once results become big, their cost grows with the number of digits.

## 5.3 Integer operations

{{example:05-integers}}

**Division and remainder.** `/` on two integers never truncates: `7 / 2` is
the rational `7/2`, with `numerator()` 7 and `denominator()` 2. An integer
quotient is a decision, so it is written: `int(7 / 2)` truncates toward zero
and gives 3, and `int(-7 / 2)` gives −3. `%` is the remainder that goes with
that quotient, so it has the sign of the dividend: `-7 % 3` is −1 and
`7 % -3` is 1, and `a == int(a / b) * b + a % b` holds for every `a` and
every nonzero `b`. Dividing by zero, with `/` or `%`, is a runtime error, a
bug in the program rather than a thrown `Error` (Section 10.4).

**Powers.** `pow` takes a non-negative exponent and is exact at any size:
`(2).pow(64)` is 18446744073709551616, one more than the largest unsigned
64-bit value printed on the next line. A negative exponent belongs to `rat`
(Section 5.4).

**Bits: a 64-bit window.** The bitwise operators work on a bit pattern, and a
bit pattern needs a width. Okkama gives every bitwise operation the same one:
the 64-bit two's complement view of an `int`, the width that hash functions,
checksums and network protocols use.
- **The operators.** `&`, `|`, `^` and `~` work bit by bit; `~5` is −6, as in
  every two's complement language.
- **Shifts.** `<<` shifts left, `>>` shifts right copying the sign bit, and
  `>>>` shifts right filling with zeros, so `-8 >> 1` is −4 and `-1 >>> 60`
  is 15. A shift count must be in `0..63`.
- **Methods.** `bit_count`, `leading_zeros`, `trailing_zeros`,
  `rotate_left`, `rotate_right`, `reverse_bits` and `swap_bytes` use the same
  window on an `int` and an 8-bit one on a `byte`.

Inside the window, a shift drops the bits that leave it: `1 << 63` is the
lowest 64-bit value, not 2⁶³. This is the one place where an Okkama integer
operation is modular, and it is deliberate. A shift asks for a bit pattern,
not for a number, and the result is the pattern a hash or a checksum expects.
Arithmetic never does this, and neither does an operand outside the window.
`(2).pow(70) & 1` is not silently truncated to 64 bits; it is a runtime error,
"Bitwise operand is outside signed-64 range".

**Comparisons.** `==`, `!=`, `<`, `<=`, `>` and `>=` compare two values of one
category, and so do the methods `less`, `greater`, `less_or_equal` and
`greater_or_equal`. `min`, `max` and `clamp` give one of their operands back:
`7.min(3)` is 3 and `7.clamp(0, 5)` is 5. Comparing an `int` with a `float` or
a `byte` is a compile-time error, like adding them. The program converts one
side, and that conversion says which comparison was meant (Section 5.7).

## 5.4 Exact rationals

Dividing two integers gives a `rat`, an exact fraction kept in lowest terms
with a positive denominator. Rationals add, subtract, multiply and divide with
other rationals; an integer joins them through `.rat()`. `rat(2, 6)` and `1 / 3` are the same value and
print as `1/3`; a fraction whose denominator reduces to one prints as an
integer, so `6 / 2` prints `3`.

Rationals are the right numbers for quantities that must add up exactly:
shares, proportions, money before rounding. The program below divides a bill
between three people without losing a cent — and makes the rounding rule,
which every real system needs and most hide, an explicit line of code.

{{example:05-money}}

`bill.share(3)` is `10000/3` cents, exactly. `int(...)` truncates it to 3333,
and the remainder goes to the first payer. The three payments add up to the
bill because every step before the explicit rounding was exact.

**Why.** Exact rationals are standard in Scheme and Lisp [@r7rs2013] and in
computer algebra systems, and absent from most industrial languages, where
`1 / 3` silently becomes `0` or `0.333…`. Okkama's choice follows from the
categories: if integer division must not lose information, its result cannot
be an integer.

**Cost.** Numerators and denominators can grow: repeatedly adding fractions
with unrelated denominators makes them large, and arithmetic slows down with
them. Programs that iterate thousands of times over rational quantities should
round deliberately at known points, as the bill example does.

## 5.5 Floating point, kept finite

`float` is an IEEE 754 binary64 number [@ieee754] — the `double` of C and
Java — with one deliberate difference: an Okkama float is always *finite*. An
operation whose result would be infinite or not a number is a runtime failure
("Float value must contain a finite number"), not a special value that
spreads through later computations.

**Why floats at all.** Binary floating point is the right tool for
measurements: physics, geometry, statistics, signal processing, where inputs
are approximate anyway and speed matters. Hardware does it in one
instruction. Goldberg's survey explains its rules and its traps
[@goldberg1991]: `0.1 + 0.2` is `0.30000000000000004` because neither `0.1` nor
`0.2` is representable in binary; addition is not associative; subtracting
nearly equal numbers loses precision. Okkama keeps floats for the problems
that want them and makes them a separate category so they cannot leak into
counting and money by accident.

**Why finite.** IEEE 754 defines infinities and NaN (not-a-number) so that a
long computation can continue past an exceptional operation and the program
can check the result at the end. William Kahan, the standard's principal
architect, defends this design: stopping at the first exception is often
worse than producing ∞ or NaN and letting the program decide [@kahan1996].
Okkama takes the other side of this argument deliberately. NaN breaks
equality (NaN ≠ NaN), breaks ordering, and propagates silently into places
far from its cause; infinities turn into NaNs at the next subtraction. In a
language whose rules say that values are what they look like and failures are
reported where they happen, a value that is not equal to itself has no
place. A failure at the operation that overflowed points at the cause.

This is a trade-off, not a free win. Numerical code that relies on IEEE
special values — for example, using ∞ as an initial minimum — must be written
differently, and a single overflow stops the computation instead of letting
the program inspect a partial result. Such code can check magnitudes before
the operation; Chapter 28 discusses whether an explicit non-finite type should
exist.

**Literals.** A float literal has a fraction, an exponent or both: `3.14`,
`6.022e23`, `1.6e-19`, `2E+10`. An exponent always makes a `float` — the
exact categories have no scientific notation, because `1e3` as an `int` would
hide the difference between a count and a measurement. The literal obeys the
same rule as arithmetic: `1e400` is a compile error rather than an infinity,
and so is `1e-400`, a nonzero number that would silently become `0.0`.

## 5.6 Bytes

`byte` is an octet, `0..255`: the unit of binary data — file contents,
network packets, encoded text. It is a separate category, not a small `int`:
`byte(256)` fails rather than wrapping, `byte(255.9)` is 255 (truncation, then
the range check), and arithmetic on bytes produces `int`. Binary data is
`@byte`, an immutable array of octets; Chapter 6 shows how text and bytes meet.

## 5.7 Conversions

Every conversion between categories is written. The table lists them.

| From → to | Spelling | Meaning |
|---|---|---|
| `int` → `float` | `x.float()` or `float(x)` | nearest binary64; may lose precision above 2⁵³ |
| `float` → `int` | `int(x)` or `x.int()` | truncation toward zero |
| `int` → `rat` | `x.rat()` | exact |
| `rat` → `int` | `int(x)` or `x.int()` | truncation toward zero |
| `rat` → `float` | `x.float()` | nearest binary64 |
| `float` → `rat` | `x.rat()` | the float's shortest decimal value, as a fraction (`0.125.rat()` is `1/8`) |
| any → `byte` | `byte(x)` | truncation, then a check of `0..255` |
| number → text | `x.string()` | canonical text: `1/3`, `0.30000000000000004`, `42` |

Two choices in the table deserve a word. `float.rat()` converts the float's
shortest decimal spelling, not its exact binary value: `0.1.rat()` is
`1/10`, not the 55-digit fraction that `0.1` really stores, because the
decimal is what the programmer wrote and meant. And float-to-text produces
the shortest string that reads back as the same float, so printing never
invents digits.

## 5.8 Numbers from text

Numbers that come from outside, a file, a form or a command line, arrive as
text. The standard library's `strings/parse` reads them strictly:

{{example:05-parse}}

- **Asking for a type.** `Number(text)` holds the text and reads nothing until
  it is asked for a type: `int()`, `rat()`, `float()` or `byte()`. That way
  the same text can be read as different categories, and the choice is
  visible where it is made.
- **Bases.** `int(base)` reads bases 2 to 36. Base 0 reads the prefixes `0b`,
  `0o` and `0x`. Integers of any size are read exactly.
- **Strictness.** Nothing is trimmed, a leading `+` is not accepted, and
  digit separators such as `1_000` are not either. `" 17"` is not a number:
  the space is in the text, and a program that wants to accept it trims it
  first, in code that says so.
- **Errors.** A text that is not a number of the asked type throws
  `InvalidInteger`, `InvalidRational`, `InvalidFloat` or `InvalidByte`, each
  carrying the exact source text. They are ordinary errors a caller catches
  (Chapter 10), unlike a division by zero, which is a bug.

This is the opposite of the forgiving parsers of many scripting languages,
which read the longest numeric prefix and ignore the rest. A forgiving parser
turns `"12 apples"` into 12 and a typo into a wrong value; a strict one turns
both into an error at the boundary, where the input can still be rejected.

## 5.9 Numbers in other languages

Every mainstream language answers the same five questions about numbers.
Their answers differ more than their syntax does:

| | Integer overflow | `int / int` | `%` sign follows | Float ∞ and NaN | Mixed `int` and `float` |
|---|---|---|---|---|---|
| C | undefined behaviour | truncates | dividend | yes | converted implicitly |
| Java | wraps | truncates | dividend | yes | converted implicitly |
| Go | wraps | truncates | dividend | yes | compile error |
| Rust | panics in debug, wraps in release | truncates | dividend | yes | compile error |
| Python | none (arbitrary size) | float (`//` floors) | divisor | yes | converted implicitly |
| Haskell | `Int` wraps, `Integer` none | not defined (`div` floors) | `mod`: divisor, `rem`: dividend | yes | compile error |
| Scheme | none (exact integers) | exact rational | `modulo`: divisor, `remainder`: dividend | `+inf.0`, `+nan.0` | exact becomes inexact |
| Okkama | none | exact `rat` | dividend | runtime failure | compile error |

Okkama is closest to Scheme in what its numbers are: exact integers and
rationals, with floats as a separate, approximate category [@r7rs2013]. It
is closest to Go, Rust and Haskell in refusing to mix categories silently.
And it differs from all of them in keeping floats finite (Section 5.5). The
remainder follows the dividend, as in C, Java, Go and Rust. That is the
remainder of truncating division, which is the division `int(a / b)` writes.

## 5.10 A worked example: drift and exact money

The choice between `rat` and `float` is easiest to see when both compute the
same thing.

{{example:05-drift}}

- **Ten dimes.** Adding `0.1` ten times in binary floating point gives
  `0.9999999999999999`, because one tenth has no finite binary expansion and
  every addition rounds. The same sum in `rat` is exactly `1`. Neither
  result is a bug: `float` is fast and approximate by definition, `rat` is
  exact by definition, and the program chose.
- **Comparing floats.** A `float` result is compared with a tolerance,
  `(x - 1.0).abs() < 1e-9`, never with `==`. An exact result needs no
  tolerance, which is why money, ratios and anything a person will check by
  hand belong in `rat` or `int`.
- **Interest.** Twelve months at one-twelfth of a percent are computed
  exactly in `rat` and converted to `float` once, for display:
  `1010.045960887182`. Rounding happens at the edge, where a person reads
  the number, not twelve times inside the computation.
- **Integers grow.** `(2).pow(70)` is the exact 22-digit integer; there is
  no overflow to guard against (§5.2).

The cost of exactness is speed and size: a long chain of `rat` operations
can grow large denominators, and a numerical simulation with millions of
steps belongs in `float`. The language makes the choice explicit in the
type, so it is made once, where the number is created, not discovered later
in a rounding difference.

## 5.11 Limits

- **No non-finite floats.** Code that relies on IEEE infinities and NaN must
  be restructured (Section 5.4).
- **Decimal arithmetic lives in the library.** The standard library's
  `Decimal`, used for JSON numbers, adds, subtracts and multiplies exactly
  and divides and rounds to a scale (issue #191); the library's
  string-backed `BigInt` still duplicates the language's own
  arbitrary-precision `int` (issues #189, #190).
- **Elementary functions.** Besides `abs`, `sign`, `pow` and `sqrt`, a
  `float` has `floor`, `ceil`, `round` (half away from zero), `trunc`,
  `exp`, `ln`, `log2`, `log10`, `sin`, `cos`, `tan` and `atan2`, and `int`
  and `float` have `min`, `max` and `clamp` (issue #192). A result that is
  not finite, such as `0.0.ln()`, is an error, as for every float.

The first two are small and fixable; the third is a stance; the last two
were gaps the standard library audit found, and 0.5.5 closes most of them.
