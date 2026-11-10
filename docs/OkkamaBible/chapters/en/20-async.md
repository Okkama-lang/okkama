# Chapter 20. Async, futures and structured concurrency

A program waits. It waits for a file, a socket, a database, a timer, another
part of itself. While one part waits, another could work, and every language
of the last twenty years has had to say how. Okkama's answer has four parts. An
`async` action returns a `Future<T>` instead of a `T`. `await` waits for a
future inside another async action, and nowhere else. `spawn` starts work that
runs beside the code that started it. And one cooperative scheduler runs one
task at a time, so the code between two `await`s of a task is never
interleaved with anybody else's.

Here is the smallest async program worth reading:

{{example:20-lazy}}

- `Oven("rye").baked()` does not bake. The call returns a future, and the
  first line of output shows that nothing has run yet: `done: false`, and no
  `baking rye`.
- The first `await pending` starts the body, which prints `baking rye` and
  produces `"rye bread"`.
- The second `await pending` does not bake again. A future runs its body at
  most once and keeps the outcome; every later `await` reads it.
- `async main()` is what makes `await` legal in `main`. A program whose `main`
  is synchronous can create futures but cannot wait for them.

## 20.1 A future is a value that is not computed yet

An action declared `async` and `is T` is checked as producing a `T`. Calling
it produces a `Future<T>`:

```text
Oven("rye").baked()          : Future<string>
await Oven("rye").baked()    : string
```

A future has exactly three public actions, and none of them reads the value:

- `done()` is `true` once the future has settled: with a value, with a
  failure, or cancelled with its cleanup finished;
- `cancel()` asks the future to stop (Section 20.5);
- `cancelled()` is `true` once cancellation has finished.

The only way to get the value is `await`. There is no `result()`, no
`value()`, no `get()` that blocks. That is deliberate: a call that blocks the
thread until a value is there would stop the one scheduler, and with it every
other task of the program.

The rules around `await` are few:

- There is no implicit await. A `Future<int>` is not an `int`, and passing one
  where the other is expected is a type error.
- `await` is legal only inside an `async` action, `async main` included.
  Awaiting in a synchronous action is a compile error, and so is awaiting
  something that is not a future.
- A failure thrown by the body is kept like a value and thrown again at every
  `await`, where an ordinary `try`/`catch` takes it.
- Locals survive suspension. The body resumes at the operand it was waiting
  for, without evaluating earlier operands or statements again.
- `await` is a unary operator: `(await reply.item()).string()` needs the
  parentheses, and `await load() + 1` means `(await load()) + 1`.
- An `async` action declared `is void` produces a future without a value. It
  is awaited for its effect, as a statement, never as an expression.

The call is lazy. That choice decides more than it seems to. A lazy future
that nobody awaits costs one object and runs no code, so returning one from an
action is free, and an action can build several and decide later which to
await. A future that starts when it is created would already be running by
then, and the caller would own a task it may not want. `spawn` is the explicit
way to start one now (Section 20.4).

## 20.2 Where futures came from

The future is older than most languages that have one. Baker and Hewitt
named it in 1977, in work on actor systems, as a value that stands for a
computation still running; their paper is about collecting the futures that
nobody will ever read [@baker1977]. Halstead's Multilisp made it a
language construct in 1985: `(future e)` starts evaluating `e` in parallel and
returns a placeholder at once, and any strict use of the placeholder waits
for the value [@halstead1985]. That future is eager and implicit: a program
does not say where it waits.

Liskov and Shrira's promises (1988) moved the idea to distributed calls and
made it typed: a call returns a promise, the caller continues, and claiming
the promise later yields the result or the exception the call raised
[@liskov1988]. The two halves Okkama keeps are already there: the outcome
includes failures, and it is collected explicitly.

The `async`/`await` pair came from F#. Its asynchronous workflows (2007,
described in [@syme2011]) let a programmer write a sequential-looking body
that the compiler cuts at each `let!`, and an F# `Async<T>` is a *recipe*: it
runs only when started, and it can be started more than once. C# 5 (2012)
took the syntax with a different value: its `Task<T>` is already running when
the call returns. JavaScript's promises (2015) and `async` functions (2017)
are eager in the same way. Rust's futures (2019) went back to lazy values that
do nothing until polled.

