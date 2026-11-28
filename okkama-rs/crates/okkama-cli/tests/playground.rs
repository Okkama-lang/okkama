//! The playground answers as the CLI does (#1078): `okkama check`, `fmt` and `ir` of this binary, and the SPB of
//! its `okkama build`, against the okkama-wasm API (its plain Rust functions, no wasm runtime) on t/, the
//! Bible's rejected examples and some std modules: the same standard error, standard output and exit status,
//! byte for byte. The CLI runs in the repository root on paths relative to it, and the playground's workspace
//! is the page's: the bundled std under `std/` and the files beside the checked one, named relative to its
//! root. Neither side's paths are rewritten: an imported file's diagnostic names it as diagnostics name a found
//! file (relative to the current directory, #1119) on both. Completion, hover and definition are compared with
//! `okkama lsp`'s.
mod tree;

use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::atomic::{AtomicUsize, Ordering};

use okkama_wasm::Workspace;
use serde_json::{Value, json};

/// A file to compare and its import directories (`OKKAMA_IMPORT_DIRS`), relative to the repository root.
struct Case {
    file: PathBuf,
    import_dirs: Vec<PathBuf>,
}

/// The repository root, canonical: the CLI's current directory, the playground's root.
fn root() -> PathBuf {
    canonical(&tree::root())
}

/// `path` relative to the repository root.
fn relative(path: &Path) -> PathBuf {
    path.strip_prefix(root()).unwrap_or_else(|_| panic!("{} is outside the repository", path.display())).to_path_buf()
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
    let root = root();
    let mut cases = Vec::new();
    let support = PathBuf::from("t/support");
    let mut t = Vec::new();
    ok_files(&root.join("t"), true, &mut t);
    cases.extend(t.into_iter().map(|file| Case { file: relative(&file), import_dirs: vec![support.clone()] }));
    let mut bible = Vec::new();
    ok_files(&root.join("docs/OkkamaBible/examples"), false, &mut bible);
    bible.retain(|file| file.file_name().unwrap().to_string_lossy().contains("-rejected"));
    assert!(bible.len() >= 60, "the Bible's rejected examples: {}", bible.len());
    cases.extend(bible.into_iter().map(|file| Case { file: relative(&file), import_dirs: Vec::new() }));
    // template/model imports "json": std's, not template/json beside it.
    for module in ["io/stdout", "json", "fs/file", "collections/sequence", "compare", "template/model"] {
        cases.push(Case { file: PathBuf::from(format!("std/{module}.ok")), import_dirs: Vec::new() });
    }
    let synthetic = relative(&synthetic());
    for name in ["warn.ok", "shadow/main.ok", "shadow/json.ok", "fakes/main.ok", "imported/main.ok"] {
        cases.push(Case { file: synthetic.join(name), import_dirs: Vec::new() });
    }
    cases.sort_by(|a, b| a.file.cmp(&b.file));
    cases
}

/// What the corpus lacks (tests/playground/): a warning, a module beside the file that shadows std (and imports
/// std's under the same name), a test that uses the fake of an imported role, which only an import parsed in
/// test mode has, and an error in an imported file below the checked one.
fn synthetic() -> PathBuf {
    canonical(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/playground"))
}

/// The workspace of `case`, as the page's: the bundled std under `std/`, the files in the case's directory and
/// in its import directories, named relative to the root.
fn workspace(case: &Case) -> Workspace {
    let mut workspace = Workspace::default();
    let mut files = Vec::new();
    let root = root();
    let dir = root.join(case.file.parent().unwrap());
    // Imports name files beside the importer and below it (`"shop/money"`).
    if !case.file.starts_with("std") {
        ok_files(&dir, true, &mut files);
    }
    for import in &case.import_dirs {
        ok_files(&root.join(import), false, &mut files);
    }
    workspace.set_files(files.into_iter().map(|file| {
        let text = std::fs::read_to_string(&file).unwrap();
        (relative(&file).display().to_string(), text)
    }));
    workspace.set_import_dirs(case.import_dirs.iter().map(|dir| dir.display().to_string()));
    workspace
}

fn cli(case: &Case, command: &str) -> Output {
    cli_with(case, &[command])
}

/// `okkama ARGS FILE` in the repository root, with the case's import directories (absolute, as a user sets them).
fn cli_with(case: &Case, args: &[&str]) -> Output {
    let mut okkama = tree::okkama();
    okkama.env_remove("OKKAMA_IR_PASSES").env_remove("OKKAMA_IMPORT_DIRS").current_dir(root());
    if !case.import_dirs.is_empty() {
        let dirs: Vec<String> = case.import_dirs.iter().map(|dir| root().join(dir).display().to_string()).collect();
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
    json!({"source": std::fs::read_to_string(root().join(&case.file)).unwrap(), "path": case.file.display().to_string()})
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
    // Some diagnostics are in an imported file, which the CLI names relative to its directory (#1119).
    let imported: Vec<String> = cases
        .iter()
        .filter_map(|case| {
            let stderr = String::from_utf8_lossy(&cli(case, "check").stderr).into_owned();
            let file = stderr.split(':').next().unwrap_or_default().to_string();
            (stderr.contains(".ok:") && file != case.file.display().to_string()).then_some(file)
        })
        .collect();
    for file in ["docs/OkkamaBible/examples/cycle/customers.ok", "okkama-rs/crates/okkama-cli/tests/playground/imported/lib/broken.ok"] {
        assert!(imported.iter().any(|found| found == file), "{file} among {imported:?}");
    }
}

/// `okkama lsp` of this binary over standard input and output.
struct Lsp {
    child: std::process::Child,
    input: std::process::ChildStdin,
    messages: std::sync::mpsc::Receiver<Value>,
    next: u64,
}

impl Lsp {
    fn start() -> Self {
        use std::process::Stdio;
        let mut child = tree::okkama()
            .arg("lsp")
            .env("OKKAMA_STDLIB_DIR", canonical(&tree::std_dir()))
            .env_remove("OKKAMA_IMPORT_DIRS")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let mut output = std::io::BufReader::new(child.stdout.take().unwrap());
        let (sender, messages) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(Some(message)) = lsp_server::Message::read(&mut output) {
                if sender.send(serde_json::to_value(message).unwrap()).is_err() {
                    break;
                }
            }
        });
        let mut lsp = Self { child, input, messages, next: 0 };
        lsp.request("initialize", json!({"capabilities": {}}));
        lsp.notify("initialized", json!({}));
        lsp
    }

    fn send(&mut self, message: Value) {
        use std::io::Write;
        let text = message.to_string();
        write!(self.input, "Content-Length: {}\r\n\r\n{}", text.len(), text).unwrap();
        self.input.flush().unwrap();
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let message = self.messages.recv_timeout(std::time::Duration::from_secs(30)).expect("an LSP response");
            if message.get("id") == Some(&json!(id)) {
                assert!(message.get("error").is_none(), "{message}");
                return message["result"].clone();
            }
        }
    }
}

