# Chapter 23. Building a CLI tool

The chapters before this one describe the language a piece at a time. This
one and the next put the pieces together into programs of the kind people
actually write. The first is a command-line tool: a program started from a
shell, given arguments and perhaps a stream of text, which does one job and
reports how it went.

The tool is `tally`. It counts the lines and words of some text, either the
files named on its command line or its standard input, and lists the words
that occur most often:

```text
$ okkama run tally.ok --top 2 notes.txt
2 lines, 12 words, 8 distinct
3 the
2 on
```

The program is small, but it touches most of what a real tool has to get
right: arguments and their mistakes, files that are missing, the difference
between output and messages, exit status, help text and tests. The chapter
builds it in the order a reader would meet those questions, and each section
leans on an earlier chapter: roles (Chapter 14), failures (Chapter 10),
resources (Chapter 11) and the decide-then-do shape of Section 26.5.

## 23.1 What a tool owes its user

The conventions a command-line tool follows are older than most languages in
use today. In the foreword to the 1978 issue of the *Bell System Technical
Journal* devoted to Unix, Doug McIlroy and his co-authors summed up the style
in a few maxims: make each program do one thing well, and expect the output
of every program to become the input to another, as yet unknown, program
[@mcilroy1978]. POSIX later wrote down the shape of the command line itself:
options before operands, single-letter options introduced by a hyphen, and
`--` to end the options so that an operand may start with a hyphen
[@posix2017]. And the *Command Line Interface Guidelines* of Prasad and his
co-authors restate the old rules for programs that people use as much as
scripts do: help that is easy to reach, errors that say what to do next, and
output meant for people kept apart from output meant for programs
[@prasad2020].

From these sources, and from the shape of the language, `tally` takes five
rules:

1. **Data goes to the standard output, messages to the standard error.** A
   script that pipes `tally` into another program must never receive a
   warning in place of a count.
2. **The exit status says how it went.** POSIX fixes only that zero means
   success. `tally` adds a common split: 1 when the work failed (a file could
   not be read), 2 when the command line was wrong.
3. **No files means the standard input.** Then `tally` works at either end of
   a pipe.
4. **`--help` is always there,** prints to the standard output and exits
   with 0, because asking for help is not an error.
5. **A wrong command line is answered with a reason and a hint,** not with a
   trace.

None of this is specific to Okkama. What Okkama changes is how much of it the
program has to do by hand.

## 23.2 The command line as data

The usual way to read a command line is a loop over the arguments with a
growing set of flags: a variable for each option, a branch for each
spelling, a counter for operands, and a hand-written usage text that slowly
drifts away from what the loop actually accepts. The standard library's
`cli/command` module turns the command line into a declaration. A command is
an object that lists its options, flags, operands and subcommands; reading
arguments is an action on that object; the usage text is another:

```okkama
let command = CliCommand(
    name: "tally",
    summary: "Count lines and words, and list the most frequent words",
    options: [Option(name: "top", short: "n", value: "N", help: "How many words to list (default 3)")],
    flags: [
        Flag(name: "ignore-case", short: "i", help: "Count Word and word as one word"),
        Flag(name: "help", short: "h", help: "Show this help"),
    ],
    operands: [Operand(name: "FILE", count: OperandCount.repeated, help: "Files to read; none reads the input")],
);
let call = command.invocation(arguments);
```

One declaration gives three things that the loop-and-flags style keeps in
three places:

- **Parsing.** `invocation(arguments)` reads an argument list into an
  `Invocation`: the selected subcommand, the operands, and every value by
  the name of its option. Reading is pure. It has no side effects, can be
  repeated, and does not look at the process, so a test can call it with any
  list it likes.
- **Help.** `usage(program:)` writes the usage text from the same
  declaration, wrapped to a width. It cannot disagree with the parser.
- **Completion.** `completion(program:, shell:)` writes a completion script
  for bash, zsh or fish, again from the same declaration.

