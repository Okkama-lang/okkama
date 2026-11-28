# Okkama for agents: the language

A compact reference of Okkama for a reader who has never seen it. Every
example here is compiled, and run where it shows output, by
`docs/agents/check.py` against the compiler it ships with; a block marked
`okkama,fail` shows code the compiler rejects, followed by its exact
diagnostic. The reasons behind each rule are in the Okkama Bible.

Okkama is an object language without classes, inheritance, null, mutable
fields, free functions or implicit conversions. These rules are where code
written from habit fails:

- **No free functions.** Every action belongs to an object; the only action
  at module scope is `main`. An object calls its own action as
  `self.name(…)`, never `name(…)`.
- **Fields are private and immutable.** Nobody outside reads a field; only a
  constructor binds one. A change is a *successor*, an action `is ^T` that
  returns a new object.
- **Names are bound once.** `let x = 1; x = 2;` is an error. There is no `+=`,
  `++` or `while`. Accumulate with `reduce`, a successor or `cell.Cell`.
- **Nothing converts silently.** `1 + 1.0`, `"n = " + 1` and `1 == 1.0` are
  compile errors. `7 / 2` is the exact rational `7/2`; `int(7 / 2)` is `3`.
- **No null.** Absence is `Maybe<T>`. A missing index or key is a failure
  unless the expression gives a fallback: `xs[9] else 0`.
- **Errors are enums** (`enum E is Error = (…)`). There is no `Exception`,
  no catch-all and no `finally`.
- **A block's value is its last expression.** `return` is for early exits.
- **Resources are released by scope.** `destroy` runs when the owning scope
  ends, `defer` when the current block ends.

```okkama,run
module hello;

import "io/stdout" take Stdout;

main() is int {
    Stdout().writeln("Hello, Okkama");
    0;
}
```
```output
Hello, Okkama
```

## 1. Lexical structure

**Source text.** Outside literals and comments only ASCII is allowed.
Spaces, tabs and line breaks separate tokens.

**Comments.** `//` to the end of the line; `/* … */` does not nest. The `//`
lines directly above a declaration are its *doc comment*, in CommonMark,
shown by `okkama man` and the editor. In a doc comment a fence marked
`okkama` must compile, `okkama,run` runs and must print the `output` fence
after it, and `okkama,ignore` is only shown; `okkama run -t` checks them.

```okkama
module docs/price;

// A price in whole cents.
//
// ```okkama
// let price = Price(cents: 250);
// ```
object Price {
    has cents is int;
    Price(:cents);  /* a shorthand constructor (3.2) */
}
```

**Identifiers** are `[A-Za-z_][A-Za-z0-9_]*`. Case depends on position:
objects, roles, enums, type parameters and constructors start upper-case;
fields, parameters, actions, enum cases, module segments and import aliases
start lower-case or with `_`; locals (`let`, `for`, `catch`) and constants
may have either case.

**Keywords** (never names, except as a member name after `.` or an action
name): `as async await case catch confirm const defer destroy does else enum
false fake for has if import is last let loop match module native next object
panic return role self spawn take test throw true try`. Contextual: `hidden`,
`cap`, `once`, `void`, `main`, `_`. The parser rejects, with a message:
`null`, `nil`, `none` as a value, `super`, `abstract`, `protected`, `with`,
`finally`, `?`, `??`, `?.`, `**`, `class`, `interface`, `while`, `do`,
`x += 1`, `x++`. An action may not be named `with` or `with_…`.

| Literal | Examples | Notes |
|---|---|---|
| `int` | `42`, `1_000_000`, `0xFF`, `0b1010` | unbounded; `_` only between digits; no octal |
| `float` | `3.14`, `6.022e23`, `1.5E-3` | digits on both sides of `.`: no `.5`, no `1.` |
| `string` | `"a\tb\n"`, `"caf\u{e9}"` | escapes `\n \r \t \\ \" \u{H…}`; may span lines; no interpolation |
| raw `string` | ` ```C:\new "q"``` ` | no escapes; the first ` ``` ` closes it |
| `string` | `endl` | exactly `"\n"` |
| `char` | `'x'`, `'\u{41}'`, `'\''` | one code point |
| `bool` | `true`, `false` | |

`byte` and `rat` have no literals: write `byte(255)`, `1 / 3`, `rat(1, 3)`.

```okkama,run
module lexical;

import "io/stdout" take Stdout;

main() is int {
    let out = Stdout();
    out.writeln(1_000_000.string(), " ", 0xFF.string(), " ", 0b1010.string());
    out.writeln(6.022e23.string(), " ", 1.5E-3.string(), " ", 2.0.string());
    out.write("first line", endl);
    out.writeln('A'.code().string(), " ", ```C:\new "quoted"```);
    0;
}
```
```output
1000000 255 10
6.022e+23 0.0015 2
first line
65 C:\new "quoted"
```

## 2. Modules and imports

**A file is a module.** It starts with `module name/path [vX.Y.Z];`, then
its imports, then declarations in any order. A file has no executable top
level, so importing never runs code. Only the major version reaches the
types: `v1` and `v2` of one module declare different types.

**Imports** come before every declaration. The path is a file path without
`.ok`, looked up in the importing file's directory, the package root (the
nearest directory with `blade.toml`), `OKKAMA_IMPORT_DIRS`, then the bundled
standard library.

| Form | Brings in |
|---|---|
| `import "shop/basket";` | namespace `basket`: `basket.Basket(…)`, type `basket.Basket` |
| `import "shop/money" as cash;` | namespace `cash` |
| `import "shop/money" take Money, RATE;` | the bare names, no namespace |
| `import "io/stdout" as out take Stdout;` | namespace `out` holding only `Stdout` |

There is no wildcard import, no re-export (a module exports only what it
declares) and no import cycle. A std declaration can also be named in place
by a `std.` path (`std` is reserved).

```okkama,run
module modules;

import "strings/buffer";
import "io/stdout" as console take Stdout;

