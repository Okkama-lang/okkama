# Chapter 21 — archived implementation passages

Replaced mixed paragraphs are preserved in full for context.

## Chapter 21. Processes, natives and capabilities

described 0.5.x

Everything the earlier chapters built computes. An object holds values, an
action derives new ones, a future waits for a result that another part of
the program will produce. None of it reaches outside the program. The world
outside — files, sockets, clocks, other processes, the terminal — is reached
in one way only: through a *native*, a function the engine provides and the
program declares. This chapter is about that boundary: how a native is
declared and answered, what a program holds when it holds a file or a
socket, how authority over the host is passed around as an ordinary value,
how work moves into another process, and what an embedder can deny.

## 21.1 One door to the host

described 0.5.x

A native declaration has a signature and no body. It is a request: when the
program calls `monotonic_clock()`, the engine looks up a provider registered
under that module and name and runs it. The provider is Rust on the IR
engine and on Sparrow, Java on the JVM, and the three are tested to give the
same results (Chapter 26). The `runtime/*` modules are the only place in the
standard library where natives are declared; everything above them is
ordinary Okkama that can be read, tested and faked.

## 21.1 One door to the host

described 0.5.x

The program compiles and starts. `before the call` is printed; the call
fails with `RuntimeError: Native symbol "bible/unregistered@1::vault_open/1"
is not registered`, and the run ends with status 1. A native's name is its
module, the module's major version, its own name and its arity, and only a
provider registered under exactly that name answers it. A program cannot
reach the host by inventing a declaration: the set of doors is the set the
engine was built with, plus whatever an embedder adds (Section 21.7).

## 21.2 What a native may take and return

described 0.5.x

A call returns a future like any async action. The engine starts the host
operation without blocking the scheduler, other tasks run meanwhile, and
the future settles when the host reports (Chapter 20). The program never
sees the host's threads or callbacks.

## 21.3 Native objects: handles

described 0.5.x

The provider keeps every handle it issued in a table, with a generation and
an open or closed state, and checks the table on every call:

## 21.3 Native objects: handles

described 0.5.x

- `close` returns `true` both times. Closing is idempotent, and the
  resource behind the handle is released exactly once.
- Writing through the closed handle does not reach a reused descriptor or
  crash: it fails with a `NativeFailure` whose code is `already_closed`,
  which an ordinary `catch` takes.
- If the program forgot `close`, the runtime would close every remaining
  handle when the run ends, however it ends, without waiting for a garbage
  collector.

The same three engines print the same four lines. Programs rarely hold
handles directly: std wraps each in an object with a `destroy` (Chapter 11),
so `FileRoot`, `TcpConnection` and the database connections close their
handles when their owner's scope ends, and the handle is a field nobody
else can reach.

## 21.5 Running other programs

described 0.5.x

