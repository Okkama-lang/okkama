# Chapter 21. Processes, natives and capabilities

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

An Okkama program cannot open a file by itself. `io/stdout`, `fs/file`,
`net/tcp`, `time/clock` and the rest of the library that touches the host
are written in Okkama, and each of them ends, a few calls down, in a
declaration such as this one from `runtime/time`:

```okkama
native monotonic_clock() is int;
```

A native declaration has a signature and no body. It is a request: when the
program calls `monotonic_clock()`, the engine looks up a provider registered
under that module and name and runs it. The provider is Rust on the IR
engine and on Sparrow, Java on the JVM, and the three are tested to give the
same results (Chapter 26). The `runtime/*` modules are the only place in the
standard library where natives are declared; everything above them is
ordinary Okkama that can be read, tested and faked.

Any module may declare a native. Declaring one does not create it:

{{example:21-unregistered}}

The program compiles and starts. `before the call` is printed; the call
fails with `RuntimeError: Native symbol "bible/unregistered@1::vault_open/1"
is not registered`, and the run ends with status 1. A native's name is its
module, the module's major version, its own name and its arity, and only a
provider registered under exactly that name answers it. A program cannot
reach the host by inventing a declaration: the set of doors is the set the
engine was built with, plus whatever an embedder adds (Section 21.7).

The rule has a cost that the next sections pay off. Every effect a program
can have outside itself has a name in `runtime/*`, so the list of what a
program might do is short and fixed: it is the list of natives its imports
reach. That list is what the native groups of Section 21.6 restrict.

## 21.2 What a native may take and return

The native ABI is deliberately narrow. A native takes and returns
`int`, `float`, `bool`, `string`, arrays and maps of such values (`@byte`,
`@string`, `%string:int`) and *native objects*. It takes no objects of the
program, no closures and no `any`, so it cannot call back into Okkama. A
native declaration has no body, no defaults and no variadic parameters.
What crosses is data, copied or encoded; what comes back is data or a
handle.

A native may be `async`. `runtime/network` declares thirteen of them, among
them DNS lookups and socket reads and writes:

```okkama
native async dns_addresses(host is string, port is int) is @@string;
native async udp_receive(origin is SocketState, limit is int) is @byte;
```

A call returns a future like any async action. The engine starts the host
operation without blocking the scheduler, other tasks run meanwhile, and
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
stands behind it — a file descriptor, a socket, a Java channel — never
becomes a value the program can see.

The provider keeps every handle it issued in a table, with a generation and
an open or closed state, and checks the table on every call:

{{example:21-handle}}

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

## 21.4 Authority is a value

A program run from the command line has the authority of the user who ran
it: it may read any file that user may read. Okkama does not change that
(Section 21.8). What std does is make the authority *visible*. Wherever a
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

The design leaves out what the shell gives for free and what most process
APIs inherit by default: globbing, word splitting, redirection syntax, an
implicit environment. Each of them is a way for data to become code or for
one part of a program to act with another part's settings. A command
built from values carries exactly what it was given.

## 21.6 What an embedder can deny

A program run by `okkama run` may use every native. A program run by a
host — an application that embeds `sparrow-core` to run plug-ins, or a
service that runs users' scripts — usually should not. The embedding entry
point takes `budget::Limits`, and `Limits::natives` lists the *native
groups* the program may use:

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

The same `Limits` carries the budgets of the security model: steps, memory,
output and an interrupt flag the host may raise from any thread. Running
out stops the run with its own report, which no `catch` intercepts
(Chapter 10). Since 0.5.1 the memory budget can measure one runtime
instead of the whole process (`MemoryScope::Runtime`, issue #269): on
Sparrow it reads the runtime's own heap, so a host can run several
programs side by side, and the one that grows past its budget stops while
the others go on.

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
and the runtime modules declare 653 of them.

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
