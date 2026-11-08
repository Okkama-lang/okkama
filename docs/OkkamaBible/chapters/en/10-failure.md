# Chapter 10. Failure: throw, panic, try/catch, causes, confirm

Every program meets two kinds of trouble, and they have little in common.

- **The world says no.** A file is missing, a server refuses the connection,
  a user types `sixty` where a number belongs, an account would go below
  zero. Nothing is wrong with the program: this is an outcome it must
  expect, and someone — the caller, or the caller's caller — knows what to
  do about it.
- **The program is wrong.** An index runs past the end of an array, an
  invariant the author was sure of turns out false, a divisor is zero where
  the code assumed it never would be. No caller can repair this, because
  the caller cannot know which assumption broke. The useful response is to
  stop, clean up, and report precisely where.

Okkama gives the two kinds two channels and does not let them mix. Expected
failures are **values**: cases of an *error enum*, thrown with `throw` and
caught with a typed `catch`. Bugs are **not values**: a `panic`, a failed
bare `confirm` or a runtime error ends the run after cleanup, and no
`catch` clause can see it. Everything in this chapter follows from that
split.

The program below imports a tiny ledger. Each line is `account,amount`. The
import can fail in four ways the caller cares about: a blank line, a line
of the wrong shape, an amount that is not a number, and an overdraft. Each
is a case of an error enum with the context in its payload. `report` turns
each failure into a sentence, and `try?` turns one into a fallback value.

{{example:10-failure}}

The rest of the chapter takes the pieces apart:

- the two channels, and why the history of error handling points at them;
- error enums and `throw`;
- `try`/`catch` and its deliberate omissions;
- causes, `confirm`, `try?` and `try!`;
- failures from the host;
- what reaches the top of the program.

Cleanup — `defer`, `destroy`, regions — runs on every one of these paths;
Chapter 11 describes it.

## 10.1 Who should hear about a failure

Before choosing a mechanism, it helps to sort failures by who can act on
them.

| Kind | Example | Who can act | Okkama |
|---|---|---|---|
| Expected failure | file not found, bad input, refused connection | the caller | error enum, `throw`, typed `catch` |
| Bug | index out of bounds, broken invariant | the developer, after reading the report | `panic`, `confirm`, runtime error: uncatchable |
| Exhaustion | step, memory or time budget spent | whoever started the run | budget stop: uncatchable, not a failure of the program |
| Cancellation | the task that awaited this work went away | the scheduler | `Cancelled` (Chapter 20) |

The first row is the only one where code *inside* the program can do
better than stop. A mechanism that treats all four rows alike — a single
`catch (Exception e)` that sees an out-of-bounds index and a missing file in
the same variable — invites code that "handles" a bug by logging it and
carrying on with corrupted state. Okkama's split is the table above turned
into syntax.

## 10.2 A short history: codes, exceptions, values

Error handling has been reinvented roughly once a decade, and each
reinvention fixed a real problem of the previous one.

**Return codes.** C reports failure through the return value and a global
`errno`. It costs nothing at run time, and nothing forces the caller to
look. Unix's `close` returns an error that almost no program ever checks,
and a full disk discovered on `close` is silently lost. The failure and the
normal result also share one channel: `-1` is both "error" and, for some
functions, a legitimate value.

**Exceptions.** John Goodenough's 1975 paper laid out the design space still
in use: how a failure is raised, how a handler is found, and whether control
*resumes* after the handler or *terminates* the failed operation
[@goodenough1975]. CLU, in 1979, chose termination and made each procedure
declare the exceptions it can signal; an undeclared one becomes a `failure`
[@liskov1979]. That pair of choices — termination, and a distinction between
declared exceptions and everything else — is the ancestor of nearly every
modern design, Okkama's included.

**Exception hierarchies.** C++ and Java made exceptions objects of a class
hierarchy, caught by type [@stroustrup1994]. Java went further with *checked*
exceptions: a method must declare in its signature the checked exceptions it
may throw, and the compiler verifies that each one is caught or passed on
[@gosling2023]. Checked exceptions have been argued over ever since. The
usual complaint is that signatures fill with exception types that change with
the implementation, and callers wrap or swallow them to make the compiler
quiet. Java also kept a second, unchecked family, `RuntimeException` and
`Error`. That is CLU's distinction again, though a handler catching
`Exception` sees both.

