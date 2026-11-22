# Appendix D. Glossary

The terms below are used in this book in a specific sense. Each entry points
to the section that explains the term.

**action** — A named piece of an object's behaviour, with typed parameters and
a declared result (`is T` or `is void`). Okkama has no free functions: every
action belongs to an object, except the module's `main` (§13.1).

**action type** — The type of an action value, written as its signature
without names: `(int, int) is int`, or `async (int) is string` for an async
one (§18.1).

**adopt** — What a called action does with a fresh argument nobody owns yet: it
becomes the owner and destroys the object when it returns, unless it returns
it (§11.5).

**anonymous action** — An action written in place as a value,
`(value is int) is int { value * 2; }`. It captures values, not variables,
and cannot be async (§18.1).

**any** — The top type. A value put into `any` becomes an opaque box: it can
be stored and passed on, but not narrowed back to what it was (§4.6).

**async action** — An action declared `async`. Calling it returns a
`Future<T>`, and only an async action can `await` (§20.1).

**async destroy** — A `destroy` block that may `await`. The scope that owns
the object awaits it when it closes, and cancellation does not interrupt it
(§11.8).

**Atomic** — `async/atomic`'s `Atomic<T>`: one value that tasks share and
replace whole, with `compare_and_replace` for coordination (§20.8).

**authority** — What code may touch outside the program: files, the clock,
the environment, other processes. The standard library passes it as values,
such as a `DirectoryLocating`, a `Timing` or an `Environment` snapshot
(§21.4).

**await** — The unary operator that waits for a future inside an async
action. It returns the future's value or rethrows its failure (§20.1).

**blade** — Okkama's package tool. It adds, syncs and updates dependencies
from git and picks the lowest version that satisfies every manifest (§19.7).

**borrow** — A second name or a parameter that refers to an object another
scope owns. A borrow never destroys the object (§11.5).

**bound action** — An object's action taken as a value without calling it,
`retail.of`. It carries its receiver and exists only where its action type is
written (§18.5).

**budget stop** — The end of a run because a step, memory, output or time
budget ran out. It cannot be caught and is not a failure of the program
(§10.13).

**builtin action** — An action of a built-in value (`int`, `string`, arrays,
maps, `Future`) that needs no import, such as `size()` or `length()`
(Appendix B).

**Cancelled** — The built-in failure that `await` throws for a future whose
cancellation has finished (§20.5).

**cap** — A named contract of computed fields that an object grants: a
read-only view, acquired with `object::Cap` and read with `card::field`
(§12.8).

**Cell** — `cell`'s `Cell<T>`: the one mutable container for synchronous code.
It cannot be a field and cannot be used in async code (§13.11).

**channel** — `async/channel`'s bounded FIFO `Channel<T>`, used through its
`Sending<T>` and `Receiving<T>` ends (§20.6).

**compact case** — A `match` case written on one line, `case pattern: value;`
(§16.4).

**compact if** — The one-line `if`, `if condition: a else: b`, an expression
whose two branches are single expressions (§9.4).

**confirm** — The statement that checks a condition: without `else` a false
condition panics, and with `else throw …` it throws a typed error (§10.9).

**const** — A module constant: a value the compiler computes from literals,
other constants and operators. It is public unless marked `hidden` (§19.6).

**constructor** — How an object is made. Every object declares at least one
constructor, either a full one with parameters and a body or a shorthand
`Name(:field)` (§12.3, §12.4).

**defer** — An expression registered to run when its scope ends, newest
first, before the scope's objects are destroyed (§11.3).

**destroy** — An object's cleanup block, run when the scope that owns the
object ends, on every path out of it (§11.4).

**doc example** — A fenced code example in a doc comment. `okkama run -t`
compiles it, and runs it when it is marked `okkama,run` (§15.9).

**does** — The word in an object header that declares the roles the object
implements: `object Bottle does Package` (§14.1).

**else (fallback)** — `value else fallback`: the answer when a `Maybe` is
`none` or an index or key is missing (§7.5).

**enum** — A closed set of named cases, `enum Priority = (low, normal,
urgent);`. A case may carry payload fields (§16.1, §16.2).

**error enum** — An enum declared `is Error`. Only error enums are thrown and
caught as typed failures (§10.5).

**fake** — A test-only implementation of a role, written inside the role and
made with `Role.fake()` (§15.3).

**field** — State declared with `has`. A field is private to its object and
never changes after construction (§12.2).

**for await** — The loop over an asynchronous source: each pass awaits the
source's `next()` (§20.10).

**functional block** — The block after `map`, `filter`, `reduce`, `any`,
`all` or `find`: `xs.map() -> x { x * 2; }` (§8.2).

**Future** — `Future<T>`, the value an async call returns. It is lazy, runs at
most once and keeps its outcome. Its value is reached only through `await`
(§20.1).

**generic parameter** — A type parameter of an object, role or enum, such as
the `T` of `Box<T>`, inferred at construction (§17.1).

**has** — The keyword that declares fields: `has given, family is string;`
(§12.2).

