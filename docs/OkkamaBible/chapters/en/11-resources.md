# Chapter 11. defer, destroy, regions: deterministic resources

Memory is not the only thing a program borrows from the system. A file
descriptor, a socket, a database transaction, a lock, a temporary directory
and a child process all have to be given back, and given back at a
predictable moment: before the next request reuses the port, before the
transaction times out, before the disk fills with temporary files. Okkama
manages memory for the programmer, with a collector or reference counts
depending on the engine. It does not leave resources to the collector.

Two constructs give resources back:

- **`defer`** registers work to run when the current block ends.
- **`destroy`** declares, in an object, what releasing that object means. The
  scope that *owns* the object runs it when the scope ends.

Both run on every exit from a scope: normal completion, `return`, `last`,
`next`, a thrown error, a panic, and the cancellation of an async task. The
program below opens handles in a block, in a called action and in a loop,
and prints every open and close:

{{example:11-resources}}

The output reads like a contract.

- The block's `defer`s run first, newest first. Then its resources are
  destroyed, newest first: `b` before `a`.
- `alias` names the same object as `a`, and `a` is closed once.
- The action `kept` closes its temporary handle when it returns, and the
  handle it returns lives on in `main` until `main` ends.
- Each loop iteration closes its own handle before the next begins.

This chapter explains each of those rules: where they come from, how the
compiler and the engines implement them, and what they cost.

## 11.1 Memory and resources are different problems

Garbage collection solved memory well, because memory has a property other
resources lack: running out of it is the collector's signal to act. The
collector runs when memory is short, and that is exactly when memory needs
reclaiming. No such signal exists for file descriptors or database
connections. A program can run out of descriptors with plenty of free
memory, and the collector, which is not short of anything, does nothing.

*Finalizers*, code the collector runs when it reclaims an object, were the
first answer, and they failed. Hans Boehm showed that a finalizer runs at
an unpredictable moment, on an unpredictable thread, possibly after other
objects it needs have been finalized, and that making finalizers correct
requires synchronization most programmers never write [@boehm2003]. Java
deprecated `Object.finalize` and then marked finalization for removal
[@jep421]. Its replacement is explicit: `try`-with-resources, which closes
an `AutoCloseable` at the end of a block.

Okkama never ties a resource to the collector. When a `destroy` runs is a
property of the program text, the same on every engine and for every heap
size. The Sparrow core has a precise, non-moving collector, the IR engine
counts references, and the JVM has its own collector. All three run the
same `destroy`s in the same order.

## 11.2 The lineage

Deterministic cleanup has three ancestors, and Okkama takes from each.

- **Destructors.** C++ ties a resource to the lifetime of a scoped object:
  the constructor acquires it and the destructor releases it when the scope
  ends, whatever the exit. Stroustrup calls the idiom "resource
  acquisition is initialization" [@stroustrup1994]. Rust keeps the idea,
  as `Drop`, and adds ownership checked at compile time, so a value is
  dropped exactly once by its single owner [@klabnik2023].
- **Block-scoped release.** Python's `with` statement [@pep343], Java's
  `try`-with-resources and C#'s `using` release a resource at the end of a
  block. The call site must remember to use the construct; forgetting it
  compiles.
- **`defer`.** Go and Swift register a call that runs when the function
  (Go) or the block (Swift) exits. It handles work that is not about one
  object, such as an audit line, restoring a setting or removing a file.

Okkama's `destroy` is a destructor: the object says how it is released, and
callers do not have to remember. Its `defer` is Swift's: block-scoped, not
function-scoped. Ownership, the question "who runs the destructor", follows
object identity at run time, not a static borrow checker (§11.5).

## 11.3 defer

```okkama
defer log.line("defer 1, sees " + a.label());
```

A reached `defer` registers its expression and runs it exactly once when
the block that contains it ends.

- **The expression is evaluated at exit, not at registration.** The receiver
  and the arguments are read when the block ends. In the opening example
  the first `defer` sees `a`, which is still alive, because `defer`s run
  before the block's resources are destroyed.
- **Only reached `defer`s count.** A `defer` in a branch that did not run
  registers nothing.
- **Newest first.** The `defer`s of a block run in reverse order of
  registration, and then the block's owned resources are destroyed, also
  newest first.
- **The current block, not the function.** A `defer` inside a loop body runs
  at the end of that iteration, and one inside an inner block runs when the
  inner block ends. This is Swift's rule, not Go's.