A child that has to be talked to rather than waited for is driven through
`Process.streams()` (issue #196). Its input does `AsyncWriting` and
`Closing`; its outputs do `AsyncReading`, with `lines()` for `for await`.
A slow child then holds up only the task that talks to it, on every engine:

## 21.5 Running other programs

described 0.5.x

Section 20.4 introduced the other way an Okkama program starts a process:
`spawn` of a *synchronous* action runs that action in a worker process, in
parallel with the program. Seen from this chapter, a worker is a command the
runtime builds itself, and each of its properties follows from that.

**The worker is the same program.** The runtime starts the program again,
with the same engine, in a mode that runs one action instead of `main`:
Tomato its `.spb` (a program compiled in memory is first written to a
temporary file), the IR engine its source files, the JVM its classes. The
child gets the parent's environment and working directory as the parent's
runtime sees them, so a relative path means in the worker what it meant in
the parent. It does not get the parent's heap, its open handles or its
tasks. There is no fork and no copy of memory: the worker starts empty,
loads the same code, and is handed a receiver and arguments.

## 21.5 Running other programs

described 0.5.x

**Cancellation is a kill.** Section 20.5's cancellation is cooperative: it
reaches a task at its suspension point and unwinds it, defers and `destroy`s
included. A worker is a synchronous action with no suspension points, so
there is nothing to cooperate with. Cancelling its future kills the process
(SIGKILL): on Tomato and the IR engine the worker leads its own process group
and the whole group goes, so processes the worker started go with it. The
future settles `Cancelled` only after the child has been reaped, so the code
after `await` never runs beside a worker that is still writing. A worker
cancelled before its process started is never started. Its defers do not
run, and a worker that must clean up after itself must not be cancelled; it
should be given a deadline among its arguments instead. (Until 0.5.3 the IR
engine and the JVM sent TERM and settled the future at once; issue #359.)

## 21.5 Running other programs

described 0.5.x

Standard output is buffered (issue #132). A terminal receives each line as
it ends; a pipe or a file receives blocks of 64 KiB; `Stdout().flush()`
writes out what is pending. The order of what a reader sees survives,
because the runtimes flush before standard error is written, before
standard input is read from a terminal, before a child process starts,
before the terminal is written to, and when the program ends. The reason is
plain arithmetic: writing 200 000 lines into a pipe took Sparrow 155 ms and
the IR engine 1 084 ms when every line was its own system call, and 56 ms
and 590 ms after. The JVM still flushes every write.

## 21.5 Running other programs

described 0.5.x

A line loses its LF and the one CR before it, so a file written on Windows
reads the same as one written on Unix. Input that is not valid UTF-8 is a
failure by default; `Stdin(decoding: InputDecoding.lenient)` puts U+FFFD in
place of each bad sequence instead, as browsers do, and
`Stdin().chunks(size: 65536)` walks bytes. Every `Stdin()` reads through the
one buffer the runtime keeps, so a program can read a header line by line
and the rest as bytes without losing what sat between them.

## 21.6 What an embedder can deny

described 0.5.x

A program run by `okkama run` may use every native. A program run by a
host — an application that embeds `sparrow-core` to run plug-ins, or a
service that runs users' scripts — usually should not. The embedding entry
point takes `budget::Limits`, and `Limits::natives` lists the *native
groups* the program may use:

## 21.6 What an embedder can deny

described 0.5.x

The same `Limits` carries the budgets of the security model: steps, memory,
output and an interrupt flag the host may raise from any thread. Running
out stops the run with its own report, which no `catch` intercepts
(Chapter 10). Since 0.5.1 the memory budget can measure one runtime
instead of the whole process (`MemoryScope::Runtime`, issue #269): on
Sparrow it reads the runtime's own heap, so a host can run several
programs side by side, and the one that grows past its budget stops while
the others go on.

### 21.6.1 An embedder, written out

A host that runs a plug-in it did not write states the whole contract in one
value. In Rust, against `sparrow-core`:

```rust
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use sparrow_core::budget::{Limits, MemoryScope, NativeGroup};

fn run_plugin(spb: Vec<u8>, arguments: Vec<Vec<u8>>, stop: Arc<AtomicBool>)
    -> Result<i64, sparrow_core::Error>
{
    let limits = Limits {
        steps: Some(200_000_000),          // function entries and loop back-edges
        memory: Some(64 << 20),            // 64 MiB of this runtime's own heap
        memory_scope: MemoryScope::Runtime,
        output: Some(1 << 20),             // 1 MiB of stdout and stderr
        interrupt: Some(stop),             // any thread may raise it
        natives: Some(vec![NativeGroup::Clock]),
    };
    // No worker command: `spawn` of a synchronous action fails here.
    sparrow_core::run_limited(spb, arguments, false, Vec::new(), limits)
}
```

Each field is one line of the host's policy, and no field has a default
that grants something. `natives: Some(vec![NativeGroup::Clock])` is the
answer to "what may this program do outside itself?": read the clocks, and
the computation and printing every program may do. A plug-in that opens a
file fails at that call with a report that names the group it lacked. A
plug-in that loops forever runs out of steps; one that builds an unbounded
list runs out of its 64 MiB; one that prints in a loop runs out of output.
The host's watchdog thread can end any of them early with
`stop.store(true, Ordering::Relaxed)`. Each of these ends the run with its
own message (`Execution budget exhausted: steps`, `… memory`, `… output`,
`Execution interrupted`), which no `catch` in the plug-in intercepts and
after which no `destroy` runs: the runtime is discarded with its heap.

`MemoryScope::Runtime` is the reason one host process can run many such
plug-ins at once. The default scope, `Process`, reads the process allocator
through a probe the embedder installs, which is right for the CLIs (one run
per process) and wrong for a server, where one plug-in's memory would be
charged to all of them. With `Runtime`, Sparrow reads the bytes its own heap
holds, its pages and large objects and the Rust memory its objects own, so
every runtime keeps a budget of its own and no probe is needed.

The empty worker command is a decision too. Process workers (21.5.1) start
the same program again as a child process, so a host that allows them hands
the plug-in a way to create processes. The core refuses a `spawn` of a
synchronous action when the host gave it no command to run, and a host that
does give one should also allow `NativeGroup::Process`, which says so.

The guarantees hold only as far as the classification of the natives does.
Every native of `runtime/*` must be named either in a group or among the
pure ones. A native that is in neither is refused whenever the host
restricts the groups, and a test fails the build when a module is left
unnamed. Until 0.5.3 the table failed open: a module it did not name counted
as pure, and two natives that read host facts, the preferred languages and
the system's trusted certificates, passed a host that had denied `Env`
(issue #361). What a group cannot express, such as "files, but only under
this directory", is expressed in the program's own terms: the host passes a
`FileRoot` as an argument (21.4), and the program can name nothing outside it.

## 21.7 The command line keeps the user's authority

described 0.5.x

Other systems put the boundary elsewhere. Deno runs a script with no
access at all and asks for flags such as `--allow-read=./data` and
`--allow-net=example.org` [@deno]. WebAssembly's system interface gives a
module only the directories the host pre-opens for it, as descriptors, the
way `FileRoot` works [@wasi]. Java went the opposite way: its Security
Manager, which checked permissions on every sensitive call inside one
process, was deprecated for removal in Java 17, because few programs used
it and every library had to be written for it [@jep411]. Okkama's choice
sits between them. Inside the language, std passes authority as values
(Section 21.4), and a host that embeds the runtime can cut whole groups.
Below that, the operating system does the isolating: a container, a jail,
a user without rights. The boundary is drawn there because an
interpreter-level sandbox is only as strong as its least careful native,
and the runtime modules declare 653 of them.

## 21.9 What this design costs

described 0.5.x

The narrow native ABI costs copies: a string or a byte array crosses by
value, and a large one is copied each time it crosses. The opaque handles
cost a table lookup and a generation check on every call. Explicit
authority costs parameters: a function that reads one file takes an
object that names where it may read, where another language would let it
open a path. The command line's full authority costs isolation that a
host must provide itself.

What the design buys is a short answer to "what can this program do":
the natives its imports reach, filtered by the groups its host allows.
That answer fits on one page, and every engine gives the same one.

