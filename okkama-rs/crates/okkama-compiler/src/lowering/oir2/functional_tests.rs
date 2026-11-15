//! C7 fixtures are small independent modules, verified at every text boundary.
use crate::analysis::{self, thir};
use crate::oir2 as ir;
use crate::syntax::{self, ParseOptions};
use crate::SourceFile;

fn with_analysis(text: &str, test: impl FnOnce(&mut analysis::Analysis<'_>)) {
    let library = SourceFile::new("iteration.ok", include_str!("fixtures/p2_3/iteration-protocol.ok")).unwrap();
    let protocol = syntax::parse(&library, &ParseOptions::default()).unwrap();
    let source = SourceFile::new("functional.ok", text).unwrap();
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
fn oir2_c7_functional_goldens() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lowering/oir2");
    let mut paths: Vec<_> = std::fs::read_dir(root.join("fixtures/p2_3/functional")).unwrap()
        .map(|entry| entry.unwrap().path()).collect();
    paths.sort();
    let names: Vec<_> = paths.iter().map(|path| path.file_stem().unwrap().to_str().unwrap()).collect();
    assert_eq!(names, GOLDENS);
    if std::env::var_os("OKKAMA_OIR2_P2_WRITE_GOLDENS").is_none() {
        let mut stored: Vec<_> = std::fs::read_dir(root.join("goldens/p2_3/functional")).unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap()).collect();
        stored.sort();
        assert_eq!(stored, GOLDENS.iter().map(|name| format!("{name}.oir")).collect::<Vec<_>>());
    }
    for path in paths {
        let name = path.file_stem().unwrap().to_str().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        with_analysis(&text, |analysis| {
            if name.starts_with("map-find") {
                resolved_map_find(analysis);
            }
            let module = super::lower_to_oir2(analysis).unwrap_or_else(|error| panic!("{name}: {error}"));
            ir::verify::verify_high(&module).unwrap_or_else(|errors| panic!("{name}: {errors:?}"));
            check_cfg(&module, name);
            let actual = ir::text::print(&module).unwrap();
            let parsed = ir::text::parse(&actual).unwrap();
            ir::verify::verify_high(&parsed).unwrap();
            assert!(ir::text::normalized_eq(&module, &parsed).unwrap());
            assert_eq!(actual, ir::text::print(&parsed).unwrap());
            let second = super::lower_to_oir2(analysis).unwrap();
            ir::verify::verify_high(&second).unwrap();
            assert_eq!(actual, ir::text::print(&second).unwrap(), "{name}");
            let expected = root.join("goldens/p2_3/functional").join(format!("{name}.oir"));
            if std::env::var_os("OKKAMA_OIR2_P2_WRITE_GOLDENS").is_some() {
                std::fs::write(&expected, &actual).unwrap();
            }
            let stored = std::fs::read_to_string(expected).unwrap();
            ir::verify::verify_high(&ir::text::parse(&stored).unwrap()).unwrap();
            assert_eq!(actual, stored, "{name}");
        });
    }
}

// The frontend deliberately rejects map.find. Exercise the High traversal on
// resolved THIR, preserving Maybe<value> and adding the checked key binding.
fn resolved_map_find(analysis: &mut analysis::Analysis<'_>) {
    let body = analysis.bodies.iter_mut().find(|body| matches!(body.owner, thir::BodyOwner::Entry)).unwrap();
    let key = thir::LocalId(body.locals.len());
    let mut info = body.locals[0].clone();
    info.name = "key".into();
    info.ty = analysis::ty::Ty::Int;
    body.locals.push(info);
    let mut changed = false;
    for stmt in &mut body.block.stmts {
        let thir::StmtKind::Expr(thir::Expr { kind: thir::ExprKind::Functional(functional), .. }) = &mut stmt.kind
            else {
                continue;
            };
        let thir::Source::Array(value) = &functional.source else {
            continue;
        };
        let thir::ExprKind::Call { callee: thir::Callee::Builtin { receiver, op: thir::Builtin::MapValues }, .. }
            = &value.kind else {
                continue;
            };
        functional.source = thir::Source::Map(receiver.clone());
        functional.bindings.insert(0, key);
        changed = true;
    }
    assert!(changed);
}

