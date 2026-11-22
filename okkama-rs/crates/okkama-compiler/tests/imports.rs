//! The import graph (#51): a module that many modules import is read and
//! parsed once; loops are found in linear time.

use okkama_compiler::lowering::{self, LoadedModule, ModuleLoader, RootFile};
use okkama_compiler::source::SourceFile;
use okkama_compiler::syntax::{self, ParseOptions};
use std::collections::HashMap;

/// Modules from memory by import path; counts the modules it parses.
struct Counting {
    modules: HashMap<String, String>,
    parsed: HashMap<String, usize>,
    resolves: bool,
}

impl ModuleLoader for Counting {
    fn load(&mut self, path: &str, _importer: &str) -> Result<Option<LoadedModule>, String> {
        let Some(text) = self.modules.get(path) else {
            return Ok(None);
        };
        *self.parsed.entry(path.to_string()).or_default() += 1;
        let source = SourceFile::new(format!("/lib/{path}.ok"), text.clone()).unwrap();
        let program = syntax::parse(&source, &ParseOptions::default()).map_err(|_| "syntax".to_string())?;
        Ok(Some(LoadedModule { id: source.name().to_string(), program, source }))
    }

    fn resolve(&mut self, path: &str, _importer: &str) -> Result<Option<String>, String> {
        Ok((self.resolves && self.modules.contains_key(path)).then(|| format!("/lib/{path}.ok")))
    }
}

/// `main` imports `count` modules, and each of them imports `shared`.
fn graph(count: usize, resolves: bool) -> (Counting, String) {
    let mut modules = HashMap::from([(
        "shared".to_string(),
        "module shared v1.0.0;\nobject Shared {\n    has n is int;\n    Shared(:n);\n}\n".to_string(),
    )]);
    let mut root = String::from("module app v1.0.0;\n");
    for index in 0..count {
        modules.insert(
            format!("m{index}"),
            format!("module m{index} v1.0.0;\nimport \"shared\" take Shared;\nobject M{index} {{\n    has s is Shared;\n    M{index}(:s);\n}}\n"),
        );
        root.push_str(&format!("import \"m{index}\";\n"));
    }
    root.push_str("main() is int { 0; }\n");
    (Counting { modules, parsed: HashMap::new(), resolves }, root)
}

fn check(loader: &mut Counting, root: &str) {
    let source = SourceFile::new("/app/main.ok", root.to_string()).unwrap();
    let program = syntax::parse(&source, &ParseOptions::default()).unwrap();
    let checked = lowering::check_linked_files(&[RootFile { program: &program, source: &source }], loader);
    assert!(checked.is_ok(), "{:?}", checked.err());
}

#[test]
fn a_shared_import_is_parsed_once() {
    let (mut loader, root) = graph(256, true);
    check(&mut loader, &root);
    assert_eq!(loader.parsed["shared"], 1);
    assert!(loader.parsed.values().all(|&count| count == 1));
    // A loader that cannot resolve ids still links the same graph, parsing
    // the shared module once per importer.
    let (mut loader, root) = graph(8, false);
    check(&mut loader, &root);
    assert_eq!(loader.parsed["shared"], 8);
}

/// #472: inspecting an already parsed file gives what inspecting the text
/// gives. The full parse of a file without test items stands in for its
/// production parse; with test items it is the test pass's own parse.
#[test]
fn inspecting_a_parsed_file_matches_inspecting_its_text() {
    let modules = HashMap::from([(
        "shared".to_string(),
        "module shared v1.0.0;\nobject Shared {\n    has n is int;\n    Shared(:n);\n}\n".to_string(),
    )]);
    let cases = [
        ("module app v1.0.0;\nimport \"shared\" take Shared;\nmain() is int { let _s = Shared(n: 1); 0; }\n", false),
        ("module app v1.0.0;\nmain() is int { let x is int = \"s\"; 0; }\n", false),
        ("module app v1.0.0;\nmain() is int { 0; }\ntest \"t\" { let y is bool = 1; }\n", true),
    ];
    for (text, test_mode) in cases {
        let source = SourceFile::new("/app/main.ok", text).unwrap();
        let full = syntax::parse(&source, &ParseOptions::default()).expect("parses");
        let mut loader = Counting { modules: modules.clone(), parsed: HashMap::new(), resolves: true };
        let own = lowering::inspect(&source, &mut loader, test_mode);
        let mut loader = Counting { modules: modules.clone(), parsed: HashMap::new(), resolves: true };
        let given = lowering::inspect_parsed(&source, Some(full), &mut loader, test_mode);
        assert_eq!(given.parsed, own.parsed, "{text}");
        assert_eq!(given.diagnostics, own.diagnostics, "{text}");
        assert_eq!(format!("{:?}", given.outline), format!("{:?}", own.outline), "{text}");
        assert_eq!(own.diagnostics.is_empty(), text.contains("Shared(n: 1)"), "{text}: {:?}", own.diagnostics);
    }
}
