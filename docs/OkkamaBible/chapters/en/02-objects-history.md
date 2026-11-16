# Chapter 2. A Short History of Objects

Okkama calls itself an object language, and it disagrees with most other
object languages about what an object is. To see why, we have to go back to
where objects came from, follow how the idea changed as it moved from
research into industry, and look at the problems that the industrial version
created. Okkama is an attempt to keep what the original idea got right and to
remove, by rule, what the industrial version added and later regretted.

## 2.1 Simula: objects for simulation

The first object language was built to simulate ships in a harbour. At the
Norwegian Computing Center in Oslo, Ole-Johan Dahl and Kristen Nygaard needed
to describe systems made of many independent, interacting things — ships,
cranes, queues — each with its own state and its own course of action over
time [@dahl1966; @nygaard1978]. Simula I (1962–1965) gave each such thing a
*process*; Simula 67 generalised it into the constructs every object
language has since borrowed: the *class*, describing a kind of object; the
*object*, an instance with its own data; *subclassing* ("prefixing"), where a
class extends another; and *virtual procedures*, which a subclass may
redefine.

Two things about Simula matter for the rest of this chapter. First, objects
were invented to *model behaviour over time*: a ship is not a record of
fields but something that acts. Second, inheritance arrived together with
objects, as a way to say "a tanker is a ship with more cargo". The two ideas
were born joined, and for thirty years most languages kept them joined.

## 2.2 Smalltalk: everything is a message

Alan Kay saw Simula in 1966 and drew a different lesson from it. In his
history of Smalltalk he describes his idea of objects as being like
biological cells or computers on a network: independent units that
communicate only by sending messages, none of them able to reach into
another's insides [@kay1993]. Smalltalk, developed at Xerox PARC in the 1970s
by Kay, Dan Ingalls, Adele Goldberg and others, made this uniform:
everything is an object, and all computation is objects sending messages to
objects.

Kay later summed up what he meant in a sentence that is quoted almost as
often as Hoare's: "OOP to me means only messaging, local retention and
protection and hiding of state-process, and extreme late-binding of all
things" [@kay2003]. Notice what is *not* in the list: classes, inheritance,
types. For Kay these were implementation choices; the essence was that an
object hides its state and responds to messages.

Okkama is closer to Kay's list than to Simula's. Its objects hide their state
absolutely — there is no syntax to read another object's field — and they are
used only through their actions. It parts with Smalltalk on the last item:
Okkama binds early and checks statically, because late binding of everything
makes the program text say less about what the program does (Chapter 14).

## 2.3 C++ and Java: objects for industry

The idea reached industry through C++ and Java. Bjarne Stroustrup began "C
with Classes" at Bell Labs in 1979, bringing Simula's classes to C so that
large systems could be organised without giving up C's efficiency
[@stroustrup1994]. Java, released by Sun in 1995, simplified C++ and made
classes and objects the only way to organise a program, with single
inheritance of classes, multiple inheritance of interfaces, garbage
collection — and `null` [@gosling2023].

In these languages an object became, above all, a *type with data and
methods*, and inheritance became the main mechanism of *reuse*: to get the
behaviour of a class, extend it. Conventions grew around this model. Fields
were made private, but every field got a public getter and often a setter, so
that frameworks could fill objects from databases and forms. Objects became
mutable containers of data with procedures attached — far from Kay's cells.

The model was enormously successful, and it also produced a set of problems
so persistent that a large part of the object-oriented literature of the
1990s and 2000s is about avoiding its own core mechanisms.

## 2.4 The trouble with inheritance

Already in 1986 Alan Snyder pointed out that inheritance breaks the very
encapsulation objects were supposed to provide: a subclass depends on how its
parent is implemented, not only on what it promises, so the parent can no
longer change its insides freely [@snyder1986]. The phenomenon got a name, the
*fragile base class problem*: an apparently safe change to a base class —
adding a method, calling one method from another — breaks subclasses its
author has never seen [@mikhajlov1998].

Barbara Liskov and Jeannette Wing gave the precise condition under which a
subtype may stand in for its supertype: it must preserve the supertype's
behaviour — its invariants, preconditions and postconditions — not only its
method signatures [@liskov1994]. Inheritance in mainstream languages checks
the signatures and leaves the behaviour to luck. A `Square` that extends a
mutable `Rectangle` type-checks perfectly and breaks every caller that sets
the width and expects the height to stay.