main() is int {
    let page = buffer.TextBuffer();
    page.write("built ", "in pieces");
    console.Stdout().writeln(page.string());
    std.io.stdout.Stdout().writeln("no import needed");
    0;
}
```
```output
built in pieces
no import needed
```

**Visibility.** A module-level declaration is public unless `hidden`
(before `object`, `role`, `enum` or `const`). A public signature cannot name
a hidden type. A hidden object may do a public role: a public role, a hidden
implementation and a public action returning the role is the usual way to
hide an implementation.

**One module, several files:** `okkama run main.ok part.ok` joins files that
declare the same module and version; each file keeps its own imports.

## 3. Declarations

### 3.1 Objects and fields

An object holds fields (`has`), at least one constructor, actions, at most
one `destroy`, caps and tests. The header takes no parameters and there is
no `extends`: behaviour is shared by holding another object, contracts by
roles (3.5).

- `has a, b is int;` declares fields of one type; `has n is int = 0;` gives
  a default; `has label = "x";` takes its type from the initializer.
- A field is **private**: no code outside the object reads it, not even in
  the same module. Ask the object for behaviour.
- A field **never changes** after construction.
- Inside the object a field is read by its bare name or as `self.name`. An
  action may share a field's name: `size() is int { size; }`.

### 3.2 Constructors

A constructor has the object's name; none is generated.

- **Full:** `Name(params) { … }` binds every field without a default, as
  `self.field = value;` or `field = value;`, and may validate with
  `confirm … else throw …`. It cannot call the object's own actions.
- **Shorthand:** `Name(:a, :b);` takes one *named* argument per listed field
  and may have a body. `Name();` is the constructor of a stateless object.
- Constructors overload by parameter count and types. A call is positional
  or named, never mixed.

```okkama,run
module cash;

import "io/stdout" take Stdout;

enum InvalidCash is Error = (negative(cents is int));

object Cash {
    has units is int;
    has cents is int = 0;
    has currency = "USD";

    Cash(units is int, cents is int = 0) {
        self.units = units;
        self.cents = cents;
        confirm units * 100 + cents >= 0 else throw InvalidCash.negative(units * 100 + cents);
    }
    Cash(:units, :currency);

    total() is int { self.in_cents(units) + cents; }
    shown() is string { self.total().string() + " " + currency; }
    hidden in_cents(count is int) is int { count * 100; }
}

main() is int {
    let out = Stdout();
    out.writeln(Cash(3, 25).shown(), "; ", Cash(3).shown(), "; ", Cash(units: 2, currency: "EUR").shown());
    out.writeln((Cash(1, 0) == Cash(units: 1, cents: 0)).string());
    0;
}
```
```output
325 USD; 300 USD; 200 EUR
true
```

Objects compare **structurally**, field by field; there is no reference
equality. Typical mistakes, each reported:

```okkama,fail
module mistakes;

import "cell" take Cell;

object Counter {
    has count is int;
    has cache is Cell<int>;
    Counter(:count, :cache);
    doubled() is int { twice(count); }
    twice(n is int) is int { n * 2; }
}

main() is int {
    let counter = Counter(5, Cell(0));
    Counter(count: 1, cache: Cell(0)).count;
    0;
}
```
```error
example.ok:7:9: semantic error: Field "cache" cannot hold a Cell: a cell stays in synchronous code; state an object shares with tasks is an Atomic
example.ok:9:24: semantic error: Undefined action "twice"
example.ok:14:19: semantic error: Constructor of "Counter" takes named arguments only: write Counter(count: …, cache: …)
example.ok:15:39: semantic error: Field "count" of object "Counter" is private; call an action instead
```

### 3.3 Actions

`name(params) is Result { body }`. The result is mandatory: a type, or `void`
for an action called for its effect. The value is the last expression.
`hidden` keeps an action inside its object.

- `a, b is int` shares a type. Trailing parameters may have defaults,
  evaluated at each call in the declaring module.
- Calls are positional or named (any order), never mixed.
- Overloads differ by parameter count, parameter types or variadic shape;
  names and result types never tell them apart. A tie is an error.
- One last parameter may be variadic, `values is ...T`, called positionally;
  inside, it is only unpacked with `values...`. A `...string` parameter
  accepts any value that has `string()`.
- Recursion is an ordinary `self.` call; a self tail call runs in constant
  stack. An action whose every path calls itself is a compile error.
- Returning the receiver is written `same() is Name { self; }`.

```okkama,run
module actions;

import "io/stdout" take Stdout;

object Greeter {
    Greeter();
    greet(name is string, mark is string = "!") is string { "Hello, " + name + mark; }
    total(values is ...int) is int {
        let all = values...;
        all.reduce(0) -> sum, value { sum + value; };
    }
    describe(value is int) is string { "int"; }
    describe(value is string) is string { "string"; }
    factorial(n is int) is int { if n <= 1 { 1; } else { n * self.factorial(n - 1); } }
}

main() is int {
    let out = Stdout();
    let hello = Greeter();
    out.writeln(hello.greet("Ada"), " ", hello.greet(mark: "?", name: "Alan"));
    out.writeln(hello.total().string(), " ", hello.total(1, 2, 3).string(), " ", hello.describe("x"));
    out.writeln(hello.factorial(20).string(), ", ready: ", true);
    0;
}
```
```output
Hello, Ada! Hello, Alan?
0 6 string
2432902008176640000, ready: true
```

### 3.4 Successors and rebinding

A successor is an action declared `is ^Self`. It stages new field values
with `^field = value;`; the result is a new object with those fields
replaced and the rest copied. The receiver is untouched, and reads in the
body see its old values. The body's last expression is discarded.

- The result **must be used**: returned, passed, stored, called on or
  rebound.
- **Rebinding** `name = expr;` (no `let`) is allowed only for a local whose
  type has a `^` successor. Scalars, arrays, maps and enums are never
  rebound. A rebind inside a loop body updates the outer local.
- A failing successor produces nothing; the receiver stays as it was.
- Name it after the event (`ordered`), not as a setter; `with…` is rejected.

```okkama,run
module tab;

import "io/stdout" take Stdout;

enum TabError is Error = (over_limit(total is int));

object Tab {
    has limit is int;
    has items is int = 0;
    has total is int = 0;
    Tab(:limit);
    ordered(price is int) is ^Tab {
        confirm total + price <= limit else throw TabError.over_limit(total + price);
        ^items = items + 1;
        ^total = total + price;
    }
    summary() is string { items.string() + " items, " + total.string(); }
}

main() is int {
    let empty = Tab(limit: 100);
    let tab = empty;
    for [10, 20, 30] -> price {
        tab = tab.ordered(price);
    }
    Stdout().writeln(tab.summary(), "; still ", empty.summary());
    0;
}
```
```output
3 items, 60; still 0 items, 0
```

```okkama,fail
module unused;

