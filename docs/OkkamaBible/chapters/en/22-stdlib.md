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

Writing uses the same object built in code: `Json(JsonField("id", 4711), …)` holds members in order; `string()` returns compact text and `pretty(2)` indents it. Numbers are exact, never `float`: a JSON number reads as an `int`, a `BigInt` or a `Decimal`.

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
- **An object reveals itself through a cap** (Section 12.8). What callers
  may see of an object is a cap of computed fields, named for the part it shows:
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

- *No quadratic growth.* Build collections with builders or `reduce`; `acc + [x]` in the supported accumulator pattern has amortized constant append cost (Section 8.5).
- *Measured.* Measure hot paths on release builds.
- *Natives where needed.* Use them for host services or demonstrated performance needs; keep ordinary parsing in Okkama unless measurements justify otherwise.
- *No fixed cost per call.* Parse a table, pattern or template once when its object is built, not at every use.

## 22.6 Explicit authority, no global state

Chapter 21 showed the mechanism: a directory, an environment and a clock
are objects, and code can touch only the ones it was given. For the
library this is a rule with four parts.

- What a module may touch arrives as a value: a `DirectoryLocating` for
  where it writes, an `Environment` snapshot for what it reads from the
  process, a `Timing` for what time it is.
- No process-wide mutable state. A module keeps nothing between calls that
  another module could change: no registry of handlers, no default locale
  set from somewhere else, no cache another part of the program can fill.
- No `chdir`-like operations, nothing that changes what a relative name
  means for everyone at once.
- Tests can pass fakes and manual clocks, because nothing in a module
  reaches past its parameters for the real ones.

The common case stays short through a default, not through an ambient
source. Sixteen signatures in std, across the HTTP client and server,
JWT, logging, mail and web sessions, take a clock as
`clock is Timing = SystemClock()`: the JWT token checks its expiry with
it, the mail message stamps its date, the HTTP client measures its
timeouts. A program writes `HttpClient()` and gets the system's clock; a
test writes `HttpClient(clock: ManualClock(…))` and gets one it controls.
The default is visible in the signature, and `okkama man` prints it, so
the authority is still a value: it is just one the caller did not have to
spell.

The rule is the library's, not the language's (Section 21.4): a program
can still construct a `SystemClock()` anywhere it likes. What the rule
buys is that the library itself never does so behind its caller's back.

## 22.7 Failures and asynchrony

A library speaks to its caller twice: in what it returns and in what it
throws. The second voice gets the same care as the first.

**One error type per domain.** Each domain has one error enum, declared as
`enum X is Error = (…)`, whose cases say what went wrong: a missing file is
`FileError.not_found(path)`, a JSON value of the wrong kind is
`JsonError.wrong_kind(pointer, …)`, a status number with a registered name
given as `other` is `InvalidStatus.named(code)`. The cases carry what the
caller needs to act — the path, the pointer, the refused value — and never
only a message to be parsed. The enum lives in a module below everything
that throws it, `fs/error` below `fs/file` and `fs/directory`, so throwing
it never creates an import cycle.

**Natives are translated at the border.** The runtime reports a failed
host call as a `NativeFailure` with a short code such as `"not_found"` or
`"would_block"`. That type never leaves a module: the module that called
the native catches it and throws its own domain's error, or, when the code
means something ordinary, turns it into an ordinary answer. `File.lock`
does both: `"would_block"`, another lock in the way, becomes `Maybe.none`;
anything else goes through `FileFailure`, in `fs/error`, which turns it into
the `FileError` that names the path.

