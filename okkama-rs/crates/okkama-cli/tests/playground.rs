//! The playground answers as the CLI does (#1078): `okkama check`, `fmt` and `ir` of this binary, and the SPB of
//! its `okkama build`, against the okkama-wasm API (its plain Rust functions, no wasm runtime) on t/, the
//! Bible's rejected examples and some std modules: the same standard error, standard output and exit status,
//! byte for byte. The playground's workspace holds the bundled std under this tree's std directory, and the
//! files beside the checked one, so every file is named as the CLI names it.
mod tree;

use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::atomic::{AtomicUsize, Ordering};

use okkama_wasm::Workspace;
use serde_json::{Value, json};

/// A file to compare and its import directories (`OKKAMA_IMPORT_DIRS`).
struct Case {
    file: PathBuf,
    import_dirs: Vec<PathBuf>,
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn ok_files(dir: &Path, deep: bool, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            if deep {
                ok_files(&path, deep, out);
            }
        } else if path.extension().is_some_and(|extension| extension == "ok") {
            out.push(canonical(&path));
        }
    }
}

fn sample() -> Vec<Case> {
    let root = canonical(&tree::root());
    let mut cases = Vec::new();
    let support = root.join("t/support");
    let mut t = Vec::new();
    ok_files(&root.join("t"), true, &mut t);
    cases.extend(t.into_iter().map(|file| Case { file, import_dirs: vec![support.clone()] }));
    let mut bible = Vec::new();
    ok_files(&root.join("docs/OkkamaBible/examples"), false, &mut bible);
    bible.retain(|file| file.file_name().unwrap().to_string_lossy().contains("-rejected"));
    assert!(bible.len() >= 60, "the Bible's rejected examples: {}", bible.len());
    cases.extend(bible.into_iter().map(|file| Case { file, import_dirs: Vec::new() }));
    // template/model imports "json": std's, not template/json beside it.
    for module in ["io/stdout", "json", "fs/file", "collections/sequence", "compare", "template/model"] {
        cases.push(Case { file: canonical(&tree::std_dir().join(format!("{module}.ok"))), import_dirs: Vec::new() });
    }
    let synthetic = synthetic();
    for name in ["warn.ok", "shadow/main.ok", "shadow/json.ok", "fakes/main.ok"] {
        cases.push(Case { file: synthetic.join(name), import_dirs: Vec::new() });
    }
    cases.sort_by(|a, b| a.file.cmp(&b.file));
    cases
}