The "Gang of Four" put the conclusion at the front of their catalogue of
design patterns: "Favor object composition over class inheritance", and
"Program to an interface, not an implementation" [@gamma1994]. Most of their
patterns — Decorator, Strategy, Composite, Proxy — are ways to get the reuse
inheritance promised by composing objects instead. Joshua Bloch, who designed
much of the Java collections library, gave the same advice to Java
programmers: favour composition over inheritance, and "design and document
for inheritance or else prohibit it" [@bloch2018].

When the most-cited books on a paradigm advise against using one of its two
founding mechanisms, the razor has an obvious candidate.

## 2.5 The trouble with state

The second problem is mutable state, and objects made it worse by hiding it
behind friendly names. A mutable object shared between two parts of a program
is a channel between them: a change made by one is seen by the other, at a
time the other did not choose. Moseley and Marks argue that this kind of
state is the largest source of accidental complexity in software
[@moseley2006]: to understand what a method does, you must know every state
the object can be in, and every path by which it got there.

Getters and setters make the problem invisible. An object with a getter and a
setter for each field is a record with extra steps; any part of the program
can change it, and its invariants live in the heads of the people who
remember to call the setters in the right order. Bloch's advice — "minimize
mutability", make classes immutable unless there is a very good reason
[@bloch2018] — is, again, advice to avoid what the language makes easiest.

Bertrand Meyer attacked the same problem from the side of contracts. In
Eiffel and in *Object-Oriented Software Construction*, a class states its
invariants and each routine its preconditions and postconditions, and the
runtime checks them [@meyer1992; @meyer1997]. Meyer also proposed the
*command–query separation* principle: a routine either changes the state
(a command) or returns information (a query), never both. Okkama's successor
actions can be read as command–query separation taken to its end: there are
no commands in place; a "command" is a query that returns the next object.

## 2.6 Objects as behaviour

What, then, is an object if not data with methods? William Cook gave a
precise answer by contrasting objects with *abstract data types* [@cook2009].
An abstract data type hides a representation behind a set of operations that
all know the representation — a module can compare two of its values by
looking inside both. An object is a *procedural* abstraction: it knows only
itself, and it interacts with another object only through that object's
interface. Two objects implementing the same interface may have completely
different representations and still work together.

This is Kay's idea in a formal dress, and it is the idea Okkama keeps. An
Okkama object is defined by the actions it offers; a role names a set of
actions; any object that declares it does the role can stand in for it, and
nothing — not even an object of the same kind — can look at its fields.

## 2.7 Elegant Objects: the discipline

In 2016 and 2017 Yegor Bugayenko published *Elegant Objects*, two short and
deliberately provocative volumes that turn the criticisms above into a code of
practice for Java programmers [@bugayenko2016; @bugayenko2017]. Its rules are
blunt: never use `null`; no getters and setters; objects must be immutable;
constructors only assign fields, and real work happens in methods; no static
methods and no utility classes; no class inheritance beyond what is
unavoidable; extend behaviour with decorators; replace mocks with fake
implementations of interfaces; name objects as things, not as "-er" doers
(a `Cash`, not a `CashFormatter`).

Many experienced programmers find the rules excessive, and in Java they are
costly to follow: the language, the libraries and the frameworks all push the
other way, and nothing checks that a team actually follows them. Code review
is the only enforcement, and code review is tired at five in the afternoon.

Okkama starts where *Elegant Objects* stops. It takes the discipline and
makes it the language, so that the compiler and the runtime enforce what a
reviewer would otherwise have to catch:

| *Elegant Objects* rule | Okkama rule |
|---|---|
| Never use `null` | there is no null; `null` and `nil` are reserved words that do not parse |
| No getters and setters | fields are private, always; there is no field access from outside and no setter syntax |
| Objects are immutable | fields are bound only during construction |
| A changed object is a new object | successor actions `is ^T` return a new object; discarding it is an error |
| No inheritance | there is no `extends`, `super`, `abstract` or `protected` |
| Program to interfaces | roles are nominal contracts implemented with `does` |
| Fakes instead of mocks | a role can carry a `fake` block, its reference test double |
| Constructors bind and validate | constructors bind fields and check invariants with `confirm` |
| Fail fast | `confirm` failures are panics or typed errors; there are no partial results |

