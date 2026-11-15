//! The W1 cases of the async plan (#601, plan §4 V10): plain await, compiled through OIR-2 and run in Champ in
//! process, each checked in full against its case.json. With `SPARROW_CORE_GC_STRESS=1` every safepoint collects.
#![cfg(feature = "champ-abi")]

mod champ_support;

use champ_support::{case_dir, compile, run};
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
}