impl Drop for Lsp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// An LSP completion item as the playground's: its kind's name and the fields both have.
fn lsp_item(item: &Value) -> Value {
    let kind = match item["kind"].as_u64() {
        Some(2) => "method",
        Some(3) => "function",
        Some(5) => "field",
        Some(6) => "variable",
        Some(7) => "object",
        Some(8) => "role",
        Some(9) => "module",
        Some(10) => "argument",
        Some(13) => "enum",
        Some(14) => "keyword",
        Some(19) => "folder",
        Some(20) => "enumMember",
        other => panic!("kind {other:?}: {item}"),
    };
    json!({
        "label": item["label"], "kind": kind, "detail": item["detail"].as_str().unwrap_or_default(),
        "documentation": item["documentation"]["value"].as_str().unwrap_or_default(),
    })
}

fn playground_item(item: &Value) -> Value {
    json!({"label": item["label"], "kind": item["kind"], "detail": item["detail"], "documentation": item["documentation"]})
}

/// Completion answers as the language server's (#1115, #1117): `std.…` paths from the bundled std as from the
/// std directory (levels, whole paths, declarations, enum cases), in the order the editor shows them, and the
/// members of an object, a successor action's detail with one caret.
#[test]
fn completion_answers_as_the_language_server_does() {
    let dir = std::env::temp_dir().join(format!("okkama-playground-lsp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dir = canonical(&dir);
    let file = dir.join("main.ok");
    let program = "module play v1.0.0;\n\n// A bottle.\nobject Bottle {\n    has opened is bool = false;\n    Bottle() {}\n    // Opens it.\n    open() is ^Bottle { ^opened = true; }\n    state() is string { \"closed\"; }\n}\n\nmain() is int {\n    let bottle = Bottle();\n    bottle.state();\n    0;\n}\n";
    std::fs::write(&file, program).unwrap();
    let uri = format!("file://{}", file.display());
    let workspace = Workspace::default();
    let mut lsp = Lsp::start();
    // Both answer from the last inspection that was analysed (#1128): the typed texts do not parse, and `std.coll`
    // parses but does not resolve.
    lsp.notify("textDocument/didOpen", json!({"textDocument": {"uri": uri, "version": 0, "languageId": "okkama", "text": program}}));
    lsp.request("textDocument/documentSymbol", json!({"textDocument": {"uri": uri}}));
    workspace.hover(&json!({"source": program, "path": file.display().to_string(), "line": 0, "character": 0}));
    let mut version = 0;
    let mut compared = 0;
    for typed in [
        "bottle.", "let x = std.", "std.io.", "std.io.stdout.", "std.compare.Order.", "std.Stdo", "std.json.", "std.coll",
        // #1128: after a std path that does not resolve, both answer from the same inspection.
        "bottle.",
    ] {
        let text = if typed == "bottle." {
            program.replace("bottle.state();", typed)
        } else {
            program.replace("    0;\n}\n", &format!("    0;\n    {typed}\n}}\n"))
        };
        let line = text.lines().position(|line| line.trim_start() == typed).unwrap();
        let character = text.lines().nth(line).unwrap().encode_utf16().count();
        version += 1;
        lsp.notify("textDocument/didChange", json!({"textDocument": {"uri": uri, "version": version}, "contentChanges": [{"text": text}]}));
        let trigger = typed.ends_with('.').then_some(".");
        let mut context = json!({"triggerKind": 1});
        if let Some(trigger) = trigger {
            context = json!({"triggerKind": 2, "triggerCharacter": trigger});
        }
        let found = lsp.request("textDocument/completion", json!({"textDocument": {"uri": uri}, "position": {"line": line, "character": character}, "context": context}));
        let mut items: Vec<&Value> = found.as_array().unwrap_or_else(|| panic!("{typed}: {found}")).iter().collect();
        // The editor's order: by sort text, else by label.
        items.sort_by_key(|item| item["sortText"].as_str().or(item["label"].as_str()).unwrap_or_default().to_string());
        let expected: Vec<Value> = items.into_iter().map(lsp_item).collect();
        let answer = workspace.complete(&json!({"source": text, "path": file.display().to_string(), "line": line, "character": character, "trigger": trigger}));
        let mut actual: Vec<Value> = answer["items"].as_array().unwrap().iter().map(playground_item).collect();
        if typed == "bottle." {
            // Members come unordered from both; the editor sorts them by label.
            actual.sort_by_key(|item| item["label"].as_str().unwrap_or_default().to_string());
            let open = actual.iter().find(|item| item["label"] == "open").unwrap_or_else(|| panic!("{answer} vs {expected:?}"));
            assert_eq!(open["detail"], "() is ^Bottle");
        }
        assert!(!expected.is_empty(), "{typed}: the language server offers nothing");
        assert_eq!(actual, expected, "{typed}");
        compared += expected.len();
    }
    assert!(compared > 500, "{compared} items");
    drop(lsp);
    std::fs::remove_dir_all(dir).unwrap();
}

/// Hover and go to definition answer as the language server's (#1139): at every word of a program the same
/// markdown, range and definition, and, while the text being typed does not parse, the same answer at the same
/// position, which both place in the last text that did (a line of the typed text that does not parse moves the
/// lines below it).
#[test]
fn hover_and_definition_answer_as_the_language_server_does() {
    let dir = std::env::temp_dir().join(format!("okkama-playground-hover-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dir = canonical(&dir);
    let file = dir.join("main.ok");
    let program = "module play v1.0.0;\n\n// A bottle.\nobject Bottle {\n    has opened is bool = false;\n    Bottle() {}\n    // Opens it.\n    open() is ^Bottle { ^opened = true; }\n    state() is string { \"closed\"; }\n}\n\nmain() is int {\n    let bottle = Bottle();\n    let label = bottle.state();\n    label.length();\n    0;\n}\n";
    std::fs::write(&file, program).unwrap();
    let uri = format!("file://{}", file.display());
    let path = file.display().to_string();
    let workspace = Workspace::default();
    let mut lsp = Lsp::start();
    lsp.notify("textDocument/didOpen", json!({"textDocument": {"uri": uri, "version": 0, "languageId": "okkama", "text": program}}));
    lsp.request("textDocument/documentSymbol", json!({"textDocument": {"uri": uri}}));
    let typing = program.replace("main() is int {\n", "main() is int {\n    let not_yet_parsed = ;\n");
    let (mut hovers, mut definitions) = (0, 0);
    for (version, text) in [program.to_string(), typing].into_iter().enumerate() {
        if version > 0 {
            lsp.notify("textDocument/didChange", json!({"textDocument": {"uri": uri, "version": version}, "contentChanges": [{"text": text}]}));
        }
        // Inside every word of the program, the text both answer from.
        for (line, row) in program.lines().enumerate() {
            let chars: Vec<(usize, char)> = row.char_indices().collect();
            for (at, (index, ch)) in chars.iter().enumerate() {
                if !ch.is_alphanumeric() || (at > 0 && chars[at - 1].1.is_alphanumeric()) {
                    continue;
                }
                let character = row[..*index].encode_utf16().count() + 1;
                let position = json!({"line": line, "character": character});
                let request = json!({"source": text, "path": path, "line": line, "character": character});
                let found = lsp.request("textDocument/hover", json!({"textDocument": {"uri": uri}, "position": position}));
                let expected = if found.is_null() {
                    Value::Null
                } else {
                    hovers += 1;
                    json!({"markdown": found["contents"]["value"], "range": found["range"]})
                };
                let answer = workspace.hover(&request);
                assert_eq!(answer["hover"], expected, "hover {version}:{line}:{character}");
                let found = lsp.request("textDocument/definition", json!({"textDocument": {"uri": uri}, "position": position}));
                let expected = if found.is_null() {
                    Value::Null
                } else {
                    definitions += 1;
                    let path = found["uri"].as_str().unwrap().strip_prefix("file://").unwrap();
                    json!({"path": path, "range": found["range"]})
                };
                let answer = workspace.definition(&request);
                assert_eq!(answer["definition"], expected, "definition {version}:{line}:{character}");
            }
        }
    }
    assert!(hovers > 20 && definitions > 10, "{hovers} hovers, {definitions} definitions");
    drop(lsp);
    std::fs::remove_dir_all(dir).unwrap();
}
