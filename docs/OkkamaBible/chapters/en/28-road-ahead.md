# Chapter 28. Weaknesses, Open Problems and the Road Ahead

Every chapter of this book ends by counting what its design costs and what
does not work yet. This chapter gathers those counts in one place and sorts
them by kind, because the kinds call for different answers:

- what was missing and has since been added, which shows how the razor's
  gaps are closed;
- what the language intends to have and does not have yet;
- what the standard library and the tools still lack;
- open questions, where both answers have a case and none is decided;
- costs accepted on purpose, which are not on any road to change.

Each item appears in one group only. Items with an issue are cited by its
number, so a reader can check the state after November 2026, when this
chapter was written; items without one point to the section that argues
them.

## 28.1 Closed while this book was written

Chapter 1 admitted that some removals were made before their replacements
were complete, and that the standard library audit of November 2026 had
asked for the operations a smaller design needs. Most of those requests were
answered within days, in 0.4.1 and 0.4.2:

| Issue | What was missing | What the language has now |
|---|---|---|
| #210 | searching, trimming and case in strings | builtins (Section 6.4) |
| #211 | searching and reshaping arrays | `contains`, `index_of`, `reversed`, `take`, `drop` (Section 8.1) |
| #231 | turning a `char` into a number or a string, and back | `code()`, `char(n)`, `string()` (Section 6.7) |
| #145, #151 | appending to arrays, updating maps | `+` and `-` on collections (Section 8.5) |
| #150 | searches that stop at the first decisive element | `any`, `all`, `find` (Section 8.2) |
| #149 | default parameter values | trailing parameters with defaults (Section 13.2) |
| #152 | a mutable value for synchronous code | `Cell` (Section 13.11) |
| #153 | named compile-time values | `const` (Section 19.6) |
| #154 | releasing a resource that must wait | `async destroy` (Section 11.8) |
| #147 | cancelling the future a task awaits | structured await (Section 20.5) |
| #214 | result types on anonymous actions | `(n is int) is int { … }` (Section 4.5) |
| #215 | operators on library number types | operator roles (Section 14.7) |
| #216 | asserting that a block throws | `test.throws` (Section 15.2) |
| #233 | a silent requirement of `ordering()` | the diagnostic names the import (Section 8.1) |
| #256–#258 | diagnostics for `while`, `+=` and `break`; `else if`; postfix `return` | Section 9.10 |

