# Chapter 22. The standard library and its style

A language is learned from its grammar and lived in through its library.
Most of the code a programmer reads in Okkama is not `match` and `once let`
but calls into `std`: a file opened, a request answered, a date printed. If
those calls disagree with each other in shape, every module has to be learned
from scratch, and the economy the language bought with its small grammar is
spent again in the library. This chapter is about the shape the Okkama
library takes, why, and what it costs.

## 22.1 Why a library needs a style

By the end of 0.5.3, `std` had grown to 537 modules and 6.7 MB of source. Of
those, 404 were ordinary code, 64 were the substrate that talks to the
runtime (`std/runtime` and the natives), and 69 were generated tables of
locale data. The library worked, and every module had tests, but its owner's
verdict before the 0.5.4 review was blunt: it was awkward to use, with
objects nested in "matryoshkas" that were hard to read and harder to find
one's way in, and it was inefficient, with odd structures and detours.

The review that followed (issue #352) did not add features. It read every
module against one written set of principles, `docs/std-principles.md`, and
kept, simplified or rewrote it in place; nothing was removed from the
library or moved into a package. The principles are the subject of this
chapter. They are not a style guide in the sense of where braces go; they
say what an object in the library is for, how it answers, and how much a
caller should have to write.

Libraries have always been where a language's ideas are tested. Smalltalk-80
shipped its class library as part of the system and taught the language
through it [@goldberg1983]. Java's first collections, `Vector`, `Hashtable`
and `Enumeration` in 1996, were joined two years later by a Collections
Framework designed as one piece, and the two styles have lived side by side
ever since. Python codified the tone of its library in a short list of
aphorisms [@peters2004], and the standard book on Go teaches the language
largely through its standard packages [@donovan2015]. Okkama's principles are its own, but the problem they answer
is old: a library that grows module by module, by different hands, drifts
into many small dialects unless something holds it together.

## 22.2 The reference: `json`

Three modules were rewritten first, as exemplars for the rest: `json`,
`fs/file` and the HTTP client. `json` is the one this chapter returns to,
because it is small enough to read in full and touches every principle.

{{example:22-json}}

Everything a caller does with a JSON document is in those twenty lines.
`ParsedJson(text).root()` parses once; the result is a `Json`, one immutable
value whatever its kind. Reading asks the value a question and gets an
Okkama value back: `order.int("id")` is an `int`, `order.get("customer")` is
another `Json`, `order.list("items")` an array of them. There is no cast and
no intermediate "node" object to unwrap. An optional member is read with
`maybe(name)`, which answers a `Maybe` for each kind, so the default is
written where the value is used: `order.maybe("note").text() else "none"`.

Writing is the same object built in code: `Json(JsonField("id", 4711), …)`
holds its members in order, and `string()` asks the runtime for the compact
text in one call; `pretty(2)` indents it. Numbers are exact, never `float`:
a JSON number reads back as an `int`, a `BigInt` or a `Decimal`.

Everything that goes wrong is a `JsonError`, and every case names the value
it is about by its RFC 6901 pointer [@rfc6901]. Asking for text where the
document has a number is `JsonError.wrong_kind("/id", …)`, a missing member
is `JsonError.missing`, a number that does not fit the integer asked for is
`JsonError.invalid_number`. Parsing is strict RFC 8259 [@rfc8259]: member
order is kept, a repeated name is an error rather than a silent overwrite,
and nesting stops at 128 levels unless the caller allows more.

## 22.3 Objects are mechanisms, not records

The first principle is stated in the owner's own words in the document that
governs the library: write object code, avoid accessors and mutators,
delegate to an object its own actions, and do not turn an object into a
nominal structure of data; it should be a working mechanism, a box that does
something and hands out a result.

Five rules follow from it.

- **Tell, don't ask.** Code that reads three fields of an object and
  computes something from them belongs inside that object, as an action.
  The idea is older than Okkama: it is the "Law of Demeter" of Lieberherr
  and Holland [@lieberherr1989], and the core of the object style the
  language takes from Elegant Objects [@bugayenko2016].
- **An object knows what it gives, and in what form.** A caller never wraps
  an answer in a decoder or a formatter to use it. A captured process
  answers its output as text through a cap, `done::Captured::output`, and
  as exact bytes through another, `done::Bytes::output`; an instant answers
  `instant.string()`; a date answers `date.days_until(other)`. Decoders and
  formatters still exist for the rare explicit case, and the object uses
  them inside its own actions.
- **An object reveals itself through a cap.** What callers may see of an
  object is a cap of computed fields, named for the part it shows:
  `page::Head::status`, `date::Calendar::year`, `range::Bounds::start`. A
  cap holds only what is already computed or trivially computable; work —
  opening a stream, parsing a body — stays an action: `file.text()`,
  `response.body()`.
- **Constructors bind; actions compute.** A constructor checks its
  arguments with `confirm`, so that an invalid object never exists, and
  binds them. Sorting, parsing and counting move into a hidden action whose
  `once let` keeps the answer (Chapter 13): `Sorted(xs)` binds `xs` and
  sorts on the first question. A constructor never calls other objects or
  touches the outside world.
- **No mutators, no records.** A "with" action that changes one field is a
  mutator in disguise; the library prefers the action the change was for,
  `money.plus(other)` rather than `money.with_cents(…)`. An object whose
  actions are mostly accessors is a record with a constructor, and the
  review either gave it behaviour or folded it into the object that used it.

Data that really is data still answers questions rather than exposing
columns: a parsed `Accept-Encoding` header answers
`header.allows("gzip")`, not a list of tokens and their weights.

## 22.4 Flat, short, one way

The second principle is about how much the caller writes. A common task
takes one to three lines; if it takes more, the module is missing an entry
point. Before 0.5.4, reading an integer argument took four constructors
nested in one expression:

```okkama
Number(Utf8Arguments(ProcessArguments()).item(0)).int()
```

After the review the command-line module hands the program a parsed call,
and the same question is one action, `call.int("port")`, which answers
`Maybe<int>`. Three or more constructors nested to do one thing is what the
principles call a matryoshka, and it is treated as a missing object, not as
a style to put up with.

Several rules keep the surface flat.

- **One way to do each thing.** A second way needs a reason written in the
  module's comment. The one second way allowed everywhere is a shortcut for
  a common verb, and a shortcut builds the general form and nothing else:
  `client.get(url)` is `client.response(Request(url: url))`.
- **Fold what only forwards.** An object that only passes calls on, or a
  view with two actions, joins its neighbour, and one constructor with
  defaults replaces a family of overloads. The rule has a limit: an object
  with twenty-five actions is no improvement.
- **Named arguments and safe defaults.** Constructors in std take named
  arguments, `File(path: Path("notes.txt"), mode: FileMode.appending)`,
  except a constructor of a single value, where a name adds nothing:
  `Path("notes.txt")`. Every part with a sensible default has one, and the
  defaults are the safe ones. `HttpClient()` works as written, reaches only
  public endpoints and follows no redirects; a special case names only what
  differs.
- **Named numbers are enums.** An HTTP status with a registered name is a
  case, `Status.not_found`; the rest of 100–599 is `Status.other(418)`. One
  converter, `StatusCode`, goes to and from the wire, and it refuses `other`
  for a number that has a name, so every status has exactly one spelling.
- **`for`, not closures.** A sequence is walked with `for source -> item`.
  There is no `each(fn)`: a JSON object is walked by its names,
  `for doc.names() -> name { … }`, and rebuilt with `map`.
- **One name, one meaning.** An action never returns an iterator in one
  module and an array under the same name in another. Actions that answer
  are nouns, `lines()`, `content()`; verbs are commands that answer
  nothing, `write`, `close`. An object that computes one thing has no
  `value()` or `result()`: its action names the answer,
  `DaysBetween(left: a, right: b).count()`, `Crc32(bytes).checksum()`, or is
  an adjective when the answer is yes or no, `FileEncoding(name).supported()`.
  When the review started, 406 actions answered under a generic name; they
  were renamed module by module.
- **No dependency cycles.** A domain's errors live in a module below
  everything that throws them, such as `fs/error`, and a lower concept never
  imports a higher one.

Each of these rules alone would be a matter of taste. Together they make
the library predictable: a reader who has met `json` can guess the shape of
`http` or `time` before opening them, which is the reason to have a style at
all.

## 22.5 Readable and efficient

Two shorter principles keep the library honest about what it costs to read
and what it costs to run.

**Readable.** Every module is formatted by `okkama fmt`, so a reader never
meets a module written as one long line. Each public object carries a
comment with a short example of its common use, and the examples are
checked to compile. Names say what an object does and what an action does
to the world or returns; there is no `Manager`, `Helper` or `Util`, the
words a library reaches for when it has not decided what an object is for.
The comments are also the source of `okkama man`, described later in this
chapter, so a
module that is documented in its source is documented everywhere.

**Efficient.** The library is the code most programs spend their time in,
so its costs are everyone's costs. Four rules govern them.

- *No quadratic growth.* Collections are built with builders, `reduce`
  accumulators or the persistent arrays and maps, never by copying in a
  loop. The runtime makes `acc + [x]` inside `reduce` grow in place
  (section 8.5), but a module does not rely on a reader knowing that.
- *Measured.* A hot path has a micro-benchmark, and a change is judged on
  the release build, not on a debug or gate profile whose code layout
  exaggerates small differences.
- *Natives only where measured,* or for what Okkama cannot do itself:
  cryptography, TLS, X.509 and PEM stay native; a parser written in Okkama
  stays in Okkama unless a measurement says otherwise.
- *No fixed cost per call.* A table, a pattern or a template is parsed
  once, when its object is built, not every time an action uses it.

The two principles meet in the runtime underneath. The 0.5.7 release, for
instance, changed no public API of `std`, yet it
made `join`, ranges in array literals, map literals and the future
collections of `async/collect` several times faster on large inputs: the
library's shape stayed put while its cost moved. A style that keeps the
surface small is what lets that happen without breaking the programs above
it.
