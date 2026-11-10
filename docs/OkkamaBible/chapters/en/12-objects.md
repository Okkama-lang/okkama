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
the same rank and suit. A key object must be a value through and through:
every field a scalar, an enum or another such object, and no `destroy` and
no caps. `%Tagged:int` for an object with a `tags is @string` field is
rejected: `Map key type Tagged cannot be a key: field "tags" is @string`.
Objects that should compare by something else, such as a business key,
expose an action for it, and the map keys on that action's result instead.

## 12.10 What a constructor compiles to

The compiler turns each constructor into an IR function marked `[ctor]`.
`okkama ir` prints them. For a `Point` with a checked full constructor and
a shorthand:

```okkama
object Point {
    has x is int;
    has y is int = 0;

    Point(x is int, y is int) {
        self.x = x;
        self.y = y;
        confirm x >= 0 else throw InvalidPoint.negative(x);
    }
    Point(:x);

    sum() is int { x + y; }
}
```

the two constructors come out as follows:

```text
func @Point.new#0(%0: int, %1: int) -> Point [ctor Point] {
^b0:
    %2: int = call @Point.default.y()
    %3: int = const 0
    %4: bool = int.ge %0, %3
    cond %4, ^b1, ^b2
^b1:
    %5: Point = obj.new Point { x: %0, y: %1 }
    ret %5
^b2:
    %6: int = const 0
    %7: array<int> = array.new [%0]
    %8: InvalidPoint = call @InvalidPoint.new(%6, %7)
    throw %8
}
func @Point.new#1(%0: int) -> Point [ctor Point] {
^b0:
    %1: int = call @Point.default.y()
    %2: Point = obj.new Point { x: %0, y: %1 }
    ret %2
}
```

Four things are visible here.

- **Overloads are separate functions.** `new#0` and `new#1` are the two
  constructors. The overload was chosen at compile time, so a call site
  names one of them directly: `call @Point.new#0(%1, %2)`.
- **A field bind is not a store.** `self.x = x` emits no instruction. The
  compiler keeps the bound value in a register, and the object is made by
  one `obj.new` with every field at once. That is how "each field is bound
  once" is enforced cheaply: there is no half-filled object to write into.
- **The object exists only after the checks.** `obj.new` sits in the branch
  where `confirm` held. On the other branch there is no `Point` at all, only
  the `InvalidPoint` being thrown. "No `Cash` is ever made" (§12.6) is meant
  literally.
- **Defaults are functions.** `@Point.default.y` is called on each
  construction, before the body, as §12.2 says. The full constructor calls it
  too, although its body binds `y` itself.

Reading a field is `obj.get %0, x`. The action `sum` is a function that takes
the object as its first parameter:

```text
func @Point.sum(%0: Point) -> int [method Point] {
^b0:
    %1: int = obj.get %0, x
    %2: int = obj.get %0, y
    %3: int = int.add %1, %2
    ret %3
}
```

## 12.11 Handing `self` out of a constructor

A constructor cannot call its own actions. `self.initials()` inside the
constructor is rejected, even after the fields that `initials` reads are
bound:

{{example:12-self-call-rejected}}

An action may rely on everything the constructor establishes, including the
checks that have not run yet. The compiler cannot see which fields an
action reads through the actions it calls, so it forbids the call outright.

The constructor *may* pass `self` itself to a collaborator. This is the
only way two immutable objects can refer to each other: the second one is
made while the first is still being built.

{{example:12-escape}}

`Leaf(up: self)` receives a `Tree` whose `leaf` field is not bound yet. The
compiler handles this with a second form of construction. When `self` is
used as a value, the object is created at that point, with the unbound
fields left undefined. Each later bind becomes an `obj.init` of the same
object:

```text
func @Tree.new#0(%0: str) -> Tree [ctor Tree] {
^b0:
    %1: int = const 0
    %2: $variant_Maybe_2 = undef
    %3: Tree = obj.new Tree { name: %0, leaf: %2 }
    %4: Leaf = call @Leaf.new#0(%3)
    %5: array<Leaf> = array.new [%4]
    %6: $variant_Maybe_2 = call @$variant_Maybe_2.new(%1, %5)
    obj.init %3, leaf, %6
    ret %3
}
```

The collaborator holds an unfinished object, and the rule from the start of
this section follows it there. The specification forbids calling *any*
action on a receiver whose construction is pending, "through aliases, role
references, bound actions and collaborators", and requires the run time to
reject such a call before evaluating its arguments. `Leaf` above only
stores `up`; calling `up.describe()` in `Leaf`'s constructor would break
the rule. The compiler catches the direct `self.` call, and every engine
catches the rest at run time:

{{example:12-escape-call-rejected}}

