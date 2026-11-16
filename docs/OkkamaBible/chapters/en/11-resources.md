# Chapter 11. defer and destroy: deterministic resources

Memory is not the only thing a program borrows. Files, sockets, transactions, locks, temporary directories and child processes need timely release. Okkama manages memory automatically and guarantees resource release at scope exit.

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

This chapter explains those rules, their origins and their costs.

## 11.1 Memory and resources are different problems

Automatic memory management alone does not promise timely resource release. A program can exhaust file descriptors or database connections while still having plenty of memory; these resources need an explicit lifetime tied to scopes.

*Finalizers*, code the collector runs when it reclaims an object, were the
first answer, and they failed. Hans Boehm showed that a finalizer runs at
an unpredictable moment, on an unpredictable thread, possibly after other
objects it needs have been finalized, and that making finalizers correct
requires synchronization most programmers never write [@boehm2003]. Java
deprecated `Object.finalize` and then marked finalization for removal
[@jep421]. Its replacement is explicit: `try`-with-resources, which closes
an `AutoCloseable` at the end of a block.

When a `destroy` runs is determined by the program’s scopes and ownership, independently of memory pressure.

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
call it. It runs once, when the scope that owns the object ends.
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
- **A cap is its owner.** A cap (Section 12.8) carries the object it was
  taken from. Returned from an action, it moves the object and its
  resources to the caller; taken from an object a scope owns, it borrows.
- **Successors are independent.** A successor action returns a new object
  with a new identity and its own lifetime (Chapter 13).
- **Loop iterations are scopes.** What an iteration owns is destroyed when
  the iteration ends. A value the loop produces, through `last value` or the
  result of `map`, `reduce` and the other functional operations, moves to
  the enclosing scope.
- **Branches are scopes.** An `if`, `match`, `try` or `catch` branch destroys
  what it owns when it ends, its last `let` included, in reverse order. Only
  a value the branch produces and the program uses moves out:
  `let log = if verbose { File.create(a); } else { File.create(b); };`
  (#308; before 0.4.2 the last `let` of a branch lived until the enclosing
  scope ended).

The conformance suite pins these rules:

The conformance suite checks alias borrowing, returned resources, independent successors, branch scopes and resources gathered by `reduce`.

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

