//! Failure texts of the checks Sparrow's executors make themselves rather than a shared helper (#1120): one text,
//! one place, for the JIT's emitter and the interpreter alike.

/// An `arr.get` (or `arr.lookup`'s caller) outside the array.
pub(crate) const ARRAY_INDEX: &str = "Array index out of bounds";

/// `variant.get` of a case that is not the active one.
pub(crate) const INACTIVE_CASE: &str = "Variant payload of an inactive case";

/// `worker.spawn` in a runtime with the host and no worker command (without the host workers are emulated, #1127).
#[cfg(feature = "host")]
pub(crate) const NO_WORKER_COMMAND: &str = "process workers require an explicit worker command";

/// The program error of a reached `unreachable` (#1072).
pub(crate) const UNREACHABLE: &str = "Unreachable code reached";

/// #1076: a field read while it still holds the stub of `undef`; `object` is the declared object's name.
#[cold]
pub(crate) fn unbound_field(field: &str, object: &str) -> String {
    format!("Field `{field}` of `{}` is read before it is bound", okkama_runtime_core::testing::display_name(object))
}
