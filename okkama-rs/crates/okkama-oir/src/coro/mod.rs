//! Own → Coro for async bodies without resources across suspension (async ABI D20, D21, C-1…C-3).
//!
//! The caller supplies semantically verified Own and verifies the returned Coro with `verify_coro`.
//! This producer never invokes the independent verifier.
//!
//! For every async body the pass:
//! - takes the frame as the only parameter and restores the receiver, the arguments, the descriptors
//!   and the input packet from continuation 0 (C-2);
//! - brackets every suspending operation with `frame.save k` / `frame.resume k` (D20, both forms);
//! - saves the values that are live after the operation and are not its operands; the duties of a
//!   service schedule count as used at every close site of their owner scope (D20, review B4);
//! - emits the D21 frame layout, whose per-scope area the loader derives from the lexical scopes;
//! - publishes the body outcome through `fut.publish` at every exit (C-2). A fixed root (`async main`, an
//!   async test: observation ABI, no exit transaction) publishes its value with an empty packet and the
//!   value's deps, taken before its scope closes.
//!
//! In every function, `fut.new` keeps its deferred or external environment and gains the owner's
//! `cache-state` (C-1). Own declares the Services schedules (D16); the pass renames each Future of them to
//! its token, and every close of the owner names the versions of the tokens that reach it, in own's order.
mod body;
pub(crate) mod graph;
mod layout;
mod liveness;
mod repair;

use crate::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoroOptions {
    pub max_steps: usize,
}
impl Default for CoroOptions {
    /// #806: the budget is one per module. Repair scans the body once per value saved across a suspension, so
    /// the work grows with a function's size times its suspensions: the SQLite programs' module spends about
    /// 15 000 000 steps (one std function, `SqliteOpening.connection`, 9 000 000).
    fn default() -> Self {
        Self { max_steps: 100_000_000 }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CoroError {
    InvalidInput { function: Option<FuncId>, message: String },
    /// A form that a later slice of the async plan lowers.
    Unsupported { function: FuncId, message: String },
    LimitExceeded(&'static str),
}
impl std::fmt::Display for CoroError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}
impl std::error::Error for CoroError {}
impl From<ArenaError> for CoroError {
    fn from(error: ArenaError) -> Self {
        match error {
            ArenaError::CapacityExceeded => Self::LimitExceeded("arena index space"),
            _ => Self::InvalidInput { function: None, message: error.to_string() },
        }
    }
}

pub(super) struct Budget<'a> {
    options: &'a CoroOptions,
    steps: usize,
}
impl Budget<'_> {
    fn step(&mut self, count: usize) -> Result<(), CoroError> {
        self.steps = self.steps.checked_add(count).ok_or(CoroError::LimitExceeded("steps"))?;
        if self.steps > self.options.max_steps {
            return Err(CoroError::LimitExceeded("steps"));
        }
        Ok(())
    }
}

/// Lower verified Own to Coro. Synchronous functions keep their bodies except for the cache duties of
/// the Futures they create.
pub fn lower_coro(mut module: Module, options: &CoroOptions) -> Result<Module, CoroError> {
    if module.profile != Profile::Own {
        return Err(CoroError::InvalidInput { function: None, message: "expected Own profile".into() });
    }
    let mut budget = Budget { options, steps: 0 };
    let functions: Vec<_> = module.functions.iter().map(|(id, _)| id).collect();
    for function in functions {
        body::lower(&mut module, function, &mut budget)?;
    }
    module.profile = Profile::Coro;
    Ok(module)
}

#[cfg(test)]
mod tests;
