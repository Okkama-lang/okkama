# Source documentation, Phase B (#513)

Scope: ticket #513 and the owner's decision on #512, sections 11, 13, 16
and the applicable regressions in section 18. Phases C and D are separate.
No language or std API changes.

## Implementation

- [x] Resolve links using compiler scopes, namespaces, public declaration
  metadata and the existing `ModuleLoader`, including modules not imported
  by the documented file.
- [x] Preserve overload sets and filter hidden declarations and members.
- [x] Preserve code blocks, inline code, images, escaped brackets and existing
  CommonMark links.
- [x] Provide stable declaration targets and exact original source spans.
- [x] Report unresolved links as warnings in `man`, IDE/LSP and `lint`.
- [x] Derive public hover and completion documentation from `doc`, including
  defaults, constructor and role overloads, and module identity.
- [x] Refresh unsaved imported buffers on open, change and close, with
  correct hover and definition ranges.
- [x] Use the public doc inventory for std completion, including overlays,
  and invalidate the disk index when its source inventory changes.
- [x] Implement `man -k QUERY` in text and Markdown and update `man --help`.
- [x] Finish final narrow checks and update code maps.

## Choices where the ticket leaves behavior open

1. A syntactically valid, undefined CommonMark shortcut reference is a symbol
   link: `[Name]`, `[Name.member]`, `[module/path.Name]`. Identifiers follow
   the existing ASCII name convention. Existing CommonMark reference
   definitions take precedence. Code and image syntax are never symbol links.
2. Compiler scope names and import aliases take precedence over explicit
   module qualification. Lower-case single-segment module names work too.
   `std/` forces the std loader.
3. A member link names the complete public overload set: one target, every
   matching declaration's file and span. Hover uses the exact declaration
   selected by analysis, rather than an overload name alone.
4. Targets are `okkama-doc:/MODULE?version=VERSION#DECLARATION`, omitting
   the version query when the module declares none and percent encoding
   reserved bytes. Targets never depend on line numbers or overload order.
   `doc::declaration_target` exposes this convention to future renderers.
   Text pages print destinations in angle brackets; Markdown/LSP use links.
5. Unresolved and hidden targets keep their original text and produce
   `unresolved-doc-link` warnings. `man` writes warnings to stderr and exits
   successfully. `lint` retains its existing nonzero status for any warning.
   Diagnostics concern the documented/inspected file, not all dependencies.
6. Hover and completion may format content differently from a man page, but
   use the same signatures, comments, public members and resolved links.
   Selected actions show only their overload; types include all public member
   documentation. Locals, parameters and builtins retain IDE descriptions.
7. Std completion supplies current public-model documentation immediately.
   `completionItem/resolve` resolves the selected item's imports and links,
   avoiding this work for every std index entry. It rereads overlays and
   clears stale documentation when an item becomes hidden or disappears.
   Ordinary imported-symbol completion is fully resolved during inspection.
8. Search covers std, like the existing no-argument index. Matching is a
   literal substring after Unicode lowercase conversion, over module paths,
   qualified public names and existing first-sentence summaries, excluding
   bodies and later paragraphs. Results follow sorted module paths and source
   order; overloads with different summaries remain separate. Adjacent
   identical results collapse.
9. Search trims its single query argument, which can contain spaces. Empty
   queries, repeated `-k` and simultaneous module/declaration arguments are
   usage errors (2). No matches prints `No matches.` and exits 0. Source
   read/parse failures are reported, not silently skipped.

## Verification

Tests were added before implementation in existing compiler `doc`/`ide`
tests, CLI `man.rs`/`lsp.rs` test modules and `tests/cli.rs`. The std completion
regression lives in its existing inline test module. The integration client
uses stdio LSP, a 20-second response deadline, and only kills its own child.

The requested target directory was denied by the session filesystem sandbox:

```text
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/t-codex2 cargo test -p okkama-compiler --offline -j 4 --lib doc::tests
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/t-codex2 cargo check -p okkama-compiler --offline -j 4
```

