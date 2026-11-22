# Chapter 13. Actions, successors and immutability

Chapter 12 built objects that never change. This chapter is about what they
do: actions, which answer questions about an object, and successors, which
answer the question "what would this object be after …?" with a new object.
Between them they cover everything other languages do with methods,
setters and mutable fields.

{{example:13-successors}}

The program keeps a bar tab:

- `ordered` and `closed` are successors. Each returns a new `Tab` and leaves
  the one it was called on as it was;
- `empty` still has no items after `two` was made from it;
- the loop moves the name `tab` from one successor to the next;
- a refused order throws, and `tab` is unchanged after the failure;
- a closed tab refuses every order.

## 13.1 An action is a typed request

```okkama
summary() is string {
    items.string() + " items, " + total.string() + …;
}
```

An action has a name, a parameter list and a declared result. The result
declaration is mandatory: `is <type>`, or `is void` for an action called for
its effect. Leaving it out is a syntax error. There is no inference of
results across a declaration. A reader of the signature knows what comes
back without reading the body.

The value of an action is its final expression. `return value;` leaves
earlier:

```okkama
first_negative(values is @int) is int {
    for values -> value {
        if value < 0 { return value; }
    }
    return 0;
}
```

- `return` leaves the *action*, not the block it is in. Blocks, loops,
  `if`, `match` and `try` are not return boundaries. An anonymous action
  is.
- A functional iteration block (`map`, `reduce` and the like, with `->`)
  is not a boundary either. A `return` inside it returns from the
  enclosing action, not from the block.
- `return` takes exactly one expression. To return two values, return an
  array or an object.
- The value is computed before cleanup. Then the `defer`s that were reached
  run, and the owned objects are destroyed (Chapter 11). The returned object
  itself stays alive for the caller.
- `return` is not allowed directly in a constructor, a `destroy` or a test
  body.

`void` is a result marker, not a type. There is no `void` value, no `Unit`
and no `null` behind it. A field, a parameter, a variable or a collection
element cannot have it as a type. Using the absent result of a void call as
a value is a compile error. The final expression of a void body is
evaluated and discarded.

An action that returns its own receiver says so: `touch() is Cash { self; }`.
That is the only way to return `self`, and it needs both the explicit
result and the explicit `self`.

## 13.2 Parameters and calls

{{example:13-actions}}

- **Grouped names.** Names before one type share it: `full(separator,
  suffix is string)`. The grouping works in every parameter list except
  for a variadic parameter, which takes its own type.
- **Positional or named, never both.** `hello.greet("Ada", "?")` and
  `hello.greet(mark: ".", name: "Alan")` are calls; a mix of the two is an
  error. Named arguments may come in any order.
- **Defaults.** Trailing parameters may declare a default:
  `greet(name is string, mark is string = "!")`. A call leaves them out from
  the end positionally, or in any combination by name. A parameter after a
  defaulted one needs a default too.
- **Where a default is computed.** The default is evaluated at each call,
  in the *declaring* module. It sees module names, not the caller's locals
  and not `self`. Moving a call to another module never changes what a
  default means.
- **Evaluation order.** The receiver and the arguments are evaluated once,
  left to right, in the caller's scope.

Variadic actions, role signatures, natives and enum payloads have no
defaults.

## 13.3 Overloads

An object may declare several actions with one name. They differ by the
number of parameters, by their types, and, for named calls, by parameter
names. `describe(int)`, `describe(string)` and `describe(any)` in the
example are three actions. The call with `1.5` takes the `any` one, because
neither of the others accepts a float.

The rules are short, and they never guess:

1. Candidates with the wrong arity or the wrong names drop out. Then
   argument types are checked against declared types.
2. If a fixed candidate fits, variadic candidates are not considered.
3. Exact concrete types beat role-compatible types, and both beat `any`.
4. A candidate wins only if it is no less specific in *every* position and
   more specific in at least one.
