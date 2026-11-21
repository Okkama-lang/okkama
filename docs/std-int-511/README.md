# #511: canonical std integers

Owner authorization: supplied ticket/review and 2026-11-22 instructions. The
review's planning-only and remote issue/report steps are replaced by this
implementation record. Its “v0.7.0” means the current release. All work stays
in `l-codex2`. No Git operations, CHANGELOG changes, Rust/native signature
changes, language changes, object deletions, Docker or whole suites.

## Audit and signatures

[Every original signature and its classification](inventory.md), also
[machine-readable](inventory.json), accounts for 136 signatures: 104 actions
and 32 constructors. Among actions, **83 return BigInt and 28 accept it**;
seven do both. Constructors add 32 input signatures / 41 parameter positions,
so there are **60 input signatures / 69 input type positions** in total.
The historical “82 returns / 50 inputs” estimates are corrected accordingly.

**125 signatures migrate** (93 actions, 32 constructors). **11 actions remain**,
all on `BigInt` itself: `order`, `plus`, `minus`, `times`, `divided`, `remainder`,
`negated`, `gcd`, `lcm`, and the two `power` overloads. They are the operations
of the explicitly selected numeric wrapper. Its constructors, errors, caps and
seven operator/ordering role instantiations remain. No other public signature
mentions the BigInt type. Hidden implementation uses were migrated separately;
fields are implementation details, not extra public API sites.

The complete before/after table includes roles, constructors, arrays,
`Maybe<int>` and `Presence<int>`. The main groups are:

- JSON `integer`, `JsonPresenceReader` / `JsonMaybeReader`, request JSON readers.
- DB integer writer callbacks, value capture, native values, row readers,
  execution counts, SQLite blob row/offset parameters and backup counts.
- HTTP body lengths, byte ranges and size limits; XML-RPC, forms, cache and WebSocket limits.
- Plist integer/UID constructors and writer callbacks.
- Duration nanoseconds; archive owner IDs, file inode, storage capacities.
- Logging failure/drop/generation counts, test recording indexes, mail sizes,
  IMAP generations/UID numeric components, plural operands.
- Decimal coefficient, BigRational numerator/denominator, locale numeric
  magnitudes/quantization/localized integer parsing, IP network sizes.

The `BigRational`, `ImapUid`, `ImapUidValidity`, `Duration`, `FileIdentity` and
other domain objects retain their invariants and identities. The existing
`LocalizedBigInteger` object also remains, now taking int. No std object was
removed. Four redundant BigInt constructor overloads fold into existing int
ones: Json, JsonField and the direct/Maybe DbInteger constructors.

## Behavior choices

1. Integer quantities use exact arbitrary-precision int, without implicit
   conversions, floats or a new int64 restriction.
2. `(a / b).int()` truncates toward zero. Runtime source `rat.rs::to_int` and
   differential tests against BigInt.divided confirm every sign combination.
   `%` retains the dividend's sign and `a == q*b+r` remains true. Locale and
   plist extraction explicitly truncate their quotients. BigRational reduces
   through native exact rat and retains a positive denominator and unique 0/1.
   BigInt's typed division-by-zero errors and BigRational's zero-denominator
   validation remain; the migrated scale divisors are strictly positive.
3. JSON `integer()` is arbitrary precision. Its existing `int()` signed-64
   reader remains bounded, including optional readers. Fractions remain errors,
   and serialization retains exact values and its existing canonical decimal
   spelling (which may use an exponent). Presence/Maybe distinctions remain.
   The 4096-bit JSON guard bounds the reconstructed power of ten, as before;
   it is not a new total-bit restriction on int or on a literal coefficient.
4. DB preserves its 4096-decimal-digit limit and SQL NULL. Row `int()` still
   checks signed 64 bits; row `integer()` is exact. Driver widths, unsigned
   fields, blob offset/row constraints and existing limits remain at their
   boundaries. The unchanged DB native ABI uses canonical decimal strings;
   those necessary boundary encodings remain. Redundant BigInt allocations
   and int→text→BigInt round trips are removed.
5. Binary plist retains signed 128-bit integers and unsigned 128-bit UIDs;
   XML retains its 4096-character numeric spelling guard. Byte extraction is
   exact before the final byte conversion. IMAP UIDs retain 1..4294967295.
   Existing HTTP, archive, storage and native size validations remain.
