//! The playground runs a program as `okkama run` does (#1125, design `sparrow-interpreter-1079.md` §8 item 6): the
//! examples of the Bible and the Book that the playground's profile admits give the same standard output, standard
//! error and exit status, byte for byte, built by okkama-wasm and run by sparrow-wasm (their plain Rust functions:
//! the interpreter under `Limits::playground()`, no wasm runtime) as by this binary. Both run in the example's
//! directory on its bare name, as the Bible's build.py runs them, with empty standard input; the playground's
//! workspace names std by the tree's directory and the example's neighbours relative to it, so a stack shows the
//! same file names on both sides. The output is streamed as the page takes it (#1134). An example whose run can reach
//! a native the profile refuses (#1135: over the call graph, not every native its imports declare) is counted, not
//! compared: what it needs is outside the playground (files, processes, the environment), and the profile's own
//! refusal is tested in sparrow-core. An example that declares such a native and is compared shows that the
//! reachability is not too narrow: its run would hit the refusal and differ.
mod tree;

use std::path::{Path, PathBuf};

use okkama_wasm::Workspace;
use serde_json::{Value, json};

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// `.ok` files below `dir`, relative to it.
fn ok_files(dir: &Path, prefix: &Path, out: &mut Vec<(String, String)>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        let name = prefix.join(entry.file_name());
        if path.is_dir() {
            ok_files(&path, &name, out);
        } else if path.extension().is_some_and(|extension| extension == "ok") {
            out.push((name.display().to_string(), std::fs::read_to_string(&path).unwrap()));
        }
    }
}

/// One example: the directory both sides run in, and the file's bare name there.
struct Case {
    dir: PathBuf,
    name: String,
}

/// What one side showed.
#[derive(Debug, PartialEq, Eq)]
struct Shown {
    stdout: String,
    stderr: String,
    code: i64,
}

fn cli(case: &Case) -> Shown {
    cli_with(case, &["run"])
}

/// `okkama ARGS NAME` in the case's directory.
fn cli_with(case: &Case, args: &[&str]) -> Shown {
    let output = tree::okkama()
        .env_remove("OKKAMA_IMPORT_DIRS")
        .env_remove("OKKAMA_TRACE")
        .env_remove("SPARROW_CORE_INTERPRET")
        .current_dir(&case.dir)
        .args(args)
        .arg(&case.name)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    Shown {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        code: i64::from(output.status.code().unwrap_or(-1)),
    }
}

/// Refused examples: refused natives declared, and reachable (#1135).
static NARROWED: std::sync::Mutex<Vec<(String, usize, usize)>> = std::sync::Mutex::new(Vec::new());

