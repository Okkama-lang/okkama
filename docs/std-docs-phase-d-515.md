# Source documentation, Phase D (#515)

Scope: the supplied #515 and the owner's #512 decision, sections 0.2, 7,
16 and applicable section 18 regressions. Authorized in full on 2026-11-22,
with no approval round. Work is confined to l-codex2; no language or std API
changes, Git operations or CHANGELOG edits.

## Execution tracker

- [x] Read the supplied tickets, guidance, Phase B/C records and shared model,
  CLI, lint, inline-test and doctest execution paths.
- [x] Write compiler unit and CLI integration tests before implementation.
- [x] Implement roff/man(7), static HTML and a versioned JSON model export.
- [x] Add CLI formats and whole-std export with stable declaration targets.
- [x] Add optional man-page installation and package/site integration.
- [x] Pass narrow publishing, doc/IDE, doctest and lint regressions.
- [x] Build release; manually run all four pilot doctests and inspect lint.
- [x] Verify a rendered man page, update maps and remove target-515.

## Behavior choices

1. Keep the existing `man` command and add `--format man|html|json`. Module,
   local-file and declaration lookup retain their current resolver and public
   overload-set semantics. `-k` remains text/Markdown only. Non-text formats
   never start a pager or emit ANSI styling. Publishing never compiles or runs
   examples, nor applies a new missing-docs gate.
2. Full std publishing is `man --all --format FORMAT --output DIRECTORY`.
   Both options are required, and module/declaration/search arguments are
   rejected. Without a module, plain HTML/man produce an index; plain JSON
   emits the complete model. Export success has empty stdout; source-link
   warnings remain on stderr. Read/parse/resolution/write errors fail clearly.
3. Full export uses one synthetic root importing the discovered std paths
   through the ordinary FileLoader and compiler graph. The root is never
   exported. Source declarations, signatures and links still come solely from
   `doc::Documentation`. This avoids repeatedly rebuilding dependency docs.
   Modules sort by declared identity; declarations retain source order.
4. Man output uses man(7), section `3ok`, one page per module, named by replacing
   slash with hyphen (`io-stdout.3ok`). It uses the specified optional section
   order, literal code blocks, lists and simple textual table rows. Links use
   `.UR`/`.UE` with canonical Phase B targets. Source backslashes, quotes and
   line starts are escaped so documentation cannot become roff requests.
5. HTML is a self-contained UTF-8 document with inline CSS, no JavaScript or
   remote dependencies. The catalogue has `index.html` and flat module pages
   (`io-stdout.html`), deployable below any site path. Each overload set has
   one declaration anchor. Canonical targets remain in `data-okkama-target`;
   `href` maps to the relative page/fragment. One catalogue contains one
   version per module, so filenames omit the version query; the canonical
   identity retains it. Individual pages use this same link layout and are
   intended to live beside the exported catalogue.
6. Parse the existing CommonMark subset with the existing maintained parser;
   escape raw HTML and omit unsupported executable URL schemes. Images, which
   are outside the documented subset, retain their alt text. Recognized module
   section headings now use parser ranges too, so tilde fences and indented
   code cannot accidentally become reference sections in any renderer.
7. JSON v1 is pretty-printed UTF-8 with a final LF and `schema_version: 1`.
   It exports module identity/version/file, public items/members/kinds/generated
   signatures, original comment text and resolved Markdown, link records,
   source spans, attachment notes and link warnings. Missing text/version/target
   values are `null`; no version is fabricated. Overloads are separate records
   with the same target. Spans are end-exclusive UTF-8 byte offsets; link ranges
   index original comment text. Full export writes `reference.json` and
   `schema-v1.json`, a strict JSON Schema 2020-12 artifact included by the
   compiler. Consumers select a supported schema version explicitly.
8. Optional installation is `make install MAN_PAGES=1` or `install.sh
   --man-pages`; optional packages are `make dist MAN_PAGES=1` or
   `tools/package.sh --man-pages`. Pages go to `PREFIX/share/man/man3`;
   `lib/okkama/man-pages.txt` records exactly the filenames owned by this
   installation. Generation completes in a private staging directory before
   installation. Archive uninstall preserves unrelated pages. The same pages
   are included in tar.gz, pkg, deb and rpm staging trees; no formatter is
   needed at install time. Cross packages can set `OKKAMA_DOC_GENERATOR` to
   a matching host compiler. Default installation/package behavior is unchanged.
9. Site integration is an artifact build target: `make reference
   DOC_OUTPUT=DIRECTORY`, also available directly through the CLI. The generated
   catalogue can be copied or served by okkama-docs without another maintained
   reference or a prescribed site framework. No external repository is edited.
