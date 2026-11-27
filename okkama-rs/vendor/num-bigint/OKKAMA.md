# num-bigint 0.4.8, patched for Okkama (#1148)

This is the crates.io release 0.4.8, unchanged except for one function: `add_lo` in `src/biguint/addition.rs`, used by `__add2`. It is the low-digit loop of every magnitude addition.

**Why.** On aarch64, the generic loop computes each digit through a double-width (`u128`) sum, at about 4 cycles per 64-bit digit. x86_64 already has a fast path, `_addcarry_u64`. The patch adds the same kind of path for aarch64: an `ADCS` chain over four digits per step, with the remainder done by the generic `adc`.

**Effect.** On Apple M2 Max, a 1630-digit addition takes 0.44 ns per digit instead of 1.18. 04-bigint's steady time goes from 201 to 101 ms with identical output.

**Tests.** `sparrow-core`'s `bigint_addition_matches_a_reference_1148` adds pairs of up to 41 digits, including all-ones carry chains, and checks each sum against a reference.

**Remove this copy** and the `[patch.crates-io]` entry in `okkama-rs/Cargo.toml` once num-bigint ships an equivalent (upstream PR pending).
