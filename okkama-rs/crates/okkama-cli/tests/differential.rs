//! Sparrow's two engines in one process (#1121): each program is built once, and its SPB runs on the JIT and on
//! the interpreter (`sparrow_core::observe`). Both must show the same thing: `main`'s result, or the report and
//! the uncaught failure compared as a structure (category, type, message, sites, cause traces), and each output
//! stream byte for byte. The samples are t/, examples of the Book and the Bible, and conformance's cases that
//! expect a run to fail; step parity runs a few of them under five step budgets. A program the interpreter does not
//! run yet (an async body, #1122) is counted apart, never compared.
mod tree;

use sparrow_core::budget::Limits;
use sparrow_core::{Engine, Observed};
use std::path::{Path, PathBuf};

/// The interpreter's refusal of what it does not run yet.
const NOT_YET: &str = "is not supported by the interpreter yet";

/// Build `path` (with `imports` as an import directory) into a fresh SPB; `None` when it does not build.
fn build(path: &Path, imports: Option<&Path>) -> Option<Vec<u8>> {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let out = std::env::temp_dir().join(format!(
        "okkama-differential-{}-{}.spb",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let mut command = tree::okkama();
    command.arg("build").arg(path).arg("-o").arg(&out);
    if let Some(imports) = imports {
        command.env("OKKAMA_IMPORT_DIRS", imports);
    }
    let built = command.output().expect("okkama runs").status.success();
    let bytes = built.then(|| std::fs::read(&out).expect("the SPB"));
    let _ = std::fs::remove_file(&out);
    bytes
}

#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Same,
    NotYet,
}

/// Run `bytes` on both engines under `limits`: the same observation, or a refusal of the interpreter.
fn compare(name: &str, bytes: &[u8], limits: &Limits) -> Verdict {
    let observe = |engine| sparrow_core::observe(engine, bytes.to_vec(), Vec::new(), limits.clone()).expect("the SPB loads");
    let jit: Observed = observe(Engine::Jit);
    let interpreted: Observed = observe(Engine::Interpreter);
    if let Err((report, _)) = &interpreted.result
        && report.contains(NOT_YET)
    {
        return Verdict::NotYet;
    }
    assert_eq!(interpreted, jit, "{name}: the interpreter differs from the JIT");
    Verdict::Same
}

/// Build and compare every program of `paths`, a few at a time; how many were compared and refused.
fn sample(paths: &[PathBuf], imports: Option<&Path>, limits: &Limits) -> (usize, usize) {
    let (mut same, mut not_yet) = (0, 0);
    for chunk in paths.chunks(6) {
        let verdicts: Vec<Option<Verdict>> = std::thread::scope(|scope| {
            let runs: Vec<_> = chunk
                .iter()
                .map(|path| scope.spawn(move || build(path, imports).map(|bytes| compare(&path.display().to_string(), &bytes, limits))))
                .collect();
            runs.into_iter().map(|run| run.join().unwrap()).collect()
        });
        for verdict in verdicts.into_iter().flatten() {
            match verdict {
                Verdict::Same => same += 1,
                Verdict::NotYet => not_yet += 1,
            }
        }
    }
    (same, not_yet)
}

/// The `.ok` files of a directory of this tree, sorted.
fn programs(directory: &str) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(tree::root().join(directory))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "ok"))
        .collect();
    files.sort();
    files
}

#[test]
fn t_runs_alike_on_both_engines() {
    let mut paths = programs("t/base");
    paths.extend(programs("t/op"));
    paths.extend(programs("t/lang"));
    let (same, not_yet) = sample(&paths, Some(&tree::root().join("t/support")), &Limits::default());
    // Every file builds; only async ones wait (#1122).
    assert_eq!(same + not_yet, paths.len());
    assert!(same >= paths.len() - 6, "{same} of {} compared, {not_yet} refused", paths.len());
}

/// Examples whose output depends on nothing but the program: failures and their causes, cleanups that fail,
/// panics, unbound fields, closures, roles, variants, sorting, text.
#[test]
fn examples_run_alike_on_both_engines() {
    let examples = [
        "bible/03-absence.ok",
        "bible/08-sorting.ok",
        "bible/09-rpn.ok",
        "bible/10-cause.ok",
        "bible/10-failure.ok",
        "bible/10-marks.ok",
        "bible/10-panic.ok",
        "bible/11-cleanup-fails.ok",
        "bible/11-unwind.ok",
        "bible/12-unbound.ok",
        "bible/14-roles.ok",
        "bible/16-variants.ok",
        "bible/18-closures.ok",
        "bible/25-refusals.ok",
        "collections/sort_and_search.ok",
        "text/layout.ok",
    ];
    let paths: Vec<PathBuf> = examples.iter().map(|example| tree::root().join("examples").join(example)).collect();
    let (same, not_yet) = sample(&paths, None, &Limits::default());
    assert_eq!((same, not_yet), (paths.len(), 0));
}

/// Conformance's single-file cases that expect a run to fail (`error`, `failure`): the uncaught report is the
/// observation. Those the compiler rejects (most `error` cases) are not runs.
#[test]
fn conformance_failures_report_alike_on_both_engines() {
    let mut cases: Vec<PathBuf> = Vec::new();
    for level in std::fs::read_dir(tree::root().join("conformance/cases")).unwrap() {
        for case in std::fs::read_dir(level.unwrap().path()).unwrap() {
            let case = case.unwrap().path();
            let Ok(text) = std::fs::read_to_string(case.join("case.json")) else { continue };
            let squeezed: String = text.chars().filter(|c| !c.is_whitespace()).collect();
            let fails = squeezed.contains(r#""outcome":"error""#) || squeezed.contains(r#""outcome":"failure""#);
            if squeezed.contains(r#""mode":"run""#) && fails && !squeezed.contains(r#""args""#) && squeezed.contains(r#""sources":["main.ok"]"#) {
                cases.push(case.join("main.ok"));
            }
        }
    }
    cases.sort();
    let (same, not_yet) = sample(&cases, None, &Limits::default());
    assert!(same >= MIN_CONFORMANCE_RUNS, "{same} of {} compared, {not_yet} refused", cases.len());
}

/// The cases of `conformance_failures_report_alike_on_both_engines` that build and fail at run time (#1121).
const MIN_CONFORMANCE_RUNS: usize = 50;

/// A step budget stops both engines at the same step: the same output before it and the same report.
#[test]
fn step_budgets_stop_both_engines_at_the_same_point() {
    let paths = [tree::root().join("t/op/loop.ok"), tree::root().join("t/op/call.ok"), tree::root().join("examples/bible/18-closures.ok")];
    for path in &paths {
        let imports = tree::root().join("t/support");
        let bytes = build(path, Some(&imports)).expect("it builds");
        for steps in [1, 10, 100, 1_000, 10_000] {
            let limits = Limits { steps: Some(steps), ..Limits::default() };
            assert_eq!(compare(&format!("{} --max-steps {steps}", path.display()), &bytes, &limits), Verdict::Same);
        }
    }
}
