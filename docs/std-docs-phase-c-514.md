# Source documentation, Phase C (#514)

Scope: the supplied ticket #514 and the owner's decision on #512, sections
12, 14, 16 and the applicable regressions in section 18. The owner authorized
implementation of the whole ticket on 2026-11-22, with no approval round.
No language or public std API changes. Phase D remains separate.

## Execution tracker

- [x] Read both supplied tickets, project guidance, Phase B, code maps and
  the existing doc model, CLI, resolver, IDE lint and test execution paths.
- [x] Add compiler unit and CLI integration regressions before implementation.
- [x] Extract three example kinds from the canonical source doc model.
- [x] Compile applicable blocks through analysis, lowering, code generation
  and verified SPB encoding, preserving imports and original source positions.
- [x] Execute explicitly runnable examples with deterministic expected output
  in a bounded child with no host capabilities, or explicitly compile only.
- [x] Add user warnings and completed-wave std errors for missing public docs.
- [x] Count real public declarations, including overloads, enum cases and
  computed cap fields, excluding hidden declarations and implementation code.
- [x] Document and enforce the four pilots. Edit fs/file last, comments only.
- [x] Record initial std coverage and reject completed-wave regressions.
- [x] Verify that disabling output comparison or std enforcement breaks tests;
  restore the exact original source after each mutation.
- [x] Finish final narrow checks and the release build.
- [x] Run each pilot's doctests manually once and inspect missing-docs lint.
- [x] Update code maps and remove target-514 and temporary task backups.

## Convention and behavior choices

1. Fenced `okkama` and `okkama,compile` blocks are compile-checked by default.
   `okkama,ignore` is illustrative and never checked. Untagged, indented and
   other-language blocks are illustrative: they carry no Okkama language tag.
   Unknown or conflicting Okkama metadata is an error, including space-based
   spellings such as `okkama run`; marked examples are never silently skipped.
2. `okkama,run` opts into execution and requires the immediately following
   `output` fence. Its content is exact UTF-8 stdout, including LF line endings
   and the final newline; an empty fence expects no stdout. Success also
   requires exit status 0 and empty stderr. No output normalization or shell
   interpolation is performed. Fixtures in runnable examples are explicit
   literal data in the example; examples needing external files remain
   compile-only, without a new fixture directive or host permissions.
3. Examples without a `module` declaration inherit the documented file's
   module/version, imports and declarations from all supplied root files.
   Statements get a generated integer main ending in 0; declaration snippets
   get an empty main when needed. A complete example with its own `module`
   uses its own declarations and imports, resolved relative to the original
   file. Original root main actions are omitted from the example's context.
   An example's own main never authorizes execution by itself.
4. `run -t` checks root-file doctests before running existing inline tests.
   Dependency doctests do not run automatically. A module with doctests and
   no inline tests can succeed. The existing repeated literal `--filter`
   applies to names `doctest FILE:LINE` as well as inline test names. An
   unrelated filter does not execute a doctest.
5. Test/fake bodies in examples are parsed in full and checked/lowered with
   the existing test profile, rather than silently skipped in production
   parsing. Such profiles remain compile-only even with `okkama,run`: the
   existing inline-test executor has no bounded execution path. This is an
   explicit isolation fallback, not an ignored compile check.
6. Runnable examples use a private directory (0700 on Unix), a separate copy
   of the current executable, null stdin and a cleared environment containing
   only temporary HOME/TMPDIR, TZ=UTC and LANG=C. All Sparrow host-native groups
   are denied: filesystem, network, processes, environment, clocks, random
   sources and terminal input. Unknown natives fail closed. Computation and
   stdout/stderr remain available. No worker-spawning command is supplied.
   Denied operations fail the example; no repository writes or network access
   are granted to make an example pass.
7. Limits are fixed: 3 seconds of wall time, 64 MiB process-allocation growth,
   256 MiB whole-process peak RSS and 64 KiB combined stdout/stderr. The parent
   bounds captured output and kills/reaps its own child. The child's independent
   watchdog samples RSS every 10 ms and also enforces the timeout, including
   time in native calls. RSS monitoring uses the safe cached nix resource API;
   Darwin reports bytes, other supported Unix systems report KiB. The normal
   runtime memory budget also bounds stack and JIT work. These are sampled
   memory bounds, as in the repository's local cage, not a VM reservation.
8. Missing process-memory monitoring/probing, a platform without that monitor,
   inability to create the temporary directory/write bytecode/start the child,
   or an unbounded test profile produces a visible
   `compile-only: isolation unavailable (REASON)` result after successful
   compilation. It never falls back to ordinary unrestricted execution.
9. Syntax/semantic errors map generated bytes back to the exact original
   comment, including UTF-8 and CRLF. Runtime instruction positions are
   returned through a bounded harness-owned trace sidecar and mapped by the
   parent to original files. Output mismatches, limits and setup errors name
   the original fence. Temporary bytecode/sidecars are cleaned on success,
   failure and timeout; early-return guards kill/reap the child too.