fn check_cfg(module: &ir::Module, name: &str) {
    let main = module.functions.iter().find(|(_, function)| function.symbol == "main").unwrap().1;
    let body = main.body.as_ref().unwrap();
    let mut calls = vec![];
    for (_, block) in body.blocks.iter() {
        for id in &block.insts {
            if let ir::Operation::Call(ir::CallOp::Direct { function, .. }) = &body.insts.get(*id).unwrap().op {
                calls.push(module.functions.get(*function).unwrap().symbol.as_str());
            }
        }
    }
    if name == "init-order" {
        let observed: Vec<_> = calls.into_iter().filter(|name| {
            name.contains("Observe.source") || name.contains("Observe.initial")
        }).collect();
        assert_eq!(observed, ["app.Observe.source#0", "app.Observe.initial#1"]);
    }
    if name == "filter-borrow" {
        // Projections survive by retention; filtering never emits a take intent.
        assert!(!body.insts.iter().any(|(_, inst)| {
            matches!(inst.op, ir::Operation::Core(ir::CoreOp::SuccTake { .. })
                | ir::Operation::Lexical(ir::LexicalOp::Temp { .. }))
        }));
    }
    if name == "cursor-init-order" {
        let mut ordered = vec![];
        for (_, block) in body.blocks.iter() {
            for id in &block.insts {
                match &body.insts.get(*id).unwrap().op {
                    ir::Operation::Call(ir::CallOp::Role { role, slot, .. }) => {
                        let ir::DeclarationKind::Role { methods, .. } = &module.declarations.get(*role).unwrap().kind
                            else {
                                unreachable!();
                            };
                        ordered.push(methods[*slot as usize].name.as_str());
                    }
                    ir::Operation::Call(ir::CallOp::Direct { function, .. })
                        if module.functions.get(*function).unwrap().symbol == "app.Observe.initial#0" => {
                            ordered.push("initial");
                        }
                    _ => {}
                }
            }
        }
        assert_eq!(ordered, ["cursor", "initial", "available", "item", "successor"]);
    }
    if ["cursor-any", "cursor-all", "cursor-find"].contains(&name) {
        let mut tested = false;
        for (_, block) in body.blocks.iter() {
            let ir::TermKind::Cond { condition, yes, no } = &block.term.kind else {
                continue;
            };
            if !matches!(body.values.get(*condition).unwrap().def, ir::Definition::BlockParam { .. }) {
                continue;
            }
            let (decisive, resumed) = if name == "cursor-all" { (no, yes) } else { (yes, no) };
            assert!(!reaches_protocol(module, body, decisive.block, "successor"));
            assert!(!reaches_protocol(module, body, decisive.block, "item"));
            assert!(reaches_protocol(module, body, resumed.block, "successor"));
            tested = true;
        }
        assert!(tested, "{name}: decisive CFG edge was not checked");
    }
    if name == "cursor-return" {
        assert!(!body.insts.iter().any(|(_, inst)| {
            let ir::Operation::Call(ir::CallOp::Role { role, slot, .. }) = &inst.op else {
                return false;
            };
            let ir::DeclarationKind::Role { methods, .. } = &module.declarations.get(*role).unwrap().kind else {
                unreachable!();
            };
            methods[*slot as usize].name == "successor"
        }));
    }
    if name == "map-resource" {
        let mut checked = false;
        for (_, inst) in body.insts.iter() {
            let ir::Operation::Collection(ir::CollectionOp::BuilderPush { value, .. }) = inst.op else {
                continue;
            };
            let ir::Definition::BlockParam { block, .. } = body.values.get(value).unwrap().def else {
                panic!("publication must use the completed body result");
            };
            let completed = body.blocks.get(block).unwrap();
            assert_eq!(module.scopes.get(completed.lexical_scope).unwrap().kind, ir::ScopeKind::Iteration);
            assert!(body.high_results.iter().any(|result| {
                let source = body.blocks.get(result.block).unwrap();
                result.demand == ir::ResultDemand::Want && result.prepared.is_some()
                    && matches!(&source.term.kind, ir::TermKind::Br(target) if target.block == block)
                    && source.lexical_scope != completed.lexical_scope
            }));
            checked = true;
        }
        assert!(checked);
    }
    if name.starts_with("reduce-") {
        assert!(body.insts.iter().any(|(_, inst)| {
            matches!(inst.op, ir::Operation::Collection(ir::CollectionOp::ArrayConcat { .. }))
        }));
        assert!(!body.insts.iter().any(|(_, inst)| {
            matches!(inst.op, ir::Operation::Collection(ir::CollectionOp::BuilderPush { .. }))
        }));
    }
}

