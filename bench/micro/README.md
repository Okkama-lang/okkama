# Micro-benchmarks (protocol v2)

The suite contains **64 language programs** and **60 std/builtin programs**.
`manifest.json` is the machine-readable inventory. `coverage.py` reports declared
measurement targets, not incidental imports or whole-module API coverage.

These measure **optimized composite iterations**, including the loop, checksum,
any per-iteration construction and dispatch. They do not directly measure an
isolated CPU instruction, allocation, compiler phase or raw method-call latency.
The compiler may legally inline, fold, hoist or eliminate work. Inspect generated
code and use separate instrumentation before attributing a change to one operation.

## Protocol and measurements

All programs import `support/harness.ok`. Arguments are `N WARMUP`, with positive
N and nonnegative WARMUP. A single Bench instance runs the warmup and the measured
batch. Input parsing, fixture construction, printing and JVM process startup are
outside the reported clock interval. The interval still includes the call into
Bench, GC, scheduler work and any compilation/tier transitions occurring inside it.
The warmup checksum is printed too, so warmup is observable.

Output is exactly:

```
micro-v2 N WARMUP ELAPSED_NS CHECKSUM WARMUP_CHECKSUM
```

This is a **warmed batch**, not proof of stationary steady state. HotSpot can still
compile during measurement. Establish warmup sensitivity with W/2W/4W and inspect
compilation/GC counters in separate diagnostic runs. Do not mix instrumented and
uninstrumented samples or subtract JIT timing from wall time to invent steady state.

`oracles.py` specifies independent expected checksums for every case and N.
The runner checks both warmup and measurement. Before timed samples it checks
N=17 and N=31. Matching engine failures or matching wrong answers are never success.
These checksums catch important errors but do not prove that all intended work
survived optimization; full semantic tests remain necessary.

The primary result is **gross ns/iteration**. There is **no automatic baseline
subtraction**. `baseline` contains recursion, arithmetic and checksum accumulation;
it is not an empty loop and is not interchangeable with async loops or allocators.
`baseline-async` provides a separate successor/for-loop control. Use both as
context, not as a universal correction. Negative differences are not clamped into
fictional zero-cost operations.

`event-many-waiters` measures one batch of eight tasks per iteration, not one task.
N is identical across engines; there is no hidden N/8 adjustment. Preserve N when
comparing because it can change input distribution, GC frequency and JIT behavior.

## Running safely

The runner requires Docker and a **Linux** Okkama installation. It never falls
back to host execution and does not lower priority with `taskpolicy`.
The image must already exist locally and contain the runtime dependencies, Python
for debugging if desired, `sha256sum`, and JDK 22+ for JVM measurements.

```sh
python3 run.py --image okkama-dev \
  --toolchain /path/to/frozen-linux-install \
  --stdlib /path/to/matching-stdlib \
  --okkama /toolchain/bin/okkama \
  --engines sparrow --runs 7 --n 10000 --warmup 10000 \
  --label candidate-unique-id --build-profile "release fat-LTO" --revision COMMIT
```

Alternatively, use a compiler already installed in an immutable image and pass
its absolute container path without `--toolchain`. An external toolchain requires
an explicit std directory; it must be a complete matching installation, not a
host macOS binary. The runner does not silently borrow std from `okkama-new`.
Crypto-dependent programs also require the matching native library in that installation.