/// What the corpus lacks (tests/playground/): a warning, a module beside the file that shadows std (and imports
/// std's under the same name), and a test that uses the fake of an imported role, which only an import parsed in
/// test mode has.
fn synthetic() -> PathBuf {
    canonical(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/playground"))
}

/// The workspace of `case`: the files in its directory and in its import directories, as the CLI sees them.
fn workspace(case: &Case) -> Workspace {
    let mut workspace = Workspace::new(&canonical(&tree::std_dir()).display().to_string());
    let mut files = Vec::new();
    let dir = case.file.parent().unwrap();
    // Imports name files beside the importer and below it (`"shop/money"`).
    if !dir.starts_with(canonical(&tree::std_dir())) {
        ok_files(dir, true, &mut files);
    }
    for import in &case.import_dirs {
        ok_files(import, false, &mut files);
    }
    workspace.set_files(files.into_iter().map(|file| {
        let text = std::fs::read_to_string(&file).unwrap();
        (file.display().to_string(), text)
    }));
    workspace.set_import_dirs(case.import_dirs.iter().map(|dir| dir.display().to_string()));
    workspace
}

fn cli(case: &Case, command: &str) -> Output {
    cli_with(case, &[command])
}

fn cli_with(case: &Case, args: &[&str]) -> Output {
    let mut okkama = tree::okkama();
    okkama.env_remove("OKKAMA_IR_PASSES").env_remove("OKKAMA_IMPORT_DIRS");
    if !case.import_dirs.is_empty() {
        let dirs: Vec<String> = case.import_dirs.iter().map(|dir| dir.display().to_string()).collect();
        okkama.env("OKKAMA_IMPORT_DIRS", dirs.join(":"));
    }
    okkama.args(args).arg(&case.file).output().unwrap()
}

/// What differs between the CLI's `command` and the playground's answer to it, if anything.
fn compare(case: &Case, command: &str, answer: &Value) -> Option<String> {
    compare_with(case, &[command], answer)
}

fn compare_with(case: &Case, args: &[&str], answer: &Value) -> Option<String> {
    let command = args.join(" ");
    let output = cli_with(case, args);
    let field = |name: &str| answer[name].as_str().unwrap_or_default().to_string();
    let code = answer["code"].as_u64().unwrap_or(u64::MAX);
    let cli_code = u64::from(output.status.code().unwrap_or(-1) as u8);
    let same = field("stderr").as_bytes() == output.stderr
        && field("stdout").as_bytes() == output.stdout
        && code == cli_code;
    (!same).then(|| {
        format!(
            "{} {}:\n  cli ({cli_code}) stderr {:?}\n  playground ({code}) stderr {:?}\n  stdout same: {}",
            command,
            case.file.display(),
            String::from_utf8_lossy(&output.stderr),
            field("stderr"),
            field("stdout").as_bytes() == output.stdout,
        )
    })
}

fn request(case: &Case) -> Value {
    json!({"source": std::fs::read_to_string(&case.file).unwrap(), "path": case.file.display().to_string()})
}

/// Every case on a few threads, each with its own workspaces; the differences found.
fn differences(run: fn(&Case) -> Vec<String>) -> (usize, Vec<String>) {
    let cases = sample();
    let count = cases.len();
    let chunks: Vec<&[Case]> = cases.chunks(cases.len().div_ceil(6)).collect();
    let found = std::thread::scope(|scope| {
        let handles: Vec<_> = chunks
            .into_iter()
            .map(|chunk| {
                std::thread::Builder::new()
                    .stack_size(64 << 20)
                    .spawn_scoped(scope, move || chunk.iter().flat_map(run).collect::<Vec<_>>())
                    .unwrap()
            })
            .collect();
        handles.into_iter().flat_map(|handle| handle.join().unwrap()).collect()
    });
    (count, found)
}

#[test]
fn check_reports_what_okkama_check_reports() {
    let (count, found) = differences(|case| {
        let answer = workspace(case).check(&request(case));
        compare(case, "check", &answer).into_iter().collect()
    });
    assert!(count > 120, "the sample: {count} files");
    assert!(found.is_empty(), "{} of {count} differ:\n{}", found.len(), found.join("\n"));
}

#[test]
fn fmt_formats_as_okkama_fmt_does() {
    let (count, found) = differences(|case| {
        let answer = workspace(case).fmt(&request(case));
        compare(case, "fmt", &answer).into_iter().collect()
    });
    assert!(found.is_empty(), "{} of {count} differ:\n{}", found.len(), found.join("\n"));
}

#[test]
fn ir_prints_what_okkama_ir_prints() {
    let (count, found) = differences(|case| {
        let workspace = workspace(case);
        let mut request = request(case);
        let mut found: Vec<String> = compare_with(case, &["ir"], &workspace.ir(&request)).into_iter().collect();
        request["after"] = json!("fold");
        found.extend(compare_with(case, &["ir", "--after", "fold"], &workspace.ir(&request)));
        found
    });
    assert!(found.is_empty(), "{} of {count} differ:\n{}", found.len(), found.join("\n"));
}

/// `spa` is the assembly of the SPB `okkama build` writes, and fails as the build does.
#[test]
fn spa_disassembles_what_okkama_build_makes() {
    let (count, found) = differences(|case| {
        let answer = workspace(case).spa(&request(case));
        let stem = case.file.file_stem().unwrap().to_string_lossy().into_owned();
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir()
            .join(format!("okkama-playground-spa-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join(format!("{stem}.spb"));
        let built = cli_with(case, &["build", "-o", out.to_str().unwrap()]);
        let expected = std::fs::read(&out).ok().map(|bytes| okkama_sparrow::disassemble(&okkama_sparrow::decode(&bytes).unwrap()));
        let _ = std::fs::remove_dir_all(&dir);
        let same = answer["stdout"].as_str().unwrap_or_default() == expected.as_deref().unwrap_or_default()
            && answer["stderr"].as_str().unwrap_or_default().as_bytes() == built.stderr
            && answer["ok"].as_bool() == Some(built.status.success());
        (!same).then(|| format!("spa {}: build {:?}", case.file.display(), String::from_utf8_lossy(&built.stderr))).into_iter().collect()
    });
    assert!(found.is_empty(), "{} of {count} differ:\n{}", found.len(), found.join("\n"));
}

/// The sample is worth comparing: it has failing checks, warnings and files that import their neighbours.
#[test]
fn the_sample_has_errors_and_imports() {
    let cases = sample();
    let rejected = cases.iter().filter(|case| case.file.to_string_lossy().contains("-rejected")).count();
    let failing = cases
        .iter()
        .filter(|case| case.file.to_string_lossy().contains("-rejected"))
        .filter(|case| !cli(case, "check").status.success())
        .count();
    // Some rejected examples are refused only when they run (their `.expected` is a run-time failure).
    assert!(failing * 2 > rejected, "{failing} of {rejected} rejected examples fail `okkama check`");
    assert!(cases.iter().any(|case| !case.import_dirs.is_empty()));
}
