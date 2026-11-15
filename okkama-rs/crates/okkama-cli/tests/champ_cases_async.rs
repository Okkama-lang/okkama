//! The W1 cases of the async plan (#601, plan §4 V10): plain await, compiled through OIR-2 and run in Champ in
//! process, each checked in full against its case.json; and R3's async programs with resources that reach Champ.
//! With `SPARROW_CORE_GC_STRESS=1` every safepoint collects.
#![cfg(feature = "champ-abi")]

mod champ_support;

use champ_support::{case_dir, compile, run, run_source};
use okkama_compiler::SourceFile;
use serde_json::Value;

fn full_check(id: &str) -> Result<(), String> {
    let dir = case_dir(id);
    let case: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("case.json")).unwrap()).unwrap();
    assert_eq!(case["mode"], "run", "{id}: only run-mode cases");
    let mut sources = Vec::new();
    for name in case["sources"].as_array().unwrap() {
        let file = std::fs::canonicalize(dir.join(name.as_str().unwrap())).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        sources.push(SourceFile::new(file.to_string_lossy(), text).unwrap());
    }
    let run = run(compile(&sources)?)?;
    let stdout = run.stdout;
    let expect = &case["expect"];
    let expected_stdout = expect["stdout"]["text"].as_str().unwrap_or("");
    match (expect["outcome"].as_str().unwrap(), &run.result) {
        ("success", Ok(code)) => {
            let status = expect["exit_status"].as_i64().unwrap_or(0);
            if *code != status {
                return Err(format!("exit {code}, expected {status}; stdout={stdout:?}"));
            }
        }
        ("success", Err(error)) => return Err(format!("failed: {error}; stdout={stdout:?}")),
        (_, Ok(code)) => return Err(format!("completed with {code}, expected a failure; stdout={stdout:?}")),
        (_, Err(_)) => {}
    }
    if stdout != expected_stdout {
        return Err(format!("stdout {stdout:?}, expected {expected_stdout:?}"));
    }
    Ok(())
}

macro_rules! full_check {
    ($($name:ident => $id:literal,)*) => {
        $(
            #[test]
            fn $name() {
                full_check($id).unwrap_or_else(|error| panic!("{}: {error}", $id));
            }
        )*
    };
}

full_check! {
    w1_await_in_async_main => "L08.await.in-async-main",
    w1_failure_rethrown_at_every_await => "L08.failure.rethrown-at-every-await",
    w1_future_lazy_and_cached => "L08.future.lazy-and-cached",
    w1_future_never_awaited => "L08.future.never-awaited",
    w1_future_passed_and_awaited_elsewhere => "L08.future.passed-and-awaited-elsewhere",
    w1_order_across_suspension => "L08.order.across-suspension",
    w1_order_lazy_branches => "L08.order.lazy-branches",
    w1_void_await_for_effect => "L08.void.await-for-effect",
    r3_role_restriction_through_async => "L12.role-restriction-through-async",
}

/// A resource that prints its destroy, so the run shows when each one closes.
const LEASE: &str = concat!(
    "module app; import \"io/stdout\" take Stdout; ",
    "object Lease { has name is string; Lease(name is string) { self.name = name; } label() is string { name; } ",
    "destroy { Stdout().writeln(\"destroy \" + name); } } ",
);

fn lease_run(rest: &str) -> String {
    let run = run_source(&format!("{LEASE}{rest}")).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(run.result, Ok(0), "stdout={:?}", run.stdout);
    run.stdout
}

/// OIR-2 §10.2: the cache owns the resource result; both awaits see it, the body's local closes before the
/// result is published, and the Future's owner destroys the result once (corpus `async-results` checks 2–4).
#[test]
fn r3_resource_result_is_cached_and_destroyed_once() {
    let stdout = lease_run(concat!(
        "object Work { Work(); async make() is Lease { let local = Lease(\"L\"); Lease(\"R\"); } } ",
        "async main() is int { { let f = Work().make(); let r = await f; Stdout().writeln(\"first \" + r.label()); ",
        "let again = await f; Stdout().writeln(\"again \" + again.label()); } Stdout().writeln(\"end\"); 0; }",
    ));
    assert_eq!(stdout, "destroy L\nfirst R\nagain R\ndestroy R\nend\n");
}

/// OIR-2 §§10.2–10.3: an awaited temporary's fresh result belongs to the awaiting scope, a discarded one too
/// (corpus `async-results` checks 5–6); a temporary inside an expression closes with its statement.
#[test]
fn r3_awaited_temporary_result_closes_with_its_awaiting_scope() {
    let stdout = lease_run(concat!(
        "object Work { Work(); async make(n is string) is Lease { Lease(n); } } ",
        "async main() is int { { await Work().make(\"T\"); Stdout().writeln(\"in block\"); ",
        "let kept = (await Work().make(\"K\")).label(); Stdout().writeln(\"kept \" + kept); } ",
        "Stdout().writeln(\"end\"); 0; }",
    ));
    assert_eq!(stdout, "in block\ndestroy K\nkept K\ndestroy T\nend\n");
}
