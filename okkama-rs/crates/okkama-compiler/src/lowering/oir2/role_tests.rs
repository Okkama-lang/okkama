//! C3 role producer contracts.
use crate::analysis::{self, ty::Ty};
use crate::oir2 as ir;
use crate::syntax::{self, ParseOptions};
use crate::SourceFile;

#[test]
fn oir2_role_successor_is_resolved_by_frontend() {
    let source = SourceFile::new("role-successor.ok", include_str!("fixtures/p2_3/role-successor.ok"))
        .unwrap();
    let program = syntax::parse(&source, &ParseOptions::default()).unwrap();
    let analysis = analysis::analyze(vec![analysis::SourceModule {
        path: "app".into(),
        program: &program,
        imports: vec![],
    }]).unwrap();
    let role = analysis.registry.roles.iter().find(|role| role.name == "Package").unwrap();
    assert!(matches!(&role.methods[0].result, Some(Ty::Successor(inner)) if matches!(**inner, Ty::Role(..))));
    let object = analysis.registry.objects.iter().find(|object| object.name == "Box").unwrap();
    assert!(matches!(&object.methods[0].result,
        Some(Ty::Successor(inner)) if matches!(**inner, Ty::Object(..))));
    assert_eq!(object.does.len(), 1);
}

#[test]
fn oir2_role_successor_high_contract() {
    let ordinary = ir::text::parse("oir 2 high\nmodule app\nrole Package {\n    open() -> Package\n}\n")
        .unwrap();
    ir::verify::verify_high(&ordinary).unwrap();
    let successor = ir::text::parse("oir 2 high\nmodule app\nrole Package {\n    open() -> ^Package\n}\n")
        .unwrap();
    ir::verify::verify_high(&successor).unwrap();
}

#[test]
fn oir2_role_successor_lowering() {
    let source = SourceFile::new("role-successor.ok", include_str!("fixtures/p2_3/role-successor.ok")).unwrap();
    let program = syntax::parse(&source, &ParseOptions::default()).unwrap();
    let analysis = analysis::analyze(vec![analysis::SourceModule {
        path: "app".into(), program: &program, imports: vec![],
    }]).unwrap();
    let module = super::lower_to_oir2(&analysis).unwrap();
    ir::verify::verify_high(&module).unwrap();
}

const GOLDENS: &[&str] = &[
    "role-static", "role-dynamic", "role-two-implementations", "role-narrow-success",
    "role-narrow-failure", "role-successor", "role-generic", "role-return",
];

fn with_analysis<T>(source: &str, test: impl FnOnce(&analysis::Analysis<'_>) -> T) -> T {
    let source = SourceFile::new("oir2-role.ok", source).unwrap();
    let program = syntax::parse(&source, &ParseOptions::default()).unwrap();
    let analysis = analysis::analyze(vec![analysis::SourceModule {
        path: "app".into(), program: &program, imports: vec![],
    }]).unwrap_or_else(|errors| panic!("{errors:?}"));
    test(&analysis)
}

fn golden(name: &str, module: &ir::Module) {
    ir::verify::verify_high(module).unwrap_or_else(|error| panic!("{name}: {error:?}"));
    let actual = ir::text::print(module).unwrap();
    let parsed = ir::text::parse(&actual).unwrap();
    ir::verify::verify_high(&parsed).unwrap();
    assert!(ir::text::normalized_eq(module, &parsed).unwrap());
    compare_golden(name, "oir", &actual);
}

fn compare_golden(name: &str, extension: &str, actual: &str) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/lowering/oir2/goldens/p2_3/roles").join(format!("{name}.{extension}"));
    if std::env::var_os("OKKAMA_OIR2_P2_WRITE_GOLDENS").is_some() {
        std::fs::write(&path, actual).unwrap();
    }
    let expected = std::fs::read_to_string(path).unwrap();
    if extension == "oir" {
        ir::verify::verify_high(&ir::text::parse(&expected).unwrap()).unwrap();
    }
    assert_eq!(actual, expected, "{name}");
}