object Tab {
    has total is int = 0;
    Tab();
    ordered(price is int) is ^Tab { ^total = total + price; }
}

main() is int {
    let tab = Tab();
    tab.ordered(5);
    let n = 1;
    n = 2;
    0;
}
```
```error
example.ok:13:5: semantic error: Cannot rebind variable "n": type "int" has no successor actions
example.ok:11:5: semantic error: Successor action result must be used
```

### 3.5 Roles

A role is a nominal contract: action signatures, no fields, no bodies. An
object implements it only by declaring `does`; matching names are not
enough. Parameter types must match exactly, and `^Object` satisfies
`^Role`. A role-typed value is a restricted reference: only the role's
actions are reachable and nothing casts it back. A value becomes one only
where the role is the declared type (a typed `let`, field, parameter, result
or `@Role` element). A role may have `async` actions (implementations repeat
`async`), type parameters, successors and one `fake` block (3.12).

```okkama,run
module roles;

import "io/stdout" take Stdout;

role Shape {
    area() is rat;
}

object Square does Shape {
    has side is int;
    Square(:side);
    area() is rat { (side * side).rat(); }
}

object Triangle does Shape {
    has base, height is int;
    Triangle(:base, :height);
    area() is rat { base * height / 2; }
}

main() is int {
    let shapes is @Shape = [Square(side: 3), Triangle(base: 5, height: 3)];
    let total = shapes.reduce(0.rat()) -> sum, shape { sum + shape.area(); };
    let first is Shape = Square(side: 2);
    let second is Shape = Square(side: 2);
    Stdout().writeln(total.string(), " ", (first == second).string());
    0;
}
```
```output
33/2 true
```

**Operators are roles** of the std module `operators`: `Adding<T, R>`
(`plus`, `+`), `Subtracting` (`minus`), `Multiplying` (`times`), `Dividing`
(`divided`), `Remaindering` (`remainder`), `Negating<R>` (`negated`, unary
`-`). Ordering comes from `compare`'s `Comparing<T>` (`order(other) is
Order`). An object has no operator it does not declare; `==` is always
structural.

```okkama,run
module money;

import "io/stdout" take Stdout;
import "operators" take Adding;
import "compare" take Comparing, Order;

object Money does Adding<Money, Money>, Comparing<Money> {
    has cents is int;
    Money(:cents);
    plus(other is Money) is Money { Money(cents: cents + other.amount()); }
    order(other is Money) is Order { cents.order(other.amount()); }
    amount() is int { cents; }
}

main() is int {
    let sum = Money(cents: 250) + Money(cents: 100);
    Stdout().writeln(sum.amount().string(), " ", (Money(cents: 1) < sum).string());
    0;
}
```
```output
350 true
```

```okkama,fail
module nominal;

role Labelled {
    label() is string;
}

object Invoice {
    Invoice();
    label() is string { "invoice"; }
}

object Fault does Error {
    Fault();
    text() is string { "fault"; }
}

main() is int {
    let labelled is Labelled = Invoice();
    0;
}
```
```error
example.ok:12:19: semantic error: Object "Fault" cannot do Error: errors are enums, declare `enum Fault is Error = (...)`
example.ok:18:32: semantic error: Cannot initialize variable "labelled" of type Labelled with Invoice
```

### 3.6 Enums and variants

`enum Name = (a, b, c);` lists lower-case cases, always written with the
type: `Unit.celsius`. An enum value has no number, no order and no actions;
it has equality and can be a map key (sorted in declaration order). A case
may carry fields: the enum is then a *variant* (tagged union). Payloads are
positional and read only through a `match` pattern. A variant may be
generic; a case without payload needs its type arguments where nothing else
gives them (`Reading<int>.missing`). `Maybe<T>` and `Presence<T>` are
built-in variants; `is Error` makes an error enum (6).

```okkama,run
module enums;

import "io/stdout" take Stdout;

enum Unit = (celsius, kelvin);
enum Reading<T> = (missing, single(value is T), range(low is T, high is T));

object Sensor {
    has unit is Unit;
    has reading is Reading<int>;
    Sensor(:unit, :reading);
    text() is string {
        let suffix = match unit { case Unit.celsius: " C"; case Unit.kelvin: " K"; };
        match reading {
            case Reading.missing { "no data"; }
            case Reading.single(value) { value.string() + suffix; }
            case Reading.range(low, _) { "from " + low.string() + suffix; }
        };
    }
}

main() is int {
    let sensors is @Sensor = [
        Sensor(unit: Unit.celsius, reading: Reading.single(21)),
        Sensor(unit: Unit.kelvin, reading: Reading.range(280, 290)),
        Sensor(unit: Unit.celsius, reading: Reading<int>.missing)
    ];
    for sensors -> sensor { Stdout().writeln(sensor.text()); }
    0;
}
```
```output
21 C
from 280 K
no data
```

### 3.7 Constants

`const NAME [is T] = expr;` at module scope is computed by the compiler from
literals, payload-free enum cases, `rat(n, d)`, scalar casts, operators,
other constants, and arrays and maps of those, in any order. Calls are
rejected. Constants are public unless `hidden const`. There is no
module-level `let`, static field or singleton: build shared objects in
`main` and pass them in.

```okkama,run
module limits;

import "io/stdout" take Stdout;

const MB = 1024 * KB;
const KB = 1024;
const LIMITS = ["upload": 10 * MB, "header": 64 * KB];
hidden const HALF = 1 / 2;

main() is int {
    Stdout().writeln(LIMITS["upload"].string(), " ", HALF.string());
    0;
}
```
```output
10485760 1/2
```

### 3.8 `once let`

`once let name = expr;` in an object's action computes `expr` on the first
call and keeps it for that object. It stands only at the top level of an
action body, reads fields, `self`, constants and earlier `once let`s (no
parameters), and must be free of effects (no clock, files, `Cell`, `await`).
If it throws, the next call tries again; a successor computes its own.

```okkama,run
module once;

import "io/stdout" take Stdout;

object Words {
    has items is @string;
    Words(:items);
    longest() is string {
        once let found = items.reduce("") -> best, word { if word.length() > best.length() { word; } else { best; }; };
        found;
    }
}

