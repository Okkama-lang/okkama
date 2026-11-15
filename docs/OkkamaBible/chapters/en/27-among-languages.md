# Chapter 27. Okkama Among Languages

The earlier chapters compared Okkama with other languages one construct at a
time: numbers in Chapter 5, types in Chapter 4, collections in Chapter 8,
roles in Chapter 14, tests in Chapter 15. This chapter turns the comparison
around. It takes six languages that a reader of this book is likely to know,
Java, Kotlin, Go, Rust, Haskell and OCaml, and asks of each the same
questions the manifesto answered for Okkama (Chapter 3).

A comparison of languages is easy to make unfair. Each of these languages
was designed for a purpose, by people who weighed the same trade-offs and
chose differently, and each has decades of programs behind it that Okkama
does not. The aim here is not a ranking. It is to show where Okkama's
choices sit among choices that working languages have made, so that a
reader who knows one of them can see what is familiar, what is different,
and what is missing.

## 27.1 The rules side by side

| | Java | Kotlin | Go | Rust | Haskell | OCaml | Okkama |
|---|---|---|---|---|---|---|---|
| Absence | `null` in every reference type [@gosling2023] | nullable types `T?`, checked [@kotlinspec] | `nil` for pointers, interfaces, maps, slices [@gospec] | `Option` [@klabnik2023] | `Maybe` [@haskell2010] | `option` [@ocamlmanual] | `Maybe` (Chapter 7) |
| Change | mutable fields; immutable records | `val`/`var`; `copy` on data classes [@kotlindata] | mutable | immutable bindings, `mut` | immutable; `IORef`, `ST` | immutable bindings; mutable fields and `ref` | successors; `Cell` in synchronous code (Chapter 13) |
| Implementation inheritance | classes, single | classes, final unless `open` [@kotlininheritance] | none; embedding | none | none | classes, rarely used | none (Chapter 2) |
| Contracts | interfaces, with default methods | interfaces | interfaces, structural | traits | type classes | module signatures | roles, nominal, no bodies (Chapter 14) |
| Failures | checked and unchecked exceptions | unchecked exceptions [@kotlinexceptions] | error values, `panic` | `Result`, `panic` | `Maybe`, `Either`; exceptions in `IO` | exceptions, `result` | typed errors and panics (Chapter 10) |
| Resource release | `try`-with-resources | `use` [@kotlinuse] | `defer` | `Drop` | `bracket` [@hsexception] | `Fun.protect` [@ocamlfun] | `destroy`, `defer` (Chapter 11) |
| Integers | fixed width, wrap | fixed width, wrap | fixed width, wrap | panic in debug builds, wrap in release | `Int` bounded, `Integer` unbounded | fixed width, wrap [@ocamlint] | unbounded (Chapter 5) |
| Concurrency | threads; virtual threads [@jep444]; structured concurrency in preview [@jep453] | coroutines, structured [@kotlincoroutines] | goroutines and channels | threads; `async` with library runtimes | green threads, STM | domains [@ocamlparallel] and libraries | structured async, process workers (Chapter 20) |

Read across a row and Okkama is rarely alone. Rust, Haskell and OCaml share
its answer to absence; Go, Rust and Haskell share its refusal of
implementation inheritance; Haskell's `Integer`, the type its integer
literals default to, is unbounded like Okkama's `int`. Read down a column and the picture changes: no other language in
the table gives all of Okkama's answers at once, and that is the design's
one new idea (Section 1.10).

## 27.2 Java

Java is the language Okkama argues with most, because *Elegant Objects*,
the discipline Okkama turns into rules, was written for Java programmers
(Section 2.7). Almost every Okkama rule is something a Java team can adopt
by convention: final fields, no setters, no `null`, composition instead of
`extends`, small interfaces with fakes. What Java cannot give is the
guarantee. A convention holds as long as every reviewer catches every
exception to it; in Okkama the compiler does.

