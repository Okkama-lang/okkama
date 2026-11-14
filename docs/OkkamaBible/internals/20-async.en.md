# Chapter 20 — archived implementation passages

Replaced mixed paragraphs are preserved in full for context.

## 20.2 Where futures came from

described 0.5.x

The `async`/`await` pair came from F#. Its asynchronous workflows (2007,
described in [@syme2011]) let a programmer write a sequential-looking body
that the compiler cuts at each `let!`, and an F# `Async<T>` is a *recipe*: it
runs only when started, and it can be started more than once. C# 5 (2012)
took the syntax with a different value: its `Task<T>` is already running when
the call returns. JavaScript's promises (2015) and `async` functions (2017)
are eager in the same way. Rust's futures (2019) went back to lazy values that
do nothing until polled.

## 20.3 One scheduler, one task at a time

described 0.5.x

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

The narrower rule, that waking never runs the woken code inside the waker,
has its own history in JavaScript. Promises/A+ requires that a promise's
callbacks "must not be called until the execution context stack contains
only platform code" [@promisesaplus], that is, never inside the call that
settled the promise. The reason was a class of bugs that Isaac Schlueter
called releasing Zalgo [@schlueter2013]. An API that calls its callback
sometimes at once and sometimes later runs the code after the call in two
different orders, and only one of them gets tested. His advice was to pick
one timing and keep it. Before 0.5.0 Okkama had the engine-level form of the
same bug: which of `main: after set` and `waiter: woke` came first depended
on the engine. The fix was JavaScript's too: a queue of ready work that runs
only when the current task yields.

Cooperative multitasking is older than async. Classic Mac OS ran its
applications this way from MultiFinder in 1987 to Mac OS 9, and 16-bit
Windows did the same: a program kept the processor until it called back into
the system, so one program stuck in a loop froze the machine. Okkama accepts
that cost inside one run and not across programs. A task that computes
without awaiting holds up every other task of its run, and nothing preempts
it (Section 20.17). Erlang went the other way: its virtual machine switches
processes after a fixed budget of reductions, so no process starves the
rest. It does not need the guarantee above, because its processes share no
memory. Go started cooperative and made goroutines asynchronously
preemptible in Go 1.14, in 2020, so that "loops without function calls no
longer potentially deadlock the scheduler" [@go114].

## 20.4 `spawn`: work that runs beside its starter

described 0.5.x

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

## 20.5 Structured await and cancellation

described 0.5.x

