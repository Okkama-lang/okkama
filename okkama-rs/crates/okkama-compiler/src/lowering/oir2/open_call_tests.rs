//! #569 O1 end-to-end evidence: one source-level open invocation reaches High and
//! own under the registered conservative open contract. No runtime acceptance.
use crate::oir2 as ir;
use super::super::native_summaries::P1NativeSummaries;

const SOURCE: &str = concat!(
    "module app; object R { R(); destroy { } } ",
    "object Apply { Apply(); call(f is (R) is R) is R { f(R()); } } ",
    "main() is int { 0; }",
);

/// High -> own -> verify_own with the P1 catalogue; returns the own module.
fn own_module(source: &str) -> ir::Module {
    super::with_analysis(source, |analysis| {
        let high = super::super::lower_to_oir2_with_open_calls(analysis).unwrap_or_else(|error| panic!("{error}"));
        super::verify_high(&high, "open invocation");
        let text = ir::text::print(&high).unwrap();
        let parsed = ir::text::parse(&text).unwrap();
        assert!(ir::text::normalized_eq(&high, &parsed).unwrap());
        assert!(text.contains("oir.sync_closure.cleanup"), "{text}");
        let catalog = P1NativeSummaries::for_profile(&high, ir::Profile::Own).unwrap();
        let options = ir::own::OwnOptions::default();
        let inferred = ir::own::elaborate_ownership(high, &catalog, &options).unwrap_or_else(|error| {
            panic!("{error:?}")
        });
        let own = ir::own::rewrite_subset(inferred, &options).unwrap_or_else(|error| panic!("{error:?}"));
        let catalog = P1NativeSummaries::for_module(&own.module).unwrap();
        let context = ir::VerifyContext { external_contracts: &catalog, ..ir::VerifyContext::default() };
        ir::verify_own_with(&own.module, &options.verify, context).unwrap_or_else(|report| panic!("{report:#?}"));
        assert_eq!(own.census.module_count(ir::DynamicReason::Closure), own_open_calls(&own.module));
        own.module
    })
}

/// Every open call carries the canonical contract and borrows its callable.
fn own_open_calls(module: &ir::Module) -> u64 {
    let protocol = ir::catalogue::open_callable_protocol_id(module).unwrap();
    let mut open_calls = 0;
    for (_, function) in module.functions.iter() {
        let Some(body) = &function.body else { continue };
        for (_, block) in body.blocks.iter() {
            let ir::TermKind::InvokeOwn { op, .. } = &block.term.kind else { continue };
            let ir::Operation::Call(ir::CallOp::Closure { callee, call }) = op.as_ref() else { continue };
            let callable = body.values.get(callee.value).unwrap().ty;
            let expected = ir::catalogue::open_callable_summary(module, callable, protocol).unwrap();
            assert_eq!(module.summaries.get(call.summary.unwrap()).unwrap(), &expected);
            assert!(expected.results[0].opaque);
            assert_eq!(callee.convention, ir::OperandConvention::Borrow);
            open_calls += 1;
        }
    }
    open_calls
}

#[test]
fn oir2_o1_open_invocation_reaches_high_and_own() {
    let module = own_module(SOURCE);
    assert_eq!(own_open_calls(&module), 1);
    // Without the registration the independent verifier rejects the same output.
    let report = ir::verify_own(&module).unwrap_err();
    assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.rule == ir::RuleId::B55), "{report:?}");
}

#[test]
fn oir2_o1_same_open_callable_is_called_twice() {
    let source = concat!(
        "module app; object R { R(); destroy { } } ",
        "object Apply { Apply(); twice(f is (R) is R) is R { let a = f(R()); f(a); } } ",
        "main() is int { 0; }",
    );
    let module = own_module(source);
    assert_eq!(own_open_calls(&module), 2);
}

#[test]
fn oir2_o1_same_open_callable_is_called_three_times() {
    let source = concat!(
        "module app; object R { R(); destroy { } } ",
        "object Apply { Apply(); thrice(f is (R) is R) is R { let a = f(R()); let b = f(a); f(b); } } ",
        "main() is int { 0; }",
    );
    let module = own_module(source);
    assert_eq!(own_open_calls(&module), 3);
}

#[test]
fn oir2_o1_open_callable_is_called_in_a_loop() {
    let source = concat!(
        "module app; object R { R(); destroy { } } ",
        "object Apply { Apply(); each(f is (R) is R, n is int) is int { for 0..n -> i { f(R()); } 0; } } ",
        "main() is int { 0; }",
    );
    let module = own_module(source);
    assert_eq!(own_open_calls(&module), 1);
}

#[test]
fn oir2_o1_production_lowering_keeps_open_invocations_explicit() {
    let error = super::print_oir2(SOURCE).unwrap_err();
    assert!(error.to_string().contains("open action invocation"), "{error}");
}
