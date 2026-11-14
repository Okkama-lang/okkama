//! C4 source -> verified high fixtures. No ownership acceptance is claimed here.
use crate::analysis;
use crate::oir2 as ir;
use crate::syntax::{self, ParseOptions};
use crate::SourceFile;

pub(super) fn with_analysis<T>(text: &str, test: impl FnOnce(&analysis::Analysis<'_>) -> T) -> T {
    let source = SourceFile::new("oir2-collections.ok", text).unwrap();
    let program = syntax::parse(&source, &ParseOptions::default()).unwrap();
    let analysis = analysis::analyze(vec![analysis::SourceModule {
        path: "app".into(), program: &program, imports: vec![],
    }]).unwrap_or_else(|errors| panic!("{errors:?}"));
    test(&analysis)
}

fn golden(name: &str, module: &ir::Module) {
    ir::verify::verify_high(module).unwrap_or_else(|errors| panic!("{name}: {errors:?}"));
    let actual = ir::text::print(module).unwrap();
    let parsed = ir::text::parse(&actual).unwrap();
    ir::verify::verify_high(&parsed).unwrap();
    assert!(ir::text::normalized_eq(module, &parsed).unwrap());
    assert_eq!(actual, ir::text::print(&parsed).unwrap());
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/lowering/oir2/goldens/p2_3/collections").join(format!("{name}.oir"));
    if std::env::var_os("OKKAMA_OIR2_P2_WRITE_GOLDENS").is_some() {
        std::fs::write(&path, &actual).unwrap();
    }
    let expected = std::fs::read_to_string(path).unwrap();
    ir::verify::verify_high(&ir::text::parse(&expected).unwrap()).unwrap();
    assert_eq!(actual, expected, "{name}");
}

#[test]
fn oir2_c4_collection_goldens() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/lowering/oir2/fixtures/p2_3/collections");
    let mut sources: Vec<_> = std::fs::read_dir(directory).unwrap()
        .map(|entry| entry.unwrap().path()).collect();
    sources.sort();
    let mut names: Vec<_> = sources.iter().map(|path| path.file_stem().unwrap().to_str().unwrap()).collect();
    names.sort();
    assert_eq!(names, GOLDENS);
    for path in sources {
        let name = path.file_stem().unwrap().to_str().unwrap();
        let source = std::fs::read_to_string(&path).unwrap();
        with_analysis(&source, |analysis| {
            let first = super::lower_to_oir2(analysis).unwrap_or_else(|error| panic!("{name}: {error}"));
            golden(name, &first);
            let second = super::lower_to_oir2(analysis).unwrap();
            ir::verify::verify_high(&second).unwrap();
            assert_eq!(ir::text::print(&first).unwrap(), ir::text::print(&second).unwrap(), "{name}");
        });
    }
}

#[test]
fn oir2_c4_frontend_negatives() {
    let corpus = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/ownership-corpus/rejected/map-resource-key.ok");
    let resource_key = std::fs::read_to_string(corpus).unwrap();
    for (name, source, message) in [
        ("wrong-key", "module app; main() is int { let m = [1: 2]; m[\"x\"]; }", "key"),
        ("wrong-element", "module app; main() is int { let a is @int = [\"x\"]; 0; }", "Array element"),
        ("resource-key", resource_key.as_str(), "destroy"),
        ("byte-key", "module app; main() is int { let m = [byte(1): 2]; 0; }", "key"),
    ] {
        let source = SourceFile::new(name, source).unwrap();
        let program = syntax::parse(&source, &ParseOptions::default()).unwrap();
        let errors = analysis::analyze(vec![analysis::SourceModule {
            path: "app".into(), program: &program, imports: vec![],
        }]).unwrap_err();
        assert!(errors.iter().all(|error| !error.unsupported));
        let diagnostic = errors.iter().map(|error| format!("frontend: {}\n", error.message)).collect::<String>();
        assert!(diagnostic.contains(message), "{name}: {diagnostic}");
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/lowering/oir2/goldens/p2_3/collections").join(format!("{name}.txt"));
        if std::env::var_os("OKKAMA_OIR2_P2_WRITE_GOLDENS").is_some() {
            std::fs::write(&path, &diagnostic).unwrap();
        }
        assert_eq!(diagnostic, std::fs::read_to_string(path).unwrap());
    }
}

const GOLDENS: &[&str] = &[
    "array-ops",
    "bounds",
    "bounds-drop",
    "bounds-first",
    "bounds-last",
    "bounds-map",
    "bounds-remove",
    "bounds-replace",
    "bounds-take",
    "empty-results",
    "evaluation-order",
    "generic-collections",
    "large-literal",
    "literals",
    "lookup-lazy",
    "map-ops",
    "operators-empty-chain",
    "projection-control",
    "projection-index",
    "projection-keys",
    "projection-map",
    "projection-map-index",
    "projection-map-key-index",
    "projection-path",
    "projection-remove",
    "projection-slice",
    "projection-values",
    "range-expansion",
    "resource-projections",
    "resource-reversed",
    "role-elements",
    "scalar-keys",
    "search-contains",
    "search-index",
    "shape-drop",
    "shape-reversed",
    "shape-take",
];
