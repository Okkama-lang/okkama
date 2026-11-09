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
and not variables, how the engines run them, and how they compare with
lambdas in other languages.

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

The Book also says that rebinding a captured name *inside* the action is an
error. The compiler does not enforce it yet: such a rebind changes a copy
that lives for one call (#321).

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

## 18.4 Closures or objects

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

## 18.5 How closures run

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
- **The Sparrow core** allocates a heap object laid out like the function's
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
| Sparrow core | 0.31 s | 0.41 s |
| IR engine (1 million calls) | 0.97 s | 0.93 s |
| JVM | 0.59 s | 0.49 s |

On the core the helper costs about ten nanoseconds per call over a direct
call. On the JVM the closure is slightly faster: the class of each anonymous
action has one implementation of `call`, which the JIT inlines.

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
at the same speed with or without the import (IR 1.02 s, core 0.10 s, JVM
0.22 s).

## 18.6 Okkama among other languages

| | Captures | Assigning a captured name | Typing |
|---|---|---|---|
| Java | values of effectively final locals | rejected | functional interfaces, target-typed |
| Kotlin | variables (boxed `Ref` when written) | allowed, visible outside | function types, with variance |
| C# | variables (a shared display class) | allowed, visible outside | delegates, with variance |
| Go | variables (per-iteration loop variables since 1.22) | allowed, visible outside | function types, exact |
| Rust | by reference, by mutable reference, or by value (`move`) | through `FnMut`, checked by the borrow checker | `Fn`, `FnMut`, `FnOnce` traits |
| Haskell | values (everything is immutable) | not applicable | inferred function types |
| OCaml | values; `ref` cells are values too | through a `ref` cell | inferred function types |
| Okkama | values of immutable locals, and `self` | not intended (#321) | declared, exact, no variance |

Okkama sits with Haskell and OCaml on what is captured, a value and never a
variable, and with Java on the declaration being explicit. It is stricter
than all of them on types: an action value has exactly its declared
signature, and nothing converts it to a role or to a wider function type.

## 18.7 Limits

- **No recursion by name.** The Book says a `let` bound to an anonymous
  action can call itself by that name; the compiler rejects it as an
  undefined action (#318). Recursion belongs to named actions, which get
  constant-stack self tail calls and the compiler's check for recursion that
  cannot end.
- **Rebinding a captured name inside the action is not rejected** (#321).
- **Positional calls only**, exactly the declared number of arguments; no
  variadic anonymous parameters, no defaults.
- **No currying or partial application.** `make_adder` has to be written
  out.
- **No async anonymous actions.** An action that awaits must be a named
  `async` action.
- **No `any`, no native ABI, no conversion to a role.**
- **One helper call per closure call on the Sparrow core**, about ten
  nanoseconds.

## 18.8 What this design costs

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
