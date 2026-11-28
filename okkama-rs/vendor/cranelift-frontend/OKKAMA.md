# cranelift-frontend 0.137.0, patched for Okkama (#1151)

The crates.io release 0.137.0, with one switch: `FunctionBuilder::set_stack_map_reloads(bool)`.
With reloads off, the safepoint spiller still stores each needs-stack-map value to its stack slot at its definition
and lists the slot in the stack map of every safepoint the value is live across, but uses after a safepoint keep the
original SSA value instead of loading it back (`SafepointSpiller::rewrite_use`; `try_call` edges are not split for
reloads). That is exact for Sparrow's collector, which never moves objects: slot and value stay equal.

**Removal:** drop this copy and the `[patch.crates-io]` entry in `okkama-rs/Cargo.toml` if Cranelift gains an
equivalent option for non-moving collectors, or when the JIT stops using frontend stack maps.
