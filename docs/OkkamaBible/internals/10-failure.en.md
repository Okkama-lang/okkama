# Chapter 10 — archived implementation passages

Replaced mixed paragraphs are preserved in full for context.

## Chapter 10. Failure: throw, panic, try/catch, causes, confirm

described 0.5.x

Cleanup — `defer`, `destroy`, regions — runs on every one of these paths;
Chapter 11 describes it.

## 10.4 Bugs are not values: panic and runtime errors

described 0.5.x

- `panic "message"` — the program's own declaration that something
  impossible happened. The message must be a literal string: a panic carries
  a fixed sentence, not data.
- A bare `confirm condition;` whose condition is false (§10.9).
- **Runtime errors.** An index or slice out of bounds, division or modulo by
  zero, a failed narrowing of a role or `any` value. Each is detected by the
  engine and reported as `RuntimeError: …`.

## 10.5 Error enums

described 0.5.x

That rule is the most visible difference from Java or C#, and it is
deliberate. In those languages an exception is an object of a class
hierarchy. Callers catch a superclass to mean "any of these", the hierarchy
grows with the implementation, and fields are added to carry whatever
someone needed once. An error enum is a closed list, visible in one
declaration. Adding a case is a change every exhaustive `match` notices.
Grouping is explicit, because a component that wraps another component's
failures says so with a payload, not with a superclass. And since the value
is data, two errors compare by value and print the same on every engine.

## 10.8 Causes

described 0.5.x

**As a payload.** When the cause is part of what the caller needs to
decide, put it in the error:
`enum OrderError is Error = (storage(cause is StoreError), …)`. The cause is
then an ordinary value. The caller can match on it, `text()` prints it, and
it means the same on every engine.

## 10.8 Causes

described 0.5.x

**As a runtime link.** Every new error raised while a `catch` body is
handling another one keeps the handled error as its *cause*, with its value,
origin and stack. This happens whether or not the new error mentions the old
one, so a `catch` that throws a less detailed error does not lose the
evidence. It also holds after an `await`. The link is diagnostic: the
program cannot read it, but the report of an uncaught failure prints it. On
Sparrow:

## 10.11 Failures from the host

described 0.5.x

Code should branch on `domain` and `code` only. Raw `errno` values, host
exception classes and localized OS messages never become language values,
so a program behaves the same on macOS, Linux and the JVM.

## 10.13 Budgets are not failures

described 0.5.x

The stop happens at the next point where the engine looks at the budget:

## 10.16 How the engines carry a failure

described 0.5.x

## 10.16 How the engines carry a failure

The two channels exist in the compiled program, not only in the source. In
OIR, the compiler's intermediate form (Chapter 23), every basic block may
name an *unwind target*: the block that receives a failure raised by any of
its instructions, as its last parameter. Here is the end of a small method
that owns a resource region:

```text
^b1 unwind ^b4:
    region.adopt %1, %0
    br ^b2
...
^b4(%6: failure):
    %7: failure = region.unwind %1, %6
    raise %7
```

`^b4` is the landing block. It closes the method's region on the failure
path, which runs `defer` blocks and destroys what the region owns, and
passes the failure on with `raise`. A `try` lowers the same way: the body's
blocks unwind to a landing block that closes the body's scopes and then
tests the `catch` types in order. A panic travels the same edges, but no
clause accepts it.

Each engine carries the failure along those edges in its own way.

- **The IR engine** interprets the blocks. A failure is the error branch of
  a Rust result, which travels up through the interpreter's frames.
- **Sparrow** compiles to machine code. A failing helper sets the
  `status` and `failure` fields of the run's context, and every caller
  checks `status` after each call or helper and branches to its landing
  block. A failure thus crosses machine frames without Rust unwinding and
  without `setjmp`. A `try` that does not throw costs nothing beyond those
  checks.
- **The JVM** throws a Java exception, `okkama.rt.Failure`, a
  `RuntimeException` that keeps the JVM's stack from where it was created.
  Each method of the program numbers its source places in its
  LineNumberTable, and a table next to the classes, `okkama-debug.tsv`,
  maps those numbers back to files, lines and columns, so the uncaught
  report shows the Okkama stack and never Java frames. Remembering the stack
  is the price of a throw: a loop that throws 500 000 failures twelve calls
  deep takes 0.48 s instead of 0.27 s, the same as Sparrow.
  Regions close in Java `catch` blocks.

The conformance suite runs the same programs on all three and requires the
same output, exit status and `catch` behaviour. Since 0.4.2 the uncaught
report is the same too, byte for byte: the failure, its secondary cleanup
failures and its causes, the place and the stack (#270). Chapters 24 and 25
return to the engines.

## 10.20 What this design costs

described 0.5.x

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
- **The JVM keeps at most 1 024 frames of a stack.** A failure deeper than
  that reports the most recent frames and says that older ones are missing;
  the other engines show the whole stack, compressed
  ([#270](https://github.com/shootnix/okkama/issues/270)).
- **No named handlers yet.** A handler shared by several `try` blocks has to
  be an action called from each clause. Named catch handlers were proposed
  and postponed as too magical for 0.4.1
  ([#148](https://github.com/shootnix/okkama/issues/148)).

## 10.17 Designing an error vocabulary

described 0.5.x

## 10.17 Designing an error vocabulary

## 10.18 Testing failures

described 0.5.x

## 10.18 Testing failures

## 10.19 Okkama among other languages

described 0.5.x

## 10.19 Okkama among other languages
