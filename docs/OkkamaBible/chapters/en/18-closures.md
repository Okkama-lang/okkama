# Chapter 18. Anonymous actions and closures

Sometimes the piece of behaviour a caller wants to hand over is too small to
deserve a name: a key to sort by, a check to apply, one step of a pipeline.
Okkama lets an action be written where it is used, as a value, without a
name. Such an *anonymous action* remembers the values it saw when it was
created, so it can be returned, stored and called long after the code that
made it has finished. That memory is what makes it a *closure*.

{{example:18-closures}}

- `make_adder` is an anonymous action that returns another one. The inner
  action uses `amount`, a parameter of the outer one, and keeps it:
  `add_ten(5)` is 15 after `make_adder` has returned.
- `Apply.value` takes an action as a parameter, typed `(int) is int`, and
  calls it like any action.
- An anonymous action can be passed by name, `operation: (value is int) is int { … }`,
  like any other argument.
- `Pipeline` stores a list of actions of one type and applies them in order.

This chapter covers how anonymous actions are written and typed, what a
closure remembers and what it does not, why Okkama's closures are values
and not variables, how they compare for equality, how they serve as data,
how the engines run them, where the idea came from, and how they compare
with lambdas in other languages.

## 18.1 Writing an anonymous action

An anonymous action uses the declaration syntax of a named action with the
name left out:

```okkama
() is int { 42; }
(value is int) is int { value * 2; }
(left is int, right is int) is int { left + right; }
(message is string) is void { log.write(message); }
```

Every parameter has a type, and the result is always declared, `void`
included. Nothing is inferred from the body: the declaration is the whole
contract. A list of parameter names followed by `is`, or an empty `()`
followed by `is`, starts an anonymous action; ordinary parentheses still
group expressions, so `(value)` and `(a + b)` mean what they always did.

The type of an anonymous action is its signature without the names:

```okkama
() is int
(int) is int
(int, int) is string
((int) is int) is (int) is int     // takes an action, returns an action
```

Such types appear wherever types do: in fields, parameters, results,
collections (`@(int) is int`) and type arguments.

An action value is called with parentheses and positional arguments, exactly
as many as its type has parameters. A named call is rejected, because the
names belong to the anonymous declaration and not to the type:

{{example:18-positional-rejected}}

### Not the blocks of `map` and `for`

The arrows in `values.map() -> value { … }` and `for values -> value { … }`
belong to other constructs. Those blocks are not values; they cannot be
stored or returned, and they run inside the expression that holds them. The
compiler lowers them to loops in the enclosing function. An anonymous action,
by contrast, is a value with its own frame. That difference shows in control
flow: `next` and `last` in a `for` block continue or leave the loop, but
inside an anonymous action they have no loop to act on, even when the action
is created inside one:

{{example:18-escape-rejected}}

A call of an action value is a boundary for control flow, as any action call
is. A `throw` inside it travels out to the caller, and a `defer` inside it
runs when that call ends.

## 18.2 What a closure remembers

When an anonymous action is evaluated, it captures the bindings it uses from
the scope around it: locals, parameters, and `self` if it reads the
receiver. It does not capture the scope itself.

{{example:18-capture}}

The example shows the three rules.

- **A capture is a snapshot.** `early` is created while `counter` holds a
  count of 1. `counter = counter.bumped()` then rebinds the local to a
  successor (Chapter 13), and `late` captures that one. Calling `early` later
  still sees the count it captured: rebinding a name outside the action does
  not reach into it.
- **Objects keep their identity.** The snapshot holds the object, not a copy
  of it. Okkama objects are immutable, so a captured object always answers
  the same way, and nothing is lost by sharing it.
- **Reading a field captures the receiver.** `reader()` returns an action
  that reads `count`. The action keeps its `Counter` alive after `reader()`
  returns, as a closure over `self`.

Only the names the body actually uses are captured. A large local next to a
closure that never mentions it is not kept alive by the closure.

### Values, not variables

In many languages a closure captures *variables*. A JavaScript closure in a
loop over `var i` famously sees the last value of `i`, and Go before
version 1.22 shared one loop variable among all the closures made in a
loop [@gospec]. Kotlin and C# let a lambda assign to a captured local and
the change is visible outside. Java forbids that: a captured local must be
*effectively final*, never assigned after its declaration [@gosling2023].

