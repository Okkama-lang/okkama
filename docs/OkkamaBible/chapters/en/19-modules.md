# Chapter 19. Modules, imports, packages and blade

A program of any size is several files, and every language has to answer the
same questions about them. What is the unit? How does one unit name another?
What crosses the boundary between them, and what stays inside? Where is an
imported file found, and how is a library from somebody else put next to the
program? Okkama's answers are short. A file is a module. An import names
another file by its path and lists what it takes. Only what a module declares
itself crosses its boundary, and only if it is not `hidden`. Nothing runs
because something was imported.

Here is a program made of three files. The first is a module that keeps an
amount of money in cents:

{{example:shop/money}}

The second keeps a basket of prices. It sits in the same directory and
imports the first by its file name:

{{example:shop/basket}}

The program imports both:

{{example:19-shop}}

- `import "shop/money" take Money, CURRENCY;` brings two names into the
  file as they are: `Money(cents: 1250)` and `CURRENCY`.
- `import "shop/basket";` brings no bare name. It makes a namespace named
  after the last component of the path, and `basket.Basket(…)` reaches the
  declaration through it.
- `import "shop/money" as cash;` makes a namespace under a chosen name, and
  `cash.Money` and `cash.CENTS` are reached through it. The module was
  already imported by the first line; importing it again costs nothing and
  creates nothing twice.
- The basket's `Money` and the program's `Money` are one type. `basket.ok`
  imports `"money"`, which is found beside it, and that is the same file as
  the program's `"shop/money"`. One file is one module however its path is
  spelled.
- `money.ok` has a `main()` that prints `money: main`. The output does not
  contain it: an imported module's `main()` never runs.
- `Rounding` is `hidden`. `Money` uses it, and nothing outside `money.ok` can
  name it.

This chapter covers what a module is, the three forms of import, where an
import is found, what crosses a module's boundary and what does not, hidden
declarations, module constants, packages and the `blade` tool, where these
ideas came from, how they compare with other languages, and what the design
costs.

## 19.1 A module is a file

Every Okkama source file begins with a module declaration:

```okkama
module app v1.0.0;
module money/cash v1.0.0;
module money/cash;
```

The name is one or more lower-case identifiers separated by `/`. The version
is optional; when present it is `vMAJOR.MINOR.PATCH`, and a module without
one has major version 0. The declaration must be the first thing in the file,
before the imports, and the imports must come before every other
declaration. That order is fixed so that a reader learns what a file is and
what it depends on from its first lines.

The name a module declares and the path other files import it by are two
different things. An import names a *file*: `import "shop/money"` is
`shop/money.ok`, found by the rules of 19.3. The declaration names the
*module*: `shop/money` with major version 1. The two usually agree, and
nothing forces them to. What matters is the declaration, because it is part
of every type the module declares. Inside the runtime, `Money` from this
module is `shop/money#1::Money`: the module, its major version and the
declared name. The prefix appears in no ordinary output and no diagnostic,
but it decides which types are the same (19.4).

### What a version means

Only the major part of a version reaches the language. `v1.4.2` and `v1.5.0`
of `shop/money` declare the same types: a program built against one can be
rebuilt against the other, and every `Money` stays a `Money`. `v2.0.0`
declares new ones. `shop/money#2::Money` is not `shop/money#1::Money`, so
code that passes a value of the old major where the new one is expected
fails to compile, at the exact place where the two meet. That is the
compiler's reading of Semantic Versioning [@semver]: a major version is a
promise broken on purpose, and the type system makes the breakage visible
instead of letting it surface as a missing method at run time. The minor and
patch parts are for people and for `blade` (19.7), which uses them to choose
among releases.

A module's `main()` is the program's entry point when the module is the one
being run, and it is nothing at all when the module is imported. There is no
`if __name__ == "__main__"` test to write, because importing never runs
anything: not `main()`, not a static initializer, not the top level of a
file, because a file has no executable top level. The only things a module
contains are declarations. That is the first and most important difference
from scripting languages, where importing a module runs its code: an import
in Okkama is a fact about names, and adding one cannot change what a program
does.

### Multi-file modules

A module can span several files. `okkama run main.ok part.ok` (or `okkama
build` with several files) combines them into one declaration space, under
three rules.
- **One name, one version.** Every file declares the same module name and the
  same version, or every file no version; a mismatch is an error.