`--engines tomato` remains an alias for Sparrow. Since 0.6.0 Sparrow is the only engine and `okkama run` has no
`--engine`; the runner passes it only to an older toolchain that knows it (whose default before #584 was IR). `--only NAME,NAME` validates all
names; `--plan` prints the selection without executing it. `--n` is now one positive
integer, not the old per-engine assignment string. JVM artifacts are built once
per campaign inside the same limited environment. IR/Sparrow source setup occurs
outside the internal measured interval; it can still affect process history.

Every invocation has a wall deadline, CPU-time ulimit, CPU quota, memory+swap cap,
PID/FD caps, bounded stdout/stderr, no network, read-only root/source/toolchain and
a limited temporary filesystem. The runner explicitly removes the named container
on completion/error/timeout; killing the Docker CLI alone is insufficient.
No credentials, home or Docker socket are mounted. Resource failures invalidate
measurement; they are not timing samples. Do not change the machine's resource
limits between baseline and candidate.

**Do not benchmark while a gate, rebuild or other benchmark is running.** One CPU
of container quota is not a dedicated CPU. Record host load and thermal/power
conditions externally when producing release evidence. Container limits preserve
safety and reproducibility, not immunity to shared-host noise.

## Noise, GC and comparison

The runner shuffles program/engine order each round with a recorded seed and
stores every elapsed/checksum sample. At least three runs are required; seven is
the default. Median, range, MAD and full-range spread are preserved.

A complete cell is `VALID` only if every batch lasts at least `--min-ms` (default
10 ms) and full-range spread is at most 10%. Otherwise it is `INCONCLUSIVE`.
A failed or partial cell is `INVALID`; an earlier successful sample cannot overwrite
that state. Campaign `COMPLETE` means execution completed, not every cell is stable.
For automated release checks use `--require-valid`: exit 2 means at least one
cell remains inconclusive; exit 1 means an invalid execution/campaign.

For allocation-sensitive `closure-create` and `array-literal`, run **separate
N/2N/4N campaigns**, inspect raw sample distributions and GC/JIT diagnostic runs,
and look for a stable per-iteration region. Do not select the minimum, discard
slow GC samples, force a collection just for one candidate, or increase N until
a preferred result appears. Increasing N can also increase GC work or memory.
The runner refuses to call a visibly noisy series a speedup; it does not claim
that GC is the cause without counter evidence.

`--compare results/OLD.json` compares the same gross metric it reports. It checks
protocol, suite hashes, environment, N and warmup first. Std changes make the
comparison incompatible by default; diagnose a deliberate combined compiler+std
change separately. Raw artifact provenance is still retained for investigation.
A change must exceed 10% and twice the larger full-range spread. This is a
**conservative heuristic, not a confidence interval or statistical significance**.
`INCOMPATIBLE`, `INCONCLUSIVE` and `WITHIN_NOISE` must not be presented as a win.

## Immutable results and release workflow

Results use schema 2 and include source hashes, image ID, architecture, Docker host
identity/kernel, binary hash/version, declared source revision/build profile, installation hashes when mounted, explicit
stdlib hashes, Java version, configuration, sample order and raw samples.

- Labels are safe filename stems. Existing result names are never overwritten.
- Reserve unique candidate names, e.g. `v0.5.5-rc1-commit-host-run01`.
- Keep partial/invalid results with their status; do not edit them into successes.
- Final releases retain their evidence permanently. A correction is a new file
  linked to the old file in a report, not replacement of its contents.
- Legacy files, including `v0.5.4.json` and `v0.5.5.json`, are unchanged. They lack
  source hashes/raw samples and use different programs/methodology. V2 marks them
  `INCOMPATIBLE` rather than inventing provenance. Rebuild both versions against
  the same frozen v2 suite and matching std policy for a meaningful new comparison.
- Freeze input/toolchain directories during a campaign. Suite changes during
  execution invalidate the run. Do not point a benchmark at an active gate output.

The whole micro directory is versioned in the repository (`bench/micro`, #894):
results without exact programs, oracles and runner do not establish reproducibility.
Result files are not committed (`bench/micro/results/` is ignored); the archive lives
in `okkama-docs/benchmarks/micro/results`, which `--results` names. Do not commit
generated JAR/SPB artifacts or machine caches.

`check.py` runs the programs as correctness tests without timing or Docker: N=17
and N=31 against the oracles on Sparrow. The Wave CI
runs it on both engines; locally use `--cage`.

## Verification and coverage

```sh
python3 -m unittest test_runner          # use the limited Docker environment
python3 coverage.py --stdlib /path/to/std
python3 run.py --image IMAGE --okkama /path/in/image/okkama --plan
```

`test_runner.py` tests protocol/oracles, partial/noisy data, comparison compatibility,
late-failure retention, bounded output, timeouts and failed subprocesses.
See `REVIEW-2026-11-13.md` for findings, the full workload inventory and actual
validation results. This suite does not replace conformance or release gates.

## Public action inventory and scaffolds (#244)

`manifest.json` keeps the existing module `targets` and adds optional `actions`:

```json
{
  "hex-decode": {
    "path": "std/hex-decode.ok",
    "targets": ["encoding/hex"],
    "actions": ["encoding/hex:HexDecoded.new", "encoding/hex:HexDecoded.bytes"]
  }
}
```

This example shows only the target fields; retain the protocol metadata when
adding a real entry. Every existing std program declares its measured calls.
Only calls inside the timed iteration count, including construction there;
fixture construction before timing and transitive implementation calls do not.
A supporting module called explicitly in the timed loop is also a target.
Builtin string targets use `builtin:string.replace` and `builtin:string.split`
and remain outside the std inventory.

```sh
python3 bench/micro/actions.py --okkama okkama --stdlib std
python3 bench/micro/actions.py --okkama okkama --stdlib std --check
```

`STD-ACTIONS.md` lists public typed signatures, measuring benchmark names, and
measured/total counts per module and overall. The generator discovers every
non-runtime `.ok` file in the selected std directory and calls
`okkama ast FILE --format json --mode production --prune-tests` for each.
It uses this checkout's std by default, never the installed library. Missing
compiler, parse failures and unknown/hidden manifest actions fail explicitly.
`--check` compares the complete deterministic report without writing it and
returns 1 for missing/stale output. Unmeasured actions are a work list and do
not make `--check` fail once the report is current. `--output` and `--manifest`
allow checking alternate reports/inventories.

Identifiers are `module:Owner.action`. Explicit object constructors use
`Owner.new`, payload enum constructors use `Enum.case`; role signatures are
separate targets from concrete implementations. Overloads share one identifier,
with every distinct signature shown; one measured identifier does **not** claim
coverage of every overload. Hidden objects/actions, runtime modules, cap fields
and non-callable enum constants are excluded. Generics and type bounds are
preserved. Module-level `STD-COVERAGE.md` remains the output of `coverage.py`.

```sh
python3 bench/micro/actions.py --scaffold 'encoding/hex:HexDecoded.bytes'
```

The scaffold validates the public target and writes a new `action-*.ok` under
`bench/micro/std/` plus a sibling `.oracle.py` stub. Existing files are never
overwritten. The source preserves protocol v2: one Bench, tail-recursive batch,
observable warmup, `N WARMUP`, and the shared Measurement harness. TODOs mark
input generation, the measured call and checksum reduction; the unresolved
`TODO_INPUT_AND_MEASURED_CALL` deliberately prevents accidental execution.
The oracle stub raises `NotImplementedError`.

**Real inputs and independent oracles stay hand-written.** Choose bounded,
representative inputs varying with the iteration index, call the listed action,
and derive expected checksums independently of VM output. Move the completed
oracle logic into `oracles.py`, remove the sibling stub, then register the
program and its `targets`/`actions` in `manifest.json`. The generator deliberately
does not register unfinished scaffolds or count them as measured. Validate the
new program with `check.py --only NAME` before collecting timings, then
regenerate both coverage reports.

Generator and runner unit tests need no installed Okkama or Docker:

```sh
python3 -m unittest discover -s bench/micro -p 'test_*.py'
```

`test_actions.py` uses recorded production AST JSON (see `fixtures/README.md`)
to check parsing, visibility, constructors, overloads, typed signatures,
determinism, stale checks, manifest errors and scaffold/oracle shape.
