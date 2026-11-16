# Chapter 25. Patterns and anti-patterns

Every language comes with a second grammar that no parser checks: the shapes
experienced programmers reach for when a problem has a familiar outline. Some
of those shapes exist because the language makes the direct solution
awkward; some exist because the direct solution is wrong and the shape is
how people learned to avoid it. This chapter collects the shapes Okkama
programs take, the ones it makes unnecessary, and the ones it forbids. Most
of them have appeared already, one chapter at a time; here they are put side
by side, with the reasons that connect them.

## 25.1 What a pattern is, and what a language does to it

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

## 25.2 Behaviour by composition, not by flags

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

{{example:25-flags}}

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

{{example:25-composite}}

`Bundle` sums its parts with a `Cell` inside one synchronous action, which is
exactly the local use cells are for, and applies its own discount on top.
The gift contains an item and a bundle and prices both the same way. Two
constructors give the discount a default without a flag that every bundle
must pass.

### Strategy: a role, or an action value

Strategy, a behaviour the caller chooses, has two forms in Okkama, and the
choice between them is the same each time. A behaviour that has a name, is
used in several places or is replaced in tests is a role with objects that
do it. A question asked once, by one caller, is an action value passed to
the call (Chapter 18).

{{example:25-strategy}}

- `Ranking` is a policy of the board. The board keeps it, every leader
  query uses it, and a test can give the board another `Ranking`. `ByScore`
  keeps the earlier player on a tie, because `better` is strict.
- `first` asks one question, with a condition the caller writes in place. A
  role for "under 40 seconds" would be a declaration used once.
- When the same anonymous action appears at a second call site, it has
  become a policy and wants a name: a role, or at least an object that holds
  the action.

## 25.3 Change as a value: successor pipelines

Builders, setters and mutable "state" objects exist in other languages
because an object there can change in place. In Okkama it cannot. A field is
bound once, by the constructor; an action that changes state is declared
`is ^Type`, stages new field values with `^field = …`, and returns a new
object, the successor (Chapter 13). The old object stays as it was.

This one rule turns several patterns into ordinary code. A builder is a
chain of successor calls. An undo stack is a list of earlier values. A
snapshot before a risky step is a variable that is not touched. The next
example takes an order through four stages and keeps all of them.

{{example:25-pipeline}}

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

### Builder: options are named arguments

Bloch recommends a builder for a constructor with many parameters, most of
them optional [@bloch2018]. A positional constructor with a dozen arguments
is unreadable where it is called, and one constructor per combination of
options multiplies. Named arguments with defaults remove both problems, and
the builder with them.

{{example:25-options}}

- Every option is a field with a default. A call names the ones that
  differ, in any order, and the rest keep their defaults (Section 12.4).
- The constructor body checks the combination once. A `Fetch` that exists
  is valid, which a builder guarantees only if its `build()` repeats the
  checks.
- When options are gathered in several steps, say from a file and then from
  the command line, the successor chain of this section does the gathering:
  each step returns an object with one more decision made.

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

{{example:25-typestate}}

Each object does exactly one role, and a variable typed by the role sees only
the steps of that state. The guarantee holds even when the object itself
has more actions than its role exposes. In the next program `NewPayment`
does have a `refunded` action, but a value typed `Created` cannot reach it:

{{example:25-typestate-rejected}}

Typestate costs one role and one object per state, and transitions that
build new objects rather than successors, because the next state is a
different type. It is worth it where a wrong step is expensive: payments,
connections, documents that move through approval. Where the states are many
and the transitions few, a successor with `confirm` checks (Section 25.3's
`Order`) is the lighter tool.

### Memento: the past is a list of values

Undo is a pattern in most languages because keeping the past takes work: the
state must be copied before it changes, or every change must know how to
reverse itself [@gamma1994]. In Okkama a successor leaves the earlier value
as it was, and nothing can change it later. Keeping the past means keeping
the earlier values.

{{example:25-undo}}

- `typed` makes a new `Draft` and keeps the old one in `past`. Nothing has
  to be copied: a value that cannot change is its own snapshot.
- `undone` takes the last kept draft back and drops it from the list. With
  nothing to undo it refuses by name, instead of returning the editor
  unchanged.
- Redo has the same shape mirrored: a second list for the drafts that undo
  dropped, emptied by the next `typed`.

## 25.4 Refusals have names

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

{{example:25-refusals}}

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
type for it (Section 25.8); when it is not, the action should refuse.

At the scale of a module, the same rule makes refusals change their names at
the boundary. A layer catches the refusals of the layer below that it
understands and refuses in its own terms, so its callers never name the
layer below and do not change when it is replaced (Section 10.12).

## 25.5 Every dependency is a role, every role has a fake

Objects in Okkama reach the outside world through the objects they are given:
there is no global state and no static field (Chapter 22). An object that
sends mail holds a mailer; an object that reads time holds a clock. When the
held object's type is a role rather than a concrete object, the dependency
can be replaced, and the replacement for tests comes with the role itself:
every role may declare a `fake`, a test-only implementation that the
compiler keeps out of production code (Chapter 15).

{{example:25-signup-tests}}

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

### Decide, then do

