//! The Okkama compiler for the playground (#1078): `okkama check`, `fmt`, `tokens`, `ast`, `ir`, the Sparrow
//! assembly of `okkama build`, and the editor's hover, completion, definition and signature help, over a std
//! bundled into the crate. Nothing runs: there is no Sparrow here.
//!
//! Every call takes one JSON request and returns one JSON response (shapes in `docs/map/decisions.md`, #1078).
//! The text a command prints comes from `okkama_compiler::commands`, the CLI's own, so the playground and
//! `okkama` report the same thing; imports resolve through [`Workspace`], a loader over the bundled std and the
//! files the page adds, never a file system or the environment. The wasm build exports the same functions to
//! JavaScript (`wasm-bindgen`); natively they are plain Rust, which is how `okkama-cli/tests/playground.rs`
//! compares them with the CLI.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use okkama_compiler::commands::{self, Finding, Report};
use okkama_compiler::ide::{self, Completion, CompletionKind, Inspection, LineIndex, Position, Severity};
use okkama_compiler::lowering::{self, LoadedModule, ModuleLoader};
use okkama_compiler::source::SourceFile;
use okkama_compiler::syntax::ParseMode;
use okkama_compiler::syntax::print::{Format, PrintOptions};
use serde_json::{Value, json};

mod bundle {
    include!(concat!(env!("OUT_DIR"), "/std_files.rs"));
}

/// The directory the bundled std has in the playground's file names: `std/io/stdout.ok`.
pub const STD_ROOT: &str = "std";

/// The bundled std: path under std (`io/stdout.ok`) and text.
pub fn std_files() -> &'static [(&'static str, &'static str)] {
    bundle::FILES
}

/// The exit status of a command that reported diagnostics, and of a usage error (`okkama`'s).
const EXIT_DIAGNOSTICS: u8 = 1;
const EXIT_USAGE: u8 = 2;

/// The files a program can import: the bundled std under its root, the page's own files, and import
/// directories (`OKKAMA_IMPORT_DIRS`). It keeps the last inspection of each file that parsed, as the language
/// server does, so completion works while the text being typed does not parse.
#[derive(Debug)]
pub struct Workspace {
    std_root: String,
    std: HashMap<String, &'static str>,
    files: BTreeMap<String, String>,
    import_dirs: Vec<String>,
    parsed: RefCell<HashMap<String, Parsed>>,
}

/// The text a file last parsed from and its inspection.
type Parsed = (Rc<str>, Rc<Inspection>);

impl Default for Workspace {
    fn default() -> Self {
        Self::new(STD_ROOT)
    }
}

impl Workspace {
    /// The bundled std under `std_root`: `std` in the playground; the native parity test names the
    /// repository's std directory, so the files are named as the CLI names them.
    pub fn new(std_root: &str) -> Self {
        let std_root = normalize(std_root);
        let std = bundle::FILES.iter().map(|(path, text)| (join(&std_root, path), *text)).collect();
        Self { std_root, std, files: BTreeMap::new(), import_dirs: Vec::new(), parsed: RefCell::default() }
    }

    /// The page's own files besides the one being checked, which it may import (`path` → text).
    pub fn set_files(&mut self, files: impl IntoIterator<Item = (String, String)>) {
        self.files = files.into_iter().map(|(path, text)| (normalize(&path), text)).collect();
        self.parsed.borrow_mut().clear();
    }

    /// Directories searched after the importing file's own, before std (`OKKAMA_IMPORT_DIRS`).
    pub fn set_import_dirs(&mut self, dirs: impl IntoIterator<Item = String>) {
        self.import_dirs = dirs.into_iter().map(|dir| normalize(&dir)).collect();
    }

    fn text(&self, file: &str) -> Option<&str> {
        self.files.get(file).map(String::as_str).or_else(|| self.std.get(file).copied())
    }

