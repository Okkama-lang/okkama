# Chapter 16. Enums and pattern matching

Some values come from a short list that is known in advance: a priority is
low, normal or urgent; an order is a draft, paid, shipped or cancelled. Some of
those alternatives carry data of their own. A paid order has an amount, a
shipped one has a carrier and a tracking number, and a draft has nothing.
Okkama covers both with one declaration, `enum`, and takes them apart with
`match`, in blocks or in compact one-line cases. The compiler knows every case, so it can
tell when code forgets one.

{{example:16-enums}}

- `Priority` is a **payload-free enum**: three names and nothing else.
- `Stage` is a **variant enum**: some of its cases carry typed fields.
- `match stage` has one branch per case and binds each case's fields to
  names. Drop the `cancelled` branch and the program no longer compiles.
- `match priority { case Priority.urgent: "!"; case _: " "; }` is the one-line
  form: compact cases, and `_` for everything else.

This chapter covers enums of both kinds and the rules of matching, and then
asks the design question that enums share with roles: when should a set of
alternatives be closed, and when open?

## 16.1 A closed set of names

```okkama
enum Priority = (low, normal, urgent);
```

An enum declaration names a type and lists its cases. The type name is
capitalised, like every type; the cases are lower-case identifiers
(`enum Status = (Ready, failed)` is a syntax error: `Expect enum case
name`). A case is always written with its type, `Priority.urgent`. There is no
bare `urgent` in scope, so two enums can share a case name without conflict. A
duplicate case is an error: `Enum "Status" already has case "ready"`. A long
enum is written one case per line, and, as in any list of parameters,
arguments or literals, a trailing comma after the last case is allowed.

What an Okkama enum value *does not* have is as important as what it has:

- **No number.** A case has no ordinal you can read, and there is no
  conversion to or from `int`.
- **No order.** `Priority.low < Priority.urgent` is rejected:

{{example:16-order-rejected}}

- **No actions.** An enum value is not an object. `Priority.low.string()` is
  an error, `Can only call actions on objects`, and there is no `values()` that
  lists the cases.
- **No mixing.** Two cases are equal only if they belong to the same enum and
  are the same case. `Status.ready == Other.ready` is not `false`, it is a
  compile error, `Cannot compare Status and Other`, like any comparison across
  categories (Chapter 4).

What it does have is equality, and the right to be a map key and an array
element. `[Unit.celsius: 2, Unit.kelvin: 1]` is a `%Unit:int`, and `@Status`
is an array of statuses.

Each refusal has a history. In C an enumerator is an `int` constant. It can be
added, compared, stored in any integer variable and printed as a number
[@kernighan1988], so an enum gives a name to a number without giving it a
type. Pascal made the enumerated type a type of its own, but an *ordinal* one:
its values are ordered, `succ` and `pred` step through them, and they index
arrays and drive `for` loops [@wirth1971]. Java turned each case into an
object with methods, fields, an `ordinal()` and a generated `values()`
[@gosling2023]. Bloch's advice in *Effective Java* is to use enums instead of
`int` constants and never derive anything from `ordinal()`
[@bloch2018]. Go has no enum declaration at all: a block of constants with
`iota` numbers them [@gospec].

Okkama takes the type from Pascal and leaves out the arithmetic. An
ordinal invites code that depends on the order of a declaration: `if level >
Priority.normal` silently changes meaning when someone inserts `high`
between `normal` and `urgent`. If a domain does have an order, the order is a
fact about the domain, and the program states it: a `match` that maps each
case to a weight, or an object that does `Comparing` (Chapter 14). The same
goes for text. The word printed for a case is a presentation decision. It
depends on the audience and the language. A `string()` on the enum would pick
one silently. A `match` makes the choice where it is used.

Declaration order is visible in maps: enum keys sort by their position in the declaration:

```okkama
enum Unit = (celsius, kelvin, fahrenheit);
let units = [Unit.fahrenheit: 3, Unit.kelvin: 2, Unit.celsius: 1];
// units.keys(): celsius, kelvin, fahrenheit
```

That gives iteration a stable order without making the order an operator.

## 16.2 Cases that carry data

```okkama
enum Stage = (draft, paid(amount is int), shipped(carrier is string, tracking is string),
    cancelled(reason is string));
```

A case may declare fields. The enum is then a **variant**, a *tagged union*,
or what the ML family calls an algebraic data type. A value of `Stage` is
exactly one of four things, and the case decides which fields exist:

- `Stage.paid(4250)` builds a value. Payload fields are given positionally,
  in declaration order. `Stage.paid(amount: 4250)` is rejected: `Variant
  Stage.paid expects 1 positional payload field(s)`.
- Payload fields are immutable and have no accessors. `stage.amount` does not
  exist, because on a `draft` there would be nothing to return. The only way to
  read a field is a pattern that has first established the case.
- Equality compares the enum, the case and the payload, field by field.

A variant may be generic:

{{example:16-variants}}

`Reading.single(21)` infers `Reading<int>` from its argument. A case without
fields has nothing to infer from, so without an expected type it needs explicit
arguments: `Reading<int>.missing`. Writing `Reading.missing` alone is
rejected: `Cannot infer type arguments for Reading.missing; provide an expected
type or explicit type arguments`. In the `Sensor` constructor the
field type supplies the context.

Two enums of the standard prelude are variants of exactly this kind:
`Maybe<T> = (some(value), none)` and `Presence<T>`, which adds `missing`
(Chapter 7). Error enums declared `is Error` are variants too, even when no
case has a payload, and a `catch` takes them apart with the same patterns
(Chapter 10).

Pascal's variant records gave the idea wide use in an unsafe form
[@wirth1971]. The tag was an ordinary field, and the language did not check
that a program read the fields of the variant the tag named. HOPE combined
data types defined by cases with functions defined by patterns over them
[@burstall1980], and ML made the pair standard, with the compiler checking
the patterns [@milner1997]. Haskell kept it [@haskell2010], and Rust brought
it to systems programming as `enum` with `match` [@klabnik2023]. Java reached
it in steps: records, then sealed interfaces that close a hierarchy, then
`switch` with patterns that checks the hierarchy is covered
[@jep395; @jep409; @jep441].

Okkama's variant is the ML one with the field names of a record. Its fields
are named in the declaration and positional at use, and the compiler checks
both arity and types.

## 16.3 `match`

```okkama
let state = match stage {
    case Stage.draft { "draft"; }
    case Stage.paid(amount) { "paid " + amount.string(); }
    case Stage.shipped(carrier, tracking) { "shipped by " + carrier + ", " + tracking; }
    case Stage.cancelled(reason) { "cancelled: " + reason; }
};
```

The subject is evaluated once. Each `case` lists one or more patterns, and the
first case with a matching pattern runs. There is no fall-through and no
`break`; a branch is a block, and its last value is the value of the `match`.
`else` takes whatever no case matched.

Patterns are of two kinds:

- **A value.** `case Priority.low, Priority.normal`, `case 404`, `case "GET"`.
  The subject is compared with `==`, so the rules of equality apply: the value
  must be of the subject's category, and `case 1..5` against an `int` is
  rejected rather than read as a range test (Chapter 9).
- **A variant pattern.** `case Stage.shipped(carrier, tracking)` tests the
  case and binds each field to a name, or ignores it with `_`. The number of
  bindings must equal the number of fields: `Wrong number of variant pattern
  bindings`. A binding exists only inside its branch.

All branches of a `match` that produces a value must produce one type:

```text
semantic error: Branches produce different types: int and string
```

A branch that returns, throws or panics produces no value and does not take
part in the check.

### Exhaustiveness

The compiler knows every case of an enum, so it checks that a `match` covers
them:

{{example:16-missing-rejected}}

The rule depends on the kind of enum:

- A `match` over a **variant enum** — one with a payload anywhere, an error
  enum, `Maybe`, `Presence` — must cover every case or have `else`. That holds
  for a statement `match` as well as for one that produces a value.
- A `match` over a **payload-free enum** that produces a value must cover
  every case or have `else`. Otherwise there would be no value to produce:

{{example:16-value-rejected}}

