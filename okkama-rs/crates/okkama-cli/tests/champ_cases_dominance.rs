//! #702 on Champ, in process: a Low invocation that SPB 17 computes totally drops its unwind edge, and the
//! cleanup chain behind it must not reach the loader as unreachable code that reads other blocks
//! ("definition does not dominate use"). Each case runs in full and is compared with its case.json.
//!
//! Only pure language cases: no processes, workers, network, outside files or test mode.
#![cfg(feature = "champ-abi")]

mod champ_support;

use champ_support::{check, run_source};

#[test]
fn unwind_chains_of_total_operations_leave_no_unreachable_reads() {
    check(&[
        "L05.batching.functional",
        "L07.traversal.batches",
        "L07.traversal.cursor-lifetimes",
        "L07.traversal.early-exits",
        "L07.traversal.escaping",
        "L07.traversal.failures",
        "L09.strings.parsed-scalars",
        "L09.time.rfc3339",
    ]);
}

/// The reduced fixture: a total `variant.new` after a call inside `try` with a local live into the catch.
#[test]
fn a_thrown_payload_error_after_a_call_reaches_its_catch() {
    let run = run_source(r#"module app v1.0.0;

import "io/stdout" take Stdout;

enum Failing is Error = (failing(code is int));

main() is int {
    let out = Stdout();
    try {
        out.writeln("x");
        throw Failing.failing(2);
    } catch e is Failing {
        out.writeln("caught");
    };
    0;
}
"#).unwrap();
    assert_eq!(run.stdout, "x\ncaught\n");
    assert_eq!(run.result, Ok(0));
}