    fn loader(&self, test_mode: bool) -> Loader<'_> {
        Loader { workspace: self, test_mode }
    }

    /// `okkama check PATH`: request `{source, path, tests?}`.
    pub fn check(&self, request: &Value) -> Value {
        let source = match self.source(request) {
            Ok(source) => source,
            Err(response) => return response,
        };
        let tests = flag(request, "tests");
        let report = commands::check(std::slice::from_ref(&source), tests, &mut |test_mode| self.loader(test_mode));
        if report.failed() {
            return self.failure(report, &source);
        }
        self.outcome(String::new(), report, &source)
    }

    /// `okkama fmt PATH`: request `{source, path}`; the formatted text on `stdout`.
    pub fn fmt(&self, request: &Value) -> Value {
        let source = match self.source(request) {
            Ok(source) => source,
            Err(response) => return response,
        };
        match commands::format(&source) {
            Ok(formatted) => self.outcome(formatted, Report::default(), &source),
            Err(report) => self.failure(report, &source),
        }
    }

    /// `okkama tokens [--format text|json] PATH`: request `{source, path, format?}`.
    pub fn tokens(&self, request: &Value) -> Value {
        let source = match self.source(request) {
            Ok(source) => source,
            Err(response) => return response,
        };
        let json = match text_field(request, "format") {
            None | Some("text") => false,
            Some("json") => true,
            Some(other) => return usage(&format!("unknown token format `{other}` (use text or json)")),
        };
        match commands::tokens(&source, json) {
            Ok(out) => self.outcome(out, Report::default(), &source),
            Err(report) => self.failure(report, &source),
        }
    }

    /// `okkama ast [--format tree|json] [--mode full|production] [--spans] [--prune-tests] PATH`: request
    /// `{source, path, format?, mode?, spans?, pruneTests?}`.
    pub fn ast(&self, request: &Value) -> Value {
        let source = match self.source(request) {
            Ok(source) => source,
            Err(response) => return response,
        };
        let format = match text_field(request, "format") {
            None | Some("tree") => Format::Tree,
            Some("json") => Format::Json,
            Some(other) => return usage(&format!("unknown AST format `{other}` (use tree or json)")),
        };
        let mode = match text_field(request, "mode") {
            None | Some("full") => ParseMode::Full,
            Some("production") => ParseMode::Production,
            Some(other) => return usage(&format!("unknown mode `{other}` (use full or production)")),
        };
        let options = PrintOptions { spans: flag(request, "spans") };
        match commands::ast(&source, mode, flag(request, "pruneTests"), format, options) {
            Ok(out) => self.outcome(out, Report::default(), &source),
            Err(report) => self.failure(report, &source),
        }
    }

    /// `okkama ir [--passes LIST | --after PASS] [--tests] PATH`: request `{source, path, passes?, after?,
    /// tests?}`; OIR-2 text on `stdout`, the warnings on `stderr`.
    pub fn ir(&self, request: &Value) -> Value {
        let source = match self.source(request) {
            Ok(source) => source,
            Err(response) => return response,
        };
        let spec = match (text_field(request, "after"), text_field(request, "passes")) {
            (Some(after), _) => commands::passes_after(after).map(Some),
            (None, Some(passes)) => commands::passes_named(passes).map(Some),
            (None, None) => Ok(None),
        };
        let spec = match spec {
            Ok(spec) => spec,
            Err(message) => return usage(&message),
        };
        let lower = || commands::lower(std::slice::from_ref(&source), flag(request, "tests"), &mut |test_mode| self.loader(test_mode));
        let lowered = match &spec {
            Some(spec) => lowering::with_passes(spec, lower),
            None => lower(),
        };
        match lowered {
            Ok((module, warnings)) => self.outcome(module.to_string(), warnings, &source),
            Err(report) => self.failure(report, &source),
        }
    }

    /// The Sparrow assembly of the program `okkama build` makes, as `sparrow --disassemble` prints it:
    /// request `{source, path, tests?}`; the assembly on `stdout`, the warnings on `stderr`, the size of the
    /// SPB in `spbBytes`.
    pub fn spa(&self, request: &Value) -> Value {
        let source = match self.source(request) {
            Ok(source) => source,
            Err(response) => return response,
        };
        let name = source.name().rsplit('/').next().unwrap_or_default();
        let name = name.strip_suffix(".ok").unwrap_or(name);
        let built = commands::build(std::slice::from_ref(&source), flag(request, "tests"), &mut |test_mode| self.loader(test_mode), name);
        let (artifact, warnings) = match built {
            Ok(built) => built,
            Err(report) => return self.failure(report, &source),
        };
        match okkama_sparrow::decode(artifact.main()) {
            Ok(program) => {
                let mut response = self.outcome(okkama_sparrow::disassemble(&program), warnings, &source);
                response["spbBytes"] = json!(artifact.main().len());
                response
            }
            Err(error) => self.failure(Report::listed(vec![Finding::rendered(format!("{error}\n"))]), &source),
        }
    }

    /// Hover at `{source, path, line, character}` (zero-based, UTF-16): `{hover: {markdown, range} | null}`.
    pub fn hover(&self, request: &Value) -> Value {
        let Some((_, offset, text, inspection)) = self.at(request) else { return json!({"ok": true, "hover": null}) };
        let hover = inspection.symbol_at(offset).map(|symbol| {
            json!({"markdown": symbol.hover, "range": range(LineIndex::new(&text).range(symbol.span))})
        });
        json!({"ok": true, "hover": hover})
    }

    /// Go to definition at `{source, path, line, character}`: `{definition: {path, range} | null}`.
    pub fn definition(&self, request: &Value) -> Value {
        let Some((_, offset, _, inspection)) = self.at(request) else { return json!({"ok": true, "definition": null}) };
        let definition = inspection.symbol_at(offset).and_then(|symbol| symbol.definition.as_ref()).map(|location| {
            json!({"path": location.path, "range": range(location.range)})
        });
        json!({"ok": true, "definition": definition})
    }

    /// Signature help at `{source, path, line, character}`: `{signature: {label, parameters, active} | null}`.
    pub fn signature(&self, request: &Value) -> Value {
        let Some((source, offset, text, inspection)) = self.at(request) else {
            return json!({"ok": true, "signature": null});
        };
        let signature = inspection.signature(&text, source.text(), offset).map(|(label, parameters, active)| {
            json!({"label": label, "parameters": parameters, "active": active})
        });
        json!({"ok": true, "signature": signature})
    }

    /// Completion at `{source, path, line, character, trigger?}`: `{items: [{label, kind, detail,
    /// documentation, insert, snippet}]}`. After `(` and `,` only argument names; `"` and `/` open completion
    /// for import paths only, which the playground does not offer yet.
    pub fn complete(&self, request: &Value) -> Value {
        let trigger = text_field(request, "trigger");
        if matches!(trigger, Some("\"" | "/")) {
            return json!({"ok": true, "items": []});
        }
        let Some((source, offset, text, inspection)) = self.at(request) else { return json!({"ok": true, "items": []}) };
        let found = if matches!(trigger, Some("(" | ",")) {
            inspection.argument_names(&text, source.text(), offset)
        } else {
            inspection.complete(&text, source.text(), offset)
        };
        json!({"ok": true, "items": found.iter().map(completion).collect::<Vec<_>>()})
    }

    /// The request's file, its offset of `line`/`character`, and the last inspection of it that parsed, with
    /// the text it inspected.
    fn at(&self, request: &Value) -> Option<(SourceFile, u32, Rc<str>, Rc<Inspection>)> {
        let source = self.source(request).ok()?;
        let line = request.get("line")?.as_u64()? as u32;
        let character = request.get("character")?.as_u64()? as u32;
        let offset = LineIndex::new(source.text()).offset(Position { line, character });
        let inspection = commands::inspect(&source, &mut |test_mode| self.loader(test_mode));
        let mut parsed = self.parsed.borrow_mut();
        if inspection.parsed {
            parsed.insert(source.name().to_string(), (Rc::from(source.text()), Rc::new(inspection)));
        }
        let (text, inspection) = parsed.get(source.name())?.clone();
        Some((source, offset, text, inspection))
    }

    /// The request's `source`, named by its `path` (`main.ok` without one).
    fn source(&self, request: &Value) -> Result<SourceFile, Value> {
        let Some(text) = text_field(request, "source") else { return Err(usage("the request needs a `source` text")) };
        let name = normalize(text_field(request, "path").unwrap_or("main.ok"));
        // A text past the size limit is refused as the CLI refuses the file.
        SourceFile::new(name, text).map_err(|error| {
            let text = format!("okkama: {error}\n");
            json!({"ok": false, "code": EXIT_DIAGNOSTICS, "stdout": "", "stderr": text, "diagnostics": [{
                "file": "", "severity": "error", "kind": "", "message": text.trim_end(), "text": text,
                "span": null, "range": null,
            }]})
        })
    }

    /// A command that did its work: `stdout`, and its warnings on `stderr`.
    fn outcome(&self, stdout: String, report: Report, root: &SourceFile) -> Value {
        json!({"ok": true, "code": 0, "stdout": stdout, "stderr": report.text, "diagnostics": self.diagnostics(&report, root)})
    }

    /// A command that failed with diagnostics.
    fn failure(&self, report: Report, root: &SourceFile) -> Value {
        json!({"ok": false, "code": EXIT_DIAGNOSTICS, "stdout": "", "stderr": report.text, "diagnostics": self.diagnostics(&report, root)})
    }

    /// The report's findings, each with its range in its file (zero-based lines, UTF-16 columns) where the
    /// file is one the workspace knows.
    fn diagnostics(&self, report: &Report, root: &SourceFile) -> Vec<Value> {
        report
            .findings
            .iter()
            .map(|found| {
                let text = if found.file == root.name() { Some(root.text()) } else { self.text(&found.file) };
                let span = found.span.map(|span| json!({"start": span.start, "end": span.end}));
                let range = found.span.zip(text).map(|(span, text)| range(LineIndex::new(text).range(span)));
                json!({
                    "file": found.file,
                    "severity": match found.severity { Severity::Error => "error", Severity::Warning => "warning" },
                    "kind": found.kind,
                    "message": found.message,
                    "text": found.text,
                    "span": span,
                    "range": range,
                })
            })
            .collect()
    }
}

