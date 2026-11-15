# Runner self-tests

* `test_runner.py` — builds throwaway mini-suites and drives the real `ocs`
  command against `fake_adapter.py`, whose behaviours model wrong output,
  whitespace/line-ending changes, unexpected stderr, wrong exit status, invalid
  programs that run, wrong stage, wrong category, wrong uncaught type, timeout,
  crash, internal error, malformed or missing responses, verdict-like keys,
  protocol mismatches, lying exit codes, unsupported modes, missing fixtures,
  unavailable environment, omitted execution, missing async completion, artifact
  corruption, partial-order violations, plus discovery and gate rules. Every
  failure mode has a positive control.
* `test_schema_sync.py` — the JSON Schemas agree with the executable validator.
* `test_rust_adapter.py` — the Rust adapter's engine selection (`sparrow`,
  `champ`, removed engines), per-engine target directories and probe caches,
  the Champ config file, and the translation of `okkama run` on Champ into
  protocol responses (unit level and against a fake `okkama`). Builds nothing.
* `test_champ_blockers.py` — `tools/champ_blockers.py` on synthetic
  `results.jsonl` fixtures: stages, key normalisation, ranking, JSON, compare.
* `mutations.json` — source/expectation mutants of real cases for `ocs mutate`;
  all must be reported as non-PASS.