**Errors as values.** Go returned to return values, but as a convention
rather than an accident: a function returns `(result, error)`, and the
caller checks `err != nil`. Rob Pike's "Errors are values" argues that an
error is data the program can compute with, not a special control path
[@pike2015]. The cost is repetition, and the old weakness of return codes:
nothing forces the check. Rust keeps the value approach but makes it
impossible to ignore: a fallible function returns `Result<T, E>`, which must
be unwrapped or propagated with `?`, and a separate mechanism, `panic!`, is
for bugs [@klabnik2023]. Two channels again.

**Let it crash.** Erlang drew the line most sharply. A process that meets a
condition it was not written for does not try to recover. It crashes, and a
supervisor that knows the larger picture restarts it in a known state
[@armstrong2003]. Defensive code that tries to handle every imaginable
failure locally is, in Armstrong's account, where reliability goes to die.

**Abandonment.** The design closest to Okkama's is the one Joe Duffy
describes for Midori, Microsoft's research operating system. Bugs — contract
violations, out-of-bounds indexing, arithmetic overflow — *abandon* the
whole process, with no chance for local code to intercept them.
Recoverable errors are declared in signatures and handled with a lightweight
`try` [@duffy2016]. Duffy's argument is that the two need different
mechanisms. Code that treats a bug as recoverable continues in a state
nobody designed for, and a model without that option is both simpler and
more reliable.

The empirical record outside research systems agrees. Yuan and colleagues
studied 198 user-reported failures of five widely used distributed systems
(Cassandra, HBase, HDFS, Hadoop MapReduce and Redis). In 92% of the
catastrophic ones, the last step was the incorrect handling of a non-fatal
error the code had explicitly signalled. Many of the faulty handlers were
empty, or did nothing but log, or contained a `TODO` [@yuan2014]. The lesson
is uncomfortable. Error handlers are code that rarely runs, so they are
rarely tested, so they are where the bugs live. A language cannot make
handlers correct, but it can make them fewer, typed and impossible to write
by accident.

Okkama takes from each step:

- From CLU and Midori: termination, and a hard line between declared
  failures and bugs.
- From Go and Rust: the declared failures are plain values.
- From Erlang: bugs are not handled locally at all.
- From Yuan et al.: no construct catches "anything".

## 10.3 Bugs are not values: panic and runtime errors

These end the run, whatever `catch` clauses surround them:

- `panic "message"` — the program's own declaration that something
  impossible happened. The message must be a literal string: a panic carries
  a fixed sentence, not data.
- A bare `confirm condition;` whose condition is false (§10.8).
- **Runtime errors.** An index or slice out of bounds, division or modulo by
  zero, a failed narrowing of a role or `any` value. Each is detected by the
  engine and reported as `RuntimeError: …`.

The run does not simply stop. It *unwinds*: every scope between the failure
and the top runs its `defer` blocks and destroys the resources it owns, in
reverse order (Chapter 11). The report then goes to standard error and the
process exits with status 1. Here is a withdrawal in which the program
throws an expected error for a zero amount, and panics on a case it
believes cannot happen:

{{example:10-panic}}

The second `try` has a `catch e is Error`, the widest typed clause there is,
and it still does not run. The report names the panic, its location and the
call stack:

```text
Panic: Withdrawal checked above, still overdrawn
  --> 10-panic.ok:14:9
   |
14 |         confirm amount <= balance else panic "Withdrawal checked above, still overdrawn";
   |         ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
Stack trace (most recent call first):
  at Account.withdraw (10-panic.ok:14:9)
  at main (10-panic.ok:24:23)
```

A runtime error looks the same, with `RuntimeError:` in place of `Panic:`.
An index past the end prints `RuntimeError: Array index out of bounds`, and
`7 % d` with `d` zero prints `RuntimeError: Modulo by zero`, each with the
offending expression underlined.

Why not let the program catch these? Because a caught bug is a hidden bug.
A handler that catches an out-of-bounds index has no way to know which
assumption failed or what state the half-finished operation left behind. In
practice such a handler logs the error and lets the program carry on with
that state. That is the pattern Yuan et al. found at the root of
catastrophic failures. Stopping preserves the evidence. A long-running
service that must survive its own bugs does what Erlang does: it runs the
risky work in a separate unit (a task or a worker process) and lets the
supervising code react to the unit's death, not to the bug inside it.

