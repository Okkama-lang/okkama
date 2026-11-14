# Chapter 26. Patterns and anti-patterns

Every language comes with a second grammar that no parser checks: the shapes
experienced programmers reach for when a problem has a familiar outline. Some
of those shapes exist because the language makes the direct solution
awkward; some exist because the direct solution is wrong and the shape is
how people learned to avoid it. This chapter collects the shapes Okkama
programs take, the ones it makes unnecessary, and the ones it forbids. Most
of them have appeared already, one chapter at a time; here they are put side
by side, with the reasons that connect them.

## 26.1 What a pattern is, and what a language does to it

The word comes from architecture. Christopher Alexander and his colleagues
described a *pattern* as a problem that occurs over and over in an
environment, together with the core of a solution that can be used a
million times without being done the same way twice, and they wrote 253 of
them, from the shape of a region down to the height of a window sill
[@alexander1977]. The idea crossed into software in 1994, when Gamma, Helm,
Johnson and Vlissides catalogued 23 patterns of object-oriented design:
Strategy, Decorator, Observer, Builder, Singleton and the rest
[@gamma1994]. The book gave a generation of programmers a shared vocabulary.

It also gave critics a target. Two years later Peter Norvig showed that 16 of
the 23 patterns become invisible or much simpler in a language with
first-class functions, dynamic types and macros: a Strategy is a function, a
Command is a closure, a Visitor is a generic function [@norvig1996]. His
point was not that the patterns were wrong but that a pattern is often the
shadow of a missing feature. Where the language has the feature, the
pattern disappears into it.

The opposite shadow has a name too. Andrew Koenig called an *antipattern* a
solution that looks like a pattern but makes things worse [@koenig1995], and
Brown and his co-authors built a catalogue of them, from the Blob to Lava
Flow [@brown1998]. An antipattern is a pattern a language allows and
should not.

Okkama sits at a particular point between the two. It removes several
features whose absence creates patterns elsewhere: there is no inheritance,
no null, no mutable field, no global state, no getter syntax. And it adds
rules that turn some well-known antipatterns into compile errors. The result
is a language in which a handful of shapes are not patterns at all but the
only way to write the program, and in which the remaining judgement is
narrower than in Java or Kotlin, though not absent.

## 26.2 Behaviour by composition, not by flags

A common way to make an object do several related things is to give it
switches. A label that can be prefixed and framed gets a `prefix` field and
a `framed` flag, and its one action reads them on every call. Adding a third
option means touching the constructor, the fields and every branch of the
action; combining options means testing every combination of flags.

Okkama has no inheritance to tempt a programmer into a subclass per option,
so the alternative the language offers is the one Elegant Objects argues
for: one small object per behaviour, each holding the object it decorates,
all doing the same role [@bugayenko2016]. Chapter 14 showed the technique on
prices; here it is applied to the flags it replaces.

{{example:26-flags}}

The program prints:

```text
[! draft]
[! draft]
! [draft]
```

The first line comes from `FlagLabel`, whose one action decides everything.
The second and third come from compositions of three objects that each do
one thing, and they differ only in the order of composition. That order is
a decision `FlagLabel` cannot express without another flag: it hard-codes
"prefix inside the frame". With decorators, both orders are available, and a
new behaviour, a `Truncated` or an `Upper`, is a new object of a few lines
that no existing object has to know about.

The decorators pay for this freedom with one indirection per layer and with
more declarations than the flag version. For a label with two options the
flag version is shorter, and nothing in the language forbids it. The rule of
thumb that Okkama code follows is about change, not about size: as soon as
an option interacts with another, or a second caller wants a different
combination, the flags become a small interpreter of their own, and the
composition is the cheaper program to keep.

### Composite: a container that does its parts' role

The same idea covers trees. When a container does the role of the things it
holds, a caller cannot tell one item from a bundle of them, and a bundle of
bundles needs no special case. Gamma and his co-authors called this
Composite [@gamma1994]; in Okkama it is a role, a leaf object and a
container object whose field is an array of the role.

{{example:26-composite}}

```text
tea: 300
cups of 2: 300
gift of 2: 540
```

`Bundle` sums its parts with a `Cell` inside one synchronous action, which is
exactly the local use cells are for, and applies its own discount on top.
The gift contains an item and a bundle and prices both the same way. Two
constructors give the discount a default without a flag that every bundle
must pass.

## 26.3 Change as a value: successor pipelines