/// Imports from the workspace, in the CLI's order without Blade packages (#1078): a std file finds its imports
/// in std first, then beside itself; any other file beside itself, then in the import directories, then in std.
/// A module never imports itself: `"json"` from `app/json.ok` finds `std/json.ok`.
#[derive(Debug)]
struct Loader<'w> {
    workspace: &'w Workspace,
    test_mode: bool,
}

impl Loader<'_> {
    fn in_std(&self, file: &str) -> bool {
        file.strip_prefix(self.workspace.std_root.as_str()).is_some_and(|rest| rest.starts_with('/'))
    }

    /// The file name of import `path` and the directories to look in, by order.
    fn candidates(&self, path: &str, importer: &str) -> (String, Vec<String>) {
        let file = if path.ends_with(".ok") { path.to_string() } else { format!("{path}.ok") };
        let importer = normalize(importer);
        let base = parent(&importer);
        let dirs = if self.in_std(&importer) {
            vec![self.workspace.std_root.clone(), base]
        } else {
            let mut dirs = vec![base];
            dirs.extend(self.workspace.import_dirs.iter().cloned());
            dirs.push(self.workspace.std_root.clone());
            dirs
        };
        let dirs = dirs.into_iter().filter(|dir| join(dir, &file) != importer).collect();
        (file, dirs)
    }

    fn locate(&self, file: &str, dirs: &[String]) -> Option<String> {
        dirs.iter().map(|dir| join(dir, file)).find(|found| self.workspace.text(found).is_some())
    }

    fn load_from(&self, file: &str, dirs: &[String]) -> Result<Option<LoadedModule>, String> {
        let Some(found) = self.locate(file, dirs) else { return Ok(None) };
        let text = self.workspace.text(&found).unwrap_or_default().to_string();
        commands::module(found, text, self.test_mode).map(Some)
    }
}