Values come out typed and with absence spelled out. `call.text("name")` is a
`Maybe<string>`, so the default is written where the value is used,
`call.int("top") else 3`, and a value that is present but malformed, such as
`--top many`, is a `CliFailure` naming the argument's index. Flags are
`bool`. Repeated options come back as an array. Asking for a name that no
command declares, `call.flag("verbos")`, throws `UndeclaredCliName`: that is
a mistake in the program, not in the user's arguments, and the module keeps
the two kinds of error apart.

The arguments themselves come from `cli/arguments`.
`ProcessArguments().strings()` is the process's argument list as text,
without the program name, read as strict UTF-8. An argument that is not
valid UTF-8 is refused with its index and byte offset rather than guessed
at. Arguments are what the operating system passed, never shell text, so
there is no quoting to undo and no word splitting to worry about.

## 23.3 The tool

The whole of `tally` is one module. It is shown here entire and then taken
apart.

{{example:tally/tool}}

The module has five parts, and each is a pattern from an earlier chapter.

**`Sources`** is the role for "where text comes from" (Chapter 14). A real
run uses `Disk`, which reads a file through `fs/file` and closes it with
`defer` (Chapter 11), and reads the standard input through `Utf8Input`. A
test gives its own implementation with prepared lines (Section 23.6). The
role is the only place where the tool touches the outside world on the way
in. Everything after it works on arrays of strings.

**`WordCount`** is a value with an order of its own. It does
`Comparing<WordCount>`, and its `order` puts more frequent words first and
breaks ties alphabetically. With that, the built-in `ordering()` sorts an
array of counts, and no comparator object is needed (the `compare` module
advises against one).

**`Tally`** holds the counts and changes by successor (Chapter 13). `read`
returns a new `Tally` with one more line and the line's words added to the
map, so counting a whole text is a `reduce` over its lines. `top` builds the
counts, sorts them and keeps the first few. Nothing in `Tally` knows about
files, arguments or output.

**`Outcome`** is what a run decides: the lines meant for the standard output,
the lines meant for the standard error, and the exit status. It is a plain
value with three fields.

**`Tool`** connects them. Its constructor builds the command declaration
once. `outcome(arguments, sources)` reads the arguments, picks the sources,
runs the counts and returns an `Outcome`. It catches the failures it can
answer and turns them into messages and statuses (Section 23.5).

## 23.4 Deciding, then printing

`Tool.outcome` writes nothing. It returns its decisions as a value, and a
separate, very small shell carries them out. This is the decide-then-do
shape of Section 25.5, and for a command-line tool it pays off at once: the
part that has logic can be tested by comparing values, and the part that
does input and output has no logic left to test.

The program a user runs is three lines on top of the module:

```okkama
main() is int {
    Shell().printed(Tool().outcome(ProcessArguments().strings(), Disk()));
}
```

`main` returns the exit status, and the runtime hands it to the operating
system. The example below runs the same tool twice from inside a program,
with fixed arguments over a small file, `tally/sample.txt`, so that its output
can be checked:

{{example:23-tally}}

- The first run lists the two most frequent words. "the" and "The" count
  as different words.
- The second run adds `-i`, short for `--ignore-case`, and `-n 2`, the short
  form of `--top`. The distinct count drops from 8 to 7 and "the" reaches 4.
- `Shell.printed` sends `out` to `Stdout` and `err` to `Stderr` in order, and
  returns the code. A real program returns that code from `main`.

The split between `Tool` and `Shell` also decides where a future change
goes. Coloured output for a terminal, a `--json` flag or a progress line on
the standard error are changes to how decisions are printed, and they stay
in the shell. Counting differently is a change to the decisions, and it
stays in `Tally`.

## 23.5 Failures at the edge

A tool meets three kinds of failure, and they deserve three different
answers.

**A wrong command line** is the user's mistake, made a moment ago and easy to
fix. `cli/command` reports it as `CliFailure.detailed(index, token, message)`.
`tally` adds one refusal of its own for a negative `--top`, as a case of its
own `TallyError` enum (Chapter 10). Both are answered on the standard error
with the reason and a hint, `Try 'tally --help'.`, and exit status 2. No
trace is shown: the user needs to know what to type, not where the
program was.

