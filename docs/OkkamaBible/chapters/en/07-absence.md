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

Okkama has no null, and the reserved words `null` and `nil` do not parse
(Chapter 3). Instead, each of the five situations has its own construct.

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

## 7.4 `Presence<T>`: three states for updates

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

## 7.5 How other languages handle absence

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
| Okkama | `Maybe<T>`, `Presence<T>` | no | none: every exit names the `none` case |

Kotlin, Swift and Rust made the decisive move — a plain `T` cannot be absent —
and their experience shows that it removes most null-related crashes. Each of
them also kept an escape hatch for "I know it is there": `!!`, `!`,
`.unwrap()`. Okkama's only equivalent is `else panic "reason"`, which costs a
message and therefore a moment's thought. The difference is small in
characters and large in culture: the shortest way to get a value out of a
`Maybe` is to say what absence means.

## 7.6 What it costs

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

## 7.7 Limits

- `Maybe` has no library of combinators such as `map`, `and_then` or `filter`
  in the language today; chained optional computations are written with
  compact choices or `match`. Whether to add them is an open question: each is
  convenient, and each is another way to say what `??` already says.
- Standard-library APIs are not yet uniform: some modules return `Maybe`,
  some throw a typed error for a missing value, some take an explicit
  fallback. The json module's style is the reference the library is being
  aligned with.
