//! The Champ test runner: checks fail their case, never the engine, and a suite verifies its program once.
use super::*;
use crate::runtime::{FAILED, OK};
use std::sync::{Arc, Mutex};

fn body(symbol: &str, registers: Vec<s::Type>, entry: Option<s::EntryContract>) -> s::Function {
    s::Function {
        symbol: symbol.into(),
        entry_abi: s::EntryAbi::Observation,
        registers,
        services: vec![],
        params: vec![],
        results: vec![],
        blocks: vec![s::Block { pc: 0, params: vec![] }],
        proofs: s::FunctionProofs {
            entry,
            roots: vec![s::RootRequirement { pc: 0, locations: vec![] }],
            ..s::FunctionProofs::default()
        },
        code: vec![s::Instruction::Ret(vec![])],
    }
}

/// `main` and two empty tests, `first` and `second`, in a test profile.
fn suite() -> s::Program {
    let test = || body("test", vec![], Some(s::EntryContract { failures: 2, ..s::EntryContract::default() }));
    s::Program {
        module: "app".into(),
        functions: vec![body("main", vec![], None), test(), test()],
        entry: Some(0),
        tests: Some(vec![
            s::Test { name: "first".into(), function: 1 },
            s::Test { name: "second".into(), function: 2 },
        ]),
        ..s::Program::default()
    }
}

#[derive(Clone, Default)]
struct Report(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Report {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_suite_runs_every_case_over_one_verified_program() {
    let report = Report::default();
    let passed = run_suite(suite(), Box::new(report.clone()), crate::budget::Limits::default()).unwrap();
    assert!(passed);
    let text = String::from_utf8(report.0.lock().unwrap().clone()).unwrap();
    assert_eq!(text, "=== RUN   first\n--- PASS: first\n=== RUN   second\n--- PASS: second\nPASS\n");
}

#[test]
fn runtimes_of_one_verified_program_share_its_source() {
    let verified = machine::Verified::new(suite(), Vec::new()).unwrap();
    let first = Runtime::new_champ_verified(&verified).unwrap();
    let second = Runtime::new_champ_verified(&verified).unwrap();
    let source = |runtime: &Runtime| runtime.champ.machine.as_ref().unwrap().source.clone();
    assert!(Arc::ptr_eq(&source(&first), &source(&second)));
    assert_eq!(verified.source(), &*source(&first));
}

#[test]
fn a_suite_of_a_program_the_loader_refuses_reports_the_refusal_before_any_case() {
    let mut program = suite();
    program.tests.as_mut().unwrap()[1].function = 0;
    let report = Report::default();
    let error = run_suite(program, Box::new(report.clone()), crate::budget::Limits::default()).unwrap_err();
    assert!(error.0.contains("TEST_ENTRY"), "{}", error.0);
    assert!(report.0.lock().unwrap().is_empty(), "no case runs on an unverified program");
}

/// A comparison `value_equal` refuses (here two native handles, which the loader rejects as test operands)
/// fails the running case as a program failure; it never stops the engine.
#[test]
fn an_incomparable_check_fails_its_case_instead_of_stopping() {
    let mut program = suite();
    program.objects.push(s::Object {
        name: "runtime.buffer.BufferState".into(), error_cases: vec![], enum_cases: 0, variant_cases: vec![],
        handle: true, role: None, implements: vec![], any_box: false, fields: vec![],
        layout: s::ObjectRuntimeLayout {
            allocation_words: 2, identity_offset: None, has_destroy: false, may_reach_resource: false,
            may_async_destroy: false, storage_fields: vec![], trace_layout: vec![], selection_layout: vec![],
        },
    });
    let mut runtime = Runtime::new_champ(program, Vec::new()).unwrap();
    runtime.host.as_mut().unwrap().test_failures = Some(Vec::new());
    // Stand in for a program that bypassed the loader: register 0 of function 1 holds a handle.
    let machine = runtime.champ.machine.as_mut().unwrap();
    let mut source = (*machine.source).clone();
    source.functions[1].registers = vec![s::Type::Handle(0)];
    machine.source = Arc::new(source);
    let mut ctx = Ctx { status: OK, stack_limit: 0, failure: 0, runtime: &mut *runtime, entry_frame: 0, scratch: 0 };
    rt_machine_test(&mut ctx, 1, 1, 0, 1, 2);
    assert_eq!(ctx.status, FAILED);
    let failure = runtime.failure(ctx.failure).unwrap();
    assert!(matches!(&failure.kind, FailureKind::Program(text) if text.contains("native handles")),
        "{:?}", failure.kind);
}