impl ModuleLoader for Loader<'_> {
    fn load(&mut self, path: &str, importer: &str) -> Result<Option<LoadedModule>, String> {
        let (file, dirs) = self.candidates(path, importer);
        self.load_from(&file, &dirs)
    }

    fn resolve(&mut self, path: &str, importer: &str) -> Result<Option<String>, String> {
        let (file, dirs) = self.candidates(path, importer);
        Ok(self.locate(&file, &dirs))
    }

    fn resolve_std(&mut self, path: &str, _importer: &str) -> Result<Option<String>, String> {
        Ok(self.locate(&format!("{path}.ok"), std::slice::from_ref(&self.workspace.std_root)))
    }

    fn load_std(&mut self, path: &str, _importer: &str) -> Result<Option<LoadedModule>, String> {
        self.load_from(&format!("{path}.ok"), std::slice::from_ref(&self.workspace.std_root))
    }
}

/// `a/./b/../c.ok` as `a/c.ok`; a leading `/` stays.
fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    let joined = parts.join("/");
    if path.starts_with('/') { format!("/{joined}") } else { joined }
}

/// The directory of `file`: empty for a bare name; a virtual name (`<stdin>`) has none.
fn parent(file: &str) -> String {
    if file.starts_with('<') {
        return String::new();
    }
    match file.rsplit_once('/') {
        Some(("", _)) => "/".to_string(),
        Some((dir, _)) => dir.to_string(),
        None => String::new(),
    }
}

fn join(dir: &str, file: &str) -> String {
    if dir.is_empty() { normalize(file) } else { normalize(&format!("{dir}/{file}")) }
}

fn text_field<'a>(request: &'a Value, name: &str) -> Option<&'a str> {
    request.get(name).and_then(Value::as_str)
}

fn flag(request: &Value, name: &str) -> bool {
    request.get(name).and_then(Value::as_bool).unwrap_or(false)
}

