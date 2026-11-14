# Chapter 3. The Manifesto, Rule by Rule

Chapter 1 gave the razor; Chapter 2 gave the history it cuts through. This
chapter states Okkama's rules one by one. For each rule it says what the rule
removes, why — with the strongest argument from the literature — what the
compiler answers when a program breaks it, and what the rule costs. The
later chapters teach each construct in detail; here the aim is to see the
whole design at once and to see that it is *one* design: the rules support
each other, and removing one would weaken several others.

Every rejected program in this chapter is real. `build.py --check` compiles
each one and compares the compiler's message with the text printed here.

## 3.1 Rule one: objects expose behaviour, not state

An object's fields are visible only inside the object. There is no syntax to
read another object's field, no `public` modifier to open one, and no getter
generated from a field. An object is used through its actions.

{{example:03-field-rejected}}

**Why.** Information hiding is the oldest principle of modular design:
Parnas showed in 1972 that a module should hide the decisions most likely to
change, and that the representation of data is the first among them
[@parnas1972]. Cook's analysis of objects adds that an object is defined by
its interface alone; two objects with different representations that offer
the same actions are interchangeable [@cook2009]. If a field can be read from
outside, its representation becomes part of every reader's contract, and it
can never change again.

Getters do not solve this — they restate it. A getter for every field is a
public field with a function call around it. *Elegant Objects* argues that an
object should be asked to *do* something with its data, not to give it away
[@bugayenko2016]: not `invoice.total()` followed by formatting in the caller,
but `invoice.printed(format)`. Okkama does not forbid an action that returns a
field's value — `cents() is int { cents; }` is legal and common — but it makes
that a decision written in the object, not a default.

**Cost.** Some objects really are just data: a point, a date, a row from a
database. For them, actions that return fields are boilerplate. Okkama's
answer today is to write them; a future shorthand is discussed in Chapter 28.

## 3.2 Rule two: state is bound at construction, and constructors guard it

An object's state is declared with `has` and bound when the object is
constructed; after that it never changes. A constructor's job is to bind
fields and to check that together they make sense. If they do not, the
object is never created.

{{example:03-invariant}}

**Why.** If an object can exist only after its constructor accepted its
state, then every object of that type in the program satisfies the
invariant — there is no window in which a half-built object is visible, and no
method has to re-check what the constructor checked. Meyer's *Design by
Contract* made class invariants the centre of object-oriented correctness
[@meyer1992]. Alexis King's "parse, don't validate" gives the modern form of
the same advice: turn an unchecked value into a type that *cannot* hold an
invalid value, once, at the boundary, so that the rest of the program never
asks again [@king2019]. A `Percent` in the program above is not an `int` that
someone hopefully checked; it is a value that cannot be 150.

`confirm condition else throw Error` is the tool: without `else`, a failed
`confirm` is a *panic* — a bug in the program; with `else throw`, it is a
typed error that a caller can handle (Rule eight).

**Cost.** Validation in constructors means construction can fail, so code
that creates objects from untrusted input must be ready for a typed error.
That is the point of the rule, but it moves work to the edges of a program
where Java code would often skip it.

## 3.3 Rule three: change is a new object

An action that "changes" an object declares the successor type `^T` and
returns a new object of the same kind with some fields re-bound
(`^opened = true`). The old object is unchanged and still usable. A caller
that drops the successor has almost certainly made a mistake, so dropping it
is an error.

{{example:03-discard-rejected}}

**Why.** Moseley and Marks identify mutable shared state as the main source
of accidental complexity [@moseley2006]; Bloch's first advice about classes is
to minimise mutability, because immutable objects are simple (one state),
thread-safe without locks and freely shareable [@bloch2018]. Successors give
the convenience of "changing" an object — `cart.addition(item)` reads like a
mutation — without the aliasing: whoever held the old cart still holds the
old cart. The must-use rule closes the most common bug of immutable APIs in
other languages: calling `date.plusDays(1)` and forgetting that it returns
the answer instead of changing `date`.

Purely functional data structures show that this need not be slow: persistent
lists, trees and maps share most of their structure between versions, so a
"new" version costs logarithmic work, not a full copy [@okasaki1998].

