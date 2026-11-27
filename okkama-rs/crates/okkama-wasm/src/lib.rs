//! The Okkama compiler for the playground (#1078): `okkama check`, `fmt`, `tokens`, `ast`, `ir`, the Sparrow
//! assembly of `okkama build`, and the editor's hover, completion, definition and signature help, over a std
//! bundled into the crate. Nothing runs here: the playground's runtime is sparrow-wasm, a module of its own (#1125);
//! [`Workspace::build`] makes the SPB it runs and keeps the module, so that [`Workspace::failure_text`] draws an
//! uncaught failure's stack in the sources, as `okkama run` does.
//!
//! Every call takes one JSON request and returns one JSON response (shapes in `docs/map/decisions.md`, #1078).
//! The text a command prints comes from `okkama_compiler::commands`, the CLI's own, so the playground and
//! `okkama` report the same thing; imports resolve through [`Workspace`], a loader over the bundled std and the
//! files the page adds, never a file system or the environment. The wasm build exports the same functions to
//! JavaScript (`wasm-bindgen`); natively they are plain Rust, which is how `okkama-cli/tests/playground.rs`
//! compares them with the CLI and the language server.
//!
//! A compiler panic is a trap on wasm32 (`RuntimeError: unreachable`), which carries no message: the panic
//! hook keeps the message and location of the last one for [`last_panic`] (#1118).

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use okkama_compiler::commands::{self, Finding, Report};
use okkama_compiler::ide::{
    self, Completion, CompletionKind, Inspection, LineIndex, Position, Severity, Stamp, StdCompletion, StdKind, StdTree,
};
use okkama_compiler::lowering::{self, LoadedModule, ModuleLoader};
use okkama_compiler::source::SourceFile;
use okkama_compiler::syntax::ParseMode;
use okkama_compiler::syntax::print::{Format, PrintOptions};
use okkama_runtime_core::failure::{Site, Uncaught};
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
/// directories (`OKKAMA_IMPORT_DIRS`). It keeps the last analysed inspection of each file (`ide::Answering`, the
/// language server's rule, #1128), so completion works while the text being typed does not parse or resolve.
#[derive(Debug)]
pub struct Workspace {
    std_root: String,
    std: HashMap<String, &'static str>,
    files: BTreeMap<String, String>,
    import_dirs: Vec<String>,
    answers: RefCell<HashMap<String, ide::Answering>>,
    built: RefCell<Option<Built>>,
}

/// The last [`Workspace::build`]: the SPB and the module its stacks are placed by.
struct Built {
    module: okkama_compiler::ir::Module,
    spb: Vec<u8>,
}

impl std::fmt::Debug for Built {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Built").field("spb", &self.spb.len()).finish_non_exhaustive()
    }
}


impl Default for Workspace {
    fn default() -> Self {
        Self::new(STD_ROOT)
    }
}

impl Workspace {
    /// The bundled std under `std_root`: [`STD_ROOT`] in the playground ([`Workspace::default`]); the native
    /// parity test (`okkama-cli/tests/playground_run.rs`) passes std's path as the CLI in the case's directory
    /// names it, so file names in diagnostics and stacks are the CLI's.
    pub fn new(std_root: &str) -> Self {
        let std_root = normalize(std_root);
        let std = bundle::FILES.iter().map(|(path, text)| (join(&std_root, path), *text)).collect();
        Self { std_root, std, files: BTreeMap::new(), import_dirs: Vec::new(), answers: RefCell::default(), built: RefCell::default() }
    }