Both exited 101 before compilation (`Operation not permitted`). Actual checks
use `CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-513`
inside the authorized worktree, from `okkama-rs`. Final results follow below.

Regressions failed before fixes for LSP open/close refresh, hover ranges,
resolved summary links, std disk-index refresh and immediate std completion
documentation. Mutation checks disabled link resolution, search filtering
and exact overload lookup in turn. Each corresponding test failed (101);
the original implementation was restored after each experiment.

No whole suites, workspace clippy, formatting command, Docker, Git operation,
CHANGELOG edit, code generation or Okkama program execution was performed.

## Final results (2026-11-22)

```sh
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-513 cargo check -p okkama-cli --offline -j 4
```

Result: passed, no warnings; exit 0.

```sh
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-513 cargo test -p okkama-compiler --offline -j 4 --lib doc::tests
```

Result: 21 passed; exit 0.

```sh
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-513 cargo test -p okkama-compiler --offline -j 4 --lib ide::
```

Result: 33 passed; exit 0.

```sh
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-513 cargo test -p okkama-cli --offline -j 4 --test cli man_
```

Result: 10 passed (also selects the hover/man equivalence test); exit 0.

```sh
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-513 cargo test -p okkama-cli --offline -j 4 --test cli lsp_doc_
```

Result: 2 passed; exit 0.

The added tests cover public/hidden references, overload sets, imported and
explicit external modules, multiple source files, versioned target identity,
Unicode/CRLF source spans, CommonMark code/images/reference links, CLI search
fields and invalid arguments, warning severity, hover/model/CLI equivalence,
constructor and role selection, unsaved buffer open/change/close, std overlays,
disk-index refresh, stale hidden completion resolution, and hover ranges.

The CLI inline unit tests in `man.rs` and `lsp.rs` were added but not invoked
separately: the authorized CLI commands select an integration test file. Their
behaviors are covered by the command and stdio protocol integration tests above.
No release build or broader test suite was run.

Mutation commands (each failed its test with exit 101, then source was restored):

```sh
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-513 cargo test -p okkama-compiler --offline -j 4 --lib doc::tests::declaration_links_resolve
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-513 cargo test -p okkama-compiler --offline -j 4 --lib doc::tests::search_matches
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-513 cargo test -p okkama-compiler --offline -j 4 --lib ide::tests::doc_model_drives
```

Manual smoke check, using the binary built by the integration tests:

```sh
OKKAMA_STDLIB_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/std /Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-513/debug/okkama man --no-pager -k Stdout
```

Exit 0: module, declaration, overload-summary and cross-module summary matches
were printed as plain text. The temporary `target-513` directory was removed
after verification; no build artifacts are left in this worktree.

Code maps: `python3 tools/map/gen.py` updated `files.md`; `rules.md` was already
current. `python3 tools/map/gen.py --check` passed. No open functional questions
or implementation blockers remain for Phase B. The sandbox target-directory
workaround and the CLI inline-unit invocation restriction are recorded above.

## Changed files

```text
okkama-rs/crates/okkama-compiler/src/doc/mod.rs
okkama-rs/crates/okkama-compiler/src/doc/links.rs (new)
okkama-rs/crates/okkama-compiler/src/doc/render.rs
okkama-rs/crates/okkama-compiler/src/doc/tests.rs
okkama-rs/crates/okkama-compiler/src/ide/mod.rs
okkama-rs/crates/okkama-compiler/src/ide/index.rs
okkama-rs/crates/okkama-compiler/src/ide/std_paths.rs
okkama-rs/crates/okkama-compiler/src/ide/tests.rs
okkama-rs/crates/okkama-compiler/src/lowering/mod.rs
okkama-rs/crates/okkama-cli/src/man.rs
okkama-rs/crates/okkama-cli/src/lsp.rs
okkama-rs/crates/okkama-cli/tests/cli.rs
docs/std-docs-phase-b-513.md (new)
docs/map/state.md
docs/map/decisions.md
docs/map/pipeline.md
docs/map/files.md (generated)
```
