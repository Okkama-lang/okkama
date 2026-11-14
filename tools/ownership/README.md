# Ownership oracle (#562)

Use this oracle at every step of the 0.6.0 ownership rebuild. Python 3.8+
standard library only; the binaries come from an installed toolchain with
`bin/okkama`, `bin/sparrow`, `lib/okkama/std` and the native library.

```sh
python3 tools/ownership/oracle.py MODE --toolchain /tc \
  [--root /work] [--corpus ownership --corpus std] [--pending] \
  [--bench /bench] [--program-arg 10] \
  [--a-env NAME=VALUE] [--b-env NAME=VALUE] \
  [--jobs 4] [--timeout 60] [--build-timeout 120] \
  [--out /out/artifacts] [--json /out/report.json]
```

`MODE` is one of:

- `engines`: build SPB, run Sparrow from source and SPB, compare stdout bytes, exit code
  and destroy-trace bytes. Test traces are compared as ordered blocks starting
  with `# test <name>`; both engines restart creation serials for each test.
  Empty tests retain their separator. No trace sorting or serial normalization.
- `ab`: build independently with environments A and B; execute A, A again,
  then B on Sparrow. Classify each corpus as `same`, `DIFF`,
  `nondeterministic`, `build-fail` (plus explicit infrastructure failures).
  Both determinism and A/B differences are retained, even if A is unstable.
- `stats`: the same A/A/B protocol with `SPARROW_CORE_STATS=1`; sum
  `helper region_*`, `helper created`, `region.enter, count` and all numeric
  `OKKAMA_OWNERSHIP_REPORT.totals` fields (including reasons and static/dynamic
  region operations), per corpus and globally for A/B. A2 is excluded from
  totals. Only matched pairs without instrumentation/infrastructure errors
  or nondeterminism contribute. Missing reports are explicit errors. Within a
  report, omitted zero-call helpers count as zero (Sparrow omits them); the
  unconditional heap counter proves the report exists. Shared imported functions count once per
  compiled program, as in the census totals; there is no symbol deduplication.
- `tap`: run Sparrow from source and SPB, compare their outputs/traces, and count TAP
  `ok`/`not ok` per corpus and engine. Check plan/count/order, list failing
  assertions with preceding `#` diagnostics and following diagnostics/YAML.
  SKIP/TODO counts remain visible; those assertions do not fail the run.
  Non-TAP programs are executed but excluded from TAP totals. Ownership and
  t/ programs require TAP; nonzero exits fail except the two intentional
  ownership runtime failures below.

Discovery defaults to `ownership`, `conformance`, `std`, `t`, plus `bench`
when supplied. Repeat `--corpus` to restrict it. Ownership main `*.ok` files
run normally; `--pending` adds a separate pending corpus. `rejected/**/*.ok`
are always compile-only when ownership is selected: every selected build
variant must exit 1 (not timeout, signal, launch/I/O/usage failure).
`panic-unwind.ok` and `secondary-failures.ok` must exit 1 and match the stderr
patterns documented in the corpus README; TAP mode also checks their cleanup
assertions. Other matching nonzero exits in comparison modes remain `same`:
comparison is not an assertion that every source is a successful program.