Okkama sits between them. Its future is lazy like F#'s and Rust's: a call
starts nothing. But it runs at most once and keeps its outcome, like C#'s and
JavaScript's: awaiting it twice does not run it twice. A future in Okkama is a
single computation that has not started yet, not a recipe for many.

The price of `async`/`await` in every language that has it is what Nystrom
called the colour of a function [@nystrom2015]: an async function can be
called from anywhere, but its result can be awaited only from another async
function, so asynchrony spreads up the call graph. Okkama pays it in full, and
on purpose. An action that can suspend says so in its signature, because a
suspension point is where other tasks run (Section 20.3), and a reader who
cannot see where that happens cannot reason about the code.

## 20.3 One scheduler, one task at a time

A run has one scheduler. `async main` drives it until the root future
settles; a synchronous `main` does not drive it at all. Async does not create
threads, and it does not make CPU-bound work faster: two tasks that compute
take turns on one core. What async gives is that a task waiting for
something does not hold up the others.

Scheduling is cooperative. One task runs at a time, until it suspends at an
`await` of a future that has not settled, or until it ends. Then the
scheduler runs the next ready task. A task becomes ready when something it
waits for happens: a future it awaits completes, an `Event` it waits on is
set, a channel hands it a value, a permit it asked for is returned, or it is
cancelled. Ready tasks run in the order they were woken.

The rule that matters most is what waking does *not* do. It does not run the
woken task. The task that called `set()` or `cancel()` keeps running until it
suspends, and only then does the woken task get its turn:

{{example:20-wake}}

`main: after set` comes before `waiter: woke`, on every engine. So the code
of a task between two suspension points is never interleaved with another
task's code. A task can read some state, decide, and write the result
without a lock, as long as it does not `await` in between. Every `await` is a
point where the world may have changed; nothing else is.