6. One shared `numbers/bigint::BoundedPower(base: int).int(exponent: int,
   maximum_bits: int = 1048576) is int` serves the migrated parsers and
   BigInt.power. Existing BigPowerLimitExceeded / InvalidBigExponent /
   InvalidBigPowerLimit errors remain. Negative exponents and nonpositive
   budgets are rejected; 0^0 = 1. Zero and ±1 have constant-size results,
   including very large exponents. Lower bounds reject impossible requests
   before allocation; proven-fitting powers take the native fast path;
   tight budgets use checked exponentiation by squaring, refusing oversized
   products before multiplication. No duplicate bounded-power algorithm.
7. All **13 existing bounded-power call sites** retain their budgets:

   | Module | Sites | Result-bit budget |
   |---|---:|---:|
   | json | 1 | 4096 |
   | plist/parse | 1 | 129 |
   | plist/write | 2 | 129 |
   | net/ip | 1 | 129 |
   | locale/number | 3 | 8192 |
   | numbers/float | 2 + 2 | 1100 / 1400 |
   | archive/tar | 1 | 1048576 (previous default) |

   Decimal's independent 4096-digit shift guard is unchanged. Plist width
   comparisons use powers with fixed small exponents after explicit 128-bit
   validation. They do not introduce an untrusted unbounded exponent.

## Verification

- [x] API inventory and classification; no lost declarations or ordinary
  public BigInt signatures.
- [x] 11 new inline tests; **13 runtime mutants killed**. Each original test
  passed before its mutation. [Exact commands, changes and failure output](mutations.json).
- [x] New coverage: all quotient/remainder signs and identity, very large
  integers, typed zero errors, all six bit budgets, tight-budget rejection,
  invalid exponent/budget, exact JSON and presence/nullability, bounded JSON
  reads, DB precision/nullability/digit limit, signed/unsigned plist limits
  and XML/binary round trips, locale rounding/digit guards, IPv6 /0,
  signed durations and fractional-nanosecond rejection, HTTP length grammar.
- [x] Targeted module runs: **180 changed/direct-importing modules** (not the
  whole std suite). 150 modules pass; 20 contain no inline tests and pass
  `check`. Across the runs, **400 tests pass**.
- [ ] **30 network tests in 10 modules are blocked by sandbox socket binding**
  (`permission_denied`, TCP/UDP). Each failure was inspected and is a denied
  bind, not a numeric assertion failure. A socket-capable environment is still
  required to verify these tests. Docker and whole-suite runs are excluded.
- [x] All 14 changed conformance source files pass `check` with the existing
  harness import directory. All 14 cases run individually and match expected output/exit status. The two
  filesystem cases use disposable working directories inside the worktree.
  No conformance suite runner was used.
- [x] Both changed/new examples pass `check` and match `.expected` exactly.
  No existing examples or t/ files contained BigInt; no t/ caller required a
  migration. Added `examples/json/integers.ok`, extended time/durations.
- [x] English/Russian Book signatures and migration guidance updated.
- [x] A/B checksums for eight representative operations match; methodology,
  raw samples and medians are in [benchmarks.json](benchmarks.json).
- [x] Public conformance inventory and code maps regenerated.

Every module command and its result is recorded in [validation.json](validation.json).
Example/conformance commands, output comparisons and checks are in
[consumers.json](consumers.json). Final new-test runs are also recorded in [new-tests.json](new-tests.json).
The commands use the installed `okkama 0.6.1` binary:

```sh
export OKKAMA_STDLIB_DIR=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/std
# For each of the 180 modules listed in validation.json:
okkama run -t std/<module>.ok
# For the 20 modules without inline tests:
okkama check std/<module>.ok
# New tests (one module at a time):
okkama run -t --filter '511' std/numbers/bigint.ok
okkama run -t --filter '511' std/numbers/bigrational.ok
okkama run -t --filter '511' std/json.ok
okkama run -t --filter '511' std/db/value.ok
okkama run -t --filter '511' std/plist/write.ok
okkama run -t --filter '511' std/plist/parse.ok
okkama run -t --filter '511' std/locale/number.ok
okkama run -t --filter '511' std/net/ip.ok
okkama run -t --filter '511' std/time/duration.ok
okkama run -t --filter '511' std/http/http1_framing.ok
okkama check examples/json/integers.ok
okkama examples/json/integers.ok
okkama check examples/time/durations.ok
okkama examples/time/durations.ok
export OKKAMA_IMPORT_DIRS=/Volumes/SamsungSSD/stuff/okkama-work/l-codex2/conformance/harness/okkama
# Each exact conformance path and invocation is in consumers.json.
okkama check conformance/cases/<level>/<case>/main.ok
okkama conformance/cases/<level>/<case>/main.ok
okkama docs/std-int-511/integer-511.ok
# Changed Okkama files were individually formatted with: okkama fmt -w <file>
(cd conformance && python3 tools/inventory_std.py ../std --write)
python3 tools/map/gen.py
```