Builders, setters and mutable "state" objects exist in other languages
because an object there can change in place. In Okkama it cannot. A field is
bound once, by the constructor; an action that changes state is declared
`is ^Type`, stages new field values with `^field = …`, and returns a new
object, the successor (Chapter 13). The old object stays as it was.

This one rule turns several patterns into ordinary code. A builder is a
chain of successor calls. An undo stack is a list of earlier values. A
snapshot before a risky step is a variable that is not touched. The next
example takes an order through four stages and keeps all of them.

{{example:26-pipeline}}

```text
0 lines, due 0, open
2 lines, due 500, open
2 lines, due 450, open
2 lines, due 450, closed
OrderError.closed
```

Each stage is a value. `empty`, `basket`, `offer` and `paid` can be printed,
compared, stored in a collection or passed to another task, and none of them
can be changed by a later step. The rule that a closed order accepts nothing
more is checked where the change is asked for, by `confirm` in the
successor, and the refusal is a typed error rather than a silent no-op.

Two details of the example are deliberate. The successor stages only the
fields it changes; the others carry over from the original. And the object
calls its own query through `self.due()`: inside an object, its actions are
reached through `self`, its fields by name.

The antipattern this replaces is the object with a "current state" that
every action updates, where the order of calls matters and nothing records
it. Okkama refuses it at the level of the language: assigning a field after
construction is a compile error, with a diagnostic that names the
alternative:

```text
Field "count" cannot be assigned after construction: objects are immutable; return a successor instead (`is ^Counter` and `^count = …`)
```

What the language does not refuse is a value that changes over time inside
one piece of synchronous code, such as a running total in a loop. For that
it has `Cell`, a box with `value()` and `replace()` (Chapter 13). A cell is a
local tool: it cannot be a field of an object, so it cannot become the
hidden mutable state of one.

```text
Field "reading" cannot hold a Cell: a cell stays in synchronous code; state an object shares with tasks is an Atomic
```

### Typestate: when an invalid step should not compile

A successor chain checks at run time that a step is allowed. When a protocol
has a few well-known states, the check can move into the types. Each state
becomes a role that exposes only the actions valid in it, and each transition
returns the role of the next state. A created payment offers `authorize`; an
authorized one offers `capture`; a captured one offers `refund`; and calling
`refund` on a created payment does not type-check, because the role
`Created` has no such action. The pattern is old: Strom and Yemini called
it *typestate* in 1986 [@strom1986]. In Okkama it needs nothing beyond roles and
ordinary actions.

{{example:26-typestate}}

```text
p-17 captured 250
p-17 refunded
```

Each object does exactly one role, and a variable typed by the role sees only
the steps of that state. The guarantee holds even when the object itself
has more actions than its role exposes. In the next program `NewPayment`
does have a `refunded` action, but a value typed `Created` cannot reach it:

{{example:26-typestate-rejected}}

```text
Role "Created" has no action "refunded"
```

Typestate costs one role and one object per state, and transitions that
build new objects rather than successors, because the next state is a
different type. It is worth it where a wrong step is expensive: payments,
connections, documents that move through approval. Where the states are many
and the transitions few, a successor with `confirm` checks (Section 26.3's
`Order`) is the lighter tool.

## 26.4 Refusals have names

A function that cannot do what was asked has three classic ways to say so: a
special value (`-1`, `""`, `null`), an untyped exception carrying a message,
or a typed failure the caller can match. Okkama keeps only the third. There
is no null to return, an object cannot be an error, and a failure that
carries information is a case of an `Error` enum (Chapter 10):

```text
Object "Failure" cannot do Error: errors are enums, declare `enum Failure is Error = (...)`
```

The pattern built on this rule has three parts. A module names all its
refusals in one enum, with the data that explains each in its payload. Every
precondition is checked at the boundary, where the bad input arrives, with
`confirm … else throw`. And a caller catches only the refusals it can
answer, letting everything else go up.

{{example:26-refusals}}

```text
30 -> balance 70
0 -> refused: Refusal.non_positive(0)
150 -> refused: Refusal.insufficient(100, 150)
```

The teller turns refusals into a reply because replying is its job. A
`Program` failure such as a broken invariant, or a refusal from some other
module, passes through the `catch` untouched, because the clause names
`Refusal` and nothing else. A `catch` can also name a single case and bind
its payload, `catch Refusal.insufficient(balance, amount) { … }`, which is
how a caller answers one refusal and leaves the rest to someone else.