## 2.8 Composition in practice

Here is the decorator pattern, which the Gang of Four offered as the
alternative to subclassing, written in Okkama. A role says what a price can
do; one object holds a listed amount; two decorators each wrap any price and
change one thing about it.

{{example:02-decorators}}

There is no hierarchy here. `Discounted` and `Taxed` are not kinds of
`Listed`; they are things that *have* a price and *are* prices. They can be
stacked in any order, wrapped around any present or future implementation of
`Price`, and tested with a fake price instead of a real one. Each object has
one reason to exist and is a few lines long.

Two details show the rest of the language at work. The shorthand constructor
`Discounted(:origin, :percent);` binds two fields from named arguments, and
callers must name them: `Discounted(origin: book, percent: 10)`. And the
discount is computed with an explicit `int(...)`: in Okkama, dividing two
integers gives an exact rational number, not a truncated integer, so turning
it back into cents is a decision the program must write down. Chapter 5
explains why.

## 2.9 Roads not taken: prototypes, actors and generic functions

Classes were not the only way objects could have gone. Three other lines came
out of the same decades, and each answers a question Okkama also had to
answer.

**Prototypes.** In Self, designed by David Ungar and Randall Smith, there are
no classes at all [@ungar1987]. A new object is made by copying an existing
one, its prototype, and an object that has no slot for a message passes the
message on to a parent object. The model is smaller than Smalltalk's: one
kind of thing instead of objects, classes and metaclasses. It reached a very
large audience through JavaScript, whose objects delegate to prototypes in
the same way [@wirfsbrock2020].

Okkama keeps the kind. An `object` declaration states the fields and actions
every instance has, and the compiler checks every call against it. A
prototype language can check nothing of the sort before the message is sent,
because any object may gain or lose a slot at any time. What Okkama takes
from Self is narrower: a successor (Section 13.5) is a copy of an existing
object with some fields replaced, which is the one operation of prototypes
that does not need mutation.

**Actors.** In 1973 Carl Hewitt, Peter Bishop and Richard Steiger proposed
the actor as the single unit of computation [@hewitt1973]. An actor receives
a message, and in response it may send messages, create actors and decide how
it will handle the next message. There is no shared memory; an actor's state
is reachable only through its mailbox. This is Kay's "local retention and
protection of state-process" made into a model of concurrency, and Erlang
built telephone switches on it, with processes that share nothing and are
restarted when they fail [@armstrong2003].

Okkama reaches the same place from the other side. Its objects share nothing
that can change, because nothing in them changes. A task that owns a changing
state keeps it in a local, moves it forward by rebinding the local to
successors, and is reached by others through a channel (Section 20.6). An
actor is therefore a pattern in Okkama, not a kind of object: most objects
are plain values that need no mailbox, and only the few that carry a state
through time are wrapped in a task.

**Generic functions.** Smalltalk sends a message to one receiver, and the
receiver alone chooses the method. The Common Lisp Object System, specified
in 1988, turned this around [@bobrow1988]. A method belongs to a *generic
function*, not to a class, and the generic function chooses among its
methods by the classes of all its arguments: multiple dispatch. Methods can
also be combined. `:before`, `:after` and `:around` methods wrap a primary
method, and new ones can be added to a generic function from anywhere, for
classes the author of the function never saw. Julia made multiple dispatch
the centre of a numerical language, where `+` on an integer and a rational,
or on a matrix and a vector, is just another method chosen by both argument
types [@bezanson2017].

The model is strong where Okkama is weak. A binary operation is symmetric
in CLOS and Julia, while an Okkama operator looks only to its left operand
(Section 14.7). And an operation can be added to existing types, which is
one half of the expression problem (Section 16.5). Okkama declined both
halves of the price. A method added from anywhere is retroactive
conformance, which roles refuse (Section 14.2), because a reader can no
longer find an object's behaviour in its declaration. Method combination is
inheritance of behaviour by another name, which the language removed
(Section 1.9). What Okkama keeps is the closed form of the same idea. A
`match` over a variant (Chapter 16) chooses by the case of a value from a set
the declaration fixes, and all the choices sit in one place.

