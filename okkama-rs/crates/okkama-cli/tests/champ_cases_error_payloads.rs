//! #704 on Champ, in process: a single-case Error variant is its case's object in SPB 17, so `confirm … else
//! throw`, an index fallback and a case pattern read its tag as the only case and its payload as a field.
//! A failure raised in a closure reaches its caller's catch. Each case runs in full against its case.json.
//!
//! Only pure language cases: no processes, workers, network, outside files or test mode.
#![cfg(feature = "champ-abi")]

mod champ_support;

use champ_support::{check, run_source};

#[test]
fn single_case_error_tags_and_payloads() {
    check(&[
        "L01.gram.confirm.index-else",
        "L07.confirm.lazy-else",
        "L09.encoding.base64-strict-decoding",
        "L09.strings.parse-rejections",
        "L09.time.zones",
    ]);
}

/// Variant layouts end to end, among them an Error payload of a plain variant whose text shows the variant's case:
/// `Fault.major(1, Shape.rect(2, 3))` (#763).
#[test]
fn plain_variant_payloads_of_errors() {
    check(&["L05.variants.case-layouts"]);
}

#[test]
fn a_case_pattern_binds_the_payload_of_the_only_case() {
    let run = run_source(r#"module app v1.0.0;

enum Failing is Error = (failing(code is int));

main() is int {
    try {
        throw Failing.failing(2);
    } catch Failing.failing(code) {
        code;
    };
}
"#).unwrap();
    assert_eq!(run.result, Ok(2));
}

/// The owning entry of a closure leaves through its raise with its input packet already passed on.
#[test]
fn a_failure_raised_in_a_closure_reaches_the_callers_catch() {
    let run = run_source(r#"module app v1.0.0;

import "io/stdout" take Stdout;

enum Missing is Error = (zone(name is string));

main() is int {
    let resolved = (text is string) is string {
        throw Missing.zone(text);
    };
    let r = try { resolved("Berlin"); } catch Missing.zone(name) { "Missing " + name; };
    Stdout().writeln(r);
    0;
}
"#).unwrap();
    assert_eq!(run.stdout, "Missing Berlin\n");
    assert_eq!(run.result, Ok(0));
}