An async destroy replaces a pattern that needed two `defer`s, a `close` and
an awaited `settled()`. In 0.4.2 the standard library dropped 33 helper
objects that existed only to await one child in cleanup, because a
structured `await` already waits for it
([#264](https://github.com/shootnix/okkama/issues/264)). Its remaining
cleanup guards still close through `defer await`; moving them onto
`async destroy` is [#329](https://github.com/shootnix/okkama/issues/329).

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

## 11.11 Replacing a resource: rebinding keeps, folding releases

A log that rotates its file is a resource that is replaced over and over.
In Okkama the replacement is a successor, `rotated()`, which builds a new
`Log` around a new `Segment`. Where the old `Log` goes then depends on how
the program holds it, and the two usual ways give different lifetimes.

{{example:11-rotation}}

- **Rebinding keeps every old value.** In `rebound`, `log = log.rotated()`
  gives the name a new value, but it does not destroy the old one early: the
  old `Log` belongs to the scope that declared `log`, and its `destroy` runs
  when that scope ends (Section 13.7). All four segments stay open until
  `done rebound`, and then close newest first. For three rotations this is
  harmless; for a server that rotates every hour and runs for a month, it is
  a file per hour held open until the action returns.
- **A fold releases each value.** In `folded`, `reduce` passes each `Log`
  to the next step and keeps only what the step returns. Each step is an
  iteration, and an iteration's scope ends with it (Section 11.5), so the
  old segment closes as soon as its successor exists: `open log.1`, then
  `close log.0`. Only the last `Log` leaves the loop.

The rule behind both is the one this chapter has used throughout: a
resource lives as long as the scope that owns it, not as long as a name
refers to it. A long-running program that replaces a resource should
therefore replace it inside a scope that ends: a fold, a loop iteration
that produces the next value, or a state owner whose request handler is
such a scope (Section 20.6). Rebinding a name in a scope that lives for the
whole program is the one shape to avoid.

## 11.12 A worked example: a connection pool

A pool is the textbook case where resources outlive the code that uses them.
In Okkama a pool is an ordinary object whose field holds its connections;
lending one out is a borrow, and nothing else is needed.

{{example:11-pool}}

- **The pool owns, through its field.** The constructor builds an array of
  fresh connections with `map`, and the array becomes the pool's field, so
  the pool's owner owns every connection in it (§11.5).
- **Lending borrows.** `lend` returns a connection that the pool still owns.
  The worker queries it and never closes it: a borrower cannot destroy what
  it does not own, and no "return to pool" call exists to be forgotten.
- **One end, in a fixed order.** When the block that holds the pool ends, the
  pool's own `destroy` runs first and sees intact connections, then the
  connections close, newest first: `drain pool of 2`, `close 2`, `close 1`.
  The worker's three jobs used the connections in turn and released nothing.

What the example does not do is as telling. There is no reference counting
on the connections and no `release` in a `finally`. The worker holds the pool
it borrowed and lives inside the same block, so every connection it used is
still owned when it uses it, and released once, by the pool's owner. A pool
that must hand a connection to code that outlives it moves the connection
out instead, and that is a different, visible operation.

## 11.13 A worked example: commit or roll back

A transaction is the classic resource whose release depends on how the work
ended: if the work finished, commit; if anything went wrong, roll back. In
Okkama the rollback is the transaction's `destroy`, and the commit is an
explicit call.

{{example:11-transaction}}

- **The rollback cannot be skipped.** Whatever path leaves `order` — the
  `confirm` that refuses a non-positive amount, a failure further down, a
  bug — the transaction's scope ends and its `destroy` runs (§11.7). The
  second order shows it: `begin`, then `rollback`, then the caller's
  `catch`.
- **The commit is the one decision.** Without `tx.commit()` the transaction
  rolls back, so forgetting the commit loses the work instead of half-doing
  it. The commit is the last step of `order` that can change the outcome.
- **What `destroy` must see is a field it can read.** The flag is an
  `Atomic<bool>`, declared in the object, where a reader sees it. A
  successor would be the wrong tool here: `committed()` returning `^Transaction`
  would make a new identity with its own lifetime (§11.5), and the original,
  never committed, would still be destroyed and roll back.

Other languages put the same decision in different places. Python's
`sqlite3` connection, used as a context manager, commits or rolls back when
the `with` block ends [@pysqlite3]; D's `scope(failure)` runs a block only
when the scope is left by an exception [@dscope]. Both depend on the caller
writing the construct at every use. In Okkama the transaction carries the
rule itself: there is no way to hold one that its scope will not settle.

## 11.14 When cleanup itself fails

Releasing can fail as well. A journal that flushes its buffer when it is
released may find the disk full, and a connection may fail to send its
goodbye. Section 11.4 stated the rule in one sentence; this example shows
what it means:

{{example:11-cleanup-fails}}

- **Every `destroy` runs.** Two of the three journals fail to flush, and
  `audit` and `orders` are still flushed after `metrics` fails. A failed
  release does not stop the others, so one full disk cannot leak a file
  handle.
- **On a normal exit, the first cleanup failure becomes the failure.**
  `run` would have returned "3 rows". It throws `Unflushed.full("metrics")`
  instead: `metrics` was released first, newest first, so its failure came
  first. The other failure, from `orders`, is attached to it as a
  secondary.
- **On a failing exit, the original failure stays.** With no rows, `run`
  throws `Rejected.empty`, and that is what the caller catches. Both flush
  failures happened while unwinding from it, and they ride along as
  secondaries instead of replacing it. A failure in cleanup cannot hide the
  failure that caused the cleanup.
- **Secondaries are for the reader, not the program.** A `catch` receives
  only the primary failure. A program cannot list secondaries and decide by
  them. The report of an uncaught failure prints them:

```text
Rejected: Rejected.empty
Secondary cleanup failure:
  Unflushed: Unflushed.full("metrics")
Secondary cleanup failure:
  Unflushed: Unflushed.full("orders")
  --> cleanup.ok:33:9
```

The question is old, and languages have answered it in four ways.

- **C++ forbids the failure.** A destructor that throws while the stack
  unwinds from another exception ends the program with `std::terminate`.
  Since C++11, destructors are `noexcept` unless they say otherwise.
  Stroustrup's guarantees of exception safety assume destructors that do
  not throw [@stroustrup2001]. A release that can fail must therefore
  report it some other way, or not at all.
- **Python keeps both by chaining.** An exception raised while another is
  being handled keeps the first as its `__context__`, and the traceback
  shows both [@pep3134]. But the new exception is the one that propagates.
  A failing `__exit__` or `finally` replaces the original as the thing the
  caller catches.
- **Go leaves it to the programmer.** `defer f.Close()` discards the error
  unless the code captures it. Since Go 1.20, `errors.Join` combines
  several errors into one value, so the deferred close and the original
  error can travel together, if the programmer writes it [@go120].
- **Java suppresses.** `try`-with-resources closes resources in reverse
  order, and a close does not stop the others. An exception from the body
  propagates, and close failures are added to its suppressed list. When
  only closes fail, the failure of the resource closed first propagates,
  and the rest are suppressed on it (JLS §14.20.3 [@gosling2023]).

Okkama's rule is Java's, with two differences. It applies to every owned
resource at every scope end, not only inside a `try` that names the
resource. And the suppressed list is not a value a program can read. Java's
`getSuppressed()` lets a handler branch on a cleanup failure that was not
the reason the code failed. Okkama keeps the handler on the reason, and it
leaves the secondaries to the person reading the report.

## 11.15 What is not promised

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

## 11.16 What this design costs

- **Ownership is checked at run time, not at compile time.** Rust rejects a
  program that would release a value twice or use it after release. Okkama
  prevents the double release by construction (at most once per identity),
  but a program can still use a closed resource. The `Closing` objects of
  the standard library answer that with a catchable `already_closed`
  failure, not with undefined behaviour.
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