## 10.4 Error enums

An expected failure is a value, and its type is an **error enum**: an enum
declared `is Error`. Its cases are the ways the component can fail, and
their payloads carry the context.

```okkama
enum LineError is Error = (
    blank(line is int),
    shape(line is int, text is string),
    amount(line is int, cause is InvalidInteger)
);
enum LedgerError is Error = (line(cause is LineError), overdraft(account is string, short is int));
```

The rules are short.

- **`is Error` is the only role an enum may declare.** An error enum is not
  generic, and it is always a *variant* enum. A `match` over it must cover
  every case or have `else`, even when no case has a payload.
- **Payloads are values.** A payload field may hold `string`, `int`, `byte`,
  `float`, `rat`, `bool` and enums whose payloads follow the same rule. That
  includes other error enums, which is how one failure carries another
  (`LedgerError.line(cause)`). Objects, roles, collections, `char`, actions
  and `any` are rejected. An error must survive the scope that raised it,
  and its owner's resources must not, so an error cannot hold a resource.
- **`text()` is generated.** It spells the case the way the source would:
  `LineError.blank(2)`, `LineError.amount(2, InvalidInteger.source("sixty"))`.
  Strings are quoted, numbers use `string()`, booleans are `true`/`false`,
  and nested enums are written out recursively. The report of an uncaught
  error starts with it.

The role `Error` is built in and requires a public `text() is string`. Three
kinds of value do it:

- error enums;
- `NativeFailure`, a host failure (§10.10);
- `Cancelled`, a cancelled task (Chapter 20).

An object cannot. `object Fault does Error` is a compile error, and the
message names the fix:
``Object "Fault" cannot do Error: errors are enums, declare `enum Fault is Error = (...)` ``.

That rule is the most visible difference from Java or C#, and it is
deliberate. In those languages an exception is an object of a class
hierarchy. Callers catch a superclass to mean "any of these", the hierarchy
grows with the implementation, and fields are added to carry whatever
someone needed once. An error enum is a closed list, visible in one
declaration. Adding a case is a change every exhaustive `match` notices.
Grouping is explicit, because a component that wraps another component's
failures says so with a payload, not with a superclass. And since the value
is data, two errors compare by value and print the same on every engine.

One enum per component is the intended granularity. `LineError` is
everything the line parser can report, and `LedgerError` everything the
import can report, including "a line was bad" as a case that carries the
`LineError`.

## 10.5 throw

`throw` raises an expected failure. It takes an error enum value, an
`Error` reference — usually a caught error being passed on — or a
`NativeFailure`:

```okkama
throw LineError.blank(number);
throw LedgerError.line(e);        // a new error carrying the caught one
throw e;                          // pass on a caught error
throw;                            // rethrow the error being handled
```

Bare `throw;` exists only inside a `catch` body, and the compiler checks
that lexically. An action called from a `catch` body cannot rethrow
implicitly, because it cannot know what is being handled. Bare `throw;`
preserves the thrown value's identity and origin: the report still points
at the place where the error was first raised, not at the rethrow.

`throw` never produces a value, like `return`, so a `catch` body that throws
needs no value of its own. The parser in the opening example relies on
that:

```okkama
let amount = try { ParsedInt(parts[1]).value(); }
             catch e is InvalidInteger { throw LineError.amount(number, e); };
```

The `try` yields an `int`. The `catch` body never yields, because it throws.

`confirm condition else throw value;` (§10.8) is the guarded form, and it is
how most errors are raised in practice: a check, and the failure if the
check does not hold.

## 10.6 try and catch

`try` evaluates a block and, if the block throws, selects a `catch` clause
by the type of what was thrown:

```okkama
let message = try {
    "balance " + Import(lines: lines).run(start).string();
}
catch LedgerError.overdraft(account, short) {
    "overdraft on " + account + " by " + short.string();
}
catch e is LedgerError {
    "rejected: " + e.text();
};
```

The rules:

- **`try` is an expression and a statement.** Like `if`, it yields the value
  of the block that ran, the `try` body or the selected `catch` body. It can
  also stand alone as a statement.
