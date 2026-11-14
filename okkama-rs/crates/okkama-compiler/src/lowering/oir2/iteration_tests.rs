//! C6 small, independently verified traversal fixtures.
use crate::analysis::{self, thir};
use crate::oir2 as ir;
use crate::syntax::{self, ParseOptions};
use crate::SourceFile;

fn with_analysis(text: &str, test: impl FnOnce(&mut analysis::Analysis<'_>)) {
    let library = SourceFile::new("iteration.ok", include_str!("fixtures/p2_3/iteration-protocol.ok")).unwrap();
    let protocol = syntax::parse(&library, &ParseOptions::default()).unwrap();
    let source = SourceFile::new("traversal.ok", text).unwrap();
    let program = syntax::parse(&source, &ParseOptions::default())
        .unwrap_or_else(|errors| panic!("{text}\n{errors:?}"));
    let imports = program.imports.iter().map(|import| (0, import)).collect();
    let mut analysis = analysis::analyze(vec![
        analysis::SourceModule { path: "collections/iteration".into(), program: &protocol, imports: vec![] },
        analysis::SourceModule { path: "app".into(), program: &program, imports },
    ]).unwrap_or_else(|errors| panic!("{text}\n{errors:?}"));
    test(&mut analysis);
}

#[test]
fn oir2_c6_traversal_goldens() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lowering/oir2");
    let mut paths: Vec<_> = std::fs::read_dir(root.join("fixtures/p2_3/traversal")).unwrap()
        .map(|entry| entry.unwrap().path()).collect();
    paths.sort();
    let names: Vec<_> = paths.iter().map(|path| path.file_stem().unwrap().to_str().unwrap()).collect();
    assert_eq!(names, GOLDENS);
    let expected_names: Vec<_> = GOLDENS.iter().map(|name| format!("{name}.oir")).collect();
    if std::env::var_os("OKKAMA_OIR2_P2_WRITE_GOLDENS").is_none() {
        let mut stored_names: Vec<_> = std::fs::read_dir(root.join("goldens/p2_3/traversal")).unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap()).collect();
        stored_names.sort();
        assert_eq!(stored_names, expected_names);
    }
    for path in paths {
        let name = path.file_stem().unwrap().to_str().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        with_analysis(&text, |analysis| {
            if name.starts_with("map-resolved-") {
                resolve_map_source(analysis);
            }
            let module = super::lower_to_oir2(analysis).unwrap_or_else(|error| panic!("{name}: {error}"));
            ir::verify::verify_high(&module).unwrap_or_else(|errors| panic!("{name}: {errors:?}"));
            let actual = ir::text::print(&module).unwrap();
            let parsed = ir::text::parse(&actual).unwrap();
            ir::verify::verify_high(&parsed).unwrap();
            assert!(ir::text::normalized_eq(&module, &parsed).unwrap());
            assert_eq!(actual, ir::text::print(&parsed).unwrap());
            let second = super::lower_to_oir2(analysis).unwrap();
            ir::verify::verify_high(&second).unwrap();
            assert_eq!(actual, ir::text::print(&second).unwrap(), "{name}");
            check_protocol(&module, name);
            check_fixture(&module, name);
            let expected = root.join("goldens/p2_3/traversal").join(format!("{name}.oir"));
            if std::env::var_os("OKKAMA_OIR2_P2_WRITE_GOLDENS").is_some() {
                std::fs::write(&expected, &actual).unwrap();
            }
            let stored = std::fs::read_to_string(expected).unwrap();
            ir::verify::verify_high(&ir::text::parse(&stored).unwrap()).unwrap();
            assert_eq!(actual, stored, "{name}");
        });
    }
}

