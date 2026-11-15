# Chapter 25. Building a web service

A command-line tool runs once and exits. A web service starts and keeps
answering, many requests at a time, until someone stops it. This chapter
builds a small one: a JSON API for notes, with the four operations most
services begin with, list, read, create and delete:

```text
GET    /api/notes        a page of notes: {"items": […], "next": "…"}
POST   /api/notes        create one from {"title": "…"}; 201 and its location
GET    /api/notes/:id    one note, or 404
DELETE /api/notes/:id    remove it; 204, or 404
```

The previous chapter's habits carry over: decide in values, keep the outside
world behind a role, test without the real thing. What is new is
concurrency. Requests overlap, so state shared between them needs an owner
(Section 26.8), and every endpoint is an `async` action (Chapter 20). The
chapter builds the service over memory first, then puts PostgreSQL behind
the same role without touching a line of the API.

## 25.1 What a service is made of

HTTP is a protocol of messages. A request carries a method, a target, header
fields and perhaps a body; a response carries a status, header fields and
perhaps a body [@rfc9110]. A service is a function from requests to
responses that keeps some state between them. The standard library's
`web` modules give that function a shape with four parts, each of them a
value:

- **A route table** (`web/routing`) says which method and path reach which
  endpoint. It is declared, like `cli/command`'s commands in Chapter 24, and
  it is checked when it is built: two routes with the same shape are refused,
  a known path with the wrong method answers 405 with `Allow`, and `HEAD`
  follows `GET`.
- **An endpoint** is any bound action of type
  `async (WebRequest) is Responding`. A controller object keeps its
  dependencies in its fields and offers one action per operation.
- **A request** (`web/request`) gives the route parameters, the query, the
  header fields and a body read under a byte limit, each typed.
- **A reply** (`web/reply`) is a value that knows how to become a response:
  `JsonReply`, `CreatedReply`, `NoContentReply`, `TextReply` and others, each
  with the defaults of its kind.

The server itself (`web/app`) binds the table to an address and serves it.
It is the only part that touches the network, and the tests in this chapter
never start it.

## 25.2 The service

The notes service is one module. Its parts are shown together, then taken
one at a time.

{{example:notes/api}}

- **`Note`** is a value with an identifier and a title, and knows its JSON
  form.
- **`NoteStore`** is the role for storage. The API depends on the role, so
  memory today and a database tomorrow are two implementations of one
  contract (Section 26.5). Its actions are `async`, because a database
  answers later; Section 25.9 shows that doer.
- **`StoreRefusal`** is what a store may refuse: a title that is already
  taken. The store knows the rule; it does not know HTTP.
- **`MemoryStore`** keeps every note in one `Shelf` value inside an `Atomic`.
  Section 25.8 explains why it is built that way.
- **`NotesApi`** is the controller. Each endpoint reads what it needs from
  the request, asks the store, and returns a reply.
- **`StoreRefusals`** is an exception scope: it turns the store's refusals
  into statuses (Section 25.6).
- **`NotesService.routes()`** is the route table: one place that says which
  URL reaches which action. `Scope("/api", …)` writes the prefix once, and
  `Resource` writes a path once for several methods.

## 25.3 Endpoints

An endpoint reads the request through typed accessors.
`request.path().int("id")` reads the `:id` of the pattern as an `int`; a value
that is not a number is answered 400 by the framework before the endpoint
continues, so `/api/notes/seven` never reaches the store. The query and the
header fields read the same way. A parameter that may be absent is read with
`maybe`, which gives a `Maybe` (Chapter 7), so a default is one `else`:
`request.query().maybe("limit").int() else 20`. Absent means the default;
present but malformed, `?limit=many`, is still a 400. The endpoint never sees
a string where it asked for a number.

The body is read with a limit: `await request.json(4096)` accepts at most
4096 bytes, requires a JSON media type and parses the document at once. A
body that is too large is a 413, a body of the wrong type a 415, a syntax
error a 400, and a member that is missing or of the wrong kind, read later
from the returned `RequestJson`, a 422 that names the JSON pointer. None of
these needs a line in the endpoint. Reading without a limit is not offered:
a service that accepts any body accepts a body larger than its memory.