main() is int {
    let words = Words(items: ["a", "abc", "ab"]);
    Stdout().writeln(words.longest(), " ", words.longest());
    0;
}
```
```output
abc abc
```

### 3.9 Anonymous actions, closures and bound actions

An anonymous action is an action without a name:
`(value is int) is int { value * 2; }`. Parameters are typed and the result
is declared. Its type is the signature without names: `(int) is int`,
`() is void`, `((int) is int) is int`. It is called with positional
arguments only.

- It captures the **values** it uses, as a snapshot, and `self` when it reads
  a field. Rebinding a captured name inside it is an error.
- It cannot be `async` or variadic, take defaults or recurse by name;
  `next`/`last` cannot leave a loop around it.
- Action types are exact (no variance) and never go into `any`. Two action
  values are equal only when they are the same value.
- A **bound action** `object.action` (without a call) carries its receiver;
  it is allowed only where its action type is written.

```okkama,run
module closures;

import "io/stdout" take Stdout;

object Price {
    has rate is int;
    Price(:rate);
    of(amount is int) is int { amount * rate; }
}

main() is int {
    let make_adder = (amount is int) is (int) is int {
        (value is int) is int { value + amount; };
    };
    let triple is (int) is int = Price(rate: 3).of;
    let steps is @(int) is int = [make_adder(10), (value is int) is int { value * 2; }, triple];
    let result = steps.reduce(1) -> current, step { step(current); };
    Stdout().writeln(make_adder(1)(4).string(), " ", result.string());
    0;
}
```
```output
5 66
```

### 3.10 Async, `await` and `spawn`

- Calling an `async` action gives a `Future<T>`. The call is **lazy**: the
  body starts at the first `await` (or at `spawn`). A future runs at most
  once and keeps its outcome.
- `await` is allowed only in an `async` action, `async main` or `async test`.
  It returns the value or rethrows the failure. `await x + 1` is
  `(await x) + 1`.
- A `Future<T>` has only `done()`, `cancel()` and `cancelled()`. The
  compiler insists on an `await` only for the future of a successor action
  (it carries the receiver's next value). Any other future may be dropped
  without a word, and since the call is lazy its body then never runs:
  `Worker().run();` does nothing. Await it, hand it on (return, pass,
  store) or `spawn` it.
- `spawn Receiver(…).action(…)` starts a call now. An async action becomes a
  task of the run: one scheduler, tasks switch only at `await`. A
  synchronous action runs in a worker process; only scalars, arrays and maps
  cross.
- `future.cancel()` unwinds the task like a failure; its awaits throw the
  built-in `Cancelled`. Cancelling a task also cancels the future it awaits.
- `for await source -> item { … }` walks anything with
  `async next() is Maybe<T>`, such as a channel's receiving end.
- State shared between tasks is `async/atomic`'s `Atomic<T>`; channels,
  events, locks and groups live under `async/`.

```okkama,run
module bakery;

import "io/stdout" take Stdout;

object Oven {
    has name is string;
    Oven(:name);
    async baked() is string {
        Stdout().writeln("baking " + name);
        name + " bread";
    }
}

object Bakery {
    Bakery();
    async both() is string {
        let rye = spawn Oven(name: "rye").baked();
        let wheat = Oven(name: "wheat").baked();
        Stdout().writeln("ordered");
        (await rye) + ", " + (await wheat);
    }
}

async main() is int {
    Stdout().writeln(await Bakery().both());
    0;
}
```
```output
baking rye
ordered
baking wheat
rye bread, wheat bread
```

### 3.11 Generics

Objects, roles and enums take type parameters (`object Box<T>`,
`role Echo<T>`, `enum Slot<T>`); actions take none of their own. Type
arguments are inferred from constructor arguments or from the declared type
of the receiving place: write `let b is Box<int> = Box(…);`, never
`Box<int>(…)`. In one specialization `T` is one exact type, so
`Least(1, 2.0)` is rejected. A bound `object Least<T: Comparing<T>>` (objects
only, one per parameter) is what lets the body call `order` on a `T`; an
unbounded `T` answers no action, not even `string()`. `Box<Square>` is not a
`Box<Shape>`.

```okkama,run
module generics;

import "io/stdout" take Stdout;
import "compare" take Comparing, Order;

object Pair<K, V> {
    has key is K;
    has value is V;
    Pair(:key, :value);
    key() is K { key; }
}

object Least<T: Comparing<T>> {
    has first is T;
    has rest is @T;
    Least(first is T, rest is ...T) {
        self.first = first;
        self.rest = rest...;
    }
    value() is T {
        rest.reduce(first) -> best, item { if item.order(best) == Order.less { item; } else { best; }; };
    }
}

main() is int {
    let empty is @int = [];
    let typed is Pair<string, @int> = Pair(key: "none", value: empty);
    Stdout().writeln(typed.key(), " ", Least(3, 9, 1).value().string(), " ", Least("pear", "fig").value());
    0;
}
```
```output
none 1 fig
```

### 3.12 Tests and fakes

`test "name" { … }` stands at module scope, in an object or in a role.
`okkama run -t file.ok` runs the tests (not `main`) in order. Inside, the
binding `test` offers `test.ok(bool)`, `test.is(actual, expected)`,
`test.isnt`, `test.close(a, b, tolerance)`, `test.text`, `test.bytes`,
`test.fail(reason)`, `test.skip(reason)`, `test.throws(Type) { … }` (returns
the caught error) and `test.case(name) { … }` (a named sub-case). A failed
check does not stop the test. `async test` may await. A role's `fake { … }`
block is a working implementation, made with `Role.fake()` in test mode
only; its fields need defaults, and changing state lives in an `Atomic`.

```okkama,test
module library;

import "async/atomic" take Atomic;

enum Refused is Error = (because(reason is string));

role Books {
    remember(id is int, title is string) is void;
    has(id is int) is bool;
    fake {
        has shelf is Atomic<%int:string> = Atomic([:]);
        remember(id is int, title is string) is void { shelf.replace(shelf.value() + [id: title]); }
        has(id is int) is bool { shelf.value().has(id); }
    }
}

object Library {
    has books is Books;
    Library(:books);
    shelve(id is int, title is string) is void {
        confirm !books.has(id) else throw Refused.because("id " + id.string() + " is taken");
        books.remember(id, title);
    }
    test "a shelved id is known" {
        let books = Books.fake();
        Library(books: books).shelve(7, "Dune");
        test.ok(books.has(7));
    }
}

