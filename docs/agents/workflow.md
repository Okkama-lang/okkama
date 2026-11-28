# The Okkama workflow for agents

Write, then let the toolchain judge. Run these in order after every change; each one is cheap.

## 1. `okkama check FILE.ok`

Reports every compile-time error of the program and its imports, one per line:

    app.ok:2:32: semantic error: Cannot initialize variable "x" of type int with string

The place is `file:line:column`; the kind is `syntax error`, `semantic error` or `import error`. Fix them in order:
a later error is often a consequence of an earlier one. Exit status 0 means the program compiles.

## 2. `okkama fmt -w FILE.ok`

Rewrites the file in the one canonical layout (4-space indentation, one statement or member per line, `else` and
`catch` on their own line). Only whitespace changes. Always run it; `okkama fmt --check FILE.ok` only reports.

## 3. `okkama lint FILE.ok`

The checks of `check` plus lint rules: unused imports, lets and hidden members, unreachable code, snake_case
names, non-exhaustive `match`, missing doc comments on public declarations, unresolved doc links. Output is
`file:line:column: severity[rule]: message`; exit status 1 if anything was reported. A `let` whose name starts with
`_` is never reported as unused.

## 4. `okkama run -t FILE.ok`

Runs the module's `test` blocks (they live next to the code they test) and its documentation examples: fenced
`okkama` blocks in doc comments compile, `okkama,run` blocks run and must print exactly the adjacent `output` block.
`--filter TEXT` runs only matching tests; `--seed N` fixes the order. A failure prints the test, the failure and a
trace in Okkama terms.

## 5. `okkama run FILE.ok [ARGS]`

Runs `main`. Its `int` result in 0..255 becomes the exit status; an uncaught failure is reported on stderr with
the call stack (exit 1). Several files of one module: `okkama run a.ok b.ok`.

## Looking things up

- `okkama man` — every std module with its one-line summary; `okkama agents std` prints the same as Markdown.
- `okkama man MODULE` — a module's page; `okkama man MODULE NAME` — one declaration; `NAME.ACTION` — one action.
- `okkama man -k QUERY` — search std's documentation.
- `okkama env` — the version, the binary, the std directory, and what else a build depends on.
- `okkama repl` — try an expression or a declaration live (`:help` inside).

## When something fails

- **A compile error you do not understand:** read the declaration involved with `okkama man`, and the rule with
  `okkama agents language`; Okkama is not Java, Rust or Python, and guessing from them misleads.
- **A runtime failure:** the trace names the actions in order; the first line is the failure itself.
- **A test you cannot make pass:** run it alone with `--filter`, and print values with `io/stdout`'s `Stdout`.