Replies are values. `JsonReply(value:)` is 200 with `application/json`;
`CreatedReply(location:, value:)` is 201 with a `Location` field and the new
resource; `NoContentReply()` is 204 with no body at all. Each takes only what
differs from its defaults, so an endpoint states its outcome and not the
framing. A reply cannot be inconsistent: `TextReply(text: "", status:
Status.no_content)` is refused with `InvalidReply`, because a 204 cannot
carry a body, even an empty one. `NoContentReply` is the way to say
"nothing".

An endpoint ends with one reply or one thrown error, and nothing in between
writes to the network. `add` is typical: read the body, check the title,
ask the store, answer 201. It has no status codes for the body's own
failures, no header handling and no serialisation code, because each of
those belongs to a value that already knows it.

## 25.4 Lists come in pages

A list endpoint that returns every note works on the first day and fails
on a later one, when a client asks for a hundred thousand notes and the
service builds the whole answer in memory. A list in an API is therefore a
page: some items, and a way to ask for the ones after them.

There are two common ways to say "the ones after them". An *offset* page asks
for "items 40 to 59": `?offset=40&limit=20`. It is easy to write and has two
faults. A note created or deleted before position 40 while the client reads
shifts every later item by one, so the client sees one note twice or misses
one. And a database answers an offset by reading and throwing away the first
forty rows, so the fortieth page costs forty times the first. A *keyset* page,
also called a cursor page, asks for "items after the last one I saw":
`?after=40&limit=20`, where 40 is the identifier of the previous page's last
note. Creations and deletions elsewhere in the list do not move it, and a
database answers it from the index on the key, so every page costs the same.

`NotesApi.list` is a keyset page:

- `after` and `limit` come from the query, with defaults `0` and `20`. A
  `limit` outside 1..100 is a 400; a service sets its own upper bound,
  because a client that may ask for any number of items may ask for all of
  them.
- The endpoint asks the store for `limit + 1` notes. If it gets more than
  `limit`, there is a next page, and the extra note is not shown; this costs
  one item and saves a second query that would count.
- The answer is `{"items": […]}`, plus `"next": "/api/notes?after=2&limit=2"`
  when there is a next page. The client follows the link it was given and
  never builds one. When `next` is absent, the list is over.

The order is the store's, and it must be stable. `MemoryStore.after` filters
and takes from `notes().values()`, which is in identifier order because a map
keeps its keys sorted (Section 8.3): iteration order never depends on
hashing or on the order of insertion, so two calls with the same `after` see
the same sequence. A store over a database writes the same contract as
`ORDER BY id`.

The cursor here is the identifier itself, which is fine for a list ordered
by identifier. A list ordered by something else, a date or a title, needs a
cursor that carries every key of the order, and services usually make it
opaque (an encoded token) so that clients cannot build one by hand. The
endpoint changes; the shape, items plus a link, does not.

## 25.5 Failures become statuses

A service has the same three kinds of failure as a tool (Section 24.5), and
HTTP gives each a place.

**The client's mistakes** are 4xx statuses. An endpoint states its own with
`throw Exception.status(404, "no note 9")`, and `confirm … else throw`
turns a precondition into one (`confirm await store.removed(id) else throw
…`). The framework answers a thrown `Exception` with a problem-details body,
RFC 9457's standard JSON form for errors [@rfc9457]:

```text
HTTP/1.1 404 Not Found
Content-Type: application/problem+json

{"type":"about:blank","title":"Not Found","status":404,"detail":"no note 9"}
```

Clients get one shape for every error, and the `detail` goes to them, so it
must never carry internal diagnostics.

**Translated failures** of the layers below are the subject of the next
section: the store refuses in its own terms, and an exception scope turns
the refusal into a status.

**Bugs** are not caught. Any other error reaches the server, which answers
500 without a body. The client learns that the request failed and nothing
about why, which is both the safe answer and the honest one. A failure of
the database itself, a lost connection or a timeout, takes the same path: it
is not the client's mistake, and nothing the client could change would fix
it.

## 25.6 Exception scopes

The store refuses a duplicate title with `StoreRefusal.duplicate(title)`,
an error of its own. It could have thrown `Exception.status(409, …)`
directly, and it would have been wrong to: a store that speaks HTTP cannot
be used by a command-line import tool, and every other caller would have to
know that 409 means "taken". The refusal belongs to the store; the status
belongs to the service.