/// Compared examples that declare a refused native no run of theirs reaches (#1135).
static UNREACHED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The playground's run of `case`, or `None` when its profile refuses a native the program declares.
fn playground(case: &Case, std_dir: &str) -> Option<Shown> {
    let mut workspace = Workspace::new(std_dir);
    let mut files = Vec::new();
    ok_files(&case.dir, Path::new(""), &mut files);
    let source = files.iter().find(|(name, _)| *name == case.name).map(|(_, text)| text.clone()).unwrap();
    workspace.set_files(files.into_iter().filter(|(name, _)| *name != case.name));
    let built = workspace.build(&json!({"source": source, "path": case.name}));
    let mut stderr = built["stderr"].as_str().unwrap().to_string();
    if built["ok"] != true {
        return Some(Shown { stdout: String::new(), stderr, code: built["code"].as_i64().unwrap() });
    }
    let spb = workspace.built_spb();
    let needs: Value = serde_json::from_str(&sparrow_wasm::requirements(&spb)).unwrap();
    if !needs["refused"].as_array().unwrap().is_empty() {
        let count = |field: &str| needs[field].as_array().unwrap().len();
        NARROWED.lock().unwrap().push((case.name.clone(), count("declared"), count("refused")));
        // The run stops at the first refused native it calls: one `requirements` named (#1135: not too narrow).
        let ran: Value = serde_json::from_str(&sparrow_wasm::run(&spb, r#"{"stdin": ""}"#)).unwrap();
        let report = ran["report"].as_str().unwrap_or_default();
        if let Some(at) = report.find("Native ") {
            let symbol = report[at + 7..].split(' ').next().unwrap();
            assert!(needs["refused"].as_array().unwrap().iter().any(|named| named == symbol), "{}: {symbol} called, not named in {}", case.name, needs["refused"]);
        }
        return None;
    }
    if !needs["declared"].as_array().unwrap().is_empty() {
        UNREACHED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    // As the page runs it (#1134): every write streamed while the program runs.
    let chunks = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let into = chunks.clone();
    let ran = sparrow_wasm::run_streaming(&spb, r#"{"stdin": ""}"#, move |chunk| into.lock().unwrap().push(chunk));
    assert_eq!(ran["output"], json!([]), "a streamed run's answer carries no output");
    let mut stdout = String::new();
    for piece in chunks.lock().unwrap().iter() {
        let text = piece["text"].as_str().unwrap();
        if piece["stream"] == "stdout" { stdout.push_str(text) } else { stderr.push_str(text) }
    }
    if let Some(report) = ran["report"].as_str() {
        let cwd = case.dir.display().to_string();
        let failure = workspace.failure_text(&json!({"report": report, "uncaught": ran["uncaught"], "cwd": cwd}));
        stderr.push_str(failure["text"].as_str().unwrap());
    }
    Some(Shown { stdout, stderr, code: ran["exit"].as_i64().unwrap() })
}

/// The Bible's examples that run (not `-rejected`, not `-tests`: a test report is not v1's).
fn bible() -> Vec<Case> {
    let dir = canonical(&tree::root().join("docs/OkkamaBible/examples"));
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".ok") && !name.contains("-rejected") && !name.ends_with("-tests.ok"))
        .collect();
    names.sort();
    names.into_iter().map(|name| Case { dir: dir.clone(), name }).collect()
}

/// The Book's programs: each `okkama` block that declares a module and `main`, alone in a directory of its own.
fn book() -> Vec<Case> {
    let text = std::fs::read_to_string(tree::root().join("docs/OkkamaBook.md")).unwrap();
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("playground_run");
    let _ = std::fs::remove_dir_all(&base);
    let mut cases = Vec::new();
    for (index, block) in text.split("```okkama\n").skip(1).enumerate() {
        let code = block.split("```").next().unwrap_or_default();
        if !code.trim_start().starts_with("module ") || !code.contains("main(") {
            continue;
        }
        let dir = base.join(format!("book-{index:03}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("main.ok"), code).unwrap();
        cases.push(Case { dir: canonical(&dir), name: "main.ok".to_string() });
    }
    cases
}

#[test]
fn the_playground_runs_the_bible_and_the_book_as_the_cli_does() {
    let std_dir = canonical(&tree::std_dir()).display().to_string();
    let (bible, book) = (bible(), book());
    assert!(bible.len() >= 150 && book.len() >= 3, "examples: {} of the Bible, {} of the Book", bible.len(), book.len());
    let mut compared = 0;
    let mut failed = 0;
    let mut refused = Vec::new();
    let mut differences = Vec::new();
    for case in bible.iter().chain(&book) {
        let Some(played) = playground(case, &std_dir) else {
            refused.push(case.name.clone());
            continue;
        };
        compared += 1;
        failed += usize::from(played.code != 0);
        let shown = cli(case);
        if played != shown {
            differences.push(format!(
                "{}/{}:\n  cli:        {shown:?}\n  playground: {played:?}",
                case.dir.file_name().unwrap().to_string_lossy(),
                case.name
            ));
        }
    }
    let unreached = UNREACHED.load(std::sync::atomic::Ordering::Relaxed);
    eprintln!(
        "playground_run: {compared} compared ({failed} failing, {unreached} declaring a refused native they never reach), {} refused by the profile: {refused:?}",
        refused.len()
    );
    eprintln!("playground_run: refused natives declared → reachable: {:?}", NARROWED.lock().unwrap());
    assert!(differences.is_empty(), "{} of {compared} differ:\n{}", differences.len(), differences.join("\n"));
    assert!(compared >= 150, "only {compared} examples ran in the playground");
    // Failures and their stacks are compared too, not only output.
    assert!(failed >= 5, "only {failed} compared examples fail");
}

/// #1135: over examples/ (programs that read files, open sockets, start processes), a run that stops at a native the
/// profile refuses stops at one `requirements` named, and a run `requirements` cleared meets no refusal: the call
/// graph is not too narrow.
#[test]
fn requirements_name_every_refused_native_a_run_calls() {
    let std_dir = canonical(&tree::std_dir()).display().to_string();
    let root = canonical(&tree::root().join("examples"));
    let mut programs = Vec::new();
    ok_files(&root, Path::new(""), &mut programs);
    programs.retain(|(_, text)| text.contains("main()"));
    // Four threads take the programs in turn: building the web examples takes seconds in a debug build.
    let check = |(name, source): &(String, String)| -> (usize, usize, usize) {
        let path = root.join(name);
        let dir = path.parent().unwrap();
        let mut workspace = Workspace::new(&std_dir);
        let mut files = Vec::new();
        ok_files(dir, Path::new(""), &mut files);
        let file = path.file_name().unwrap().to_string_lossy().into_owned();
        workspace.set_files(files.into_iter().filter(|(other, _)| *other != file));
        if workspace.build(&json!({"source": source, "path": file}))["ok"] != true {
            return (0, 0, 0);
        }
        let spb = workspace.built_spb();
        let needs: Value = serde_json::from_str(&sparrow_wasm::requirements(&spb)).unwrap();
        let refused = needs["refused"].as_array().unwrap();
        let ran: Value = serde_json::from_str(&sparrow_wasm::run(&spb, r#"{"stdin": ""}"#)).unwrap();
        let report = ran["report"].as_str().unwrap_or_default();
        match report.find("is not available in the playground").and_then(|end| report[..end].rfind("Native ").map(|at| &report[at + 7..end])) {
            Some(symbol) => {
                assert!(refused.iter().any(|named| named == symbol.trim()), "{name}: {symbol} called, not in {refused:?}");
                (1, 0, 0)
            }
            None => {
                let declared = !needs["declared"].as_array().unwrap().is_empty();
                (0, usize::from(refused.is_empty()), usize::from(refused.is_empty() && declared))
            }
        }
    };
    let next = std::sync::atomic::AtomicUsize::new(0);
    let counts: Vec<(usize, usize, usize)> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..4)
            .map(|_| {
                scope.spawn(|| {
                    let mut counts = Vec::new();
                    while let Some(program) = programs.get(next.fetch_add(1, std::sync::atomic::Ordering::Relaxed)) {
                        counts.push(check(program));
                    }
                    counts
                })
            })
            .collect();
        workers.into_iter().flat_map(|worker| worker.join().unwrap()).collect()
    });
    let (stopped, cleared, narrowed) = counts.iter().fold((0, 0, 0), |(a, b, c), (d, e, f)| (a + d, b + e, c + f));
    eprintln!(
        "examples: {} programs, {stopped} stopped at a refused native, {cleared} cleared by requirements ({narrowed} of them declaring one)",
        programs.len()
    );
    assert!(stopped >= 20, "only {stopped} runs met the profile");
}

/// The playground's test run of `case` (#1127), as the page runs it (streamed): `None` when the profile refuses a
/// native its tests can reach. `std_root` names std as the CLI in `case.dir` names it.
fn playground_tests(case: &Case, std_root: &str) -> Option<Shown> {
    let mut workspace = Workspace::new(std_root);
    let mut files = Vec::new();
    ok_files(&case.dir.join(Path::new(&case.name).parent().unwrap()), Path::new(""), &mut files);
    let source = std::fs::read_to_string(case.dir.join(&case.name)).unwrap();
    let file = Path::new(&case.name).file_name().unwrap().to_string_lossy().into_owned();
    if !case.name.starts_with("std/") {
        workspace.set_files(files.into_iter().filter(|(name, _)| *name != file));
    }
    let built = workspace.build(&json!({"source": source, "path": case.name, "tests": true}));
    let mut stderr = built["stderr"].as_str().unwrap().to_string();
    if built["ok"] != true {
        return Some(Shown { stdout: String::new(), stderr, code: built["code"].as_i64().unwrap() });
    }
    let spb = workspace.built_spb();
    let needs: Value = serde_json::from_str(&sparrow_wasm::requirements(&spb)).unwrap();
    if !needs["refused"].as_array().unwrap().is_empty() {
        return None;
    }
    let chunks = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let into = chunks.clone();
    let ran = sparrow_wasm::test_streaming(&spb, r#"{"stdin": ""}"#, move |chunk| into.lock().unwrap().push(chunk));
    let mut stdout = String::new();
    for piece in chunks.lock().unwrap().iter() {
        let text = piece["text"].as_str().unwrap();
        if piece["stream"] == "stdout" { stdout.push_str(text) } else { stderr.push_str(text) }
    }
    if let Some(report) = ran["report"].as_str() {
        stderr.push_str(report);
    }
    Some(Shown { stdout, stderr, code: ran["exit"].as_i64().unwrap() })
}

/// #1127: the tests of the Bible's `-tests` examples and of std modules that the profile admits report as `okkama run
/// -t` reports them, byte for byte: the human report, the tests' output and the exit status. `okkama run -t` also
/// checks the file's documentation examples (`ok - doctest …` after the report), each a program of its own; the
/// playground does not, so those lines are left out of the CLI's side.
#[test]
fn the_playground_runs_tests_as_okkama_run_t() {
    let examples = canonical(&tree::root().join("docs/OkkamaBible/examples"));
    let mut names: Vec<String> = std::fs::read_dir(&examples)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with("-tests.ok"))
        .collect();
    names.sort();
    assert!(names.len() >= 10, "the Bible's test examples: {}", names.len());
    let std_dir = canonical(&tree::std_dir()).display().to_string();
    let mut cases: Vec<(Case, String)> = names.into_iter().map(|name| (Case { dir: examples.clone(), name }, std_dir.clone())).collect();
    let root = canonical(&tree::root());
    let modules = [
        "json", "csv", "diff", "statistics", "calendar", "cell", "strings/join", "strings/format", "strings/buffer", "toml/parse",
        "toml/value", "plist/parse", "crypto/hmac", "crypto/kdf", "crypto/constant_time", "digest/sha2", "digest/sha3",
        "digest/blake2", "digest/legacy", "digest/host", "uuid/value", "uuid/generation", "checksum/crc", "checksum/adler32",
        "checksum/state", "parallel/map",
    ];
    for module in modules {
        cases.push((Case { dir: root.clone(), name: format!("std/{module}.ok") }, "std".to_string()));
    }
    let (mut compared, mut failing, mut refused) = (0, 0, Vec::new());
    let mut differences = Vec::new();
    for (case, std_root) in &cases {
        let Some(played) = playground_tests(case, std_root) else {
            refused.push(case.name.clone());
            continue;
        };
        compared += 1;
        failing += usize::from(played.code != 0);
        let mut shown = cli_with(case, &["run", "-t"]);
        shown.stdout = shown.stdout.lines().filter(|line| !line.starts_with("ok - doctest ") && !line.starts_with("not ok - doctest ")).map(|line| format!("{line}\n")).collect();
        if played != shown {
            differences.push(format!("{}:\n  cli:        {shown:?}\n  playground: {played:?}", case.name));
        }
    }
    eprintln!("playground tests: {compared} compared ({failing} failing), {} refused: {refused:?}", refused.len());
    assert!(differences.is_empty(), "{} of {compared} differ:\n{}", differences.len(), differences.join("\n"));
    assert!(compared >= 36, "only {compared} test suites ran in the playground");
}


/// #1127 (design §9 #6, awaiting the owner): digests, HMAC, HKDF, named UUIDs and checksums are in the profile and
/// give the CLI's output; their std modules' own tests run in `the_playground_runs_tests_as_okkama_run_t`.
#[test]
fn the_playground_hashes_as_the_cli_does() {
    let std_dir = canonical(&tree::std_dir()).display().to_string();
    let case = Case { dir: canonical(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/playground/digests")), name: "main.ok".into() };
    let played = playground(&case, &std_dir).expect("the profile admits digests, MACs, HKDF, UUIDs and checksums");
    assert_eq!(played, cli(&case));
    assert_eq!(played.code, 0, "{}", played.stderr);
    assert!(played.stdout.starts_with("sha256 ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\n"), "{}", played.stdout);
}

/// #1127 (owner's decision 8): workers are emulated in the playground's instance and give what the CLI's process
/// workers give: results, a worker's output passed on whole before its result, a failure as text, a bound action's
/// receiver rebuilt, an ordered parallel map, and a value that cannot cross refused (tests/playground/workers/).
#[test]
fn the_playground_emulates_workers_as_the_cli_runs_them() {
    let std_dir = canonical(&tree::std_dir()).display().to_string();
    let dir = canonical(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/playground/workers"));
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".ok"))
        .collect();
    names.sort();
    assert!(names.len() >= 6, "{names:?}");
    let mut differences = Vec::new();
    for name in names {
        let case = Case { dir: dir.clone(), name };
        let played = playground(&case, &std_dir).unwrap_or_else(|| panic!("{}: the profile admits workers", case.name));
        let shown = cli(&case);
        if case.name == "failure.ok" {
            // #1141: the worker's failure is the parent's runtime failure with one prefix, as the CLI prints the same
            // failure of the parent's own.
            let stderr = &shown.stderr;
            assert!(stderr.contains("RuntimeError: confirm failed") && !stderr.contains("RuntimeError: RuntimeError"), "{shown:?}");
        }
        if played != shown {
            differences.push(format!("{}:\n  cli:        {shown:?}\n  playground: {played:?}", case.name));
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}