- **At least one typed clause.** Each clause names its types, and the types
  must guarantee `Error`: `Error` itself, an error enum, `NativeFailure` or
  `Cancelled`. A clause can name several types, `catch e is DbError, NetError
  { … }`, and the binding then has the type `Error`.
- **Patterns instead of a binding.** A clause may list error enum cases with
  their payloads bound positionally, `_` for an ignored field:
  `catch LedgerError.overdraft(account, short) { … }`. A case of the right
  enum that does not match the pattern goes on to the next clause of the
  same `try`. Patterns are one level deep. `catch LedgerError.line(LineError.shape(line, text))`
  is rejected with `Expected a binding or _ in variant pattern`, so a nested
  cause is taken apart with a `match` in the clause body, as `report` does in
  the opening example.
- **Source order.** Clauses are tried top to bottom, and the first that fits
  wins. A failure no clause fits leaves the `try` unchanged, with its type,
  value and origin.
- **Separate scopes.** The `try` body and each `catch` body are separate
  child scopes. Names bound in the body are not visible in a clause, and the
  binding of a clause exists only in that clause.
- **Cleanup comes first.** Before a clause is selected, the `try` body's
  scope is closed: its `defer` blocks run and the resources it owns are
  destroyed, newest first. Only the thrown error's own payload survives into
  the clause. A failure during that cleanup does not replace the body's
  failure; it is attached to it (§10.7).

What `try` does not have is as important as what it has.

**No catch-all.** There is no `catch e { }` and no `catch (...)`. The
narrowest way to catch everything expected is `catch e is Error`. Even that
clause sees only thrown errors, never a panic, a runtime error or a budget
stop, and a reader sees in the source that it is the widest net. A
lowercase name alone does not compile:

{{example:10-catchall-rejected}}

**No `finally`.** Work that must happen whether the block succeeds or fails
is a `defer` in that block (Chapter 11). A `finally` clause would be a second
spelling of the same idea, and it would run in a different scope from the
code it cleans up after.

**No guards.** A clause cannot add a condition (`catch e is DbError if
e.retryable()`). The condition belongs in a case of the error enum, where
the thrower decides it once, not in each handler.

## 10.7 Causes

A failure often has a reason that is itself a failure. A storage error makes
an order fail, and a parse error makes a line fail. Okkama keeps the reason
in two ways.

**As a payload.** When the cause is part of what the caller needs to
decide, put it in the error:
`enum OrderError is Error = (storage(cause is StoreError), …)`. The cause is
then an ordinary value. The caller can match on it, `text()` prints it, and
it means the same on every engine.

{{example:10-cause}}

**As a runtime link.** Every new error raised while a `catch` body is
handling another one keeps the handled error as its *cause*, with its value,
origin and stack. This happens whether or not the new error mentions the old
one, so a `catch` that throws a less detailed error does not lose the
evidence. It also holds after an `await`. The link is diagnostic: the
program cannot read it, but the report of an uncaught failure prints it. On
the Sparrow core:

```text
OrderError: OrderError.failed(7)
Caused by:
  StoreError: StoreError.closed
  --> uncaught.ok:10:56
```

A `catch` body that completes normally leaves no link behind, so a later,
unrelated throw is not blamed on an old failure. Bare `throw;` keeps the
original carrier instead of creating a new one.

The third kind of extra failure is a **secondary**. When cleanup itself
fails while a scope unwinds from a failure, the cleanup failure does not
replace the original; it is attached to it as a secondary and reported
after it. The first failure is usually the one worth reading, and cleanup
code should not be able to hide it.

## 10.8 confirm: the contract inside the code

`confirm` states a condition the code relies on. It is Okkama's form of the
assertions of Design by Contract, in which preconditions, postconditions and
invariants are part of the program's text, not of its comments [@meyer1992].
There are three forms, one per failure channel:

```okkama
confirm items.size() == 3;                            // a bug if false
confirm amount > 0 else throw Rejected.rejected;      // an expected failure
confirm amount <= balance else panic "Overdrawn";     // a bug, with a sentence
```

- **Bare `confirm`** is an internal invariant. If it fails, the run stops
  with `RuntimeError: confirm failed` and the location of the `confirm`.