5. Declaration order never breaks a tie. A tie is an error.
6. Overloads of one name and arity may declare different results when
   their parameter types differ. A call gets the result of the overload it
   selects: `minus(duration is Duration) is Instant` and
   `minus(other is Instant) is Duration` sit side by side (§14.7).

Rule 4 makes `(int, any)` and `(any, int)` ambiguous for a call with two
integers. Each is better in one position and worse in the other:

{{example:13-ambiguous-rejected}}

The message names the argument types and the candidates that tie, so the
fix is visible: convert one argument, or narrow one overload. Until 0.4.2
it named neither (#273). The no-match message lists the types too:
`No action "joined" on object "Pairing" accepts (string, string)`.

Overloading is resolved before execution from static types. A value typed `any` is not narrowed to its contents to choose an overload. Okkama has no dispatch on argument types at run time; dispatch on the receiver happens through roles (Chapter 14).

## 13.4 Variadic actions

`total(values is ...int)` takes zero or more integers after its fixed
prefix:

- The variadic parameter is the last one, unique, explicitly typed and has
  no default.
- Calls to it are positional. Each value in the tail is checked against the
  element type.
- Inside the body, `values` is not an array. It is an opaque binding that
  can only be unpacked with `values...`: as the whole value of a `let`, as
  the whole value of a constructor field bind, as a `for` source, or as the
  last argument of another variadic call.
- A fixed candidate that fits wins over a variadic one. `total(1)` picks
  `total(a is int)` if both exist, and `total()` or `total(1, 2)` picks the
  tail.

`hello.total()` in the example is `0`: the tail is empty, and
`reduce(0)` over an empty array returns its seed.

A `...string` tail turns the values it is given into text. A value that is
not a string reaches it through its own `string()`: `int`, `float`, `rat`,
`byte`, `bool`, `char`, and any object or role value whose type answers
`string() is string`. So `out.writeln("count: ", 5, ", ready: ", true)`
prints `count: 5, ready: true`, and `Formatted("{} version {}", id,
id.version())` fills its slots. The conversion is chosen at compile time and
only as a last resort: a signature that fits the call as written wins, so
`kind(1, "a")` still picks `kind(values is ...any)` over `kind(values is
...string)`. A value without `string()` is a compile error, and a spread
`values...` is not converted.

## 13.5 Successors

Fields never change (Chapter 12). A state change is therefore a new
object, and the language has a construct for making it: an action whose
result type is `^Type`.

```okkama
ordered(price is int) is ^Tab {
    confirm open else throw TabError.closed;
    confirm total + price <= limit else throw TabError.over_limit(total + price);
    ^items = items + 1;
    ^total = total + price;
}
```

- `^field = value;` *stages* a new value for a field. It is allowed only in
  a successor action. Anywhere else it is an error:

  {{example:13-staged-rejected}}

- When the action finishes, the result is a new object. Every field is
  copied from the receiver, and the staged fields are replaced. The receiver
  is untouched: `empty` stays at `0 items` after `two` is made from it.
- The successor is a new identity with its own lifetime (Chapter 11).
- On an object, a successor returns the object's own type. `is ^Tab` on
  `Tab` is legal; `is ^Package` is not, and a `^Role` result belongs in a
  role signature (Chapter 14).
- The body's final expression is discarded, as in a void action. Ending a
  successor with a value of its own type is an error, because it would read
  as a replacement result that is then thrown away:

  {{example:13-replacement-rejected}}

Two details matter when a body stages several fields.

**Reads see the receiver, not the staged values.** After `^items = items + 1;`, the name `items` still means the receiver’s original field.

A successor that stages `^count = count + 1; ^twice = count * 2;` sets
`twice` from the *old* count. The order of the staging statements does not
change what the reads see.

**The last stage wins.** `^count = 10; ^count = 20;` makes a successor with
`count` 20. Each staging statement replaces the value staged before it;
there is no error.

## 13.6 The result must be used

A successor that is called and then dropped is almost always a bug: the
caller meant to change the tab and kept the old one. Okkama makes the
common form of that bug a compile error:

{{example:13-unused-rejected}}

The compiler tracks every successor result from where it is made to where
it is *consumed*. A result is consumed when it is:

- returned, explicitly or as the final expression;
- used as the receiver of another call, as in `empty.ordered(30).ordered(25)`;
- passed as an argument;
- bound to a field in a constructor or staged in a successor;
- stored in an array, a map or another aggregate;
- used in an eligible rebind (§13.7).

`let unused = bottle.open();` is not enough if `unused` then leaves its
scope without a use. Every normal path has to consume the value. A use
only inside a `try` does not count for the path through its `catch`, which
starts from what was pending when the `try` began. That is why the example
calls `done.summary()` once before the `try`. Without that line, the
compiler reports that `done` "can leave scope without consumption", at the
binding (before 0.5.0 it pointed at the first line of `main`, #272).

The check is not linear typing. A consumed successor can be used again.
Aliases share one obligation, and consuming any of them is enough. The rule
is about the result being *looked at*, not about it being used once.

## 13.7 Rebinding a name

A loop that advances a value needs a name that moves with it. A bare
`name = expr;` without `let` rebinds an existing local:

```okkama
let tab = Tab(limit: 100);
for [10, 20, 30] -> price {
    tab = tab.ordered(price);
}
```

The rule for rebinding is deliberately narrow. The local must already
exist, and its declared type must be an object with at least one real
`^`-marked successor action. A plain action that happens to return the
same type does not qualify. Neither do scalars, arrays, maps or enums:

{{example:13-rebind-rejected}}

The narrowness is the point. A rebind is allowed exactly where the type's
author declared that the type has successors, so a name can only be moved
along a path of states the author designed. `n = n + 1` has no such path:
`int` is a value, not a state machine.

- The new value must satisfy the declared type of the binding.
- A rebind in a nested scope updates the scope that declared the name. That
  is what makes the loop above work: each iteration is its own scope.
- The previous value is not destroyed early. If it owns resources, its
  `destroy` runs at the normal end of its owner's scope (Chapter 11).

## 13.8 Successors and failure

A successor that fails produces nothing. If `confirm` throws in the middle
of `ordered`, no partial `Tab` exists. The staged values are dropped, the
resources the successor itself owned are destroyed, and the receiver
remains what it was:

```text
TabError.over_limit(110)
3 items, 60 (open)
```

This is what makes successors safe to call in a `try`. Nothing needs to be
rolled back, because nothing was changed. In a language with mutable
fields, a method that fails after updating two of three fields leaves the
object half-changed. The usual remedy is copying the state first and
swapping at the end. A successor does that copy-and-swap by construction.

## 13.9 Naming a transition

A successor describes a domain transition: `opened()`, `ordered(price)`,
`closed()`, `enqueued(item)`. It is not a setter in disguise. The parser
rejects the most common disguise outright:

{{example:13-with-rejected}}

Other names will compile, so the rest of the rule is a matter of design.
One successor per field, or a chain of successors that configure an object
step by step, is a public setter API with extra copies. Elegant Objects
argues that getters and setters turn an object into a data structure with
methods attached [@bugayenko2016]. A `Tab` with `with_limit`,
`with_items` and `with_total` would let any caller build any combination,
including the ones `ordered` refuses.

The alternatives:

- complete state in a constructor, when the values are known together;
- a successor named after the event that changes them, which checks the
  transition (`ordered` checks the limit);
- a new composed object, when a part of the state changes independently.

Meyer's command–query separation distinguishes queries, which return a
value and change nothing, from commands, which change state and return
nothing [@meyer1997]. Okkama keeps the separation and changes what a
command is. A query is an ordinary action. A command is a successor: it
returns the changed object instead of changing this one.

## 13.10 Composition

An object whose part changes stages a successor of that part:

{{example:13-composition}}

`entered` stages two fields: `visits`, and `lamp` replaced by
`lamp.switched()`. The successor of a `Room` holds a successor of its
`Lamp`. The original `hall` still has its old lamp, which is off. The
change travels up the chain of owners. Each level makes a new object, and
everything the change does not touch is shared.

A successor can be hidden. Then it is a transition only the object's own
actions take through `self`:

```okkama
object Door {
    has open is bool = false;
    Door();
    toggled() is Door { if open { self.shut(); } else { self.opened(); } }

    hidden opened() is ^Door { ^open = true; }
    hidden shut() is ^Door { ^open = false; }
}
```

`toggled` is a plain action that returns a `Door`. It is not a successor,
so it cannot stage anything itself. It picks the transition and returns
its result. A hidden successor keeps the must-use rule and cannot
implement a role's action.

## 13.11 When a value must change: `Cell`

Some state really is a variable: a running total, a parser's position,
headers gathered while reading. `reduce` covers most accumulations. Where
it reads badly, the standard library has one explicit mutable container,
`Cell` from the module `cell`:

```okkama
let lengths = Cell(0);
for ["ab", "cde", "f"] -> word {
    lengths.replace(lengths.value() + word.length());
}
lengths.value();    // 6
```

A cell is a fenced exception to immutability:

- It has three operations: `Cell(value: initial)`, `value()` and
  `replace(value)`. The values it holds are ordinary immutable values.
  Cells compare by identity, not by content.
- It stays in synchronous code. An async action cannot create, receive or
  use one. A synchronous action that uses a cell may still be called from
  async code. Synchronous code has no suspension point, so no task can see
  a cell between a read and a write.
- An object cannot hold one:

  {{example:13-cell-field-rejected}}

The message points to the other exception: state shared between tasks is
`async/atomic`'s `Atomic`, with compare-and-replace and an explicit
`close()` (Chapter 20). An object's fields stay immutable even then. The
object holds the same `Atomic` for its whole life, and only what is inside
the `Atomic` changes.

## 13.12 What a successor costs

A successor keeps unchanged fields and replaces the fields named by the transition. Objects held in unchanged fields retain their identities; a transition does not duplicate everything the receiver reaches.

So the cost of a transition is proportional to the number of fields of the
object being copied, not to the size of everything it reaches. A change
deep inside a composition copies the path from the changed part up to the
outer object. Persistent data structures use the same path copying:
the new version shares every untouched branch with the old one
[@okasaki1998]. Clojure built its whole model of state on that sharing
[@hickey2008].

Two consequences for design:

- **Wide objects make transitions more expensive.** Group fields that change together into a part to keep transitions small.
- **A long successor loop creates a value per step.** For a hot loop over a large count, consider `reduce` over a range or a scalar `Cell`.

## 13.13 A value computed once: `once let`

Immutability makes one kind of cache safe without any discipline. An answer
that depends only on an object's fields cannot change while the object
lives, so the object can keep it. `once let` says so in the action that
computes it:

```okkama
hidden values() is @int {
    once let kept = source.items();   // read once per object
    kept;
}
```

The first successful evaluation is remembered for that object; later calls reuse it (#491). Several actions can share a result by calling one hidden action. The following restrictions apply:

- A `once let` reads fields, `self`, constants and earlier `once let`s, never a call parameter.
- It must be free of effects: no clock, file, network, process, randomness, `Cell`, async object, `await` or `spawn`. Calls through roles rely on their contracts.
- Its value must not retain a resource. A closure or role value is allowed only when it cannot retain a resource in this program (issue #520).

The remembered result does not participate in `==`, map keys or `Dumper`. A successor computes its own result on first use. If evaluation throws, the next call tries again.

`once let` is the reason a constructor in the standard library never does
work: an object binds its input when it is made, and computes from it on
the first question.

## 13.14 Async successors

A successor that has to wait before it can build the new state is declared
`async`, and its caller awaits the new object.

{{example:13-async-successor}}

- **It is still a successor.** The awaits yield `first` and `second`, and
  the receiver `opened` still has its 100. Waiting changes when the new
  object exists, not what happens to the old one.
- **A failure produces nothing.** The refused deposit throws inside the
  successor, so no new account is made and `second` stays at 350, as for a
  synchronous successor (Section 13.8).
- **The obligation follows the Future.** `await opened.deposited(200);`
  with its result thrown away is rejected, exactly like the synchronous
  call. Until it is awaited, the obligation belongs to the Future, which
  must be awaited or handed on: passed, returned, stored or spawned. A
  Future bound to a name and then dropped is rejected at the binding, and
  so is one that is only asked `done()` or `cancelled()`.
- **Cancelling is a decision.** `f.cancel()` discharges the obligation.
  The program says in its text that the transition will not happen, so
  there is nothing left to lose, and no `await` is needed just to observe
  `Cancelled`. Forgetting the Future silently is still an error. This was
  the language owner's choice when the check was made complete (#754).

## 13.15 A worked example: an order's life

Successors are how an object goes through stages without a mutable field.
An order is drafted, paid, shipped and delivered; each transition is a
successor action that checks the stage it starts from.

{{example:13-lifecycle}}

- **Each step returns the next order.** `order.pay(1250).ship().deliver()`
  is a chain of three successors. The original `order` is still a draft
  afterwards, printed as such: nothing was updated in place, and both values
  can be used side by side.
- **A transition guards its own precondition.** `ship()` on a draft is
  refused with a typed error that names the stage and the action; the caller
  decides what to do with it. There is no "status" field that some code path
  forgot to check.
- **A transition cannot be ignored.** Calling `order.pay(1250);` as a
  statement is rejected: "Successor action result must be used". A step whose
  result is dropped is almost always a bug — the programmer believed the
  order changed — so the language refuses it (§13.6).
- **Only the changed fields are written.** `pay` sets `^stage` and
  `^paid_cents`; `ship` sets only `^stage`, and the successor keeps every
  other field of its receiver.

The same life could be an enum of stages with a `match` per transition
(Chapter 16). Successors fit when the stages share most of their data and
the transitions are actions of one object; variants fit when each stage
carries different data.

## 13.16 Immutability elsewhere

| | Okkama | Java | Kotlin | Rust | Clojure |
|---|---|---|---|---|---|
| Fields can change after construction | never | unless `final` | `var` yes, `val` no | through `mut` | no (values) |
| Copy with changes | successor (`^field`) | by hand | `copy(x = …)` | `Struct { x, ..old }` | `assoc` |
| Mutable escape hatch | `Cell` (sync), `Atomic` (tasks) | any field | `var` | `Cell`, `RefCell`, `Mutex` | `atom`, `ref` |

Two things set Okkama apart. The copy is a named transition that can
check its input, rather than a generic copy of selected fields. And
dropping the new value is a compile error. Most languages say nothing about
a discarded result; Rust warns, but only for functions and types marked
`#[must_use]`. Kotlin's
`copy` and Rust's struct update syntax make any combination of fields
reachable from any caller [@kotlindata; @klabnik2023]. A successor makes
reachable only what its author named.

## 13.17 What this design costs

- **More allocation.** Every state change is an object. Most are small
  and short-lived, but a design that ignores the cost of wide objects
  (§13.12) pays it on every step.
- **Ceremony for simple counters.** `^count = count + 1` in an action with
  `is ^Counter`, plus a rebind at the call site, where another language
  writes `count++`. `Cell` exists for the cases where that ceremony buys
  nothing.
- **The must-use check is conservative.** A use only inside a `try` does not
  count for the `catch` path, and loops cannot prove a use by mentioning
  the value in a body that may not run. Sometimes the fix is an extra use
  that a human can see is unnecessary.
- **No setters, by rule.** A team used to builders and `withX` methods has
  to find the domain transitions instead. That is the intent, and it is
  also work.

In return:

- an object never changes under the code that holds it;
- a failed transition leaves nothing to roll back;
- a forgotten state change is a compile error;
- every legal state change has a name the author chose.