**A failure of the work**, here a file that does not exist or cannot be
read, is answered with the file system's own description and exit status 1.
`fs/file` has already turned the host's error into a case of `FileError`
(Section 10.11), so the tool never sees an operating-system code and behaves
the same on every platform.

**A bug in the program** is not caught at all. An index out of range, a
broken invariant or an `UndeclaredCliName` ends the run with the language's
failure report, and the exit status says the run failed. This is the
argument of Section 10.4 applied to a tool. A `catch` broad enough to turn
bugs into a friendly "something went wrong" would hide the one report that
lets someone fix the program.

The `try` in `Tool.outcome` therefore names exactly the failures the tool
can answer, `CliFailure`, `TallyError` and `FileError`, and nothing else.
Each `catch` produces an `Outcome`, so the error paths are values too and are
tested the same way as the happy path.

## 23.6 Testing a tool without a terminal

A command-line tool is often tested last and worst, by running it in a shell
and looking at the output, because its inputs and outputs are the process's
own. `tally` needs none of that. Its arguments are a parameter, its sources
are a role, and its output is a value. A test gives all three:

{{example:23-tally-tests}}

- `Given` implements `Sources` with a map of file names to lines and a list
  of lines for the standard input. A missing name throws the same
  `FileError.not_found` the disk would throw, so the error path is real.
- Each test calls `Tool().outcome` with an argument list and compares the
  `Outcome`: the status, the output lines and the message lines. The tests
  read like a specification of the command line: what happens with two
  files, with none, with a missing one, with a bad value and with `--help`.
- Nothing is mocked inside the tool. `Tally` and `WordCount` are covered
  through the tool's own behaviour, and they could be tested directly as
  values if they grew more complex.

Code that reads the process's arguments itself, instead of taking them as a
parameter, can still be tested: `ProcessArguments(arguments: …)` builds the
same object from a prepared list. Taking arguments as a parameter is
simpler, and is the shape this chapter recommends.

## 23.7 Input: whole or streamed

`Disk` reads a file whole, `file.text().lines()`, and reads the standard
input whole too, through `Utf8Input(origin: Stdin()).content()`. For a tool
that counts, this is the simple and honest choice: the result needs every
line anyway, and the counts are much smaller than the text. Two properties
of the standard library make it safe:

- **Decoding is strict.** A file or an input that is not valid UTF-8 is
  refused with the position of the first bad byte. A tool that should read
  anything can ask for `Stdin(decoding: InputDecoding.lenient)`, which
  replaces bad bytes with U+FFFD, and say so in its help.
- **Resources close on every path.** The file is closed by `defer` whether
  `lines()` succeeds or throws, and `Utf8Input` is closed the same way
  (Chapter 11).

A tool that filters rather than counts, printing each line as it arrives the
way `grep` does, should stream instead. `for Stdin() -> line { … }` reads one
line at a time in batches, and `for file.text() -> line { … }` does the same
for a file. Streaming changes the shape of the tool more than its code:
the decisions can no longer be collected into one `Outcome` at the end.
The core then becomes a value that takes one line and returns the lines to
print for it, and the shell prints them as it goes. The decide-then-do split
survives, at the granularity of a line.

{{example:23-filter}}

- `Matcher` is the core. `shown(line)` says what to print for one line and
  `after(line)` returns the matcher for the next one, with the count of
  matches so far. It keeps only what the end of the run needs, never the
  text.
- `Filter.run` is the shell. It walks the lines, prints what `shown` returns
  and rebinds `current` to its successor. In a real tool the loop is
  `for Stdin() -> line`, and nothing else changes.
- `status()` follows `grep`'s convention: 0 when something matched, 1 when
  nothing did. A tool's exit status is part of its contract, and scripts
  test it (`if tally-filter error log; then …`), so the convention is chosen
  once and documented in the help.

