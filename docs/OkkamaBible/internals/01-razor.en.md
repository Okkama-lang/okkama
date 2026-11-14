# Chapter 1 — archived implementation passages

Replaced mixed paragraphs are preserved in full for context.

## 1.5 The other side: worse is better

described 0.5.x

Gabriel's essay is not a rejection of the razor; it is a dispute about *whose*
simplicity counts. Worse-is-better shaves the implementation and lets the
user carry the complexity; the "right thing" shaves the user's model and lets
the implementation carry it. Okkama sits firmly on the second side: the
compiler, the verifier and the runtime do a great deal of work — exact
numbers that promote to big integers, precise garbage collection with
deterministic destruction, a bytecode verifier — so that the program text can
stay small and the set of things a reader must consider stays small.

## 1.6 Okkama's razor, entity by entity

described 0.5.x

Notice what the table does *not* contain. Okkama is not a minimal language in
the sense of Scheme or Forth. It has exact rationals and big integers, a
standard library that speaks HTTP/2 and PostgreSQL, structured concurrency,
precise collection, and a JIT compiler. The razor is applied to *kinds of
entities in a program*, not to the size of the toolbox. A language may have
many tools and few kinds of things; Okkama aims to be such a language.

## 1.8 What the razor costs

described 0.5.x

- **More objects, more words.** Without setters and with successors, programs
  name more things: a new object per state, a role per capability. Okkama's
  standard library shows both sides — clear contracts, and occasionally
  constructions like `BigQuotient(left: a, right: b).value()` where a method
  would read better (issue #190 proposes exactly that).
- **Performance of immutable updates.** A new object per change costs
  allocations; collection updates without in-place mutation cost copies.
  Sparrow makes allocation cheap and the compiler
  removes much of it, but some idioms are quadratic today — arrays have no
  append, so building one element by element copies it each time
  (issue #145). The razor removed
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
  not an excuse for those gaps; Chapter 28 lists them openly.

## 1.9 How to read this book

described 0.5.x

The book has seven parts. Part I, which you are reading, gives the
philosophy and the history that shaped Okkama. Parts II–V teach the language
from values to whole programs; each chapter explains a construct, shows it
working in examples you can run, cites the research and practice it builds
on, and ends with its limits. Part VI opens the implementation: the compiler,
the three engines, Sparrow with its JIT and garbage collector, and
the machinery that keeps it safe and fast. Part VII is about judgement —
building real programs, comparing Okkama with other languages, and the open
problems.

Every program in the book is in `docs/OkkamaBible/examples`, and every one is
checked: `build.py --check` runs it on the IR interpreter and on Sparrow and
compares the output with the text you read. Tables of builtin
behaviour and of the standard library are generated from the code. If the
book and the implementation ever disagree, the implementation is right and
the book has a bug — please report it.