// The frontend reserves Source::Map for functional traversal and rejects direct
// map `for`. Feed its already checked values() receiver to the High producer;
// this fixture tests resolved THIR, without changing or claiming new syntax.
fn resolve_map_source(analysis: &mut analysis::Analysis<'_>) {
    let body = analysis.bodies.iter_mut().find(|body| matches!(body.owner, thir::BodyOwner::Entry)).unwrap();
    let mut changed = false;
    for statement in &mut body.block.stmts {
        let thir::StmtKind::Expr(thir::Expr { kind: thir::ExprKind::For { source, .. }, .. }) = &mut statement.kind
            else {
                continue;
            };
        let thir::Source::Array(value) = source else {
            continue;
        };
        let thir::ExprKind::Call { callee: thir::Callee::Builtin { receiver, op: thir::Builtin::MapValues }, .. }
            = &value.kind else {
                continue;
            };
        *source = thir::Source::Map(receiver.clone());
        changed = true;
    }
    assert!(changed);
}

fn check_protocol(module: &ir::Module, name: &str) {
    for (_, function) in module.functions.iter() {
        let Some(body) = &function.body else {
            continue;
        };
        for (_, block) in body.blocks.iter() {
            for (index, id) in block.insts.iter().enumerate() {
                let ir::Operation::Call(ir::CallOp::Role { receiver, role, slot, .. })
                    = &body.insts.get(*id).unwrap().op else {
                        continue;
                    };
                let ir::DeclarationKind::Role { methods, .. } = &module.declarations.get(*role).unwrap().kind else {
                    unreachable!();
                };
                if methods[*slot as usize].name == "available" {
                    assert!(block.insts[..index].iter().any(|id| {
                        matches!(body.insts.get(*id).unwrap().op,
                            ir::Operation::Lexical(ir::LexicalOp::Bind { value, .. }) if value == receiver.value)
                    }), "{name}: cursor must be bound before available");
                }
            }
        }
    }
}


fn role_calls<'a>(module: &'a ir::Module, body: &ir::Body) -> Vec<&'a str> {
    let mut calls = vec![];
    for (_, block) in body.blocks.iter() {
        for id in &block.insts {
            if let ir::Operation::Call(ir::CallOp::Role { role, slot, .. }) = &body.insts.get(*id).unwrap().op {
                let ir::DeclarationKind::Role { methods, .. } = &module.declarations.get(*role).unwrap().kind else {
                    unreachable!();
                };
                calls.push(methods[*slot as usize].name.as_str());
            }
        }
    }
    calls
}

fn check_fixture(module: &ir::Module, name: &str) {
    let main = module.functions.iter().find(|(_, function)| function.symbol == "main").unwrap().1;
    let body = main.body.as_ref().unwrap();
    if name == "batch-preferred" {
        assert_eq!(role_calls(module, body), ["batch"]);
    }
    if name == "cursor-order" {
        let mut calls = vec![];
        for (_, block) in body.blocks.iter() {
            for id in &block.insts {
                match &body.insts.get(*id).unwrap().op {
                    ir::Operation::Call(ir::CallOp::Role { role, slot, .. }) => {
                        let ir::DeclarationKind::Role { methods, .. }
                            = &module.declarations.get(*role).unwrap().kind else {
                                unreachable!();
                            };
                        calls.push(methods[*slot as usize].name.as_str());
                    }
                    ir::Operation::Call(ir::CallOp::Direct { function, .. })
                        if module.functions.get(*function).unwrap().symbol == "app.Observe.body#0" => {
                            calls.push("body");
                        }
                    _ => {}
                }
            }
        }
        assert_eq!(calls, ["cursor", "available", "item", "body", "successor"]);
    }
    if ["cursor-return", "cursor-failure", "cursor-valued-last"].contains(&name) {
        assert_eq!(role_calls(module, body), ["cursor", "available", "item"]);
    }
}