**hidden** — Not part of the public contract: a `hidden` object, role, enum
or const stays inside its module (§19.5), and a `hidden` action stays inside
its object (§12.7).

**key parameter** — A generic parameter used as a map key. Every
specialization must give it a type that can be a key (§17.5).

**match** — The expression that takes a value apart by its cases, with block
or compact cases and a checked remainder (§16.3).

**Maybe** — `Maybe<T>`, a value or its absence: `Maybe.some(value)` or
`Maybe.none` (§7.3).

**module** — One source file, which begins with `module name vX.Y.Z;`. A
module may span several files listed together (§19.1).

**module path** — The slash-separated name of a module, `shop/money`, and the
path that imports it: `import "shop/money"` (§19.1, §19.3).

**must-use rule** — The rule that the result of a successor action must be
consumed: returned, passed, stored, called on or rebound (§13.6).

**native** — A function that the host provides and a module declares with
`native` and no body. It is the only way to reach outside the program
(§21.1).

**native group** — One of the groups of natives (`Fs`, `Net`, `Process`,
`Env`, `Clock`, `Random`, `Terminal`) that a host embedding the runtime can
deny (§21.6).

**native object** — A handle type, such as a file descriptor or a socket, that
only a native can make. A program stores it and passes it back to natives
(§21.3).

**nominal** — Identified by name and declaration, not by shape. Types are
nominal (§4.4), and an object implements a role only by declaring `does`
(§14.2).

**object** — The only place for behaviour: private fields, explicit
constructors and actions, with no inheritance (§12.1).

**once let** — A binding computed once per object, on first use, and kept
for that object (§13.13).

**operator role** — A standard role, such as `Adding<T, R>`, that gives an
object an operator: `a + b` means `a.plus(b)` (§14.7).

**owner** — The scope that destroys an object when it ends. A binding owns
the fresh object it is given, and a returned object moves to the caller
(§11.5).

**package** — A git repository whose `vX.Y.Z` tags are its versions and whose
modules are in `src/`, installed into `pkg/` by blade (§19.7).

**panic** — A failure that signals a broken invariant. It cannot be caught,
and it ends the run after cleanup (§10.4).

**Presence** — `Presence<T>`, which adds `missing` to `some` and `none`, for
updates where "leave it" and "clear it" differ (§7.7).

**process worker** — A synchronous action started with `spawn`. It runs in a
separate process, and only scalars, arrays and maps cross the boundary
(§20.4, §21.5.1).

**rebind** — `name = expr;` without `let`: moving a local to a successor of
its object. It is allowed only for types with successor actions (§13.7).

**role** — A nominal contract of action signatures, with no fields and no
bodies, that objects implement with `does` (§14.1).

**role reference** — A value typed by a role. Only the role's actions are
reachable through it, and nothing casts it back to the object (§14.3).

**runtime error** — A defect found while the program runs, such as an index
out of range or an unbound field. It cannot be caught, and it ends the run
after cleanup (§10.4).

**scope** — A block, a loop iteration or a branch. A scope owns what it
creates and releases it when it ends (§11.5).

**spawn** — The operator that starts a call now. An async action becomes a
task of the run, and a synchronous one becomes a process worker (§20.4).

**specialization** — A generic type with its arguments, such as `Box<int>`.
Each specialization is a type of its own, with no variance (§17.6).

**state owner** — A task that holds some state as a local and changes it
through messages that other tasks send over a channel (§20.6).

**std. path** — A standard-library declaration named in place without an
import, such as `std.io.stdout.Stdout()` (§19.2).

**structured await** — The rule that cancelling a task cancels the future it
is awaiting, if it started it, and waits for that future's cleanup (§20.5).

**successor** — An action declared `is ^T` that returns a new object: fields
staged with `^field = value` are replaced, and the rest are copied from the
receiver (§13.5).

**task** — A unit of concurrency inside one run. One scheduler runs one task
at a time, switching only at an `await` (§20.3).

**test** — A block `test "name" { … }` beside the code it tests, run by
`okkama run -t`. Inside it, the binding `test` offers the assertions (§15.1,
§15.2).

**test.case** — A named sub-case inside a test, `test.case(name) { … }`. Its
failures are reported on their own, and a filter can select it (§15.9).

**test.throws** — `test.throws(Type) { … }`, which requires the block to throw
a `Type` and returns the error caught (§15.2).

**throw** — The statement that raises a value of an error enum. A bare
`throw;` in a `catch` rethrows (§10.6).

**try? and try!** — Marks that turn a call's typed failure into a `Maybe`
(`try?`) or into a panic (`try!`) (§10.10).

**typestate** — A protocol written as one role per state, where each
transition returns the role of the next state (§14.5).

**value object** — An object whose fields are scalars, enums or other value
objects, with no `destroy` and no caps. It compares by its fields and can
be a map key (§12.9).

**variadic parameter** — A last parameter `values is ...T` that takes zero or
more positional arguments, unpacked with `values...` (§13.4).

**void** — The marker for an action that returns no value. It is not a type
(§13.1).
