# runtime/diff migration experiment (#13)

2026-11-22, worktree `l-codex2`. **Rejected: retain the native.**

The supplied task authorizes this one-module experiment with the existing
public std API, overriding the older ticket's planning-only/API-redesign scope.
No language, compiler, uuid, Git, CHANGELOG or public API changes.

## Decision and implementation

The actual native is Algorithm::Diff LCSidx (Hunt–Szymanski), **not Myers** as
previously stated in the matrix. It trims equal prefixes/suffixes, indexes right
line positions in descending order, maintains an increasing threshold frontier
with binary search and the previous-slot shortcut, then reconstructs links.
The deletion-before-insertion ordering and all original indexes matter.

[candidate.ok](candidate.ok) ports that algorithm. It was installed temporarily
behind the existing `DiffSearching` call, retaining `DiffLines`, edit conversion,
renderers, errors and `Diff.script()`'s once cache. The candidate's input contract
is already validated arrays, not a new public interface. An initial local-Cell
implementation was refused by the existing `once let` effect checker before
execution; the measured implementation instead uses pure immutable frontier
states, persistent arrays/maps and shared list links. No checker bypass, new
primitive or caching change was introduced.

The short case adds only 2.49 µs/call, which alone would not reject a port under
the owner's absolute-time policy. At 512 lines the increase is 127 µs for one
replacement, 487 µs for disjoint inputs, and **20.88 ms** with repeated lines
(30.43×). This substantially exceeds both the 1.5× target and a negligible
absolute difference, so production continues to call `runtime/diff::diff_script`.
ID 141 stays active. ABI registration, origin dispatch, PURE classifications,
provider bridge, catalogue counts and `std/runtime/diff.ok` remain unchanged.
The experimental implementation lives only in this report directory.

## Method and measurements

A one-off local Sparrow experiment, release profile (fat LTO), built offline from
this worktree before writing the candidate. There is only one supported backend;
Sparrow calls the origin implementation in runtime-core. Inputs are generated
outside timed spans. Each workload has 20 warm-up calls followed by 5 timed
batches; reporting, JIT startup and input generation are excluded. Each batch
observes a checksum of the returned script/edit count. Checksums match across
implementations. Values below are **median µs per call**; all individual timings
and batch counts are retained in [native-times.txt](native-times.txt) and
[candidate-times.txt](candidate-times.txt).

- Small: `[a,b,c,d]` versus `[a,x,c,d,e]`, 2,000 calls/batch.
- Medium/large: 128 or 512 unique `line i` lines; sparse replaces the middle
  line, disjoint uses `other i` throughout, 100 calls/batch.
- Repeated: `(i % 4).string()` versus `((i + 1) % 4).string()`, 10 calls/batch.
  There is no equal prefix or suffix; this exercises many equal line pairs.

Raw measures the script producer; API measures a fresh `Diff(...).edits()` on
every iteration, including ordinary public validation and edit construction.
Raw native also validates its arrays; raw candidate relies on `DiffLines` when
used via the API and does not repeat validation. Thus raw figures favor the
candidate; the acceptance decision uses equivalent whole API operations.
There is no parser/serializer split for this operation.

| Lines/workload | Native raw | Okkama raw | Native API | Okkama API | API ratio | API increase |
|---|---:|---:|---:|---:|---:|---:|
| small | 0.971 | 3.759 | 1.688 | 4.173 | 2.47× | +2.485 |
| 128 sparse | 13.330 | 43.820 | 25.651 | 57.407 | 2.24× | +31.757 |
| 128 disjoint | 26.360 | 137.898 | 40.936 | 151.482 | 3.70× | +110.547 |
| 128 repeated | 51.233 | 1204.713 | 62.067 | 1216.054 | 19.59× | +1153.987 |
| 512 sparse | 47.163 | 172.947 | 95.358 | 222.336 | 2.33× | +126.978 |
| 512 disjoint | 100.177 | 584.461 | 156.285 | 643.191 | 4.12× | +486.906 |
| 512 repeated | 667.763 | 21174.821 | 709.458 | 21586.496 | 30.43× | +20877.038 |

Scaling: sparse 128→512 is 3.72× native / 3.87× Okkama; disjoint is
3.82× / 4.25×. Repeated lines generate R=N×M/4 matching pairs; growing each
side 4× gives 16× pairs and costs 11.43× / 17.75×. This is the existing
match-pair-heavy LCS behavior (roughly O((N+M+R) log L)), not Myers O(ND).
Persistent replacement copies a tree path; links share tails. No extra full
N×M dynamic-programming table or growing flat-copy loop was introduced.
The existing 512-line bound limits R to 262,144. Prefix/suffix and binary search
are tail recursive and bounded by the input limit/logarithmic frontier size.

Separate full-process RSS runs (including compilation/JIT, warm-up and all
workloads) used `wait4`, not an allocation counter: native **32.28 MiB**,
candidate **34.94 MiB**. See [native-rss.json](native-rss.json),
[candidate-rss.json](candidate-rss.json) and their `*-rss-times.txt` logs.
These are process peaks, not per-call allocations; no allocation-count claim
is made. `time -l` cannot query `kern.clockrate` in the current sandbox.
The repository cage also fails before launch because its `ps` monitor is denied;
these pure, finite, narrow programs ran directly in the current sandbox.