Java answers some of the same problems with newer features. Records are
immutable carriers of data [@gosling2023], close to what Section 28.4 calls
a shorthand for data objects. Virtual threads make blocking code cheap
[@jep444], and structured concurrency, still a preview in Java 21, gives
tasks a parent scope that waits for them [@jep453]. Java keeps `null`,
checked exceptions and class inheritance, and for a language with Java's
installed base, removing them is not an option. Okkama had no installed
base, which is the only reason it could start where Java's best practice
ends.

## 27.3 Kotlin

Kotlin took Java's runtime and changed its defaults: classes are final
unless declared `open` [@kotlininheritance], nullability is part of a type
and checked by the compiler [@kotlinspec], data classes come with
`copy` [@kotlindata], and all exceptions are unchecked
[@kotlinexceptions]. Its coroutines are structured: a coroutine belongs to
a scope that does not complete until its children do [@kotlincoroutines],
which is the model of Chapter 20.

The difference from Okkama is one of degree. Kotlin makes the safer choice
the easier one, and keeps the other within reach: `var`, `open`, `!!` and
platform types from Java are all available when a programmer wants them.
Okkama removes the other choice. A Kotlin team can write Okkama-style code;
an Okkama program cannot be written any other way.

## 27.4 Go

Go and Okkama agree on more than their surfaces suggest. Neither has
classes or inheritance, both treat errors as values that a function hands
back rather than throws past its caller, both release resources with
`defer` [@gospec], and both keep the language small. The
disagreements are just as basic. Go's interfaces are structural, satisfied
by any type with the right methods; Okkama's roles are declared (Section
14.2). Go's values are mutable and its `nil` reaches pointers, interfaces,
maps and slices. And Go's `go` statement starts a goroutine that nothing
waits for, the unstructured concurrency that Chapter 20 argues against.

Go chose simplicity of the language and left discipline to the programmer
and to tools such as linters. Okkama chose simplicity of the *program*: a
smaller set of things that can go wrong, bought with a stricter language.

## 27.5 Rust

Rust is the closest relative in purpose. It has no `null`, no inheritance,
values immutable unless declared `mut`, errors as `Result`, and resources
released by `Drop` when their owner goes out of scope
[@klabnik2023]. Its traits are nominal contracts, like roles, though they
may carry default methods. Where Rust differs is in what it asks of the
programmer to get there. Ownership and borrowing are checked at compile
time and written in the program: lifetimes, `&` and `&mut`, moves. The
reward is memory safety without a garbage collector and control over every
allocation.

Okkama makes a different trade. Its memory is managed automatically, and
deterministic destruction is reserved for resources (Section 11.1), so most
programs never mention ownership, and a resource is released by the scope
that made it without annotations. Rust also overflows: integer arithmetic
panics in debug builds and wraps in release builds [@klabnik2023], where
Okkama's integers grow (Chapter 5). A program that needs Rust's control of
memory should be written in Rust.

## 27.6 Haskell

Haskell is the purest language in the table: values never change, effects
are confined to types such as `IO`, absence is `Maybe`, and contracts are
type classes [@haskell2010]. Much of Okkama's vocabulary comes from this
line, through ML: enums with payloads and `match` (Chapter 16), `Maybe`,
roles that resemble type classes with declared instances (Section 14.9).

Two differences matter. Haskell infers types across a whole program, while
Okkama infers only inside bodies and asks for every declaration to be
annotated (Section 4.5). And Haskell is lazy by default: an expression is
evaluated when its value is needed, which makes some programs elegant and
the cost and the order of evaluation harder to see. Okkama is strict, and
its order of evaluation is fixed and written down (Section 9.7). Haskell's
purity is enforced by types; Okkama's immutability is enforced by the
absence of assignment, and its effects are ordinary calls on objects that
carry the authority for them (Chapter 21).

## 27.7 OCaml

OCaml is ML with objects and an industrial track record. Bindings are
immutable, absence is `option`, failures are exceptions or `result`, and
modules with signatures and functors are its main tool for abstraction
[@ocamlmanual]. It also has a class system with inheritance, which most
OCaml code does not use, and mutable record fields and references when a
program wants them. Its integers have a fixed width and wrap on overflow
[@ocamlint]. Since OCaml 5, domains run in parallel on several cores
[@ocamlparallel].

