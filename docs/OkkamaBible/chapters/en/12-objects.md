# Chapter 12. Objects, fields and constructors

Part IV is about objects, and Okkama has nothing else to put behaviour in.
There are no free functions, no static methods, no classes with class-level
state and no inheritance. A program is objects calling objects, and a
`main` that creates the first of them. Chapter 2 told the history that
led here, and Chapter 3 the rules. This chapter shows what an object is in
the language: its fields, its constructors, its actions and the few things
it may not do.

{{example:12-objects}}

The rest of the chapter takes that program apart:

- `Cash` holds three fields and never changes them;
- it has two constructors, one of which checks its input;
- it exposes behaviour (`total`, `label`), never its fields;
- a private helper sits below the `private:` line;
- two `Cash` values with the same fields are equal;
- `Bag` shows an empty and a variadic constructor side by side.

## 12.1 Declaring an object

```okkama
object Cash {
    has digit is int;
    has cents is int = 0;
    has currency is string = "USD";
    Cash(digit is int, cents is int) { … }
    total() is int { … }
    private:
    in_cents(units is int) is int { … }
}
```

An object declaration has a name and a body, and nothing else: the header
takes no parameters, because state is declared only by `has`. The name is a
type name and starts with an upper-case letter. Two objects of one module
cannot share a name. The body may hold:

- `has` fields;
- constructors, at least one;
- public actions;
- a `private:` section of private actions;
- at most one `destroy` (Chapter 11);
- `cap` contracts (§12.8);
- test blocks.

The order of these parts does not matter to the compiler. The style used in
the standard library and in this book puts fields first, then constructors,
then the public actions, then `private:`.

An object is not a class in the Java sense. There is no `extends`, no
`super`, no `protected`, no abstract or virtual method, no static member.
Sharing behaviour between objects is done by composition: an object holds
another and calls it. Sharing a contract between objects is done with roles
(Chapter 14). The rationale is Chapter 2's: implementation inheritance
couples a subclass to every detail of its superclass, and the fragile base
class problem is the result [@mikhajlov1998]. Composition couples it only
to a contract.

## 12.2 Fields are private and permanent

`has` is the only way to declare state.

```okkama
has digit is int;                 // required: a constructor must bind it
has cents is int = 0;             // optional: a default
has given, family is string;      // several names, one type
```

Two rules make fields different from fields in most languages.

**A field is private to its object.** Code outside the object cannot read
it, not even with the same type in the same module:

{{example:12-field-rejected}}

The message says what to do instead: call an action. That is the Elegant
Objects rule "no getters" turned into a compile error [@bugayenko2016]. A
caller who needs something derived from the state asks the object for the
*behaviour* (`total()`, `label()`), and the object decides how to compute
it. The idea is Parnas's information hiding: a module's data
representation is the decision most likely to change, so it is the one to
hide [@parnas1972].

**A field never changes.** Outside a constructor there is no assignment to a
field at all:

{{example:12-assign-rejected}}

The fix the message suggests is the subject of Chapter 13: an action that
returns a *successor*, a new object with updated fields. A value is the
same value for as long as anyone holds it. Concurrent tasks can share it
without a lock, and a caller cannot be surprised by a change made behind
its back.

Inside the object, a field is read by its bare name (`cents`) or as
`self.cents`. A parameter or a local may shadow the bare name, and
`self.cents` then still means the field. Field defaults are evaluated for
each construction, before the constructor body, in the scope of the
declaration: a default may use literals, other declarations and imports,
but not the caller's locals and not other fields of the object being built.

## 12.3 Constructors are explicit

Every object declares at least one constructor, even an object without
fields. None is generated from the fields:

```okkama
object Marker {
    Marker();
}
```

An object with fields and no constructor is an error, `Object "Marker" must
declare a constructor`, even if nothing ever constructs it. The rule is
deliberate. A generated constructor would make the field list part of every
caller's code, and adding a field would silently change how the object is
made.

A **full constructor** has parameters and a body. The body binds fields, with
`field = value;` or `self.field = value;`, and may run any other statement:

```okkama
Cash(digit is int, cents is int) {
    self.digit = digit;
    self.cents = cents;
    confirm digit * 100 + cents >= 0 else throw InvalidCash.negative(digit * 100 + cents);
}
```

- A field bind is legal only inside a constructor body and its nested
  blocks. It is not an assignment: each field is bound once, and there is
  no other kind of assignment in the language.
- Every field must be bound or defaulted by the end of the constructor.
  Otherwise the compiler reports `Object "Cash" field "cents" was not
  initialized`.
- The call uses positional or named arguments, never a mix:
  `Cash(3, 25)` or `Cash(digit: 3, cents: 25)`. Positional arguments bind to
  the constructor's *parameters*, not to the order of the fields.

## 12.4 Shorthand constructors

Most constructors only copy their arguments into fields. The shorthand says
so in one line:

```okkama
Cash(:digit, :currency);
```

`:digit` names a field, and the constructor takes an argument of the field's
name and type and binds it. The list is a closed allow-list:

- **Named arguments only.** `Cash(digit: 2, currency: "EUR")` works, and
  `Member(7)` is rejected with `No constructor of "Member" accepts (int)`
  before any argument is evaluated.
- **Unlisted fields cannot be supplied.** They take their defaults, or the
  constructor's body must bind them.
- **A bodyless shorthand must complete the object.** If an unlisted field has
  no default, the declaration is an error: `Constructor shorthand for
  "Member" cannot initialize required field "name"`.
- **A shorthand may have a body.** The listed fields are bound first, and
  then the body runs. It may bind the remaining fields and check the input:
  `Score(:value) { confirm value >= 0 else throw InvalidScore.negative(value); }`.