Conformance selects `cases/**/main.ok` and reads adjacent `case.json` (the repository's current `ocs-case/1` schema). The oracle copies the case tree to an isolated work directory, omitting `case.json` and `expected` as the runner does; declared `support` files are included through that copy. It applies `requires.fixtures` of kind `files` using fixture `path` and `target`, and passes `args`, `stdin`, `env`, `budget.timeout_s`, and the copied directory as cwd. Runtime timeout is capped by `--timeout`. Cases requesting `harness`, service fixtures, or fixture placeholders needing service-provided values are skipped with a reason in `summary.skipped` and per-program `skip_reason`; they are not comparison mismatches.
Std selects modules with a test
declaration using the gate's detection convention and builds/runs with `-t`.
The t/ suite selects all existing `**/*.ok` except `support/`, as ordinary
programs; it does not generate additional sources. Benchmarks select
`**/main.ok` under the supplied directory, or the supplied single file;
`--program-arg` applies only to benchmarks. This is a correctness/counter
oracle, not the alternating steady-state timing benchmark from `ab555.sh`.

Every subprocess gets closed stdin, a wall timeout, and a separate process
group killed at completion/timeout. Default concurrency is four programs.
Build and runtime cwd is the source directory for both engines and every A/A2/B
variant; each command records its cwd in the JSON report. `OKKAMA_IMPORT_DIRS` defaults to
`<root>/t/support`; std comes from the installation. Environment overrides
apply to both build and run, independently for A/B, on top of the inherited
environment. For a clean baseline explicitly set
`--a-env OKKAMA_OWNERSHIP_PROTOTYPE= --b-env OKKAMA_OWNERSHIP_PROTOTYPE=1`.
The oracle owns all four instrumentation variables and removes inherited
values. Other inherited settings are not scrubbed; run in a clean container
for reproducibility. Reports record explicit overrides, not the entire
potentially secret-bearing host environment.

Fresh artifact directories hold SPB, census, stdout, stderr and trace files.
The JSON contains per-program commands, codes, timeouts, artifact paths,
short diffs (40 lines), differing-program lists, TAP details and totals.
Comparisons use complete byte streams; displayed diffs decode as UTF-8 with
replacement. A report is always saved in the artifact directory; `--json`
writes another copy. Container paths in reports map `/out` to the host result
directory. Exit 0 means no detected comparison/TAP/rejection failure; positive
build failures are reported coverage gaps and do not fail the suite. Review
those counts even after exit 0. Exit 1 means a detected failure; exit 2 is
invalid configuration. Host execution is refused unless `--allow-host`.

## Docker and verification

Use the gate's existing `okkama-dev` image and populated
`okkama-cargo-registry` volume (see `tools/gate/docker.sh`). The wrapper does
not build the image or fetch dependencies. It copies the current checkout,
builds a fresh offline release toolchain using the former `micro_toolchain.sh`
logic, installs it in `/tc`, and runs the oracle in the same disposable
container. Only the registry is cached, preventing stale worktree builds.
No Docker socket is mounted. Network is disabled; defaults are 3 CPUs, 6 GiB,
512 PIDs, two Cargo jobs, and a 1800-second toolchain build timeout.
`ORACLE_CPUS`, `ORACLE_MEMORY`, `ORACLE_CARGO_JOBS`, `ORACLE_BUILD_TIMEOUT`
override these defaults. Each wrapper invocation builds a fresh toolchain.

Claude should run these checks (runtime checks were intentionally not run
while implementing #562):

```sh
python3 -m py_compile tools/ownership/oracle.py
python3 tools/ownership/oracle.py --help
bash -n tools/ownership/toolchain.sh tools/ownership/oracle-docker.sh

tools/ownership/oracle-docker.sh engines --corpus ownership
tools/ownership/oracle-docker.sh tap --corpus ownership --pending
# Full corpora, including build-failure coverage and known TAP failures:
tools/ownership/oracle-docker.sh engines
tools/ownership/oracle-docker.sh tap
tools/ownership/oracle-docker.sh ab \
  --a-env OKKAMA_OWNERSHIP_PROTOTYPE= --b-env OKKAMA_OWNERSHIP_PROTOTYPE=1
tools/ownership/oracle-docker.sh stats \
  --a-env OKKAMA_OWNERSHIP_PROTOTYPE= --b-env OKKAMA_OWNERSHIP_PROTOTYPE=1
# Optional external benchmark corpus:
tools/ownership/oracle-docker.sh stats --corpus bench --bench /HOST/PATH/TO/benchmarks \
  --program-arg 10 --b-env OKKAMA_OWNERSHIP_PROTOTYPE=1
```

Results default to `tools/ownership/results/{MODE}.json` plus unique
`artifacts/MODE-*` directories; `--results HOST_DIR` changes the host mount.
`--bench HOST_PATH` is mounted read-only. Additional oracle options pass
through; paths supplied to `--out`/`--json` are container paths (use `/out/…`).

Inspect all six compile-negative fixtures, intentional exit-1 diagnostics,
known pending failures, and trace diffs. Verify A=A yields no deterministic
A/B differences; force a tiny `--timeout` and confirm timeout is never `same`;
force a tiny `--build-timeout` and confirm processing continues with
`build-fail`. Cross-check helper/census totals against saved raw files and
confirm A2 is not counted. Test with `--jobs 1` and `--jobs 4` to confirm
per-program artifact isolation. Host refusal can be checked by invoking
without `--allow-host` outside Docker; do not actually execute corpus programs
on the host. No performance or runtime acceptance claim is made by the static
implementation checks.

## Two incompatible toolchains (#593)

The existing four modes retain report schema **2** and their existing exit
policy. `two-toolchains` uses report schema **3** and a mandatory coverage gate.
It consumes two **already installed** independent prefixes; it never calls
`toolchain.sh`. Both installations need their own `lib/okkama/std`, compiler,
Sparrow loader/runtime and native libraries, built for the container's platform.
A is frozen OIR-1, B is the Champ/OIR-2 candidate. Do not share ABI readers,
stdlib installations or compiler caches between them. Prefer a clean inherited
environment; use `--a-env` / `--b-env` for independent runtime/cache settings.

For a first real run, prepare the two installations and immutable revisions,
then run this command (not run during implementation):

```sh
tools/ownership/oracle-docker.sh two-toolchains \
  --a-toolchain /HOST/tc-baseline --b-toolchain /HOST/tc-p1 \
  --a-revision BASELINE_COMMIT --b-revision CANDIDATE_COMMIT \
  --a-spb-version 16 --b-spb-version 17 \
  --manifest /src/tools/ownership/p1-manifest.json \
  --filter scopes-nested-shadowing --timeout 60 --build-timeout 120
```

The wrapper mounts these prefixes read-only at `/tc-a` and `/tc-b`, keeps the
source checkout at `/src` read-only, and writes only to `/out` and disposable
container storage. It retains network isolation, CPU/memory/PID bounds, and
uses the existing `okkama-dev` image. No toolchain build occurs in this mode.
`--manifest` and all command paths are **container paths**. Keep custom manifest,
fixtures and command scripts under the mounted source tree, or provide explicit
mounts when invoking Docker directly. `--results HOST_DIR` works as before.

Equivalent direct CLI inside a bounded container:

```sh
python3 -B /src/tools/ownership/oracle.py two-toolchains \
  --root /src --a-toolchain /tc-a --b-toolchain /tc-b \
  --a-revision BASELINE_COMMIT --b-revision CANDIDATE_COMMIT \
  --a-spb-version 16 --b-spb-version 17 --out /out/artifacts --json /out/p1.json
```

Default commands are independently configurable for A and B. Each option is
a JSON argv array, executed without a shell:

- `--a-build-command` / `--b-build-command`: default
  `["{prefix}/bin/okkama","build","{source}","--target","sparrow","-o","{binary}"]`.
- `--a-run-command` / `--b-run-command`: default
  `["{prefix}/bin/okkama","run","--engine","sparrow","{source}"]`.
- `--a-exec-command` / `--b-exec-command`: default
  `["{prefix}/bin/sparrow","{binary}"]`.

All three accept `{prefix}`, `{source}`, `{binary}`, `{root}`; escape literal
braces as `{{` / `}}`. The executable must be an absolute path. For the hidden
OIR-2 developer switch decided at G1, insert the actual flag into B's build
**and** source-run arrays. The oracle does not assume a switch spelling or a
separate public OIR-2 binary. Command scripts can be installed in the prefix.
Source-run must select Sparrow; exec must use that prefix's SPB loader.

For each supported entry the runner performs **A1 build/run/exec, A2
build/run/exec, B1 build/run/exec, B2 build/run/exec**, serially (the legacy
`--jobs` option does not parallelize this mode). Every process is bounded and
has the source directory as cwd. Both repetitions rebuild independently.
Within each toolchain, source/exec and repeated execution must match exit code,
stdout, stderr and **raw trace bytes** exactly. Build artifacts are hashed but
binary reproducibility is not required. Across toolchains, stdout and failure
diagnostics must match the exact manifest contract; traces use the mapping
below. Complete stderr contracts preserve failure kind, primary/secondary/cause
order. They intentionally do not normalize stack traces. If diagnostic formats
differ, create a reviewed fixture with stable diagnostics before enabling it.

### Manifest and identity mapping

`p1-manifest.json` has schema identifier `okkama-p1-manifest/1`; the companion
`p1-manifest.schema.json` describes its JSON structure. The stdlib-only runner
validates required fields, selection, paths and mapping invariants without
fetching a schema or installing dependencies. Repeat `--filter ID` for exact
entry selection. Unknown IDs, empty selections, malformed mappings and missing
input/model files are configuration errors (exit 2).

Each entry carries a unique `id`, root-relative `source`, original corpus
`checks`, normative `rule`, `features`, `checkpoints`, `failure` (`code` and
exact `stderr`), normative ordered `destroy` events, `baseline` revision/toolchain,
`model_fixture`, `cleanup` (`static`, `journal`, `dynamic`), `expected`
(`supported`, `unsupported`) and `identities`. Supported entries also require
exact `stdout`, including all observable checkpoints; checkpoint strings and
check numbers are provenance, not a command to slice a full program or infer
expected output from runtime results. Every nonempty destroy list must have
one explicit identity per event. For example:

```json
{
  "event": "outer.Res-construction@line-20/iteration-1",
  "A": {"block": 0, "serial": 7, "type": "ownership.Res"},
  "B": {"block": 0, "serial": 2, "type": "ownership.Res"}
}
```

The stable event names are chosen from **source creation events**, including
execution occurrence for loops, and reviewed against the independent model.
The A/B serials are diagnostic trace IDs for those events, not Champ ownership
keys. The mapping is frozen before a comparison; it is never inferred from
destroy position. Containers without their own destroy have **no entry**:
numbering gaps in A are allowed, silent deletion of trace lines is not.
`type` is the exact emitted descriptor spelling for that side; the canonical
semantic identity is the shared event name.

`block: 0` means the pre-test prefix; each `# test ...` line increments the
block index, even for duplicate names or empty tests. Serials can restart in
another block. Every non-separator trace line must have the current runtime's
`TYPE SERIAL\n` format. The runner replaces the explicit `(block, serial)`
reference with its event, preserves all event/separator order, and rejects
unknown, missing, duplicate or wrongly typed identities. The result must equal
the normative `destroy` sequence and the other pipeline's mapped trace.
**No sorting, renumbering by destruction order, or raw-trace rewriting occurs.**

The manifest now selects 25 standalone fixtures in `tests/ownership-corpus/p1/`,
with 68 checkpoints and 78 creation-event identities. All 25 pass the High
producer and `verify_high`. This is **not executable P1 acceptance**: the
in-process source pipeline currently fails for all 25 entries. The gate keeps
`expected: supported` so these failures cannot become silent coverage skips.
`p1-results.json` records the observed phase failures; its null output/trace
fields mean execution was never reached. Expected stdout, diagnostics, maps and
cleanup paths in the manifest remain contracts awaiting executable validation.

`defer-return-replacement` is excluded explicitly: the producer rejects
`return from defer (#567 S3/D8)`. Its source is retained under `p1/excluded/`
and a negative producer test checks the diagnostic. Anonymous actions, closure
snapshots, `try?`, resource collections and array traversal are excluded by the
P1 plan. The retained cases use direct calls, latest defer capture, explicit
try/catch and bounded integer ranges. Missing model links are explicit JSON
`null`, never fabricated model evidence.

### Hidden source pipeline and current integration blockers

Only `okkama-cli/champ-abi` builds read `OKKAMA_PIPELINE=oir2`. The exact value
selects the independent source pipeline for both `run` and `build`; there is
no public help option. Other values, an absent variable, or a build without
the feature use the existing OIR-1 path. No phase error retries OIR-1.
Producer-unsupported input (including test profiles) exits 4 with
`unsupported in OIR-2 lowering`; analysis, verifier and downstream integration
failures exit 1. Build writes its artifact only after every gate succeeds.

The driver resolves/analyzes source directly, lowers High, verifies High,
elaborates and rewrites Own, verifies Own, lowers and verifies Low, calls
`compile_low`, then verifies and encodes SPB 17. The candidate runtime dispatches
by the explicit version header and verifies SPB 17 before JIT execution. SPB 16
continues through its existing reader/runtime so feature builds without the
source switch preserve the existing CLI path. A reader never retries another
ABI after an error. Test profiles/workers are not P1 source features.

Observed blockers in the merged prerequisites:

- Own rejects `array.new` reached through the finite Stdout source body (15 cases).
- Own's conservative analysis bound exceeds its 256 MiB limit (10 cases).
  The limit is not raised to mask the failed gate.
- A separate minimal resource source without imports passes Own/Low but
  `compile_low` rejects `OwnershipOp::Dependencies` (`deps.of`). Erasing it was
  tried and rejected by the SPB verifier as `undefined location`; no such
  verifier bypass is retained.
- Closed Atomic/Stdout/Error SPB declarations, final builtin layout overrides
  and independently registered executable external contracts are still needed.
  The driver/loader keep `NoExternalContracts` until those implementations
  exist; they do not trust contracts copied from the input artifact.
- Source `return` from defer remains an explicit producer exclusion.

The 28 in-process tests comprise the 25 source cases plus inventory, scalar
JIT execution and unsupported/no-fallback gates. Only the latter three pass.
Every source case has a named Rust test in `rust_test`; the accepted inventory
is 25 and both Rust/Python checks detect missing entries. Output capture lives
on the VM thread and uses the real stdout formatter. Destroy capture and the
file trace share `champ_trace_destroy`: `TYPE SERIAL\n`, using runtime-local
creation serials. The existing reverse-destroy trace check yields
`Resource 2\nResource 1\n` on two independent runtimes.

### Building both toolchains in Docker

Claude can build the frozen default-feature baseline and the current candidate
and run the manifest with:

```sh
tools/ownership/oracle-p1-docker.sh
```

This sibling wrapper uses the existing `okkama-dev` image and populated offline
registry volume. It freezes baseline `34342aa698e80d9e9d3d98d3e49a6c5e55842b2e`,
builds both CLIs with `cargo build --offline -j 4` in separate source/target
roots and installs separate std snapshots. Only B has `champ-abi` and
`OKKAMA_PIPELINE=oir2`; both have `OKKAMA_TRACE=off` for exact failure contracts.
The candidate source snapshot (including untracked implementation files) and
its SHA-256 are retained beside the report. The runner clears an inherited pipeline variable before applying side-local
overrides. Network is disabled; CPU, memory, PIDs, build and run time are
bounded. `--filter ID` may select one case, but does not close the complete gate.
The existing `oracle-docker.sh two-toolchains` still accepts installed prefixes.
Docker and CLI binaries were not run during this integration work.

Integration tracker:

- [x] Hidden feature-gated CLI routing, independent phase gates and scalar JIT execution.
- [x] Destroy formatter/capture and creation-event mapping validation.
- [x] Standalone source extraction, 25 High-verified cases and explicit exclusions.
- [x] Manifest-linked Rust test inventory and two-toolchain offline Docker builder.
- [x] Default/candidate check and clippy; 94 compiler, 76 Champ and 24 oracle unit tests.
- [ ] Own/Stdout integration and bounded analysis of the selected source CFGs.
- [ ] `deps.of`, final builtin layouts and closed executable std contracts through SPB/JIT.
- [ ] All 25 source runs, observed checkpoints/traces and cleanup-path execution evidence.
- [ ] Source/model/oracle acceptance and CI runtime gate; no G1/G2 completion claim.

**G1 is blocked, not complete.** The current wrapper/oracle should return 1
for the selected source build failures. Neither this manifest nor passing
handwritten Champ fixtures supplies source runtime acceptance or G2 evidence.

### Report and gate

Schema 3 retains raw build/run stdout, stderr, traces and binaries in a fresh
artifact directory. The report includes selected manifest entries and its
SHA-256; toolchain prefixes, declared immutable revisions, expected SPB versions,
actual `SPRW` big-endian u16 versions from every built binary; command arrays
and executable hashes; per-toolchain stdlib file/tree hashes; source/model
hashes; SPB hashes; environment overrides, timeouts, raw comparison details,
mapping errors and `determinism_diff`. Hashes use SHA-256. It does not attempt
to derive expected destroy order from generated IR or from the model runner.

Exit **1** is mandatory for any unexpected selected build-fail, missing binary,
SPB version mismatch, timeout, signal/launch error, missing execution, output or
order mismatch, nondeterminism, TAP failure or TAP SKIP/TODO. There is no
allow-build-fail switch. Exit **0** means every supported selected entry passed;
inspect the explicit unsupported count before claiming coverage. Exit **2**
means invalid configuration. Legacy modes retain their documented schema and
coverage-gap policy.

Oracle unit checks (fake toolchains only; no Docker or network):

```sh
python3 -I tools/ownership/test_oracle_two.py
```

Fake Python toolchains cover A/A and B/B nondeterminism, equal raw traces,
mapped equality with different serial gaps, real order differences, missing/
duplicate/unmapped identities, block-local serial restart, source/exec parity,
SPB versions, exact typed/panic and secondary diagnostics, unexpected build-fail,
timeout, TAP skip, unsupported selection and the existing single-toolchain CLI.
