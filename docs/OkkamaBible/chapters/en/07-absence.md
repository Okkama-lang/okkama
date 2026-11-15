# Chapter 7. Absence Without Null

"I call it my billion-dollar mistake. It was the invention of the null
reference in 1965." Tony Hoare's confession [@hoare2009] is the most quoted
sentence about programming-language design of the last twenty years, and for
good reason: almost every program written in a mainstream language contains
code whose only purpose is to ask whether a reference is null, and almost
every such program has crashed, somewhere, because one such question was not
asked. This chapter explains what null was trying to express, how Okkama
expresses each of those things separately, and what the replacement costs.

## 7.1 What null means — too many things

Hoare added null to ALGOL W because it was easy to implement: a reference
that points nowhere costs one reserved address. The trouble is not the
implementation but the meaning. In real programs a null reference stands for
at least five different situations:

| Situation | Example |
|---|---|
| A value that may legitimately be absent | a user without a middle name |
| A lookup that found nothing | `map.get(key)` for a missing key |
| A failure | a file that could not be opened |
| A value not yet initialised | a field set later in the object's life |
| An explicit "clear this" | a JSON `null` in an update request |

A language with null uses one value for all five, so the type of a reference
cannot tell the reader which situation applies — or whether any applies at
all. Every reference of every type may be null; the type system, which is
supposed to say what operations a value supports, says nothing about the most
common way those operations fail.

## 7.2 Okkama's answer: one construct per meaning

Okkama has no null. The words `null`, `nil` and a bare `none` do not parse;
the compiler stops with "Null/none/nil values are not part of Okkama v1.0
syntax", even where `none` would only have been a variable name (Chapter 3).
`none` exists only as a case, written `Maybe.none` or `.none` in a choice. Instead, each of the five situations has its own construct.

| Situation | Okkama construct | Chapter |
|---|---|---|
| A value that may be absent | `Maybe<T>`: `some(value)` or `none` | this chapter |
| A lookup that found nothing | an explicit fallback (`map[key] else default`) or `Maybe` | 7, 8 |
| A failure | a typed error, thrown and caught by type | 10 |
| A value not yet initialised | impossible: an object exists only after its constructor bound every field | 12 |
| An explicit "clear" next to "leave alone" | `Presence<T>`: `some(value)`, `none`, `missing` | this chapter |

Two of these lines remove the problem rather than represent it. A failure is
not a value at all: it is a typed error that travels up the call stack to a
handler that names its type. And an uninitialised field cannot be observed,
because there is no moment at which an object exists with a field unbound.

The remaining cases are values that may be absent, and Okkama gives them a
type that says so.

## 7.3 `Maybe<T>`

`Maybe<T>` is an ordinary enum, available without import, with two cases:
`some(value)` and `none`. An action that may have no answer says so in its
result type, and a caller cannot use the value without first deciding what to
do when it is not there. Okkama offers four ways to decide, in increasing
order of ceremony; the program below uses all four.

{{example:07-maybe}}

1. **`else` gives a fallback.** `users.find(3) else "nobody"` is the value if
   present, the fallback otherwise. The fallback is evaluated only when
   needed, and it may be `else throw Error...` or `else panic "..."` when
   absence is not acceptable at that point.