Sparrow is the repository's only current backend. No Rust build or temporary
`target-511` directory was needed.

## Narrow A/B benchmark

`docs/std-int-511/integer-511.ok` runs eight workloads, 1000 operations per sample, one
warmup followed by five measured samples. Three alternating before/after
process pairs use the same installed Sparrow binary and local std tree.
For “before”, the original std files were temporarily restored from pre-edit
copies, along with the original BigInt.power implementation; they were restored
to the final implementation after every baseline group. No Git operations.
[Historical baseline source](benchmark-before.ok.txt) documents the corresponding
old calls. Run it against the pre-migration std; run the current benchmark
against the migrated std. The baseline is documentation, not a current caller.

Medians are ns/operation over 15 warmed samples; this is a local, narrow
comparison, not the release performance gate. Every workload consumes its
result in a checksum; before/after checksums agree. DB uses the real in-memory
native cell boundary without an external server. Compilation is outside the
timed regions. Small differences may be noise.

| Operation | Before | After | Change |
|---|---:|---:|---:|
| json-parse-access | 2403.0 | 1473.5 | -38.7% |
| json-serialize | 468.7 | 457.0 | -2.5% |
| db-encode | 815.0 | 742.6 | -8.9% |
| db-decode | 969.4 | 952.6 | -1.7% |
| duration | 2149.4 | 1943.5 | -9.6% |
| http-length | 105.7 | 36.5 | -65.5% |
| large-arithmetic | 1699.0 | 1497.5 | -11.9% |
| bounded-power | 1097.0 | 731.3 | -33.3% |

## Migration notes

| Old call | Current call |
|---|---|
| `doc.integer().int()` | `doc.integer()` |
| `Json(BigInt(n))` / `JsonField(name, BigInt(n))` | `Json(n)` / `JsonField(name, n)` |
| `DbInteger(BigInt(n))` | `DbInteger(n)` |
| `DbInteger(Maybe<BigInt>.some(BigInt(n)))` | `DbInteger(Maybe<int>.some(n))` |
| `row.integer(name).int()` | `row.integer(name)` |
| `done.affected() else BigInt(0)` | `done.affected() else 0` |
| `duration.nanoseconds().int()` | `duration.nanoseconds()` |
| `PlistInteger(BigInt(n))` / `PlistUid(BigInt(n))` | `PlistInteger(n)` / `PlistUid(n)` |
| `BigRational(numerator: BigInt(a), denominator: BigInt(b))` | `BigRational(numerator: a, denominator: b)` |
| `HttpLimits(header_bytes: h, line_bytes: l, body_bytes: BigInt(n))` | `HttpLimits(header_bytes: h, line_bytes: l, body_bytes: n)` |
| `BigInt(10).power(e, 4096).int()` | `BoundedPower(base: 10).int(e, 4096)` |

Custom implementations of affected roles must change their signatures too:
`BodyOpening.length() is int`, `DbValueWriting.integer(value is int)`, and
`PlistWriting.integer(value is int)` / `uid(value is int)`, for example.
A caller that intentionally owns a BigInt passes `.int()` at an ordinary API
boundary; callers that intentionally need the wrapper can construct
`BigInt(integer_result)`. No implicit conversion or compatibility layer is added.

## Open questions and remaining verification

No owner/API design question remains. Implementation scope and the runnable
local numeric checks are complete. Full integration verification is limited
by the 30 denied socket binds listed above. No success is claimed for these unrun/blocked
checks. CHANGELOG and remote metadata updates were explicitly excluded by
the owner's task instructions.