test "an id cannot be shelved twice" {
    let library = Library(books: Books.fake());
    library.shelve(3, "Emma");
    let refused = test.throws(Refused) { library.shelve(3, "Persuasion"); };
    test.is(refused, Refused.because("id 3 is taken"));
}

main() is int { 0; }
```
```output
=== RUN   Library :: a shelved id is known
--- PASS: Library :: a shelved id is known
=== RUN   an id cannot be shelved twice
--- PASS: an id cannot be shelved twice
PASS
```

### 3.13 Caps

A cap is a named, read-only view of computed fields that an object grants:
`cap Card { name is string { … } }`. `object::Card` acquires it (type
`Object::Card`) and `card::name` reads a field. A cap body reads the owner's
fields but has no `self` and calls no owner action. A cap has no equality.

```okkama,run
module caps;

import "io/stdout" take Stdout;

object Customer {
    has given, family, card_number is string;
    Customer(:given, :family, :card_number);
    cap Card {
        full_name is string { given + " " + family; }
    }
}

main() is int {
    let card is Customer::Card = Customer(given: "Ada", family: "Lovelace", card_number: "4111")::Card;
    Stdout().writeln(card::full_name);
    0;
}
```
```output
Ada Lovelace
```

## 4. Types

| Type | Values | Notes |
|---|---|---|
| `int` | `42`, `-7` | exact, unbounded; bit operations use a 64-bit two's-complement window |
| `rat` | `1 / 3`, `rat(2, 6)` | exact, always reduced; `6 / 2` prints `3` |
| `float` | `3.14` | binary64, always finite: overflow or NaN is a runtime failure |
| `byte` | `byte(255)` | 0..255; `byte(256)` fails; `byte + byte` is an `int` |
| `bool` | `true`, `false` | `string()` gives `"true"`/`"false"` |
| `char` | `'x'` | one code point; `code()`, `string()`; no `<` |
| `string` | `"text"` | immutable code points; `+` joins strings only; no `<` |
| `@T` | `[1, 2]`, `@@int` | immutable array |
| `%K:V` | `["a": 1]`, `[:]` | immutable map, iterated in sorted key order |
| range | `0..3`, `1..=5` | a value; start ≤ end; no chaining |
| `Maybe<T>` | `Maybe.some(1)`, `Maybe<int>.none` | absence (5) |
| `Presence<T>` | `Presence.some(v)`, `Presence<T>.none`, `Presence<T>.missing` | set, clear or leave a value |
| `Future<T>` | an async call | 3.10 |
| `(A, B) is R` | an anonymous or bound action | `async (A) is R` when async |
| `any` | any value but an action | opaque: store and pass only |
| object, role, enum | `Rect`, `Shape`, `Unit` | nominal |

**Arithmetic stays in its category.** `+ - * %` keep it, `/` on two `int`s
gives a `rat`, `%` has the sign of the dividend, division by zero is a
runtime failure, and there is no `**` (use `pow`). Mixing categories is a
compile error everywhere, comparisons included.

**Conversions are named:** `n.float()`, `n.rat()`, `x.int()` (toward zero),
`byte(n)`, `n.char()`, `c.code()`, `v.string()`; also `int(x)`, `float(x)`,
`string(x)`, `char(n)` and `rat(a, b)`. Text becomes a number through
`strings/parse`: `Number(text).int()` is strict and throws `InvalidInteger`.

```okkama,run
module numbers;

import "io/stdout" take Stdout;

main() is int {
    let out = Stdout();
    let half = 7 / 2;
    out.writeln(half.string(), " ", int(half).string(), " ", (-7 % 3).string(), " ", (2).pow(64).string());
    out.writeln((1 / 10 + 2 / 10).string(), " ", (0.1 + 0.2).string(), " ", (1.rat() + 1 / 2).string());
    out.writeln((3.float() * 1.5).string(), " ", (byte(250) + byte(10)).string(), " ", (1 << 63).string());
    0;
}
```
```output
7/2 3 -1 18446744073709551616
3/10 0.30000000000000004 3/2
4.5 260 -9223372036854775808
```

**Strings:** `length()` (code points), `slice(start, end)`, `split(sep)`,
`replace`, `trim`, `contains`, `starts_with`, `find(part)` (`Maybe<int>`),
`lower()`, `upper()`, `characters()`, `less(other)`, `order(other)`. Long
text is built with `strings/buffer`'s `TextBuffer` or `strings/format`.

**Arrays `@T`:** `size()`, `xs[i]` (fails out of range; `xs[i] else v`),
`first()`, `last()`, `slice`, `take(n)`, `drop(n)`, `reversed()`,
`replaced(i, v)`, `removed_at(i)`, `contains(v)`, `index_of(v)`, `join(sep)`,
`ordering()` (stable sort; import `compare`'s `Comparing`), `+`
(concatenate), `-` (remove equal elements). A range inside a literal is
spread: `[0, 2..=4]` has four elements. An empty literal needs a declared
type, `let none is @string = [];`, and a mixed one an `@any` boundary.

**Maps `%K:V`:** `size()`, `has(key)`, `keys()`, `values()`, `m[key]` (fails
when missing; `m[key] else v`), `m + ["k": v]` sets, `m - ["k": v]` removes
by key. Keys are expressions: `[name: 1]` uses the variable `name`. A key is
a scalar (not `byte` or `any`), a payload-free enum or a value object (only
scalar, enum or value-object fields, no `destroy`, no caps). A repeated key
in a literal is an error.

**Roles as types:** `@Shape` holds any objects that `does Shape`. A role
reference narrows to another role its object does whose actions it already
offers; it never widens or casts back. **Equality** is by value everywhere:
`1 / 2 == 2 / 4`, collections element by element, objects field by field.
Comparing two different concrete types is a compile error, not `false`.

```okkama,fail
module types;

object Box<T> {
    has value is T;
    Box(:value);
}

enum Stage = (draft, paid(amount is int));