OCaml shows that an expression-oriented, immutable-by-default language with
algebraic data types can be fast and practical. It keeps the escape
hatches, mutable fields and classes, that Okkama removed, and it has the
recursive variants and nested patterns that Okkama does not have yet
(Section 28.2).

## 27.8 A port, step by step

The rules are easiest to compare on one small class. Here is a bank account
in Java, written the way a Java course would write it:

```java
import java.util.HashMap;
import java.util.Map;

public class Account {
    private final String owner;
    private long cents;

    public Account(String owner, long cents) {
        if (cents < 0) throw new IllegalArgumentException("negative balance");
        this.owner = owner;
        this.cents = cents;
    }

    public void withdraw(long amount) {
        if (amount > cents) throw new IllegalStateException("overdrawn: " + cents + " < " + amount);
        cents -= amount;
    }

    public String shown() { return owner + " " + cents; }

    public static void main(String[] args) {
        Map<String, Account> accounts = new HashMap<>();
        accounts.put("ada", new Account("ada", 1000));
        Account ada = accounts.get("ada");
        ada.withdraw(300);
        System.out.println(ada.shown());
        Account bob = accounts.get("bob");
        System.out.println(bob == null ? "no bob" : bob.shown());
        try {
            ada.withdraw(5000);
        } catch (IllegalStateException e) {
            System.out.println("refused: " + e.getMessage());
        }
        System.out.println("in the map: " + accounts.get("ada").shown());
    }
}
```

Run with Java 26, it prints:

```text
ada 700
no bob
refused: overdrawn: 700 < 5000
in the map: ada 700
```

The same program in Okkama:

{{example:27-port}}

Each change in the port is a decision that the Java version left implicit.

- **The balance is no longer a mutable field.** `withdraw` becomes
  `withdrawn`, a successor (Chapter 13). The Java map shows the changed
  balance, `ada 700`, because the map and `ada` share one mutable object.
  The Okkama map still holds the account it was given, `ada 1000`, and the
  new state reaches the map only when the program puts it there:
  `accounts + ["ada": ada]`. A port has to decide, at every mutation, who
  should see the change.
- **The missing key is no longer `null`.** `accounts.get("bob")` returns
  `null`, and the code must remember to check it. In Okkama a missing key is
  a failure unless the lookup says what to do: here `accounts.has`, or
  `accounts["bob"] else …`, or a `Maybe` (Chapter 7).
- **The exceptions become a vocabulary.** `IllegalArgumentException` and
  `IllegalStateException` say that something is wrong; the enum `Refused`
  says what: `negative` and `overdrawn(balance, wanted)`. The caller catches
  the case it handles and reads its payload instead of parsing a message
  (Chapter 10).
- **The constructor keeps its check.** Java's `if … throw` becomes
  `confirm … else throw`, and in neither language can an account exist with
  a negative balance.

What the port did not need: getters (`shown()` is the one question the
program asks), a `final` keyword (every field is final), and a check that
`owner` is not null (a `string` cannot be absent). The port is about as long
as the original; it says more about what can happen.

## 27.9 What the comparison shows

Three things stand out.

- **Okkama's rules are not new.** Each comes from a language in this table
  or from the literature behind it. What is new is that all of them hold at
  once, in one language, by rule rather than by convention.
- **The other languages keep exits.** Kotlin's `var`, Rust's `unsafe`
  [@klabnik2023],
  OCaml's mutable fields, Java's `null`: each language with a safer default
  also keeps the less safe choice for the programs that need it. Okkama
  has fewer exits: `Cell` for synchronous code, `Atomic` for state shared
  between tasks, and natives for the host. A program that needs another one
  is a program Okkama does not serve well, and Chapter 28 lists where that
  shows.
- **Maturity is the largest difference.** Every language in the table has
  libraries, tools, books and production systems built over years or
  decades. Okkama has a conformance suite, a standard library being
  rewritten, and this book. A comparison of designs is only half of a
  comparison of languages.