- **Imports stay with their file.** Each file keeps its own imports: a name
  that `part.ok` takes is not visible in `main.ok`, even though their
  declarations are visible to each other. A file states its own dependencies
  and reads correctly on its own.
- **No order.** The files are taken in name order, so the order they are given
  in changes nothing. A name declared in two of them is an error reported in
  both, and the combined module has at most one `main()`.

There is no directory-as-module rule, as in Go: the files of a module are the
files the command names, or the files `blade.toml` lists (19.7).

## 19.2 Three forms of import

An import line has one of three forms, or the last two combined.

| Form | What it brings into the file |
|---|---|
| `import "shop/basket";` | a namespace `basket`: `basket.Basket(…)`, `basket.Basket` as a type |
| `import "shop/money" as cash;` | a namespace `cash` instead of `money` |
| `import "shop/money" take Money, CURRENCY;` | the bare names `Money` and `CURRENCY`, and no namespace |
| `import "io/stdout" as out take Stdout;` | a namespace `out` that contains only `Stdout` |

A namespace works in expressions and in types alike: `basket.Basket(prices:
…)` constructs, and `amount is shop.Money` declares a parameter. Generic
types are applied through it too: `boxes.Box<int>`.

There is no fourth form. Okkama has no wildcard import: no `import "shop/money"
take *`, no Python `from money import *`, no Java `import money.*`, no
Haskell or OCaml `open`. The reason is the reader. In an Okkama file every
bare name is declared in that file or listed on an import line, so the origin
of a name is always on the same page as its use. A wildcard moves that
knowledge into another file, and changes its meaning whenever that other
file gains a declaration. Two wildcard imports can make a name ambiguous
that was not ambiguous yesterday, without a line of the importing file
changing.

The cost of the rule is long `take` lists in files that use many
declarations of one module. The usual answer is the namespace form: one
import line, and `cash.Money` at each use.

### One name, one source

A bare name is declared once in a file's scope. Taking the same name from two
modules is an error that names both:

```okkama
import "shop/money" take Money;
import "bank/money" take Money;
```

```text
semantic error: Object "Money" is already declared by module "shop/money";
this import takes another from module "bank/money"
```

The same holds for a `take` against a declaration of the file itself, and
for a namespace name bound twice. Namespaces are how two same-named
declarations live in one file: `import "shop/money" as shop;` and `import
"bank/money" as bank;` give `shop.Money` and `bank.Money`.

Importing the same module twice is not a collision. The second import
shares the first one's module: nothing is loaded or registered twice, and a
module reached through two different paths of the import graph, a
*diamond*, is still one module.

### Standard library paths

A declaration of the standard library can also be named in place, without
an import, by a path starting with the reserved word `std`:

{{example:19-std-path}}

The segments up to the first capitalized one are the module path
(`std.io.stdout.Stdout` is `Stdout` from `io/stdout`); a path of lower-case
segments only names a native action and must be called
(`std.runtime.debug.data_dump(order)`). Such a path behaves exactly like a
namespace import of that module made by the same file, and resolves only in
the bundled standard library. It is meant for the one-off use. A file that
writes `std.io.stdout.Stdout()` in five places is better off with an import
line, so its dependencies are listed where a reader looks for them. Because
the word is reserved, `let std = 1;` is an error:

```text
semantic error: "std" is reserved for standard library paths (std.io.stdout.Stdout)
```

### Naming a module

Because a plain import names its namespace after the last component of the
path, a module's file name is read at every use: `stdout.Stdout()`,
`basket.Basket(…)`, `path.Path("/tmp")`. Three conventions follow from that,
and the standard library keeps them.
- **Paths group by area, files name the thing.** `io/stdout`, `fs/path`,
  `fs/file`, `strings/parse`, `http/client`. The directories say where to look,
  and the file name is the noun a namespace reads as.
- **One noun per module.** A module named after what it holds reads well as a
  namespace (`path.Path`, `json.Json`). A module named `utils` or `helpers`
  reads as nothing, and gathers whatever has no better home.
- **`take` for the main noun, the namespace for the rest.** A file that builds
  paths all day writes `import "fs/path" take Path;` and `Path(…)`. A file that
  touches a module once writes the namespace, or a `std.` path, and leaves its
  bare names to its own declarations.

