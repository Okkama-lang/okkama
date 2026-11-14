# Chapter 5 — archived implementation passages

Replaced mixed paragraphs are preserved in full for context.

## 5.2 Exact integers

described 0.5.x

The algorithms behind big-integer arithmetic — schoolbook and Karatsuba
multiplication, long division, conversion to decimal — are the classical ones
of Knuth's *Seminumerical Algorithms* [@knuth1997]. A product whose result
cannot fit in the run's memory budget is refused before it is computed, so a
program cannot exhaust memory with one multiplication (Chapter 26). Powers are
exact too: `(2).pow(100)` is `1267650600228229401496703205376`; an `int`
cannot be raised to a negative power — that result is not an integer — while
`rat.pow` takes one and inverts (issue #228).

**Cost.** Each integer operation must check for overflow. On Sparrow
this is one compare-and-branch after the machine instruction, taken almost
never; Figure 5.1 shows that integer-heavy code is still several times faster
than in the dynamic languages Okkama is usually compared with. Where results
do become big, they cost what big integers cost in any language.

## 5.7 What exactness costs

described 0.5.x

## 5.7 What exactness costs

Figure 5.1 compares three numeric benchmarks of the Okkama benchmark suite —
integer arithmetic, floating-point arithmetic and big-integer arithmetic — on
Sparrow, on the JVM backend, and in Python, Ruby and Perl, measured
for the 0.4.0 release.

{{figure:numbers-cost|Figure 5.1. Wall-clock time of three numeric benchmarks (logarithmic scale). Data: data/benchmarks-0.4.0.json.}}

On Sparrow, exact integer arithmetic takes 0.043 s where Python
takes 0.52 s and Ruby 0.19 s: the overflow check costs little next to
dynamic dispatch. Floating point is fastest (0.026 s), as expected of one
machine instruction per operation. Big integers cost what big integers cost —
0.23 s on Sparrow against 0.36 s in Python and 0.40 s in Ruby, which use
mature C libraries; Perl's pure-Perl big integers take 24 s. The lesson of the
figure is not that Okkama is the fastest — it is not, against C or Rust — but
that making the default numbers exact did not make them slow.

## 5.2 Exact integers

described 0.5.x

An Okkama `int` behaves as a mathematical integer. Internally the
implementations keep small values in a machine word and switch to an
arbitrary-precision representation when a result does not fit; the program
cannot observe the switch except by the time it takes.

## 5.8 Limits

described 0.5.x

## 5.8 Limits
