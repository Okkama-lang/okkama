//! #569 O1 and C3b: source-level open invocations reach High and own under the
//! registered conservative open contract, through the production producer. An
//! invocation is open exactly where own treats it as open; known invocations get
//! no open ABI. No runtime acceptance (the Low and SPB 17 lanes come later).
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
        let high = super::super::lower_to_oir2(analysis).unwrap_or_else(|error| panic!("{error}"));
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
            // A managed result is opaque; a plain scalar result owns nothing.
            let managed = expected.contract.results.iter().any(|ty| !ir::catalogue::plain_scalar(module, *ty));
            assert_eq!(expected.results[0].opaque, managed);
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

/// Open ABI entities of a High module: protocols, closure cleanups, `type.of` of `fn` types.
fn open_abi(module: &ir::Module) -> (usize, usize, usize) {
    let cleanups = module.functions.iter().filter(|(_, function)| function.attrs.iter().any(|attr| {
        matches!(attr, ir::FunctionAttr::Intrinsic { name, .. } if name == ir::catalogue::SYNC_CLOSURE_CLEANUP)
    })).count();
    let descriptors = module.functions.iter().filter_map(|(_, function)| function.body.as_ref())
        .flat_map(|body| body.insts.iter())
        .filter(|(_, inst)| matches!(&inst.op, ir::Operation::Core(ir::CoreOp::TypeOf { ty, .. })
            if matches!(module.types.get(*ty), Ok(ir::Type::Fn(_)))))
        .count();
    (module.protocols.len(), cleanups, descriptors)
}

fn high(source: &str) -> ir::Module {
    super::with_analysis(source, |analysis| {
        let module = super::super::lower_to_oir2(analysis).unwrap_or_else(|error| panic!("{error}"));
        super::verify_high(&module, source);
        module
    })
}

#[test]
fn oir2_c3b_production_lowering_attaches_the_open_abi() {
    let text = super::print_oir2(SOURCE).unwrap();
    assert!(text.contains("intrinsic \"oir.sync_closure.cleanup\""), "{text}");
    assert!(text.contains("type.of ty fn(app.R) -> app.R"), "{text}");
    assert_eq!(open_abi(&high(SOURCE)), (1, 1, 1));
}

#[test]
fn oir2_c3b_two_open_fn_types_get_one_cleanup_each() {
    let source = concat!(
        "module app; object R { R(); destroy { } } ",
        "object Apply { Apply(); call(f is (R) is R) is R { f(R()); } ",
        "count(g is (int) is int) is int { g(1) + g(2); } } ",
        "main() is int { 0; }",
    );
    assert_eq!(open_abi(&high(source)), (1, 2, 2));
    let module = own_module(source);
    assert_eq!(own_open_calls(&module), 3);
    let catalogue = ir::catalogue::RuntimeCatalogue::for_module(&module);
    assert_eq!(catalogue.cleanup_keys(ir::catalogue::SYNC_CLOSURE_CLEANUP).count(), 2);
}

#[test]
fn oir2_c3b_known_invocations_stay_precise() {
    for source in [
        "module app; main() is int { let f = () is int { 7; }; f(); }",
        concat!("module app; main() is int { let c = true; ",
            "let g = if c { () is int { 1; }; } else { () is int { 2; }; }; g(); }"),
        concat!("module app; object A { A(); add(n is int) is int { n + 1; } } ",
            "main() is int { let run is (int) is int = A().add; run(6); }"),
    ] {
        assert_eq!(open_abi(&high(source)), (0, 0, 0), "{source}");
    }
}

#[test]
fn oir2_c3b_open_where_own_sees_no_finite_origin() {
    for (source, open) in [
        // A parameter: the caller's closures are not visible to own in the callee.
        (concat!("module app; object Apply { Apply(); call(f is () is int) is int { f(); } } ",
            "main() is int { Apply().call(() is int { 7; }); }"), 1),
        // A produced result and a field read.
        (concat!("module app; object Make { Make(); adder(n is int) is (int) is int { ",
            "(v is int) is int { v + n; }; } } main() is int { let add = Make().adder(1); add(2); }"), 1),
        (concat!("module app; object Hold { has f is () is int; Hold(:f); call() is int { f(); } } ",
            "main() is int { Hold(f: () is int { 7; }).call(); }"), 1),
    ] {
        assert_eq!(open_abi(&high(source)), (1, 1, open), "{source}");
        own_module(source);
    }
}
