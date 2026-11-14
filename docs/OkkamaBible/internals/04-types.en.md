# Chapter 4 — archived implementation passages

Replaced mixed paragraphs are preserved in full for context.

## 4.1 What a type is for

described 0.5.x

- **To reject meaningless programs** before they run — the examples of Chapter 3
  were all rejected this way.
- **To document intent.** A signature such as `share(people is int) is rat`
  tells a reader what goes in and what comes out without reading the body.
- **To make the runtime fast.** Sparrow chooses a machine
  representation for every register from its type — a raw 64-bit integer, a
  double, a pointer the garbage collector traces — which is only safe because
  the types are checked (Chapter 25).

## 4.7 What the compiler checks, and what remains for run time

described 0.5.x

The boundary between the two lists has moved over time. The first Okkama
implementation, an interpreter in Perl, checked most types while running;
the Rust compiler checks most of them statically, and the remaining dynamic
checks are concentrated where values, not types, decide.