What happens to the future a cancelled task was waiting for? Okkama's answer,
since 0.4.2, is that `await` is *structured* (issue #147). Cancelling a task
cancels the future it awaits, if that future is its own: one its `await`
started, a task it spawned, or a request to the runtime. And the cancelled
task does not finish until that future has settled, cleanup included. Only
then does its own `await` throw `Cancelled` and its own cleanup run:

There is a moment between two states that the rule above does not cover. A
task's awaited future settles; the task is woken and put in the queue of
ready tasks; before its turn comes, someone cancels it. Is the task still
"waiting at a suspension point"?

Until 0.5.7 the two Rust engines answered differently. The IR engine said
yes and turned the finished `await` into `Cancelled`, throwing away the
value that had arrived. Sparrow said no: the `await` was over, the task ran
on with its result, and the cancellation reached it at its next `await`.
The difference was invisible in most programs and very visible in one:
when an `AllFutures` sibling failed and the collection cancelled the rest,
siblings that had already been woken either vanished as cancelled (IR) or
failed on their own and showed up as secondary failures (Sparrow).

The owner chose Sparrow's rule (issue #546), and since 0.5.7 every engine
keeps it:

The rule is the one that keeps information: a value that arrived is not
discarded, a failure that happened is not hidden behind `Cancelled`, and a
task that is cancelled still stops at its next suspension. A sibling of an
`AllFutures` that failed in the same turn now appears among the secondary
failures on every engine, in input order.

## 20.6 Channels and the state owner

described 0.5.x

- values, blocked senders and waiting receivers are served in FIFO order, and
  each value is received exactly once;
- either endpoint may `close()` the channel, any number of times; after it,
  sends throw `ChannelClosed`, values already buffered can still be received,
  and then `item()` throws `ChannelClosed` too;
- dropping an endpoint does not close the channel, and neither does the
  failure of a task that used it. The owner closes it, usually in a `defer`;
- cancelling a task blocked in `send` or `item()` takes it out of the queue,
  and the next value goes to the next waiter.

## 20.9 Resources across suspension

described 0.5.x

Some cleanup has to wait itself: return a connection to a pool, flush a
buffer to a socket, tell a server that a session ends. An `async destroy`
block is allowed to `await`, and the scope that owns the object awaits it
when it closes (issue #154, 0.4.2):

## 20.12 Worked examples: a pool and a pipeline

described 0.5.x

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

- Each stage closes its output in a `defer`, so the end of the input travels
  down the pipeline: the source finishes and closes `numbers`, `Square` sees
  `ChannelClosed`, leaves its loop and closes `squares`, and `Total` returns
  its sum.
- The channels hold one value each. The source can be at most a value or two
  ahead of the consumer; when `Total` is slow, `send` suspends the stages
  behind it. That is backpressure, and it needs no code: the bound of the
  channel is the policy.
- The interleaving of `source: sent` and `total: got` looks irregular, but it
  is fixed by the rules of Section 20.3, so it is the same on every engine
  and on every run.

## 20.13 Failures across tasks

described 0.5.x

A task that waits for an event only another waiting task can set waits
forever. The scheduler notices when that has happened to the whole program:
no task is ready, and nothing outside the runtime (a timer, a socket, a
process worker) is left that could wake one.

Both clerks print their line and wait; `main` waits for the first of them.
The ready queue is now empty and nothing is pending outside, so the run ends
with status 1 and, on stderr, `RuntimeError: async main cannot complete: no
task can make progress`. The lines already printed stay printed.

The check is exact, not a guess. It never fires because something took long:
a task that waits for a timer an hour away is not stuck, and the scheduler
sleeps until the timer instead. It is also global. Two tasks that wait for
each other while a third keeps a timer or a socket alive are not reported
until nothing else is pending, so a server that listens forever never trips
it, and a deadlock inside it shows up as requests that never complete, as in
any other language. The conformance cases `L08.deadlock.*` pin the message
and the status on every engine.

## 20.14 How the engines run tasks

described 0.5.x

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

described 0.5.x

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

0.5.1 made the JVM's hand-off cheaper (issue #327). A thread that gives up
control used to park on a semaphore at once and be woken a few microseconds
later; it now spins on its permit for up to 20 µs first, because control
usually comes back sooner than that. The order of hand-offs is the same.
Only two threads spin at a time: when more did, eight tasks woken by one
`Event` competed for the cores with their spinning neighbours and got
slower. An `await` now costs 0.77 µs on the JVM, `spawn` and `await`
0.87 µs, a channel round trip 1.15 µs, a wake through an `Event` 7.4 µs,
eight waiters 136 µs and a cancelled task 20 µs.

### What a waiting task holds

Ten thousand tasks, each waiting on one `Event`, then woken and awaited all
together, against the same program with none (0.5.1, peak resident memory):

| engine | memory per waiting task | 10 000 tasks, whole run |
|---|---:|---:|
| Sparrow | about 270 bytes | 0.03 s |
| IR | about 35 KB | 0.72 s |
| JVM | about 220 KB (measured with 200 tasks) | — |

A Sparrow task is its heap frame: the locals that live across the `await`,
and nothing else. The IR engine gives each task a coroutine with a machine
stack of its own, and the JVM a thread whose stack grows as it is used. A
program that keeps one task per open connection can hold a hundred thousand
of them on Sparrow in 27 MB; on the JVM their threads alone would take 22 GB.
For scale, Go starts a goroutine with a stack of 2 KB [@go14].

### The costs that are not in the loop

A micro benchmark runs code that is already compiled. Sparrow compiles each
function to machine code on its first call, and a program that touches a lot
of code once pays for that before anything else. An HTTP server and client in
one program, serving 50 requests on loopback, ran for 834 ms on Sparrow in
0.5.1, and 765 ms of it went to compiling 601 functions, 2 MB of machine code;
the IR engine, which compiles nothing, finished the same program in about
350 ms (issue #345). Sparrow wins once the code runs long enough to repay its
compilation: a server that lives for hours, a loop over a large input. It
loses on short runs through a lot of library code.

`async destroy` had a cost of the same kind. A scope that may hold an object
with an async destroy closes with a settle, which may suspend, so on Sparrow
it is a resumption point like an `await`. Until 0.5.1 the compiler decided
this for the whole program: once any object had an async destroy, every
scope of every async action closed with a settle. Turning one cleanup guard
of the HTTP server into an `async destroy` grew the machine code from 2.0 to
2.9 MB and the compile time from 765 to 1 236 ms, half a second on every run,
while the work done stayed the same (issue #328). Now the compiler records
what each async action's scopes own, and once every action is compiled it
keeps the settle only where an owned value may reach an object with an async
destroy: the object, anything holding one in a field, a collection of them,
a role one of them does. A value hidden in `any` or captured by a closure
counts only if some such value is boxed or captured somewhere in the
program. The same HTTP program now settles in one action, the guarded loop
itself, and runs 11 ms slower than with `defer await`, not 480.

## 20.17 Limits

described 0.5.x

- **JVM task switches cost microseconds.** Each task is a thread. Since
  0.5.1 an `await` of an async action costs about 0.8 µs there, twenty times
  Sparrow (Section 20.15, issue #327); in 0.5.0 it was 6.7 µs.

- **std's cleanup guards are still `defer await`.** Since 0.5.1 an `async
  destroy` guard costs only the scopes that own it (issue #328, previous
  section); moving std's guards to it waits for the std rewrite (issue
  #329).

- **Short runs pay for compilation on Sparrow.** Compiling a program's code
  can take longer than running it (issue #345).

- **Until 0.5.0** the IR engine and the JVM ran a woken task inside the call
  that woke it (issue #340, Section 20.3), and the channel documentation
  described an older API (issue #341).

- **Until 0.5.1** Sparrow worded a deadlock differently from the IR engine
  and the JVM (issue #342).

## 20.18 What this design costs

described 0.5.x

What they buy is a program in which every point where other code can run is
written down as `await`, in which no child that a task waits for outlives it,
in which a failure cannot disappear, and in which three engines that share no
code print the same lines in the same order.