fn reaches_protocol(module: &ir::Module, body: &ir::Body, start: ir::BlockId, name: &str) -> bool {
    let mut seen = std::collections::HashSet::new();
    let mut pending = vec![start];
    while let Some(id) = pending.pop() {
        if !seen.insert(id) {
            continue;
        }
        let block = body.blocks.get(id).unwrap();
        for inst in &block.insts {
            if let ir::Operation::Call(ir::CallOp::Role { role, slot, .. }) = &body.insts.get(*inst).unwrap().op {
                let ir::DeclarationKind::Role { methods, .. } = &module.declarations.get(*role).unwrap().kind else {
                    unreachable!();
                };
                if methods[*slot as usize].name == name {
                    return true;
                }
            }
        }
        match &block.term.kind {
            ir::TermKind::Br(target) => pending.push(target.block),
            ir::TermKind::Cond { yes, no, .. } => {
                pending.push(yes.block);
                pending.push(no.block);
            }
            _ => {}
        }
    }
    false
}

#[test]
fn oir2_c7_frontend_restrictions_are_preserved() {
    for text in [
        "module app; main() is int { [1].map() -> x { next; x; }; 0; }",
        "module app; main() is int { [1].map() -> x { last; x; }; 0; }",
        "module app; main() is int { [1: 2].find() -> k, v { true; }; 0; }",
        "module app; main() is int { [:].all() -> k, v { true; }; 0; }",
    ] {
        let source = SourceFile::new("restriction.ok", text).unwrap();
        let program = syntax::parse(&source, &ParseOptions::default()).unwrap();
        let errors = analysis::analyze(vec![analysis::SourceModule {
            path: "app".into(), program: &program, imports: vec![],
        }]).unwrap_err();
        assert!(errors.iter().all(|error| !error.unsupported));
    }
}

#[test]
fn oir2_c7_anonymous_action_inside_block_has_its_own_boundary() {
    with_analysis("module app; main() is int { [1].map() -> x { () is int { x; }; }; 0; }", |analysis| {
        let module = super::lower_to_oir2(analysis).unwrap();
        super::tests::verify_high(&module, "closure inside functional block");
        assert!(module.functions.iter().any(|(_, function)| function.attrs.iter().any(|attr| {
            matches!(attr, ir::FunctionAttr::Lambda { captures: 1, .. })
        })));
    });
}

const GOLDENS: &[&str] = &[
    "array-all-empty",
    "array-all",
    "array-any-empty",
    "array-any",
    "array-filter-empty",
    "array-filter",
    "array-find-empty",
    "array-find",
    "array-map-empty",
    "array-map",
    "array-reduce-empty",
    "array-reduce",
    "array-untyped-all",
    "array-untyped-any",
    "array-untyped-filter",
    "array-untyped-map",
    "array-untyped-reduce",
    "batch-all-empty",
    "batch-all",
    "batch-any-empty",
    "batch-any",
    "batch-filter-empty",
    "batch-filter",
    "batch-find-empty",
    "batch-find",
    "batch-map-empty",
    "batch-map",
    "batch-reduce-empty",
    "batch-reduce",
    "caught",
    "cursor-all-empty",
    "cursor-all",
    "cursor-any-empty",
    "cursor-any",
    "cursor-filter-empty",
    "cursor-filter",
    "cursor-find-empty",
    "cursor-find",
    "cursor-init-order",
    "cursor-map-empty",
    "cursor-map",
    "cursor-reduce-empty",
    "cursor-reduce",
    "cursor-return",
    "deferred-functional",
    "failure",
    "filter-borrow",
    "find-resource",
    "generic-map",
    "init-order",
    "map-all-empty",
    "map-all",
    "map-any-empty",
    "map-any",
    "map-defer",
    "map-filter-empty",
    "map-filter",
    "map-find-empty",
    "map-find",
    "map-map-empty",
    "map-map",
    "map-reduce-empty",
    "map-reduce",
    "map-resource",
    "nested",
    "range-all-empty",
    "range-all",
    "range-any-empty",
    "range-any",
    "range-filter-empty",
    "range-filter",
    "range-find-empty",
    "range-find",
    "range-map-empty",
    "range-map",
    "range-reduce-empty",
    "range-reduce",
    "range-value-all-empty",
    "range-value-all",
    "range-value-any-empty",
    "range-value-any",
    "range-value-filter-empty",
    "range-value-filter",
    "range-value-find-empty",
    "range-value-find",
    "range-value-map-empty",
    "range-value-map",
    "range-value-reduce-empty",
    "range-value-reduce",
    "reduce-append",
    "reduce-observable",
    "return",
];