## Correctness and tests

[differential.ok](differential.ok) calls the still-available native and candidate
on exactly the same arrays and compares **every integer of the entire script**.
[Result](differential-result.txt): **4,981 inputs, zero differences**:

- All 3,969 pairs of binary sequences of lengths 0..5, including ties/repeats.
- 1,000 deterministic generated pairs (seed 130017, LCG 48271 mod 2147483647),
  lengths 0..32, with empty strings, CR, tab, Cyrillic, emoji and composed /
  decomposed Unicode as distinct line contents.
- 12 bounded cases at 128 and 512 lines: equal, empty on either side, sparse,
  disjoint, and shifted repeats.

`std/diff.ok` has 9 passing inline tests (4 existing, 5 added). The added tests
pin repeated-line tie order and indexes, empty/Unicode/CR lines, inclusive
512-line / 4,096-scalar / 1,048,576-scalar-per-side limits, both-side invalid
inputs, LF refusal, and the rendering budget error for both formats.
All validation and error construction remains the original code.
The direct std importer is `std/testing/check.ok` (one test).
Both modules pass with native and with the candidate behind the same API;
see `std-*-result.txt` and `importer-*-result.txt`.

[mutations.py](mutations.py) changes implementation code, runs only the associated
test, requires a real test failure (not merely a compiler failure), and restores
`std/diff.ok` in `finally`. **9/9 mutations caught**, one for every inline test;
[exact argv and output](mutations-result.txt). Filter names below are passed as
one quoted argument to `okkama run -t --filter FILTER std/diff.ok`:

| Test filter | Implementation mutation |
|---|---|
| `unchanged lines` | map equal edits as removed |
| `one comparison` | default unified context 3 → 0 |
| `the ratio` | numerator 2×matched → matched |
| `edits walk` | left index x → x+1 |
| `repeated lines` | equal opcode check 0 → 1 |
| `line count accepts` | maximum 512 → 511 |
| `lines accept` | maximum 4096 → 4095 |
| `aggregate scalar` | maximum 1048576 → 1048575 |
| `rendering refuses` | output maximum 1048576 → 4194304 |

Narrow Rust checks: runtime-abi `catalogue` **5 passed**, `wire_ids` **1 passed**;
Sparrow `budget` filter **6 passed**. No Rust implementation changed, so no
additional `cargo check` was needed. No broad suites, Docker, clippy or cargo fmt.

## Exact reproduction commands

From the worktree root (the build directory is deleted after verification):

```sh
export CARGO_TARGET_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/target-13
export OKKAMA_STDLIB_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/std
cd okkama-rs
cargo build -p okkama-cli --release --offline -j 4
cargo test -p okkama-runtime-abi --offline -j 4 --lib catalogue
cargo test -p okkama-runtime-abi --offline -j 4 --lib wire_ids
cargo test -p sparrow-core --offline -j 4 --lib budget
cd ..
target-13/release/okkama run docs/diff-13/native-bench.ok
python3 docs/diff-13/candidate-api.py docs/diff-13/candidate-bench.ok
target-13/release/okkama run docs/diff-13/differential.ok
target-13/release/okkama run -t std/diff.ok
target-13/release/okkama run -t std/testing/check.ok
python3 docs/diff-13/candidate-api.py -t std/diff.ok
python3 docs/diff-13/candidate-api.py -t std/testing/check.ok
python3 docs/diff-13/mutations.py
python3 docs/diff-13/rss.py target-13/release/okkama run docs/diff-13/native-bench.ok
python3 docs/diff-13/candidate-api.py --rss docs/diff-13/candidate-bench.ok
cd conformance
python3 tools/inventory_std.py ../std --write
cd ..
python3 tools/map/gen.py
```

`candidate-api.py` temporarily switches only the hidden collaborator import and
call in `std/diff.ok` and always restores it, including after failures. Run it
sequentially with mutation tests and other runs of that module.

## Completion and changed files

- [x] Inspect ticket, matrix, url_text retirement and diff implementation/callers.
- [x] Measure native first, on small/medium/large lines in release.
- [x] Implement and temporarily test a pure Okkama replacement behind the same API.
- [x] Differential comparison against the retained native, program preserved.
- [x] Measure candidate and decide using absolute and relative timings.
- [x] Reject the candidate and restore the production native call.
- [x] Add std tests, confirm all 9 with mutations, retest after restoration.
- [x] Verify direct importer, ABI catalogue/wire IDs and Sparrow budgets.
- [x] Update matrix/results, progress/decisions and run inventory/map generators.
- [x] Delete the experiment's `target-13` build directory after checks.

Changes: `std/diff.ok` (tests only), `docs/format-natives-13.md` (diff row/result),
`docs/map/state.md`, `docs/map/decisions.md`, and this `docs/diff-13/` evidence
bundle. Generated inventories/maps are refreshed; the public declarations and
runtime catalogue are unchanged. uuid is untouched.

Open questions: none required to complete this module. A future performance
experiment may revisit the immutable frontier cost; this result does not justify
a language change, additional primitive or native removal.