10. Coverage uses the shared public model: objects, roles, enums, constants,
    natives, constructors, overloads, actions, caps, computed cap fields and
    enum cases. Module overviews, stored fields, main, tests, fakes, destroy
    and all hidden API are outside the denominator. Empty comments do not
    count. A parent comment cannot document an inline child implicitly.
11. Inline enum code can stay unchanged: ``- `case`: Description.`` list entries
    in the enum comment document named cases. A directly attached case comment
    takes precedence. CommonMark code blocks cannot supply case documentation.
    The model exposes case/field signatures to existing renderers and links;
    the same original link gets only one warning if present in both enum prose
    and a derived case description.
12. `missing-docs` warns in user lint/IDE diagnostics and in unfinished std
    areas. Only files resolved by the existing std loader to an entry in
    `doc/completed-waves.txt` receive errors. Spoofing a std module name in a
    user file cannot enable std errors; changing the module declaration inside
    a completed std file cannot disable them. `check`, `ir`, `build` and
    `run` also apply that completed-wave gate to their root files. Normal
    frontend diagnostic formatting and locations are preserved. Existing
    `lint` exit status 1 for warnings remains unchanged. No global std error.

## Coverage baseline (2026-11-22)

The reproducible compiler coverage test traverses source modules, parses their
public model, prints the counts and asserts 100% for every completed wave.
It also rejects deleted or renamed completed modules. This is a source/API
count, not a line/comment count.

| Area | Documented | Public declarations |
|---|---:|---:|
| io/stdout | 14 | 14 |
| collections/sequence | 8 | 8 |
| fs/file | 40 | 40 |
| json | 111 | 111 |
| Completed pilot wave | 173 | 173 |
| All 499 std modules | 10069 | 11722 |

The 1653 undocumented declarations outside the completed wave remain warnings;
this ticket does not migrate those areas or introduce a global gate. Adding a
public declaration to a completed area immediately requires its documentation.

All std edits add doc comments only. In particular, fs/file was edited last
among std files, and an exact comparison excluding comment-only lines verified
that every other byte/line stayed unchanged. Enum case documentation uses
named comment entries to preserve the original inline declarations. The
conformance public-API inventory does not need regeneration: no declarations
or signatures changed.

## Verification

Tests live in the existing compiler `doc/tests.rs`, `ide/tests.rs` and CLI
`tests/cli.rs`. Added tests cover example categories, metadata errors,
main safety, compile failures and source mapping, exact output/mismatch,
timeout, allocation/output limits, sandbox permissions, cleanup, unavailable
isolation, local imports/multiple roots, full-module context, test/fake bodies,
filters, runtime locations, user/std identity, warning/error severity, real
coverage and no completed-wave regressions. The non-Unix fallback regression
is conditionally compiled there; this macOS session exercises the Unix path.

The first compiler doctest command failed with missing APIs (exit 101). The
CLI doctest tests also failed before implementation, after correcting invalid
constructor fixtures. Additional focused regressions first failed for wrong
syntax/runtime locations, missing fallback, skipped test bodies, duplicate
source-link warnings, altered frontend diagnostics and renamed-wave bypass.
These failures drove implementation fixes, not relaxed assertions.

All Cargo commands below run from `okkama-rs`, with the target directory
explicitly inside the authorized worktree. No whole suite, workspace clippy,
formatting command, Docker, Git operation or CHANGELOG edit is used.

One negative-output assertion initially matched digits in the temporary
file's process id. It now rejects an actual output line `42`, preserving the
main-safety expectation without treating a source pathname as program output.

Final narrow commands (all exit 0):

```sh
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-514 cargo check -p okkama-cli --offline -j 4
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-514 cargo test -p okkama-compiler --offline -j 4 --lib doc::tests -- --nocapture
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-514 cargo test -p okkama-compiler --offline -j 4 --lib ide::
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-514 cargo test -p okkama-cli --offline -j 4 --test cli doctest_
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-514 cargo test -p okkama-cli --offline -j 4 --test cli missing_docs_
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-514 cargo test -p okkama-cli --offline -j 4 --test cli man_
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-514 cargo test -p okkama-cli --offline -j 4 --test cli lsp_doc_
```

Results: check passes without warnings; doc 27/27; IDE 34/34; doctests 11/11;
missing-docs 2/2; man 10/10; LSP docs 2/2. The man filter also selects one
hover/man equivalence test included in the LSP filter; counts overlap there.
The doc test prints the exact coverage baseline above.

Mutation checks (each exited 101, as intended):

```sh
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-514 cargo test -p okkama-cli --offline -j 4 --test cli doctest_runnable_expected_output
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-514 cargo test -p okkama-cli --offline -j 4 --test cli missing_docs_user_warning
```

