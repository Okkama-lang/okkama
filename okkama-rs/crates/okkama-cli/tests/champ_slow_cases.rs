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
    within(id, 40, 8);
}

/// As `within_budget`, with a case's own budgets in seconds for debug and optimized builds.
fn within(id: &str, debug: u64, optimized: u64) {
    let budget = Duration::from_secs(if cfg!(debug_assertions) { debug } else { optimized });
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

/// #779: about 14 s of CPU in a debug build before (codegen and the load check 4.2 s), about 11 s after the
/// load check's dominator tree and obligations table and the pipeline's single verify_low (2.3 s).
#[test]
fn ini_native_equivalence_runs_within_budget() {
    within_budget("L09.ini.native-equivalence");
}

/// #781: past the allocation checker's storage bound before its states forgot dead aliases; #779 then brought
/// its codegen and load check from 26 s to about 7 s in a debug build.
#[test]
fn db_row_readers_runs_within_budget() {
    // About 24 s in a local debug build and 8.3 s on a CI runner in the `gate` profile, past the shared 8 s; its
    // run time is the Champ runtime's (owned calls), measured after Champ is production-ready.
    within("L09.db.row-readers", 60, 20);
}

/// #781: an observation entry returning a managed result from a retained scope (T2); the large instantiations of
/// `testing/generation` take about 18 s in a debug build after #779.
#[test]
fn once_generic_std_instantiation_runs_within_budget() {
    within_budget("L04.once.generic-std-instantiation");
}
