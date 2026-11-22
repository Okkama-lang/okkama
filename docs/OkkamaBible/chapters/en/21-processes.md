# Chapter 21. Processes, natives and capabilities

Everything the earlier chapters built computes. An object holds values, an
action derives new ones, a future waits for a result that another part of
the program will produce. None of it reaches outside the program. The world
outside — files, sockets, clocks, other processes, the terminal — is reached
in one way only: through a *native*, a function supplied by the host and the
program declares. This chapter is about that boundary: how a native is
declared and answered, what a program holds when it holds a file or a
socket, how authority over the host is passed around as an ordinary value,
how work moves into another process, and what an embedder can deny.

## 21.1 One door to the host

An Okkama program cannot open a file by itself. `io/stdout`, `fs/file`,
`net/tcp`, `time/clock` and the rest of the library that touches the host
are written in Okkama, and each of them ends, a few calls down, in a
declaration such as this one from `runtime/time`:

```okkama
native monotonic_clock() is int;
```

A native declaration has a signature and no body. Calling it requests the provider registered for that module and name. The `runtime/*` modules are where the standard library declares natives; the layers above them are ordinary Okkama that can be read, tested and faked.

Any module may declare a native. Declaring one does not create it:

{{example:21-unregistered}}

The program compiles and starts. `before the call` is printed; the call
fails with `RuntimeError: Native symbol "bible/unregistered@1::vault_open/1"
is not registered`, and the run ends with status 1. A native's name is its
module, the module's major version, its own name and its arity, and only a
provider registered under exactly that name answers it. A program cannot
reach the host by inventing a declaration: the set of doors is the set the host provides (Section 21.6).

The rule has a cost that the next sections pay off. Every effect a program
can have outside itself has a name in `runtime/*`, so the list of what a
program might do is short and fixed: it is the list of natives its imports
reach. That list is what the native groups of Section 21.6 restrict.

## 21.2 What a native may take and return

The native ABI is deliberately narrow. The natives of the standard library
take and return `int`, `float`, `bool`, `string`, arrays and maps of such
values (`@byte`, `@string`, `%string:int`) and *native objects*; a few
diagnostic ones take `any`. No native takes or returns an action value, so
none can call back into Okkama. A native declaration has no body, no
defaults and no variadic parameters.
What crosses is data, copied or encoded; what comes back is data or a
handle.

A native may be `async`. `runtime/network` declares thirteen of them, among
them DNS lookups and socket reads and writes:

```okkama
native async dns_addresses(host is string, port is int) is @@string;
native async udp_receive(origin is SocketState, limit is int) is @byte;
```

A call returns a future like any async action. The host operation starts without blocking the scheduler, other tasks run meanwhile, and
the future settles when the host reports (Chapter 20). The program never
sees the host's threads or callbacks.

This is narrower than most foreign-function interfaces, on purpose. A C
library called through an FFI can keep a pointer into the caller's memory,
call a function it was given at any later time, or run on a thread the
caller does not know about. None of that is possible here: when a native
returns, or its future settles, it holds nothing of the program's.

## 21.3 Native objects: handles

A file, a socket, a timer or a database connection is represented in the
program by a *native object*:

```okkama
native object FileHandle;
native open(path is @byte, flags is int, mode is int) is FileHandle;
native close(handle is FileHandle) is bool;
```

A `native object` is a type with no fields, no actions and no constructor.
Only its provider can make one. The program can store it, pass it and give
it back to the natives of the same module, and nothing else: it cannot be
compared, used as a map key, put into `any`, printed or taken apart. What
stands behind it — a file descriptor, a socket, a TLS session — never
becomes a value the program can see.

Closing a handle is idempotent, and using a closed handle fails safely:

{{example:21-handle}}

- `close` returns `true` both times. Closing is idempotent, and the
  resource behind the handle is released exactly once.
- Writing through the closed handle does not reach a reused descriptor or
  crash: it fails with a `NativeFailure` whose code is `already_closed`,
  which an ordinary `catch` takes.
- If the program forgot `close`, the runtime would close every remaining
  handle when the run ends, however it ends, at the end of the run.

Programs rarely hold handles directly. std wraps each in an object with a `destroy` (Chapter 11), so files and connections close when their owner’s scope ends; the handle is a private field.

## 21.4 Authority is a value

