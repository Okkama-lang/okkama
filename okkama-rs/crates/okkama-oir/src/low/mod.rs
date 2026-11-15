//! Own → Low and Coro → Low cleanup specialization.
//!
//! The caller supplies semantically verified Own or Coro and verifies the returned Low.
//! This producer never invokes the independent verifier (including on error paths).
//! The verifier runs the complete conservation, lifetime, failure and cleanup automaton on this output.
//!
//! For Coro input (plain async, plan V4) the pass also:
//! - drops the bracket of every close, whose cleanup steps cannot suspend in 0.6.0 (D20 §2.6, `narrow`);
//! - settles the scheduled service duties of each close before its plan (OIR-2 §9.3, `services`);
//! - delivers the final cancellation of a D18 close after its plan (`cleanup`);
//! - recomputes the D21 frame layout (`layout`).
mod cleanup;
mod compact;
mod eligibility;
mod layout;
mod narrow;
mod roots;
mod graph;
mod plans;
mod services;
pub use plans::check_cleanup_plans;

use crate::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LowOptions {
    pub max_entities: usize,
    pub max_steps: usize,
    pub max_generated_blocks: usize,
    pub max_root_bindings: usize,
}
impl Default for LowOptions {
    fn default() -> Self {
        Self {
            max_entities: 1_000_000,
            max_steps: 10_000_000,
            max_generated_blocks: 100_000,
            max_root_bindings: 1_000_000,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LowError {
    InvalidInput { function: Option<FuncId>, message: String },
    Unsupported { function: FuncId, message: String },
    LimitExceeded(&'static str),
    InvalidOutput(String),
}
impl std::fmt::Display for LowError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}
impl std::error::Error for LowError {}
impl From<ArenaError> for LowError {
    fn from(error: ArenaError) -> Self {
        match error {
            ArenaError::CapacityExceeded => Self::LimitExceeded("arena index space"),
            _ => Self::InvalidInput { function: None, message: error.to_string() },
        }
    }
}
pub(super) struct Budget<'a> {
    options: &'a LowOptions,
    steps: usize,
    blocks: usize,
    roots: usize,
}
impl Budget<'_> {
    fn step(&mut self, count: usize) -> Result<(), LowError> {
        self.steps = self.steps.checked_add(count).ok_or(LowError::LimitExceeded("steps"))?;
        if self.steps > self.options.max_steps {
            return Err(LowError::LimitExceeded("steps"));
        }
        Ok(())
    }
    fn block(&mut self) -> Result<(), LowError> {
        self.blocks = self.blocks.checked_add(1).ok_or(LowError::LimitExceeded("generated blocks"))?;
        if self.blocks > self.options.max_generated_blocks {
            return Err(LowError::LimitExceeded("generated blocks"));
        }
        self.step(1)
    }
    fn roots(&mut self, count: usize) -> Result<(), LowError> {
        self.roots = self.roots.checked_add(count).ok_or(LowError::LimitExceeded("root bindings"))?;
        if self.roots > self.options.max_root_bindings {
            return Err(LowError::LimitExceeded("root bindings"));
        }
        self.step(count)
    }
}

/// Expand every close, preserving lexical boundaries and the already elaborated D8 CFG.
/// Direct plans require complete finite inventory, serial evidence and the B2 reserve condition.
pub fn lower_cleanup(mut module: Module, options: &LowOptions) -> Result<Module, LowError> {
    if !matches!(module.profile, Profile::Own | Profile::Coro) {
        return Err(LowError::InvalidInput { function: None, message: "expected Own or Coro profile".into() });
    }
    let coro = module.profile == Profile::Coro;
    let mut budget = Budget { options, steps: 0, blocks: 0, roots: 0 };
    eligibility::check(&module, &mut budget)?;
    let fresh = plans::fresh_summaries(&module, &mut budget)?;
    let functions: Vec<_> = module.functions.iter().map(|(id, _)| id).collect();
    for function in functions {
        let Some(mut body) = module.functions.get_mut(function)?.body.take() else {
            continue;
        };
        let mut removed = std::collections::BTreeSet::new();
        let mut duties = std::collections::BTreeMap::new();
        if coro {
            removed = narrow::closes(&mut body, &mut budget)?;
            compact::compact(&mut body)?;
            duties = services::assign(&module, function, &body, &mut budget)?;
            // Low keeps the schedules close-local (B10.6).
            body.service_schedules.clear();
        }
        cleanup::expand(&mut module, &mut body, &fresh, &duties, &mut budget)?;
        if coro {
            layout::relayout(&mut body, &removed, &mut budget)?;
        }
        roots::collect(&module, &mut body, &mut budget)?;
        module.functions.get_mut(function)?.body = Some(body);
    }
    module.profile = Profile::Low;
    plans::check(&module, &mut budget)?;
    Ok(module)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod executor_tests;

#[cfg(test)]
mod direct_tests;

#[cfg(test)]
mod merge_tests;

#[cfg(test)]
mod async_tests;