/// A usage error, as `okkama` prints one.
fn usage(message: &str) -> Value {
    let text = format!("okkama: {message}\nTry `okkama --help` for more information.\n");
    json!({"ok": false, "code": EXIT_USAGE, "stdout": "", "stderr": text, "diagnostics": []})
}

fn range(range: ide::Range) -> Value {
    json!({
        "start": {"line": range.start.line, "character": range.start.character},
        "end": {"line": range.end.line, "character": range.end.character},
    })
}

fn completion(found: &Completion) -> Value {
    let kind = match found.kind {
        CompletionKind::Method => "method",
        CompletionKind::Field => "field",
        CompletionKind::Variable => "variable",
        CompletionKind::Object => "object",
        CompletionKind::Role => "role",
        CompletionKind::Enum => "enum",
        CompletionKind::EnumMember => "enumMember",
        CompletionKind::Function => "function",
        CompletionKind::Keyword => "keyword",
        CompletionKind::Module => "module",
        CompletionKind::Argument => "argument",
    };
    json!({
        "label": found.label, "kind": kind, "detail": found.detail, "documentation": found.documentation,
        "insert": found.insert, "snippet": found.snippet,
    })
}

thread_local! {
    /// The playground's one workspace: the bundled std under `std/` and the page's files.
    static WORKSPACE: RefCell<Workspace> = RefCell::new(Workspace::default());
}

/// Parse `request`, run `command` on the playground's workspace, and print the response.
pub fn handle(request: &str, command: fn(&Workspace, &Value) -> Value) -> String {
    let response = match serde_json::from_str::<Value>(request) {
        Ok(request) => WORKSPACE.with(|workspace| command(&workspace.borrow(), &request)),
        Err(error) => usage(&format!("the request is not JSON: {error}")),
    };
    response.to_string()
}

/// The page's own files: request `{files: {path: text}, importDirs?: [dir]}`; replaces the previous set.
pub fn set_files(request: &str) -> String {
    let request: Value = match serde_json::from_str(request) {
        Ok(request) => request,
        Err(error) => return usage(&format!("the request is not JSON: {error}")).to_string(),
    };
    let files = request.get("files").and_then(Value::as_object).map(|files| {
        files.iter().filter_map(|(path, text)| Some((path.clone(), text.as_str()?.to_string()))).collect::<Vec<_>>()
    });
    let dirs = request.get("importDirs").and_then(Value::as_array).map(|dirs| {
        dirs.iter().filter_map(|dir| dir.as_str().map(str::to_string)).collect::<Vec<_>>()
    });
    WORKSPACE.with(|workspace| {
        let mut workspace = workspace.borrow_mut();
        workspace.set_files(files.unwrap_or_default());
        workspace.set_import_dirs(dirs.unwrap_or_default());
    });
    json!({"ok": true}).to_string()
}

/// The compiler's version and the bundled std: `{version, stdModules, stdBytes}`.
pub fn version() -> String {
    let bytes: usize = bundle::FILES.iter().map(|(_, text)| text.len()).sum();
    json!({"version": env!("CARGO_PKG_VERSION"), "stdModules": bundle::FILES.len(), "stdBytes": bytes}).to_string()
}

/// The exports to JavaScript: each takes the request's JSON text and returns the response's.
#[cfg(target_arch = "wasm32")]
mod bindings {
    use wasm_bindgen::prelude::wasm_bindgen;

    use super::{Workspace, handle};

    #[wasm_bindgen]
    pub fn version() -> String {
        super::version()
    }

    #[wasm_bindgen(js_name = setFiles)]
    pub fn set_files(request: &str) -> String {
        super::set_files(request)
    }

    #[wasm_bindgen]
    pub fn check(request: &str) -> String {
        handle(request, Workspace::check)
    }

    #[wasm_bindgen]
    pub fn fmt(request: &str) -> String {
        handle(request, Workspace::fmt)
    }

    #[wasm_bindgen]
    pub fn tokens(request: &str) -> String {
        handle(request, Workspace::tokens)
    }

    #[wasm_bindgen]
    pub fn ast(request: &str) -> String {
        handle(request, Workspace::ast)
    }

    #[wasm_bindgen]
    pub fn ir(request: &str) -> String {
        handle(request, Workspace::ir)
    }

    #[wasm_bindgen]
    pub fn spa(request: &str) -> String {
        handle(request, Workspace::spa)
    }

    #[wasm_bindgen]
    pub fn hover(request: &str) -> String {
        handle(request, Workspace::hover)
    }