- **`confirm … else throw`** is a precondition of a public action, checked
  against input the caller supplied. A false condition throws the given
  error, and the caller may catch it. Most errors in the standard library
  are raised this way. `ParsedInt` starts with
  `confirm self.source.length() > 0 && self.source != "-" else throw InvalidInteger.source(self.source);`.
- **`confirm … else panic`** is an invariant that deserves a sentence in the
  report.

The condition must be a `bool`, and there is no way to switch `confirm` off.
A contract that disappears in production is a contract nobody tests, and a
`confirm` costs one evaluation of its condition.

## 10.9 try? and try!

Two prefix forms adapt a fallible expression to code that wants to treat
failure as a value, or as impossible:

{{example:10-marks}}

- **`try? value`** has the type `Maybe<T>`. It is `Maybe.some(value)`, or
  `.none` if evaluating it throws any error. It combines with `else`, which
  supplies a fallback, and with the two-way choice `?? !!` of Chapter 7. A
  void call cannot be wrapped, since there is no value to put in the
  `Maybe`, and neither can a successor result, since its must-use obligation
  would be lost.
- **`try! value`** keeps the type of the value and turns a thrown error into
  a panic, `Unexpected error: <text()>`, located at the `try!`. It says "this
  cannot fail here". When the author is wrong, the run stops with the
  original error in the report, not with a silently wrong value.

Both forms see thrown errors only. Panics and runtime errors pass through
them, and cleanup runs as for `try`. `try?` discards the error, so it is for
the cases where *why* something failed does not matter, such as a fallback
port or an optional setting. Where the reason matters, the error stays an
error.

## 10.10 Failures from the host

Code that reaches outside the program — files, sockets, processes,
databases, cryptography — gets the host's failures as values too. A native
provider reports a failure as a `NativeFailure` with a stable `domain`
(`core.fs`, `core.crypto`, …), a stable `code` (`not_found`,
`already_closed`, `invalid_input`, `permission_denied`, …), the canonical
`operation` that failed and a diagnostic `message`:

```okkama
try { open(path, 1, 0); }
catch failure is NativeFailure {
    failure.domain();     // "core.fs"
    failure.code();       // "not_found"
    failure.message();    // for people, never for branching
}
```

Code should branch on `domain` and `code` only. Raw `errno` values, host
exception classes and localized OS messages never become language values,
so a program behaves the same on macOS, Linux and the JVM.

Most programs never see a `NativeFailure` directly. The standard library
catches it at the boundary and rethrows a typed error that belongs to the
module's own vocabulary. `crypto/error` turns a `NativeFailure` with
`invalid_input` into `InvalidCryptoInput.because(message)`, `permission_denied`
into `AuthenticationFailed.failed`, and anything else into
`CryptoUnavailable`. A program that uses `crypto/kdf` catches the crypto
errors, not the host's codes.

An *unexpected* host error, one the provider has no stable code for, is not
a `NativeFailure`. It is a runtime error and ends the run.

`Cancelled` is the failure a task sees when the work it awaited was
cancelled. Chapter 20 explains it with structured concurrency.

## 10.11 A boundary in practice

The pieces meet at module boundaries. Below is a settings loader. Reading the
file can fail in the file system's terms, and parsing can fail in the
loader's own terms. The caller should see only the loader's terms.

{{example:10-config}}

Three things happen at the boundary.

1. `fs/file` already caught the host's `NativeFailure` and rethrew it as
   `FileSystemFailure.detailed(code, message)`. The loader never touches a
   host code directly.
2. The loader translates once, in one place. A missing file becomes
   `ConfigError.missing(path)`, the case a caller is likely to handle, by
   falling back to defaults. Any other file-system failure becomes
   `ConfigError.unreadable(path, code)`, which keeps the stable code and
   drops the host's message.
3. The caller's `describe` names the cases it handles and lets
   `catch e is ConfigError` print the rest. Nothing outside the loader
   mentions `FileSystemFailure`. If the loader one day reads settings from a
   database, its callers do not change.

The `confirm … else throw` inside the `catch` body reads as a sentence:
the code must not be `not_found`, or the file is missing. A `catch` body is
ordinary code, and the guarded throw is as natural there as anywhere.