Two antipatterns are on the other side of this rule. One is the catch-all:
catching every failure to keep a loop running, which also catches the
failures that mean the program is broken. Okkama makes it hard: a typed
`catch` names a type, and panics and program failures are not typed throws
at all, so they cannot be caught as one (Chapter 10). The other is the
refusal that is not a refusal: an action that returns a default when it
cannot do its work. When absence is a legitimate answer, the language has a
type for it (Section 26.7); when it is not, the action should refuse.

## 26.5 Every dependency is a role, every role has a fake

Objects in Okkama reach the outside world through the objects they are given:
there is no global state and no static field (Chapter 22). An object that
sends mail holds a mailer; an object that reads time holds a clock. When the
held object's type is a role rather than a concrete object, the dependency
can be replaced, and the replacement for tests comes with the role itself:
every role may declare a `fake`, a test-only implementation that the
compiler keeps out of production code (Chapter 15).

{{example:26-signup-tests}}

```text
=== RUN   every member gets one welcome mail
--- PASS: every member gets one welcome mail
=== RUN   the first signup is untouched by its successors
--- PASS: the first signup is untouched by its successors
PASS
```

The fake records what happened in its own fields, through successors, the
same way production objects do. A test does not configure expectations the
way a mock framework would; it uses the fake and then asks it what it saw.
This is the distinction Martin Fowler drew between state verification with
stubs and behaviour verification with mocks [@fowler2007], and the one
Meszaros's catalogue of test doubles calls a fake object: a working
implementation, simpler than the real one [@meszaros2007]. Because the fake
lives next to the role, every implementation of the role and every test of
its users agree on what the role means.

The example shows one more consequence of successors. Sending mail is a
state change of the mailer, so `sent` returns a successor, and `Signup`
stages it: `^mailer = mailer.sent(name)`. A production mailer that only
prints still returns a successor of its own type, `^PrintedMailer`; the
compiler insists on that, and its diagnostic says how:

```text
Successor action "sent" on object "PrintedMailer" must return ^PrintedMailer; declare ^Mailer in a role signature and implement it as ^PrintedMailer
```

The antipattern this replaces is the hidden collaborator: an object that
creates its own database connection, reads the clock directly or writes to
a global logger, and so can be tested only by replacing the world around
it. In Okkama such an object cannot exist for clocks and loggers, because
there is no world to reach into; it can exist for objects it constructs
itself, and the habit of the standard library is not to (Section 22.6).

## 26.6 Resources belong to scopes

A resource is an object with a `destroy` block. The scope that owns it
destroys it when the scope ends, whatever way it ends, in reverse order of
creation, after the scope's `defer` blocks have run (Chapter 11). The
pattern that follows is simple to state: open a resource where it is needed,
bind it with `let` in the narrowest block that uses it, and put its cleanup
in `destroy`, not in an action a caller must remember to call.

The antipatterns it replaces are familiar from languages with explicit
cleanup: the `close()` that is skipped on an early return, the `dispose()`
called twice, the object that is "opened" in a second step after
construction and is unusable in between. In Okkama none of them is
necessary. An object that needs a resource takes it in its constructor and
is complete when the constructor returns; the end of its scope is the only
cleanup point; and a result returned from an action carries its resources
to the caller's scope, so the caller owns what it was given (Chapter 11).

`defer` covers the cases that are actions rather than objects: restoring a
setting, writing a final line to a log, releasing something the program did
not create. It runs at the end of the scope, reads the bindings as they are
then, and runs before the scope's resources are destroyed. A rule of thumb
from code review: if the same `defer` appears around every use of an object,
the object wants a `destroy`.

## 26.7 Absence is a type

Okkama has no null. A value that may be missing has the type `Maybe<T>`, and
the type system makes the caller decide what absence means: a fallback with
`else`, a compact choice with `??`/`!!`, or a `match` (Chapter 7). The
pattern is to put absence in the signature of the action that can produce
it, and nowhere else. A lookup that can fail to find returns `Maybe`; a
lookup that must find refuses with a typed error.

The antipatterns on the other side are the sentinel value and the "empty
object": `-1` for "not found", `""` for "no name", an object whose every
field is a default. They survive in Okkama only if a programmer writes them,
since the language provides no null to tempt anyone and an explicit type for
the honest answer. When a value is optional for a while and then becomes
required, the change is visible in the types, and the compiler finds every
caller that has to decide.

## 26.8 Shared state has one owner

