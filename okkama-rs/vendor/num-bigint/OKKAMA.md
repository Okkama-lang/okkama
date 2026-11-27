# num-bigint 0.4.8, patched for Okkama (#1148)

The crates.io release 0.4.8, with two changes on targets without an add-carry intrinsic (aarch64, wasm32,
everything but x86):

- **The fallback `adc` (`src/biguint/addition.rs`) and `sbb` (`src/biguint/subtraction.rs`) combine the two
  carries with `|` instead of `||`.** The short circuit became a branch per digit: on Apple M2 Max a
  1630-digit addition took 2.3x as long.
- **`__add2`'s low-digit loop takes four digits per step.** This saves another ~15%.

**Effect:** 04-bigint steady time drops from 205 to 118 ms, with identical output. An ADCS inline-assembly
version reached 104 ms, but it was not taken: upstream prefers Rust-native code (rust-num/num-bigint#314).

**Tests:** sparrow-core's `bigint_addition_matches_a_reference_1148` adds every pair of lengths up to 41 digits,
including all-ones carry chains, and subtracts back.

**Removal:** drop this copy and the `[patch.crates-io]` entry in `okkama-rs/Cargo.toml` once upstream num-bigint
ships the same change (rust-num/num-bigint#365, opened 2026-11-28; its four-digit add step is ours only).