#[test]
fn oir2_c6_structural_protocol_is_rejected_by_frontend() {
    let source = SourceFile::new("structural.ok", concat!(
        "module app; object Structural { Structural(); ",
        "batch() is @int { [1]; } cursor() is int { 0; } } ",
        "main() is int { for Structural() -> x { x; } 0; }",
    )).unwrap();
    let program = syntax::parse(&source, &ParseOptions::default()).unwrap();
    let errors = analysis::analyze(vec![analysis::SourceModule {
        path: "app".into(), program: &program, imports: vec![],
    }]).unwrap_err();
    assert!(errors.iter().all(|error| !error.unsupported));
    assert!(errors.iter().any(|error| error.message.contains("For source")));
}

#[test]
fn oir2_c6_object_key_keeps_existing_verifier_boundary() {
    with_analysis(include_str!("fixtures/p2_3/traversal-object-key-unsupported.ok"), |analysis| {
        let module = super::lower_to_oir2(analysis).unwrap();
        let report = ir::verify::verify_high(&module).unwrap_err();
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.rule == ir::verify::RuleId::T1 && diagnostic.message == "map key type is not permitted"
        }));
    });
}

#[test]
fn oir2_c6_protocol_without_witness_is_unsupported() {
    with_analysis(concat!(
        "module app; import \"collections/iteration\" take Iterating; ",
        "object Run { Run(); run(source is Iterating<int>) is int { for source -> x { x; } } } ",
        "main() is int { 0; }",
    ), |analysis| {
        let error = super::lower_to_oir2(analysis).unwrap_err();
        assert!(matches!(error, super::LowerError::Unsupported { span, .. } if !span.is_empty()));
        assert!(error.to_string().contains("role has no reachable implementation"));
    });
}

const GOLDENS: &[&str] = &[
    "array-accumulator",
    "array-carried-alias",
    "array-empty",
    "array-failure",
    "array-last",
    "array-next",
    "array-normal",
    "array-resource-iteration",
    "array-resource-last-scope",
    "array-resource-last",
    "array-resource-next-scope",
    "array-resource-result",
    "array-return",
    "array-value-last",
    "array-value-next",
    "array-valued-last",
    "batch-empty",
    "batch-failing-refill",
    "batch-failure",
    "batch-last",
    "batch-nested",
    "batch-next",
    "batch-normal",
    "batch-preferred",
    "batch-resource-caught",
    "batch-resource-failure",
    "batch-resource-last",
    "batch-resource-refill",
    "batch-resource-result",
    "batch-resource-return",
    "batch-resource-valued-last",
    "batch-return",
    "batch-value-last",
    "batch-value-next",
    "batch-valued-last",
    "cursor-empty",
    "cursor-failing-available",
    "cursor-failing-cursor",
    "cursor-failing-item",
    "cursor-failing-successor",
    "cursor-failure",
    "cursor-last",
    "cursor-next",
    "cursor-normal",
    "cursor-order",
    "cursor-resource-iteration",
    "cursor-resource-last",
    "cursor-resource-result",
    "cursor-return",
    "cursor-value-last",
    "cursor-value-next",
    "cursor-valued-last",
    "map-keys",
    "map-resolved-empty",
    "map-resolved-failure",
    "map-resolved-last",
    "map-resolved-next",
    "map-resolved-normal",
    "map-resolved-return",
    "map-resolved-value-last",
    "map-resolved-value-next",
    "map-resolved-valued-last",
    "map-values",
    "nested-last",
    "range-literal-empty",
    "range-literal-failure",
    "range-literal-last",
    "range-literal-next",
    "range-literal-normal",
    "range-literal-return",
    "range-literal-value-last",
    "range-literal-value-next",
    "range-literal-valued-last",
    "range-value-empty",
    "range-value-failure",
    "range-value-last",
    "range-value-next",
    "range-value-normal",
    "range-value-return",
    "range-value-value-last",
    "range-value-value-next",
    "range-value-valued-last",
    "variadic-empty",
    "variadic-failure",
    "variadic-last",
    "variadic-next",
    "variadic-normal",
    "variadic-return",
    "variadic-value-last",
    "variadic-value-next",
    "variadic-valued-last",
];
