# Part I. The Razor

# Chapter 1. Entities Must Not Be Multiplied

A programming language is a theory of programs. It says which things exist —
values, objects, functions, errors, threads — and which relations between
them are allowed. Like any theory, it can explain a great deal with few
entities or explain the same with many. Okkama is named after the medieval
philosopher who gave the most famous rule for choosing between such theories,
and the name is a promise: wherever two designs do the same work, the language
keeps the one with fewer things in it.

This chapter is about that promise. It tells where the rule comes from, what
it does and does not claim, why there are good mathematical reasons to trust
it, how the best-known computer scientists arrived at the same rule for
software, and what it means concretely for Okkama — including the price.
Everything else in the book follows from it.

## 1.1 A Franciscan from Surrey

William of Ockham was born around 1287 in the village of Ockham in Surrey,
studied and taught theology at Oxford, and spent the second half of his life
in exile after a dispute with Pope John XXII about the poverty of the
Franciscan order; he died in Munich around 1347 [@spade-sep]. He was a
logician and a nominalist: he held that only individual things exist, and
that general terms such as "humanity" or "redness" are signs we use for many
individuals, not additional entities standing behind them.

His nominalism is the razor at work. Where his predecessors explained why two
white things are both white by positing a third thing — a real universal,
"whiteness" — Ockham asked whether the explanation needed it. If two white
things, our concept and the word "white" already account for everything we
observe, the universal adds nothing but itself. He wrote the principle in
several forms; one that recurs in his works is *numquam ponenda est pluralitas
sine necessitate*, "plurality is never to be posited without necessity"
[@spade-sep].

The words people usually quote — *entia non sunt multiplicanda praeter
necessitatem*, "entities must not be multiplied beyond necessity" — are not
in Ockham's writings at all. They are a later formulation, and the
attribution of the "razor" to Ockham by name is itself a product of later
centuries [@thorburn1918]. The principle is older than him, too: Aristotle
and many scholastics used versions of it. Ockham's name stuck because he used
it so relentlessly that it became his signature tool.

For our purposes the history teaches two things. First, the razor is a rule
about *explanations*, not about the world: it does not claim that the world is
simple, only that we should not complicate our account of it without a reason.
Second, it cuts in one direction only. It removes entities that do no work; it
says nothing against entities that do.

## 1.2 What the razor says, and what it does not

Philosophers distinguish two kinds of simplicity that the razor can mean
[@baker-sep]. **Elegance** (syntactic simplicity) is about the number and
complexity of the principles of a theory: fewer axioms, shorter laws.
**Parsimony** (ontological simplicity) is about the number and kinds of
things a theory says exist. A theory can be elegant and extravagant at once —
one short law that requires a new kind of particle — or parsimonious and
clumsy.

Both readings matter for a language. Elegance is the size of the language
definition: how many rules a programmer must hold in their head. Parsimony is
the set of things that can exist at run time: whether there is a null, a
mutable cell, a class hierarchy, a global variable, a thread. Okkama cares
about both, but it is above all a *parsimonious* language. Its specification
is not short — Okkama is careful and explicit, and careful rules take words —
but the inventory of things a program can contain is deliberately small.

The razor is also a **heuristic, not a proof**. Elliott Sober's careful study
of its uses shows that "Ockham's razor" names several different principles,
each justified — when it is justified — by a different argument, and that
none of them says "the simpler theory is true" [@sober2015]. In practice the
razor is a rule for *choosing* when the evidence does not decide, and a rule
for *burden of proof*: whoever wants to add an entity must show the work it
does.

That is exactly how Okkama uses it. Every construct that other languages have
and Okkama lacks — null, inheritance, setters, truthiness, implicit
conversions, catch-all handlers — had to answer the question "what work does
this do that the remaining constructs cannot?" Where the answer was "it saves
some typing", the construct was removed. Where the answer was "nothing else
can express this", the construct stayed, or a smaller one replaced it.

## 1.3 Why simple explanations predict better

It is tempting to treat the razor as an aesthetic preference — simpler is
prettier. There are, however, at least three independent mathematical reasons
to prefer the simpler of two theories that fit the facts equally well, and
all three translate into software.

### The Bayesian Occam factor

