# Chapter 18 — archived implementation passages

Replaced mixed paragraphs are preserved in full for context.

## Chapter 18. Anonymous actions and closures

described 0.5.x

This chapter covers how anonymous actions are written and typed, what a
closure remembers and what it does not, why Okkama's closures are values
and not variables, how they compare for equality, how they serve as data,
how the engines run them, where the idea came from, and how they compare
with lambdas in other languages.

## 18.1 Writing an anonymous action

described 0.5.x

The arrows in `values.map() -> value { … }` and `for values -> value { … }`
belong to other constructs. Those blocks are not values; they cannot be
stored or returned, and they run inside the expression that holds them. The
compiler lowers them to loops in the enclosing function. An anonymous action,
by contrast, is a value with its own frame. That difference shows in control
flow: `next` and `last` in a `for` block continue or leave the loop, but
inside an anonymous action they have no loop to act on, even when the action
is created inside one:

## 18.3 Types are exact

described 0.5.x

Futures follow the same rule, for the same reason: there is no useful
structural equality to offer. Whether two functions give the same result for
every argument cannot be decided in general, by Rice's theorem [@rice1953].
Comparing their code and captured values instead would make equality depend
on how an engine compiles an action, which Okkama does not let into a
program's behaviour. Identity is cheap, predictable and the same on every
engine. An action value is not a map key either: keys are scalars and value
objects (Chapter 8).

## 18.6 How closures run

described 0.5.x

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
- **Sparrow** allocates a heap object laid out like the function's
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
| Sparrow | 0.31 s | 0.41 s |
| IR engine (1 million calls) | 0.97 s | 0.93 s |
| JVM | 0.59 s | 0.49 s |

On Sparrow the helper costs about ten nanoseconds per call over a direct
call. On the JVM the closure is slightly faster: the class of each anonymous
action has one implementation of `call`, which the JIT inlines.

The micro-benchmarks of 0.4.2 (#244) isolate the creation. Net of an empty
loop, median of five runs on the same machine, creating a closure that
captures a loop variable and calling it once costs 17 ns on Sparrow, 155 ns on
the IR engine and 6 ns on the JVM. Constructing an object with two fields
costs 4, 271 and 9 ns. On the JVM a closure costs no more than a small
object; on Sparrow it costs four times as much, because creating and calling
it both go through the runtime helper, while an object is allocated inline
in the compiled code.

## 18.6 How closures run

described 0.5.x

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
at the same speed with or without the import (IR 1.02 s, Sparrow 0.10 s, JVM
0.22 s).

## 18.6 How closures run

described 0.5.x

The anonymous action has no name, so its frame carries the name of its
function in the IR, `lambda#0`, and the location points into its body. The
three engines print the same report since 0.4.2 (#270): the JVM maps its own
stack frames back to the IR functions through a table the compiler writes
into the jar. The frame does not say which action created the closure. In
the IR the closure and its creator are separate functions, and the creating
call may have returned long before the closure fails; the source location is
what identifies it.

## 18.9 Limits

described 0.5.x

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
- **One helper call per closure call on Sparrow**, about ten
  nanoseconds.