The translation is an object that does `ExceptionScope`. Its one action,
`scoped`, receives the request and the endpoint, runs the endpoint, and
answers what it throws:

```okkama
object StoreRefusals does ExceptionScope {
    StoreRefusals();

    async scoped(request is WebRequest, endpoint is async (WebRequest) is Responding) is Responding {
        let reply is Responding = try {
            await endpoint(request);
        }
        catch StoreRefusal.duplicate(title) {
            ProblemReply(status: Status.conflict, detail: "a note titled '" + title + "' exists");
        };
        reply;
    }
}
```

The table gives it a place: `Scope("/api", StoreRefusals(), …)` puts it
around every route of the scope. A scope can sit on the whole table, on a
`Scope` or on a `Resource`, at the level where its errors can occur, and the
rules are the ones of Section 10.12's boundary:

- **It catches what it knows.** `StoreRefusals` names one case of one enum.
  Every other error passes outwards unchanged, to the next scope or to the
  framework.
- **Outer before inner.** Exception scopes and arounds run outer before
  inner, and within one level in the order they are written. An error thrown
  by the endpoint meets the innermost scope first.
- **The framework is last.** After every scope has had its chance, the
  framework answers a thrown `Exception` with its problem reply, and lets
  anything else reach the server's 500.

A scope answers with a reply, `ProblemReply(status:, detail:)` here, which
gives the same RFC 9457 body as a thrown `Exception`. It could also throw
`Exception.status(409, …)` and let the framework answer; the reply is more
direct, and it lets a scope add header fields, a `Retry-After` for a quota
for example.

The test that checks it is a client's test: post the same title twice and
read a 409 with the problem's `detail`. When Section 25.9 replaces the store,
the refusal comes from a database constraint instead of a scan, and this
scope and this test stay as they are.

## 25.7 Code around endpoints

Some concerns belong to every request rather than to one endpoint: an access
log, authentication, a maintenance switch, headers that every reply carries.
`web/routing` puts them in the table with `Around`, at the level they apply
to: the whole table, a `Scope`, or a `Resource`. An around is any object that
does `Surrounding`: it receives the request and the rest of the chain as
`inner`, and may call it, work with its reply, or answer by itself.

The standard library brings `AccessLog`, `BearerAuth` and `Cors`. The example
below makes reading public and changing guarded, and adds its own around:

{{example:25-guarded-tests}}

- `Around(BearerAuth(token: …))` sits on the inner `Scope("/notes", …)` that
  holds only `POST` and `DELETE`, so the `GET` routes beside it are untouched.
  A request without `Authorization: Bearer s3cret` gets 401 with
  `WWW-Authenticate: Bearer`; the token is compared in constant time.
- `Maintenance` is the application's own around. While its switch is on it
  answers 503 itself and never calls `inner`, so no endpoint runs. The switch
  is an `Atomic<bool>`: an operator can flip it from another task, and every
  request sees one consistent value.
- Arounds run outer before inner, in the order they are written, so
  maintenance is decided before authentication, and authentication before
  the endpoint.

An around and an exception scope look alike and differ in what they see. An
around sees every request and every reply, including the problem reply of an
`Exception` thrown further in; an exception scope sees only the errors that
an endpoint throws, and only the ones it names. Logging and authentication
are arounds; translating a layer's errors is a scope.

An around is where cross-cutting code goes, and only there. An endpoint that
checks a token itself, or writes its own log line, is the "same `defer` around
every use" smell of Section 26.6 in HTTP form.

## 25.8 State shared between requests

Requests are served concurrently, so the store is shared by every task that
serves a request. Okkama does not let objects change in place, and a `Cell`
cannot cross into async code (Section 26.8). That leaves two shapes: an
owner task that receives messages, or an `Atomic`, one value that tasks
share and replace whole.

`MemoryStore` uses an `Atomic`, and keeps everything it owns in one value,
`Shelf`: the next identifier and the map of notes. Adding a note reads the
shelf, checks that the title is free, builds a new shelf with the note
added, and publishes it with `compare_and_replace(expected:, revised:)`,
which succeeds only if no other task published since the read. If one did,
the loop reads again and decides again. Deleting works the same way. Each
operation is one atomic publication of a complete, consistent value, so a
reader never sees a note without its identifier counted, and two concurrent
creations never get the same identifier.