10. Export creates its directory and overwrites only its named output files;
    it does not delete unrelated files or old pages. Use a clean directory for
    a fresh site. Invalid/duplicate page identities and the HTML `index` name
    collision are errors. Optional source/package tools respect
    `CARGO_TARGET_DIR`; the site target also accepts `DOC_BIN`.

## Verification

Tests were written first in the existing compiler `doc/tests.rs` and CLI
`tests/cli.rs`. Initial compiler tests failed with missing `doc::publish`;
initial CLI tests failed because `--format man` did not exist. Later tests
cover JSON schema conformance and incompatible records, CommonMark heading
ranges, batch graph resolution, optional staging and an actual package archive
installation/uninstallation preserving an unrelated man page.

A fixture initially expected a cross-module link in io/stdout, whose comments
contain no such shortcut. The real cross-module fixture now covers all five
formats and checks the HTML destination; the all-std test checks actual module
anchors. No production std text was changed to manufacture a link.

The first all-std debug test was interrupted (130) after revealing repeated
per-module dependency extraction; the shared graph implementation replaced
that path. One Cargo invocation from the repository root exited 101 before
building because Cargo.toml lives in okkama-rs; subsequent commands use that
working directory. Source/schema tests and final verification follow below.

Current narrow results: compiler doc 32/32, IDE 34/34, CLI doctest 11/11,
missing-docs 2/2, LSP docs 2/2; cargo check passes without warnings. All-std
exports and archive installation/uninstallation passed. The current std tree
contains 503 modules, including additions since the Phase C baseline.

Manual man rendering: macOS `/usr/bin/man` rejects GNU `-l` before consuming
stdin (exit 1). The generated page renders with `mandoc -T utf8` and the local
man's supported absolute-file invocation (both exit 0), showing NAME, declared
VERSION, SYNOPSIS, DESCRIPTION, PUBLIC API and ERRORS with readable code and
signatures. This is a host-man CLI difference, not a generator fallback.

## Final commands and results

All Cargo commands ran from `okkama-rs`, with this exact target directory.
No whole suite, clippy, formatter, Docker or Git operation was run.

```sh
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-515 cargo check -p okkama-cli --offline -j 4
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-515 cargo test -p okkama-compiler --offline -j 4 --lib doc::tests
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-515 cargo test -p okkama-compiler --offline -j 4 --lib ide::
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-515 cargo test -p okkama-cli --offline -j 4 --test cli man_
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-515 cargo test -p okkama-cli --offline -j 4 --test cli doctest_
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-515 cargo test -p okkama-cli --offline -j 4 --test cli missing_docs_
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-515 cargo test -p okkama-cli --offline -j 4 --test cli lsp_doc_
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-515 cargo build -p okkama-cli --release --offline -j 4
```

All exit 0: check without warnings; doc **32/32**, IDE **34/34**, man
**15/15**, doctest **11/11**, missing-docs **2/2**, LSP docs **2/2**.
The filters overlap on the hover/man test. The release build passed without
warnings. The full man filter includes actual all-std exports, deterministic
JSON rerendering, five-format link diagnostics, archive packaging with the
already built test binary, installation into a prefix with spaces, and
uninstallation preserving an unrelated page. Standalone helper/package tests
were also run with the `man_publishing_package` and `man_publishing` filters.

New tests: five compiler publishing tests and five CLI publishing tests, plus
the existing cross-module link test extended to all formats. The additional
CLI regression reads an ill-typed fenced example in all new formats, then
proves that `run -t` still rejects it and lint still warns about a public
undocumented declaration. Phase C regressions cover compile-only versus
explicit run, wrong expected output, compilation/runtime positions, timeout,
output/memory bounds, temporary cleanup, sandbox denial and incremental lint.

Tests-first failures (exit 101 before implementation):

```sh
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-515 cargo test -p okkama-compiler --offline -j 4 --lib doc::tests::publishing
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-515 cargo test -p okkama-cli --offline -j 4 --test cli man_publishing_formats
```

Mutation check: bypass HTML URL filtering and roff source escaping, then run:

```sh
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-515 cargo test -p okkama-compiler --offline -j 4 --lib doc::tests::publishing
```

Exit 101, exactly the two escaping tests failed: executable HTML URL admitted
and source `.unsafe` interpreted as a roff request. The exact original source
was restored byte-for-byte, its backup removed, and `doc::tests` passed 32/32
again. The release binary had been built from that same restored source.

Manual pilots, from the worktree root, each run exactly once (all exit 0):

