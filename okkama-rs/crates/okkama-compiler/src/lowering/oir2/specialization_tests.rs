use crate::analysis;
use crate::oir2 as ir;
use crate::syntax::{self, ParseOptions};
use crate::SourceFile;
use super::{LowerError, lower_to_oir2};

fn with_analysis<T>(text: &str, test: impl FnOnce(&mut analysis::Analysis<'_>) -> T) -> T {
    let source = SourceFile::new("oir2.ok", text).unwrap();
    let program = syntax::parse(&source, &ParseOptions::default()).unwrap();
    let mut analysis = analysis::analyze(vec![analysis::SourceModule {
        path: "app".into(), program: &program, imports: vec![],
    }]).unwrap_or_else(|errors| panic!("analysis: {errors:?}"));
    test(&mut analysis)
}

fn print_oir2(source: &str) -> Result<String, LowerError> {
    with_analysis(source, |analysis| {
        let module = lower_to_oir2(analysis)?;
        ir::verify::verify_high(&module).unwrap();
        let text = ir::text::print(&module).unwrap();
        let parsed = ir::text::parse(&text).unwrap();
        ir::verify::verify_high(&parsed).unwrap();
        assert!(ir::text::normalized_eq(&module, &parsed).unwrap());
        Ok(text)
    })
}

const BOX: &str = include_str!("fixtures/p2_3/generic-object.ok");

#[test]
fn oir2_generic_object_two_substitutions() {
    let text = print_oir2(BOX).expect("two concrete object instances");
    assert!(text.contains("Box<int>"));
    assert!(text.contains("Box<string>"));
    assert_eq!(text, print_oir2(BOX).unwrap());
}

#[test]
fn oir2_generic_enum_two_payload_layouts() {
    let source = concat!("module app; enum Slot<T> = (empty, full(value is T)); ",
        "main() is int { let a = Slot.full(1); let b = Slot.full(\"s\"); 0; }");
    let text = print_oir2(source).expect("two concrete variant instances");
    assert!(text.contains("Slot<int>"));
    assert!(text.contains("Slot<string>"));
}

#[test]
fn oir2_generic_defaults_and_destroy() {
    let source = concat!("module app; object Box<T> { has value is T; has n is int = 7; ",
        "Box(:value, :n); get() is T { value; } destroy { value; } } ",
        "main() is int { let b = Box(value: 1); b.get(); }");
    print_oir2(source).expect("concrete default, shorthand omissions and destroy");
}

#[test]
fn oir2_generic_recursive_call_deduplicates() {
    let source = concat!("module app; object Box<T> { has value is T; Box(:value); ",
        "get(n is int) is T { if n == 0 { value; } else { self.get(n - 1); } } } ",
        "main() is int { Box(value: 1).get(3); }");
    with_analysis(source, |analysis| {
        let module = lower_to_oir2(analysis).expect("reserved recursive function");
        assert_eq!(module.functions.iter().filter(|(_, f)| f.symbol.contains(".get")).count(), 1);
    });
    print_oir2(source).unwrap();
}

#[test]
fn oir2_generic_recursive_type() {
    let source = concat!("module app; object Node<T> { has value is T; has tail is Maybe<Node<T>>; ",
        "Node(value is T) { self.value = value; tail = Maybe<Node<T>>.none; } ",
        "get() is T { value; } } main() is int { Node(1).get(); }");
    print_oir2(source).expect("recursive nominal layout");
}

#[test]
fn oir2_generic_payload_get_uses_concrete_case() {
    let source = concat!("module app; enum Slot<T> = (empty, full(value is T)); ",
        "main() is int { let a = Slot.full(7); ",
        "match a { case Slot.empty { 0; } case Slot.full(v) { v; } } }");
    print_oir2(source).expect("concrete payload selection");
}

#[test]
fn oir2_generic_growth_limit_has_request_chain() {
    let source = concat!("module app; object Box<T> { has value is T; Box(:value); ",
        "grow() is int { Box(value: self).grow(); } } main() is int { Box(value: 1).grow(); }");
    with_analysis(source, |analysis| {
        let limits = super::specialize::Limits { depth: 8, count: 12 };
        let error = super::specialize::prepare_with_limits(analysis, limits).unwrap_err();
        let text = error.to_string();
        assert!(text.contains("specialization limit"), "{text}");
        assert!(text.contains("main ->"), "{text}");
        assert!(text.contains("Box<int>"), "{text}");
        assert!(text.contains(".grow"), "{text}");
    });
}

#[test]
fn oir2_generic_constraints_rechecked_on_concrete_arguments() {
    with_analysis(BOX, |analysis| {
        let param = analysis.registry.objects.iter().find(|object| object.name == "Box").unwrap().params[0];
        analysis.registry.params[param.0].bound = Some(analysis::ty::Ty::role(analysis.registry.system.error));
        let error = lower_to_oir2(analysis).unwrap_err();
        assert!(error.to_string().contains("specialization bound"), "{error}");
        compare_fixture("generic-bound", "txt", &format!("{error}\n"));
    });
    let source = concat!("module app; object R { R(); destroy { } } ",
        "object Box<T> { has value is T; Box(:value); } main() is int { Box(value: R()); 0; }");
    with_analysis(source, |analysis| {
        let param = analysis.registry.objects.iter().find(|object| object.name == "Box").unwrap().params[0];
        analysis.registry.params[param.0].key = true;
        let error = lower_to_oir2(analysis).unwrap_err();
        assert!(error.to_string().contains("specialization key constraint"), "{error}");
        compare_fixture("generic-key-constraint", "txt", &format!("{error}\n"));
    });
}

const GOLDENS: [&str; 7] = [
    "generic-object", "generic-enum", "generic-action", "generic-recursive",
    "generic-recursion-dedup", "generic-destroy", "generic-defaults",
];

fn compare_fixture(name: &str, extension: &str, actual: &str) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/lowering/oir2/goldens/p2_3").join(format!("{name}.{extension}"));
    if std::env::var_os("OKKAMA_OIR2_P2_WRITE_GOLDENS").is_some() {
        std::fs::write(&path, actual).unwrap();
    }
    let expected = std::fs::read_to_string(&path).unwrap();
    if extension == "oir" {
        let stored = ir::text::parse(&expected).unwrap();
        ir::verify::verify_high(&stored).unwrap();
    }
    assert_eq!(actual, expected, "{name}");
}

