# Chapter 14. Roles: nominal contracts and composition

Without inheritance, two things that other languages get from a base class
have to come from somewhere else. The first is a contract: "anything that
can be opened and labelled". The second is shared behaviour. Okkama
answers the first with **roles** and the second with composition. This
chapter is about both, and about the third thing a role does that most
interfaces do not: it limits what its holder may do.

{{example:14-roles}}

- `Package` is a role: two signatures and no bodies.
- `Bottle` and `Crate` each declare `does Package` and implement both
  actions.
- `Shelf` holds `@Package`. It never knows which objects it holds.
- `opened()` is a successor on the role. Each object implements it with its
  own `^Bottle` or `^Crate`, and the caller gets a `^Package` back.
- `poured()` exists on `Bottle`, but nothing holding a `Package` can call
  it.

## 14.1 Declaring a role

```okkama
role Package {
    label() is string;
    opened() is ^Package;
}
```

A role is a list of action signatures: names, parameters and results. It
has no fields, no constructors and no bodies. (The one exception is a
`fake` block for tests, the subject of Chapter 15.) A role may declare
several signatures with one name, as long as their parameter types differ
and their results agree.

An object implements a role by saying so:

```okkama
object Bottle does Package { … }
object Wallet does Account, Labelled { … }
```

The compiler checks the claim:

- every action of the role exists on the object;
- parameter types, and the fixed or variadic shape, match *exactly*;
- result types are compatible: `^Bottle` satisfies `^Package` because
  `Bottle does Package`.

"Exactly" means exactly. A role action that takes `int` is not implemented
by one that takes `float`, even though the float version could accept every
integer:

{{example:14-mismatch-rejected}}

Function subtyping in type theory would allow it, since parameters are
contravariant [@pierce2002]. Okkama keeps parameters invariant for the same
reason it keeps overloads strict (Chapter 13): a call through the role and
a direct call must select the same code, with no conversion in between.

## 14.2 Nominal, not structural

An object that has the right actions but does not declare the role does not
satisfy it:

{{example:14-nominal-rejected}}

`Invoice` has `label() is string`, exactly what `Presentation` asks for.
It still cannot be used as one, because nobody said it was. Go makes the
opposite choice: a type satisfies an interface whenever its method set
contains the interface's methods, with no declaration at all [@gospec].
TypeScript does the same. Java, C# and Rust are nominal: a class
`implements` an interface, a type has an `impl Trait` [@klabnik2023]. Pierce
sets out the trade-off: structural typing is more flexible, while nominal
typing makes the type names carry intent and gives the checker
something to report against [@pierce2002].

Okkama's reason is the one behind Liskov and Wing's behavioural subtyping.
A contract is more than a set of signatures. It includes what the actions
*mean*: their preconditions, their results, the states they lead to
[@liskov1994]. Two `label()` actions with the same signature may promise
different things. `does Presentation` is the author's statement that this
object keeps the role's promises, and the compiler holds them to the parts
of it that it can check.

The cost is ceremony at the boundary between modules. An object written
before a role existed cannot satisfy it after the fact. A role can only be
implemented in the module that declares the object. To use a foreign
object as a role, wrap it in a small object that `does` the role and
forwards to it (§14.6).

## 14.3 A role reference limits what its holder can do

In Java, a variable of an interface type can be cast back to the class,
and `instanceof` tells which class it is. In Okkama a role-typed value is a
*restricted reference*. It points to the same object, nothing is copied,
and only the role's actions are reachable through it:

{{example:14-restricted-rejected}}

An object becomes such a reference only where the role is already the
expected type: a typed `let`, a field, a parameter of an action or of a
builtin (`shapes.contains(Square(side: 2))` on a `@Shape`), a result, the
fallback of a `Maybe<Shape>` or of `find()`, and the branches of an `if`,
`match` or choice whose value has a declared role type. Okkama never looks
for a role two values happen to share. `shape == Square(side: 2)` is
rejected: the operands keep their exact types, and the comparison goes
through a role-typed binding. `if c { Rect(w: 1, h: 2); } else { Square(side: 2); }`
with no declared type is rejected too, even though both objects do `Shape`.
The conversion happens where the program states the contract, and nowhere
else.