main() is int {
    let box = Box<int>(value: 1);
    let n is int = Maybe.some(1);
    let shown = match Stage.paid(5) { case Stage.draft: "draft"; };
    let mixed = [1, "two"];
    let same = 1 == 1.0;
    let total = 3 * 2.5;
    let label = "n = " + 1;
    0;
}
```
```error
example.ok:11:15: semantic error: A type is not a value
example.ok:12:20: semantic error: Cannot initialize variable "n" of type int with Maybe<int>
example.ok:13:17: semantic error: Non-exhaustive match: missing paid
example.ok:14:17: semantic error: Array elements have incompatible types int and string; declare an explicit @any boundary to allow heterogeneous values
example.ok:15:16: semantic error: Cannot compare int and float
example.ok:16:17: semantic error: Operator * cannot be applied to int and float
example.ok:17:17: semantic error: Operator + cannot be applied to string and int
```

## 5. Expressions and control flow

**Blocks have values:** the last expression, with an optional final `;`.
`if`, `match`, `loop` and `try` are expressions; a bare `{ … }` is a scope
with no value. **`let`** declares and binds at once (`let x = e;`,
`let x is T = e;`); a name is not declared twice in one scope, and an inner
block may shadow it.

**Operators**, loosest first: `else` (fallback), `||`, `&&`, `<-` (write to a
stream), `== !=`, `.. ..=`, `< <= > >=`, `|`, `^`, `&`, `<< >> >>>`, `+ -`,
`* / %`, prefix `! - ~ await spawn panic`, postfix `.`, `::`, calls, `[i]`,
`...`. Bitwise operators bind tighter than comparisons. Conditions must be
`bool`. Operands and arguments are evaluated left to right, once; only `&&`
and `||` short-circuit.

**`if`** has mandatory braces. As a value it needs `else` and one type in
every branch; `else if` chains work everywhere. The compact form
`if c: a else: b` has one expression per branch and does not mix with
blocks.

**`match`** compares the subject with each case by `==`, or takes a variant
apart with `Enum.case(name, _)` (one level deep; no ranges). There is no
fall-through. `case a, b` lists values; `case _` or `else { … }` takes the
rest; `case p: value;` is a compact case. A `match` used as a value, or over
a variant, must cover every case or have a remainder.

```okkama,run
module choosing;

import "io/stdout" take Stdout;

object Grade {
    has score is int;
    Grade(:score);
    letter() is string {
        if score >= 90 { "A"; } else if score >= 75 { "B"; } else { "C"; }
    }
    verdict() is string {
        match self.letter() {
            case "A", "B" { "pass"; }
            else { "retake"; }
        }
    }
}

main() is int {
    for [95, 80, 40] -> score {
        let grade = Grade(score: score);
        let mark = if score > 50: "+" else: "-";
        Stdout().writeln(grade.letter(), " ", grade.verdict(), " ", mark);
    }
    Stdout().writeln(match 3 { case 0: "none"; case 1: "one"; case _: "many"; });
    0;
}
```
```output
A pass +
B pass +
C retake -
many
```

**Loops.** There is no `while`, `do` or C-style `for`.

- `for source -> name { … }` walks an array, a range, `m.keys()` or
  `m.values()`, `text.characters()`, or an object that does `Iterating<T>` or
  `Batching<T>` (`collections/iteration`). One binding; an index comes from a
  range: `for 0..xs.size() -> i`.
- `loop { … }` repeats until `last`, `return` or a failure; `last value;`
  gives the loop a value.
- `next;` and `last;` act on the nearest loop (no labels) and take a postfix
  condition: `next if x < 0;`, `last found if done;`. A postfix `if` also
  works on `return`, `throw` and expression statements.
- Every iteration is a scope of its own.

**Functional operations** on arrays, ranges, maps and `Iterating` objects
take a block after `->`: `map`, `filter`, `reduce(initial)`, `any`, `all`,
`find` (a `Maybe` of the first match). They are eager and in order; maps
bind `key, value` in key order and `filter` gives a map. Their blocks cannot
hold `next`/`last`, and `return` in one leaves the enclosing action. No
other name is accepted. A functional call as a `for` source needs
parentheses.

```okkama,run
module loops;

import "io/stdout" take Stdout;

object Search {
    has items is @int;
    Search(:items);
    first_negative() is int {
        for items -> item {
            return item if item < 0;
        }
        0;
    }
}

main() is int {
    let out = Stdout();
    let squares = (1..=5).map() -> n { n * n; };
    let odd = squares.filter() -> n { n % 2 == 1; };
    out.writeln(odd.map() -> n { n.string(); }.join(","), " ", squares.reduce(0) -> sum, n { sum + n; }.string());
    out.writeln((squares.find() -> n { n > 10; } else 0).string(), " ", squares.all() -> n { n > 0; }.string());
    out.writeln(["b": 2, "a": 1].map() -> key, value { key + value.string(); }.join(" "));
    out.writeln(Search(items: [3, -4, -5]).first_negative().string());
    let rolls = [4, 2, 6, 1];
    let at = loop {
        last rolls.index_of(6) else -1;
    };
    for 0..rolls.size() -> i {
        next if i % 2 == 0;
        out.write(rolls[i].string(), " ");
    }
    out.writeln("six at ", at);
    0;
}
```
```output
1,9,25 55
16 true
a1 b2
-4
2 1 six at 2
```

```okkama,fail
module habits;

main() is int {
    let i = 0;
    while i < 3 { i = i + 1; }
    0;
}
```
```error
example.ok:5:5: syntax error at "while": Okkama has no `while`: write `loop { last if !(condition); ... }`.
  |
5 |     while i < 3 { i = i + 1; }
  |     ^^^^^
```

**Mutable state.** Prefer `reduce`; then a successor rebound in a loop
(3.4); then `cell.Cell<T>` (`Cell(0)`, `value()`, `replace(v)`), the one
mutable container of synchronous code, never a field and never in async
code; and `async/atomic.Atomic<T>` for state that objects or tasks share.

```okkama,run
module cells;

import "io/stdout" take Stdout;
import "cell" take Cell;

main() is int {
    let lengths = Cell(0);
    for ["ab", "cde", "f"] -> word {
        lengths.replace(lengths.value() + word.length());
    }
    let seen is %string:int = [:];
    let counts = ["to", "be", "to"].reduce(seen) -> map, word { map + [word: (map[word] else 0) + 1]; };
    Stdout().writeln(lengths.value().string(), " ", counts["to"].string());
    0;
}
```
```output
6 2
```

**Absence.** `Maybe<T>` is `Maybe.some(v)` or `Maybe<T>.none` (the type
argument may be left out where the context gives it). There is no unwrap:
leave a `Maybe` with `else`, `match` or `try?`. The `else` after a `Maybe`,
an index or a key is lazy and has the payload's type; its right side may be
`throw …` or `panic "…"`, never `return`, `next` or `last`. Fallbacks chain:
`a else b else c`. Inside a branch of a compact `if`, write `(x else y)`.

```okkama,run
module absence;