A program run from the command line has the authority of the user who ran
it: it may read any file that user may read. Okkama does not change that
(Section 21.7). What std does is make the authority *visible*. Wherever a
library could act on an ambient, process-wide notion — the current
directory, the environment, the standard streams — it asks instead for an
object that names what it may use:

{{example:21-directory}}

`Reports` is given a `DirectoryLocating`, a role with one action, `path()`,
and builds its file names inside that directory. It has no way to name
another one: there is no `chdir` in std, no global "current directory" that
some other code could change behind its back, and `WorkingDirectory()`, the
process's own directory, is just one more object that does the role and has
to be passed like any other. Two `Reports` with two directories are two
writers with two different authorities, side by side in one program.

`FileRoot` takes the same idea down to the operating system. It opens its
directory once, as a descriptor, and resolves every later path relative to
it, segment by segment, so a name like `../../etc/passwd` or a symbolic
link planted by another process cannot lead outside: "coordinates never
become joined ambient paths", as its module says. The children it returns
hold descriptors of their own and stay valid after the root is closed.

This is the *object-capability* discipline, named by Mark Miller in his
work on the E language [@miller2006] and older than it: Dennis and Van Horn
described capabilities as unforgeable references that carry the right to
use what they designate [@dennis1966]. In a capability system a function
can affect only what it was given, so reading its parameters tells you what
it can do. Okkama's std follows the discipline where it costs nothing and
the language keeps it honest — objects cannot be forged, fields are
private, roles cannot be downcast (Chapter 14) — but it is a discipline of
the library, not a guarantee of the language: a program may still import
`runtime/fs` and open a path by name, as the example of Section 21.3 did.

### 21.4.1 The environment and the clock

The same discipline covers the two other things a program usually reaches
for without asking: its environment variables and the time.

