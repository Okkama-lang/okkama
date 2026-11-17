#![cfg(feature = "champ-abi")]
//! D26 duty slots (#856, S2 step 5; #857, S3): the coro bodies with `duty.empty` and slot operands, hand-written
//! and produced by coro, from Coro to Champ in process. The Full verifier checks coro and low, codegen emits
//! `duty.empty` and the slot operands, the load gate checks them, and Champ runs the program. A body whose slot is
//! filled in one branch runs both ways, so a close settles a full slot and, on the other run, an empty one (a
//! settle of nothing at run time).

use okkama_compiler::lowering::oir2;
use okkama_compiler::oir2::{VerifyMode, text};
use std::path::Path;

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../okkama-oir/tests/fixtures/async/coro").join(format!("{name}.oir"));
    std::fs::read_to_string(path).unwrap()
}

/// Compile the coro text and run it; the program's exit status.
fn run(name: &str, source: &str) -> i64 {
    let module = text::parse(source).unwrap_or_else(|error| panic!("{name}: {error}"));
    let bytes = oir2::compile_coro_with(module, VerifyMode::Full).unwrap_or_else(|error| panic!("{name}: {error}"));
    let limits = sparrow_core::budget::Limits { steps: Some(10_000_000), ..sparrow_core::budget::Limits::default() };
    let run = sparrow_core::run_captured(bytes, limits).unwrap_or_else(|error| panic!("{name}: load: {error}"));
    run.result.unwrap_or_else(|error| panic!("{name}: {error}"))
}

#[test]
fn duty_slot_bodies_run_in_champ_with_full_and_empty_slots() {
    // The exit status of each body: main's result, or 0 for a void main. d26-slot-across-await publishes the
    // value it awaited, `@work(7)` = 7 + 7.
    let cases = [("d26-branch-temporary", 0), ("d26-branch-spawn", 0), ("d26-shared-handler", 0),
        ("d26-outer-slot-across-inner-close", 0), ("d26-two-futures-order", 0), ("d26-slot-across-await", 14),
        // Coro's own output from the S3 variants (#857): the slots coro emits.
        ("d857-branch-temporary", 0), ("d857-shared-handler", 0), ("d857-branch-spawn", 0),
        ("d857-slot-across-await", 14), ("d857-rehome-slotted", 4), ("d857-stored-dynamic-slotted", 7)];
    for (name, status) in cases {
        let source = fixture(name);
        assert_eq!(run(name, &source), status, "{name}");
        // The branch that leaves the slot empty: its close settles an empty slot.
        let branch = "bool = const true";
        if source.contains(branch) {
            let empty = source.replace(branch, "bool = const false");
            assert_eq!(run(name, &empty), status, "{name} with the empty branch");
        }
    }
}
