# Chapter 3 — archived implementation passages

Replaced mixed paragraphs are preserved in full for context.

## 3.3 Rule three: change is a new object

described 0.5.x

**Cost.** Performance and vocabulary. A new object per change costs an
allocation; Sparrow makes small allocations cheap, and the compiler
removes many, but arrays and maps without efficient persistent updates make
some idioms quadratic today (issues #145 and #151). And every state a program
cares about must be named and returned, which makes long chains of updates
more verbose than assignments.

## 3.6 Rule six: numbers are exact, and categories never mix silently

described 0.5.x

**Cost.** Speed and ceremony. Exact integers need an overflow check on every
operation (Sparrow makes it one branch); rationals are slower than
floats; and the explicit casts that Chapter 2's decorator needed —
`int(amount * percent / 100)` — are characters other languages do not ask
for. Chapter 5 measures the cost.

## 3.9 Rule nine: resources are released deterministically

described 0.5.x

**Why.** Stroustrup's RAII — resource acquisition is initialisation — made
object lifetime the lifetime of the resource it owns, so that release cannot
be forgotten [@stroustrup1994]. Garbage-collected languages lost this,
because objects die whenever the collector decides, and they compensated with
`try/finally`, `using` and `with` blocks that programmers must remember to
write. Okkama has both a precise garbage collector for memory and
deterministic destruction for resources. Its ownership regions are related to
Tofte and Talpin's region-based memory management, in which the lifetimes of
groups of objects are tied to the program's block structure [@tofte1997].