#[test]
fn oir2_c3_role_goldens_and_inventory() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lowering/oir2/fixtures/p2_3");
    for name in GOLDENS {
        let source = std::fs::read_to_string(directory.join(format!("{name}.ok"))).unwrap();
        with_analysis(&source, |analysis| {
            let first = super::lower_to_oir2(analysis).unwrap_or_else(|error| panic!("{name}: {error}"));
            let second = super::lower_to_oir2(analysis).unwrap();
            assert_eq!(ir::text::print(&first).unwrap(), ir::text::print(&second).unwrap(), "{name}");
            golden(name, &first);
            if *name == "role-generic" {
                let roles: Vec<_> = first.declarations.iter().filter_map(|(id, decl)| {
                    matches!(decl.kind, ir::DeclarationKind::Role { .. }).then_some(id)
                }).collect();
                assert_eq!(roles.len(), 2);
                assert_ne!(roles[0], roles[1]);
            }
            let mut targets = vec![];
            for (_, decl) in first.declarations.iter() {
                if let ir::DeclarationKind::Object { witnesses, .. } = &decl.kind {
                    for witness in witnesses {
                        let ir::Type::Role { decl: role, .. } = first.types.get(witness.role).unwrap() else {
                            panic!("witness role type");
                        };
                        let ir::DeclarationKind::Role { methods, .. } = &first.declarations.get(*role).unwrap().kind
                            else {
                                panic!("role identity");
                            };
                        assert_eq!(witness.slots.len(), methods.len());
                        targets.extend(witness.slots.iter().copied());
                    }
                }
            }
            assert!(!targets.is_empty(), "{name}");
            assert!(targets.iter().all(|id| first.functions.get(*id).unwrap().body.is_some()));
        });
    }
}

#[test]
fn oir2_c3_role_negatives() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lowering/oir2/fixtures/p2_3");
    for name in ["role-unknown-slot", "role-narrow-forbidden"] {
        let text = std::fs::read_to_string(directory.join(format!("{name}.ok"))).unwrap();
        let source = SourceFile::new(name, text).unwrap();
        let program = syntax::parse(&source, &ParseOptions::default()).unwrap();
        let errors = analysis::analyze(vec![analysis::SourceModule {
            path: "app".into(), program: &program, imports: vec![],
        }]).unwrap_err();
        let text = format!("{errors:?}");
        assert!(text.contains(if name == "role-unknown-slot" { "missing" } else { "Cannot initialize" }), "{text}");
        let diagnostic = errors.iter().map(|error| format!("frontend: {}\n", error.message)).collect::<String>();
        compare_golden(name, "txt", &diagnostic);
    }
    with_analysis(include_str!("fixtures/p2_3/role-no-implementation.ok"), |analysis| {
        let error = super::lower_to_oir2(analysis).unwrap_err();
        assert!(matches!(error, super::LowerError::Unsupported { .. }));
        assert!(error.to_string().contains("no reachable implementation"), "{error}");
        compare_golden("role-no-implementation", "txt", &format!("lowering: {error}\n"));
    });
}

#[test]
fn oir2_c3_builtin_byte_source_adapter() {
    let library = SourceFile::new("stream.ok", include_str!("fixtures/p2_3/role-builtin-stream.ok")).unwrap();
    let program = syntax::parse(&library, &ParseOptions::default()).unwrap();
    let source = SourceFile::new("app.ok", include_str!("fixtures/p2_3/role-builtin.ok")).unwrap();
    let root = syntax::parse(&source, &ParseOptions::default()).unwrap();
    let analysis = analysis::analyze(vec![
        analysis::SourceModule { path: "io/stream".into(), program: &program, imports: vec![] },
        analysis::SourceModule { path: "app".into(), program: &root, imports: vec![(0, &root.imports[0])] },
    ]).unwrap();
    let first = super::lower_to_oir2(&analysis).unwrap();
    let second = super::lower_to_oir2(&analysis).unwrap();
    assert_eq!(ir::text::print(&first).unwrap(), ir::text::print(&second).unwrap());
    golden("role-builtin", &first);
}

