# Chapter 15. Fakes, tests and properties

A language that forbids mutation, setters, downcasts and inheritance also
forbids most of the tricks that testing frameworks are built on. There is
no monkey-patching, no mocking library that rewrites a class at run time,
and no way to reach into an object and set a field. Okkama's answer is
not a framework. It is three pieces of the language and a small library:

- tests are declared next to the code they test;
- every role can carry its own **fake**, a working implementation for tests;
- assertions are a binding that exists only while a test runs;
- `testing/property` checks a claim against generated inputs and shrinks
  the first counterexample.

{{example:15-library-tests}}

## 15.1 Tests live next to the code

```okkama
test "a bad id is refused" {
    let refused = test.throws(Refused) { Library(books: Books.fake()).shelve(0, "x"); };
    test.is(refused, Refused.because("non-positive id"));
}
```

A `test` block has a name and a body. It may stand at module level, inside
an object or inside a role. `okkama run -t` runs the tests of the file it
is given, in declaration order, and does not call `main`:

```text
=== RUN   Library :: a shelved title can be read back
--- PASS: Library :: a shelved title can be read back
=== RUN   a bad id is refused
--- PASS: a bad id is refused
…
PASS
```

- A test inside an object or a role is shown as `Owner :: name`. That is
  only a namespace. The test gets no `self` and no access to the owner's
  hidden actions or fields; it uses the object like any other caller.
- Names are unique within a module, and per owner for object and role
  tests. A duplicate is a compile error before any test runs.
- Each test uses the imports of the file that declares it.
- In a normal run, the parser skips test and fake blocks without building
  them. Test-only code cannot affect a production program, and it costs
  nothing there.

