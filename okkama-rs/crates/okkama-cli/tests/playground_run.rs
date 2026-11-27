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
    let output = tree::okkama()
        .env_remove("OKKAMA_IMPORT_DIRS")
        .env_remove("OKKAMA_TRACE")
        .env_remove("SPARROW_CORE_INTERPRET")
        .current_dir(&case.dir)
        .args(["run", &case.name])
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