## 10.12 Budgets are not failures

A run can be given budgets: `--max-steps`, `--max-memory`, `--max-output`
and `--timeout`. Running out of one is neither an expected failure nor a
bug in the program. It is the environment withdrawing permission to
continue. The run ends with `Execution budget exhausted: steps` (`memory`,
`output`) or `Execution interrupted` and exit status 1. No `catch` sees it,
not even `catch e is Error`, and `destroy` does not run, because the
budget would be spent again by the cleanup.

The stop happens at the next point where the engine looks at the budget:

- every step, function entry and loop iteration;
- inside a long sort;
- between the rounds of a PBKDF2 derivation.

A native that cannot look, such as an Argon2 hash or a blocking read, ends
when it ends. With `--timeout`, the command line then stops the run at most a
second after the deadline.

## 10.13 Failures across tasks

`await` passes a task's failure to the awaiting code unchanged: same type,
same value, same origin. The awaiting code catches it like any other:

{{example:10-tasks}}

Two rules make concurrent failure as strict as sequential failure.

- **A failure is never lost.** If a spawned task fails and no one awaits
  it, the run itself fails: the report is printed and the process exits
  with status 1, even when `main` returned 0. A forgotten task cannot hide
  an error. This is a deliberate choice, and the conformance suite pins it
  (`ASYNC.UNOBSERVED-FAILURE`).
- **Cancellation is a failure value of its own.** When the work a task
  awaited is cancelled, the task sees `Cancelled`, which does `Error` and
  can be caught. Structured await (Chapter 20) guarantees that the task
  resumes only after the cancelled work has finished its cleanup.

Within one run, a bug in any task ends the run, as it does in sequential
code. A program that must outlive its own bugs needs a boundary a bug cannot
cross, which is Erlang's lesson again. In Okkama that boundary is a separate
process, supervised from outside (Chapter 21).

## 10.14 What reaches the top

An error that no `catch` handles unwinds the whole run like a panic: cleanup
runs scope by scope, and the process exits with status 1. The report
starts with the error's type and `text()`, then gives the location of the
`throw`, the cause chain and the call stack:

```text
OrderError: OrderError.failed(7)
Caused by:
  StoreError: StoreError.closed
  --> uncaught.ok:10:56
   |
10 |         try { self.save(id); } catch e is StoreError { throw OrderError.failed(id); }
   |                                                        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^
Stack trace (most recent call first):
  at Orders.place (uncaught.ok:10:56)
  at main (uncaught.ok:14:17)
```

`main` returns the exit status of a successful run, and it must be in
0–255. A larger value is reported as `RuntimeError: main() must return an
int in 0..255, got N`, so the success channel and the failure channel stay
apart here too.

## 10.15 Designing an error vocabulary

A module's error enums are part of its public API, as much as its actions.
The standard library follows a handful of rules, and they work outside it
too.

- **One enum per kind of failure the caller tells apart.** `strings/parse`
  has `InvalidInteger`. `crypto/error` has `InvalidCryptoInput`,
  `AuthenticationFailed` and `CryptoUnavailable`, because a caller reacts
  differently to bad input, a forged message and a missing algorithm.
- **Name the case after what happened, not after who noticed.**
  `ConfigError.missing(path)`, not `ConfigError.file_system_error`. The
  caller decides from what happened. Who noticed is in the stack trace.
- **Put in the payload what the caller needs to act, and nothing else.**
  A line number for a syntax error, the missing amount for an overdraft.
  The host's message goes into a diagnostic payload at most. It is not
  stable across platforms, and code must never branch on it.
- **Translate at the boundary, once.** A module catches the failures of
  the modules it uses and rethrows them in its own terms, as the settings
  loader does. Failures that pass through many layers unchanged tie every
  caller to the bottom layer.
- **Wrap when the caller needs the cause, pass on when it does not.**
  `LedgerError.line(cause)` wraps because the caller may want to know which
  line failed and how. `throw e;` passes on when the module has nothing to
  add.
- **Do not use errors for control flow.** A lookup that may find nothing
  returns `Maybe` (Chapter 7), not an error. An error says the operation
  could not do what it promised. A missing optional value is not that.

## 10.16 Testing failures