Tests run in the same process.
`okkama run -t --filter TEXT` runs only the tests whose names contain
`TEXT`; with several `--filter`s, those that contain any of them. A test
may stand anywhere among an object's members, between hidden actions too;
it is still a test of the object. The standard library can
render a test report as human text, TAP, JSON or JUnit (`testing/report`),
but the command line offers no report formats, seeds or parallel workers
yet. They are planned with the rewritten standard library (#339).

## 15.2 The `test` binding

Inside a running test, the name `test` is bound to the current case. It is
not imported and does not exist outside a test:

- `test.ok(condition)` requires an exact `bool`;
- `test.is(actual, expected)` and `test.isnt(actual, expected)` use the
  language's structural equality, deep through collections and objects;
- `test.close(actual, expected, tolerance)` compares two floats: they pass
  when equal or no further apart than the tolerance, and a NaN is never
  close;
- `test.text(actual, expected)` compares two strings and names where they
  part: the scalar, its line and column in the expected text, and the text
  around it;
- `test.bytes(actual, expected)` compares two `@byte` arrays and names the
  first offset where they differ, or their sizes;
- `test.fail("explanation")` records a failure;
- `test.skip("reason")` ends the test and reports it as skipped;
- `test.throws(Type) { … }` requires the block to throw a `Type`, and
  returns the caught error for further checks.

`test.skip` is for a test that cannot run here: a missing volume, a
service that is not configured. The code after it does not run, and no
`catch` takes it, so a helper that skips cannot be overruled by its
caller. Cleanup runs as for any failure. A skipped test does not fail the
run, unless an assertion had already failed; a skip does not erase a
failure. The example also puts a test among the hidden actions:

{{example:15-skip-tests}}

A failed assertion does not stop the test. The remaining assertions still
run, and every failure is reported:

```text
=== RUN   a failing one
--- FAIL: a failing one
    expected 3, got 2
FAIL
```

An uncaught throw, a panic or a failure in cleanup ends only the current
case. Its `defer`s and `destroy`s run, and the next case starts. The
process exits with 0 when no case failed; a skipped case does not fail.

`test.throws(Refused) { … }` means exactly `try { …; panic …; } catch e is
Refused { e; }`. The error it returns is an ordinary enum value, so
`test.is(refused, Refused.because("non-positive id"))` compares the case
and the payload. There is no `test.panics`. A panic is a broken invariant,
not an outcome to assert (Chapter 10), and nothing catches it.

The binding follows the whole dynamic extent of the test, so a helper
object in another module can call `test.is` on behalf of the test that
called it. The bundled `test/more` module builds on that: its
`Assertions()` object chains `ok`, `is` and `isnt` and records them in the
running case.

## 15.3 Why a fake and not a mock

Meszaros sorts test doubles into five kinds: dummy, stub, spy, mock and
fake [@meszaros2007]. A **mock** is programmed with expectations: "expect
`remember(7, "Dune")` to be called once". The test then verifies that the
calls happened. A **fake** is a working implementation that takes a
shortcut, such as keeping data in memory instead of a database. Fowler's
essay draws the consequence. Mocks make a test check *how* the code under
test talks to its collaborators, and such tests break when the
conversation changes even if the result does not [@fowler2007].

Elegant Objects goes further: every interface should ship its own fake, so
that tests never build ad-hoc mocks [@bugayenko2016]. Okkama makes that a
language feature. A role may contain one `fake` block:

```okkama
role Books {
    remember(id is int, title is string) is void;
    title(id is int) is string;
    has(id is int) is bool;

    fake {
        remember(id is int, title is string) is void { fake.storage.set(id, title); }
        title(id is int) is string { fake.storage.get(id); }
        has(id is int) is bool { fake.storage.has(id); }
    }
}
```

`Books.fake()` makes one. The fake is a `Books`. Code that takes a
`Books`, like `Library`, cannot tell it from the real thing, and it has no
reason to. The test then checks *results*: the title comes back, the
second shelving of an id is refused. It does not check which calls were
made in which order.

The fake lives with the role, written once by the role's author, and
every test of every user of the role shares it. When the role changes,
the compiler checks the fake against the new contract like any other
implementation.

## 15.4 Writing a fake

- A role has at most one `fake` block.
- Its actions must implement the role exactly, as an object would: names,
  parameter types, fixed or variadic shape, results.
- It may declare fields with `has`, and every field needs a default. A fake
  takes no constructor arguments, so nothing else could set them. Defaults
  are evaluated for each `Role.fake()`.
- It may stage successors with `^field`, like an object's successor
  (§15.5).

`Role.fake()` works only in test mode. Anywhere else the compiler rejects
it:

{{example:15-fake-production-rejected}}

A production program therefore cannot reach a fake, not even by accident
in a code path that tests happen to cover. Mistakes inside the fake block
are reported when the tests are compiled, in terms of the role and at the
fake's own code: `Fake field "value" of role "Counter" needs a default: a
fake takes no constructor arguments`, `Action "count" of the fake of role
"Counter" must return int, got string`. Before 0.5.0 they named the
generated object `Counter$fake` (#276).

## 15.5 State in a fake

A fake that only returns constants is a stub. A useful fake often has to
remember something: what was stored, how many times a door was opened. It
has two places to keep it.

**Fields and successors.** A fake's fields are ordinary immutable fields,
and its successors make new fakes, exactly as for objects:

```okkama
role Door {
    opened() is ^Door;
    open() is bool;

    fake {
        has times is int = 0;
        opened() is ^Door { ^times = times + 1; }
        open() is bool { times > 0; }
    }
}
```

`Door.fake().opened().open()` is `true`; the original fake still says
`false`. This suits state that the role's own successors carry.

**`fake.storage`.** For a fake that emulates a repository, the language
provides a small key–value store, reachable only as `fake.storage.<op>(…)`
inside the fake's own actions. The operations are `set(key, value)`,
`get(key)`, `has(key)`, `remove(key)`, `clear()` and `size()`.

- Each `Role.fake()` gets its own store. A fake and all its successors
  share one, so data survives a transition without being copied.
- A missing key in `get` or `remove` is a programming error. There is no
  `Maybe` result; use `has` first.
- Keys follow the map-key rules; values may be anything, objects
  included.
- The store cannot be stored, passed, returned or compared. It is not
  part of the fake's fields, so two fakes with different contents can
  still be equal.

This is the one place where Okkama has hidden mutable state, and it is
fenced on every side: test mode only, inside fake actions only, one store
per fake.

## 15.6 Properties

An example-based test checks the cases its author thought of. A property
states a claim about *every* input and lets the machine look for a
counterexample. Claessen and Hughes's QuickCheck made the idea practical:
generate random inputs, run the claim, and when it fails, *shrink* the
input to a minimal one before reporting it [@claessen2000].

`testing/property` follows that design:

{{example:15-property-tests}}

`Property(generation, seed, cases, maximum_size, shrink_limit, condition)`:

- `IntegerGeneration(-100000, 100001)` produces integers from −100000 up
  to, but not including, 100001.
- `42` is the seed of the property's own PCG random stream. The same seed
  produces the same inputs and every run.
- `200` cases, with a size that grows from 0 to `1000` across them, so the
  early cases are small.
- When a case fails, up to `64` reductions are tried, and the smallest
  input that still fails is reported.

The second property is false. `Cents(total: -1).rest()` is `-1`, because
`int(...)` truncates toward zero (Chapter 5): the digit of −1 cents is 0,
and the rest is −1. The report gives everything needed to reproduce it:
the seed, the case index, the size and the shrunk value. The value is
`-1`, the simplest counterexample there is.

A condition that throws a typed error counts as a failure and is reported
with the error's text. A property is a `Checking` like the other checks of
`testing/check`, and `AsyncProperty` takes an async condition.

## 15.7 The rest of the toolbox

The standard library adds a few more pieces, all usable only inside a
running test:

- **`testing/check`**: checks with readable failure output. `TextEquals`
  shows a diff of two texts, `SequenceEquals` compares sequences item by
  item, `CloseTo` compares floats within a tolerance, `MatchingRegex`
  matches text, and `Eventually` polls an async condition against an
  explicit clock.
- **`testing/time`**: `ManualClock`, a clock a test advances by hand. It
  does the `Timing` role, so code that takes a `Timing` instead of reading
  the host's clock can be tested without sleeping.
- **`testing/double`**: `RecordedCalls<T>` records values in order, and
  `Stub<T>` hands out configured values one by one. They are building
  blocks for fakes that need them. There are deliberately no generated
  mocks, no reset and no implicit expectations.
- **`testing/generation`**: generators for booleans, integers, finite
  floats, choices from a list, filtered values and sequences, each with
  its own shrinking.
- **`testing/snapshot`**: compares text with a stored file. Checking and
  updating snapshots are separate commands, so a test run never rewrites
  its own expectations.

Async code is tested with `async test "…" { … }`. The runner waits for the
body and its cleanup before it records the result. Tasks still pending at
the end are cancelled and drained. There is no implicit timeout, so a test
that may hang should bound itself with a budget or a deadline.

## 15.8 What does not work yet

- Report formats, seeds and parallel workers are not in the command line;
  they are planned with the rewritten standard library (#339).

Until 0.4.1, a successor passed to `test.is` crashed the compiler and did
not count as used (#275). It is now an ordinary argument:
`test.is(bottle.open().close(), bottle)` compares the two bottles.

## 15.9 What this design costs

- **A fake must be written.** A mocking library generates a double from an
  interface in one line. A fake is code, and for a large role it is a lot
  of code. The author of the role writes it once; every user saves the
  work.
- **No interaction testing.** "Was `flush` called before `close`?" has no
  direct answer. A fake can record calls with `RecordedCalls`, and a test
  can check the record, but nothing is generated for it. Most such
  questions are better asked of the result.
- **No access to private state.** A test sees an object as its callers do.
  If something cannot be tested through the public actions, the object is
  doing too much, or the role is missing an action. That is the intended
  pressure, and sometimes it is just inconvenient.
- **Properties need thought.** A good property is harder to write than an
  example. The library makes running one cheap; it cannot invent the claim.

In return:

- tests sit beside the code and vanish from production;
- every role has a double, maintained with the role and checked by the
  compiler;
- a failed property comes back as a seed and a minimal input that
  reproduce it.