A role and its fake make a dependency replaceable. One step further takes
the dependency out of the logic altogether: the logic decides and returns
its decisions as values, and a thin outer layer carries them out. Gary
Bernhardt called this shape a functional core in an imperative shell
[@bernhardt2012]. Immutable objects and enums with payloads make it the
natural shape in Okkama.

{{example:25-decide}}

- `Checkout.effects()` reads only its own fields. It writes nothing, sends
  nothing and reads no clock, so the same order always gives the same
  decisions.
- The decisions are an enum. The shell's `match` covers every case, and a
  new kind of effect makes the compiler point at the shell.
- A test of the core compares values, as `main` does with the first effect.
  It needs no fake, because the core has no dependency to replace, and the
  shell has almost no logic left to test.

The same shape answers Observer in a language where objects do not change.
An action does not notify listeners; it returns what happened, and its
caller passes that on. Where the listeners are tasks, the values travel over
a channel to their owner (Section 25.9).

## 25.6 A table of cases

Some actions have one rule and many inputs that matter. A tariff is the
usual example: the price changes at fixed weights, and every change is a
place where an off-by-one hides. One test per weight repeats the same three
lines ten times, and the repetition hides the one thing that differs
between them. The pattern turns the cases into data: a table with one row
per case, and one loop that checks every row. Go programmers know it as
table-driven tests [@gotabledriven]. The idea is older: Ward Cunningham's
Fit let customers write business rules as tables of examples that ran as
tests [@mugridge2005].

{{example:25-postage-tests}}

- A row is an object with named fields, so the table reads as a table:
  `Case(grams: 20, expected: Postage.letter(85))`. The named arguments are
  the column headings, repeated on every line where a reader needs them.
- The expected column holds values of the domain, here an enum with
  payloads, not strings that describe them. `test.is` compares them
  structurally (Section 15.2), so a refusal's reason and a parcel's price
  are both checked.
- One run reports every wrong row. A failed assertion does not stop the
  test, so the loop goes on after the first mismatch. In Go the same choice
  falls to the author, who must call `t.Errorf` rather than `t.Fatalf`.
- The row brings its input along with its answer. `got` and `wanted` both
  return a `Priced`, which holds the weight with the postage, and so the
  report names the row.

That last point is the one that is easy to skip. Suppose the row offered
its two fields and the loop compared only the postage:
`test.is(tariff.priced(row.grams()), row.expected())`. Then two wrong rows
report this way:

```text
--- FAIL: postage at every boundary
    expected Postage.parcel(450), got Postage.parcel(900)
    expected Postage.refused("too heavy"), got Postage.refused("over 2 kg")
```

Two rows of the table expect `Postage.parcel(450)`, and the report does
not say which of them failed. With the weight carried along, it does:

```text
--- FAIL: postage at every boundary
    expected Priced(grams: 501, postage: Postage.parcel(450)), got Priced(grams: 501, postage: Postage.parcel(900))
    expected Priced(grams: 2001, postage: Postage.refused("too heavy")), got Priced(grams: 2001, postage: Postage.refused("over 2 kg"))
```

The rows are chosen at the edges. Each tier is checked at its first and its
last weight (20 and 21, 100 and 101, 500 and 501), and the table also
holds one row on each side outside the range: 0 and 2001. This is boundary
value analysis, which Myers described as one of the most useful ways to
choose test cases, and also one of the easiest to do badly [@myers1979]. A
table makes the edges visible. An edge without a row is a missing line,
and a reviewer can see that it is missing.

A table and a property (Section 15.6) answer different questions. The
table holds the cases someone chose for a reason, and the reason is
usually an edge. A property generates the cases nobody thought of, and
checks a rule that holds for all of them: here, that the price never
falls as the weight grows. A tariff deserves both.

The cost is in the report. The whole table is one test, so it is one line
when it passes, and a single row cannot be run on its own. Go's `t.Run`
and JUnit's parameterized tests [@junit5] give every row its own name in
the report and let the runner select it. Okkama's test runner has no such
thing. A row names itself only when it fails, and only because the row
carries its input. When one row needs to run alone, for example while
debugging it, the remedy is to copy it into a test of its own.

## 25.7 Resources belong to scopes

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

{{example:25-section}}

- `Section` writes `begin` in its constructor and `end` in its `destroy`.
  A caller writes one `let` and has nothing to remember.
- Each pass of the loop is a scope: one row's section ends before the next
  row's begins.
- The refusal of row −1 leaves both scopes. Both ends are written, the
  innermost first, before the caller handles the refusal. Row 4 is never
  reached.

`defer` covers the cases that are actions rather than objects: restoring a
setting, writing a final line to a log, releasing something the program did
not create. It runs at the end of the scope, reads the bindings as they are
then, and runs before the scope's resources are destroyed. A rule of thumb
from code review: if the same `defer` appears around every use of an object,
the object wants a `destroy`.

## 25.8 Absence is a type

Okkama has no null. A value that may be missing has the type `Maybe<T>`, and
the type system makes the caller decide what absence means: a fallback with
`else`, a compact choice with `??`/`!!`, or a `match` (Chapter 7). The
pattern is to put absence in the signature of the action that can produce
it, and nowhere else. A lookup that can fail to find returns `Maybe`; a
lookup that must find refuses with a typed error.

