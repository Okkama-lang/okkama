//! Closure-dense conformance cases compile, load and run on Champ within a wall-time budget (#701). Before
//! #701 each SPB 17 function went through the allocation checker five or six times per run, and that
//! checker was slow on many slots and nested loops: `L04.roles.contextual-wrap` took about 73 s in a debug
//! build and `L05.variants.search` about 61 s, both past the conformance timeout of 30 s.
#![cfg(feature = "champ-abi")]

mod champ_support;

use champ_support::run_case;
use std::time::{Duration, Instant};

/// Compile (OIR-2, structural verification), load and run one case in process; the output must match its
/// `case.json`, and the whole of it must fit in the budget.
fn within_budget(id: &str) {
    let budget = Duration::from_secs(if cfg!(debug_assertions) { 40 } else { 8 });
    let started = Instant::now();
    let outcome = run_case(id);
    let elapsed = started.elapsed();
    if let Err(error) = outcome {
        panic!("{id}: {error}");
    }
    assert!(elapsed < budget, "{id} took {elapsed:?}, budget {budget:?}");
    println!("{id}: {elapsed:?}");
}

/// The slowest of the #701 cases before the fix.
#[test]
fn roles_contextual_wrap_runs_within_budget() {
    within_budget("L04.roles.contextual-wrap");
}

#[test]
fn variants_search_runs_within_budget() {
    within_budget("L05.variants.search");
}