**Cost.** Performance and vocabulary. A new object per change costs an
allocation; Sparrow makes small allocations cheap, and the compiler
removes many, but arrays and maps without efficient persistent updates make
some idioms quadratic today (issues #145 and #151). And every state a program
cares about must be named and returned, which makes long chains of updates
more verbose than assignments.

## 3.4 Rule four: there is no null

No value stands for "nothing". `null`, `nil` and `none` are not values; the
words `null` and `nil` do not even parse.

{{example:03-null-rejected}}

A value that may be absent has a type that says so — `Maybe<T>`, with cases
`some(value)` and `none` — and the program must open it before using the
value inside: with `else` for a fallback, with `match`, or with `??`. A map
lookup that may miss takes an explicit fallback; an action that cannot answer
throws a typed error, and `try?` turns such an error into a `Maybe` when
absence is the honest answer.

{{example:03-absence}}

**Why.** Tony Hoare, who introduced the null reference in ALGOL W in 1965,
called it his "billion-dollar mistake": it was easy to implement and has
caused innumerable errors, vulnerabilities and crashes since [@hoare2009]. A
null reference is a value of *every* reference type that supports *none* of
that type's operations — a hole in every type of the program. Languages of the
ML family never had it: absence is a separate type, `option` or `Maybe`, and
the type checker forces the programmer to handle it [@pierce2002]. Okkama
takes the ML answer and makes the handling syntax short, so that the honest
version costs no more typing than the dangerous one.

**Cost.** Code that ports a design built on null must decide, at each place,
what absence *means*: a default, an error, or an explicit `Maybe`. That is
work. It is work that the original design skipped, and that its users paid for
later.

## 3.5 Rule five: conditions are booleans

`if`, `match` guards, `confirm`, `&&`, `||` and `!` accept exactly `bool`.
There is no truthiness: zero, the empty string and the empty array are
present values, not "false".

{{example:03-truth-rejected}}

**Why.** Truthiness merges two questions — "is it there?" and "is it true?" —
into one, and the merge is different in every language: `0` is false in C and
Python but true in Ruby; `"0"` is false in PHP and Perl and true in Python; an
empty array is false in Python and true in JavaScript. A condition should say
what it tests. `if count > 0` says it; `if count` makes the reader recall a
convention.

**Cost.** A few characters per condition, and occasionally an explicit
`.size() > 0` where other languages write the collection alone.

## 3.6 Rule six: numbers are exact, and categories never mix silently

Okkama has four numeric categories — `int`, `byte`, `rat` and `float` — and
does not convert between them implicitly. `int` arithmetic is exact: it never
overflows, because a result that does not fit 64 bits becomes a big integer.
Dividing two integers gives an exact rational (`7 / 2` is `7/2`, not `3`).
Comparing an `int` with a `float` is an error, not a guess.

{{example:03-mixed-rejected}}

**Why.** Silent overflow has caused famous failures: the Ariane 5 maiden
flight was lost in 1996 because a 64-bit floating-point value was converted to
a 16-bit integer that could not hold it [@lions1996]. Silent truncation in `7 / 2 == 3`
and silent rounding in `0.1 + 0.2 != 0.3` are among the most common surprises
of new programmers, and Goldberg's classic survey shows how subtle
floating-point reasoning is even for experts [@goldberg1991; @ieee754].
Okkama keeps `float` for when binary floating point is what the problem
wants — physics, graphics, statistics — and makes the exact categories the
default for counting and money. Mixing them is a decision the programmer
writes down: `int(x)`, `x.float()`, `x.rat()`.

**Cost.** Speed and ceremony. Exact integers need an overflow check on every
operation (Sparrow makes it one branch); rationals are slower than
floats; and the explicit casts that Chapter 2's decorator needed —
`int(amount * percent / 100)` — are characters other languages do not ask
for. Chapter 5 measures the cost.

## 3.7 Rule seven: roles instead of inheritance

There is no class inheritance. Behaviour is shared by *composition* — an
object holds other objects in its fields — and abstracted by *roles*: named
sets of actions that an object declares it performs with `does`. Roles are
nominal: an object that happens to have the right actions does not perform a
role unless it says so.

**Why.** Chapter 2 told the story: inheritance breaks encapsulation
[@snyder1986], produces fragile base classes [@mikhajlov1998], and is safe
only when subclasses preserve behaviour, which compilers cannot check
[@liskov1994]; the standard advice is to prefer composition and program to
interfaces [@gamma1994; @bloch2018]. Okkama keeps the interface and drops the
implementation inheritance. Roles are nominal rather than structural because
a shared method name is not a shared contract: an object with `close()` that
closes a file and one with `close()` that closes a deal should not be
interchangeable by accident.

**Cost.** Forwarding. A decorator must implement every action of its role,
even the ones it only passes on. Default action bodies in roles would remove
that, and were declined on purpose (issue #146): a role with code in it is a
base class again.

## 3.8 Rule eight: failures are typed, and bugs are not exceptions

A failure the caller may handle is a *thrown error*: a value of an error
enum, caught by naming its type or case. A failure that means the program is
wrong — a failed `confirm` without `else`, an impossible state — is a
*panic*, and panics cannot be caught. There is no catch-all handler and no
`finally`; cleanup is `defer` and `destroy` (Rule nine).

**Why.** Goodenough's 1975 paper set out the questions every exception
mechanism must answer — which failures a caller sees, whether execution can
resume, how cleanup happens [@goodenough1975]. Forty years of practice gave
an answer that Joe Duffy summarised from the Midori project: *recoverable*
errors — a missing file, a refused connection — belong in the type of an
operation and must be handled by its callers; *bugs* — a broken invariant, an
index out of range — cannot be meaningfully handled by the code that hit them
and should stop the computation ("abandonment") [@duffy2016]. Erlang's "let it
crash" reaches the same conclusion from fault-tolerant systems: a component in
an unknown state should stop, and a supervisor should decide what to do
[@armstrong2003]. A catch-all handler mixes the two kinds and tempts the
program to continue in a state nobody understands.

**Cost.** Programs must name what they catch, and wrapping errors from one
layer into another's vocabulary is explicit — sometimes repetitively so
(issue #148 proposes a construct for it).

## 3.9 Rule nine: resources are released deterministically

An object whose type declares `destroy` is a *resource*: an open file, a
socket, a database connection, a lock. Resources are owned by the scope that
created them, and `destroy` runs when that scope exits — normally, by
`return`, or by failure — in reverse order of creation. `defer` registers
cleanup for the current block. There are no finalizers and nothing to forget
in a `finally`.

**Why.** Stroustrup's RAII — resource acquisition is initialisation — made
object lifetime the lifetime of the resource it owns, so that release cannot
be forgotten [@stroustrup1994]. Garbage-collected languages lost this,
because objects die whenever the collector decides, and they compensated with
`try/finally`, `using` and `with` blocks that programmers must remember to
write. Okkama has both a precise garbage collector for memory and
deterministic destruction for resources. Its ownership regions are related to
Tofte and Talpin's region-based memory management, in which the lifetimes of
groups of objects are tied to the program's block structure [@tofte1997].

**Cost.** A resource must have a clear owner. Programs that share a resource
between independent owners must make one of them responsible, or use the
library's pools and leases; Chapter 11 explains the rules, and they take
practice.

## 3.10 Rule ten: authority is explicit

Code reaches the outside world — files, network, processes, environment,
clock, randomness, terminal — only through natives, and natives are grouped
by the authority they need. An embedding program can run Okkama code with
only some groups allowed and with budgets on steps, memory, output and time;
a native outside the allowed groups fails the program.

**Why.** Saltzer and Schroeder's principle of least privilege says every
part of a system should run with the smallest authority it needs
[@saltzer1975]. Capability systems, from Dennis and Van Horn's design in 1966
to Mark Miller's object-capability model, make authority something a program
must *hold* rather than something it has by default [@dennis1966;
@miller2006]. Okkama's object model is close to capabilities already — an
object can only use what it was given — and the native groups extend the idea
to the boundary with the host.

**Cost.** The command-line tools still give programs the full authority of the
user who runs them; finer, per-object capabilities for the standard library
are future work (Chapter 21).

## 3.11 Rule eleven: concurrency is structured

Asynchronous work is written with `async` actions and `await`; work started
inside a scope belongs to it, and a scope does not finish until its children
have finished or been cancelled. CPU-parallel work runs in isolated worker
processes started with `spawn`, which share nothing with their parent.

**Why.** Nathaniel Smith compared unstructured `go` and `spawn` statements to
the `goto` that Dijkstra removed: once a function can start work that
outlives it, a caller can no longer reason about what the function does by
looking at its call [@smith2018; @dijkstra1968]. Structured concurrency
restores the block structure — every task has a parent that waits for it — so
errors propagate and cancellation reaches everything that was started.
Process isolation for parallelism follows Hoare's communicating sequential
processes: independent processes that share nothing and communicate by
messages [@hoare1978].

**Cost.** Sharing memory between parallel workers is impossible by design, so
parallel algorithms must be expressed as messages; and the async standard
library needs more structure — groups, settlements, deadlines — than a
fire-and-forget API (issue #147 discusses how much of it the language should
absorb).

## 3.12 One design, not eleven

The rules are not a list of independent prohibitions; each makes the others
work. Immutability (Rule three) makes hiding state (Rule one) cheap, because
there is nothing to protect from concurrent writers. Constructors that guard
invariants (Rule two) are only meaningful because state cannot change later
(Rule three). The absence of null (Rule four) is what lets a constructor
promise that every field is bound. Typed failures (Rule eight) are what
constructors throw when an invariant fails. Deterministic destruction (Rule
nine) is safe because objects have one owner and no hidden aliases through
mutation. Explicit authority (Rule ten) is natural in a model where objects
use only what they were given. And structured concurrency (Rule eleven) is
the same block structure as destruction, applied to time.

Remove any one rule and several others weaken. Add null back, and
constructors can no longer promise bound fields. Add setters, and invariants
need re-checking in every method, sharing needs locks, and destruction needs
to know who else holds a reference. This is why Okkama adopts the rules as a
whole rather than as options: as *Elegant Objects* argued for Java, the
discipline works only when it is complete — and only a language can make it
complete.