The doubled names that result (`stdout.Stdout`, `basket.Basket`) are the
price of a namespace derived from the path. `as` shortens them where it
matters: `import "io/stdout" as out;` and `out.Stdout()`.

The module name in the declaration should match the path it is imported by.
Nothing checks it, and a mismatch is legal: the declaration decides the
types' identity, the path decides where the file is found. But a reader who
sees `import "shop/money"` expects `module shop/money`, and a module that
declares another name surprises everyone who greps for it.

## 19.3 Where an import is found

An import path is a source path, not a module name: `"money"` is `money.ok`,
`"foo/bar"` is `foo/bar.ok`, and an explicit `.ok` is accepted as written.
The file is looked up in four places, in order:

1. the directory of the importing file;
2. the root of the importing file's package, the nearest directory above it
   that holds a `blade.toml` (19.7), when there is one;
3. the directories in `OKKAMA_IMPORT_DIRS`;
4. the bundled standard library.

The directory the program is started from plays no part. A module tree
behaves the same wherever it is run from, and two modules that import the
same relative path from different directories load different files. This is
why `basket.ok` can write `import "money"`: it finds its sibling, whoever
imports the basket and from wherever.

The lookup is *local-first*. A local `json.ok` shadows the standard
library's `json` for the program's own imports, so a program can replace a
library module without editing the files that import it. The standard
library is protected in the other direction: its own imports resolve only
inside the standard library, so a local file named `io/stdout.ok` cannot
change what the library's modules see. A program that shadows a library
module cannot also name it by a `std.` path. One program has one file for one
module path.

The import graph is bounded: at most 1,024 nested imports from the root,
16,384 modules and 256 MiB of module sources. An import that crosses a bound
is an error at that import. The bounds exist because compiling an untrusted
tree must end, and no real program comes near them.

### No cycles

Two modules that import each other are rejected:

{{example:cycle/orders}}

{{example:cycle/customers}}

{{example:19-cycle-rejected}}

The diagnostic names the chain. Some languages allow cycles and pay for it
in other ways. Python gives the second module a half-initialized first one,
which fails at a distance with an attribute error. JavaScript modules hand
out live bindings that are in a temporal dead zone until the cycle unwinds.
Java allows cycles between classes and packages, though not between its
newer modules. Go and OCaml forbid them, as Okkama does.

In a language whose modules contain only declarations, a cycle has no
initialization problem to cause, so the rule is not forced by the
implementation. It is a design rule. Two modules that need each other are one
module split in two, or two modules missing a third. A cycle-free import
graph is also a graph that can be read bottom-up, which is how a newcomer
reads a code base.

### Breaking a cycle with a contract module

Here the third module is the missing piece. An `Order` needs to know who
buys, and a `Customer` needs to know what was bought, but neither needs the
other's object: each needs a role. Those roles go into a module of their own,
which imports nothing:

{{example:trade/parties}}

{{example:trade/orders}}

{{example:trade/customers}}

{{example:19-contracts}}

`orders` and `customers` both import `parties`, and neither imports the
other. The graph is a V instead of a loop: `parties` at the bottom, the two
object modules above it, the program on top.

The roles say exactly what each side uses of the other: a buyer's name, a
purchase's total. That is less than the objects offer, and it is all the
modules may rely on. A third kind of buyer, a company rather than a person,
can join without touching `orders`. The cycle was a symptom: two modules
that knew each other's objects instead of each other's contracts.

The standard library is layered the same way at its bottom.
- `compare` declares `Order` and `Comparing`, imports nothing, and 88 of the
  library's modules import it.
- `collections/iteration` declares `Cursor` and `Iterating`, also imports
  nothing, and 40 modules import it.

A contract module is cheap: a few roles, no state, no imports. It turns a
tangle into layers.

## 19.4 What crosses the boundary

A module exports what it declares itself, and nothing else. Its imports are
its own business:

{{example:19-reexport-rejected}}

`shop/basket` imports `Money`, and its `Basket` depends on it. But `Money` is
not declared in `basket.ok`, so `take Money` from `"shop/basket"` finds
nothing. The same holds for namespaces: `basket.Money` is not a name. There
is no re-export, no Rust `pub use`, no JavaScript `export * from`, no
OCaml `include`. A program that needs `Money` imports the module that
declares it.