`Environment` is a snapshot, not a window. `Environment()` reads the
process environment exactly once, when it is constructed, and from then on
the object answers from its own copy; nothing another part of the program
does to the process can change what it says. `Environment(values: …)` makes
one that never looked at the process at all. `addition(name, value)` and
`difference(name)` return new snapshots and leave the receiver as it was,
which is how a child process gets an environment of its own (Section 21.5)
without the parent's changing. A variable is read like a json value (#134):
`text(name)` gives it as a string and throws `MissingEnvironmentVariable`
when it is not set, `int`, `boolean` and `list` read it typed (`list`
splits `PATH`-like values at the platform's separator), and
`maybe(name).text() else "stranger"` reads an optional one in one line. A
variable that is missing and one that is empty are different answers:
`membership` asks the first question, `text` the second. A name with `=`
or NUL in it is refused before any snapshot can hold it.

Time comes through a role. `Timing` has three actions — the wall-clock
`instant()`, the `monotonic_instant()` that only moves forward, and an
`async wait` — and `SystemClock` is the object that does it with the host's
clocks. Code that measures or waits asks for a `Timing`; a test gives it the
`ManualClock` of Chapter 20, which advances only when the test says so, and
a program can write its own:

{{example:21-authority}}

`Greeting` can read exactly two things outside itself, and both are its
fields. Run it twice with two snapshots and one fixed clock, and it prints
two lines that do not depend on who runs it or when. That is the practical
payoff of authority as a value: the parameters of an object are a complete
list of what it can observe, so a test needs no mocking framework and a
replay needs no patched system calls.

The clock keeps one more promise. A monotonic instant carries the *domain*
of the clock that made it, and subtracting or comparing instants of two
domains throws `IncompatibleClock` instead of returning a number that means
nothing: a reading of the manual clock and a reading of the system clock
are not on one line, and std will not pretend they are. An instant made
directly, as `FixedClock` makes its own, belongs to the `explicit` domain.

As with directories, this is the library's discipline, and std does not yet
keep it everywhere. `Environment()` and `SystemClock()` can be constructed
anywhere, and some of std's own modules still do so where they should ask:
the HTTP client and its sessions, TLS, `async/timeout` and the cookie jar
make their own `SystemClock()`, and `os/command` reads `Environment()` to
pass it on. A test of such code cannot hand it a manual clock. Making every
module take its clock and its environment as values is one of the
principles of the standard library's review after 0.5.3 (issue #352).

## 21.5 Running other programs

A program that runs another one describes it first, as a value:

```okkama
let status is Commanding = Command(
    program: Path("/usr/bin/git"),
    arguments: ["status", "--short"],
    environment: environment,
    directory: repository,
);
```

`Command` (`os/command`) holds the program, the exact arguments, the
environment and the directory, and nothing reads the process's own state
behind its back. No shell is involved at any point: each argument reaches
the child as one string, so a file name with a space, a quote or a
semicolon is a file name and not a second command. `Path("git")` without a
directory asks the runtime to look the program up in the `PATH` of the
command's *own* environment. A program that does want to inherit says so
with `InheritedCommand`, which reads the process environment and working
directory when it runs and only then.

`Process` (`os/process`) owns the child of a command: its standard streams,
`signal`, and `async exit()`, which waits for the child without blocking
the other tasks. Constructing a `Process` does not start it; its first
operation does, exactly once. `close()` is idempotent: it stops and reaps a
running child and discards what was left unread. A process handle still
open when the run ends is closed by the runtime like any other
(Section 21.3), so no child outlives the program by accident.

The common case needs neither object spelled out. `Run` (`os/run`, issue
#197) runs a program found on `PATH` with this process's environment and
directory, and gives back what it printed:

```okkama
let branch = await Run("git", ["rev-parse", "--abbrev-ref", "HEAD"]).text();
let finished = await Run("make", ["test"]).completion();   // any exit code
```

`text()` and `bytes()` want exit code 0. Any other end is
`CommandFailed.exited(code, errors)`, carrying what the program wrote to
standard error, so a tool that failed cannot pass for one that printed
nothing. Each stream is kept up to 1 MiB; more is `CaptureLimitExceeded`,
not a silent cut. `Run` inherits the way `InheritedCommand` does, and its
documentation says so; code that must not hand the process's environment to
a child builds a `Command` and captures it with `CommandCapture`, which also
sets the input, the directory and the limit.

A child that has to be talked to rather than waited for is driven through
`Process.streams()` (issue #196). Its input does `AsyncWriting` and
`Closing`; its outputs do `AsyncReading`, with `lines()` for `for await`.
A slow child then holds up only the task that talks to it:

```okkama
let child = Process(command: InheritedCommand(program: Path("sort")));
await child.streams().input().write("pear\napple\n");
child.streams().input().close();
for await child.streams().output().lines() -> line {
    Stdout().writeln(line);    // apple, then pear
}
```

The design leaves out what the shell gives for free and what most process
APIs inherit by default: globbing, word splitting, redirection syntax, an
implicit environment. Each of them is a way for data to become code or for
one part of a program to act with another part's settings. A command
built from values carries exactly what it was given.

### 21.5.1 Process workers

Section 20.4 introduced process workers: `spawn` of a synchronous action runs it in another process, in parallel with the program. The process boundary determines what it can receive and return.

**The worker runs the same program.** It runs one action instead of `main`, with the parent’s environment and working directory. It receives a receiver and arguments but no objects, open handles or tasks from the parent.

**Only values cross.** The receiver's fields and the arguments are encoded in
the parent and decoded in the child: `int`, `float`, `string`, `char`,
`byte`, `bool`, and arrays and maps of them. An object cannot cross, because
an object is an identity (Chapter 12), and an identity does not survive a
copy into another address space; a future, a closure or a native handle is
the same kind of thing, and a `spawn` given one fails at once with
`spawn cannot transfer …`, before any process starts. A worker that
needs a file gets the path, not the open `File`. It opens the file itself,
with the authority it was started with.

**A failure comes back as text.** If the action fails, its failure is
written into the reply as a description, and `await` of the future raises a
runtime failure with that text. The failure's object stays in the worker.
`catch e is ParseError` in the parent does not match a `ParseError` thrown
in a worker. This is the same rule seen from the other side: the error
object is an identity too. A worker that should report several kinds of
outcome returns them as data, an enum of results, which does cross.

**Cancellation kills the worker.** Unlike an async task, a synchronous worker has no suspension points for cooperative cancellation. Cancelling its future kills it; the future settles as `Cancelled` only after the worker has ended. A worker cancelled before starting is never started. Its `defer`s and `destroy`s do not run. If cleanup matters, pass it a deadline and let it finish cooperatively instead.

**Output is passed on whole.** What the action prints reaches the parent's
standard output and error when the worker reports back, before its result,
so the output of one worker is never interleaved with another's line by
line. A killed worker reports nothing, and its output is dropped with it.

The narrowness is deliberate, and other languages show what the wider
designs cost. Python's `multiprocessing` pickles arbitrary objects across the
boundary, which makes an object in the child a
look-alike of one in the parent: equal-looking, separately mutable, and
silently out of date. Erlang processes share nothing and exchange only
values, as Okkama's workers do, but they live inside one runtime and are
cheap enough to create by the million [@armstrong2003]; an Okkama worker is
an operating-system process, costs what one costs to start, and is meant for
CPU-bound or blocking work, not for modelling every concurrent activity.
Concurrency inside a run is Chapter 20's tasks; workers are for using
another core.

### 21.5.2 The program's own streams

The streams a program is born with are handles too, and two of their
defaults changed in 0.5.5.

Standard output is buffered (issue #132). A terminal receives complete lines; pipes and files receive blocks of 64 KiB. `Stdout().flush()` writes pending output. Pending output is also flushed before writing standard error, reading terminal input, starting a child process, writing to the terminal and ending the program. Use `flush()` when a reader must see a partial line promptly.

Standard input is read by lines (issue #221):

```okkama
for Stdin() -> line {
    if line.starts_with("#") { next; }
    Stdout().writeln(line.upper());
}
```

A line loses its LF and the one CR before it. Invalid UTF-8 fails by default; `Stdin(decoding: InputDecoding.lenient)` replaces bad sequences with U+FFFD. `Stdin().chunks(size: 65536)` reads bytes. All `Stdin()` values share one input position, so a program may read a header by lines and then the remaining bytes without losing intervening data.

## 21.6 What an embedder can deny

A program run by `okkama run` may use every native. A host running plug-ins or user scripts may restrict the native groups available to a run:

| group | what it reaches |
|---|---|
| `Fs` | files, directories, file snapshots, SQLite databases |
| `Net` | sockets, TLS, PostgreSQL |
| `Process` | child processes, and the process workers of Chapter 20 |
| `Env` | environment variables, users and groups, host facts, the local time zone |
| `Clock` | wall and monotonic clocks, sleeping |
| `Random` | secure random bytes |
| `Terminal` | the controlling terminal and standard input |

Every native of `runtime/*` belongs to one group or to none. The ones in
none compute without reaching the host — UTF-8, hashing, number parsing —
or write to standard output and standard error, and they are always
allowed. A call to a native of a group the host did not allow fails the
program at that call: `Native runtime/time@1::wall_clock/0 needs the Clock
permission`. Because every
effect passes through some native (Section 21.1), the list of groups is
the whole list of what the program can do outside itself; an embedder that
allows only `Clock` has a program that can read the time and print, and
nothing else.

A host may also set step, memory and output budgets and interrupt a run. Exhaustion or interruption stops it with an uncatchable report (Chapter 10); cleanup does not run after a budget stop.

A host restricting native groups refuses unclassified natives too. Finer authority, such as access only below one directory, is expressed by passing a `FileRoot` (21.4).

## 21.7 The command line keeps the user's authority

From the command line Okkama does not sandbox anything. A program has the
authority of the user who runs it, as a Python, Perl or Go program does,
and the security model says so plainly. The native groups are an
embedding feature; there is no `--allow-read` flag.

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
and each native must respect the host’s policy.

## 21.8 Limits

- **Authority is a library discipline.** `DirectoryLocating`, `FileRoot`
  and `Command` make authority explicit, but `runtime/fs` and the other
  substrate modules can be imported directly. Nothing in the language stops
  a module from reaching a native its parameters did not grant.
- **No per-call permissions.** A native group is allowed or not for the
  whole run; there is no "this directory only" at the embedding level. A
  host that needs it gives the program a `FileRoot` instead.
- **Process workers need `Process`.** A host that denies child processes
  also denies the `spawn` of a synchronous action (Chapter 20.4).
- **Natives are looked up at the call.** A program that declares a native
  nobody provides starts and fails only when it calls it (Section 21.1).

## 21.9 What this design costs

The narrow native boundary costs copies when large strings or byte arrays cross by value. Explicit authority costs parameters: code reading a file takes an object that names where it may read. The command line’s full authority leaves isolation to the host.

The design gives a short answer to “what can this program do”: the natives its imports reach, filtered by the groups its host allows.
