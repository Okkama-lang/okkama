//! Conformance cases that own's elaboration used to stop before Champ (#706), run end to end in
//! Champ in process and checked against their case.json.
#![cfg(feature = "champ-abi")]

mod champ_support;

use champ_support::check;

/// #706: witness entries, generic overloads and lambda own entries declare the program failure that own's
/// elaboration adds (scope.split), so the loader accepts their failure masks.
#[test]
fn champ_cases_own_elaboration_failure_masks() {
    check(&[
        "L09.collections.heap-and-deque",
        "L09.collections.persistent-list",
        "L09.math.aggregates",
        "L12.caps-in-collections-and-closures",
    ]);
}