The first bypassed exact output comparison; the invalid expected-output
regression failed because it incorrectly received success. The second disabled
completed-wave file enforcement; the std regression failed on warning rather
than error. Both source backups were restored byte-for-byte and removed;
subsequent final checks above pass.

Initial tests-first commands that demonstrated missing behavior (exit 101):

```sh
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-514 cargo test -p okkama-compiler --offline -j 4 --lib doc::tests::doctest
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-514 cargo test -p okkama-cli --offline -j 4 --test cli doctest_
```

Other focused failing regressions were invoked with these exact commands
before their respective fixes (exit 101):

```sh
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-514 cargo test -p okkama-compiler --offline -j 4 --lib doc::tests::missing_docs
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-514 cargo test -p okkama-compiler --offline -j 4 --lib doc::tests::std_documentation_coverage -- --nocapture
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-514 cargo test -p okkama-compiler --offline -j 4 --lib doc::tests::inline_enum
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-514 cargo test -p okkama-cli --offline -j 4 --test cli doctest_memory
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-514 cargo test -p okkama-cli --offline -j 4 --test cli doctest_runtime_failure
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-514 cargo test -p okkama-cli --offline -j 4 --test cli doctest_syntax
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-514 cargo test -p okkama-cli --offline -j 4 --test cli doctest_compile_checks_test
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-514 cargo test -p okkama-cli --offline -j 4 --test cli missing_docs_
```

The additional self-import regression passed immediately: the existing module
graph already supports importing the documented file through a caller alias;
no alternative resolver or special self-import behavior was introduced.

Release build (from `okkama-rs`, exit 0, no warnings):

```sh
CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-514 cargo build -p okkama-cli --release --offline -j 4
```

Manual pilot runs (from the worktree root, each executed once and exited 0):

```sh
OKKAMA_STDLIB_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/std /Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-514/release/okkama run -t std/io/stdout.ok
OKKAMA_STDLIB_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/std /Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-514/release/okkama run -t std/collections/sequence.ok
OKKAMA_STDLIB_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/std /Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-514/release/okkama run -t std/fs/file.ok
OKKAMA_STDLIB_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/std /Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-514/release/okkama run -t std/json.ok
```

| Module | Doctest | Existing inline tests |
|---|---|---:|
| io/stdout | run, exact output matched | 1 passed |
| collections/sequence | compile | none |
| fs/file | compile | 3 passed |
| json | run, exact output matched | 11 passed |

The two runnable examples really ran; neither used the isolation fallback.
The compile-only file example did not read notes.txt. No fs inline-test
scratch directory remained after the manual runs.

Manual lint:

```sh
OKKAMA_STDLIB_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/std /Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-514/release/okkama lint std/io/stdout.ok std/collections/sequence.ok std/fs/file.ok std/json.ok
```

Result: **zero missing-docs findings**. Exit 1 reflects five existing unrelated
warnings: unused imports Iterating, OutOfRange and Sizing in sequence; unused
Cell import and unused local found in json. Those code changes are outside
this documentation ticket and are intentionally left intact.

Final maintenance (from the worktree root):

```sh
python3 tools/map/gen.py
```

Exit 0: files.md regenerated; rules.md already current. No source changed
after generation. A final idempotent run verifies the maps again. The
`target-514` directory and all temporary task backups were deleted after
verification. No build artifact or std test scratch directory remains from
this task. No Git operation or commit was performed.

## Changed files

- okkama-rs/crates/okkama-compiler/src/doc/mod.rs
- okkama-rs/crates/okkama-compiler/src/doc/examples.rs (new)
- okkama-rs/crates/okkama-compiler/src/doc/coverage.rs (new)
- okkama-rs/crates/okkama-compiler/src/doc/completed-waves.txt (new)
- okkama-rs/crates/okkama-compiler/src/doc/links.rs
- okkama-rs/crates/okkama-compiler/src/doc/tests.rs
- okkama-rs/crates/okkama-compiler/src/ide/index.rs
- okkama-rs/crates/okkama-compiler/src/ide/lint.rs
- okkama-rs/crates/okkama-compiler/src/ide/tests.rs
- okkama-rs/crates/okkama-cli/src/main.rs
- okkama-rs/crates/okkama-cli/src/doctest.rs (new)
- okkama-rs/crates/okkama-cli/Cargo.toml
- okkama-rs/Cargo.lock
- okkama-rs/crates/okkama-cli/tests/cli.rs
- std/io/stdout.ok
- std/collections/sequence.ok
- std/json.ok
- std/fs/file.ok
- docs/std-docs-phase-c-514.md (new)
- docs/OkkamaBook.md
- docs/OkkamaBook-RU.md
- docs/map/state.md
- docs/map/decisions.md
- docs/map/pipeline.md
- docs/map/files.md (generated)

## Open questions

None for Phase C. Compile-only fallbacks and the wave boundary are deliberate
policy choices described above. Global std enforcement waits for later
completed migrations, as the owner specified.
