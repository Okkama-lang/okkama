//! #569 C3b and C6 conformance cases from source: High -> own -> Low.
//!
//! The `any` cases reach Low under the build gate (the SPB 17 lane does not take
//! `any` or role layouts yet). The open-invocation cases keep the checked dynamic
//! transfer of each open call result (`own.dynamic` with the Closure protocol) and
//! reach Low, which settles it in the cleanup automaton (#683).
use super::own_case_tests::{case, case_directory, own_stage};
use crate::oir2 as ir;

const ANY_CASES: [&str; 11] = [
    "L03.overloads.selection",
    "L03.variadic.overload-by-tail-type",
    "L04.ctor.typed-overload",
    "L05.any.transport",
    "L05.array.replaced",
    "L05.boundary.typed-values",
    "L05.generic.inference",
    "L05.literal.empty-collections",
    "L05.literal.heterogeneous-boundaries",
    "L05.literal.heterogeneous-to-any",
    "L05.literal.homogeneous-inference",
];

const OPEN_CALL_CASES: [&str; 12] = [
    "L03.anon.capture-and-escape",
    "L03.anon.captures-self",
    "L03.anon.compose",
    "L03.anon.self-retained-only-when-read",
    "L03.anonymous.lexical-scope",
    "L04.caps.ordinary-block-body",
    "L04.invariant.escaped-self-bound-action",
    "L04.privacy.closure-reads-self-field",
    "L04.successor.bound-action-invoked",
    "L04.successor.closure-transferred",
    "L07.regions.role-instances",
    "L09.collections.ordered-sets",
];

fn own(id: &str) -> Result<ir::Module, String> {
    let directory = case_directory(id);
    let source = std::fs::read_to_string(directory.join("main.ok")).unwrap();
    own_stage(&source, Some(directory), ir::verify::VerifyMode::Structural).map(|(module, _)| module)
}

#[test]
fn oir2_c6_any_cases_reach_low_under_the_build_gate() {
    let mut failed = vec![];
    for id in ANY_CASES {
        if let Err(error) = case(id, ir::verify::VerifyMode::Structural) {
            failed.push(format!("{id}: {}", error.chars().take(300).collect::<String>()));
        }
    }
    assert!(failed.is_empty(), "{failed:#?}");
}

#[test]
fn oir2_c6_any_transport_under_the_full_verifier() {
    for id in ["L05.any.transport", "L05.literal.heterogeneous-to-any", "L05.literal.heterogeneous-boundaries"] {
        case(id, ir::verify::VerifyMode::Full).unwrap_or_else(|error| panic!("{id}: {error}"));
    }
}

#[test]
fn oir2_c3b_open_invocation_cases_reach_own() {
    let mut failed = vec![];
    for id in OPEN_CALL_CASES {
        match own(id) {
            Ok(module) => {
                // Every open call carries the registered contract under verify_own.
                let protocol = ir::catalogue::open_callable_protocol_id(&module);
                assert!(protocol.is_some(), "{id}: no open callable protocol");
            }
            Err(error) => failed.push(format!("{id}: {}", error.chars().take(300).collect::<String>())),
        }
    }
    assert!(failed.is_empty(), "{failed:#?}");
}

/// Every open-invocation case reaches low within the default step budget (cleanup inventories are linear in
/// their chains since #687; `L09.collections.ordered-sets` needed 14.5M steps before, 1.8M now).
#[test]
fn oir2_c3b_open_invocation_cases_reach_low() {
    let mut failed = vec![];
    for id in OPEN_CALL_CASES {
        if let Err(error) = case(id, ir::verify::VerifyMode::Structural) {
            failed.push(format!("{id}: {}", error.chars().take(300).collect::<String>()));
        }
    }
    assert!(failed.is_empty(), "{failed:#?}");
}

#[test]
#[ignore = "#569: OIR-2 has no char-to-string conversion (NumericType has no Char)"]
fn oir2_c6_string_tail_converts_reaches_low() {
    let id = "L03.variadic.string-tail-converts";
    case(id, ir::verify::VerifyMode::Structural).unwrap_or_else(|error| panic!("{id}: {error}"));
}
