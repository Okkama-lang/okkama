//! Process workers in a build without the host (#1124): no worker command can exist there (wasm32 has no
//! processes), so Sparrow behaves as a runtime whose embedder configured none: `worker.bind` makes its action,
//! a pool hands the plain action back, and `spawn` is refused with the text of a missing worker command.
use crate::drivers::Step;
use crate::runtime::{Ctx, Runtime};
use crate::word::{Repr, UNBOUND, Word};

/// `worker.bind`: the bound action, a copy of `action`; its receiver fields only matter to a spawn.
pub fn bind(runtime: &mut Runtime, action: Word, _entry: Word, _fields: Vec<(Word, Repr)>) -> Word {
    runtime.closure_copy(action)
}

/// A worker pool without a worker command reuses nothing: the receiver's own action.
pub(crate) fn pool_bind(runtime: &mut Runtime, receiver: Word) -> Result<Word, String> {
    Ok(runtime.heap.field(receiver, 0))
}

/// No pool was ever made: closing one has nothing to wait for.
pub(crate) fn pool_close(_runtime: &mut Runtime, _frame: Word, _receiver: Word) -> Step {
    Step::Done(UNBOUND)
}

/// `worker.spawn`: refused, as with the host and no worker command.
pub fn spawn(_runtime: &mut Runtime, _ctx: *mut Ctx, _entry: Word, _arguments: &[Word], _direct: bool) -> Result<Word, String> {
    Err(crate::texts::NO_WORKER_COMMAND.into())
}

/// The driver of a worker Future, which no spawn makes here.
pub fn drive(runtime: &mut Runtime, _frame: Word, _deliver: bool) -> Step {
    Step::Failed(runtime.failure_new(crate::runtime::FailureKind::Program(crate::texts::NO_WORKER_COMMAND.into())))
}