`cli/arguments` sets its own limits. By default it reads at most 4,096
arguments and 1 MiB of them, and refuses more with `ArgumentLimitExceeded`
instead of growing without bound. A tool that is meant to take huge argument
lists, for example from `xargs`, can set other limits through the
constructor that takes them.

## 23.8 Output for people and for programs

A tool's output has two readers. A person at a terminal wants alignment,
emphasis and perhaps colour. A program reading a pipe wants a stable,
parseable format and no escape codes. The *Command Line Interface
Guidelines* recommend detecting which one is there, and offering `--json`
when the output is likely to be consumed by another program [@prasad2020].

The decision belongs at the edge, made once, from facts the program is given:

{{example:23-output}}

- `DetectedTerminalCapabilities` decides the colour depth from whether the
  output is a terminal, the environment (`NO_COLOR`, `TERM`) and a policy
  (`automatic`, `never`, `always`). A pipe and `NO_COLOR=1` both give no
  colour. A terminal gives what it supports.
- The report is rendered for a depth, so the same `Report` gives plain text,
  coloured text or JSON. The counting code never sees any of this.
- The facts are passed in. In a real run they come from the terminal session
  and `Environment()`. In the example they are fixed, which is why the
  example can check what each case prints.

The JSON form is built with the `json` module's `Json` and `JsonField`, not by
concatenating strings, so quoting and escaping are always right. A field
added later does not break readers that ignore it, which is the usual reason
to prefer an object over an array at the top level.
## 23.9 Changing files safely

A tool that only reads cannot damage anything. A tool that rewrites files —
a formatter, a migration, a bulk rename — can, and its users expect two
things from it: a way to see what it would do before it does it, and an
original that survives if the run is interrupted halfway.

Both follow from the decide-then-do shape. The change is a value first, and
writing it is a separate step:

{{example:23-rewrite}}

- `Tidy.planned` reads a file and returns a `Change` that holds the lines
  before and after. Nothing is written. `--dry-run` is simply "print the
  plan and stop", and costs no extra code in the core.
- `Tidy.applied` writes the new text to a sibling file and then moves it over
  the original with `FileEntry.move(destination:, overwrite: true)`, which
  replaces the destination in one step. If the run fails while writing, the
  original is untouched and only the sibling is left behind. A tool that
  must never leave even that writes through `TemporaryFile` in the same
  directory, which removes itself on close.
- The example works inside a `TemporaryDirectory` made in the current
  directory, and `defer scratch.close()` removes it with everything in it, so
  the run leaves nothing behind.