The check and the publication are one decision. If `added` checked the title
against one shelf and published against another, two requests with the same
title could both pass the check. Because the check is inside the loop, a
retry checks again against the shelf it will publish over, and the rule
"titles are unique" holds for every value that is ever published.

When does the retry actually happen? Section 20.3 says that the code of a
task between two suspension points is never interleaved with another task's.
`MemoryStore.added` is `async` but never awaits, so two requests cannot
interleave inside it, and today its loop always succeeds the first time. The
loop is there for the version that waits: a store that asks a spam filter
about the title between reading the shelf and publishing has a suspension
point in the middle, other requests run there, and `compare_and_replace` is
what notices that the shelf moved. The loop costs one comparison when it
succeeds, and it keeps the store correct under a change that nobody will
remember to review for concurrency.

The owner-task shape of Section 26.8 suits stores that do slow work in
order, such as appending to a file. A store that already lives elsewhere,
a database, needs neither: it is a role whose implementation sends queries,
and the database is the owner.

## 25.9 A database behind the role

`NoteStore` was written for this moment. A store over PostgreSQL is another
doer of the same role, in a module of its own:

{{example:notes/postgres}}

- **It holds a pool, not a connection.** `DbPool` (`db/pool`) hands out
  connections and takes them back. Each action takes one, and
  `defer db.close()` returns it to the pool on every way out, a failure
  included (Chapter 11). Requests run concurrently, and one connection runs
  one statement at a time, so a store that held one connection would make
  every request wait for the others.
- **Parameters are never SQL text.** `$1` and `$2` are PostgreSQL's
  placeholders, and the values travel separately, as `DbInteger(id)` and
  `DbText(title)`. A title such as `'); DROP TABLE notes; --` is a strange
  note and nothing more.
- **The database decides uniqueness.** The table declares `title` `UNIQUE`,
  and an insert of a taken title fails with SQLSTATE 23505, which the driver
  reports as `DbFailure.detailed` with that code. `added` catches it, throws
  the role's `StoreRefusal.duplicate`, and rethrows every other failure with
  a bare `throw;` (Section 10.6). There is no read-then-write, and no race
  between two servers: the constraint is checked where the data lives.
- **Pages come from the index.** `WHERE id > $1 ORDER BY id LIMIT $2` is the
  keyset page of Section 25.4 in SQL. With the primary key's index, the
  thousandth page costs what the first does. `.all(count)` reads at most the
  rows it asked for.
- **Each read takes only what it needs.** `.maybe()` is at most one row,
  `.one()` exactly one, `.all(max_rows)` a bounded list (`db/row`). `removed`
  asks for the deleted row back, `RETURNING id`, so "was there a note" is the
  presence of a row and not a count to compare.

What did not change is the point. `NotesApi`, `StoreRefusals`, the route
table and every test of Section 25.10 are the same code. The duplicate title
is still a 409, because `StoreRefusals` translates the role's refusal,
whichever doer raised it. The role's actions became `async` once, when the
role was written, and the memory store pays little for it: an action that
never awaits runs its body to the end the first time it is awaited
(Section 20.3).

Wiring is a different `main` (Section 25.11), and it is the only place that
names PostgreSQL:

```okkama
let pool = DbPool(
    origin: PostgresConnector(PostgresDatabase(url: "postgresql://notes@127.0.0.1:5432/notes")),
    options: PoolOptions(minimum: 0, maximum: 8, idle_timeout: Duration(minutes: 5),
        lifetime: Duration(minutes: 30), acquisition_timeout: Duration(seconds: 5)),
    clock: SystemClock(),
);
defer pool.close();
let service = NotesService(api: NotesApi(store: PostgresStore(pool: pool)));
```

Building the pool opens nothing; the first connection is made when the
first request needs one. `PoolOptions` bounds the pool, at most eight
connections here, replaced after five idle minutes or thirty in service, and
a request that cannot get a connection within five seconds fails with
`TimedOut`, which reaches the client as a 500, not as a hang.

The tests of this chapter run over `MemoryStore`, in the book's sandbox, with
no database. They check the HTTP contract, which is the same for both
stores. What they cannot check is that the SQL is right: that `RETURNING`
names the columns `note` reads, that the constraint has the code the store
expects. That belongs to a second run of the same tests with `Fresh`
building a `PostgresStore` over a disposable database, in CI, where a
database is part of the environment. One suite, two stores: the role is the
contract, and both doers are tested against it.