```sh
OKKAMA_STDLIB_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/std /Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-515/release/okkama run -t std/io/stdout.ok
OKKAMA_STDLIB_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/std /Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-515/release/okkama run -t std/collections/sequence.ok
OKKAMA_STDLIB_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/std /Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-515/release/okkama run -t std/fs/file.ok
OKKAMA_STDLIB_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/std /Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-515/release/okkama run -t std/json.ok
OKKAMA_STDLIB_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/std /Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-515/release/okkama lint std/io/stdout.ok std/collections/sequence.ok std/fs/file.ok std/json.ok
```

| Module | Doctest | Inline tests |
|---|---|---:|
| io/stdout | run | 1 passed |
| collections/sequence | compile | none |
| fs/file | compile | 10 passed |
| json | run | 11 passed |

Runnable examples really executed, with no isolation fallback. Manual lint
exits 1 for five existing non-documentation warnings (three unused imports
in sequence, unused Cell import and unused found in json); **zero missing-docs**.
No std source was changed and no public inventory refresh was necessary.

Manual publishing with the release binary (each exit 0):

```sh
OKKAMA_STDLIB_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/std /Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-515/release/okkama man --all --format html --output target-515/reference-html
OKKAMA_STDLIB_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/std /Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-515/release/okkama man --all --format man --output target-515/reference-man
OKKAMA_STDLIB_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/std /Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-515/release/okkama man --all --format json --output target-515/reference-json
```

Results: **503 module man pages**, **504 HTML pages** including the index,
and **503 JSON modules**. An independent offline Python validation checked
all exported JSON against every keyword used by schema v1, balanced HTML
tags, unique anchors, canonical-link destinations and source-span bounds.
Six pre-existing source shortcut warnings are preserved (config/ini,
logging/redact, plist/value, toml/value); exports succeed and carry these
warnings instead of inventing declarations or modifying std prose.

Man smoke commands, from the worktree root:

```sh
OKKAMA_STDLIB_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/std /Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-515/debug/okkama man io/stdout --format man | MANPAGER=cat PAGER=cat man -l -
OKKAMA_STDLIB_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/std /Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-515/debug/okkama man io/stdout --format man > target-515/io-stdout.3ok
mandoc -T utf8 target-515/io-stdout.3ok > target-515/man-smoke.txt
PAGER=cat MANPAGER=cat man /Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-515/io-stdout.3ok > target-515/man-local-smoke.txt
bash -n install.sh tools/package.sh tools/install-man.sh
```

The GNU-style `man -l -` command exits 1 on this macOS man (unsupported -l).
The local-file man and mandoc commands exit 0 and render the generated page.
The shell syntax check exits 0. System pkg/deb/rpm installers were not run;
the shared generation and real tar.gz install/uninstall path were tested.

## Changed files

- `Makefile`
- `install.sh`
- `tools/package.sh`
- `tools/install-man.sh` (new)
- `okkama-rs/crates/okkama-compiler/Cargo.toml` (serde_json moved from dev to normal dependencies; lockfile dependency set unchanged)
- `okkama-rs/crates/okkama-compiler/src/doc/mod.rs`
- `okkama-rs/crates/okkama-compiler/src/doc/links.rs`
- `okkama-rs/crates/okkama-compiler/src/doc/render.rs`
- `okkama-rs/crates/okkama-compiler/src/doc/publish.rs` (new)
- `okkama-rs/crates/okkama-compiler/src/doc/schema-v1.json` (new)
- `okkama-rs/crates/okkama-compiler/src/doc/tests.rs`
- `okkama-rs/crates/okkama-compiler/src/lowering/mod.rs`
- `okkama-rs/crates/okkama-cli/src/man.rs`
- `okkama-rs/crates/okkama-cli/tests/cli.rs`
- `docs/OkkamaBook.md`
- `docs/OkkamaBook-RU.md`
- `docs/std-docs-phase-d-515.md` (new)
- `docs/map/state.md`
- `docs/map/decisions.md`
- `docs/map/pipeline.md`
- `docs/map/files.md` (generated)

## Open questions and verification boundaries

No functional questions remain for Phase D. The macOS/GNU man invocation
difference and unexecuted platform installer builders are validation boundaries
above. Site integration produces portable artifacts; no external site or
repository was edited. Existing documentation shortcut warnings and unrelated
lint warnings are retained, outside this publishing ticket's migration scope.

Final maintenance: `python3 tools/map/gen.py` regenerated files.md; rules.md
was current. `target-515` (builds, manual exports, smoke pages and mutation
backup) and the two failed/interrupted test export directories were deleted.
All execution tracker items above are verified and complete. No generated
reference copy, build directory or temporary test export remains from #515.