`File(path: path).text().replace(text)` does all of this in one call: it
writes a temporary file beside the original, synchronizes it, renames it
over the original and synchronizes the directory, so after a crash or a
power cut the file is the whole old text or the whole new one (#122).

A rewrite of many files should also stop cleanly when the user presses
Ctrl-C. Without help a signal ends the process at once, between two files or
in the middle of one, and no `defer` runs. An `Interruption` from `os/signal`
takes SIGINT and SIGTERM instead, and the loop decides where stopping is
safe:

```okkama
let interruption = Interruption();
for files -> file {
    interruption.check();      // throws Interrupted between two files
    Tidy(file).applied();
}
```

`interruption.status()` is then the conventional exit status, 130 after
Ctrl-C, and a second Ctrl-C during the cleanup ends the process at once
(#638).

## 23.10 Configuration and the environment

`tally` has two options and no configuration, and many tools should stay
that way. When a tool does grow settings, the *Command Line Interface
Guidelines* give the usual order of precedence: flags override environment
variables, which override a configuration file, which overrides the built-in
defaults [@prasad2020]. Two parts of the language help to keep that order
honest.

The first is that the environment is a value, not a global (Section 21.4).
A tool takes an environment snapshot in `main` and passes it, or the one
setting it needs, to the code that uses it. Nothing deep in the program
reads a variable behind its caller's back, and a test passes a prepared
environment the same way `tally`'s tests pass prepared sources.

{{example:23-settings}}

- `Environment(values: …)` is a snapshot that contains exactly the given
  variables and nothing of the process, so each case in the example is
  isolated. A real program passes `Environment()`, read once in `main`.
- The precedence is one expression: `call.int("top") else
  (environment.maybe("TALLY_TOP").int() else 3)`. Each `else` is the next
  layer down, and the order can be read off the line.
- A malformed value is not silently skipped. `--top many` is a
  `CliFailure`, and `TALLY_TOP=many` is an `InvalidEnvironmentValue` naming
  the variable. A layer that is present but wrong is an error, not a reason
  to fall through to the next layer.

The second is `Invocation.settings()`, which returns every value read from
the command line keyed by `SettingKey`. A configuration file read through
the `config` modules gives settings with the same keys, so the order of
precedence becomes a sequence of layers, each laid over the one before. An
option declared with an explicit `key` is stored under that key, which lets
`--top` on the command line and `top = 5` in a file mean the same setting.

## 23.11 Growing: subcommands

Tools grow by verbs. `git` has `commit` and `log`, `cargo` has `build` and
`test`, and a tool that started with one job often gains a second. In
`cli/command` a subcommand is a `CliCommand` listed in its parent's
`commands`. Each declares only the options it takes, and the root can
declare options shared by all of them. Reading the arguments selects one
command, and `call.command()` returns the declared object, so the program
compares objects rather than strings:

{{example:23-subcommands}}

- `top --n 5` selects `top` and reads its own option. `top` alone takes
  the default with `else`.
- A root with subcommands is a branch. It expects one of its children, so an
  empty command line and an unknown verb are both a `CliFailure` naming the
  argument, which the tool answers with status 2 as in Section 24.5.
- `call.path()` gives the names from the root to the selected command, for
  messages such as `tally top: …`. The usage text of a subcommand comes from
  its own declaration, so `tally top --help` can show only what `top` takes.

The objects in `Commands` are the same values the program later compares
against. A subcommand is renamed in one place, and a typo in a comparison is
a compile error, not a branch that is never taken.

## 23.12 Shipping it

A tool is used through its help text more than through its documentation.
`tally --help` prints `command.usage(program: "tally")`: the summary, the
usage line, and the options, flags and operands with their help, wrapped to
80 columns or to the width given. Because it is generated from the
declaration, adding an option to `CliCommand` adds it to the help.
`command.completion(program: "tally", shell: CompletionShell.zsh)` gives a
completion script for the shell, and a tool often offers it as a hidden
subcommand that users run once and save.

During development a tool runs from source, with its arguments after the
file: `okkama run tally.ok --top 5 notes.txt`. To ship it, `okkama build
tally.ok -o tally.spb` compiles it into a bytecode file that
`okkama exec` runs without recompiling (Chapter 19 covers modules, packages
and their versions). The program is the same either way. Its behaviour does
not depend on how it was started.

## 23.13 What the design costs

The tool in this chapter is longer than the shortest program that would
print the same counts. A twenty-line script with a loop over the arguments,
a mutable map and `print` calls would do the job for its author. The
difference is in what the extra lines buy:

- **The command line is declared once.** Parsing, help and completion follow
  from it, and the usage text cannot drift away from the parser.
- **Every outcome is a value.** The error paths are tested as easily as the
  normal one, and a change to output formatting cannot change what is
  counted.
- **Failures are sorted by kind.** A user mistake gets a hint and status 2, a
  failed file gets status 1, and a bug keeps its report.
- **The outside world enters through one role.** Tests need no terminal, no
  files and no process.

Two costs remain. Reading whole files holds the text in memory, which is the
right trade for a counter and the wrong one for a filter (Section 23.7). And
the successor style rebuilds a `Tally` for every line. For text in the
megabytes this is fast enough. For gigabytes, a tool would count in larger
steps or stream and keep only the counts, still without mutating anything it
shares.
