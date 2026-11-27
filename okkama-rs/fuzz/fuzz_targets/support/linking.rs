//! The program the exec target runs: the source linked as `okkama build`
//! links it (#1089), with the repository's std tree as its only modules
//! (#1091), and run on Sparrow under the target's budgets. Unlinked OIR
//! (`lowering::lower_program`) keeps its imports unresolved by contract, and
//! codegen refuses such a module; linking resolves an import of std and
//! refuses any other with the loader's diagnostic. Whatever linking accepts,
//! codegen must generate.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use okkama_compiler::{SourceFile, ir, lowering::{self, LinkError, LoadedModule, ModuleLoader}, syntax::{self, ParseOptions, ast, prune}};
use sparrow_core::budget::Limits;

/// A std module as linking takes it (test items stripped), or why it does not parse.
type Parsed = Result<(SourceFile, ast::Program), String>;

/// The std tree of the repository this crate is built in, canonical.
fn std_root() -> Option<&'static Path> {
    static ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();
    ROOT.get_or_init(|| {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .map(|dir| dir.join("std"))
            .find(|std| std.join("async/atomic.ok").is_file())
            .and_then(|std| std.canonicalize().ok())
    })
    .as_deref()
}

/// The file of std module `path`; `None` for a path that is not a plain std
/// module name (no `.`/`..` segments, no absolute or host paths) or no such file.
fn std_file(path: &str) -> Option<PathBuf> {
    let path = path.strip_suffix(".ok").unwrap_or(path);
    let plain = |segment: &str| {
        !segment.is_empty() && segment.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
    };
    if !path.split('/').all(plain) {
        return None;
    }
    let file = std_root()?.join(format!("{path}.ok"));
    file.is_file().then_some(file)
}

/// Std module `path`, read and parsed once per process (#1091): a fuzz run
/// links many programs against the same few modules.
fn std_module(path: &str) -> Option<Parsed> {
    static MODULES: OnceLock<Mutex<HashMap<PathBuf, Parsed>>> = OnceLock::new();
    let file = std_file(path)?;
    let mut modules = MODULES.get_or_init(Default::default).lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let parsed = modules.entry(file.clone()).or_insert_with(|| {
        let name = file.display().to_string();
        let text = std::fs::read_to_string(&file).map_err(|error| format!("{name}: {error}"))?;
        let source = SourceFile::new(name.clone(), text).map_err(|error| format!("{name}: {error:?}"))?;
        let program = syntax::parse(&source, &ParseOptions::default()).map_err(|error| format!("{name}: {error:?}"))?;
        Ok((source, prune::strip_test_items(program)))
    });
    Some(parsed.clone())
}

/// A loader with the std tree as its only modules.
struct StdTree;

impl ModuleLoader for StdTree {
    fn load(&mut self, path: &str, _importer: &str) -> Result<Option<LoadedModule>, String> {
        match std_module(path) {
            None => Ok(None),
            Some(parsed) => {
                let (source, program) = parsed?;
                Ok(Some(LoadedModule { id: source.name().to_string(), source, program }))
            }
        }
    }

    fn resolve(&mut self, path: &str, _importer: &str) -> Result<Option<String>, String> {
        Ok(std_file(path).map(|file| file.display().to_string()))
    }

    fn resolve_std(&mut self, path: &str, importer: &str) -> Result<Option<String>, String> {
        self.resolve(path, importer)
    }
}

/// The linked module of `program` (test items already stripped), `None` for a
/// language or load error; an internal compiler error panics.
pub fn linked(program: &ast::Program) -> Option<ir::Module> {
    match lowering::lower_linked(program, "fuzz.ok", &mut StdTree) {
        Ok(module) => Some(module),
        Err(LinkError::Lowering { error, .. }) => crate::lowering::accepted(Err(error)),
        Err(LinkError::Load(_)) => None,
    }
}

/// The exec target's budgets: pure computation only, no files, network,
/// processes, environment or clocks. A memory budget needs the process's
/// memory probe (`sparrow_core::budget::set_memory_probe`).
pub fn limits() -> Limits {
    Limits {
        steps: Some(1_000_000),
        memory: Some(64 << 20),
        output: Some(4096),
        natives: Some(Vec::new()),
        ..Limits::default()
    }
}

/// The exec target on one input: `None` when it is not a program (not UTF-8,
/// a syntax, language or load error), else the run's outcome. A compiler
/// failure on a linked module panics.
pub fn run(data: &[u8]) -> Option<Result<i64, sparrow_core::Error>> {
    let bytes = spb(data)?;
    Some(sparrow_core::run_limited(bytes, Vec::new(), false, Vec::new(), limits()))
}

/// The SPB of a fuzz input, when it is a program.
fn spb(data: &[u8]) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(data).ok()?;
    let source = SourceFile::new("fuzz.ok", text).ok()?;
    let program = syntax::parse(&source, &ParseOptions::default()).ok()?;
    let module = linked(&prune::strip_test_items(program))?;
    let program = okkama_compiler::codegen::sparrow::generate(&module).expect("codegen of linked OIR");
    Some(okkama_sparrow::encode(&program).expect("encoding generated SPB"))
}

/// The exec target's differential mode (#1123, `OKKAMA_FUZZ_DIFFERENTIAL=1`): the program runs on Sparrow's JIT
/// and on its interpreter under the same budgets, and the two must show the same result or uncaught report (as a
/// structure) and the same output, or the input crashes the target. The design's known divergences
/// (`okkama-rs/tools/engines/interp-exceptions.txt`) pass:
/// a stack overflow reached on both (its depth is each engine's), a memory budget run out on either (its moment is
/// each engine's: the interpreter's register stack is allocated memory), and the JIT's compile limits. Returns what was compared, `None` when the input is not a program.
pub fn differ(data: &[u8]) -> Option<Difference> {
    use sparrow_core::{Engine, observe};
    let bytes = spb(data)?;
    let jit = observe(Engine::Jit, bytes.clone(), Vec::new(), limits()).expect("generated SPB loads");
    let interpreted = observe(Engine::Interpreter, bytes, Vec::new(), limits()).expect("generated SPB loads");
    let report = |observed: &sparrow_core::Observed| match &observed.result {
        Err((report, _)) => report.clone(),
        Ok(_) => String::new(),
    };
    let (jit_report, interpreted_report) = (report(&jit), report(&interpreted));
    if ["compiling it would take more than", "too large to compile", "KiB limit"].iter().any(|text| jit_report.contains(text)) {
        return Some(Difference::Excepted("the JIT's compile limits (design 6.3)"));
    }
    if jit_report.contains("Stack overflow") && interpreted_report.contains("Stack overflow") {
        return Some(Difference::Excepted("a stack overflow reached on both, at each engine's depth (design 6.3)"));
    }
    let memory = "Execution budget exhausted: memory";
    if jit_report.contains(memory) || interpreted_report.contains(memory) {
        return Some(Difference::Excepted("the memory budget, run out at each engine's moment (design 6.3)"));
    }
    assert_eq!(interpreted, jit, "the interpreter differs from the JIT");
    Some(Difference::Same)
}

/// What [`differ`] found.
#[derive(Debug, PartialEq, Eq)]
pub enum Difference {
    Same,
    /// A known divergence of the design, with its reason.
    Excepted(&'static str),
}