- **Every `defer` is attempted.** If one fails, the remaining ones still run.
  An existing failure of the body has priority. The failure of a `defer`
  cancels a pending `return`, and otherwise the first `defer` failure wins.
- **A `defer` may await.** In an async action, `defer await audit.flush();`
  suspends the scope's exit until the work is done, cancellation included.
- **The discard rules still apply.** A `defer` cannot silently drop a value
  that must be used, such as the result of a successor action.

`defer` is for work that belongs to a block. Releasing a resource belongs
to the resource, and that is `destroy`.

## 11.4 destroy

```okkama
object Handle {
    has name is string;
    Handle(:name) { Stdout().writeln("open " + name); }
    destroy { Stdout().writeln("close " + name); }
}
```

An object has at most one `destroy` block. It is not an action: nothing can
call it. The engine runs it once, when the scope that owns the object ends.
It may read `self` and the fields, and call actions. It cannot assign fields
through a successor: the object is being released, not updated.

The order of destruction is fixed:

1. Inside one scope, objects are destroyed in reverse order of construction,
   so a resource opened later, which may depend on an earlier one, is
   released first.
2. An owner is destroyed before the objects held in its fields. Its
   `destroy` still sees intact fields, and the fields are destroyed after
   it, newest first. The session in §11.7 shows the order:
   `end session`, `close s2.out`, `close s2.in`.
3. Every identity enters destruction at most once. That holds even when its
   `destroy` fails, and even when the `destroy` makes an alias of `self`.

A `destroy` that fails does not stop the others. Every remaining resource
is still destroyed, and the failure is reported. If the scope was already
unwinding from a failure, the cleanup failure is attached to that failure as
a secondary (Chapter 10), not put in its place.

The standard library's resources combine `destroy` with an explicit
`close()`. A `File` or a `TlsConnection` does `Closing`, whose `close()` is
idempotent, and its `destroy` closes it too. Code that wants to release
early calls `close()`, and code that forgets gets the same release at scope
exit.

## 11.5 Who owns what

"The scope that owns the object" needs a definition, and Okkama's is about
identities, not names.

- **A binding owns a fresh object.** `let a = Handle(...)` makes the block
  of the `let` the owner of the new handle.
- **An alias borrows.** `let alias = a;` creates a second name for an object
  the scope already owns. No second owner appears, so there is no double
  destroy.
- **Parameters and receivers borrow.** An action that receives an existing
  object does not own it. A *fresh* argument nobody owns yet, such as
  `use(Handle(...))`, is adopted by the called action and destroyed when it
  returns, unless the action returns it.
- **A returned object moves to the caller.** When an action ends, the
  resources reachable from its result are released from its ownership and
  taken over by the caller. In the opening example, `kept` closes
  `report.tmp` and hands `report` to `main`.
- **Aggregates carry identities.** An array, a map, the fields of an object
  and an `any` box hold the identities they contain. A fresh resource put
  into a fresh array is owned by whoever owns the array, and repeated
  references never duplicate ownership.
- **Successors are independent.** A successor action returns a new object
  with a new identity and its own lifetime (Chapter 13).
- **Loop iterations are scopes.** What an iteration owns is destroyed when
  the iteration ends. A value the loop produces, through `last value` or the
  result of `map`, `reduce` and the other functional operations, moves to
  the enclosing scope.

The conformance suite pins these rules:

- `L07.ownership.alias-borrows`;
- `L07.ownership.returned-object-survives`;
- `L07.ownership.successor-independent`;
- the region cases added with #9 and #266, among them
  `L07.ownership.reduce-builder`, whose resources are gathered by `reduce`.

## 11.6 Resources in collections and closures

Containers do not own; they carry. A map of handles, an array of
connections and a closure that captured a socket all hold identities, and
whoever owns the container owns what it holds.

{{example:11-containers}}

The map built in `named` leaves the action as its result, so `main`'s inner
block owns both handles and closes them when it ends, newest first. The
closure returned by `greeter` captured `handle`. The handle therefore moves
with the closure, the closure keeps it usable, and it closes when the block
that holds the closure ends, not when `greeter` returns.

Two consequences are worth stating.

- **Removing an element does not release it.** Maps and arrays are values:
  removing a key with `-` builds a new map, while the old map, still owned,
  keeps the handle. A resource is released when its owner's scope
  ends, not when the last collection that mentions it is computed.
- **A closure is as much an owner's tool as a field.** A callback that keeps
  a connection alive keeps it alive exactly as long as the callback is
  reachable from an owning scope. That is usually what the author meant, and
  when it is not, the remedy is to pass the connection as an argument
  instead of capturing it.

