# Chapter 19 — archived implementation passages

Replaced mixed paragraphs are preserved in full for context.

## 19.7 Packages and blade

described 0.5.x

[build]
entry = "src/main.ok"
target = "sparrow"
output = "target/demo.spb"

[run]
engine = "sparrow"
args = []
env = {}

## 19.7 Packages and blade

described 0.5.x

First, a build depends only on manifests. A new release of `json` upstream
changes nothing in any project until some `blade.toml` raises its minimum;
the same manifests give the same versions on every machine, on every day,
with or without a lock. Second, a version a project gets has been asked for
by name: 1.4.2 is installed because some manifest says `1.4.2`, so somebody
built and tested against at least that version. Under "highest" a project
gets 1.9.0 the morning it is published, tested by nobody who depends on it.
Third, the resolution is cheap and its failures are legible. Blade walks the
requirement graph twice. The first walk finds the largest minimum of each
package and major, reading the manifest of every version it reaches, so a
minimum raised by one requirement brings that version's own requirements.
The second walk goes from the project through the selected versions only,
and what it reaches is installed: a requirement of a version that lost
cannot cause an error, and packages only it needed are not installed. There
is no search and no backtracking, so there is no satisfiability problem to
time out on.

## 19.8 How modules become one program

described 0.5.x

## 19.8 How modules become one program

An import is resolved when the program is compiled, not when it runs. The
compiler starts from the files it is given, reads their import lines,
finds each imported file by the rules of 19.3, and repeats until the graph
is closed. A missing file, a missing `take` name, a hidden declaration, a
cycle or a crossed bound stops it there. A program that compiles has found
every module it will ever use.

`okkama build` then writes the whole program into one file: an SPB for
Sparrow, or a JAR for the JVM. Every imported module is in it, the standard
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

## 19.1 A module is a file

described 0.5.x

The name a module declares and the path other files import it by are two
different things. An import names a *file*: `import "shop/money"` is
`shop/money.ok`, found by the rules of 19.3. The declaration names the
*module*: `shop/money` with major version 1. The two usually agree, and
nothing forces them to. What matters is the declaration, because it is part
of every type the module declares. Inside the runtime, `Money` from this
module is `shop/money#1::Money`: the module, its major version and the
declared name. The prefix appears in no ordinary output and no diagnostic,
but it decides which types are the same (19.4).

## 19.6 Module constants

described 0.5.x

A constant is public like an object: `take LANG` or `settings.LANG` reaches
it, and `hidden const` keeps it in its module. A scalar constant becomes a
literal wherever it is used. An array or a map is built once per run and
shared without copies, and read like any flat collection (Chapter 8).

## 19.7 Packages and blade — full source example

described 0.5.x

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