import "io/stdout" take Stdout;

enum Missing is Error = (user(id is int));

object Directory {
    has names is %int:string;
    Directory(:names);
    find(id is int) is Maybe<string> {
        if names.has(id) { Maybe.some(names[id]); } else { Maybe.none; }
    }
    require(id is int) is string { self.find(id) else throw Missing.user(id); }
}

main() is int {
    let users = Directory(names: [1: "Ada", 2: "Grace"]);
    let greeting = match users.find(1) {
        case Maybe.some(name): "hello, " + name;
        case Maybe.none: "hello, stranger";
    };
    Stdout().writeln(users.find(3) else "nobody", "; ", greeting, "; ", try? users.require(4) else "none");
    0;
}
```
```output
nobody; hello, Ada; none
```

**`return value;`** leaves the action from any depth; it is not allowed in a
constructor, a `destroy` or a test body. **`defer expr;`** runs `expr` when
the current block ends (7).

## 6. Errors and failures

| Failure | Raised by | Catchable |
|---|---|---|
| expected | `throw`, `confirm … else throw`, std operations | yes, by type |
| bug | `panic "…"`, a bare `confirm` that fails, a runtime error (index out of range, division by zero, missing key) | no: the run unwinds, reports, exits 1 |
| budget | `--max-steps`, `--max-memory`, `--max-output`, `--timeout` | no |
| cancellation | `Cancelled`, thrown by `await` | yes |

**Error values.** Only an error enum, `enum E is Error = (…)`, is thrown and
caught; there is no `Exception`, and an object cannot do `Error`. Payload
fields hold scalars and enums (other error enums included), never objects,
collections or resources. Every error answers `text()`, which spells it as
source: `Refused.short(50, 80)`. The built-in role `Error` is also done by
`NativeFailure` (a host failure: `domain()`, `code()`, `message()`) and
`Cancelled`.

- `throw E.case(…);` raises, `throw e;` passes a caught error on, a bare
  `throw;` in a `catch` rethrows.
- `try { … } catch …` is an expression or a statement. Clauses are typed:
  `catch e is E { … }`, `catch e is A, B` (`e` is then an `Error`), or a
  pattern `catch E.case(a, _) { … }`; the first that fits wins.
  `catch e is Error` is the widest and never sees a panic. There is no
  catch-all, no guard and no `finally` (use `defer`).
- The body's scope is closed (defers run, resources are destroyed) before a
  clause runs.
- `confirm c;` is an invariant (a bug when false), `confirm c else throw e;`
  a precondition, `confirm c else panic "why";` an invariant with a sentence.
- `try? e` turns any thrown error into `Maybe.none`; `try! e` into a panic.
- An error raised in a `catch` keeps the handled one as its cause in the
  report; a failure during cleanup is attached as a secondary.

```okkama,run
module errors;

import "io/stdout" take Stdout;

enum Refused is Error = (empty, short(have is int, need is int));

object Account {
    has balance is int;
    Account(:balance);
    withdraw(amount is int) is int {
        confirm amount > 0 else throw Refused.empty;
        confirm amount <= balance else throw Refused.short(balance, amount);
        balance - amount;
    }
}

main() is int {
    let out = Stdout();
    let account = Account(balance: 50);
    let left = try {
        account.withdraw(80);
    }
    catch Refused.short(have, _) {
        have;
    }
    catch e is Refused {
        0;
    };
    out.writeln(left.string());
    out.writeln(try { account.withdraw(0).string(); } catch e is Error { e.text(); });
    out.writeln((try? account.withdraw(99) else -1).string(), " ", try! account.withdraw(10));
    account.withdraw(500);
    out.writeln("not reached");
    0;
}
```
```output
50
Refused.empty
-1 40
exit 1
```

An uncaught error, a panic or a runtime error prints a report on stderr and
exits with status 1:

```text
Refused: Refused.short(50, 500)
  --> errors.ok:12:9
   |
12 |         confirm amount <= balance else throw Refused.short(balance, amount);
   |         ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
Stack trace (most recent call first):
  at Account.withdraw (errors.ok:12:9)
  at main (errors.ok:32:5)
```

```okkama,fail
module catchall;

main() is int {
    try { 1; } catch e { 0; };
    0;
}
```
```error
example.ok:4:24: syntax error at "{": Expect "is" after catch error name.
  |
4 |     try { 1; } catch e { 0; };
  |                        ^
```

## 7. Ownership and resources

Memory is automatic. *Resources*, objects with a `destroy`, are released
deterministically by scope; there are no finalizers and no manual `free`.
The rules follow objects, not names:

- **A scope owns what it creates.** Blocks, loop iterations, branches of
  `if`/`match`/`try`/`catch`, action calls and tasks are scopes. When one
  ends, on any exit (end, `return`, `throw`, panic), its reached `defer`s
  run newest first, then the objects it owns are destroyed newest first.
- **A binding owns a fresh object**; `let alias = a;` borrows it.
- **Parameters and receivers borrow.** A fresh argument that nobody owns
  (`use(Handle(…))`) is adopted by the called action and destroyed when it
  returns, unless the action returns it.
- **A returned object moves** to the caller with everything it reaches; so
  does a value produced by `last value`, a branch or `map`/`reduce`.
- **Containers carry what they hold:** whoever owns an array, map, field,
  closure, `Maybe` or cap owns its contents. Removing a map entry builds a
  new map; the old one, still owned, keeps the resource.
- **An owner is destroyed before its fields**, and fields newest first.
- **A rebind does not destroy the old value early**; it lives until its scope
  ends. In long-running code, replace a resource in a scope that ends (a
  `reduce` step, a loop iteration).
- **An object whose constructor failed is never destroyed**; the fields it
  had bound are.
- An `Atomic` names an object but does not own it.
- `destroy { … }` is not an action: nothing calls it and it runs once. An
  `async destroy` may await and is owned only in async code. Std resources
  also offer an idempotent `close()` for early release.

```okkama,run
module owners;

import "io/stdout" take Stdout;

object Tag {
    has name is string;
    Tag(:name);
    label() is string { name; }
    destroy { Stdout().writeln("  destroy " + name); }
}

object Desk {
    Desk();
    look(tag is Tag) is int { tag.label().length(); }
    make(name is string) is Tag { Tag(name: name); }
}