Selected declarations still bring everything they need to run. `Basket`
works in the program although the program never imported `Money` through it:
constructors, actions, defaults, lifecycle blocks and role fakes resolve
names in the module that declares them, not in the module that calls them.
That *dependency closure* is private. It makes the code run, and it puts no
name in the importer's scope.

### Two names, two types

Because a type's identity includes its module, two declarations with the
same name in two modules are two types, even when they are written the same
way:

{{example:bank/money}}

{{example:19-identity-rejected}}

`bank.Money` has a field `cents` and an action `cents()`, like
`shop.Money`, and the call is still rejected: Okkama types are nominal
(Chapter 4), and the name a type is nominal over is
`module#major::Name`. The same is true of two major versions of one module:
`shop/money#1::Money` and `shop/money#2::Money` would be different types,
which is what a major version is for.

The diagnostic shows a weakness. Because the module prefix is left out of
messages, the two types print the same, and the message reads as "`Money`
does not accept `Money`". When two types in a program share a name,
diagnostics should name them by the namespace the reader wrote (issue #338).

This is information hiding in Parnas's sense [@parnas1972]: a module is
the unit of a design decision, and what it exposes is a promise. A
re-export makes a module promise what it does not own. When the owner
changes, every module that re-exported the name changes its promise without
changing a line. Okkama keeps each promise in the module that can keep it.

## 19.5 Hidden declarations

Everything a module declares is public unless it says otherwise. `hidden`
before `object`, `role`, `enum` or `const` keeps a declaration inside its
module:

{{example:19-hidden-rejected}}

`Rounding` is visible in every file of `shop/money`, in its tests and its
fakes, and in every body defined in the module. Outside, `take Rounding` is
rejected, and a namespace does not reveal it either (`Type
"money.Rounding" is hidden in module "shop/money"`).

Hiding is enforced at the boundary, not only at the import line. The rules:
- **Values leave only as public types.** A value of a hidden type crosses
  only under a public one: a role it does, `Error`, or `any`. A role never
  converts back (Chapter 14), so outside code holds only the view.
- **Public signatures cannot name hidden types.** Parameters and results of
  public actions, constructor parameters, role signatures, enum payloads,
  generic arguments and bounds, and cap fields are all public. Any of them
  that names a hidden type is an error at the declaration. Private actions,
  fields and bodies may use hidden types freely.
- **A public object cannot do a hidden role.** Its `does` list is part of its
  public contract. A hidden object may do public and hidden roles, which is
  how an implementation is hidden behind a contract: a public role, a hidden
  object that does it, and a public action that returns the role.
- **A hidden error enum stays out of reach.** It can be thrown out of the
  module, but it can then be caught only as `Error`, and `okkama lint`
  reports such throws from public actions (`hidden-error`).

### A contract and a hidden implementation

The rules support one pattern above all: a public role, a hidden object that
does it, and a public entry that hands out the role.

{{example:billing/payments}}

{{example:19-payments}}

The program sees four things: `Payments`, `Payment`, and the four actions
of the role. It does not see `CardPayment`, its constructor, its fields or
the `Stage` enum. `authorized()` returns a new `Payment`, and the old one
stays `created`, as every value does (Chapter 13).

Everything the module hides it can change without telling anyone:
- split `CardPayment` into one object per stage;
- add a `Stage.refunded`;
- keep the amount as a `rat`;
- store the state in a map.

None of these changes reaches a caller, because a caller cannot have named
what changed. The compiler enforces the boundary from both sides. If
`open` were declared to return `CardPayment`, the declaration itself would
be an error, because a public action cannot name a hidden type. If the
program wrote `take CardPayment`, the import would be. The module's public
surface is exactly the role and the factory, and it cannot grow by accident.

This is the shape Chapter 14 recommends for roles in general, enforced at
the scale of a module. A role names what callers may rely on; `hidden`
makes sure they cannot rely on anything else.

`hidden` is a contextual keyword: `has hidden is int` is still a field
named `hidden`.

Okkama's default is the opposite of Go's capitalization rule, Rust's
private-by-default and Java's package-private. Those languages make the
writer opt in to exposure. Okkama makes the writer opt out, on the grounds
that most declarations of a module are its reason to exist, and a
`hidden` marks the few that are not. Whether that is the right default is a
question of style. A module of twenty declarations with three helpers reads
better with three `hidden`s than with seventeen `pub`s. A module with one
public entry and twenty internals reads worse.

## 19.6 Module constants

A module can name values the compiler computes (issue #153):

{{example:19-constants}}

`const NAME = value;` is made of literals, payload-free enum cases,
`rat(n, d)`, scalar casts, operators, other constants, and arrays and maps of
such values. The constants of a module may be declared in any order (`MB`
uses `KB`, declared after it). The computation has the semantics of the
running program, not of a separate constant language: `1 / 2` is the `rat`
1/2, and 2⁹⁶ is an exact integer that would overflow any machine word. A
failing computation (`1 % 0`) and a cycle between constants are compile
errors, and so is anything that would run code:

{{example:19-const-call-rejected}}

A constant is public like an object: `take LANG` or `settings.LANG` reaches
it, and `hidden const` keeps it in its module. A scalar constant becomes a
literal wherever it is used. An array or a map is built once per run and
shared without copies, and read like any flat collection (Chapter 8).

Constants are not a way to share objects. There is no module-level `let`,
no static field and no singleton: an expensive object such as a compiled
regular expression or a database connection is built once where the program
starts, in `main()`, and passed to the objects that need it. Its lifetime and
its users stay visible in the code, and a test can pass another one. That is
the same rule Chapter 12 states for objects: what an object needs, it gets
through its constructor.

## 19.7 Packages and blade

`blade` is Okkama's package tool. It is the youngest part of the toolchain,
and this section describes it as it is in 0.4.3: a working core with large
gaps, listed at the end of the section. Nothing in this section is run as an
example, because installing a package runs `git`, and the book's examples
run in a cage without processes.

### Packages

A directory that holds a `blade.toml` is a *package root*. A file belongs to
the package of the nearest such directory above it, and a *project* is a
package that is not installed inside another project. `blade new demo`
creates one:

```toml
[module]
name = "demo"
version = "1.0.0"

[build]
entry = "src/main.ok"
target = "sparrow"
output = "target/demo.spb"

[run]
engine = "sparrow"
args = []
env = {}

[dependencies]
```

```okkama
module demo;
import "io/stdout" take Stdout;

main() is int {

    Stdout().writeln("hello from okkama");
    0;
}
```

The module declares no version. In a package the version lives in
`blade.toml`, once, and a module file that declares one is an error at the
version. The package's files import one another as in 19.3: beside the
importing file, then from the package root.

Dependencies are listed in `[dependencies]`:

```toml
[dependencies]
web = "12.3.0"
json = { git = "https://example.org/json", version = "1.*" }
tools = { path = "../tools" }
```

`blade install` puts each one into `pkg/<name>/` of the project root, with
its own `blade.toml`, and the project imports them by that path:

```okkama
import "pkg/web/router" take Router;
```

An import path that starts with `pkg/` resolves against the project root
only. It must name a dependency declared in the `[dependencies]` of the
importing file's own package. Transitive dependencies are not importable:
if `web` depends on `json`, the project cannot `import "pkg/json/…"` until
it declares `json` itself. This is 19.4's no-re-export rule at the scale of
packages. What a program uses, it lists.

### Versions and the lock

A dependency comes from a git repository, whose versions are its `vX.Y.Z`
tags, or from a local directory, whose version is the one in its
`blade.toml`. A requirement is written as follows.

| Requirement | Means |
|---|---|
| `12.3.0` | compatible: `>=12.3.0, <13.0.0`; below 1.0 the minor is the breaking part, so `0.3.1` is `>=0.3.1, <0.4.0` |
| `12.*`, `12.3.*` | any version with that prefix |
| `=12.3.1` | exactly that version |
| `*` | any version |

This is the reading of Semantic Versioning [@semver] that Cargo also uses.
Resolution keeps *one version of each package per project*. When several
packages require the same one, the requirements meet at the highest version
that satisfies all of them. When no version does, `blade install` fails and
names who wants what. Installation is all or nothing: the new `pkg/` is
assembled beside the old one and swapped in, and `blade.lock` records what
was installed: name, version, source, git commit, a SHA-256 checksum, and
dependencies. `blade update` moves to the newest allowed versions;
`blade install` without arguments reproduces the lock.

`blade build` compiles the project as `[build]` says, and `blade run` builds
when a source, `blade.toml` or `blade.lock` is newer than the output, then
runs it as `[run]` says. Both refuse a project whose dependencies are not
installed.

Two other package managers made the opposite choices on the version
question, and the comparison shows what Okkama's rule buys and costs. npm
installs a separate copy of a package for each dependent that needs a
different version, so a program can hold two copies of one library and two
incompatible `Money` types without knowing it. Cargo allows several
semver-incompatible versions of one crate in a build and unifies compatible
ones. Go picks, for each module, the *minimal* version that satisfies every
requirement rather than the highest (*minimal version selection*,
[@cox2018]), so a build does not change when a new release appears
upstream; Okkama's lock gives the same reproducibility for an installed
project, and picks the highest version when it resolves anew.

One version per package matches the nominal identity of 19.4. Every module
of the program sees the same `json` module, so a `Json` value made by one
dependency is the `Json` type another dependency expects. The cost is the
diamond: two dependencies that need different majors of one package cannot
both be installed, although the language could tell their types apart.

### What blade does not do yet

- **No registry.** Packages come from git URLs and paths, and there is no
  index, search or `blade publish`.
- **No workspaces.** Several packages developed together each have their
  own `pkg/`.
- **Diamonds across majors fail**, as above.
- **Shallow tooling around the lock.** No `blade tree`, `blade why` or
  audit of installed checksums against a known list.
- **No build profiles or features**, and no conditional dependencies.

Most of these are the ordinary gaps of a young tool rather than design
decisions. The design decisions are the three described above: a version in
one place, imports by explicit `pkg/` paths, and one version per package.

## 19.8 How modules become one program

An import is resolved when the program is compiled, not when it runs. The
compiler starts from the files it is given, reads their import lines,
finds each imported file by the rules of 19.3, and repeats until the graph
is closed. A missing file, a missing `take` name, a hidden declaration, a
cycle or a crossed bound stops it there. A program that compiles has found
every module it will ever use.

`okkama build` then writes the whole program into one file: an SPB for
Tomato, or a JAR for the JVM. Every imported module is in it, the standard
library modules the program uses included, and nothing else. The shop
program above builds into an SPB of 2.4 KB, and `sparrow shop.spb` runs it
in an empty directory, with no sources, no standard library and no
`OKKAMA_STDLIB_DIR`. Inside, each declaration keeps its module-qualified
name (`shop.money::Money`), which is how two `Money`s from two modules stay
two types after compilation. A module reached through two branches of the
graph is compiled and registered once.

Several consequences follow from building the whole program at once.
- **No version skew at run time.** The program that was compiled is the
  program that runs. There is no class path to assemble, no `node_modules`
  to resolve, and no shared library whose newer version a deployment picks
  up by accident. Updating a dependency means building again.
- **Nothing is loaded at run time.** No `dlopen`, no `importlib`, no class
  loader. It is the other side of 19.11's "no dynamic import": a plug-in is
  a value passed in, not a file found later.
- **The standard library travels with the program.** A program built
  against one library runs with that library, whichever version is
  installed where it runs. The cost is size: every program carries its own
  copy of the modules it uses. For the ladder's programs an SPB is 1.4 to
  13 KB. A JAR is about 1.3 MB, because it also carries the JVM runtime
  `okkama.rt`.
- **The IR interpreter compiles from source each time.** It runs the same
  graph from the source files, which is why `okkama run` needs the
  standard library and `sparrow file.spb` does not.

Whole-program compilation is also what lets the compiler check what the
module rules promise. Every `take`, every hidden boundary and every type
identity is known before the first instruction runs, so none of these rules
costs anything at run time.

## 19.9 Where modules came from

The word *module* in its modern sense comes from David Parnas's 1972 paper
on decomposition [@parnas1972]. Parnas compared two ways of splitting the
same program. The first gave a module to each step of the processing. The
second gave a module to each design decision likely to change, and hid
that decision behind an interface. The second survived changes that broke
the first. Everything since is a mechanism for that idea: a boundary, a
list of what crosses it, and a promise that the rest may change.

Niklaus Wirth's Modula-2 [@wirth1982] gave the mechanism the shape most
languages still use. Its *definition module* stated what a module exports,
and a separate *implementation module* kept the rest. Its import lists, `FROM
InOut IMPORT WriteString, WriteLn;`, named exactly what a client took,
much like Okkama's `take`. Standard ML went further [@macqueen1984]: modules
(structures) are checked against interfaces (signatures) and can be
parameterized by other modules (functors). OCaml inherited that, and its
`open` and `include` are the wildcard import and the re-export Okkama
leaves out.

The languages most programmers use took simpler paths. Java organized
classes into packages with a directory each, added `import p.*`, and in Java
9 added modules on top of packages [@gosling2023]. Python and JavaScript made
a module an object built by running a file, which is why importing runs
code and cycles produce half-built modules. Go made a directory a package,
exported names by capitalization, and forbade cycles. Rust made a module
tree inside each crate, with private-by-default items and `pub use`
re-exports.

Okkama's module system is closest to Modula-2's import lists and Go's
discipline: a file of declarations, explicit lists of what is taken, no
cycles, no effects on import. It differs from both in two places: names are
public unless `hidden`, and identity includes the module and its major
version.

## 19.10 Okkama among other languages

| | Unit | Default visibility | Wildcard import | Re-export | Import runs code | Import cycles | Versions per program |
|---|---|---|---|---|---|---|---|
| **Okkama** | file (or listed files) | public; `hidden` | no | no | no | rejected | one per package |
| Go | directory | capitalized names | `import .` (discouraged) | no (aliases only) | `init()` | rejected | one per major |
| Rust | module in a crate | private; `pub` | `use m::*` | `pub use` | no | allowed in a crate | several per crate |
| Java | class in a package; modules | package-private | `import p.*` | no (modules: `requires transitive`) | static initializers | allowed | one on the class path |
| Python | file | public; `_` by convention | `from m import *` | any imported name | the whole file | allowed, half-built | one per environment |
| JavaScript | file | private; `export` | `import * as` (namespace only) | `export * from` | the whole file | allowed, live bindings | duplicates in `node_modules` |
| OCaml | file plus signature | per signature | `open M` | `include M` | top-level effects | rejected | one per build |

Two columns separate Okkama from most of the list: imports that run no code,
and boundaries that pass nothing on. Both come from the same principle as the
rest of the language. An import line is a statement about names that a
reader can trust without opening the imported file.

## 19.11 Limits

- **No re-export.** A library that wants to present one front module for many
  internal ones must wrap them: declare objects that delegate, or roles that
  its internals implement. There is no `pub use`.
- **No conditional or dynamic import.** Which modules a program uses is fixed
  when it is compiled; a plug-in system is built from roles and values passed
  in, not from loading files at run time.
- **The version lives in two places.** Outside a package, in the module line;
  inside one, in `blade.toml` only. A module moved into a package must lose its
  version, and a module taken out of one must gain it.
- **Multi-file modules are listed, not discovered.** A module's files are the
  ones the command line or `blade.toml` names; a forgotten file is a missing
  declaration, not a smaller module.
- **Same-named types print alike** in diagnostics (issue #338).
- **A shadowed library module cannot be named with `std.`.** One module path,
  one file, per program.
- **blade is young.** No registry, no workspaces, no two majors of one package
  in one project (19.7).

## 19.12 What this design costs

Every rule in this chapter removes an option that other languages offer,
and each removal has a price.

No wildcard imports means long import lines. A file that uses fifteen
declarations of one module either lists fifteen names or writes a prefix
fifteen times. Okkama accepts the length: the alternative is a file whose
names have no visible origin. No re-export means libraries cannot build a
front module that renames their internals. They must wrap, which is more
code and one more call. No cycles means that two modules that grew together
must be pulled apart before they can be imported, sometimes at an
inconvenient moment. Identity by module and major version means that two
libraries which each declare `Money` cannot pass their values to each other.
They must convert, explicitly, at the boundary between them.

What the rules buy is the same thing four times over: a reader who can
trust an import line. It says exactly which names a file uses and where they
come from. It runs nothing, so a file can be imported to read its
declarations. It brings nothing the imported file does not own, so a change
in a third file cannot add a name to it. And a type it brings is the type
the imported file declares, not another type with the same name. Chapter 1
called that the razor applied to software: no entity that does not have to
exist, and no name whose origin is not in front of the reader.