**Async only where something waits.** An action is `async` when it waits
for I/O, a timer or another task, and only then; a parser or a formatter is
never `async`, however large its input. Releasing a resource at the end of
a scope is still written `defer await guard.close()` where the close
waits; whether those guards become `async destroy` is an open question of
its own (issue #329), to be decided when `async destroy` costs no more than
the deferred close.

## 22.8 Tests

The library is tested where it is written. Every public object has at least one inline `test` (Chapter 15). Before a module changes, characterization tests pin its current behaviour, so a simplification that changes a public result is caught by a test.

Network and asynchronous code is tested against itself rather than against
the outside world: a test opens a `TcpListening` on 127.0.0.1, port 0, and
plays the peer in the same program. Such tests run in a container, like the
gate, so that a test that misbehaves cannot reach the machine around it.

Modules people use directly also have small task examples under `examples/`, each with expected output checked by `examples/check.py`. A test checks correctness; an example checks usability and the documented way to use the library.

## 22.9 Scope: what the library holds

The standard library of Okkama is large on purpose. It holds an HTTP client
and server, a mail stack, TLS, SQLite and PostgreSQL, templates, archives,
locales and a terminal toolkit, beside the collections and the text. A
language this young could have kept its library small and left the rest to
packages; Okkama decided the other way, for a reason specific to it. The
style of this chapter is only worth something if it holds across the code
a program actually calls, and a library that stops at `json` would leave
most of that code to be written in whatever style its authors chose.

The modules fall into two layers.

- **The foundation** is what everything stands on: `encoding`, `io`,
  `collections`, `compare`, `error`, `async`, `numbers`, `strings`, `time`,
  `fs`, `os`, `cli`, `url`, `json` and `regex`. It is imported most: in
  the release this book describes, `encoding/utf` is imported by 148 of
  std's 499 modules and `collections/sequence` by 113. A mistake here is
  everyone's mistake, so the foundation was reviewed first.
- **The domains** are the large areas built on it: `http`, `mail`, `mime`,
  `i18n`, `cli`, `testing`, `db` and the rest. Before the review they were
  in the worst shape: `http` alone had 102 modules and 543 accessors, `mail`
  236.

One decision shapes the whole review, and it was the owner's, made at its
start: nothing leaves std. No module was deleted or moved into a package;
the long tail — `mail`, `plist`, `xmlrpc`, `archive`, `xml`, `html`,
`template`, `diff`, `statistics`, `calendar` — was improved in place like
everything else. Two modules that did the same thing were merged, keeping
what each could do. A program written against std before the review finds
every capability it used still there, under the shape this chapter
describes.

## 22.10 Reading like json, everywhere

The `json` reference of Section 22.2 did more than fix one module. Its way
of reading — ask a value for a member by name, get back an Okkama value of
the kind you asked for, with `maybe(name)` for what may be absent — became
the vocabulary of every structured source in std.

| source | a required value | an optional one | when it is missing |
|---|---|---|---|
| JSON | `doc.int("id")` | `doc.maybe("note").text() else "none"` | `JsonError.missing` |
| URL query (#134) | `query.int("page")` | `query.maybe("limit").int() else 20` | `MissingQueryParameter` |
| TOML (#140) | `config.get("server").int("port")` | `config.maybe("debug").boolean() else false` | `MissingTomlMember` |
| plist (#141) | `info.text("CFBundleIdentifier")` | `info.maybe("LSUIElement").boolean() else false` | `MissingPlistMember` |
| CSV (#202) | `record.int("age")` | `record.maybe("note").text() else ""` | `MissingCsvValue` |
| environment | `env.text("HOME")` | `env.maybe("PORT").int() else 8080` | `MissingEnvironmentVariable` |

The readers have the same names and the same meanings in all six: `text`,
`int` and `boolean` read one typed value and throw the domain's own error
when it is not there or is not of that kind; `list` reads all values of a
name; `get` steps into a nested value; `maybe` turns the question into a
`Maybe`, so the default sits where the value is used. Each domain keeps
what is its own — a query parameter may be given twice and `list` reads
every occurrence, a CSV field that is empty reads as absent, TOML has
arrays of tables walked with `for config.tables("replica")` — but none of
them invents a second word for the same act.

The payoff is the one Section 22.4 promised. A reader who has seen
`doc.maybe("note").text() else "none"` reads a configuration file, a query
string and an environment the same way, without opening a manual.

## 22.11 Whole or in pieces

A source that can be read whole and in pieces says so with the same words
everywhere. A whole text file is `content()`, its lines are `lines()`, and
`for file.text() -> batch` walks it in batches; a reply is
`await page.text()`, `bytes()` or `json()` whole and `reading()` in pieces.
The words do not change from module to module, and neither do the rules
behind them.

**One loop for every stream.** A consumable source that is not an array
does the role `Batching<T>`: it hands out its items a batch at a time, an
array per batch, and an empty array ends it. `for`, `map`, `filter` and
`reduce` walk such a source as they walk an array, asking for the next
batch when one runs out (#465). A file of text, a socket and a stream of
directory entries are all walked with the same `for`, and none of them has
to allocate an object per item, as a cursor would.

**Read to the end, carry across.** Two objects in `io/transfer` replace the
read loop modules used to repeat (#130): `ReadUntilEnd(origin: reader,
limit: …)` reads a source to its end and refuses one longer than the
limit, and `Copy(from: reader, to: writer)` carries every byte across, 64
KiB at a time, and answers how many it carried. Both accept a synchronous
or an asynchronous source; neither flushes nor closes anything it was not
asked to.

**Structured data in pieces.** `json` reads a stream item by item with
bounded memory (#136): `for await JsonItems(origin: upload) -> item` walks
the elements of one top-level array or a sequence of values (NDJSON), each
item parsed whole and read as in Section 22.10, and `JsonLines` writes
values one per line. A broken stream is a `JsonError` at the byte offset
where it broke, so a log of a million records fails at the one that is
wrong, not at an offset nobody can find.

**Writing is one operator.** Anything that has bytes — a string, an
`@byte`, an encoded value — is a `ByteSource`, and `writer <- part` writes
it; a chain such as `writer <- "header: " <- payload <- "\n"` reads left
to right and stops at the first failed write. A write takes the whole
source or throws `IncompleteWrite.after(written)` with the number of bytes
taken; it never retries, flushes or closes behind the caller's back.

## 22.12 `okkama man`: the reference lives in the source

The library is documented where it is written. A module's comments are
plain `//` lines in CommonMark, placed above what they describe: the
module's opening comment explains what it is for and shows the common use,
and each public object and action carries a sentence or two of its own.
There is no second copy of the reference to keep in step. `okkama man`
reads the comments and the declarations together and prints a page (#512):

```text
$ okkama man json
NAME
    json - JSON values: one immutable Json, parsed or built in code, read
    without casts and serialized as compact text.

SYNOPSIS
    let order = ParsedJson(text).root();
    order.int("id");                                // 4711
    order.maybe("note").text() else "";             // "" when absent or null
    …
```

A page has the parts a Unix manual page has: the name and its one-line
summary, the version, a synopsis taken from the module's own example, the
description, the public API — every object, role and action with its
signature, defaults included — and the errors the module throws. `okkama
man json Json.int` narrows the page to one action; `okkama man` alone lists
std; `--format markdown` gives the same page as Markdown, and a module of
the program's own, `okkama man ./src/app.ok`, is documented the same way.
The language server shows the same text on hover, from the same comments,
so a programmer meets one description of an action whether they read the
source, the terminal or the editor.

The rule this sets for the library is short: a public name without a
comment is a defect, and an example in a comment is code that must
compile. Section 22.5 asked for both; `okkama man` is what makes the asking
worth it.

## 22.13 Other libraries, other answers

Every library answers the same few questions, and comparing the answers
shows what Okkama's are for.

*How does a caller read structured data?* Go's `encoding/json` decodes
into a struct whose fields carry tags, so the shape of the data is declared
once, as a type, before the data arrives [@donovan2015]. Python's `json`
returns dictionaries and lists, so the shape is never declared and every
read is a lookup that may fail anywhere. Okkama sits between: `Json` is one
value of any kind, and the caller declares the shape at the point of each
read — `int("id")`, `list("items")` — with the error naming the pointer
where the shape did not hold. There is no struct to keep in step with the
data, and no untyped dictionary to cast.

*Where do defaults live?* Java's libraries long answered with overloads
and, later, builders [@bloch2018]; Python with keyword arguments and
defaults in the signature. Okkama took Python's answer and made it the
rule: named arguments with defaults, so the common case is the empty
constructor and a special case names only what differs (Section 22.4).

*How does the library reach the world?* Most libraries reach it directly:
`time.Now()` in Go, `datetime.now()` in Python, `System.currentTimeMillis()`
in Java. Testing such code means patching the world. Okkama's library asks
for the world as a value (Section 22.6), which is the object-capability
answer, and it is the one place where std is stricter than all three.

*How many ways are there to do a thing?* "There should be one — and
preferably only one — obvious way to do it" is a line of the Zen of Python
[@peters2004], which the Python library itself does not always keep.
Okkama writes the rule into the library's principles and makes a second
way justify itself in the module's comment, which turns a slogan into a
review checklist.

*What is an object for?* Here Okkama is furthest from the mainstream and
closest to Smalltalk [@goldberg1983] and to Elegant Objects
[@bugayenko2016]: an object is a mechanism that answers questions, not a
record with getters (Section 22.3). The mainstream libraries are full of
data carriers, and they work; Okkama's bet is that a library without them
reads better and changes more safely, and the review of 0.5.4 was the
first large test of that bet.

## 22.14 What the style costs

A style that removes accessors, overloads and second ways has costs, and
they should be named.

- **More objects.** A question that another library answers with a field
  read becomes an object with an action: `DaysBetween(left: a, right: b)`
  where Python writes `(b - a).days`. Each is small, but there are more of
  them to name and to find, and `okkama man` is part of the answer to the
  finding.
- **Less familiar shapes.** A programmer arriving from Java looks for
  `getStatus()` and finds a cap, `page::Head::status`, or an action named
  for its answer. The shape is learnt once, but it is learnt.
- **Renames have a price.** Making one name mean one thing renamed 406
  actions across std in one review. Programs written before 0.5.4 had to
  follow; the library pays this kind of cost in bursts rather than in
  small drifts, and it is why the review happened before a 1.0 that would
  have frozen the names.
- **The rules are a discipline, not a proof.** The compiler enforces the
  language — private fields, no downcasts, typed failures — but not that a
  module asks for its clock or reads like `json`. Those hold because the
  library is reviewed against written principles, and a module written
  outside std can ignore them.

None of these costs is large next to the alternative, a library that every
programmer has to learn one module at a time. But they are the reason the
principles are written down: a style that is only remembered drifts, and a
library of 499 modules drifts fast.

## 22.15 Codecs: one object per direction

The encoding modules show the style at its smallest. Every codec is two
objects, named for what they hold once the work is done:
`Base64Encoded(origin: bytes).string()` is the text, and
`Base64Decoded(origin: text).bytes()` the bytes; `Utf8Encoded`,
`Utf8Decoded`, `QuotedPrintableEncoded`, `Base85Decoded` and the rest
follow the same shape. Choices that vary — an alphabet, a byte order — are
named arguments with a default (`Base64Alphabet.url`, `ByteOrder.little`),
never a second family of objects.

Decoding is strict. Input that is not exactly the encoding fails with the
domain's error, carrying the offset of the first violation:
`InvalidBase64.at(offset)`, `InvalidUtf8.at(offset)`. Offsets count Unicode
scalars for text inputs and bytes for byte inputs, so they point at what a
person sees in the input. There is no "lenient" mode that repairs, skips or
replaces: a decoder that guesses produces data nobody asked for.

The objects are thin; the work is done by natives, and how well it is done
is not part of the API. 0.5.8 rewrote most of them without changing a
single result: base64 and base32 decode through a lookup table in one pass
instead of a linear search of the alphabet per character (64 base64
characters: 4.2 → 0.37 µs), base85 likewise (64 KiB: 3.4 → 0.29 ms),
quoted-printable encodes in one pass instead of moving every token of a
line (one 64 KiB line: 1.18 s → 0.3 ms), and the validity checks no longer
decode into a buffer they throw away. Each rewrite was first pinned by
tests that pass on the old code — the published behaviour, byte for byte,
was the specification — and then measured. That is the library's
efficiency principle at work: the surface stays put while the cost moves.

## 22.16 A worked example: time zones without the host

Time zones show the library's rules under the most pressure. The rules change
by government decree, the host's database may be out of date, and local
times can be missing or repeated. `time/zone` answers every question from a
database bundled with the toolchain and never from the machine it runs on.

{{example:22-zones}}

- **Deterministic answers.** `Zone("Asia/Qostanay")` reads the bundled IANA
  database (2026c) and never the host's configured zone or files. The same
  program gives the same answer on every machine, and
  `TimeZoneDatabaseVersion()` says which rules it used. That matters here:
  Kazakhstan moved Qostanay and Almaty from UTC+6 to UTC+5 on 1 March 2024,
  a change shipped in tzdata 2024a [@iana2024a]. A host with older data
  would place the 19:00 call an hour off. With the database bundled, the
  rules change only when the toolchain does, and the change is visible.
- **The zone is a value.** Converting an instant to local time takes a
  `Zone` argument. There is no "local" default to read from an environment
  variable (Section 22.6).
- **A missing local time is a failure, not a guess.** On 8 March 2026,
  New York's clocks jump from 02:00 to 03:00, so 02:30 does not exist that
  day. `instant` throws `MissingLocalTime` instead of choosing a nearby
  moment. A repeated time in autumn takes the earlier instant unless the
  call says `fold: Fold.later`, so the choice is written in the code.

Other libraries make other choices on each of the three points.

- **Python.** `zoneinfo` reads the system database, or the `tzdata`
  package where the system has none [@pep615]. A wall time in a gap is not
  an error: PEP 495's `fold` attribute picks the offset from before or
  after the transition [@pep495].
- **Java.** `java.time` ships its own copy of tzdb with the runtime, updated
  with the JDK. `ZonedDateTime.of` resolves a gap by shifting the local time
  forward by the length of the gap [@javazdt].
- **Go.** `time.LoadLocation` reads the host's zoneinfo. A program that
  imports `time/tzdata` embeds a copy instead [@gotzdata].

Okkama takes Java's bundled database and refuses Java's and Python's
silent adjustment of a gap. The cost is a `try` around conversions of user
input, and a toolchain update to receive new rules. What it buys is the
library's promise in a domain where it is hardest to keep: an answer that
does not depend on where the program happens to run.
