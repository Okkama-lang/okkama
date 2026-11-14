# Chapter 25. Building a web service

A command-line tool runs once and exits. A web service starts and keeps
answering, many requests at a time, until someone stops it. This chapter
builds a small one: a JSON API for notes, with the four operations most
services begin with, list, read, create and delete:

```text
GET    /api/notes        the notes, as a JSON array
POST   /api/notes        create one from {"title": "…"}; 201 and its location
GET    /api/notes/:id    one note, or 404
DELETE /api/notes/:id    remove it; 204, or 404
```

The previous chapter's habits carry over: decide in values, keep the outside
world behind a role, test without the real thing. What is new is
concurrency. Requests overlap, so state shared between them needs an owner
(Section 26.8), and every endpoint is an `async` action (Chapter 20).

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
  contract (Section 26.5).
- **`MemoryStore`** keeps every note in one `Shelf` value inside an `Atomic`.
  Section 25.6 explains why it is built that way.
- **`NotesApi`** is the controller. Each endpoint reads what it needs from
  the request, asks the store, and returns a reply.
- **`NotesService.routes()`** is the route table: one place that says which
  URL reaches which action. `Scope("/api", …)` writes the prefix once, and
  `Resource` writes a path once for several methods.

## 25.3 Endpoints

An endpoint reads the request through typed accessors.
`request.path().int("id")` reads the `:id` of the pattern as an `int`; a value
that is not a number is answered 400 by the framework before the endpoint
continues, so `/api/notes/seven` never reaches the store. The query and the
header fields read the same way, with `maybe` for values that may be absent
(Chapter 7).

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

## 25.4 Failures become statuses

A service has the same three kinds of failure as a tool (Section 24.5), and
HTTP gives each a place.

**The client's mistakes** are 4xx statuses. An endpoint states its own with
`throw Exception.status(404, "no note 9")`, and `confirm … else throw`
turns a precondition into one (`confirm store.removed(id) else throw …`).
The framework answers a thrown `Exception` with a problem-details body,
RFC 9457's standard JSON form for errors [@rfc9457]:

```text
HTTP/1.1 404 Not Found
Content-Type: application/problem+json

{"type":"about:blank","title":"Not Found","status":404,"detail":"no note 9"}
```

Clients get one shape for every error, and the `detail` goes to them, so it
must never carry internal diagnostics.

**Translated failures** of the layers below, a database refusing a
duplicate key or a quota being exceeded, are turned into statuses by an
exception scope around the routes that know them: a `Scope` or the table
takes an `ExceptionScope`, which catches the errors it knows and answers
them, and lets the others pass outwards. This is Section 10.12's
boundary in HTTP form.

**Bugs** are not caught. Any other error reaches the server, which answers
500 without a body. The client learns that the request failed and nothing
about why, which is both the safe answer and the honest one.

## 25.5 Code around endpoints

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

An around is where cross-cutting code goes, and only there. An endpoint that
checks a token itself, or writes its own log line, is the "same `defer` around
every use" smell of Section 26.6 in HTTP form.

## 25.6 State shared between requests

Requests are served concurrently, so the store is shared by every task that
serves a request. Okkama does not let objects change in place, and a `Cell`
cannot cross into async code (Section 26.8). That leaves two shapes: an
owner task that receives messages, or an `Atomic`, one value that tasks
share and replace whole.

`MemoryStore` uses an `Atomic`, and keeps everything it owns in one value,
`Shelf`: the next identifier and the map of notes. Adding a note reads the
shelf, builds a new one with the note added, and publishes it with
`compare_and_replace(expected:, revised:)`, which succeeds only if no other
task published since the read. If one did, the loop reads again and retries.
Deleting works the same way. Each operation is one atomic publication of a
complete, consistent value, so a reader never sees a note without its
identifier counted, and two concurrent creations never get the same
identifier.

The owner-task shape of Section 26.8 suits stores that do slow work, such as
writing to a file in order. A store that already lives elsewhere, a
database, needs neither: it is a role whose implementation sends queries, and
the database is the owner.

## 25.7 Testing without a network

`web/testing` gives `WebTestClient`, which calls a route table the way a real
HTTP client calls a server, without a socket. A request goes through the
routes, their arounds and exception scopes, and comes back as the same
`ClientResponse` a real client reads:

{{example:25-notes-tests}}

- `Fresh().client()` builds the whole service over an empty `MemoryStore`, so
  every test starts from nothing and tests cannot see each other's notes.
- The tests check what a client sees: statuses (`::Head::code`), JSON bodies
  and the problem `detail`. They exercise the framework's own answers too, the
  400 for a non-number identifier and the 404 for a missing note.
- Tests are `async test`, because endpoints are `async` actions.
- Nothing here listens on a port, so the tests run in parallel with others,
  in the book's sandbox, and in CI without a network.

## 25.8 Running it

The program that serves the API for real is short:

```okkama
async main() is int {
    let service = NotesService(api: NotesApi(store: MemoryStore()));
    await WebApplication(service.routes()).listen(address: "127.0.0.1", port: 8000);
    0;
}
```

`listen` serves until the process receives SIGTERM or SIGINT. Then the
server stops accepting connections, gives the requests in flight ten seconds
to finish, closes, and `listen` returns, so `main` can finish its own
cleanup. A second signal ends the process at once. A deployment that stops a
service with SIGTERM therefore gets a graceful shutdown without any code in
the service.

The address is an IP literal; `web/app` does no name resolution. The port,
the address and other settings come in the same layers as a tool's
(Section 24.8): a flag, then the environment, then a default, read in `main`
and passed down. HTTPS takes a TLS identity, `listen(address:, port:, tls:)`,
and then offers HTTP/2 and HTTP/1.1 to clients.

## 25.9 What the design costs

The service is longer than a script that would answer the same four requests
with a mutable dictionary and a handler per URL. What the extra length buys:

- **Every request is checked before it reaches the endpoint:** the route,
  the method, the path parameters, the media type and the body size. An
  endpoint starts with values that already make sense.
- **Errors have one shape** (RFC 9457) and the right status, and bugs are not
  dressed up as client mistakes.
- **Shared state cannot be torn.** The store publishes whole values, and
  the language does not offer a way to share a mutable object between tasks.
- **The service is tested as its clients see it,** without a network and
  without a running server.

The cost is a copy of the shelf on every change. For a store of a few
thousand notes it is invisible; for millions, the store belongs in a
database behind the same role, and the copy disappears. The `async`
endpoints cost a little per request compared to synchronous ones; Chapter 20
explains what that buys, and Chapter 23 what it costs.