Asynchronous code needs state that more than one task can see. Okkama's
answer is structural: state shared between tasks lives in one task, the
owner, and others talk to it through channels (Chapter 20). A ledger that
several workers update is one object in one task, receiving messages; the
workers hold the sending end of a channel, not the ledger. For the few
values that really are shared, such as a counter or a flag, `Atomic` gives a
small set of operations that are safe across tasks. The ledger of
Section 20.6 is the pattern in full: an enum of commands, one task that owns
the balance and serves them in a loop, and replies sent back over a channel
the caller provides.

This replaces the most expensive antipattern of concurrent programming, the
object shared by many threads and guarded by locks taken in the right order
by convention. In Okkama a `Cell` cannot cross into an async context and an
object cannot hold one, so mutable state that tasks share is either inside
its owner or in an `Atomic`, by construction.

The structured rules of Chapter 20 add the second half of the pattern:
cancelling a task cancels what it waits for, and when `async main` returns,
the tasks still running are cancelled and their cleanup is awaited before
the run ends. Work that runs "in the background, somewhere", outliving the
program that started it, is not a shape the language provides.

## 26.9 Anti-patterns the compiler refuses

Several shapes that code review catches in other languages are diagnostics
in Okkama. The table collects the ones the earlier chapters showed, with the
compiler's words.

| Anti-pattern | Diagnostic |
|---|---|
| Setter, mutable field | `Field "count" cannot be assigned after construction: objects are immutable; return a successor instead` |
| Staging outside a successor | `` `^field = value` is allowed only in successor actions `` |
| Getter for raw state | `Field "cents" of object "Cash" is private; call an action instead` |
| Hidden mutable box in an object | `Field "reading" cannot hold a Cell: a cell stays in synchronous code; state an object shares with tasks is an Atomic` |
| Using an object before it exists | `Cannot call actions on self before construction completes` |
| Error as an object with a message | `Object "Failure" cannot do Error: errors are enums, declare `enum Failure is Error = (...)`` |
| Downcast from a role to a concrete object | `Cannot initialize variable "customer" of type Customer with Presentation` |
| Rebinding a value that has no successors | `Cannot rebind variable "n": type "int" has no successor actions` |
| Treating "present or absent" as optional with a fallback | `Optional fallback requires Maybe<T>; fold Presence explicitly` |
| Discarding a successor | must-use: a successor that is not used is an error (Chapter 13) |

Each line closes a door that an elegant-objects code review would otherwise
have to watch [@bugayenko2016; @bugayenko2017]. The language cannot tell
good design from bad, but it can make some bad designs impossible to write,
and Okkama spends its rules there.

## 26.10 Anti-patterns the compiler allows

The rest is judgement. These are the shapes that compile and still make a
program worse, and the reasons Okkama code avoids them.

- **The flag object.** Section 26.2: an object whose actions branch on its
  own fields to choose a behaviour. Decorators through a role replace it
  once the options interact.
- **The data bag.** An object that is a list of fields and one action per
  field returning it. Fields are private, so the getters must be written by
  hand, and writing them is the warning. An object should answer questions
  about what it is for, not hand out its parts: `invoice.total()`, not
  `invoice.lines()` added up by the caller.
- **The matryoshka.** Objects nested several layers deep only to reach a
  value, `Report(Section(Table(Rows(Source(...)))))`, where each layer adds
  no behaviour. The standard library's review rejected exactly this shape
  (Chapter 22): one object that knows how to answer, with its parts as named
  arguments, reads better and runs faster.
- **The god object.** One object that holds every dependency of a program
  and offers an action for every operation. Composition through roles
  splits it naturally: an object for each responsibility, wired together
  in `main`.
- **Stringly typed protocols.** Messages, states or commands encoded as
  strings and compared with `==`. An enum names the cases, carries their
  data, and makes `match` exhaustive (Chapter 16).
- **Catching to continue.** A `catch` for a broad type around a loop body,
  so that a failure becomes a skipped iteration. If an iteration may refuse,
  the refusal has a name and the loop catches that name.
- **The cell as a field substitute.** A `Cell` threaded through many
  actions to simulate a mutable object. A successor chain says the same
  thing with values that can be inspected afterwards.
- **Spawn and forget.** Starting tasks whose results nobody awaits.
  Structured concurrency limits the damage, since tasks still running when
  `async main` returns are cancelled and their cleanup is awaited, but a
  task nobody awaits is a task whose result nobody checks.