Suppose two models could have produced our observations: a simple one, H₁,
that can produce only a narrow range of outcomes, and a flexible one, H₂, that
can produce almost anything. Each model spreads one unit of probability over
the outcomes it can produce. The simple model concentrates its unit; the
flexible one thins it out. If the observed data fall in the narrow range both
can explain, the simple model assigned them a higher probability — and Bayes'
rule rewards it with a higher posterior, *without any prior preference for
simplicity* [@mackay2003]. MacKay calls the ratio by which the flexible model
is penalized the *Occam factor*.

{{figure:occam-evidence|Figure 1.1. The Bayesian Occam factor. Both models give probability 1 in total; the simple one concentrates it, so on data in C₁ it wins (after MacKay 2003, ch. 28).}}

The software analogue is a type. A function whose type says `(int) is int`
"predicts" a narrow set of behaviours; a function of type `(any) is any`
predicts almost anything. When the narrow one is correct, its type tells the
reader — and the compiler — far more. Wadler made this precise for parametric
polymorphism: the more general a type, the fewer functions can have it, and
the more a reader can deduce from the type alone [@wadler1989].

### Minimum description length

Rissanen proposed to choose the model that gives the shortest total
description of the data: the length of the model plus the length of the data
encoded with the model's help [@rissanen1978]. A model that is too simple
leaves much to encode; a model that is too complex costs more to state than it
saves. The best model compresses the most.

A program is such a description. Every concept a language forces you to state
— a null check, an adapter between two error conventions, a defensive copy of
a mutable object — is length added to every program, length that encodes
nothing about the problem.

### Kolmogorov complexity

The limit of this idea is the length of the shortest program that produces a
given object, its Kolmogorov complexity [@li2019]. It is uncomputable, but it
gives the intuition a firm ground: an object is simple if it has a short
description, and regularity *is* compressibility. Languages differ in which
regularities they make short to state. A language with immutability by
default makes "this does not change" free and "this changes" explicit; a
language with mutability by default does the opposite. Okkama chooses which
facts are cheap to say so that the common, safe case is the short one.

## 1.4 The same rule, rediscovered for software

None of the founders of software engineering cited Ockham, yet many arrived
at his rule from the opposite direction — from the pain of maintaining
complicated programs.

**Hoare** ended his Turing Award lecture with a sentence that is now
practically the razor of software: "There are two ways of constructing a
software design: one way is to make it so simple that there are obviously no
deficiencies, and the other way is to make it so complicated that there are no
obvious deficiencies. The first method is far more difficult" [@hoare1981].
In the same lecture he described how a language with too many features
(PL/I, and later Ada) becomes impossible to implement reliably and to learn
completely.

**Dijkstra** argued that a programmer's main limitation is the size of their
own head: "The competent programmer is fully aware of the strictly limited
size of his own skull; therefore he approaches the programming task in full
humility, and among other things he avoids clever tricks like the plague"
[@dijkstra1972]. Four years earlier he had removed an entity from programming
— the unrestricted jump — on the grounds that it made the dynamic behaviour of
a program unrelated to its text [@dijkstra1968].

**Brooks** separated *essential* complexity, which belongs to the problem,
from *accidental* complexity, which we add with our tools, and argued that no
single tool would remove the essential part [@brooks1987]. A language cannot
make a tax law simpler. It can stop adding to it.

**Moseley and Marks** went further and named the main source of accidental
complexity: *state*, and in particular mutable state shared between parts of
a program, followed by control flow and code volume [@moseley2006]. Their
paper is the most direct argument for the design of Okkama's object model:
state exists, but it is fixed at construction, and every "change" is a new
value whose existence the program text must acknowledge.

**Hickey** made the distinction that saves the razor from a common
misunderstanding [@hickey2011]. *Simple* means "one fold": not braided
together with other concerns, an objective property. *Easy* means "near at
hand": familiar, quick to type, a property of the reader. Null is easy — every
mainstream programmer knows it — and not simple, because it braids "a value"
with "maybe no value" in every reference of the program. Okkama chooses simple
over easy wherever the two disagree, and this book will point out each time
the choice costs ease.