The same message names the cases when the `match` is the last value of an
action. Until 0.5.0 that error named only the action ("must produce a return
value", issue #291):

{{example:16-tail-rejected}}

- A *statement* `match` over a payload-free enum may leave cases out; it then
  does nothing for them. `okkama lint` reports it:

```text
warning[non-exhaustive-match]: match over enum `Status` does not handle failed, done; add the cases or an `else` branch
```

The second and third rules differ because a payload-free enum used to be
treated as a set of constants, and a statement over constants may
legitimately act on some of them only. The lint keeps the old freedom visible.
Variants never had it. A missing case of a variant is a missing piece of data
handling, and the compiler treats it as such.

Exhaustiveness pays off most when the enum changes. Add `refunded(amount is
int)` to `Stage`, and every `match` over a stage that lists cases becomes an
error that names `refunded`. The list of places to update comes from the
compiler, not from a search. This is the reason to prefer listing cases over
`else`. An `else` covers today's remaining cases and every future one, and so
it hides exactly the places that a new case should reach. Maranget describes
the two checks a compiler can make on a match: *exhaustiveness*, whether some
value is matched by no case, and *usefulness*, whether a case can never be
reached [@maranget2007]. Okkama implements both for the cases it can compare:
a repeated case is an error (`Duplicate variant pattern`), so is a repeated
literal (`Duplicate case pattern`), and so is an `else` after cases that already
cover every value (`Unreachable else`). Until 0.6.0 only the first of those was
reported (issue #286).

## 16.4 Compact cases

A case that only computes a value fits on one line: `case pattern: value;`.
With it, a whole `match` is one expression:

```okkama
let suffix = match unit { case Unit.celsius: " C"; case Unit.kelvin: " K"; };
let flag = match priority { case Priority.urgent: "!"; case _: " "; };
let label = match reading { case Reading.single(value): value.string(); case _: "no single value"; };
```

- A compact case is a pattern, a colon, the value and a semicolon. Block cases
  and compact cases mix in one `match`.
- `case _` covers the cases not named; it is another spelling of `else`.
- Patterns are written in full (`Priority.urgent`), as in any `match`, and the
  rules are the same: coverage, repeated cases, one result type.

A `match` that produces a value and misses a case names it: ``match over enum
`Priority` does not cover normal, so it has no value here; add the cases or an
`else` branch``. Until 0.6.0 the same choices had a notation of their own,
`priority ?? .urgent: "!" !! _: " "`; it was removed in favour of compact cases
(issue #893), so that one construct does one job.

## 16.5 Closed or open: enums and roles

An enum and a role are two answers to one question: how to describe a value
that can be one of several kinds.

- An **enum** lists the kinds in one place. Code that uses it switches on the
  kind. Adding a new *operation* over stages is easy: write one more function
  with a `match`, and nothing else changes. Adding a new *kind* is harder:
  every `match` must learn it, and the compiler lists them.
- A **role** leaves the kinds open. Each kind is an object that does the role
  and carries its own behaviour. Adding a new *kind* is easy: write one more
  object, and nothing else changes. Adding a new *operation* is harder: the
  role gains an action, and every object must implement it.

Reynolds described this duality in 1975, and Cook revisits it as the
difference between abstract data types and objects [@cook2009]. Wadler named it the
*expression problem*: define a data type by cases so that both new cases and
new functions can be added without recompiling existing code and without
losing static type safety [@wadler1998]. Okkama does not solve the expression
problem; very few languages do without extra machinery. It offers both sides
and asks the programmer to choose by what is expected to change:

| The set of kinds is… | …and the operations are… | Use |
|---|---|---|
| closed, part of the domain's definition (order states, token kinds, protocol messages) | many, added over time (rendering, pricing, persistence) | an enum, with `match` |
| open, extended by other modules or by tests (storage backends, transports, clocks) | few and stable | a role, with objects |

The test is whether a newcomer must be written into one list or may be added
from outside. A stage of an order belongs to the order's definition. A new
stage is a change to the business, and every place that handles stages should
be reviewed. A storage backend does not belong to the service that uses it,
and a fake in a test is one more backend (Chapter 15). A role cannot be
matched on, because there is no downcast (Chapter 14). An enum cannot be
extended, because its declaration is the complete list. Each property is what
makes the other choice worse.

Java's sealed interfaces sit in between: a closed list of classes, each with
its own methods, which a `switch` can also take apart [@jep409; @jep441].
Okkama has no single construct for this, but the effect is reachable: a
variant enum whose payloads are objects.

## 16.6 A worked example: a protocol and a state machine

Enums with payloads are the natural shape of a protocol. A vending machine
receives commands, has a state, and answers with replies; each of the three
is a closed set of cases, and each case carries exactly the data it needs.

{{example:16-machine}}

- **Messages are variants.** `Command.choose(item, price)` carries what a
  choice needs and nothing else; `Command.refund` carries nothing. There is
  no "command type" string to dispatch on and no optional fields that only
  some commands fill.
- **The state is a variant too.** `State.idle` and `State.credit(cents)` make
  an impossible state unrepresentable: there is no credit field to forget to
  reset when the machine goes idle.
- **Transitions are a `match` over the command.** `next` and `reply` each
  handle every case, and the compiler checks it: adding a case
  `Command.service` stops the build with "Non-exhaustive match: missing
  service" at every `match` that must learn about it.
- **The machine is a value.** `step` returns a new machine; the run is a
  `reduce` over the commands, so the whole history is a fold and any
  intermediate machine could be kept and replayed.

This is the shape that object-oriented designs reach with the State pattern
and that functional languages write as algebraic data types. The trade-off
of §16.5 applies: a new kind of message touches every `match`, which is
exactly the list of places that must decide what the message means.

## 16.7 Where enums came from

Okkama's enum joins two lines of descent.

- **Named values.** Pascal gave programmers enumerated types: a closed set
  of names such as `(red, green, blue)`, with an order and the operations
  `succ` and `pred` [@wirth1971]. Its variant records attached data to a
  tag, but the compiler did not check that the tag matched the data read.
  C's enumerations went the other way and are names for integers.
- **Cases that carry data.** HOPE, designed by Burstall, MacQueen and
  Sannella, declared a type by its constructors, each with its own fields,
  and defined functions by pattern matching on them [@burstall1980]. ML
  adopted these *datatypes* [@milner1997], and through ML they reached
  Haskell, Rust and Swift as algebraic data types.
- **Checked coverage.** With a closed set of constructors, a compiler can
  tell whether a `match` covers every case and name the one it misses.
  Maranget describes how compilers do it and how to report the missing
  pattern usefully [@maranget2007].

Okkama takes the closed set from Pascal without the order: cases have no
ordinal and no `<` (§16.1). It takes payloads from HOPE and ML, and checked
coverage for every `match` that produces a value. It does not yet take
recursive datatypes (#287) or nested patterns (#288), the two features
that make algebraic data types the natural way to write trees and
interpreters in ML and Haskell.

## 16.8 Enums elsewhere

| | Okkama | C | Pascal | Java | Rust | Haskell | Go |
|---|---|---|---|---|---|---|---|
| Case is a number | no | yes (`int`) | ordinal | `ordinal()` | `as` cast for payload-free enums | `Enum` class, if derived | yes (`iota`) |
| Ordering operators | no | yes | yes | `compareTo` | if derived | if derived | yes |
| Data per case | variants | no (unions are separate and untagged) | variant records, unchecked tag | sealed classes and records | yes | yes | no |
| Exhaustiveness checked | yes; lint only for statement matches over payload-free enums | no | no | `switch` expressions and pattern `switch` | yes | warning (`-Wincomplete-patterns`) | no |
| Recursive data | no (#287) | through pointers | through pointers | yes | through `Box` | yes | — |
| Nested patterns | no (#288) | — | — | yes | yes | yes | — |
| Actions on cases | no | — | — | yes | `impl` blocks | type classes | methods on the type |

Okkama's enum keeps the type safety of the Haskell and Rust column. It leaves
out what those languages build on top of it: nested patterns, recursion,
derived orderings and methods. The first two are missing for now. The last
two are left out on purpose, for the reasons in §16.1.

## 16.9 Limits

- **No recursive variants.** A case cannot carry its own enum, directly or
  indirectly:

{{example:16-recursive-rejected}}

  Trees, lists and expression syntax trees, the classic uses of algebraic
  data types since HOPE, have to be built from objects for now (issue #287).
- **No nested patterns.** `case Pair.both(Maybe.some(x), y)` is rejected:
  `Expected a binding or _ in variant pattern`. A second `match` on the bound
  field does the same job in more lines (issue #288).
- **Variants are not map keys.** A payload-free enum is a key; a variant is
  not: `Map key type Reading is not a scalar key type` (issue #289).

## 16.10 What this design costs

- **Text and order are written out.** Printing a case or sorting by
  priority takes a `match`. A language with `toString` and
  `ordinal` would do it implicitly. The cost is a few lines per enum, paid once
  and kept in one place.
- **No list of cases.** There is no `values()`. Code that needs every case,
  for example to fill a menu, writes the array, and nothing checks that the
  array is complete.
- **Positional payloads.** Fields are named in the declaration, but a
  construction like `Stage.shipped("DHL", "JD0142")` does not show which
  string is the carrier. A case with more than two or three fields is often
  better as an object carried by a one-field case.
- **The closed world.** An enum cannot be extended from another module.
  That is what makes exhaustiveness possible, and it is why an open family of
  kinds should be a role.

In return:

- a forgotten case is a compile error, not a run-time surprise;
- a value cannot be read through the wrong case;
- equality is exact, and two enums can never be compared by accident;