## 2.10 The harbour, once more

Simula began with ships in a harbour, so the history can end there. Here is
a harbour with one berth and one crane, written in the style the rest of the
book teaches: no field ever changes, and every hour of the harbour's life is
a new value.

{{example:02-harbour}}

- **A ship acts by producing its next state.** `unloaded(3)` does not lower
  a counter inside the ship; it returns the ship with up to three fewer
  containers. The ship the crane started on is still there, unchanged, for
  anyone who holds it.
- **The harbour is a value too.** `worked()` is one hour of the simulation:
  it advances the clock, unloads the ship at the berth and lets an empty
  ship sail. `arrived(ship)` puts a ship at the end of the queue. Both are
  successors, so the harbour has no setters and no hidden state.
- **Time is data.** Simula's processes move forward and forget: the harbour
  of hour 2 is gone once hour 3 has run. Here `history` is an array of nine
  harbours, one per hour, and `history[2]` answers a question about the past
  without running anything again. The price is the memory of the states
  kept; a program that does not need the past keeps only the last one.
- **Order is written down.** The ships due in an hour join the queue before
  the crane works, in the order of the schedule. Nothing happens "at the
  same time" unless the program says so, and Chapter 20 shows how it says
  so.

Dahl and Nygaard's insight survives: a program is made of things that act.
What changed is the meaning of "act". In Simula an object acts by changing
itself; in Okkama it acts by answering with what comes next.

## 2.11 Values come back

The discipline of Section 2.7 did not stay a minority view. In the 2000s and
2010s the mainstream rediscovered immutable values from several directions
at once.

- **Value objects.** Evans's *Domain-Driven Design* separated entities,
  which keep an identity through change, from value objects, which are
  defined by their attributes, have no identity of their own and should be
  immutable. An amount of money or a date range is a value object: it is
  replaced, not edited [@evans2003].
- **Minimal mutability.** Bloch's *Effective Java* advised making a class
  immutable unless there is a good reason not to, because an immutable
  object is simple, safe to share between threads and free to reuse
  [@bloch2018].
- **Declarations for values.** Languages followed with syntax. Kotlin's
  data classes generate equality and a copying operation [@kotlindata];
  Java records, final in Java 16, are transparent carriers of immutable data
  [@jep395]; C# 9 added records with equality by value and non-destructive
  change through `with` [@csrecords].

What these share with Okkama is the value itself: equality by contents, no
mutation, a new value for every change (successors, Section 13.5; objects
as values, Section 12.9). Okkama parts from them on the other side of
Evans's distinction. A record is defined by its fields and hands them out,
while Okkama keeps the manifesto's first rule even for values: an object
answers questions and keeps its fields (Section 3.1), and a shorthand for
data objects is still an open question (Section 28.4). Nor does Okkama have
entities in Evans's sense, mutable state behind a stable identity, except
through `Cell` and `Atomic`. What persists through change is a name rebound
to successors, or state owned by one task (Section 20.6).

## 2.12 What was lost

Removing inheritance removes real conveniences, and it would be dishonest to
pretend otherwise.

- **Implementation reuse.** A subclass inherits dozens of methods for free; a
  decorator must implement every action of its role, even those it merely
  forwards. For roles with many actions this is boilerplate. Several
  languages answer with automatic delegation; Okkama has none, and default
  action bodies in roles were considered and declined (issue #146): a role
  stays a pure contract, with no implementation to inherit through the back
  door. Forwarding is written by hand.
- **Frameworks built on hierarchies.** Many libraries — GUI toolkits, web
  frameworks, ORMs — are organised as base classes to extend. Their designs
  do not translate directly; Okkama's standard library had to be written in
  the compositional style from the start, and some of it shows the strain
  (Chapter 22).
- **A familiar vocabulary.** "Is-a" is how textbooks teach modelling. Okkama
  asks for "has-a" and "does": a tanker *has* a hull and *does* the role of a
  ship. The modelling is often clearer, but it must be learned.

The trade is the one the literature has recommended for thirty years. Okkama
simply stops offering the alternative.