The same pattern works for a whole program: decide absence at the edge,
where data comes in, and let the objects inside hold values that are there.

{{example:25-edge}}

- `RawSettings` is the edge. It answers with `Maybe` where a key may be
  missing, and refuses with a typed error where the program cannot go on
  without one.
- `Settings` makes both decisions once, in its constructor: a missing host
  falls back to `localhost`, a missing user is refused. Its fields are plain
  strings, and nothing that reads them checks for absence again.
- A refused construction produces no object. There is no half-built
  `Settings` with an empty user to pass on.

The antipatterns on the other side are the sentinel value and the "empty
object": `-1` for "not found", `""` for "no name", an object whose every
field is a default. They survive in Okkama only if a programmer writes them,
since the language provides no null to tempt anyone and an explicit type for
the honest answer. When a value is optional for a while and then becomes
required, the change is visible in the types, and the compiler finds every
caller that has to decide.

## 25.9 Shared state has one owner

Asynchronous code needs state that more than one task can see. Okkama's
answer is structural: state shared between tasks lives in one task, the
owner, and others talk to it through channels (Chapter 20). A ledger that
several workers update is one object in one task, receiving messages; the
workers hold the sending end of a channel, not the ledger. For the few
values that really are shared, such as a counter or a flag, `Atomic` gives a
small set of operations that are safe across tasks. The ledger of
Section 20.6 is the pattern in full: an enum of commands, one task that owns
the balance and serves them in a loop, and replies sent back over a channel
the caller provides. The next example has several writers instead of one.

{{example:25-owner}}

- The count lives in `Tally`, a local of `Visits.run()`. The readers cannot
  reach it. Each holds a `Sending<Visit>` and nothing else.
- The owner changes its state by successor, `tally = tally.counted(path)`,
  one message at a time. Two readers never update the count at once,
  because only one task updates it.
- A question is a message too. `summary` carries the channel for the
  answer, so the reply goes to whoever asked, and the state never leaves its
  owner.

This replaces the most expensive antipattern of concurrent programming, the
object shared by many threads and guarded by locks taken in the right order
by convention. In Okkama a `Cell` cannot cross into an async context and an
object cannot hold one, so mutable state that tasks share is either inside
its owner or in an `Atomic`, by construction.

{{example:25-shared-rejected}}

The diagnostic names the way out: state an object shares with tasks is an
`Atomic`. State that is more than a counter or a flag goes to an owner.

The structured rules of Chapter 20 add the second half of the pattern:
cancelling a task cancels what it waits for, and when `async main` returns,
the tasks still running are cancelled and their cleanup is awaited before
the run ends. Work that runs "in the background, somewhere", outliving the
program that started it, is not a shape the language provides.

## 25.10 Anti-patterns the compiler refuses

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

## 25.11 Anti-patterns the compiler allows

The rest is judgement. These are the shapes that compile and still make a
program worse, and the reasons Okkama code avoids them.

- **The flag object.** Section 25.2: an object whose actions branch on its
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

## 25.12 A refactoring, step by step

The anti-patterns of the previous section rarely come alone. The next
program has three of them: a status that is a string, an urgency that is a
flag, and a ticket that hands out its fields so that the caller can decide
what to print.

{{example:25-before}}

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

{{example:25-after}}

The output is the same, and the second program is longer. What it buys is
the place where each decision lives. The meaning of "open" is in the enum;
the line of a ticket is in the ticket; the mark of urgency is in `Urgent`;
`main` only prints. A new kind of mark, a new state or a different queue
format is a change in one object, and the compiler finds every place a new
state has to be handled.

## 25.13 The classic catalogue, read in Okkama

The 23 patterns of Gamma and his co-authors are a useful checklist of what a
language makes easy, hard or unnecessary [@gamma1994]. The table goes
through the ones that change most.

| Pattern | In Okkama |
|---|---|
| Strategy | A role and the objects that do it; an anonymous action for a one-off (Section 25.2) |
| Decorator | A role, implemented by objects that hold another implementation (Section 25.2) |
| Composite | A role done by both the leaf and the container, which holds a collection of the role |
| Builder | A chain of successors, or a constructor with named and defaulted fields (Section 25.3) |
| Prototype | Unneeded: objects are immutable, so sharing one is copying it |
| Singleton | Not expressible: no global state and no static fields; one instance made in `main` and passed down |
| Factory method | Several constructors, one per form (Chapter 12), or an action that returns a role |
| Template method | No inheritance; a decorator, or an object that takes the varying step as a role |
| Observer | Events returned as values (Section 25.5); a channel to an owning task, or an event that waiters await (Chapter 20) |
| Memento | Unneeded as a mechanism: earlier values stay valid, and undo is a list of them (Section 25.3) |
| Iterator | `for` over collections, ranges and async streams (`for await`) |
| State | Typestate: a role per state, transitions as successors (Section 25.3) |
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

## 25.14 What the patterns cost

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
limits are deliberate, and Chapter 27 discusses the ones that are not.