    #[wasm_bindgen]
    pub fn complete(request: &str) -> String {
        handle(request, Workspace::complete)
    }

    #[wasm_bindgen]
    pub fn definition(request: &str) -> String {
        handle(request, Workspace::definition)
    }

    #[wasm_bindgen]
    pub fn signature(request: &str) -> String {
        handle(request, Workspace::signature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_normalize_and_join_as_the_file_system_does() {
        assert_eq!(normalize("a/./b/../c.ok"), "a/c.ok");
        assert_eq!(normalize("/x//y.ok"), "/x/y.ok");
        assert_eq!(parent("main.ok"), "");
        assert_eq!(parent("/main.ok"), "/");
        assert_eq!(parent("<stdin>"), "");
        assert_eq!(join("", "io/stdout.ok"), "io/stdout.ok");
        assert_eq!(join("std", "io/stdout.ok"), "std/io/stdout.ok");
    }

    #[test]
    fn imports_resolve_beside_the_file_then_in_std_and_never_to_itself() {
        let mut workspace = Workspace::default();
        workspace.set_files([("app/json.ok".to_string(), "module app/json;\n".to_string()), ("app/util.ok".to_string(), String::new())]);
        let mut loader = workspace.loader(false);
        // Beside the importer first.
        assert_eq!(loader.resolve("util", "app/main.ok").unwrap().as_deref(), Some("app/util.ok"));
        // `"json"` from app/json.ok is not itself: std's.
        assert_eq!(loader.resolve("json", "app/json.ok").unwrap().as_deref(), Some("std/json.ok"));
        assert_eq!(loader.resolve("json", "app/main.ok").unwrap().as_deref(), Some("app/json.ok"));
        // A std file looks in std first.
        assert_eq!(loader.resolve("io/stdout", "std/json.ok").unwrap().as_deref(), Some("std/io/stdout.ok"));
        assert_eq!(loader.resolve("missing/module", "main.ok").unwrap(), None);
    }

    #[test]
    fn check_reports_spans_in_the_buffer() {
        let workspace = Workspace::default();
        let source = "module play v1.0.0;\n\nmain() is int {\n    let x = y;\n    0;\n}\n";
        let response = workspace.check(&json!({"source": source, "path": "main.ok"}));
        assert_eq!(response["ok"], false);
        assert_eq!(response["code"], 1);
        let found = &response["diagnostics"][0];
        assert_eq!(found["file"], "main.ok");
        assert_eq!(found["range"]["start"]["line"], 3, "{response}");
        assert!(response["stderr"].as_str().unwrap().starts_with("main.ok:4:"), "{response}");
    }

    #[test]
    fn a_program_with_std_imports_checks_and_builds() {
        let workspace = Workspace::default();
        let source = "module play v1.0.0;\n\nimport \"io/stdout\" take Stdout;\n\nmain() is int {\n    Stdout().writeln(\"hi\");\n    0;\n}\n";
        let checked = workspace.check(&json!({"source": source}));
        assert_eq!(checked["ok"], true, "{checked}");
        let spa = workspace.spa(&json!({"source": source}));
        assert_eq!(spa["ok"], true, "{spa}");
        assert!(spa["spbBytes"].as_u64().is_some_and(|bytes| bytes > 0), "{spa}");
        let ir = workspace.ir(&json!({"source": source, "after": "fold"}));
        assert_eq!(ir["ok"], true, "{ir}");
        let refused = workspace.ir(&json!({"source": source, "passes": "fold,nope"}));
        assert_eq!(refused["code"], 2);
    }

    #[test]
    fn hover_and_completion_answer_from_the_last_parse() {
        let workspace = Workspace::default();
        let source = "module play v1.0.0;\n\nmain() is int {\n    let total = 1;\n    let other = total;\n    other;\n}\n";
        let hover = workspace.hover(&json!({"source": source, "line": 4, "character": 18}));
        assert!(hover["hover"]["markdown"].as_str().is_some_and(|text| text.contains("total")), "{hover}");
        // The text being typed does not parse: completion answers from the last parse.
        let typing = "module play v1.0.0;\n\nmain() is int {\n    let total = 1;\n    let other = to\n    other;\n}\n";
        let complete = workspace.complete(&json!({"source": typing, "line": 4, "character": 18}));
        let labels: Vec<&str> = complete["items"].as_array().unwrap().iter().filter_map(|item| item["label"].as_str()).collect();
        assert!(labels.contains(&"total"), "{complete}");
    }
}