Two requests were declined, with reasons that are part of the design:
default bodies in roles (#146, Section 1.9) and chainable actions for roles
(#213), where the owner's answer was that lazy views keep composition
explicit. The table is the razor's procedure in action: a missing operation
is added as an operation, not by bringing back the entity whose removal made
it necessary.

## 28.2 Missing from the language

Four open issues describe what the language plans to have and lacks. All
four are about enums and patterns, and together they are the largest
difference between Okkama's enums and the algebraic data types of ML, Rust
or Haskell (Section 16.7).

- **Recursive variants (#287).** A case cannot carry a value of its own
  enum, so a tree or an expression type cannot be declared directly. Today
  such a structure is an object graph built from roles, or an array of nodes
  that refer to each other by index.
- **Nested patterns (#288).** `case Pair.both(Maybe.some(x), y)` is
  rejected; the inner value is taken apart by a second `match` in the body.
- **Variants as map keys (#289).** A payload-free enum can key a map; a
  variant cannot, even when its payloads could.
- **Repeated cases pass silently (#286).** A `match` that repeats a
  payload-free case or a literal compiles. The repetition is dead code, and
  the compiler should say so.

The first two are the ones that matter for programs. Interpreters, parsers
and compilers are the classic programs written with recursive types and
nested patterns, and in Okkama they are written today with more objects and
more `match`es than in the languages Okkama compares itself with.

## 28.3 The library and the tools

The standard library was written quickly, before several builtins existed,
and it is to be rewritten in the style Chapter 22 describes. Most of its
open items wait for that rewrite.

- **Workarounds for missing builtins (#260).** About forty modules still
  search strings, compare prefixes and walk characters by slicing, as they
  had to before #210, #211 and #231.
- **Locale-sensitive case (#261).** `lower()` and `upper()` follow the
  Unicode default. Turkish, Azeri and Lithuanian rules need a locale, and
  the library has none yet.
- **Asynchronous decorators (#131)** and **cleanup guards that are
  `async destroy` objects (#329).** Some of the library's I/O decorators
  are synchronous only, and its cleanup guards still close through
  `defer await`.
- **Absence is not reported uniformly.** Some modules return a `Maybe`,
  some throw a typed error for a missing value, some take a fallback
  (Section 7.14). The json module's reading style is the reference the
  library is being aligned with.
- **The test runner (#339).** `okkama run -t` has no report formats, seeds,
  parallel workers or deadlines yet; the library can already render reports
  (Section 15.1).
- **Packages (Section 19.11).** blade has no registry and no workspaces,
  and a program cannot depend on two major versions of one package.
- **Authority below the native groups (Section 21.8).** A host can deny a
  group of natives, but not one call; finer, per-object capabilities for
  the library are future work.

## 28.4 Open questions

These are designs that have been considered and not decided. Each has a
case on both sides, and the razor does not settle it by itself: it asks what
work the new entity would do that nothing present can.

**A shorthand for data objects.** Chapter 3 conceded that some objects are
just data: a point, a date, a row from a database. For them, an action per
field is boilerplate, and a declaration that generated the constructor and
the accessors would remove it. Against it stands the rule it would bend.
The manifesto makes "an object answers questions about itself" the default
and "an object hands out its fields" a decision written in the object; a
record syntax would turn that around for every object declared with it, and
*Elegant Objects* argues that such objects invite logic to live outside them
[@bugayenko2016]. A shorthand that expands to an ordinary object, with
nothing the reader cannot see in the expansion, adds no entity; whether it
adds a habit is the open part.

**Narrowing `any`.** An `any` is opaque: code can store it and pass it on,
and only the native layer opens it (Section 4.6). Generic transports, such
as log records with fields of mixed types and database rows, would like to
look inside. A narrowing construct, a `match` over the type of the boxed
value, would let them. It would also be a downcast, which Section 14.3 says
the language never has, because a downcast turns a contract back into its
implementation. One middle position narrows only to scalar types and to
collections of them, which carry no contract to betray. It is not decided.

**A non-finite number type.** Okkama's `float` has no infinities and no
NaN, so an overflow is an error at the operation that caused it (Section
5.5). Numerical code that uses ∞ as an initial minimum, or NaN as "no
value", must be written differently. An explicit type for extended reals,
separate from `float`, would give such code its values without letting
them leak into code that assumes finite numbers. The case against is the
razor's: a fifth category of number, for a use that `Maybe` or an explicit
bound usually serves.

**Combinators on `Maybe`.** `map`, `and_then` and `filter` on a `Maybe`
shorten chains of optional steps. Each is also a second way to say what a
compact choice `?? !!` or a `match` already says (Section 7.14).

**Errors in signatures.** An Okkama signature does not list the errors an
action may throw (Section 10.19). Listing them is what CLU and Java's
checked exceptions did, and it brought churn through every layer that
merely passes an error on; not listing them leaves the reader to learn the
errors from documentation and source.

**Named catch handlers (#148).** A handler shared by several `try` blocks
must be an action called from each clause. Named handlers were proposed and
postponed because they add behaviour that is not visible at the `try`.

**Generic actions.** An algorithm generic in its inputs is written as a
generic object today (Section 17.11). Generic actions would remove that
object at the cost of a second place where type parameters live.

## 28.5 Costs accepted on purpose

The rest of the limits named in this book are not gaps. They are what the
language is, argued where they appear:

- forwarding by hand in decorators, and no default bodies in roles
  (Sections 2.12, 14.6);
- no retroactive conformance, and operators that look only to their left
  operand (Section 14.11);
- no downcast from a role, ever (Section 14.3);
- more objects and more allocation than in a language with mutation
  (Sections 12.13, 13.16);
- ceremony wherever a value may be absent (Section 7.13);
- no catch-all except `catch e is Error`, and errors that cannot carry
  objects (Section 10.19);
- a fake to write for every role a test replaces (Section 15.11);
- no list of an enum's cases and no order between them (Section 16.10);
- no module-level state, only compile-time constants (Section 19.6);
- a fixed cleanup order: newest first, owner before fields (Section 11.15).

A reader who finds one of these intolerable has found a real disagreement
with the language, not a bug in it. The book has tried to state each one
plainly enough that the disagreement can be had before the first program,
not after the thousandth.

## 28.6 The road ahead

This book does not predict versions. The road is the list above: the
issues in Sections 28.2 and 28.3, each to be closed or declined with a
reason, and the questions in Section 28.4, each to be answered by the
procedure Section 1.9 showed. Underneath the language its implementation is
being rebuilt, and that work changes how fast programs run, not what they
mean; it belongs to another book.

The measure of progress is the one this book has used throughout. A rule
counts when the compiler or the runtime enforces it, a claim counts when an
example runs, and a gap counts as closed when the issue that names it is.