There is no way back to the concrete type:

{{example:14-downcast-rejected}}

The restriction holds through everything a reference can pass through:

- aliases, parameters, fields, results and collections;
- bound actions, `Maybe` and `Presence`, enum payloads;
- generic inference: a `T` inferred from a role reference is the role, not
  the hidden class;
- boxing into `any`. `any` never narrows, so the box keeps the
  restriction.

The implementation still runs with its full `self`. `Bottle.label` reads
`volume` and `open` as usual. The restriction is on the *holder*, not on
the object.

The restriction is shallow. A result of an allowed action is accessible
according to its own declared type. If `Package` had an action returning
`Bottle`, the holder could use that `Bottle` fully. A role restricts
exactly what its signatures expose.

This makes a role reference a capability in the sense of object-capability
security. Holding a reference is the permission to use it, and handing out
a narrower reference hands out fewer rights. That is attenuation, Mark
Miller's term for building least authority into a program
[@miller2006; @saltzer1975]. A function that receives a `Labelled` can print
a label. It cannot learn the balance, even with a cast, because the
language has no cast to try.

Exceptions do not reopen the restriction either. Objects cannot do `Error`:

{{example:14-error-rejected}}

Errors are enums (Chapter 10), so a `catch` never names an object type and
can never recover a concrete object from a role.

## 14.4 Moving between roles

A role reference may cross into another role on two conditions:

- the underlying object *nominally* does the target role;
- every action of the target role is already reachable through the current
  role.

That is narrowing: going from more rights to fewer.

{{example:14-narrowing}}

`Account` has `label` and `balance`; `Labelled` has only `label`. A
`Wallet` does both, so an `Account` reference passes as `Labelled` to
`Printer.print`, and binds to a `Labelled` local. The reverse is widening,
and it is rejected even though the object behind the reference does
`Account`:

{{example:14-widen-rejected}}

If `Wallet` did only `Account`, the narrowing would be rejected too, with
the same message for `Labelled`. A role is a nominal contract, and a
subset of signatures is not a contract the object signed.

## 14.5 Successors through a role

A role may declare successor actions. `opened() is ^Package` says: calling
this gives the next state, and it is still a `Package`. Each implementation
returns its own type, `^Bottle` or `^Crate`. The caller holding a
`Package` receives a restricted `^Package`. The must-use rule of
Chapter 13 still applies through the role.