## 11.7 Cleanup on the failure path

Cleanup does not depend on how a scope ends. A thrown error unwinds through
the same `defer`s and `destroy`s as a normal exit, and so do a panic and a
runtime error. A `try` closes its body's scope *before* it chooses a
`catch` clause, so a clause never runs while the failed body still holds
resources.

{{example:11-unwind}}

Three rules are visible.

- In the first `try`, `defer in try` and `close first` print before
  `caught body`. The catch clause runs after the body's cleanup.
- `Session("s1", false)` opens `s1.in` and then fails its `confirm`. The
  handle it opened is closed. The session itself was never finished, so its
  `destroy` does not run: an object that did not finish construction is
  never released as if it had.
- A finished session is destroyed owner first: `end session`, then its
  fields, newest first.

The thrown error's own payload survives the cleanup: it moves into the
matching `catch` clause. It cannot hold a resource in any case
(Chapter 10), so the move releases nothing early.

## 11.8 async destroy

Some resources cannot be released instantly. A connection must flush what
it buffered, and a server must let its requests drain. Their `destroy` has
to wait, and Okkama lets it: `async destroy` (#154).

{{example:11-async-destroy}}

- **It is awaited where the owner's scope ends**, in the same reverse
  order as other `destroy`s, on every exit, a failure included. `talk`
  delivers its result only after the connection has flushed and closed.
- **A cancellation does not interrupt it.** A task cancelled while an async
  destroy runs gets `Cancelled` at its next `await`, after the scope has
  closed. Cleanup that can be interrupted halfway is worse than no cleanup.
- **Only async code may own such an object.** A synchronous scope has
  nothing to suspend, so owning one there is a compile error:

{{example:11-async-owner-rejected}}

The rule is transitive. An object that holds an async-destroy resource in a
field, an array or a map has to be owned in async code too. Borrowing one,
by passing it to a synchronous action that does not keep it, is fine.

An async destroy replaces a pattern that needed two `defer`s, a `close`
and an awaited `settled()`. The standard library still has helper objects
that exist only to await in cleanup. Moving them onto async destroy is
[#264](https://github.com/shootnix/okkama/issues/264).

## 11.9 Resources and tasks

A task is a scope too. What an async action makes and keeps is released
when the action's task ends. What it returns moves to the scope that awaits
it, exactly as a synchronous result moves to its caller:

{{example:11-tasks}}

`w1.scratch` closes before `counted` is printed: the task that made it has
ended. `w2.tmp` closes inside `opened`, and `w2` lives in the block that
awaited it, until that block ends.

A task that is never awaited still releases what it owns when it finishes,
and a cancelled task unwinds through its scopes like a failure, async
destroys included (Chapter 20). A resource cannot outlive its owner
unnoticed by escaping into a forgotten future.

## 11.10 Close early, destroy anyway

Scope exit is the latest moment a resource is released, not the only one.
The resources of the standard library follow one pattern:

- an idempotent `close()`, part of the `Closing` role;
- `closed()`, to ask whether it already happened;
- a `destroy` that calls `close()`.

Code that knows it is done releases early. Code that forgets, or fails
halfway, still gets the release when the scope ends.

{{example:11-closing}}

`used` closes its temporary directory twice. The second call does nothing,
and the directory is gone before the action returns. `forgotten` never
calls `close()`: the directory is removed when `forgotten` ends, because
the action owned it and returned only its path, a plain value.

The pattern costs one rule for authors of resource objects: `close()` must
be idempotent, and every other action of a closed resource should fail with
a catchable `already_closed`, not misbehave.

## 11.11 Regions: how ownership runs

The compiler implements ownership with **regions**: run-time sets of
identities, one per scope that may own a resource. It decides at compile
time which scopes need one, and that decision is most of the cost model.

A type is **plain** when none of its values can reach an object with a
`destroy`. Most types are plain: numbers, text, records of plain fields,
arrays and maps of plain values. Plain values never touch a region. Here
are two actions that differ only in the type they create:

```text
func @Use.plain(%0: Use, %1: int) -> int [method Use] {
^b0:
    br ^b1
^b1:
    br ^b2
^b2:
    %2: Point = call @Point.new#0(%1)
    %3: int = call @Point.value(%2)
    %4: int = const 1
    %5: int = int.add %3, %4
    br ^b3
^b3:
    ret %5
}
func @Use.owned(%0: Use, %1: int) -> int [method Use] {
^b0:
    br ^b1
^b1:
    %2: region = region.enter
    br ^b2
^b2 unwind ^b5:
    %3: Handle = call @Handle.new#0(%1)
    region.track %2, %3
    region.adopt %2, %3
    %4: int = call @Handle.value(%3)
    %5: int = const 1
    %6: int = int.add %4, %5
    br ^b3
^b3:
    region.leave %2
    br ^b4
^b4:
    ret %6
^b5(%7: failure):
    %8: failure = region.unwind %2, %7
    raise %8
}
```

`Point` has no `destroy`, so `plain` has no region at all. `Handle` has
one, so `owned` gets the full set. The instructions are a small vocabulary.

| Instruction | Meaning |
|---|---|
| `region.enter` | a new, empty region for this scope |
| `region.track r, v` | remember the free identities `v` reaches; do not own them yet (a call result, a temporary) |
| `region.adopt r, v` | own the free identities `v` reaches (`let`, a fresh argument) |
| `region.transfer from, to, v` | move what `from` owns of `v` to `to` (a value leaving a nested scope) |
| `region.leave r [, keep v]` | close: run destroys, newest first; what `keep` reaches becomes free again (a result) |
| `region.unwind r, f` | close on the failure path; cleanup failures join `f` as secondaries |
| `region.settle`, `region.settle_unwind` | the same in async functions, awaiting async destroys (#154) |

An optimization pass removes regions that provably stay empty. The
compiler's plainness is per declaration. One role implemented by one object
with a `destroy` makes every value of that role non-plain, even where no
such object is ever built ([#268](https://github.com/shootnix/okkama/issues/268)
would refine that).

Regions are named after, but are not, the regions of Tofte and Talpin's
region-based memory management [@tofte1997]. Those hold memory and free it
all at once. Okkama's regions hold *identities to release*. Memory stays
with the collector, and a region only decides when each `destroy` runs.

## 11.12 The cost of a walk

To track or adopt a value, the engine walks what the value reaches. It
looks for identities that are still free, owned by nobody. The walk stops
at objects owned by another scope, at plain values and at objects without
an identity. Until 0.4.1 it did not stop at arrays, maps and closures,
which have no owner. A fresh value pointing at a large owned structure cost
a walk over the whole structure, on every track and every close. A loop
building such values was quadratic.

Since #9 the engines remember containers whose walk met no free identity.
Containers are immutable, so what one reaches is at most as old as the
container. A remembered container is skipped until an identity at least as
old becomes free again, which happens when a scope returns it. Then all
the summaries are dropped. On the benchmark built for this
(`11-ownership`: 100 000 records in an array and a map, 300 rounds), the
change made the Sparrow core 85% faster, the IR engine 86% and the JVM 75%.

The lesson for program design is modest. Ownership costs nothing for plain
data. Non-plain data costs a walk the first time a structure is seen and
almost nothing after that. A program that keeps a million records reaching
a socket pays once, not once per request.

## 11.13 What is not promised

Deterministic cleanup has limits, and they are worth knowing.

- **A scope that never ends never cleans up.** A `loop` without an exit
  keeps its resources until the process ends.
- **A killed process runs nothing.** `kill -9`, a power failure or an
  out-of-memory kill from the operating system skip every `destroy`.
  Durable state needs a transaction or a write-ahead design, not a
  destructor.
- **A budget stop runs no `destroy`.** When `--timeout` or another budget
  ends the run, cleanup does not run, since it would spend the budget again
  (Chapter 10). The process exits, and the operating system closes what
  the process held.
- **A `destroy` cannot be retried.** It runs at most once. If it fails, the
  failure is reported and the identity is considered released.

## 11.14 What this design costs

- **Ownership is checked at run time, not at compile time.** Rust rejects a
  program that would release a value twice or use it after release. Okkama
  prevents the double release by construction (at most once per identity),
  but a program can still use a closed resource. The `Closing` objects of
  the standard library answer that with a catchable `already_closed`
  failure, not with undefined behaviour.
- **Non-plain values cost a walk.** Small since #9, but not zero, and
  per-declaration plainness makes more values non-plain than strictly need
  to be (#268).
- **Async destroy constrains the owner.** A synchronous library cannot own
  a resource whose release must wait. It has to borrow one or become
  async.
- **Cleanup order is fixed.** Newest first, owner before fields. A program
  that needs another order must express it with `defer`, or by nesting
  scopes.

Against that stands what the design removes:

- `finally` blocks;
- forgotten `close()` calls;
- finalizers and their surprises;
- resource leaks that show up only under load.

All of it comes from two constructs and one rule: the scope that owns a
resource releases it.