**Parnas** showed that the criterion for splitting a system into modules is
the hiding of design decisions likely to change [@parnas1972], and
**Ousterhout** turned it into a measure: a good module is *deep*, offering a
simple interface over a large implementation, and complexity is what you must
know to change a system safely [@ousterhout2018]. Okkama's private fields and
nominal roles are information hiding made mandatory.

**Wirth**, who designed Pascal, Modula-2 and Oberon by removing features,
warned that software grows faster than hardware improves because nobody pays
for removing things [@wirth1995]. Each of his languages was smaller than the
one before.

## 1.5 The other side: worse is better

A fair account must give the opposing argument its full strength. In 1991
Richard Gabriel observed that the systems that spread are often not the
cleanest ones [@gabriel1991]. Unix and C won over Lisp machines and more
principled designs because they were *simple to implement*, even when this
made them less simple, less consistent or less correct to use. He called this
"worse is better": implementation simplicity first, then interface
simplicity, with correctness and completeness negotiable.

Gabriel’s essay asks whose simplicity counts: the implementation’s or the user’s. Okkama chooses a small user model: exact numbers promote to big integers, and resources are released deterministically, so program text and the set of things a reader must consider stay small.

The price is real. A language that refuses conveniences is harder to adopt.
Programmers arrive with habits from languages that have null, mutable fields
and inheritance, and Okkama asks them to unlearn. Gabriel's warning applies:
correctness does not guarantee adoption. This book tries to answer the warning
in the only honest way — by showing, chapter after chapter, what each refusal
buys, and by saying plainly where it costs too much.

## 1.6 Okkama's razor, entity by entity

The table below is the shortest summary of the language. Each line names an
entity that most popular languages have, what Okkama has instead, and the
chapter that argues the case.

| Removed entity | What does the work instead | Chapter |
|---|---|---|
| `null`, `nil`, implicit optionals, `?.` | `Maybe<T>` and `Presence<T>` opened with `else` or `match` | 7 |
| Truthiness (`if 1`, `if ""`) | conditions are exactly `bool` | 9 |
| Implicit numeric conversion (`1 == 1.0`) | exact categories `int`, `byte`, `rat`, `float`; explicit casts | 5 |
| Integer overflow | `int` arithmetic is exact and promotes to big integers | 5 |
| Public fields, getters and setters | private `has` fields; objects expose behaviour | 12 |
| Mutation after construction | successor actions `is ^T` return a new object | 13 |
| Inheritance, `super`, `abstract`, `protected` | roles (nominal contracts) and composition | 14 |
| Mocking frameworks | `fake` blocks shipped with every role | 15 |
| Catch-all handlers and `finally` | typed `catch`, `defer`, failure causes | 10 |
| Finalizers and "close in finally" | `destroy` blocks run when the owner leaves scope | 11 |
| Global functions and static state | behaviour lives on objects; modules export declarations | 12, 19 |
| Unstructured threads | `async`/`await` with structured cancellation; process-isolated `spawn` | 20 |
| Ambient authority of native code | natives behind declared groups and budgets | 21 |

Some of these words are not merely absent but *removed*: `null`, `nil`,
`none`, `isa`, `super`, `abstract`, `protected`, `with` and `finally` are reserved
so that a program using them fails to parse with a message instead of meaning
something else. The language does not just leave the door open with nothing
behind it; it bricks the door up and puts a sign on it.

Okkama is not minimal in the sense of Scheme or Forth. It has exact rationals and big integers, a standard library that speaks HTTP/2 and PostgreSQL, and structured concurrency. The razor applies to kinds of entities in a program, not to the size of the toolbox.

## 1.7 A first taste

Here is a complete program. It declares an object with one private field,
one constructor and two actions, and uses it once.

{{example:01-bottle}}

Even this toy shows four of the rules from the table. The field `opened` is
private: nothing outside `Bottle` can read or write it. The object exposes
behaviour (`open`, `state`), not data. `open()` does not change the bottle — its
signature `is ^Bottle` says it returns a *successor*, a new bottle in which
`opened` is `true`, and the caller has to take that result (discarding a
successor is an error). And `confirm !opened` states an invariant: opening an
opened bottle is a failure, not a silent no-op. After `open()`, both bottles
exist, and the program can say which one it means.