main() is int {
    let out = Stdout();
    let desk = Desk();
    {
        let a = Tag(name: "a");
        let alias = a;
        defer out.writeln("  defer sees " + alias.label());
        out.writeln("block");
    }
    let kept = Tag(name: "kept");
    desk.look(kept);
    desk.look(Tag(name: "adopted"));
    let made = desk.make("made");
    for ["x"] -> name {
        let t = Tag(name: name);
    }
    out.writeln("end of main");
    0;
}
```
```output
block
  defer sees a
  destroy a
  destroy adopted
  destroy x
end of main
  destroy made
  destroy kept
```

## 8. Program shape and tools

- The entry point is `main() is int` or `async main() is int` in the module
  that is run. It takes no parameters; its result is the exit status.
- Arguments: `cli/arguments`'s `ProcessArguments().strings()` (without the
  program name); `cli/command` parses options.
- Output: `io/stdout` has `Stdout` and `Stderr`, whose `write(…)` and
  `writeln(…)` take any number of values with `string()`, and the stream
  operator `Stdout() <- "text" <- endl`.

```okkama
module tool;

import "io/stdout" take Stdout, Stderr;
import "cli/arguments" take ProcessArguments;

main() is int {
    let arguments is @string = ProcessArguments().strings();
    if arguments.size() == 0 {
        Stderr().writeln("usage: tool NAME");
        return 2;
    }
    Stdout() <- "hello, " <- arguments[0] <- endl;
    0;
}
```

| Command | Does |
|---|---|
| `okkama run app.ok [ARGS…]` | compile and run `main` |
| `okkama run -t app.ok [--filter TEXT]` | run the tests and doc examples instead of `main` |
| `okkama check app.ok` | report every compile-time error; run nothing |
| `okkama fmt -w app.ok` | format in place (`--check` only reports) |
| `okkama lint app.ok` | errors, warnings and lint findings |
| `okkama man [module [Name.action]]` | the reference of std or of your own module |
| `okkama build app.ok -o app.spb`, `okkama exec app.spb` | compile to bytecode, run it |
| `okkama run --max-steps N --max-memory 64M --timeout 5 app.ok` | run under budgets |

A good loop: `okkama check`, `okkama run -t`, then `okkama fmt -w` and
`okkama lint`.

## 9. Grammar summary

EBNF: `[ ]` optional, `{ }` repeated, `|` alternatives, `"…"` terminals;
`Name` is upper-case, `name` lower-case, `NAME` either. The checker adds the
rules of the sections above.

```okkama,ignore
program     = "module" path [ "v" INT "." INT "." INT ] ";" { import } { item } ;
import      = "import" STRING [ "as" name ] [ "take" NAME { "," NAME } ] ";" ;
item        = [ "hidden" ] ( object | role | enum | const ) | test | main
            | "cap" Name "::" Name "{" { NAME "is" type block } "}" ;
main        = [ "async" ] "main" "(" ")" "is" "int" block ;
const       = "const" NAME [ "is" type ] "=" expression ";" ;

object      = "object" Name [ generics ] [ "does" type { "," type } ] "{" { member } "}" ;
member      = "has" name { "," name } ( "is" type [ "=" expression ] | "=" expression ) ";"
            | constructor | [ "hidden" ] action | [ "async" ] "destroy" block
            | "cap" Name "{" { NAME "is" type block } "}" | test ;
constructor = Name params block | Name "(" ")" ";"
            | Name "(" ":" name { "," ":" name } ")" ( ";" | block ) ;
action      = [ "async" ] name params "is" result block ;
result      = "void" | "^" type | type ;
params      = "(" [ group { "," group } ] ")" ;
group       = name { "," name } "is" type | name "is" [ "..." ] type [ "=" expression ] ;
role        = "role" Name [ generics ] "{" { [ "async" ] name params "is" result ";"
                                         | "fake" "{" { member } "}" | test } "}" ;
enum        = "enum" Name [ generics ] [ "is" "Error" ] "=" "(" name [ params ]
              { "," name [ params ] } [ "," ] ")" ";" ;
generics    = "<" Name [ ":" type ] { "," Name [ ":" type ] } ">" ;
test        = [ "async" ] "test" STRING block ;

block       = "{" { statement } "}" ;
statement   = [ "once" ] "let" NAME [ "is" type ] "=" expression ";"
            | name "=" expression ";"              (* successor rebind; field bind in a constructor *)
            | "self" "." name "=" expression ";"   (* constructors only *)
            | "^" name "=" expression ";"          (* successors only *)
            | "for" [ "await" ] expression "->" NAME block
            | "loop" block | if | match | try | block
            | ( "next" | "last" [ expression ] | "return" [ expression ]
              | "throw" [ expression ] ) [ "if" expression ] ";"
            | "defer" expression ";"
            | "confirm" expression [ "else" ( "throw" | "panic" ) expression ] ";"
            | expression [ "if" expression ] ";" ;

expression  = binary [ "else" ( "throw" expression | expression ) ] ;
binary      = unary { operator unary } ;
unary       = ( "!" | "-" | "~" | "await" | "spawn" | "panic" ) unary | postfix ;
postfix     = primary { "." name | "::" name | "[" expression "]" | "..."
                      | "(" [ [ name ":" ] expression { "," [ name ":" ] expression } ] ")"
                        [ "->" name { "," name } block ] } ;
primary     = literal | NAME | "self" | "(" expression ")"
            | "(" [ group { "," group } ] ")" "is" ( "void" | type ) block    (* anonymous action *)
            | "[" [ expression { "," expression } ] "]"
            | "[" ":" "]" | "[" expression ":" expression { "," expression ":" expression } "]"
            | if | match | "loop" block | try | ( "try?" | "try!" ) binary ;
if          = "if" expression block [ "else" ( if | block ) ]
            | "if" expression ":" expression "else" ":" expression ;
match       = "match" expression "{" { "case" ( "_" | expression { "," expression } )
                ( block | ":" [ "throw" ] expression ";" ) | "else" block } "}" ;
try         = "try" block { "catch" ( NAME "is" type { "," type } | expression { "," expression } ) block } ;
type        = "@" type | "%" type ":" type
            | [ "async" ] "(" [ type { "," type } ] ")" "is" ( "void" | type )
            | NAME { "." NAME } [ "<" type { "," type } ">" ] [ "::" Name ] ;
```