The call fails before `Echo().said("argument")` is evaluated, as the
specification requires, and the same holds through a role or a bound action.
Until 0.4.1 there was no such check: a call that read a field not yet bound
failed differently on each engine, and on Sparrow it crashed the process
(#271). If a constructor fails after `self` escaped, the escaped object
never completes, and every later call on it is rejected the same way.

The first `self` may sit anywhere in a statement: inside a `map`,
`filter` or `reduce` block, in a nested block or in one branch of an `if`.
The object is built once, just before the constructor statement that holds
that first use, so every leaf made in a loop points at the same object.
Until 0.4.2 such a constructor crashed the compiler, and the workaround was
to name `self` first with `let me = self;` (#279).

The problem is old. Java lets `this` escape from a constructor and leaves
the consequences to the programmer. A non-null field of an object under
construction can still read as `null`. Fähndrich and Leino gave such
objects their own "raw" types [@fahndrich2003]. Summers and Müller's
"Freedom Before Commitment" tracks which objects are fully initialised and
which may still be "free", with few annotations [@summers2011]. Okkama
takes the cheaper road: one static rule for `self`, and a run-time check
for the indirect cases.

## 12.12 How the engines store an object

The three engines keep the same object in three different shapes. A
program cannot tell them apart; the costs differ.

**The IR engine** (`okkama-runtime-core`) keeps an object as a
reference-counted Rust struct:

- the number of its declaration;
- a vector of field values;
- a serial number, the identity's place in creation order;
- an owner cell, the region that will destroy it (Chapter 11);
- an optional side part, allocated only when needed.

The side part holds fields bound by `obj.init` and the run-time state of
std objects with built-in actions, such as channels. An object whose
constructor hands out `self` has one more field than it declares: a hidden
`$pending`, left unbound until the constructor returns. The check of §12.11
reads it with `obj.pending`. Objects whose constructor never hands out
`self` have neither the field nor the check. The field vector is
never written after `obj.new`. An `undef` field bound later is looked up in
the side part. Most objects never allocate one.

**Sparrow** keeps an object in its own non-moving heap. The object
is one header word followed by one 8-byte word per field. The header holds
the kind (`OBJECT`), a mark bit for the collector, a few flags, and the
object's descriptor number in its upper 32 bits. Each field word has a
representation fixed by the descriptor:

- `Int`: a tagged small integer, or a pointer to a big one;
- `Raw`: the bits of a float, `bool`, byte or char;
- `Ref`: a pointer to another heap cell.

The collector follows `Ref` words and the `Int` words that point to big
integers; it never looks inside `Raw` words.
`Point(3, 4)` is three words, 24 bytes, with both integers stored inline.
Small cells come from per-size free lists in 64 KiB pages: 2, 3, 4, 5, 6,
8 … 128 words. Machine code produced by the JIT can pop a cell from its
list without calling into the runtime. A field read is one load at
`object + 8 + 8 × index`.

**The JVM backend** makes every object a `final` class that extends the
runtime's `okkama.rt.Obj`. `Obj` holds the serial number, the owning region
and the slot for built-in state. Each role the object plays becomes a Java
interface it implements. Fields become instance fields. They are not
`final` in the class file, because `obj.init` must still be able to bind
them after an early `self`. The language guarantees immutability; the class
file does not. The class gets generated `equals` and `hashCode`, field by
field and consistent with each other, so the runtime's Java collections
see the same structural equality as the program. The JIT of the JVM then
treats the object like any other small Java object.

## 12.13 Objects elsewhere

Most languages that took immutability seriously added a shorthand for "an
object that is its fields". Comparing them shows what Okkama chose to
leave out.

| | Okkama object | Java record | Kotlin data class | Rust struct |
|---|---|---|---|---|
| Fields readable from outside | never | via generated accessors | yes (properties) | within the module, or if `pub` |
| Constructor | written by hand; shorthand `(:x)` | generated canonical, may be compact | primary constructor | none; a `new` function by convention |
| Validation | `confirm` in each constructor | in the compact constructor | `init` block | in the `new` function |
| Equality | structural, always | generated from components | generated from the primary constructor | `#[derive(PartialEq)]` |
| Mutation | none | none (fields `final`) | `var` allowed | `mut` binding |
| Copy with changes | successor actions (Chapter 13) | none built in | `copy(x = …)` | struct update `..old` |

Java records (JEP 395) are the closest: final fields, a canonical
constructor, generated `equals`, `hashCode` and `toString`
[@jep395]. The difference is the accessors. A record publishes every
component, so its representation *is* its API. Kotlin's data classes do
the same, and only properties of the primary constructor take part in the
generated `equals` [@kotlindata]. Rust hides fields behind module privacy,
which is coarser than an object: any code in the same module can read
them [@klabnik2023].

Okkama keeps the generated equality and drops the generated access. A
record is a tuple with names. An Okkama object is a value with behaviour,
and its fields are nobody else's business. Bloch's advice for Java,
"minimize mutability" and "favor composition over inheritance", needs no
discipline here: the language allows nothing else [@bloch2018].

## 12.14 What this design costs

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