A Java programmer would write this with a mutable field and a setter in fewer
characters. The Okkama version is longer by a word or two and removes a whole
category of question: "who else holds a reference to this bottle, and did they
expect it to stay closed?"

## 1.8 What the razor costs

A book that only praised its subject would not be worth reading, and the
razor itself demands that we count costs as carefully as benefits. Here are
the main ones; later chapters return to each.

- **More objects, more words.** Without setters and with successors, programs
  name more things: a new object per state, a role per capability. Okkama's
  standard library shows both sides — clear contracts, and occasionally
  constructions like `BigQuotient(left: a, right: b).value()` where a method
  would read better. That one became `a.divided(b)` in 0.5.5 (issue #190),
  and the review of the library looked for the rest (Chapter 22).
- **Performance of immutable updates.** A new object per change costs
  allocations; collection updates without in-place mutation cost copies.
  Repeatedly rebuilding a collection with `map` can be quadratic; use
  the construction operations of Chapter 8. The razor removed
  mutation; it must not remove efficiency, and the remedy is to add the right
  *operation*, not the mutable entity.
- **Unlearning.** Most programmers think with null, inheritance and mutable
  state. Okkama asks for a different habit of thought — the one *Elegant
  Objects* argued for in Java [@bugayenko2016] — and enforces it.
- **Things that are genuinely missing.** Some removals were made before their
  replacements were complete. The standard library audit of November 2026
  filed more than a hundred issues where the language or library lacks an
  operation that a smaller design needs — string searching, hashing for
  object keys, default parameters (issues #145–#155, #210–#216). The razor is
  not an excuse for those gaps; Chapter 27 lists them openly.

## 1.9 The razor at work: three decisions

A principle is easiest to judge by its verdicts. Here are three proposals
the language received, each weighed the same way: what entity it adds, what
work that entity does, and whether something already in the language could
do the work.

**Default bodies in roles: declined (issue #146).** The proposal let a role
carry code for some of its actions, so that a decorator would not have to
forward every action by hand. The entity is code inside a contract. The work
it does is real but small: it saves typing. Against it stands what the
entity brings back. A role with bodies is a base class by another name, and
two roles that supply a body for the same action need a rule for which one
wins, the question traits exist to answer (Chapter 14). An object that does
the shared work, held by the decorators that need it, already does the job.
Forwarding stays explicit, and Section 2.12 counts it among the costs.

**`once let`: accepted (issue #491).** Objects never change, so an answer
computed from their fields could be kept. Without a construct for it, a
program had three choices, and each broke another rule: compute in the
constructor, which would make constructors do work instead of binding and
checking their input; keep a cache in a mutable field, which the language
forbids; or compute again on every call. `once let x = expr;` does work
nothing else could do, and it was admitted in its narrowest form: it reads
only fields and constants, has no effects, holds no resource and takes no
part in equality (Section 13.13).

**Module constants: accepted, half of it (issue #153).** A module needs
names for its limits and tables. The proposal could have been read as
"module-level values", and that would have admitted global objects, static
fields and singletons: shared state with a lifetime nobody can see. The
language took the half that does the work and adds no state. `const` names
a value the compiler computes; an expensive object is still built in `main`
and passed to whoever needs it (Section 19.6).

The pattern is the same in all three. An entity is admitted when it does
work that no existing entity can do without breaking another rule, and then
only in the smallest form that does that work. "It would be convenient" is
an argument; it is weighed, not granted.

## 1.10 How to read this book

The book has six parts. Part I gives the philosophy and history of Okkama. Parts II–V teach the language from values to whole programs, with runnable examples, sources and limits. Part VI covers practice and judgement: real programs, comparisons with other languages and open problems.

Every program in the book lives in `docs/OkkamaBible/examples`, with expected output checked by `build.py --check`. Tables of builtin behaviour and the standard library are generated from code. A disagreement between the book and verified language behaviour is a book bug; please report it.

A note on citations: claims about the design are backed by references to the
work of other authors, listed in the bibliography at the end. They are there
not to borrow authority — a good argument stands on its own — but to show
where each idea comes from and where to read the full argument. Okkama
invented few of its ideas. What it adds is the decision to make them all
mandatory at once.