`Marker();` is the shorthand with no fields, the usual constructor of a
stateless object.

## 12.5 Several constructors

An object may have several constructors, and they overload like actions:
by the number of arguments, their types, and, for shorthands, by the names
supplied. `Cash` has a full constructor taking `(int, int)` and a shorthand
taking `digit` and `currency` by name, and each call picks the one that
fits. Two candidates that fit equally well are an ambiguity error.
Declaration order never breaks a tie.

A full constructor may end with a **variadic** parameter, as `Bag(items is
...int)` does. Its calls are positional; inside the body the parameter is
unpacked with `items...`. A fixed constructor that fits a call wins over the
variadic one, so `Bag()` takes the empty constructor and `Bag(1, 2, 3)` the
variadic one.

There is no constructor chaining, no `this(...)` call from one constructor to
another. Each constructor binds and checks on its own, and checks are not
inherited between overloads. When two constructors share logic, the shared
part is an object of its own, or a private action both call.

## 12.6 Validation belongs in the constructor

An object should not exist in an invalid state. The constructor is the only
place that can guarantee it, so validation goes there:

```okkama
confirm digit * 100 + cents >= 0 else throw InvalidCash.negative(digit * 100 + cents);
```

The last line of the opening example shows the result:
`Cash(-1, 50)` throws `InvalidCash.negative(-50)` and no `Cash` is ever made.
Everything after the constructor can rely on the invariant without checking
it again. This is the Elegant Objects advice "fail fast in constructors" and
Meyer's class invariant from Design by Contract, the condition every
instance satisfies between public calls [@meyer1997]. Since Okkama objects
never change after construction, the invariant only has to be established
once.

When a constructor fails, its `defer`s run and the resources it already
owned are released. The unfinished object is never destroyed as if it were
complete (Chapter 11, §11.7).

How much work belongs in a constructor? The standard library keeps
constructors cheap: bind, normalise, check. Expensive or effectful work —
reading a file, parsing a document, opening a connection — happens in an
action, often the first one called. `ParsedIni(text)` stores the text and
checks its limits. Parsing happens in `value()`, so constructing a
`ParsedIni` never fails for a syntax error in the document. This keeps
construction predictable: it validates what it was given and does nothing
else.

## 12.7 Actions and private actions

Public actions are the object's interface: `total() is int { … }`. Their full
treatment, with overloading, default arguments and successors, is Chapter
13. Two points belong here.

- **Calls to the object's own actions go through `self`.** Inside `Cash`,
  `self.in_cents(digit)` calls the helper. A bare `in_cents(digit)` is
  `Undefined action "in_cents"`. Okkama has no free functions, so a bare
  call would have nothing to refer to.
- **`private:` starts the private section.** Everything below the line is
  callable only by the object itself. A private action is an implementation
  detail, free to change; a public one is a promise.

## 12.8 Caps: a read-only view of an object

Sometimes a caller should see a few derived facts about an object and
nothing else. Passing the whole object hands over all its behaviour. A
**cap** is a named contract of computed fields that the object grants
explicitly:

{{example:12-cap}}

- `cap BusinessCard { full_name is string { … } }` declares the computed
  fields. Their bodies read the owner's fields by name, but have no `self`
  and cannot call the owner's actions.
- `customer::BusinessCard` acquires the view, and its type is
  `Customer::BusinessCard`. A function that takes that type, like
  `Printer.print`, can read `card::full_name` and `card::initials`. It cannot
  call `charge`, read `card_number`, or turn the view back into a
  `Customer`.
- A cap is computed on every read; nothing is cached. It cannot hold state,
  declare constructors or actions, or be awaited.

Caps are the object-oriented answer to "I need a getter for this one
caller". The answer is not to open the field to everyone, but to grant a
named, typed view to whoever is handed it.

## 12.9 Objects are values

Two `Cash` objects with the same fields are equal: `Cash(1, 0) == Cash(1, 0)`
is `true` in the opening example. Okkama compares objects structurally, field
by field, without calling any of their actions. An object has an identity
too: it decides who owns a resource (Chapter 11), and a successor gets a
new one (Chapter 13). But identity is not what `==` compares. There is no
`equals` to override and no `hashCode` to keep consistent with it.

That matches how immutable objects are used. A value that cannot change is
characterised by what it holds. Two copies of the same amount of money are
the same amount, wherever they were made. It also makes value objects usable
as map keys: a `%Card:int` keys on the card's fields, ordered by them, so
`more[Card(10, Suit.hearts)]` finds an entry made with another `Card` of
the same rank and suit. Objects that should compare by something else, such
as a business key, expose an action for it, and the map keys on that
action's result instead.

## 12.10 What this design costs

- **More objects.** Without free functions, a small computation that would
  be a function elsewhere becomes an object with one action, or an action
  on an existing object. The standard library is full of small objects:
  `ParsedInt`, `Utf8Decoded`, `HexEncoded`. Each is a noun for one
  computation. It reads well once one is used to it, and it is verbose
  until then.
- **No inheritance.** A family of similar objects shares code by
  composition and contracts by roles. The flexibility is greater, and so is
  the amount of forwarding code (Chapter 14).
- **Constructors are written by hand.** No record syntax generates them.
  Shorthands make the common case one line, and every other case is
  explicit.
- **No mutation.** An object that would naturally change — a counter, a
  buffer — becomes a chain of successors, or holds its changing part in a
  dedicated mutable cell from the library (`Atomic` in `async/atomic`).
  Chapter 13 shows the first, Chapter 20 the second.

In return:

- every object is valid from the moment it exists;
- it can be shared freely and compared by value;
- its fields can change their representation without touching a single
  caller.