2. **A compact choice** `?? … !!` maps each case to a value, with the
   case's payload bound by name: `?? .some(name): "hello, " + name !! .none:
   "hello, stranger"`. It must cover every case, and it cannot be nested —
   nested choices are a known source of unreadable code, and a separate
   variable is always clearer.
3. **`match`** handles each case with a block when the cases need statements,
   not just expressions.
4. **`try?`** converts a typed error into absence: `try? users.require(4)` is
   `Maybe.some(name)` if the call returned and `Maybe.none` if it threw any
   error. Its counterpart `try!` asserts that a call cannot fail and turns a
   thrown error into a panic.

Notice what is *not* in the list: a way to take the value out without
deciding. There is no `.get()` that throws, no `!` that asserts, no implicit
unwrapping at an assignment. Every exit from a `Maybe` names what happens in
the `none` case, and that name is visible in the code.

### No automatic wrapping, no collapsing

Two rules keep `Maybe` predictable. Results are not wrapped automatically: an
action declared `is Maybe<string>` returns `Maybe.some(name)`, not `name`, so a
reader can see which return statements produce an answer and which produce its
absence. And nested `Maybe` values are never collapsed: `Maybe<Maybe<int>>`
keeps both levels, because "the cache has no entry" and "the cache has an
entry saying there is no value" are different facts. Languages that collapse
nested optionals — TypeScript's `undefined`, JavaScript's optional chaining —
lose exactly that distinction.

## 7.4 Where a `Maybe` comes from

A `Maybe` is not something a programmer meets only in their own code. The
language and the standard library answer with one wherever a question may have
no answer, and they never answer with a sentinel — no `-1` for "not found", no
empty string for "not set".

{{example:07-sources}}

- **Searches.** `index_of(value)` on an array, `find(part)` and
  `find_last(part)` on a string answer a `Maybe<int>`. The functional `find`
  block answers a `Maybe` of the first element that satisfies it:
  `names.find() -> n { n.starts_with("b"); } else "nobody"`.
- **Indexes and keys, with a fallback.** `names[5] else "past the end"` and
  `stock["kiwi"] else 0` read an element or an entry that may not exist. Without
  the `else`, a miss is a runtime failure: the fallback is part of the
  expression, not an afterthought. A map answers `has(key)` without looking the
  value up, so an action that wants to return absence is one `if` away from a
  `Maybe` (the `Directory.phone` action above).
- **Actions that say so.** Any action may declare `is Maybe<T>`; the standard
  library does it for every reader of optional data. The json reader's
  `maybe(name)`, `Environment.maybe(name)`, the configuration sources and a
  database row's `maybe(column)` all answer `Maybe` per kind:
  `row.maybe("nick").text()` is `Maybe.none` when the column holds SQL `NULL`.
- **`try?`.** It turns a thrown error into `Maybe.none` (§7.3).
- **Streams.** `for await` asks a stream for its next element with an
  `async next() is Maybe<T>` action, and `Maybe.none` ends the loop. Absence is
  how a stream says "no more", exactly as it is how a search says "not here"
  (Chapter 20).

A value is made with `Maybe.some(value)` and `Maybe<T>.none`. The type argument
of `none` may be left out when the context gives it — `let m is Maybe<int> =
Maybe.none;` — and must be written when nothing does, because `none` alone
carries no element type.

## 7.5 `else` in detail

`else` after a `Maybe` is the most common exit, and it has precise rules.

- **The fallback is lazy.** It is evaluated only when the value is absent.
  `Maybe.some(0) else panic "eager"` never panics; a fallback that computes,
  logs or allocates costs nothing on the present path.
- **The fallback has the type of the payload.** `Maybe<int> else 0` is an
  `int`. A fallback of another type is a compile-time error ("Fallback must be
  int, got string"), not a conversion.
- **Three kinds of fallback.** An expression of the payload type; `throw` with
  an error, when absence is a failure of the caller's contract (Chapter 10);
  `panic` with a message, when absence means the program itself is wrong. These
  are the only three: `return`, `next` and `last` cannot follow `else`, because
  a fallback is a value, and control flow that depends on presence is written
  as a `match` or an `if`, where a reader looks for it.
- **Fallbacks chain.** `book.phone("brian") else office else "no number"`
  tries each source in order: the right-hand `else` turns `office` into a
  string, the left-hand one takes it when the first lookup misses.
- **The same word in `confirm`.** `confirm port > 0 else throw ...` uses `else`
  for a condition rather than an absence (Chapter 10). The fallback is lazy
  there too: the error is built only when the condition fails.

## 7.6 A `Maybe` is a value

`Maybe<T>` is an ordinary value, and everything Chapter 16 says about variants
applies to it.

- **Equality is structural.** `Maybe.some(3) == Maybe.some(3)` is true, and so
  is `Maybe<int>.none == Maybe<int>.none`. An array of `Maybe` values answers
  `contains` and `index_of` by the same equality.
- **It can be a field.** An object may hold `has note is Maybe<string>;` for
  data that is genuinely optional, and give it a meaning in one place:
  `shown() is string { note else "(no note)"; }`. Callers ask the object, not
  the field.
- **It is not a map key.** Map keys must be scalars or key objects (Chapter
  8); a field `has index is %Maybe<int>:string;` is rejected with "Map key
  type Maybe<int> is not a scalar key type". A map that needs "no key" as a key is usually two
  structures: the map and the value for the missing case.
- **It owns what it holds.** A `Maybe` that holds an object with a `destroy`
  is that object's owner: the object is destroyed when the `Maybe` goes out of
  scope, like any other owner (Chapter 11).

## 7.7 `Presence<T>`: three states for updates

Some data needs three states, and the most common example is a partial
update. When a client sends a JSON PATCH for a user record, each field can
mean one of three things: set the field to a new value, clear the field, or
leave it as it is. JSON Merge Patch encodes these as a value, a `null`, and an
absent key [@rfc7396; @rfc8259]. A language with only null cannot represent the
third state without an extra flag per field; a language with only `Maybe`
cannot represent it either.

`Presence<T>` has three cases — `some(value)`, `none` and `missing` — for
exactly this purpose.

{{example:07-presence}}

The patch object's `applied` action maps "set" to the new value, "clear" to
the empty string and "leave alone" to the current value. The compact choice
must name all three cases; forgetting `missing` is a compile-time error, not a
bug report from the first client that sends a partial update.

Because `Presence` carries more information than `Maybe`, Okkama does not let
it degrade silently. `else` works only on `Maybe`; on a `Presence` it is
rejected, and the program must say how to merge the two kinds of absence.

{{example:07-presence-rejected}}

`nickname.fold() else "anonymous"` is the explicit form: `fold()` turns
`missing` and `none` into `Maybe.none` — on purpose, in writing. (In the other
direction, `Maybe.as_presence()` keeps `some` and `none` and never invents
`missing`.)

## 7.8 Absence at the edges

Inside a program, absence is a type. At its edges — environment variables,
configuration files, JSON, databases — absence arrives as data, and the
standard library draws the same line everywhere: **absent is a value,
malformed is an error.** `Environment.maybe("PORT").int()` is `Maybe.none` when
the variable is not set and `Maybe.some(8080)` when it holds `8080`; when it
holds `80a0`, the read throws `InvalidEnvironmentValue` naming the variable,
because a value that is there but wrong is not the same fact as a value that is
not there. The configuration sources throw `MalformedSetting` by the same rule.
The json reader offers both views of a member: `maybe(name)` answers `Maybe`
and treats an absent key and a `null` alike, while `presence(name)` keeps them
apart as `Presence.missing` and `Presence.none`; in both, a member of the wrong
kind is `JsonError.wrong_kind`. `maybe` is literally `presence` followed by
`fold()`.

Databases are where null was born a second time. SQL's `NULL` came from Codd's
extension of the relational model [@codd1979], with a three-valued logic in
which `NULL = NULL` is neither true nor false but unknown. Codd later proposed
two kinds of missing value — "missing but applicable" and "missing and
inapplicable" — and a four-valued logic to go with them [@codd1990]; no major
system implemented it, and SQL kept one `NULL`. Okkama maps the database's
`NULL` to `Maybe.none` in both directions: `DbText(Maybe<string>.none)` binds a
`NULL` parameter, and `row.maybe("nick").text()` reads one back as
`Maybe.none`. Equality on the Okkama side stays two-valued:
`Maybe<int>.none == Maybe<int>.none` is simply true. Codd's second mark has a
counterpart too, in `Presence`: `none` is "there is no value", `missing` is
"nothing was said".

## 7.9 A worked example: settings

The same absent setting can mean three different things, and only the code
that uses the value knows which. A server reads its settings as text and
decides, per setting, what absence means.

{{example:07-settings}}

- `port()` has a sensible default: absence means 8080.
- `certificate()` has no default, and absence is meaningful: no certificate
  means plain HTTP. The compact choice names both cases, so the meaning of
  absence is written next to the meaning of presence.
- `workers()` has no default and no meaning for absence: the program cannot
  run without it, so absence is an error, thrown with the setting's name.

The `Settings` object knows none of this. It answers `Maybe` for "not there"
and throws `SettingError.malformed` for "there but not a number", and it leaves
every decision about absence to its caller. That split — the reader reports,
the user decides — is the design the standard library's readers follow.

## 7.10 Absence in generic code

`Maybe<T>` is an ordinary generic type, so it composes with generic objects
the way any type does. A cache is the textbook case: it knows nothing about
its keys and values, and a lookup may miss.

{{example:07-cache}}

- **The miss belongs to the answer.** `find(key) is Maybe<V>` returns
  `Maybe.some(value)` or `Maybe.none`, whatever `V` is. The cache does not
  invent a default for a type it knows nothing about, and the caller decides
  what a miss means: a fallback with `else`, a compact choice with `?? !!`,
  or a `match` (Section 7.3).
- **No sentinel can collide with a value.** A cache of prices that returned
  `0` or `-1` for a miss would be wrong for the first item that costs
  nothing. With `Maybe`, every value of `V`, including `0`, `""` and an empty
  array, is a hit.
- **The key is a type parameter used as a map key.** `K` keys the map
  `%K:V`, so the compiler holds every specialization to the key rules of
  Section 8.3: `Cache<string, int>` and `Cache<Point, string>` compile, and a
  `Cache<@int, string>` would not (Chapter 17).
- **Counting is a successor.** `looked_up(key)` returns the next state of the
  cache with one more hit or miss; the `find` that answers the question does
  not change anything.

The design keeps two questions apart that sentinel-based caches merge: "is
it there?" and "what is it?". The cost is the same as everywhere in this
chapter, one `else` or one `match` at each call site, and in generic code it
buys more than elsewhere, because a generic object has no value it could
safely use as a sentinel for every `V`.

## 7.11 Absence with behaviour: the null object

Sometimes absence has behaviour. A service that may or may not have a logger
does not want a `Maybe<Logger>` and a choice at every call; it wants a logger
that does nothing. This is Woolf's *Null Object* pattern [@woolf1998]: an
implementation of the role whose actions are harmless no-ops, passed where the
real one would go.

In Okkama the null object is an ordinary implementation of a role (Chapter
14), and the choice between it and `Maybe` follows one question: **does the
caller act differently when the thing is absent?** If it does — a certificate
turns TLS on, a missing phone number changes what the screen shows — absence is
data, and it is a `Maybe` that forces the caller to decide. If it does not — a
silent logger, an empty set of listeners, a clock that never ticks in a test —
absence is behaviour, and an object that implements the role keeps every call
site simple. Elegant Objects arrives at the same place from the other side:
never return null, return an object that knows what to do [@bugayenko2016].

What Okkama rejects is the third option, the sentinel: `-1` for "not found",
`""` for "not set", `0` for "unknown". A sentinel is a null with a disguise —
an ordinary value of the type that the reader must remember to treat
specially. Okkama's searches answer `Maybe` precisely so that no `int` has to
mean "nothing".

## 7.12 How other languages handle absence

Okkama's approach belongs to a family that started with ML's `option` type
and Haskell's `Maybe` [@pierce2002; @haskell2010]. The table compares the
mainstream designs.

| Language | Absence | Can a value of type T be absent? | Unwrapping without deciding |
|---|---|---|---|
| Java | `null`; `Optional<T>` in libraries | yes, every reference | `.get()` throws; null dereference throws |
| C# 8+ | `null`; nullable reference types as warnings | yes, with warnings | `!` suppresses the warning |
| Kotlin | `T?` nullable types | no, unless declared `T?` [@kotlinspec] | `!!` throws |
| Swift | `Optional<T>`, written `T?` | no, unless declared `T?` | `!` traps |
| Rust | `Option<T>` | no [@klabnik2023] | `.unwrap()` panics |
| Haskell | `Maybe a` | no | `fromJust` errors |
| Go | `nil` for pointers, maps, slices, interfaces, channels and functions; zero values; `v, ok := m[k]` | yes, for those kinds [@gospec] | dereferencing `nil` panics |
| Okkama | `Maybe<T>`, `Presence<T>` | no | none: every exit names the `none` case |

Kotlin, Swift and Rust made the decisive move — a plain `T` cannot be absent —
and their experience shows that it removes most null-related crashes. Each of
them also kept an escape hatch for "I know it is there": `!!`, `!`,
`.unwrap()`. Okkama's only equivalent is `else panic "reason"`, which costs a
message and therefore a moment's thought. The difference is small in
characters and large in culture: the shortest way to get a value out of a
`Maybe` is to say what absence means.

## 7.13 What it costs

- **Ceremony at the boundaries.** Every lookup that may miss needs a fallback,
  a match or a `try?`. Code that reads many optional fields — configuration,
  request parameters, JSON — is longer than in a language with null. The
  standard library's answer is the json-style reading API: `maybe(name).text()
  else "default"` reads an optional field in one line (Chapter 22), and the
  library audit asks for the same style in every module that reads data.
- **Deciding what absence means.** Porting a design built on null forces a
  decision at every nullable reference: is this a `Maybe`, an error, a
  default, or a sign that the object was built wrongly? The decision is work;
  it is also the work the original design skipped.
- **Two types instead of one.** `Maybe` and `Presence` are separate types, and
  converting between them is explicit (`fold()`, `as_presence()`). This is the
  price of keeping the third state; most programs never need `Presence`.

## 7.14 Limits

- `Maybe` has no library of combinators such as `map`, `and_then` or `filter`
  in the language today; chained optional computations are written with
  compact choices or `match`. Whether to add them is an open question: each is
  convenient, and each is another way to say what `??` already says.
- A `Maybe` cannot be a map key, and a fallback cannot carry control flow:
  `else return` and `else next` do not exist. Both are deliberate, and both
  occasionally cost a few lines.
- Standard-library APIs are not yet uniform: some modules return `Maybe`,
  some throw a typed error for a missing value, some take an explicit
  fallback. The json module's style is the reference the library is being
  aligned with.