    /// The page's own files besides the one being checked, which it may import (`path` → text).
    pub fn set_files(&mut self, files: impl IntoIterator<Item = (String, String)>) {
        self.files = files.into_iter().map(|(path, text)| (normalize(&path), text)).collect();
        self.answers.borrow_mut().clear();
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

    /// `okkama build` for a run (#1125): request `{source, path, tests?}`; the warnings on `stderr` (or the
    /// diagnostics, `ok` false), the size of the SPB in `spbBytes`. The workspace keeps the SPB ([`Self::built_spb`])
    /// and the module; a failed build forgets the previous one.
    pub fn build(&self, request: &Value) -> Value {
        self.built.replace(None);
        let source = match self.source(request) {
            Ok(source) => source,
            Err(response) => return response,
        };
        let name = source.name().rsplit('/').next().unwrap_or_default();
        let name = name.strip_suffix(".ok").unwrap_or(name);
        let compiled = commands::compile(std::slice::from_ref(&source), flag(request, "tests"), &mut |test_mode| self.loader(test_mode), name);
        let (artifact, module, warnings) = match compiled {
            Ok(compiled) => compiled,
            Err(report) => return self.failure(report, &source),
        };
        let spb = artifact.main().to_vec();
        let mut response = self.outcome(String::new(), warnings, &source);
        response["spbBytes"] = json!(spb.len());
        self.built.replace(Some(Built { module, spb }));
        response
    }

    /// The SPB of the last successful [`Self::build`]; empty without one.
    pub fn built_spb(&self) -> Vec<u8> {
        self.built.borrow().as_ref().map(|built| built.spb.clone()).unwrap_or_default()
    }

    /// The text `okkama run` prints for a failed run of the last build (#1125): request `{report, uncaught?,
    /// cwd?}`, where `report` and `uncaught` are what sparrow-wasm's `run` answered (streamed or not, #1134) and
    /// `cwd` is the directory file names are shown relative to; `{ok, text}`: the report with the failure's place
    /// and stack in the sources (`commands::trace::render_trace`).
    pub fn failure_text(&self, request: &Value) -> Value {
        let report = text_field(request, "report").unwrap_or_default();
        let uncaught = match request.get("uncaught") {
            None | Some(Value::Null) => return json!({"ok": true, "text": report}),
            Some(described) => match uncaught_from(described) {
                Some(uncaught) => uncaught,
                None => return usage("`uncaught` is not a failure sparrow-wasm described"),
            },
        };
        let built = self.built.borrow();
        let Some(built) = built.as_ref() else { return usage("nothing was built") };
        let cwd = text_field(request, "cwd");
        let text = uncaught.render_report(report, |sites| commands::trace::render_trace(&built.module, sites, cwd));
        json!({"ok": true, "text": text})
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
    /// documentation, insert, snippet}]}`, in the order to show them, as the language server answers: a
    /// `std.…` path completes from the std (#1115), current level first; after `(` and `,` only argument names;
    /// `"` and `/` open completion for import paths only, which the playground does not offer (no packages).
    pub fn complete(&self, request: &Value) -> Value {
        let trigger = text_field(request, "trigger");
        if matches!(trigger, Some("\"" | "/")) {
            return json!({"ok": true, "items": []});
        }
        if let Some(found) = self.std_path(request) {
            return json!({"ok": true, "items": found});
        }
        let Some((source, offset, text, inspection)) = self.at(request) else { return json!({"ok": true, "items": []}) };
        let found = if matches!(trigger, Some("(" | ",")) {
            inspection.argument_names(&text, source.text(), offset)
        } else {
            inspection.complete(&text, source.text(), offset)
        };
        json!({"ok": true, "items": found.iter().map(completion).collect::<Vec<_>>()})
    }

    /// The completion of a `std.…` path at the request's position, from the text as it is (it need not parse):
    /// `None` when the position is not in one.
    fn std_path(&self, request: &Value) -> Option<Vec<Value>> {
        let source = self.source(request).ok()?;
        let line = request.get("line")?.as_u64()? as u32;
        let character = request.get("character")?.as_u64()? as u32;
        let offset = LineIndex::new(source.text()).offset(Position { line, character });
        let mut found = ide::std_completions(source.text(), offset, &BundledStd { workspace: self })?;
        // The language server's order: the current level, then whole paths, each by label.
        found.sort_by(|a, b| a.deep.cmp(&b.deep).then_with(|| a.label.cmp(&b.label)));
        Some(found.iter().map(std_completion).collect())
    }

    /// The request's file, its offset of `line`/`character`, and the inspection that answers about it, with the
    /// text it inspected.
    fn at(&self, request: &Value) -> Option<(SourceFile, u32, Rc<str>, Rc<Inspection>)> {
        let source = self.source(request).ok()?;
        let line = request.get("line")?.as_u64()? as u32;
        let character = request.get("character")?.as_u64()? as u32;
        let offset = LineIndex::new(source.text()).offset(Position { line, character });
        let inspection = Rc::new(commands::inspect(&source, &mut |test_mode| self.loader(test_mode)));
        let mut answers = self.answers.borrow_mut();
        // The language server's rule (#1128): the last analysed inspection answers.
        let answers = answers.entry(source.name().to_string()).or_default();
        answers.record(&Rc::from(source.text()), &inspection);
        let (text, inspection) = answers.answering()?;
        Some((source, offset, Rc::clone(text), Rc::clone(inspection)))
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
        // The page's names are relative to its root, the playground's current directory (#1119).
        commands::module(found.clone(), found, text, self.test_mode).map(Some)
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

/// The bundled std as `std.…` completion reads it (#1115); the page's files under the std root come in front of
/// it, as imports see them, like the editor's unsaved texts. The bundle never changes, so its index is built once.
#[derive(Debug)]
struct BundledStd<'w> {
    workspace: &'w Workspace,
}

impl BundledStd<'_> {
    fn file(&self, module: &str) -> String {
        join(&self.workspace.std_root, &format!("{module}.ok"))
    }
}

impl StdTree for BundledStd<'_> {
    fn key(&self) -> String {
        format!("bundled:{}", self.workspace.std_root)
    }

    fn modules(&self) -> Vec<String> {
        bundle::FILES.iter().filter_map(|(path, _)| path.strip_suffix(".ok")).map(str::to_string).collect()
    }

    fn stamp(&self, _modules: &[String]) -> Option<Stamp> {
        Some(Stamp::default())
    }

    fn entries(&self, folder: &[String]) -> Vec<(String, bool)> {
        let prefix = if folder.is_empty() { String::new() } else { format!("{}/", folder.join("/")) };
        self.modules()
            .iter()
            .filter_map(|module| module.strip_prefix(prefix.as_str()))
            .map(|rest| match rest.split_once('/') {
                Some((folder, _)) => (folder.to_string(), true),
                None => (rest.to_string(), false),
            })
            .collect()
    }

    fn source(&self, module: &str) -> Option<SourceFile> {
        let file = self.file(module);
        let text = self.workspace.std.get(&file)?.to_string();
        SourceFile::new(file, text).ok()
    }

    fn unsaved(&self, module: &str) -> Option<SourceFile> {
        let file = self.file(module);
        let text = self.workspace.files.get(&file)?.clone();
        SourceFile::new(file, text).ok()
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

/// sparrow-wasm's description of an uncaught failure (its `uncaught_json`): stacks as runs `[function, pc, count]`.
fn uncaught_from(value: &Value) -> Option<Uncaught> {
    fn sites(value: &Value) -> Option<Vec<Site>> {
        let mut sites = Vec::new();
        for run in value.as_array()? {
            let [function, pc, count] = run.as_array()?.as_slice() else { return None };
            let site = Site { function: u32::try_from(function.as_u64()?).ok()?, pc: u32::try_from(pc.as_u64()?).ok()? };
            sites.extend(std::iter::repeat_n(site, usize::try_from(count.as_u64()?).ok()?));
        }
        Some(sites)
    }
    let mut cause_traces = Vec::new();
    for cause in value.get("causeTraces")?.as_array()? {
        let [offset, trace, omitted] = cause.as_array()?.as_slice() else { return None };
        cause_traces.push((usize::try_from(offset.as_u64()?).ok()?, sites(trace)?, omitted.as_u64()?));
    }
    Some(Uncaught {
        category: Uncaught::category_named(value.get("category")?.as_str()?)?,
        error_type: value.get("errorType").and_then(Value::as_str).map(str::to_string),
        message: value.get("message")?.as_str()?.to_string(),
        trace: sites(value.get("trace")?)?,
        trace_omitted: value.get("traceOmitted")?.as_u64()?,
        trace_offset: usize::try_from(value.get("traceOffset")?.as_u64()?).ok()?,
        cause_traces,
    })
}

fn range(range: ide::Range) -> Value {
    json!({
        "start": {"line": range.start.line, "character": range.start.character},
        "end": {"line": range.end.line, "character": range.end.character},
    })
}

fn std_completion(found: &StdCompletion) -> Value {
    let kind = match found.kind {
        StdKind::Folder => "folder",
        StdKind::Module => "module",
        StdKind::Object => "object",
        StdKind::Role => "role",
        StdKind::Enum => "enum",
        StdKind::EnumCase => "enumMember",
        StdKind::Native => "function",
    };
    json!({
        "label": found.label, "kind": kind, "detail": found.detail, "documentation": found.documentation,
        "insert": found.label, "snippet": false,
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
    /// The message and location of the last call's panic (#1118).
    static LAST_PANIC: RefCell<Option<(String, Option<String>)>> = const { RefCell::new(None) };
}

/// Keep every panic's message and location for [`last_panic`], then run the hook that was there before (natively,
/// the one that prints it). Installed once, by the first call.
pub fn keep_panics() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let payload = info.payload();
            let message = payload
                .downcast_ref::<&str>()
                .map(|text| text.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "a panic without a message".to_string());
            let location = info.location().map(|at| format!("{}:{}:{}", at.file(), at.line(), at.column()));
            // A panic inside the hook's own borrow is left alone rather than aborting twice.
            let _ = LAST_PANIC.try_with(|last| {
                if let Ok(mut last) = last.try_borrow_mut() {
                    *last = Some((message, location));
                }
            });
            previous(info);
        }));
    });
}