A failure is part of an action's contract, so it gets tests like the
successful path. The test runner (`okkama run -t`) provides
`test.throws(Type) { … }`, which passes when the block throws a value of
that type and fails otherwise. When the payload matters, a `try` with a
pattern says exactly which case is expected:

```okkama
test "an overdraft is refused with the missing amount" {
    test.throws(LedgerError) { Account(balance: 50).withdraw(80); };
    let short = try { Account(balance: 50).withdraw(80).left(); }
                catch LedgerError.overdraft(missing) { missing; };
    test.is(short, 30);
}
```

A panic or a runtime error inside a test fails that test, not the whole
run: the runner isolates each test, reports `--- FAIL` with the message, and
goes on to the next one.

Error handlers are rarely exercised in production, and that is where
Yuan et al. found the catastrophic bugs. A failure path that a test has
never run is a guess. Chapter 15 returns to tests, fakes and properties.

## 10.17 Okkama among other languages

| Language | Expected failure | Bug | Forced to handle? | Catch everything | Cleanup |
|---|---|---|---|---|---|
| Java | checked exception (declared) | `RuntimeException`, `Error` | checked ones, by the compiler | `catch (Throwable t)` | `finally`, try-with-resources |
| Kotlin | exception (all unchecked); `Result` in the library | exception | no | `catch (e: Throwable)` | `finally`, `use { }` |
| Go | `error` value | `panic` | no, by convention | `recover()` in a deferred call | `defer` |
| Rust | `Result<T, E>` | `panic!` | warned if ignored (`#[must_use]`) | `catch_unwind`, discouraged | `Drop` |
| Haskell | `Either`, `Maybe`; exceptions in `IO` | `error`, exceptions | by the type, for `Either` | `catch` on `SomeException` | `bracket` |
| Okkama | error enum value, thrown | `panic`, `confirm`, runtime error | no | none (`catch e is Error` sees thrown errors only) | `defer`, `destroy` |

Two columns set Okkama apart. In the bug column it is the only language in
the table where a bug cannot be caught at all; Rust comes closest. In the
catch-everything column it is the only one where no construct catches
everything. Okkama is weaker in the "forced to handle" column: nothing makes
a caller deal with an error, and the compiler does not know which errors an
action can throw.

## 10.18 What this design costs

The two channels are a strong opinion, and opinions cost something.

- **More declarations.** Every component that can fail declares an error
  enum. A Java programmer reaches for `IllegalArgumentException`. An Okkama
  programmer writes a case.
- **No catch-all.** Code that really means "any expected failure" writes
  `catch e is Error`, and code that wants to survive bugs has to put the
  risky work in a separate task or process. That is more structure than a
  `catch (Throwable t)`. Okkama counts it as a feature; a programmer who
  needs a quick script may not.
- **Signatures do not say what an action throws.** CLU and Java's checked
  exceptions put the failures in the signature, and Midori did too. Okkama
  does not: the caller learns from the documentation and the source which
  error enums an action may throw, and the compiler does not check that a
  caller handles them. Declaring them would bring back the churn checked
  exceptions are known for; not declaring them leaves a gap a reader must
  fill.
- **Errors cannot carry objects.** An error that would naturally hold the
  request that failed must hold the request's identifying values instead.
- **Patterns are one level deep.** A nested cause is taken apart with a
  `match` in the clause body.
- **The runtime cause chain is for reports only.** A program cannot walk
  it. A cause the program must inspect has to be a payload.
- **Engines still disagree on the report.** The Sparrow core prints the
  cause chain. The IR engine leaves out `Caused by:`, and the JVM prints
  only the first line ([#270](https://github.com/shootnix/okkama/issues/270)).
  Exit status and `catch` behaviour are the same on all three.
- **No named handlers yet.** A handler shared by several `try` blocks has to
  be an action called from each clause. Named catch handlers were proposed
  and postponed as too magical for 0.4.1
  ([#148](https://github.com/shootnix/okkama/issues/148)).

Against these costs stands what the design removes:

- handlers that swallow bugs;
- exception hierarchies that grow with the implementation;
- `finally` blocks that clean up in the wrong scope;
- error messages that differ between platforms;
- the guess, at every `catch`, about what might arrive there.

The next chapter takes up the other half of the story: what runs when a
scope ends, on success or failure.