None of these is forbidden, and some are the right answer in small
programs. They are listed because each of them was found in real Okkama
code, and each was replaced, in the standard library review or in the
examples of this book, by one of the patterns above.

## 26.11 A refactoring, step by step

The anti-patterns of the previous section rarely come alone. The next
program has three of them: a status that is a string, an urgency that is a
flag, and a ticket that hands out its fields so that the caller can decide
what to print.

{{example:26-before}}

```text
! printer
  wifi
```

It works, and it is the kind of code the rules of the language do not
prevent. The refactoring takes it apart one smell at a time, without
changing what it prints.

1. **The string becomes an enum.** `status == "open"` compiles for any
   string, including `"opne"`. `enum Status = (open, closed)` names the two
   states, and a `match` over it is exhaustive: a third state added later is
   a compile error at every place that has to decide about it (Chapter 16).
2. **The getters become a question.** The caller wanted one thing from each
   ticket: its line in the queue, if it has one. So the ticket answers that,
   `line() is Maybe<string>`, and `title()`, `status()` and `urgent()`
   disappear. Absence is in the type: a closed ticket has no line.
3. **The flag becomes a decorator.** Urgency changes how a line looks, so it
   is an object that holds a ticket and marks its line, doing the same role
   `Queued`. A ticket that stops being urgent is the ticket without the
   wrapper; no field has to be reset.

{{example:26-after}}

```text
! printer
  wifi
```

The output is the same, and the second program is longer. What it buys is
the place where each decision lives. The meaning of "open" is in the enum;
the line of a ticket is in the ticket; the mark of urgency is in `Urgent`;
`main` only prints. A new kind of mark, a new state or a different queue
format is a change in one object, and the compiler finds every place a new
state has to be handled.

## 26.12 The classic catalogue, read in Okkama

The 23 patterns of Gamma and his co-authors are a useful checklist of what a
language makes easy, hard or unnecessary [@gamma1994]. The table goes
through the ones that change most.

| Pattern | In Okkama |
|---|---|
| Strategy | A role and the objects that do it; an anonymous action for a one-off (Chapter 18) |
| Decorator | A role, implemented by objects that hold another implementation (Section 26.2) |
| Composite | A role done by both the leaf and the container, which holds a collection of the role |
| Builder | A chain of successors, or a constructor with named and defaulted fields |
| Prototype | Unneeded: objects are immutable, so sharing one is copying it |
| Singleton | Not expressible: no global state and no static fields; one instance made in `main` and passed down |
| Factory method | Several constructors, one per form (Chapter 12), or an action that returns a role |
| Template method | No inheritance; a decorator, or an object that takes the varying step as a role |
| Observer | A channel to an owning task, or an event that waiters await (Chapter 20) |
| Iterator | `for` over collections, ranges and async streams (`for await`) |
| State | Typestate: a role per state, transitions as successors (Section 26.3) |
| Command | An enum case per command, with a `match` that interprets it; or an action value |
| Visitor | `match` over an enum; across roles, the expression problem stands (Chapter 16) |
| Null object | Unneeded as a workaround: absence is `Maybe`; still useful as a real "do nothing" implementation of a role |

Norvig's observation holds in Okkama too, in its own way. Patterns that
exist to work around mutable shared state, such as Prototype and the
defensive copies of Builder, disappear because the state cannot be
mutated. Patterns that exist to work around inheritance, Template Method
first among them, are replaced by composition because there is no
inheritance to work around. And Singleton, a pattern for global state, has
no expression at all.

## 26.13 What the patterns cost

The patterns of this chapter trade size for change. A decorator chain has
more declarations than a flag object; a role and its fake are more text than
a concrete dependency; a successor allocates a new object where a setter
would write one word; a typestate needs a role per state. For a short script
these costs are real, and an Okkama programmer who writes a fifty-line tool
with two flags and one concrete dependency is not wrong.

The costs pay back as a program grows and as it is changed by people who
did not write it. Values that cannot change can be shared without fear and
reasoned about locally; dependencies that are roles can be replaced in a
test without a framework; refusals that are names can be handled exactly;
resources that belong to scopes are released on every path. The rules that
make some antipatterns into compile errors also make some useful shapes
impossible: there is no way to cache a value inside an object after
construction other than `once let` (Chapter 13), no way to share a mutable
counter between objects other than an `Atomic` or an owning task, and no way
to write the small mutable helper object other languages reach for. Those
limits are deliberate, and Chapter 28 discusses the ones that are not.