/// The last call's panic: `{panic: {message, location} | null}`; `location` is `file:line:column` or null. The
/// page reads it from the trapped instance before it starts a new one (#1118).
pub fn last_panic() -> String {
    let panic = LAST_PANIC.with(|last| {
        last.borrow().as_ref().map(|(message, location)| json!({"message": message, "location": location}))
    });
    json!({"panic": panic}).to_string()
}

fn forget_panic() {
    keep_panics();
    LAST_PANIC.with(|last| *last.borrow_mut() = None);
}

/// Parse `request`, run `command` on the playground's workspace, and print the response.
pub fn handle(request: &str, command: fn(&Workspace, &Value) -> Value) -> String {
    forget_panic();
    let response = match serde_json::from_str::<Value>(request) {
        Ok(request) => WORKSPACE.with(|workspace| command(&workspace.borrow(), &request)),
        Err(error) => usage(&format!("the request is not JSON: {error}")),
    };
    response.to_string()
}

/// The page's own files: request `{files: {path: text}, importDirs?: [dir]}`; replaces the previous set.
pub fn set_files(request: &str) -> String {
    forget_panic();
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

/// The SPB of the last successful `build` (#1125), for sparrow-wasm's `run`; empty without one.
pub fn built_spb() -> Vec<u8> {
    forget_panic();
    WORKSPACE.with(|workspace| workspace.borrow().built_spb())
}

/// The compiler's version, the bundled std and the IR passes in their order (`okkama ir --passes`, #1118):
/// `{version, stdModules, stdBytes, passes}`.
pub fn version() -> String {
    keep_panics();
    let bytes: usize = bundle::FILES.iter().map(|(_, text)| text.len()).sum();
    json!({
        "version": env!("CARGO_PKG_VERSION"), "stdModules": bundle::FILES.len(), "stdBytes": bytes,
        "passes": lowering::PASSES,
    })
    .to_string()
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

    #[wasm_bindgen(js_name = lastPanic)]
    pub fn last_panic() -> String {
        super::last_panic()
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
    pub fn build(request: &str) -> String {
        handle(request, Workspace::build)
    }

    #[wasm_bindgen(js_name = builtSpb)]
    pub fn built_spb() -> Vec<u8> {
        super::built_spb()
    }

    #[wasm_bindgen(js_name = failureText)]
    pub fn failure_text(request: &str) -> String {
        handle(request, Workspace::failure_text)
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

    fn labels(response: &Value) -> Vec<(String, String)> {
        let items = response["items"].as_array().unwrap();
        items.iter().map(|item| (item["label"].as_str().unwrap().to_string(), item["kind"].as_str().unwrap().to_string())).collect()
    }

    /// #1115: a `std.…` path completes from the bundled std as the language server's does from the directory:
    /// folders and modules of the level first, by label, then whole paths; declarations of a named module; enum
    /// cases; a page file under the std root in front of the bundled one.
    #[test]
    fn std_paths_complete_from_the_bundled_std() {
        let mut workspace = Workspace::default();
        let at = |workspace: &Workspace, text: &str| {
            let line = text.lines().count() - 1;
            let character = text.lines().last().unwrap().encode_utf16().count();
            workspace.complete(&json!({"source": text, "line": line, "character": character, "trigger": "."}))
        };
        let root = labels(&at(&workspace, "module play;\nmain() is int { let x = std."));
        assert!(root.contains(&("io".into(), "folder".into())), "{root:?}");
        assert!(root.contains(&("compare".into(), "module".into())), "{root:?}");
        // The level (`csv` is a folder and a module), by label, then whole paths from the depth, by label.
        let deep = root.iter().position(|(label, _)| label.contains('.')).unwrap();
        assert!(root.contains(&("io.stdout".into(), "module".into())), "{root:?}");
        for part in [&root[..deep], &root[deep..]] {
            let labels: Vec<&String> = part.iter().map(|(label, _)| label).collect();
            let mut sorted = labels.clone();
            sorted.sort();
            assert_eq!(labels, sorted);
        }
        assert!(root[deep..].iter().all(|(label, _)| label.contains('.')), "{root:?}");
        let stdout = labels(&at(&workspace, "module play;\nmain() is int { std.io.stdout."));
        assert!(stdout.contains(&("Stdout".into(), "object".into())), "{stdout:?}");
        let order = labels(&at(&workspace, "module play;\nmain() is int { std.compare.Order."));
        assert!(order.contains(&("less".into(), "enumMember".into())), "{order:?}");
        // Not a std path: ordinary completion.
        let other = at(&workspace, "module play;\nmain() is int { let mystd = 1; mystd.");
        assert!(!labels(&other).iter().any(|(_, kind)| kind == "folder"), "{other}");
        // A page file under std/ is what imports and completion read.
        workspace.set_files([("std/compare.ok".to_string(), "module compare;\n\n// A page enum.\nenum Page = (one, two);\n".to_string())]);
        let page = labels(&at(&workspace, "module play;\nmain() is int { std.compare."));
        assert!(page.contains(&("Page".into(), "enum".into())) && !page.iter().any(|(label, _)| label == "Order"), "{page:?}");
        let deep = labels(&at(&workspace, "module play;\nmain() is int { std.Pag"));
        assert!(deep.contains(&("compare.Page".into(), "enum".into())), "{deep:?}");
    }

    /// #1118: the version names the IR passes in their order, the one list `okkama ir --passes` takes.
    #[test]
    fn the_version_lists_the_ir_passes() {
        let version: Value = serde_json::from_str(&version()).unwrap();
        let passes: Vec<&str> = version["passes"].as_array().unwrap().iter().map(|pass| pass.as_str().unwrap()).collect();
        assert_eq!(passes, lowering::PASSES);
        let after = Workspace::default().ir(&json!({"source": "module play;\nmain() is int { 0; }\n", "after": passes[passes.len() - 1]}));
        assert_eq!(after["ok"], true, "{after}");
    }

    /// #1118: a panic's message and location are kept for `lastPanic`, and the next call forgets them.
    #[test]
    fn a_panic_is_kept_until_the_next_call() {
        keep_panics();
        let caught = std::panic::catch_unwind(|| panic!("the compiler fell over at {}", 42));
        assert!(caught.is_err());
        let panic: Value = serde_json::from_str(&last_panic()).unwrap();
        assert_eq!(panic["panic"]["message"], "the compiler fell over at 42", "{panic}");
        assert!(panic["panic"]["location"].as_str().is_some_and(|at| at.contains("lib.rs:")), "{panic}");
        let _ = std::panic::catch_unwind(|| std::panic::panic_any("static text"));
        assert_eq!(serde_json::from_str::<Value>(&last_panic()).unwrap()["panic"]["message"], "static text");
        handle("{\"source\": \"module play;\"}", Workspace::fmt);
        assert_eq!(last_panic(), "{\"panic\":null}");
    }
}
