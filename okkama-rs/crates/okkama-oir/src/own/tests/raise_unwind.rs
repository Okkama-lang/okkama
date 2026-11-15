//! A `raise` inside a try follows its block's unwind to the enclosing catch dispatch (#677).
use super::*;

const NESTED: &str = r#"oir 2 high
module nested
enum Inner is Error = (inner)
enum Outer is Error = (outer)
scope S action in @f
scope ST try in @f parent S
scope SI try in @f parent ST
scope SK catch in @f parent S
func @f() -> int {
    boundary Bauto lexical in S
    ^entry in S unwind ^failed:
    br ^outer_try
    ^outer_try in ST unwind ^outer_select:
    br ^inner_try
    ^inner_try in SI unwind ^inner_select:
    %error: Outer = enum.case Outer, case 0
    throw %error
    ^inner_select(%failure: failure) in ST unwind ^outer_select:
    catch %failure is Inner, ^inner_caught($value), ^missed(%failure)
    ^inner_caught(%caught: Inner) in ST unwind ^outer_select:
    %zero: int = const 0
    br ^done(%zero)
    ^missed(%forwarded: failure) in ST unwind ^outer_select:
    raise %forwarded
    ^outer_select(%outer_failure: failure) in S unwind ^failed:
    catch %outer_failure is Outer, ^outer_caught($value), ^unmatched(%outer_failure)
    ^outer_caught(%handled: Outer) in SK unwind ^failed:
    %one: int = const 1
    br ^done(%one)
    ^done(%n: int) in S unwind ^failed:
    want %n
    [iprep0] exit.prepare return %n at Bauto
    result.exit edge normal(0), value %n, demand want, prepare iprep0
    ret %n
    ^unmatched(%unhandled: failure) in S:
    raise %unhandled
    ^failed(%function_failure: failure) in S:
    raise %function_failure
}
"#;

fn catches(module: &Module) -> Vec<TypeId> {
    let function = module.functions.iter().find(|(_, function)| function.symbol == "f").unwrap().1;
    let body = function.body.as_ref().unwrap();
    body.blocks.iter().filter_map(|(_, block)| match &block.term.kind {
        TermKind::Catch { ty, .. } => Some(*ty),
        _ => None,
    }).collect()
}

#[test]
fn raise_in_a_nested_try_reaches_the_enclosing_catch() {
    let high = crate::text::parse(NESTED).unwrap();
    crate::verify_high(&high).unwrap();
    let before = catches(&high);
    assert_eq!(before.len(), 2);
    let inferred = elaborate_ownership(high, &TestNatives::default(), &OwnOptions::default()).unwrap();
    let output = rewrite_subset(inferred, &OwnOptions::default()).unwrap();
    verify_own(&output.module).unwrap();
    // Both dispatches survive: the inner miss reaches the outer catch instead of leaving the function.
    assert_eq!(catches(&output.module), before);
}

#[test]
fn raise_at_the_function_scope_still_leaves_the_function() {
    let missed = "^missed(%forwarded: failure) in ST unwind ^outer_select:";
    let source = NESTED.replace(missed, "^missed(%forwarded: failure) in S:");
    let high = crate::text::parse(&source).unwrap();
    crate::verify_high(&high).unwrap();
    let inferred = elaborate_ownership(high, &TestNatives::default(), &OwnOptions::default()).unwrap();
    let output = rewrite_subset(inferred, &OwnOptions::default()).unwrap();
    verify_own(&output.module).unwrap();
    // Without an unwind the miss leaves the function, so only the inner dispatch is reachable.
    assert_eq!(catches(&output.module).len(), 1);
}
