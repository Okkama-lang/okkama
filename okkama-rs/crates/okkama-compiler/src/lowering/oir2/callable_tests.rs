use crate::analysis::{self, thir, ty};
use crate::syntax::{self, ParseMode, ParseOptions};
use crate::SourceFile;
use super::{callables, specialize};

fn with_prepared(text: &str, test_mode: bool, test: impl FnOnce(callables::Prepared<'_>)) {
    let source = SourceFile::new("callables.ok", text).unwrap();
    let mode = if test_mode { ParseMode::Full } else { ParseMode::Production };
    let program = syntax::parse(&source, &ParseOptions { mode, ..Default::default() }).unwrap();
    let program = if test_mode {
        syntax::prune::desugar_test_items(program)
    } else {
        syntax::prune::strip_test_items(program)
    };
    let analysis = analysis::analyze_mode(vec![analysis::SourceModule {
        path: "app".into(), program: &program, imports: vec![],
    }], 1, test_mode).unwrap_or_else(|errors| panic!("analysis: {errors:?}"));
    let first = specialize::prepare(&analysis).unwrap();
    let second = specialize::prepare(&analysis).unwrap();
    assert_eq!(format!("{:?}", first.callables), format!("{:?}", second.callables));
    if first.callables.bodies.iter().any(|entry| matches!(entry.body.owner,
        thir::BodyOwner::Test(_) | thir::BodyOwner::CapField { .. })) {
        let error = super::lower_to_oir2(&analysis).unwrap_err();
        assert!(matches!(error, super::LowerError::Unsupported { span, .. } if !span.is_empty()));
    }
    test(first);
}

#[test]
fn oir2_callable_types_and_generic_specialization() {
    with_prepared(concat!(
        "module app; object Id<T> { has seed is T; Id(:seed); id(f is (T) is T) is (T) is T { f; } } ",
        "main() is int { let f = (x is int) is int { x; }; let g = Id(seed: 0).id(f); g(4); }",
    ), false, |prepared| {
        let object = prepared.analysis.registry.objects.iter().find(|object| object.name == "Id<int>").unwrap();
        let method = &object.methods[0];
        assert_eq!(method.params[0].1, method.result.clone().unwrap());
        assert!(!method.params[0].1.mentions_param());
        let declarations = super::declarations::Declarations::new(&prepared.analysis).unwrap();
        let ty = declarations.types[&method.params[0].1];
        let crate::oir2::Type::Fn(signature) = declarations.builder.module().types.get(ty).unwrap() else {
            panic!("concrete fn type");
        };
        assert_eq!(signature.params.len(), 1);
        assert_eq!(signature.result, Some(signature.params[0]));
        assert_eq!(prepared.callables.open_calls.len(), 1);
    });
}

#[test]
fn oir2_callable_nested_signatures_and_open_inventory() {
    with_prepared(concat!(
        "module app; object Apply<T> { has seed is T; Apply(:seed); ",
        "run(f is ((T) is T) is (T) is T, g is (T) is T, x is T) is T { f(g)(g(x)); } } ",
        "main() is int { let g = (v is int) is int { v; }; ",
        "Apply(seed: 0).run((h is (int) is int) is (int) is int { h; }, g, 1); }",
    ), false, |prepared| {
        assert_eq!(prepared.callables.open_calls.len(), 2);
        let sites = &prepared.callables.open_calls;
        assert_eq!(sites.iter().map(|entry| entry.sites.len()).sum::<usize>(), 3);
        for entry in sites {
            assert!(!entry.ty.mentions_param());
            assert!(entry.sites.iter().all(|site| site.module == prepared.analysis.registry.entry_module));
        }
        let declarations = super::declarations::Declarations::new(&prepared.analysis).unwrap();
        let nested = sites.iter().find(|entry| matches!(&entry.ty, ty::Ty::Action(action)
            if matches!(&action.params[0], ty::Ty::Action(_)))).unwrap();
        let lowered = declarations.types[&nested.ty];
        let crate::oir2::Type::Fn(signature) = declarations.builder.module().types.get(lowered).unwrap() else {
            panic!("nested fn");
        };
        assert_eq!(signature.result, Some(signature.params[0]));
    });
}

#[test]
fn oir2_callable_capture_layout_local_parameter_receiver() {
    with_prepared(concat!(
        "module app; object Box<T> { has value is T; Box(:value); ",
        "make(p is T) is () is T { let local = p; () is T { local; p; value; } } ",
        "plain() is () is int { () is int { 1; } } } ",
        "main() is int { let box = Box(value: 3); box.make(4); box.plain(); 0; }",
    ), false, |prepared| {
        let closures: Vec<_> = prepared.callables.bodies.iter().filter(|entry| !entry.key.path.is_empty()).collect();
        assert_eq!(closures.len(), 2);
        let captured = closures.iter().find(|entry| !entry.captures.is_empty()).unwrap();
        assert_eq!(captured.key.path, vec![0]);
        assert!(matches!(captured.captures[0].source, callables::Snapshot::Receiver));
        assert!(matches!(captured.captures[0].ty, ty::Ty::Object(_, ref args) if args.is_empty()));
        assert!(matches!(captured.captures[1].source, callables::Snapshot::Local(_)));
        assert!(matches!(captured.captures[2].source, callables::Snapshot::Parameter(_)));
        for capture in &captured.captures[1..] {
            assert_eq!(capture.ty, ty::Ty::Int);
            assert_eq!(captured.body.locals[capture.target.unwrap().0].ty, capture.ty);
        }
        let empty = closures.iter().find(|entry| entry.captures.is_empty()).unwrap();
        assert!(empty.receiver.is_some());
        assert_eq!(empty.body.owner, thir::BodyOwner::Closure);
    });
}

#[test]
fn oir2_callable_recursive_closure_discovery_and_distinct_keys() {
    with_prepared(concat!(
        "module app; object Box<T> { has value is T; Box(:value); ",
        "make() is () is () is T { () is () is T { self.make(); () is T { value; } } } } ",
        "main() is int { Box(value: 1).make(); Box(value: \"s\").make(); ",
        "let a = () is int { 2; }; let b = () is int { 3; }; 0; }",
    ), false, |prepared| {
        let entries = &prepared.callables.bodies;
        let closures: Vec<_> = entries.iter().filter(|entry| !entry.key.path.is_empty()).collect();
        assert_eq!(closures.len(), 6);
        let keys: std::collections::HashSet<_> = closures.iter().map(|entry| &entry.key).collect();
        assert_eq!(keys.len(), closures.len());
        assert_eq!(closures.iter().filter(|entry| entry.key.path == [0, 0]).count(), 2);
        assert_eq!(entries.iter().filter(|entry| entry.key.path.is_empty()
            && matches!(entry.body.owner, thir::BodyOwner::Method { .. })).count(), 2);
        for entry in closures {
            assert!(entry.body.locals.iter().all(|local| !local.ty.mentions_param()));
        }
    });
}

#[test]
fn oir2_callable_production_test_isolation() {
    let source = concat!("module app; main() is int { 0; } ",
        "test \"only-test\" { let f = () is int { 1; }; test.is(f(), 1); }");
    with_prepared(source, false, |prepared| {
        assert!(!prepared.callables.test_mode);
        assert!(!prepared.analysis.registry.test_mode);
        assert!(prepared.analysis.registry.tests.is_empty());
        assert!(prepared.callables.open_calls.is_empty());
        assert_eq!(prepared.callables.bodies.len(), 1);
    });
    with_prepared(source, true, |prepared| {
        assert!(prepared.callables.test_mode);
        assert!(prepared.analysis.registry.test_mode);
        assert_eq!(prepared.analysis.registry.tests[0].name, "only-test");
        let entry = prepared.callables.bodies.iter().find(|entry| entry.key.root == thir::BodyOwner::Test(0)
            && entry.key.path.is_empty()).unwrap();
        assert!(thir::first_expr(&entry.body.block, &|expr| matches!(&expr.kind,
            thir::ExprKind::Call { callee: thir::Callee::Test(name), .. } if name == "is")).is_some());
        assert_eq!(prepared.callables.open_calls[0].sites[0].body.root, thir::BodyOwner::Test(0));
        assert!(matches!(super::lower_to_oir2(&prepared.analysis), Err(super::LowerError::Unsupported { .. })));
    });
}

#[test]
fn oir2_callable_capture_local_snapshot() {
    with_prepared("module app; main() is int { let x = 7; let f = () is int { x; }; 0; }", false, |prepared| {
        let closure = prepared.callables.bodies.iter().find(|entry| !entry.key.path.is_empty()).unwrap();
        assert_eq!(closure.captures.len(), 1);
        assert_eq!(closure.captures[0].source, callables::Snapshot::Local(thir::LocalId(0)));
        assert_eq!(closure.captures[0].ty, ty::Ty::Int);
        assert!(closure.receiver.is_none());
    });
}

#[test]
fn oir2_callable_capture_parameter_snapshot() {
    with_prepared(concat!("module app; object A { A(); make(x is int) is () is int { () is int { x; } } } ",
        "main() is int { A().make(1); 0; }"), false, |prepared| {
        let closure = prepared.callables.bodies.iter().find(|entry| !entry.key.path.is_empty()).unwrap();
        assert_eq!(closure.captures.len(), 1);
        assert_eq!(closure.captures[0].source, callables::Snapshot::Parameter(thir::LocalId(0)));
        assert_eq!(closure.captures[0].ty, ty::Ty::Int);
    });
}

#[test]
fn oir2_callable_capture_receiver_snapshot() {
    with_prepared(concat!("module app; object A { has x is int; A(:x); ",
        "make() is () is int { () is int { self.x; } } } main() is int { A(x: 1).make(); 0; }"),
        false, |prepared| {
            let closure = prepared.callables.bodies.iter().find(|entry| !entry.key.path.is_empty()).unwrap();
            assert_eq!(closure.captures.len(), 1);
            assert_eq!(closure.captures[0].source, callables::Snapshot::Receiver);
            assert_eq!(Some(&closure.captures[0].ty), closure.receiver.as_ref());
            assert_eq!(closure.captures[0].target, None);
        });
}

#[test]
fn oir2_callable_generic_bound_target_is_discovered() {
    with_prepared(concat!("module app; object A<T> { has x is T; A(:x); get() is T { x; } } ",
        "main() is int { let f is () is int = A(x: 2).get; f(); }"), false, |prepared| {
        let bound = &prepared.callables.bound[0];
        let callables::BoundTarget::Method { object, method } = bound.target else {
            panic!("bound method target");
        };
        assert_eq!(bound.receiver, ty::Ty::object(object));
        assert_eq!(prepared.analysis.registry.objects[object.0].name, "A<int>");
        assert!(prepared.analysis.bodies.iter().any(|body| body.owner == thir::BodyOwner::Method { object, method }));
    });
}

#[test]
fn oir2_callable_generic_cap_computed_body_is_discovered() {
    with_prepared(concat!("module app; object A<T> { has x is T; A(:x); ",
        "cap View { value is T { x; } action is () is T { let v = x; () is T { v; } } } } ",
        "main() is int { let c = A(x: 2)::View; c::value; }"), false, |prepared| {
        let entries = &prepared.callables.bodies;
        let fields: Vec<_> = entries.iter().filter(|entry| entry.key.path.is_empty()
            && matches!(entry.key.root, thir::BodyOwner::CapField { .. })).collect();
        assert_eq!(fields.len(), 2);
        for entry in fields {
            let thir::BodyOwner::CapField { object, cap, field } = entry.key.root else {
                unreachable!();
            };
            assert_eq!(entry.receiver, Some(ty::Ty::object(object)));
            assert_eq!(prepared.analysis.registry.objects[object.0].name, "A<int>");
            assert!(!prepared.analysis.registry.objects[object.0].caps[cap].fields[field].1.mentions_param());
        }
        let nested = entries.iter().find(|entry| !entry.key.path.is_empty()).unwrap();
        assert!(matches!(nested.key.root, thir::BodyOwner::CapField { .. }));
        assert!(matches!(nested.captures[0].source, callables::Snapshot::Local(_)));
    });
}

fn inventory_text(prepared: &callables::Prepared<'_>) -> String {
    let mut lines = vec![format!("test-mode {}", prepared.callables.test_mode)];
    let name = |ty: &ty::Ty| specialize::type_name(&prepared.analysis.registry, ty).unwrap();
    for entry in &prepared.callables.bodies {
        lines.push(format!("body {:?}/{:?}", entry.key.root, entry.key.path));
        if let Some(ty) = &entry.receiver {
            lines.push(format!("  receiver {}", name(ty)));
        }
        for capture in &entry.captures {
            lines.push(format!("  snapshot {:?} -> {:?}: {}", capture.source, capture.target, name(&capture.ty)));
        }
        for local in &entry.body.locals {
            lines.push(format!("  local {}: {}", local.name, name(&local.ty)));
        }
    }
    for entry in &prepared.callables.open_calls {
        lines.push(format!("open {}", name(&entry.ty)));
        for site in &entry.sites {
            lines.push(format!("  site {:?}/{:?} #{} module {}",
                site.body.root, site.body.path, site.ordinal, site.module));
        }
    }
    lines.join("\n") + "\n"
}

#[test]
fn oir2_callable_discovery_goldens() {
    for (name, source) in [
        ("generic-callables", include_str!("fixtures/p2_4/generic-callables.ok")),
        ("nested-callables", include_str!("fixtures/p2_4/nested-callables.ok")),
        ("recursive-closures", include_str!("fixtures/p2_4/recursive-closures.ok")),
    ] {
        with_prepared(source, false, |prepared| {
            let actual = inventory_text(&prepared);
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/lowering/oir2/goldens/p2_4").join(format!("{name}.txt"));
            if std::env::var_os("OKKAMA_OIR2_P2_WRITE_GOLDENS").is_some() {
                std::fs::write(&path, &actual).unwrap();
            }
            assert_eq!(actual, std::fs::read_to_string(path).unwrap(), "{name}");
        });
    }
}

#[test]
fn oir2_callable_concrete_nominals_inside_action_types() {
    with_prepared(concat!("module app; object Box<T> { has value is T; Box(:value); } ",
        "object Id<T> { has seed is T; Id(:seed); ",
        "id(f is (Box<T>) is Box<T>) is (Box<T>) is Box<T> { f; } } ",
        "main() is int { let f = (x is Box<int>) is Box<int> { x; }; Id(seed: 1).id(f); 0; }"),
        false, |prepared| {
            let object = prepared.analysis.registry.objects.iter().find(|object| object.name == "Id<int>").unwrap();
            let ty::Ty::Action(action) = &object.methods[0].params[0].1 else {
                panic!("concrete action");
            };
            let ty::Ty::Object(id, args) = &action.params[0] else {
                panic!("concrete nominal action parameter");
            };
            assert!(args.is_empty());
            assert_eq!(prepared.analysis.registry.objects[id.0].name, "Box<int>");
            assert_eq!(action.result.as_ref(), Some(&action.params[0]));
            super::declarations::Declarations::new(&prepared.analysis).unwrap();
        });
}

#[test]
fn oir2_callable_action_as_generic_argument_and_void_result() {
    with_prepared(concat!(
        "module app; object Echo<T> { has seed is T; Echo(:seed); echo(value is T) is T { value; } } ",
        "main() is int { let f = (x is int) is void { x; }; let g = Echo(seed: f).echo(f); g(1); 0; }"),
        false, |prepared| {
            let object = prepared.analysis.registry.objects.iter().find(|object| {
                object.name.starts_with("Echo<action<")
            }).unwrap();
            assert!(matches!(object.methods[0].params[0].1, ty::Ty::Action(_)));
            assert_eq!(object.methods[0].result.as_ref(), Some(&object.methods[0].params[0].1));
            let declarations = super::declarations::Declarations::new(&prepared.analysis).unwrap();
            let lowered = declarations.types[&object.methods[0].params[0].1];
            let crate::oir2::Type::Fn(signature) = declarations.builder.module().types.get(lowered).unwrap() else {
                panic!("void callable");
            };
            assert_eq!(signature.result, None);
            assert_eq!(signature.params.len(), 1);
            assert_eq!(prepared.callables.open_calls.len(), 1);
        });
}

#[test]
fn oir2_callable_nested_worklist_obeys_limits() {
    let source = SourceFile::new("limits.ok", concat!("module app; main() is int { ",
        "let f = () is () is int { () is int { 1; } }; 0; }")).unwrap();
    let program = syntax::parse(&source, &ParseOptions::default()).unwrap();
    let analysis = analysis::analyze(vec![analysis::SourceModule {
        path: "app".into(), program: &program, imports: vec![],
    }]).unwrap();
    for limits in [specialize::Limits { depth: 2, count: 100 }, specialize::Limits { depth: 100, count: 2 }] {
        let error = specialize::prepare_with_limits(&analysis, limits).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("specialization limit"), "{message}");
        assert!(message.contains("main::closure[0] -> main::closure[0, 0]"), "{message}");
    }
}