#[test]
fn oir2_c3_generic_evidence_and_role_fields() {
    for source in [
        concat!("module app; role Ranked<T> { beats(other is T) is bool; } ",
            "object Score does Ranked<Score> { has n is int; Score(:n); ",
            "beats(other is Score) is bool { n > other.value(); } value() is int { n; } } ",
            "object Best<T: Ranked<T>> { has a is T; has b is T; Best(:a, :b); ",
            "winner() is T { if a.beats(b) { a; } else { b; } } } ",
            "main() is int { Best(a: Score(n: 1), b: Score(n: 2)).winner().value(); }"),
        concat!("module app; role Named { name() is string; } ",
            "role Counted { name() is string; count() is int; } ",
            "object Tally does Named, Counted { Tally(); name() is string { \"n\"; } count() is int { 1; } } ",
            "object Tagged<T: Named> { has item is T; Tagged(:item); label() is string { item.name(); } } ",
            "main() is int { let counted is Counted = Tally(); Tagged(item: counted).label(); 0; }"),
        concat!("module app; role Echo<T> { echo(value is T) is T; } ",
            "object EchoBox<T> does Echo<T> { has seed is T; EchoBox(:seed); echo(value is T) is T { value; } } ",
            "object Wrapper<T> { has inner is Echo<T>; has sample is T; Wrapper(:inner, :sample); ",
            "run() is T { inner.echo(sample); } } ",
            "main() is int { Wrapper(inner: EchoBox(seed: 2), sample: 3).run(); }"),
    ] {
        with_analysis(source, |analysis| {
            let module = super::lower_to_oir2(analysis).unwrap();
            ir::verify::verify_high(&module).unwrap();
            let parsed = ir::text::parse(&ir::text::print(&module).unwrap()).unwrap();
            ir::verify::verify_high(&parsed).unwrap();
        });
    }
}

#[test]
fn oir2_c3_concrete_result_witness_adapter() {
    // The frontend currently requires the ordinary result to match exactly. Exercise
    // the producer's concrete-result adapter at its resolved THIR boundary.
    let source = SourceFile::new("role-return.ok", include_str!("fixtures/p2_3/role-return.ok")).unwrap();
    let program = syntax::parse(&source, &ParseOptions::default()).unwrap();
    let mut analysis = analysis::analyze(vec![analysis::SourceModule {
        path: "app".into(), program: &program, imports: vec![],
    }]).unwrap();
    let object = analysis.registry.objects.iter().position(|object| object.name == "Box").unwrap();
    let source = analysis.registry.objects.iter().position(|object| object.name == "Source").unwrap();
    analysis.registry.objects[source].methods[0].result = Some(Ty::object(analysis::ty::ObjectId(object)));
    let owner = analysis::thir::BodyOwner::Method { object: analysis::ty::ObjectId(source), method: 0 };
    let body = analysis.bodies.iter_mut().find(|body| body.owner == owner).unwrap();
    let analysis::thir::StmtKind::Expr(expr) = &mut body.block.stmts[0].kind else {
        panic!("return expression");
    };
    let analysis::thir::ExprKind::Coerce { value, .. } = &expr.kind else {
        panic!("concrete to role coercion");
    };
    *expr = (**value).clone();
    let module = super::lower_to_oir2(&analysis).unwrap();
    let second = super::lower_to_oir2(&analysis).unwrap();
    assert_eq!(ir::text::print(&module).unwrap(), ir::text::print(&second).unwrap());
    golden("role-concrete-return-adapter", &module);
}

#[test]
fn oir2_c3_golden_inventory_is_exact() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lowering/oir2/goldens/p2_3/roles");
    let mut names: Vec<_> = std::fs::read_dir(directory).unwrap().map(|entry| {
        entry.unwrap().file_name().into_string().unwrap()
    }).collect();
    names.sort();
    let mut expected: Vec<_> = GOLDENS.iter().map(|name| format!("{name}.oir")).collect();
    for name in ["role-builtin.oir", "role-concrete-return-adapter.oir", "role-unknown-slot.txt",
        "role-narrow-forbidden.txt", "role-no-implementation.txt"] {
        expected.push(name.into());
    }
    expected.sort();
    assert_eq!(names, expected);
}

#[test]
fn oir2_c3_growing_role_uses_specializer_limits() {
    let source = concat!("module app; role R<T> { next() is R<@T>; } ",
        "object Reader { Reader(); read(value is R<int>) is int { 0; } } main() is int { 0; }");
    with_analysis(source, |analysis| {
        let limits = super::specialize::Limits { depth: 8, count: 64 };
        let error = super::specialize::prepare_with_limits(analysis, limits).unwrap_err();
        assert!(error.to_string().contains("specialization limit"), "{error}");
        assert!(error.to_string().contains("app::R<int> -> app::R<array<int>>"), "{error}");
    });
}