## 25.10 Testing without a network

`web/testing` gives `WebTestClient`, which calls a route table the way a real
HTTP client calls a server, without a socket. A request goes through the
routes, their arounds and exception scopes, and comes back as the same
`ClientResponse` a real client reads:

{{example:25-notes-tests}}

- `Fresh().client()` builds the whole service over an empty `MemoryStore`, so
  every test starts from nothing and tests cannot see each other's notes.
- The tests check what a client sees: statuses (`::Head::code`), JSON bodies
  and the problem `detail`. They exercise the framework's own answers too, the
  400 for a non-number identifier or limit and the 404 for a missing note.
- The paging test follows `next` exactly as a client would, from the first
  page to the last, and checks that the last page has no link. It never
  builds a URL of its own after the first.
- Tests are `async test`, because endpoints are `async` actions.
- Nothing here listens on a port, so the tests run in parallel with others,
  in the book's sandbox, and in CI without a network.

The last test is about concurrency. `Poster` is one client creating one note,
and `spawn Poster(…).run()` starts it as a task of the current run
(Section 20.4); four of them run concurrently against one service, and
`AllFutures(origins:).results()` waits for all four. The test then reads the
list and expects identifiers 1 to 4. A lost update would show here: notes
are keyed by identifier, so two creations that got the same one would leave
three notes, not four.

As Section 25.8 explained, this store cannot lose an update today, because
nothing interleaves inside `added`. The test is still worth its lines. It
states the property that clients rely on, four creations make four notes,
in the terms they see it, and it keeps stating it when someone adds an
`await` to `added`. A test of concurrency that passes on correct code and
would fail on a plausible wrong change is the kind worth keeping; one that
only passes is not.

## 25.11 Running it

The program that serves the API for real is short:

```okkama
async main() is int {
    let service = NotesService(api: NotesApi(store: MemoryStore()));
    await WebApplication(service.routes()).listen(address: "127.0.0.1", port: 8000);
    0;
}
```

With PostgreSQL, `main` builds the pool and the store of Section 25.9 instead
of `MemoryStore()`, and keeps everything else.

`listen` serves until the process receives SIGTERM or SIGINT. Then the
server stops accepting connections, gives the requests in flight ten seconds
to finish, closes, and `listen` returns, so `main` can finish its own
cleanup: the `defer pool.close()` of the database version runs after the
last request has finished with its connection. A second signal ends the
process at once. A deployment that stops a service with SIGTERM therefore
gets a graceful shutdown without any code in the service.

The address is an IP literal; `web/app` does no name resolution. The port,
the address, the database URL and other settings come in the same layers as
a tool's (Section 24.8): a flag, then the environment, then a default, read
in `main` and passed down. A database password does not belong in a URL that
a log might print: `PostgresDatabase(url:, password:)` takes it separately,
as `SecretBytes`. HTTPS takes a TLS identity, `listen(address:, port:, tls:)`,
and then offers HTTP/2 and HTTP/1.1 to clients.

## 25.12 What the design costs

The service is longer than a script that would answer the same four requests
with a mutable dictionary and a handler per URL. What the extra length buys:

- **Every request is checked before it reaches the endpoint:** the route,
  the method, the path and query parameters, the media type and the body
  size. An endpoint starts with values that already make sense.
- **Errors have one shape** (RFC 9457) and the right status. A layer's
  refusals are translated in one place, and bugs are not dressed up as
  client mistakes.
- **Lists are bounded.** No request can make the service build an answer of
  every note, and pages stay correct while the list changes.
- **Shared state cannot be torn.** The store publishes whole values, and
  the language does not offer a way to share a mutable object between tasks.
- **Storage is replaceable.** The database arrived as a new doer of the
  role, and the API, the routes and the tests did not change.
- **The service is tested as its clients see it,** without a network and
  without a running server.

The memory store pays for it with a copy of the shelf on every change, and
with a scan of every title on every creation. For a store of a few thousand
notes both are invisible; for millions, the store belongs in a database
behind the same role, where the index does the scan and there is no copy.
The `async` endpoints and the `async` role cost a little per request compared
to synchronous ones; Chapter 20 explains what that buys, and Chapter 23 what
it costs.