#[test]
fn oir2_p2_3_specialization_goldens_and_determinism() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lowering/oir2/fixtures/p2_3");
    for name in GOLDENS {
        let source = std::fs::read_to_string(directory.join(format!("{name}.ok"))).unwrap();
        with_analysis(&source, |analysis| {
            let first = lower_to_oir2(analysis).unwrap_or_else(|error| panic!("{name}: {error}"));
            let second = lower_to_oir2(analysis).unwrap();
            ir::verify::verify_high(&first).unwrap();
            ir::verify::verify_high(&second).unwrap();
            assert!(first.functions.iter().all(|(_, function)| function.type_params.is_empty()));
            assert!(first.declarations.iter().all(|(_, declaration)| declaration.type_params.is_empty()));
            assert!(first.types.iter().all(|(_, ty)| !matches!(ty, ir::Type::Parameter(_))));
            assert_eq!(ir::text::print(&first).unwrap(), ir::text::print(&second).unwrap(), "{name}");
        });
        let actual = print_oir2(&source).unwrap();
        compare_fixture(name, "oir", &actual);
    }
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lowering/oir2/goldens/p2_3");
    assert_eq!(std::fs::read_dir(directory).unwrap().filter(|entry| {
        entry.as_ref().unwrap().path().extension().is_some_and(|extension| extension == "oir")
    }).count(), GOLDENS.len());
}

#[test]
fn oir2_p2_3_limit_diagnostic_goldens() {
    with_analysis(include_str!("fixtures/p2_3/generic-growth.ok"), |analysis| {
        for (name, limits) in [
            ("generic-growth-count", super::specialize::Limits { depth: 256, count: 12 }),
            ("generic-growth-depth", super::specialize::Limits { depth: 5, count: 1 << 18 }),
        ] {
            let error = super::specialize::prepare_with_limits(analysis, limits).unwrap_err();
            compare_fixture(name, "txt", &format!("{error}\n"));
        }
    });
}

#[test]
fn oir2_generic_imports_and_structural_names() {
    let library = SourceFile::new("library.ok", concat!("module library; ",
        "object Box<T> { has value is T; Box(:value); get() is T { value; } }")).unwrap();
    let program = syntax::parse(&library, &ParseOptions::default()).unwrap();
    let main = SourceFile::new("main.ok", concat!("module app; import \"library\" take Box; ",
        "main() is int { Box(value: 4).get(); }")).unwrap();
    let root = syntax::parse(&main, &ParseOptions::default()).unwrap();
    let analysis = analysis::analyze(vec![
        analysis::SourceModule { path: "library".into(), program: &program, imports: vec![] },
        analysis::SourceModule { path: "app".into(), program: &root, imports: vec![(0, &root.imports[0])] },
    ]).unwrap();
    let module = lower_to_oir2(&analysis).unwrap();
    ir::verify::verify_high(&module).unwrap();
    let text = ir::text::print(&module).unwrap();
    let parsed = ir::text::parse(&text).unwrap();
    ir::verify::verify_high(&parsed).unwrap();
    assert!(text.contains("library::Box<int>.get#overload()"));

    let names = |source: &str| with_analysis(source, |analysis| {
        let module = lower_to_oir2(analysis).unwrap();
        module.functions.iter().filter(|(_, function)| function.symbol.contains("Box<"))
            .map(|(_, function)| function.symbol.clone()).collect::<Vec<_>>()
    });
    let shifted = BOX.replace("module app;", "module app; object Earlier { Earlier(); }");
    assert_eq!(names(BOX), names(&shifted));
}

#[test]
fn oir2_generic_open_types_are_explicit_diagnostics() {
    with_analysis(BOX, |analysis| {
        let param = analysis.registry.objects.iter().find(|object| object.name == "Box").unwrap().params[0];
        let body = analysis.bodies.iter_mut().find(|body| body.owner == analysis::thir::BodyOwner::Entry).unwrap();
        body.locals[0].ty = analysis::ty::Ty::Param(param);
        let error = lower_to_oir2(analysis).unwrap_err();
        assert!(matches!(error, LowerError::Unsupported { span, .. } if !span.is_empty()));
        assert!(error.to_string().contains("not closed after substitution"));
    });
}

#[test]
fn oir2_generic_later_constructs_keep_source_diagnostics() {
    for source in [
        concat!("module app; object Box<T> { has value is T; Box(:value); } ",
            "main() is int { Box(value: () is int { 1; }); 0; }"),
        concat!("module app; object Box<T> { has value is T; Box(:value); } ",
            "main() is int { let values = [Box(value: 1)].map() -> x { x; }; 0; }"),
        concat!("module app; object Box<T> { has value is T; Box(:value); ",
            "async get() is T { value; } } main() is int { Box(value: 1).get(); 0; }"),
    ] {
        with_analysis(source, |analysis| {
            let error = lower_to_oir2(analysis).unwrap_err();
            assert!(matches!(error, LowerError::Unsupported { span, .. } if !span.is_empty()), "{error}");
        });
    }
}
