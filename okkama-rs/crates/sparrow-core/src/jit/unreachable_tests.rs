//! #1072: `unreachable` verifies, so Sparrow compiles it: a function holding
//! it runs, and control that does reach it fails the program as a
//! `trap unreachable` does.
use super::*;

/// The function of the ticket, branching on `r1` to `unreachable` (2) or to the return (3).
fn program(condition: bool, handled: bool) -> String {
    format!(
        r#"sparrow 18
module "u" "v1.0.0"
type t0 int
type t1 bool
type t2 failure
function f0 "main" {{
    registers [t0, t1, t2]
    params []
    result t0
{}    0: const r1, bool {condition}
    1: br r1, @2, @3
    2: unreachable
    3: const r0, int 0
    4: ret r0
    5: const r0, int 7
    6: ret r0
}}
entry f0
"#,
        if handled { "    handler 2 3 @5 r2\n" } else { "" }
    )
}

fn run(condition: bool, handled: bool) -> Result<Word, String> {
    let program = okkama_sparrow::assemble(&program(condition, handled)).expect("verifies");
    let mut runtime = Box::new(Runtime::new(crate::program::Loaded::new(program)).unwrap());
    runtime.run_function(0, 2 << 20).map_err(|error| error.0)
}

#[test]
fn a_function_with_unreachable_compiles_and_runs() {
    assert_eq!(run(false, false), Ok(word::small(0).unwrap()));
}

#[test]
fn reaching_unreachable_fails_the_program() {
    let message = run(true, false).expect_err("reached");
    assert!(message.contains(UNREACHABLE), "{message}");
    // A program failure, as `trap` makes: a handler catches it.
    assert_eq!(run(true, true), Ok(word::small(7).unwrap()));
}