This is the guarantee asyncio, JavaScript, Trio and Kotlin's coroutines give,
and it is what makes cooperative concurrency easier to reason about than
threads. Until 0.5.0 Okkama did not keep it everywhere. The IR engine and the
JVM resumed a woken task inside the call that woke it, so the program above
printed `waiter: woke` between the two lines of `main`; only Sparrow waited
for `main` to suspend. The three engines now agree (issue #340), and the
conformance suite checks nine orderings of this kind (`L08.wake.*`).

Two kinds of start stay immediate. `spawn` runs the new task at once, until
its first suspension, before the statement after `spawn`. The first `await` of
a cold future runs its body at once too; the awaiting task is suspended
anyway. And awaiting a future that has already settled does not suspend at
all: it returns the kept outcome, and no other task runs in between.

None of this is a thread-safety guarantee. Two tasks that each read, `await`
and then write can still lose an update, because the `await` between the read
and the write lets the other task in. Cooperative scheduling tells a reader
where interleaving can happen; it does not make it impossible.

## 20.4 `spawn`: work that runs beside its starter

`spawn` takes a call of an object's action and starts it now. Its result is
the same `Future<T>` the call would have returned, so `await` collects it
later; there is no second type for a running task, and no
`Future<Future<T>>`. What `spawn` starts depends on the action:

- **An async action** becomes a task of the current run. It shares the
  scheduler and the heap with its starter, so it may hold objects, futures,
  channel endpoints and other values that only make sense inside one run.
  It gives concurrency, not parallelism: it runs when the others wait.
- **A synchronous action** runs in a separate worker process, in parallel
  with the program. Receiver and arguments are evaluated in the parent, left
  to right, and copied to the worker; only transferable values cross: `int`,
  `float`, `string`, `char`, `byte`, `bool`, and arrays and maps of them.
  Objects, futures, closures and native handles cannot. The worker's result
  or failure comes back into the future.

```okkama
async main() is int {
    let left is Future<int> = spawn Worker(seed: 7).calculate(input: 20);
    let right is Future<int> = spawn Worker(seed: 9).calculate(input: 6);
    (await left) + (await right);
}
```

Both workers compute at the same time, before either future is awaited. The
form is narrow on purpose: only `spawn Receiver(…).action(…)` or a bound
action value is a spawn target, not a block or an arbitrary expression, so
what crosses the process boundary is always a receiver and arguments that the
compiler has checked.

(This chapter's examples do not start worker processes; they run in a sandbox
that forbids it. The program above is the specification's.)

A spawned task must be accounted for. When `async main` returns, the
scheduler cancels every spawned task that has not finished and waits for its
cleanup. A spawned task that failed and that nobody awaited makes the whole
run fail, with that task's failure: an error is not lost because nobody was
listening.

## 20.5 Structured await and cancellation

`cancel()` asks a future to stop. Cancellation is cooperative: it is
delivered at the task's suspension point, unwinds the task like a failure
(its `defer`s and `destroy`s run), and the task's `await`s then throw the
built-in `Cancelled`. A future that never started is simply marked cancelled;
its body never runs.

What happens to the future a cancelled task was waiting for? Okkama's answer,
since 0.4.2, is that `await` is *structured* (issue #147). Cancelling a task
cancels the future it awaits, if that future is its own: one its `await`
started, a task it spawned, or a request to the runtime. And the cancelled
task does not finish until that future has settled, cleanup included. Only
then does its own `await` throw `Cancelled` and its own cleanup run:

{{example:20-cancel}}

- `order.cancel()` returns at once. It does not run the delivery's cleanup;
  that happens when `main` suspends at `await order`.
- The delivery is cancelled because the order was waiting for it, and only
  the order. Its `defer` runs first: the van returns before the order closes.
- `await order` throws `Cancelled` only after both have finished, and
  `cancelled()` is then `true`.

A future that another task also awaits is not cancelled with one of its
waiters: the cancelled task only stops waiting for it, and the others still
get its outcome.

The rule makes a common case need nothing at all. A task that awaits one
child needs no group, no supervisor and no "settle" object in a `defer`:
cancelling the task reaches the child and waits for it. Groups exist for real
fan-out, where one task owns several children at once (Section 20.7).

This is structured concurrency in Smith's sense [@smith2018]: no child
outlives the scope that is waiting for it. Smith's Trio makes the scope
explicit, as a *nursery* block that every task must be started in. Kotlin
(2018) made every coroutine a child of a scope, so that a scope does not
finish before its children and cancelling it cancels them
[@elizarov2018]. Okkama gets the same property from the await itself, without
a scope object: the thing a task is waiting for is its child for as long as it
waits. What it does not get is the nursery's guarantee for work that nobody
awaits. A spawned task that its starter forgets runs on until `async main`
returns and the scheduler cancels it.

### Where structured concurrency came from

Smith's argument starts from Dijkstra's letter about `goto` [@dijkstra1968].
A `goto` lets control leave a block and never come back, so a reader cannot
treat the block as a unit: whatever it was supposed to finish may still be
going on elsewhere. Structured programming replaced it with constructs that
have one entry and one exit. A function that starts a thread or a goroutine
and returns does what `goto` did, Smith says: the work it started outlives the
call, and no caller can know when it ends or where its errors go. A nursery
puts concurrency back into the shape of a block.

The idea is older than the name. Erlang [@armstrong2003] linked processes so
that a crash travels along the links, and supervisors restart what they
started, so that a tree of processes fails and recovers as one. That tree is
built and torn down at run time, by convention and library; the language does
not stop a process from outliving its parent.

After Trio the idea went into the mainstream quickly. Kotlin made every
coroutine the child of a `CoroutineScope` in 2018 [@elizarov2018]. Swift
shipped task groups and `async let` with structured child tasks in 2021
[@se0304]. Python 3.11 (2022) added `asyncio.TaskGroup`. Java 21 (2023)
previewed `StructuredTaskScope` [@jep453]. All of them give the scope an
object or a block. Okkama gives it the `await`: a task that waits for a future
it started owns it while it waits, which covers the common case of one child
without any extra construct, and leaves the objects of Section 20.7 for the
cases with many.

## 20.6 Channels and the state owner

Okkama has no mutable fields and no shared mutable state. Two tasks therefore
cannot share a counter, a cache or a repository by writing to it. They can
send each other values. `async/channel` provides `Channel<T>`, a bounded FIFO
of values of one type, and two capabilities over it: `Sending<T>`, which can
only send, and `Receiving<T>`, which can only receive.

A component with state is one task that owns the state as a local binding and
changes it by rebinding that binding to a successor. Everybody else talks to
it through a channel:

{{example:20-ledger}}

- The ledger's balance is a local of `run()`. Nobody else can reach it, so
  nobody else can change it, and the commands that change it are handled one
  at a time, in the order they arrived.
- `balance = balance.deposited(sum)` rebinds the local to the successor that
  `deposited` returned. `Balance` itself never changes.
- A command that needs an answer carries its own reply channel of capacity 1.
  Every request has its own, so concurrent answers cannot reach the wrong
  client.
- The teller holds only a `Sending<Command>`. It can ask; it cannot read the
  ledger's inbox, close the reply channels of others or touch the balance.

This is the actor pattern of Erlang [@armstrong2003] built from ordinary
parts: a task, a channel and a match over commands, with no new syntax. What
Erlang's mailboxes do not have, and Okkama's channels do, is a bound. A
channel of capacity 4 holds four values; the fifth `send` suspends until the
owner takes one, so a fast producer cannot bury a slow owner under messages.
Capacity 0 is a rendezvous: a `send` finishes only when a receiver takes the
value.

The rest of a channel's contract is short:

- values, blocked senders and waiting receivers are served in FIFO order, and
  each value is received exactly once;
- either endpoint may `close()` the channel, any number of times; after it,
  sends throw `ChannelClosed`, values already buffered can still be received,
  and then `item()` throws `ChannelClosed` too;
- dropping an endpoint does not close the channel, and neither does the
  failure of a task that used it. The owner closes it, usually in a `defer`;
- cancelling a task blocked in `send` or `item()` takes it out of the queue,
  and the next value goes to the next waiter.

There is no `try_send`, no `try_receive`, no `closed()`: a task that wants to
know whether it can proceed waits, and the scheduler runs somebody else
meanwhile.

## 20.7 Fan-out, races and deadlines

Structured await covers a task that waits for one thing. When a task starts
several and waits for all of them, or for the first, it needs an object that
owns them all. `async/collect` and `async/timeout` provide them:

{{example:20-fanout}}

- `AllFutures(origins: quotes).results()` starts all three quotes and waits
  for all of them. The results come back in the order of the inputs, not the
  order of completion. If one fails, the others are cancelled and their
  cleanup awaited before the failure is thrown.
- `Within(origin, limit, clock).result()` gives the courier 20 milliseconds.
  When time wins, the courier's request is cancelled and its cleanup runs
  (`courier: request withdrawn`) before `TimedOut` is thrown. `Before` is the
  same with a deadline computed once and shared by several operations.
- `Raced` returns the first outcome to settle, success or failure, and cancels
  the rest. `FirstSuccessful` ignores failures while a success is still
  possible. `FutureCompletions` hands back the futures in the order they
  settle. `FutureGroup` and `FutureScope` own a changing set of children.

Each of these objects follows the rule of Section 20.5: a child it cancels is
waited for, so when its `result()` returns or throws, no child it started is
still running. The deadline uses a monotonic clock that the program passes in
(`SystemClock()` here, a manual clock in a test); a civil `Instant` is not
accepted as a deadline, so a change of the wall clock cannot fire or hold
back a timeout.

## 20.8 Waiting for each other: events, locks, semaphores, barriers

Channels carry values. The other modules of `async/` coordinate tasks without
carrying anything:

- `Event` (`async/event`) is a one-way latch. `wait()` suspends until somebody
  calls `set()`; after that, every `wait()` returns at once. `Event(true)`
  starts set. The examples of this chapter use events to hand control from
  one task to another at a point the reader can see.
- `Lock` (`async/lock`) gives one task at a time a `LockGuard`; `close()` on
  the guard releases it.
- `Semaphore` (`async/semaphore`) hands out up to *n* `Permit`s; a task that
  asks for one more waits until a permit is closed.
- `Barrier` (`async/barrier`) releases a group of tasks together, once the
  last of them has arrived.
- `Atomic<T>` (`async/atomic`) is a cell with `value()`, `replace()` and
  `compare_and_replace()`. It is the one place in the standard library where a
  value can be changed in place, and it exists for coordination, not for
  application state: a counter of active tasks, a flag that a loop polls.
- `Cancellation` (`async/cancellation`) is a token that long synchronous
  loops check between steps, because cancellation reaches a task only at a
  suspension point and a loop that never suspends would never see it.

All of them are cooperative: a task waiting on any of them is suspended, and
the scheduler runs others. Waking a waiter follows the rule of Section 20.3:
`set()`, `close()` on a guard or a permit, or the last arrival at a barrier
makes the waiters ready; they run after the current task suspends, in the
order they started waiting.

A lock in Okkama protects less than a lock in Java. There is no shared
mutable memory to protect, so a lock serializes *sections of work*, for
example two tasks that must not talk to the same device at once. When the
work is "update the shared state", the state owner of Section 20.6 is the
better tool: it needs no lock because nobody but the owner can reach the
state.

## 20.9 Resources across suspension

A resource held by a task must be released when the task ends, however it
ends: by returning, by failing, or by being cancelled. Chapter 11 described
`defer` and `destroy` for synchronous code. Both work unchanged across
`await`: the scope that registered them is still the scope when the task
resumes, and unwinding a cancelled task runs them like any other unwinding.

Some cleanup has to wait itself: return a connection to a pool, flush a
buffer to a socket, tell a server that a session ends. An `async destroy`
block is allowed to `await`, and the scope that owns the object awaits it
when it closes (issue #154, 0.4.2):

{{example:20-lease}}

- `job.cancel()` returns before any cleanup has run: `done` is still `false`.
- The job is cancelled at its `await`. Its scope closes, and the lease's
  `async destroy` runs. It waits for `returned`, which `main` sets.
- The cleanup is *shielded*: cancellation does not interrupt it. The job's
  future settles only after the lease has been returned, so `await job`
  throws `Cancelled` after `lease: returned to the pool`, never before.

An `async destroy` can only belong to an object owned by an async scope. A
synchronous action cannot await, so it cannot own an object whose cleanup
suspends: `Lease has an async destroy; own it in an async action`. The rule
keeps "every cleanup has run" true at the end of every scope.

## 20.10 Streams: `for await`

A source that produces values one at a time, each after a wait, is consumed
with `for await`. The source is anything with an `async next()` that returns
`Maybe<T>`: `Maybe.some(value)` is the next item, `Maybe.none` the end.

{{example:20-stream}}

Each iteration awaits one `next()`; there is no prefetch, so the loop asks for
a value only when the body is ready for it. `last` leaves the loop without
asking again, `next` goes to the next item, and failures and cancellation
unwind as in an ordinary loop. The loop does not close its source: here the
reader closes the channel when it has sent everything, and that close is what
`next()` turns into `Maybe.none`.

## 20.11 Async in roles, generics and tests

Being async is part of an action's signature, and roles keep it. A role that
declares `async load(key is string) is string` is implemented only by an
`async` action of the same shape; a synchronous `load` does not do the role:

{{example:20-shape-rejected}}

The rule goes both ways. A caller of a role's async action always gets a
future, whoever implements it, so the caller's code does not change when a
cache in memory replaces a database; and an implementation cannot quietly
block the scheduler by answering synchronously where the role promised to
wait.

`Future<T>` is an ordinary generic type, invariant like every Okkama generic:
a `Future<Money>` is not a `Future<Amount>` even if `Money` does `Amount`
(Chapter 17). `Channel<T>`, `Sending<T>` and `Receiving<T>` are the same, and
a reply channel in a command carries its own `T`.

A role's fake (Chapter 15) may have async actions too, and tests may be async:

{{example:20-store-tests}}

The second test spawns two greeters and awaits both. `spawn` needs a call of
the form `Receiver(…).action(…)` (or a typed bound action value): the
receiver is built in the `spawn` itself, so what the task runs on is never a
local that the spawning code still uses. An async test runs on the same
scheduler as `async main`; when it ends, its spawned tasks are cancelled and
drained, and a failure nobody awaited fails the test.

## 20.12 A worked example: a pool of connections

The pieces of this chapter fit together in a pool that lends a limited number
of connections to whoever asks. Four jobs share a pool of two:

{{example:20-pool}}

The output follows from the rules of Section 20.3, step by step:

- `spawn` runs each job at once, until it suspends. Jobs 1 and 2 get a
  permit without waiting, print `connected` and suspend at `go.wait()`. Jobs
  3 and 4 find no permit left and suspend in `permit()`, in that order.
- `main` prints its line, sets `go` and suspends at `await all.results()`.
  Setting `go` made jobs 1 and 2 ready; it ran neither.
- Job 1 resumes, returns 10 and ends. Its scope closes, so the connection's
  `destroy` runs: the permit goes back, and job 3, the first in the queue of
  the semaphore, becomes ready behind job 2. Job 2 does the same and wakes
  job 4.
- Job 3 resumes inside `permit()`, connects, and its `go.wait()` returns at
  once because `go` is already set, so it ends without suspending. Then
  job 4.

Nothing in the program says "job 3 after job 2". The order comes from three
rules: a wake makes a task ready but does not run it, ready tasks run in the
order they were woken, and waiters of a semaphore are served first come,
first served. The pool itself is a dozen lines, and no line of it handles
concurrency: the `Semaphore` bounds the connections, `destroy` returns them
on every path out of a job, cancellation included, and `AllFutures` makes
`main` wait for all four.

## 20.13 Failures across tasks

A failure inside a task belongs to the task's future. It is kept there, like
a value, and thrown at every `await` of that future, where an ordinary
`try`/`catch` takes it. A failure that nobody awaits is not dropped:

{{example:20-lost}}

`main` finishes normally, but the run exits with status 1 and prints the
payment's failure on stderr: `Refused: Refused.because("card expired")`. When
`async main` returns, the scheduler cancels the spawned tasks still running,
waits for their cleanup, and then looks for a spawned task that failed and
was never awaited. The first one found in spawn order fails the run. A
failure cancelled away is not one of them: `Cancelled` is what the scheduler
asked for.

The rule is the asynchronous form of the one in Chapter 10: a failure is
either handled or reported, never lost on the way. A program that really does
not care whether a background task failed says so where it starts it, with a
`try` around the body of the task.

## 20.14 How the engines run tasks

The three engines keep the same rules with three different mechanisms.

The **IR engine** gives each task a stack of its own, a stackful coroutine
(the `corosensei` crate), on the program's one thread. `await` of an
unsettled future suspends the coroutine in the middle of the interpreter, with
the Rust frames of the interpreter on its stack; resuming it continues there.
The launch loop owns a queue of ready tasks: completing a future puts its
waiters on the queue, and the loop resumes them one at a time.

**Sparrow** is stackless. The heap frame of an async call *is* its future:
after a header come the state, the result or failure, the waiter, the future
being awaited, the resume point and every register of the function. An async
body is compiled to machine code that returns "completed" or "pending"; a
suspended `await` re-enters at the next instruction with its registers
reloaded from the frame. Asynchronous natives (timers, I/O, locks) are frames
too, run by Rust drivers. Woken frames go to the runtime's `ready` queue,
which the event loop drains. A suspended task costs only its frame, and the
garbage collector traces frames like any other object.

The **JVM** gives each task a Java thread, taken from a pool, and lets only
one run at a time: transferring control releases the target's semaphore and
blocks on one's own. The scheduler has the same ready queue as the IR engine,
and one shortcut: a task that the loop resumed hands control straight to the
next ready task when it suspends or ends, so a wake-up costs one thread switch
instead of a round trip through the loop's thread.
Threads make the JVM's task switch the most expensive of the three, a few
microseconds against tens of nanoseconds on Sparrow, and it is why the JVM is
the slowest engine on programs that are mostly waiting.

All three run on one thread at a time, which is what makes them agree: the
order of output in this chapter is the same on each, and the conformance
suite checks it.

## 20.15 What async costs, measured

The micro benchmarks of `okkama-docs/benchmarks/micro` repeat one operation in
a loop and subtract the empty loop. On an Apple M2 Max, with the 0.5.0
scheduler, one repetition costs, in nanoseconds:

| operation | Sparrow | IR | JVM |
|---|---:|---:|---:|
| a synchronous method call, for scale | 5 | 425 | about 30 |
| `await` of an async action | 38 | 707 | 6 708 |
| `spawn` and `await` of a task | 75 | 780 | 6 410 |
| a value sent and received over a channel | 89 | 913 | 13 173 |
| a task woken through an `Event` | 316 | 1 931 | 22 471 |
| eight tasks woken by one `Event` | 2 354 | 17 710 | 163 866 |
| a waiting task cancelled and awaited | 388 | 2 192 | 25 566 |
| the same, with an `async destroy` to settle | 852 | 3 580 | 40 021 |

On Sparrow an `await` costs as much as eight method calls, because an async
call is a heap frame and a resumption is a jump back into compiled code. The
IR engine pays its interpreter's price for everything, so an `await` costs
about two of its method calls. The JVM pays for threads: each task is a Java
thread, and handing control from one to another costs microseconds, two
hundred times a method call. A program that waits a lot belongs on Sparrow;
the JVM is the engine for long computations between waits.

## 20.16 Okkama among other languages

| | Okkama | C# | JavaScript | Python asyncio | Rust | Kotlin | Go |
|---|---|---|---|---|---|---|---|
| A call starts the work | no (lazy) | yes | yes | no (coroutine) | no (lazy) | `async {}`: yes | `go`: yes |
| Outcome kept, awaitable again | yes | yes | yes | Task: yes | no | `Deferred`: yes | — |
| Wait only in async code | yes | yes | yes | yes | yes | `suspend` only | no colour |
| Runs between suspension points without interleaving | yes | no (thread pool) | yes | yes | depends on executor | depends on dispatcher | no |
| Cancelling a waiter cancels what it waits for | yes (#147) | no | no | yes | dropping a future | yes (scope) | no |
| Children outlive their scope | spawned only | yes | yes | without TaskGroup | yes (`spawn`) | no | yes |
| Parallelism | process workers | threads | workers | processes, threads | threads | threads | threads |
| Lost failure | fails the run | unobserved-task event | `unhandledrejection`; Node.js exits | "never retrieved" log | `#[must_use]` warning | cancels the scope | a panic ends the program |

Two columns stand out. JavaScript and asyncio make the same promise as Okkama
about interleaving, and pay the same price, a colour on every function that
waits. Go makes no promise and pays no colour: any function can block, any
goroutine can run at any moment, and shared state needs locks or channels by
convention. Okkama chose the coloured side because it already makes effects
visible elsewhere (successors, `^` fields, `try`), and a suspension point is
an effect: the place where the rest of the program may change things.

## 20.17 Limits

- **Detached work.** A spawned task that its starter neither awaits nor
  cancels runs until `async main` returns. Nothing ties it to the scope that
  spawned it; only `FutureGroup` and `FutureScope` do that, explicitly.
- **No cancellation in synchronous code.** Cancellation is delivered at a
  suspension point. A long synchronous loop sees it only if it checks a
  `Cancellation` token.
- **JVM task switches are slow.** An `await` of an async action costs about
  6.7 µs on the JVM, 176 times Sparrow (Section 20.15, issue #327), because
  each task is a thread.
- **Async cleanup guards cost time on Sparrow.** An `async destroy` guard in
  the HTTP server's state delays every server run by about 0.8 s (issue
  #328), and the remaining cleanup guards of std stay synchronous until that
  cost goes (issue #329).
- **Most of std is synchronous.** Asynchronous files, pipes, CSV streams and
  decorators are open work (issues #121, #196, #204, #131), and `for await`
  over a channel's receiving end needs the small adapter of Section 20.10
  (issue #199).
- **Until 0.5.0** the IR engine and the JVM ran a woken task inside the call
  that woke it (issue #340, Section 20.3), and the channel documentation
  described an older API (issue #341).

## 20.18 What this design costs

Laziness costs an explicit `spawn` wherever work should start now, and a
reader has to know that `Oven("rye").baked()` alone does nothing. Colour costs
async signatures all the way up from the first action that waits, and an
`async main`. The single scheduler costs parallelism inside one process: the
only way to use two cores is a worker process, which copies its arguments and
its result and cannot share objects. Structured await costs a waiting caller
the time the child takes to clean up when it is cancelled.

What they buy is a program in which every point where other code can run is
written down as `await`, in which no child that a task waits for outlives it,
in which a failure cannot disappear, and in which three engines that share no
code print the same lines in the same order.