On an object, a successor always returns the object's own type. `is
^Package` on `Bottle` is an error. The role decides what a caller sees; the
object decides what it builds.

Successors on roles make roles into **state views**. Give each state of a
protocol its own role, exposing only what is valid in that state, and
let each transition return the role of the next state:

{{example:14-states}}

- `Payments.open` returns a `Created`. The only things a caller can do with
  it are `id()` and `authorize(...)`.
- `authorize` returns an enum whose two cases carry *different* views: an
  `Authorized` or a `Declined`. The `match` must handle both.
- `capture()` exists only on `Authorized`, and returns `^Captured`.
  `refund()` exists only on `Captured`.
- One immutable object, `Payment`, implements all five roles. A private
  `stage` field records the real state, and each transition checks it with
  `confirm`.

Skipping a state does not compile:

{{example:14-state-rejected}}

Strom and Yemini named this idea **typestate** in 1986: the operations
allowed on a value depend on its state, and the compiler tracks the state
[@strom1986]. Typestate in Rust and similar languages relies on move
semantics, so that an old state cannot be used after the transition.
Okkama does without them. The old state is an immutable object and remains
valid: `authorized.capture()` can be called twice on the same
`Authorized`, and each call returns a `Captured`. Whether the second
capture should succeed is a question about the outside world (has the
money moved?), and it belongs in durable state with idempotency keys, not
in the type system. The `confirm` in each transition is the second line of
defence inside the module.

In a real program, `Payment` and `Stage` are declared `hidden`, so that
other modules cannot name the class at all. They only ever hold one of the
five views.

## 14.6 Composition instead of inheritance

A role is a contract; it carries no code. Shared behaviour comes from
objects that hold other objects. The **decorator** is the basic form: an
object that does a role, holds another implementation of the same role,
and adds one behaviour around it [@gamma1994].

{{example:14-decorators}}

`Catalog` knows prices. `Discounted` takes 15 percent off whatever its
`origin` says. `Rounded` rounds down to ten. Each is a few lines, each can
be tested alone, and they combine in any order. The caller holds a
`Pricing` and cannot tell how many layers it has. `sale.price("tea")` goes
through three objects: 345, then 294, then 290.

Compare a class hierarchy for the same job: `Catalog`, `DiscountedCatalog`,
`RoundedDiscountedCatalog`. Every new behaviour multiplies the classes, and
every subclass depends on the details of its parent, which is the fragile
base class problem of Chapter 12. "Favor object composition over class
inheritance" is the Gang of Four's own advice [@gamma1994]. Okkama makes it
the only option. Elegant Objects leans on such decorators throughout:
small objects, each wrapping another behind a shared interface
[@bugayenko2016].

The cost is forwarding. A role with ten actions needs ten forwarding
actions in every decorator, even when the decorator changes one of them.
Okkama has no delegation shorthand ("implement `Pricing` by forwarding to
`origin`"). The forwarding code is plain to read and tedious to write.
Keeping roles small keeps it short: a role with one or two actions is the
common case in the standard library.

## 14.7 How a role call runs

A role boundary and a role call are explicit in the IR:

```text
%19: Bottle = call @Bottle.new#0(%18)
%20: Package = role.wrap %19
%21: Package = call.role %20, Package.opened()
%22: str = call.role %21, Package.label()
```

`role.wrap` marks the boundary where a `Bottle` becomes a `Package`.
`call.role` calls slot `opened` of `Package` on whatever object is behind
the reference. Each object's declaration lists, for every role it does, the
function that fills each slot:
`does Package { label = @Bottle.label, opened = @Bottle.opened }`.

The engines differ in how they find that function:

- **The IR engine** reads the receiver's declaration and looks up its
  witness for the role on each call.
- **The Sparrow core** compiles `role.wrap` to nothing: a role reference is
  the same pointer. `call.role` loads the object's header, shifts out the
  descriptor number, and indexes a table built once per role slot. The
  table holds one function per object type. Then it calls that function.
  Compared with a direct call, that is a few loads and an indirect call.
- **The JVM backend** turns each role into a Java interface. The classes of
  objects that do it implement the interface, `role.wrap` is again a no-op,
  and `call.role` is an `invokeinterface`. The JVM's inline caches and
  speculative inlining then treat a call site that sees one or two object
  types like a direct call.

In none of them does the restriction cost anything at run time. It is
checked when the program is compiled and verified, and the reference is
the plain object.

## 14.8 Operators are roles

Arithmetic on objects goes through roles as well. The standard module
`operators` declares one role per operator (#215). The first type parameter
is the right operand, the second the result:

| Role | Action | Operator |
|---|---|---|
| `Adding<T, R>` | `plus(other is T) is R` | `a + b` |
| `Subtracting<T, R>` | `minus(other is T) is R` | `a - b` |
| `Multiplying<T, R>` | `times(other is T) is R` | `a * b` |
| `Dividing<T, R>` | `divided(other is T) is R` | `a / b` |
| `Remaindering<T, R>` | `remainder(other is T) is R` | `a % b` |
| `Negating<R>` | `negated() is R` | `-a` |

{{example:14-operators}}

With an object on the left, `a + b` is the call `a.plus(b)` and nothing
more. The compiler rewrites the operator into that call and checks it like
any other call: the right operand gets no conversion, and the overload it
selects must be a role instance the type declares. The engines never see an
operator on an object. The orderings `<`, `<=`, `>` and `>=` use the role
that already existed for them, `compare.Comparing<T>`: `a < b` is
`a.order(b) == Order.less`. `==` stays structural (Chapter 12).

The roles are nominal, like every role (§14.2). An action named `plus` does
not give an object `+`:

{{example:14-operator-undeclared-rejected}}

One role may be declared several times with different arguments. `Instant`
does `Subtracting<Duration, Instant>` and `Subtracting<Instant, Duration>`:
an instant minus a duration is an instant, an instant minus an instant is a
duration. Its two `minus` actions are overloads with different results,
which §13.3 allows because their parameter types differ. In the standard
library `BigInt` does all six roles and `Comparing`; `Duration`, `Instant`
and `MonotonicInstant` do the ones time arithmetic needs.

Other languages find operators in one of three ways. C++ and Python look
them up by name: a function `operator+` [@stroustrup1994], or a method
`__add__`. Haskell puts them in a class, and `Num` bundles `+`, `-`, `*`,
`negate`, `abs`, `signum` and `fromInteger` together [@haskell2010]. A type
that can be added but not multiplied, such as a duration, cannot be a `Num`
honestly. Rust has one trait per operator, `Add`, `Sub`, `Mul`, each with
the right operand as a parameter and the result as an associated type
[@klabnik2023]. Okkama is closest to Rust: a role per operator, the result
spelled out, conformance declared. Lookup by name was ruled out because
nothing in a name says that `plus` means addition, which is the argument
of §14.2. One class for all arithmetic was ruled out because a contract
should not ask for actions the object cannot keep.

## 14.9 Roles elsewhere

| | Okkama role | Java interface | Go interface | Rust trait |
|---|---|---|---|---|
| Conformance | declared (`does`) | declared (`implements`) | implicit, by method set | declared (`impl`) |
| Default method bodies | no | yes | no | yes |
| Downcast from the contract | impossible | cast, `instanceof` | type assertion, type switch | only through `Any` |
| Parameter matching | exact | exact (overriding) | exact | exact |
| Successor (next-state) results | yes (`^Role`) | no | no | a method that consumes `self` and returns a new type |
| Conformance of a foreign type | no; wrap it | no | automatic, if the methods match | if the trait or the type is local |
| Operators on user types | through roles (`Adding`, …) | no | no | through traits (`Add`, …) |

Two rows matter most. **Default bodies**: Java and Rust let an interface
carry code, which brings back part of inheritance, including the question
of which default wins when two interfaces supply one. A role carries none;
shared code lives in an object. **Downcasting**: every other language in
the table has a way back from the contract to the concrete type. Okkama
has none, and that is what makes a role reference a capability rather than
a type annotation.

## 14.10 What this design costs

- **Forwarding code.** Decorators repeat every action of the role. No
  delegation syntax shortens it (§14.6).
- **No retroactive conformance.** A role cannot be added to an object from
  outside its module. The usual fix, an adapter object, is one more object.
- **No downcast, ever.** Code that "just needs to check what this really
  is" has to be redesigned: an action on the role that answers the
  question, or an enum that carries the alternatives.
- **Invariant parameters.** An implementation cannot accept more than the
  role asks for. Usually it does not need to.
- **Operators look only to the left.** `price * 3` works; `3 * price` does
  not, because `int` cannot be given a role. Python asks the right operand
  through `__rmul__`, and Rust allows `impl Mul<Money> for i64` in Money's
  crate. Okkama has neither (no retroactive conformance, above).
- **One dispatch per call.** A role call is an indirect call. In hot loops
  over a single type, holding the concrete type avoids it; the JVM and its
  inline caches recover most of the difference.

In return:

- a contract states intent, not an accident of shape;
- a reference can be narrowed, and narrowed is final;
- behaviour composes from small objects instead of growing down a
  hierarchy;
- protocols can be typed by state, with no linear types.