Okkama goes further than Java. Its locals are immutable to begin with, and a
rebind (Chapter 13) is the only way a name gets a new value, so there is
nothing for a closure to share but values. A closure is equivalent to a
small immutable object that holds the captured values in fields and has one
action. The equivalence is old: Landin introduced closures as a pair of a
function and the environment it was created in [@landin1964], and Scheme
made them the basis of a whole language [@sussman1975].

Rebinding a captured name *inside* the action is an error: `Cannot rebind
captured variable "counter" inside an anonymous action`. Until 0.4.2 the
compiler accepted it, and the rebind changed a copy that lived for one call
(#321). The action's own parameters and locals, including one that shadows a
captured name, stay rebindable; state that must outlive a call goes back to
the caller as a successor.

### Closures made in a loop

The difference shows in the classic test: one closure created in each pass
of a loop.

{{example:18-loop}}

Each pass of the `map` block binds a fresh `n`, and each anonymous action
captures the value of its own pass, so the three actions answer 10, 20 and
30. In JavaScript with `var`, the same three functions would all answer 30,
because they share one variable that ends at the last value. Go behaved the
same way for its loop variables until version 1.22 changed the rule after
years of bugs [@gospec]. In Okkama there is no variable to share, so the
question does not arise.

### Captured successors

The result of a successor action (Chapter 13) must be used: a binding to it
that can leave its scope unused is an error. The rule follows the successor
into closures. A closure carries the obligation of each captured successor
that its body uses on every normal path. Creating the closure, or binding it
to a name, does not use the successor; calling the closure does, and
returning it, passing it or storing it hands the obligation on with it:

{{example:18-successor}}

`alias` and `show` are one closure, so one call through either name uses
`later`. A closure that is never called, returned, passed or stored keeps
the obligation, and the error names the successor and the action value that
carries it:

{{example:18-successor-rejected}}

Two refinements keep the rule honest. A body that uses the successor on one
branch only does not carry it, since a call might not use it; the successor
must then be used elsewhere. And handing a closure on is a transfer, not a
call: like a successor passed as an argument, it moves the responsibility
without proving that the receiver ever calls it. Until 0.4.2 the compiler
did not look inside closures at all, so even a called closure left its
successor unused (#322). A bound action, `later.count` without the call,
follows the same rule since 0.4.2: it carries its receiver's obligation until
it is called or handed on, while a direct call `later.count()` uses the
receiver at once (#324).

When the closure needs only what the successor knows, capturing that is
still the simpler program: `let count = later.count();` and then
`() is int { count; }` holds a number, not an object with an obligation.

## 18.3 Types are exact

An action value has exactly the type it was declared with. A `() is Square`
is not a `() is Shape`, even though every `Square` is a `Shape`:

{{example:18-exact-rejected}}

This is the same rule as for generic types in Chapter 17: no variance, in
either the parameters or the result. A caller that wants a `() is Shape` has
to write one, `() is Shape { Square(side: 2); }`, and the wrap to the role
happens inside.

Action types take part in everything else types do. Generic inference reads
them (`SortedBy` learns `K` from the key's declared result, Section 17.3),
overload resolution compares them exactly, and roles may declare actions
that take or return them.

Two boundaries stay closed. An action value cannot become `any`, and it
cannot cross the native ABI:

{{example:18-any-rejected}}

`any` is a box that may only be passed and stored (Chapter 4); a function in
such a box could be neither called nor compared, and a native provider has no
way to call back into Okkama code. The second rule may relax when natives
get callbacks.

There is also no automatic conversion from an action to a role. A role with
one action, such as a `Comparing<T>`, is not satisfied by a matching
anonymous action; it needs an object that `does` it. Java's lambdas, which
become instances of any *functional interface* with one abstract method, are
the opposite choice.

### Equality is identity

Two action values are equal only when they are the same value:

{{example:18-identity}}

`one` and `alias` are one closure under two names, so they are equal.
`another_one` was made by the same action from the same argument, and `twin`
has the same text as `literal`, but each is a closure of its own, and
neither equals its look-alike. Objects are the opposite case: two `Maker()`
are equal, because objects compare by type and fields (Chapter 12). An
object that holds an action in a field compares that field by identity too:
two objects holding the same closure are equal, two holding different but
identical-looking closures are not.

Futures follow the same rule, for the same reason: there is no useful
structural equality to offer. Whether two functions give the same result for
every argument cannot be decided in general, by Rice's theorem [@rice1953].
Comparing their code and captured values instead would make equality depend
on how an engine compiles an action, which Okkama does not let into a
program's behaviour. Identity is cheap, predictable and the same on every
engine. An action value is not a map key either: keys are scalars and value
objects (Chapter 8).

## 18.4 Action values as data

Action types are ordinary types, so action values go where values go: into
arrays, maps, fields, generic objects and `Maybe`, and in and out of other
actions.

{{example:18-table}}

- `operations` maps strings to actions of type `(int, int) is int`. The
  type is written `%string:(int, int) is int`, with the action type in place
  of the value type. Picking an action is an ordinary lookup, and a missing
  key takes the `else` branch like any map read (Chapter 8), here with a
  fallback action written in place. A table of actions is how dispatch by
  name is written without a chain of `if`s.
- `Rule` keeps a predicate in a field and calls the field like an action,
  `accepts(value)`. The predicate was created in `main` and captured
  `limit`, so the rule carries that limit with it.

Generic objects take action types the same way. A
`let box is Box<(int) is int> = Box(item: …)` holds one action, and a
`Maybe<(int) is int>` holds one or none; `chosen else (value is int) is int
{ value; }` falls back to the identity action. Inference reads the declared
signatures, as in Chapter 17.

An action can also take actions and return one:

{{example:18-compose}}

`then` returns a closure over its two arguments. `up_then_square(4)` is
`(4 + 1)²`, 25, and `square_then_up(4)` is `4² + 1`, 17. Haskell writes
composition as one operator, `.`; in Okkama it is an action of an object
with a full signature, `((int) is int, (int) is int) is (int) is int`. That
is longer, and it leaves nothing to infer.

## 18.5 Closures or objects

An anonymous action and a small object can do the same job. The pipeline of
the first example could hold `Step` objects with an `apply` action instead
of `(int) is int` values. Which one to write?

A named object costs a declaration and gives back a name, a place for
documentation, the ability to implement roles, structural equality, and a
`destroy` if it owns a resource. A closure costs nothing to declare and is
right where it is used. The guidance that falls out of the language:

- **A closure for a parameter of behaviour.** A sort key, a predicate, a
  transformation passed to one call. Its type says everything there is to
  say.
- **An object when the behaviour has a name in the domain.** A discount
  policy, a retry strategy, a parser. It will be passed around, compared,
  tested with a fake (Chapter 15), or given a second action.
- **An object when there is state over time.** A closure is a snapshot; an
  object can have successors.

*Elegant Objects* argues for objects everywhere [@bugayenko2016], and Okkama
mostly agrees: there are no free functions, so a closure is always created by
some object's action and usually handed to another object. The anonymous
action is the escape hatch for behaviour too small to name.

Closures also carry authority. An action that can write to a log, created
where the log is in scope and handed to code that cannot see the log, gives
exactly that ability and no other. This is the object-capability pattern of
Miller's thesis, where a reference is the only way to an effect [@miller2006];
in Okkama a closure is one more kind of reference.

### Behaviour that waits needs an object

An anonymous action is always synchronous. It cannot be declared `async`
(since 0.4.2 the parser says so at the `async`, #323), and an `await` inside
it is rejected even when the closure is created inside an async action:

{{example:18-await-rejected}}

The closure is an action of its own, and a synchronous one, so the `await`
has no async action around it. Behaviour that has to wait is therefore
passed as an object that does a role with an async action, and closures
stay on the synchronous side:

{{example:18-async}}

`Loader` takes both kinds. Its source is a `Fetching`, a role with an async
`fetch`; its label is a plain `(string) is string`. In `load` the closure
runs after the `await`, with the `prefix` it captured in `main`, and nothing
it holds can have changed during the wait, because what it holds are values.
This is where the snapshot rule pays off: a closure that is called across an
`await` needs no thought about what other tasks did in the meantime.

The restriction keeps every suspension point in one kind of place: an
`await` inside a named `async` action, whose declaration says that it waits
(Chapter 20). A type such as `(int) is int` promises that calling the value
does not suspend. Kotlin draws the same line in its types with `suspend`
function types, and JavaScript with `async` functions, but both also allow
anonymous functions that suspend; Okkama does not.

## 18.6 How closures run

In the IR a closure is two instructions. `fn.closure` builds the value from a
function and its captured values; `call.fn` calls it:

```text
func @main() -> int [entry] {
^b0:
    %0: fn(int) -> fn(int) -> int = fn.closure @lambda#0 []
    %1: int = const 10
    %2: fn(int) -> int = call.fn %0(%1)
    %3: int = const 5
    %4: int = call.fn %2(%3)
    ret %4
}

func @lambda#0(%0: int) -> fn(int) -> int [lambda 0] {
^b0:
    %1: fn(int) -> int = fn.closure @lambda#1 [%0]
    ret %1
}

func @lambda#1(%0: int, %1: int) -> int [lambda 1] {
^b0:
    %2: int = int.add %1, %0
    ret %2
}
```

The body of each anonymous action is a function of its own, `@lambda#N`. Its
first parameters are the captured values, then the declared parameters:
`@lambda#1` gets `amount` as `%0` and `value` as `%1`. A closure is the
function and those first arguments, packed when the closure is created.

- **The IR engine** stores a closure as a reference-counted value holding
  the function and the captured values.
- **Tomato** allocates a heap object laid out like the function's
  parameters, whose first words hold the captured values, so the GC knows
  which words are references. Creating a closure and calling one both go through a
  runtime helper: the arguments are spilled to memory and the helper
  dispatches to the function's machine code.
- **The JVM** gets one class per anonymous action, implementing
  `okkama.rt.Fn`; its fields hold the captured values, and `call.fn` is an
  interface call that the JVM's JIT can inline.

On a loop of ten million calls that add a captured number (best of five on
an Apple M2 Max, including compilation), calling a closure and calling an
action of an object that holds the same number compare as follows:

| | object action | closure |
|---|---:|---:|
| Tomato | 0.31 s | 0.41 s |
| IR engine (1 million calls) | 0.97 s | 0.93 s |
| JVM | 0.59 s | 0.49 s |

On Tomato the helper costs about ten nanoseconds per call over a direct
call. On the JVM the closure is slightly faster: the class of each anonymous
action has one implementation of `call`, which the JIT inlines.

The micro-benchmarks of 0.4.2 (#244) isolate the creation. Net of an empty
loop, median of five runs on the same machine, creating a closure that
captures a loop variable and calling it once costs 17 ns on Tomato, 155 ns on
the IR engine and 6 ns on the JVM. Constructing an object with two fields
costs 4, 271 and 9 ns. On the JVM a closure costs no more than a small
object; on Tomato it costs four times as much, because creating and calling
it both go through the runtime helper, while an object is allocated inline
in the compiled code.

### Closures and resources

A closure that captures a resource, such as an open file, owns it the way an
object field would (Chapter 11): returning the closure moves the file out of
the scope that created it, and the file is closed when the region that ends
up owning the closure ends. To know which closures can own a resource, the compiler
looks at what each anonymous action captures across the whole program. Until
0.4.1 it did not: in a program with a `destroy` anywhere, every function
value was assumed to own one and paid for region tracking. A program as
small as `benchmarks/09-closures` with `os/environment` imported ran 6×
slower on the JVM than without that import (#292). Since 0.4.1 a closure
that captures only plain values is treated as plain, and that benchmark runs
at the same speed with or without the import (IR 1.02 s, Tomato 0.10 s, JVM
0.22 s).

### In a failure report

A failure raised inside an anonymous action is reported like any other
(Chapter 10). Here `main` hands a checking closure to `Runner.run`, which
calls it:

```okkama
module bible/closure_report v1.0.0;

enum Bad is Error = (too_big(value is int));

object Runner {
    Runner();
    run(step is (int) is int) is int { step(3); }
}

main() is int {
    let check = (value is int) is int {
        if (value > 2) { throw Bad.too_big(value); }
        value;
    };
    Runner().run(check);
}
```

```text
Bad: Bad.too_big(3)
  --> report.ok:12:26
   |
12 |         if (value > 2) { throw Bad.too_big(value); }
   |                          ^^^^^^^^^^^^^^^^^^^^^^^^^
Stack trace (most recent call first):
  at lambda#0 (report.ok:12:26)
  at Runner.run (report.ok:7:40)
  at main (report.ok:15:5)
```

The anonymous action has no name, so its frame carries the name of its
function in the IR, `lambda#0`, and the location points into its body. The
three engines print the same report since 0.4.2 (#270): the JVM maps its own
stack frames back to the IR functions through a table the compiler writes
into the jar. The frame does not say which action created the closure. In
the IR the closure and its creator are separate functions, and the creating
call may have returned long before the closure fails; the source location is
what identifies it.

## 18.7 Where closures came from

Closures are older than objects. Church's lambda calculus of 1936 already
treated functions as values [@church1936], and McCarthy built LISP on it in
1960 [@mccarthy1960]. LISP then met the question every language with function
values has to answer: when a function uses a name it did not declare, which
binding does it see, the one where it was created or the one where it is
called? Early LISPs looked the name up where the function was called, and a
function passed as an argument could see the wrong variable. This became
known as the *funarg problem*. Moses argued in 1970 that it should be called
the environment problem, because the cure is to give the function the
environment it was created in [@moses1970]. Landin had done exactly that in
1964: his SECD machine paired a function with that environment and called
the pair a closure [@landin1964]. Scheme made lexically scoped closures the
basis of a whole language in 1975 [@sussman1975].

Object-oriented languages took the idea in through blocks. Smalltalk-80
blocks are closures, and the library builds every control structure on
them, conditionals and loops included [@goldberg1983]. A `^` inside a block
returns from the method that created the block, which is what lets a block
act as the body of a loop. The mainstream came later: C# 2.0 added anonymous
methods in 2005, and Java 8 added lambdas in 2014, typed by functional
interfaces and limited to effectively final locals [@gosling2023].

Okkama follows Landin and Scheme in what a closure is, a function together
with what it saw, and departs from all of them in that what it saw is values
only. It also keeps apart the two jobs Smalltalk gave to blocks. The blocks
of `for` and `map` are control structures and are not values; anonymous
actions are values and are not control structures, so a `return` inside one
returns from that action, and `next` and `last` cannot reach an outer loop
(Section 18.1). Since Okkama locals are never assigned, the later form of the
funarg problem, which variable a closure shares with its creator, has no
room to arise.

## 18.8 Okkama among other languages

| | Captures | Assigning a captured name | Typing |
|---|---|---|---|
| Java | values of effectively final locals | rejected | functional interfaces, target-typed |
| Kotlin | variables (boxed `Ref` when written) | allowed, visible outside | function types, with variance |
| C# | variables (a shared display class) | allowed, visible outside | delegates, with variance |
| Go | variables (per-iteration loop variables since 1.22) | allowed, visible outside | function types, exact |
| Rust | by reference, by mutable reference, or by value (`move`) | through `FnMut`, checked by the borrow checker | `Fn`, `FnMut`, `FnOnce` traits |
| Haskell | values (everything is immutable) | not applicable | inferred function types |
| OCaml | values; `ref` cells are values too | through a `ref` cell | inferred function types |
| Okkama | values of immutable locals, and `self` | rejected (#321) | declared, exact, no variance |

Okkama sits with Haskell and OCaml on what is captured, a value and never a
variable, and with Java on the declaration being explicit. It is stricter
than all of them on types: an action value has exactly its declared
signature, and nothing converts it to a role or to a wider function type.

## 18.9 Limits

- **No recursion by name.** A `let` becomes visible only after its
  initializer, so an anonymous action cannot call itself through the `let`
  that holds it; the compiler reports an undefined action. The Book once
  promised otherwise, and 0.4.2 settled on the rule (#318). Recursion
  belongs to named actions, which get constant-stack self tail calls and the
  compiler's check for recursion that cannot end.
- **Equality by identity only**, and an action value is never a map key.
- **Positional calls only**, exactly the declared number of arguments; no
  variadic anonymous parameters, no defaults.
- **No currying or partial application.** `make_adder` has to be written
  out.
- **No async anonymous actions.** An action that awaits must be a named
  `async` action, passed as an object that does a role.
- **No `any`, no native ABI, no conversion to a role.**
- **One helper call per closure call on Tomato**, about ten
  nanoseconds.

## 18.10 What this design costs

Explicit signatures make short closures longer than in languages that infer
them: `(value is int) is int { value * 2; }` where Kotlin writes `{ it * 2 }`.
Capturing values instead of variables rules out the closure that counts its
own calls in a captured local; that state has to live in an object with
successors, or in a `Cell` (Chapter 13). Exact function types without
variance sometimes need a small wrapper action.

In return, a closure is predictable. What it captures is visible at the
place it is written, nothing it holds can change behind its back, and its
type is the line that declares it. A closure is a small immutable object
that the language writes for you, and that keeps closures inside Okkama's
object model instead of opening a second, functional one beside it.
