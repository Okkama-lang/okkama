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
- a helper marked `hidden` is callable only by `Cash` itself;
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
    hidden in_cents(units is int) is int { … }
}
```

An object declaration has a name and a body, and nothing else: the header
takes no parameters, because state is declared only by `has`. The name is a
type name and starts with an upper-case letter. Two objects of one module
cannot share a name. The body may hold:

- `has` fields;
- constructors, at least one;
- actions, public or marked `hidden`;
- at most one `destroy` (Chapter 11);
- `cap` contracts (§12.8);
- test blocks.

The order of these parts does not matter to the compiler. The style used in
the standard library and in this book puts fields first, then constructors,
then the public actions, then the hidden ones.

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
  blocks, while the object is being built. A later bind of the same field
  replaces an earlier one, and once the constructor returns the field never
  changes. Outside constructors the only `name = value;` is the rebinding of
  a local to its successor (Section 13.7).
- Every field must be bound or defaulted by the end of the constructor.
  Otherwise the compiler reports `Object "Cash" field "cents" was not
  initialized`.
- Parameters may have defaults, as an action's may (Section 13.2):
  `Cash(digit is int, cents is int = 0)` is called as `Cash(3)`.
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
  `Member(7)` is rejected with `Constructor of "Member" takes named
  arguments only: write Member(id: …, name: …)` before any argument is
  evaluated. A listed field that has a default may be left out of the call.
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
part is an object of its own, or a hidden action both call.

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
action, often the first one called. `ParsedIni(text)` stores the text, and
the variant that takes explicit limits checks them. Parsing happens in
`document()`, so constructing a
`ParsedIni` never fails for a syntax error in the document. This keeps
construction predictable: it validates what it was given and does nothing
else.

## 12.7 Actions and hidden actions

Public actions are the object's interface: `total() is int { … }`. Their full
treatment, with overloading, default arguments and successors, is Chapter
13. Two points belong here.

- **Calls to the object's own actions go through `self`.** Inside `Cash`,
  `self.in_cents(digit)` calls the helper. A bare `in_cents(digit)` is
  `Undefined action "in_cents"`. Okkama has no free functions, so a bare
  call would have nothing to refer to.
- **`hidden` keeps an action out of the contract.** `hidden in_cents(...)` is
  callable only by the object itself, through `self`. Each action is marked
  on its own; constructors cannot be hidden, since they are how the object is
  made. A hidden action is an implementation detail, free to change; a public
  one is a promise. The word means the same as before a hidden object or enum
  (Chapter 19): not part of the public contract.

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

Caps are the object-oriented answer to "I need a getter for this one
caller". The answer is not to open the field to everyone, but to grant a
named, typed view to whoever is handed it.

### Inside the object or beside it

A cap is usually declared inside its object, as above. It can also be
declared at module scope, beside an object of the same module. Its body
then names the owner's fields through the owner type, `Box::label`:

{{example:12-cap-outside}}

Both forms share one namespace, so an object cannot have two caps of the
same name. An imported object cannot be given a cap: only the module that
owns an object decides what it shows. A cap of a generic object takes the
object's type parameters. It is written `Box::Tag` where it is declared and
`Box<int>::Tag` where it is used.

### What a cap field may do

A cap field's body is an ordinary Okkama block, with local bindings,
`if`, `match`, `confirm` and errors. It reads the owner's private fields by
name. Three things are missing on purpose.

- There is no `self`, not even inside a nested anonymous action, so the
  body cannot call the owner's actions. Writing `self.norm()` in a cap is
  an error:

  {{example:12-cap-self-rejected}}

- A field cannot name the cap's other fields. Each field stands on the
  owner alone.
- A field is synchronous: there is no `await`. It may still return a
  Future it makes explicitly.

A cap cannot write fields or stage a successor. It may call actions of the
objects its owner holds, as `Report` below calls `hits.value()`.

### Every read computes

Acquiring a cap evaluates the owner expression once. Every read of a field
runs its body again, and nothing is cached, neither a value nor an error:

{{example:12-cap-reads}}

The first read of `share` throws `NoHits.none`, and the read after two hits
returns 50. A cap is a view, not a copy. A field whose answer is expensive
belongs in an action, where `once let` (Section 13.13) can keep it.

### A cap keeps its owner whole

A cap holds its owner, the whole object with everything it owns,
including resources in its fields. For ownership (Section 11.5) a cap is
its owner. A cap of a fresh object that an action returns moves the object
to the caller. A cap of an object that a scope already owns borrows it,
like an alias:

{{example:12-cap-owner}}

`ada`'s log is closed when `main` ends, because her account came to `main`
inside the cap. `bob`'s log is closed when the inner block that owns his
account ends.

A cap is also bound to the version it was taken from. A successor
(Chapter 13) is a new version, with caps of its own:

{{example:12-cap-snapshot}}

`before` still reads the open order after `settled()` made the paid one.

### What a cap is not

A cap's type is exact. Locals, parameters, results, fields, collections,
generics, closures and Futures keep `Customer::BusinessCard` as it is. Inside
`any` it is opaque. Nothing turns a cap back into its owner: there is no
conversion, and no pattern or reflection reaches the owner through it.

A cap has no equality, no ordering and no hashing, so it cannot be a map
key. It is not formatted or serialized automatically:

{{example:12-cap-compare-rejected}}

A cap cannot become a role, have constructors, actions, `has` fields or
nested caps, or be constructed by hand. It is an interface boundary and
nothing more. It promises no secrecy against code that holds the full
object, no purity, since a field may call actions of objects the owner
holds, and no revocation: a cap handed out stays readable as long as it
lives.

### Cap, role or hidden action

Three tools look alike.

| Tool | Question it answers | Who decides |
|---|---|---|
| hidden action | what the object does inside | the object, for itself |
| role (Chapter 14) | what any object of the role can do | the role, for every implementation |
| cap | what this object shows to whoever it hands it | the object, for one view |

A role is behaviour that many objects provide; a caller holding a role does
not know which object it holds. A cap belongs to one object and shows its
state; the caller knows exactly whose view it reads. A hidden action is not
seen at all.

The standard library follows one rule (Chapter 22): **an object reveals
itself through a cap**. It declares about 130 caps, named for the part they
show: `page::Head::status`, `date::Calendar::year`, `range::Bounds::start`,
`duration::Span::seconds`. A cap holds facts that are already computed, or
that are trivial to compute, such as a parsed part of a URL or a date's
year. Anything that is work stays an action: `file.text()` reads a file and
`response.body()` reads a body, so neither is a cap.

### Caps elsewhere

Most languages answer "show this caller a few facts" with accessors that
everyone can call. A Java record gives every component a public accessor
[@jep395]. A Kotlin data class and a C# record do the same with
properties [@kotlindata; @csrecords]. Narrowing the view takes an
interface. Then the caller can cast it back to the full class, unless the
designer hides the class.

TypeScript's `Pick<T, K>` and `Readonly<T>` name a narrower shape. They
are structural and erased, so any object of that shape fits, and a cast
gets the rest [@tscompat].

Object-capability languages take another view of what a reference
means: holding one *is* the authority to use it, and a narrower facet is a
separate object that forwards only some requests [@miller2006]. An Okkama
cap is closer to such a facet than to a getter. It is a separate,
nominal type, the holder cannot widen it, and the owner chooses what it
shows. It is not an authority boundary, though: a cap shows state and
grants no actions.

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

## 12.10 Handing `self` out of a constructor

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

The collaborator holds an unfinished object, and the rule from the start of
this section follows it there. The specification forbids calling *any*
action on a receiver whose construction is pending, "through aliases, role
references, bound actions and collaborators", and requires the run time to
reject such a call before evaluating its arguments. `Leaf` above only
stores `up`; calling `up.describe()` in `Leaf`'s constructor would break
the rule. The compiler catches the direct `self.` call, and indirect calls are rejected at run time:

{{example:12-escape-call-rejected}}

The call fails before `Echo().said("argument")` is evaluated, also through a role or bound action. If a constructor fails after `self` escaped, the object never completes and every later call on it is rejected.

The first use of `self` may occur inside a functional block, a nested block or a conditional branch. Every collaborator receives the same object identity, including collaborators created in a loop.

The problem is old. Java lets `this` escape from a constructor and leaves
the consequences to the programmer. A non-null field of an object under
construction can still read as `null`. Fähndrich and Leino gave such
objects their own "raw" types [@fahndrich2003]. Summers and Müller's
"Freedom Before Commitment" tracks which objects are fully initialised and
which may still be "free", with few annotations [@summers2011]. Okkama
takes the cheaper road: one static rule for `self`, and a run-time check
for the indirect cases.

## 12.11 When construction fails

Section 12.6 made the constructor the gate: an object that exists is valid.
This section covers the other side of the gate, the work a constructor did
before it refused, and the fields it never got to. Three rules decide both.

**Every field is bound when construction ends.** Defaults are filled before
the body runs, and then the arguments and the body bind the rest. A field
that no path of the constructor binds is a compile-time error:

{{example:12-unbound-rejected}}

A field bound on some paths and not on others is checked when construction
ends, on the path the call actually took:

{{example:12-unbound}}

- `Transfer("Almaty", true)` and `Transfer("Kostanay", false)` take paths
  that bind `via`, so they build complete objects.
- `Transfer("Aktau", false)` takes the missing `else`. Construction fails
  with `Object "Transfer" field "via" was not initialized` and points at the
  constructor.
- **This is not an error value.** It reports a defect in the program, as an
  index out of range does (Chapter 10). `catch e is Error` does not take it,
  and the run ends with the report.

The split between the two checks is the language owner's decision, recorded
as D-035 in the conformance suite's ledger:

- The compiler rejects what is certain, a field no path binds.
- The rest is checked where it happens.

The price is visible in `Transfer`: the missing branch is a mistake the
compiler could have pointed at, and it surfaces only when an input takes
that branch. A test that covers the branches (Chapter 15) finds it before a
user does.

**What was bound is released, newest first.** Section 11.7 showed the rule
on a session: a constructor that fails closes what it opened, and the
object's own `destroy` does not run. The details matter when the
constructor binds fields inside blocks:

{{example:12-partial}}

- **A temporary of the body dies first.** `scratch` belongs to the
  constructor's block and closes when the block unwinds, before any field.
- **A field bound inside a block belongs to the construction.** `lock` is
  bound in a branch of the `if`, and it outlives the branch. That is what
  makes it a field. When the `confirm` fails, `lock` is released with the
  construction, not with the branch.
- **Fields go in reverse order of binding.** `lock` closes before `staging`,
  as locals do in a scope (Section 11.4).
- **The error reaches the caller unchanged.** The `catch` sees
  `Refused.tooLarge(5000)` after all the cleanup, so it runs with nothing
  half-open.
- **The object never existed, so its `destroy` does not run.** `upload of
  ... done` prints only for the second upload. A `destroy` may rely on
  everything the constructor establishes, the same argument Section 12.10
  made for actions, and running it on a half-built object would break that.
  The fields' own `destroy`s do run, because each field is a complete
  object.

**Nobody observes the half-built object.** No reference to an unfinished
`Upload` exists outside its constructor. The one exception is `self`
handed to a collaborator (Section 12.10), and every call through such a
reference is refused until construction completes, and forever if it
fails.

Other languages split the same three questions differently:

- **C++ has the release rule.** When a constructor throws, the destructors
  of the members and bases already constructed run in reverse order, and
  the object's own destructor does not. Stroustrup builds the basic
  exception-safety guarantee of the standard library on exactly this rule
  [@stroustrup2001]. The gap is the raw resource: a pointer or file handle
  acquired in the body and not yet held by a member leaks, unless it is
  wrapped in an object first. In Okkama every resource is an object with an
  owner, so there is no raw handle to wrap.
- **Java has no unbound fields, and no release rule.** Every field starts
  as `null`, `0` or `false`, so "not initialized" cannot be told apart from
  a value. Only a `final` field must be definitely assigned by every
  constructor (JLS Chapter 16, [@gosling2023]). A constructor that throws
  leaves a partly initialized object behind, and a subclass's finalizer can
  catch and use it. SEI CERT rule OBJ11-J, "Be wary of letting constructors
  throw exceptions", describes the attack [@certobj11]. Nothing is closed.
- **Kotlin moves the check to the first read.** A `lateinit` property may
  stay unset after construction, and reading it before it is set throws
  `UninitializedPropertyAccessException` [@kotlinproperties]. The failure
  appears far from the constructor that caused it.
- **Swift checks statically.** Every stored property must have a value
  before the initializer finishes, and `self` cannot be used before then.
  A failable `init?` returns `nil` instead of an object [@swiftbook].
- **Rust has no constructors.** A struct literal must name every field, so
  a half-built struct cannot be written. Values made before a failure are
  ordinary locals and are dropped as the function unwinds.

Okkama's position is between Swift's and C++'s. It is static where that is
certain and checks the path otherwise, as no other language here does, and
it releases what was bound as C++ does, without raw handles.

## 12.12 A worked domain model: money and an order

Small objects are easiest to judge in a domain everyone knows. An order has
lines; a line has an item, a quantity and a unit price; prices are money in
one currency.

{{example:12-order}}

- **One noun per idea.** `Money` is an amount and a currency, nothing else;
  `Line` is one row; `Order` is its lines. Each object answers questions
  about itself (`total()`), and none exposes its fields: `Money` offers
  `code()` and `value()` for the one comparison `plus` needs, and `string()`
  for display.
- **The constructor is the gate.** A negative amount and a zero quantity are
  refused where the object would be born, with a typed error that names the
  bad value (§12.6). An object that exists is valid, so `total()` never
  checks its own fields again.
- **Exact arithmetic is the default.** The amounts are `rat`, so three teas
  at 7/2 and two cups at 12 make exactly `69/2`, with no rounding to explain
  later. A currency mismatch is an error, not an implicit conversion.
- **Derived values are computed.** `Order.total` folds its lines with
  `reduce`; nothing stores a running total that could drift from the lines.
  Changing an order means building a new one with other lines (Chapter 13).

The model is three short objects and two error enums. A design with getters
and setters would have more code and fewer guarantees: every reader would
have to trust that nobody set a quantity to zero after construction.

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
  `Number`, `Utf8Decoded`, `HexEncoded`. Each is a noun for one
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
