# Okkama Conformance Suite (OCS)

An implementation-independent, executable specification of Okkama. Every case is
an ordinary Okkama program (`.ok`) with a JSON sidecar stating what must be
observed. The same programs and the same expectations run unchanged against
the Rust implementation (Sparrow VM) or any future interpreter, compiler, VM or JIT — only the adapter
configuration changes.

* Suite version: see `VERSION` and `release.json` (currently **0.1.0, draft**:
  coverage obligations are not complete, so no implementation can pass the
  release gate yet — see `coverage/COVERAGE.md`).
* Normative rules of the suite: [`SPECIFICATION.md`](SPECIFICATION.md).
* Adapter protocol: [`adapters/PROTOCOL.md`](adapters/PROTOCOL.md).
* Authoring cases: [`CONTRIBUTING.md`](CONTRIBUTING.md).
* Contract inventory, discrepancy ledger and design study: [`contracts/`](contracts/README.md).

## Quick start

Requirements: Python 3.9+ (standard library only) for the runner; whatever the
implementation under test needs (Rust for the Rust adapter).

```sh
cd conformance
./ocs validate                                   # metadata, discovery, contracts, profiles, ledger
python3 runner-tests/test_runner.py              # runner self-tests (deliberate failures)
python3 runner-tests/test_schema_sync.py
python3 runner-tests/test_rust_adapter.py        # Rust adapter: engines, build caches, Champ translation
python3 runner-tests/test_champ_blockers.py      # Champ failure classification and ranking

./ocs run --adapter adapters/rust/adapter.json --profile full-release -j 8
./ocs run --adapter adapters/rust/adapter.json --profile bootstrap
./ocs run --adapter adapters/rust/adapter.json --profile full-release --adapter-env OCS_RUST_ENGINE=sparrow
./ocs run --adapter adapters/rust/adapter.json --profile full-release --case L05   # id prefix filter

./ocs coverage --results reports/runs/<run>      # contract -> cases -> results
./ocs gate --results reports/runs/<run>          # release-gate verdict (+ --out claim.json)
./ocs gate --profile frontend --results ...      # milestone verdict, never a release claim
./ocs diff --left reports/runs/<previous> --right reports/runs/<sparrow>
./ocs mutate --adapter adapters/rust/adapter.json   # mutation controls must all be killed
```

Each run writes `results.jsonl` (raw evidence per case: command, request,
response, stdout/stderr bytes, process status, source hashes), `summary.json`
and `summary.md`. Non-passing cases keep their temporary directory for
inspection (`--keep failed`).

The Rust adapter uses Sparrow. Removed engines are rejected (#584, #585).
Results for one engine do not establish support in another. The selected engine appears in each run's adapter configuration.

### Champ conformance and the blocker ranking

`adapters/rust/adapter-champ.json` selects `OCS_RUST_ENGINE=champ`: the
adapter builds `okkama-cli --features champ-abi` into
`adapters/rust/.target/engine-champ` (its own target directory and probe
cache, never shared with the Sparrow build) and runs every case with
`OKKAMA_PIPELINE=oir2`. `parse`/`check` go to the native endpoint (the shared
front end); `run` first checks the sources there, then runs
`okkama run --stdlib STD SOURCES -- ARGS` on Champ and translates its exit
status and failure report; `test` is `okkama run -t`, which OIR-2 does not
support yet, so those cases are UNSUPPORTED with a classified reason. To see
what blocks Champ, run the same profile on both engines and rank the Champ-only
failures:

```sh
./ocs run --adapter adapters/rust/adapter.json       --profile full-release -j 6 --out /tmp/runs/sparrow
./ocs run --adapter adapters/rust/adapter-champ.json --profile full-release -j 6 --out /tmp/runs/champ
python3 tools/champ_blockers.py /tmp/runs/champ --baseline /tmp/runs/sparrow \
    --json /tmp/runs/blockers.json [--compare yesterday/blockers.json]
# or all of it, in an isolated container (offline):
OCS_JOBS=6 tools/run_champ.sh /path/to/repo /tmp/runs
```

The report counts passes, cases failing on both engines (not Champ's
problem) and Champ-only failures by stage: unsupported in OIR-2 lowering, SPB
17 generation gap (`not yet in S1/S2`), loader rule, runtime failure or crash,
wrong output, timeout, test mode, driver unavailable, other unsupported, not
run. Then it ranks blockers: each failure's first diagnostic line with spans,
numbers, paths, quoted text and symbol names removed (`unsupported in OIR-2
lowering: closure capture of x in function main` becomes `unsupported: closure
capture`, `GenerateError("type Map(..): not yet in S1")` becomes `S1 type
Map`), with case counts and up to five example ids per key. `--json` keeps the
same data for day-to-day diffs; `--compare` prints new passes, new failures and
blockers whose counts moved.

## Locating the implementation and the standard library

* The standard library root defaults to `../std` (relative to this directory,
  set in `release.json`) and can be overridden with `OCS_STDLIB_ROOT=/path` or
  `--stdlib-root /path`. It is passed explicitly to every adapter.
* The Rust adapter builds `OCS_RUST_IMPLEMENTATION_ROOT` (default
  `../../okkama-rs`). Override with
  `--adapter-env OCS_RUST_IMPLEMENTATION_ROOT=/elsewhere/okkama-rs`.

If the suite is extracted into its own repository, keep this directory layout
and point those settings at the implementation and library checkouts.

## Layout

```
conformance/
  ocs                    command-line entry point
  README.md SPECIFICATION.md CONTRIBUTING.md VERSION release.json
  schema/                JSON Schemas (generated from runner/ocs/model.py)
  contracts/             contract inventories, std inventory, discrepancy ledger, design study
  cases/L00-...L13-...   one directory per case: case.json + main.ok (+ modules, data)
  harness/okkama/        Okkama support module copied into harness cases (conformance/check)
  fixtures/              suite-owned loopback service fixtures
  adapters/              adapter protocol and the Rust adapter
  runner/ocs/            runner package (stdlib-only Python)
  runner-tests/          runner self-tests, fake adapter, mutation controls
  profiles/              fixed profile definitions
  coverage/              generated coverage matrix
  defects/               implementation defect records referenced by cases
  reports/               run outputs, gate/diff/mutation reports, final report
  tools/                 std inventory, schema writer, ledger renderer
```

## What a pass means

`ocs gate` applies the release rule of `SPECIFICATION.md` §2: all mandatory
cases PASS in all member profiles, from the same suite tree, with complete
coverage and no unresolved contract. Profile runs report counts of every
status; unsupported, blocked or not-run cases are never folded into a
percentage of "supported" passes.
