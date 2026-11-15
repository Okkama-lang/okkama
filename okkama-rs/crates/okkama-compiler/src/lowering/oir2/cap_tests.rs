//! C5 source High evidence. Own elaboration and runtime acceptance are separate gates.
use crate::oir2 as ir;

const CASES: &[&str] = &[
    "cap-body", "cap-call", "cap-external-internal", "cap-field", "cap-generic", "cap-generic-transport",
    "cap-hidden", "cap-parameter", "cap-recompute", "cap-snapshot",
];

#[test]
fn oir2_c5_cap_source_goldens() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lowering/oir2");
    for directory in ["fixtures", "goldens"] {
        if directory == "goldens" && std::env::var_os("OKKAMA_OIR2_C5_WRITE_GOLDENS").is_some() {
            continue;
        }
        let mut names: Vec<_> = std::fs::read_dir(root.join(format!("{directory}/p2_4/caps"))).unwrap()
            .map(|entry| entry.unwrap().path().file_stem().unwrap().to_str().unwrap().to_owned()).collect();
        names.sort();
        assert_eq!(names, CASES);
    }
    let mut failed = vec![];
    for name in CASES {
        if std::panic::catch_unwind(|| golden(root.as_path(), name)).is_err() {
            failed.push(*name);
        }
    }
    assert!(failed.is_empty(), "failed C5 goldens: {failed:?}");
}

fn golden(root: &std::path::Path, name: &str) {
    let source = std::fs::read_to_string(root.join(format!("fixtures/p2_4/caps/{name}.ok"))).unwrap();
    let actual = super::print_oir2(&source).unwrap_or_else(|error| panic!("{name}: {error}"));
    assert_eq!(actual, super::print_oir2(&source).unwrap(), "determinism: {name}");
    assert!(actual.lines().count() <= 200, "keep C5 goldens small: {name}");
    let directory = root.join("goldens/p2_4/caps");
    let path = directory.join(format!("{name}.oir"));
    if std::env::var_os("OKKAMA_OIR2_C5_WRITE_GOLDENS").is_some() {
        std::fs::create_dir_all(directory).unwrap();
        std::fs::write(&path, wrap_high(&actual)).unwrap();
    }
    let stored = std::fs::read_to_string(path).unwrap();
    let parsed = ir::text::parse(&stored).unwrap();
    super::verify_high(&parsed, name);
    assert_eq!(actual, ir::text::print(&parsed).unwrap(), "{name}");
    assert!(stored.lines().all(|line| line.len() <= 120), "stored line width: {name}");
}

// Concrete generic symbols can make a signature wider than a readable stored golden.
// Break only at whitespace outside quoted identifiers; compare canonical text after parsing.
pub(super) fn wrap_high(text: &str) -> String {
    let mut output = String::new();
    for line in text.lines() {
        let mut remaining = line.to_owned();
        while remaining.len() > 120 {
            let mut quoted = false;
            let mut escaped = false;
            let mut split = None;
            for (index, ch) in remaining.char_indices().take_while(|(index, _)| *index < 120) {
                if ch == '"' && !escaped {
                    quoted = !quoted;
                }
                if ch.is_whitespace() && !quoted && index > 8 {
                    split = Some(index);
                }
                escaped = ch == '\\' && !escaped;
            }
            let split = split.expect("golden has a token wider than 120 columns");
            output.push_str(&remaining[..split]);
            output.push('\n');
            remaining = format!("        {}", remaining[split..].trim_start());
        }
        output.push_str(&remaining);
        output.push('\n');
    }
    output
}

fn with_case(name: &str, test: impl FnOnce(&ir::Module)) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("src/lowering/oir2/fixtures/p2_4/caps/{name}.ok"));
    let source = std::fs::read_to_string(path).unwrap();
    super::with_analysis(&source, |analysis| {
        let module = super::lower_to_oir2(analysis).unwrap();
        super::verify_high(&module, name);
        test(&module);
    });
}

fn main_body(module: &ir::Module) -> &ir::Body {
    module.functions.get(module.entry.unwrap()).unwrap().body.as_ref().unwrap()
}

#[test]
fn oir2_c5_full_owner_graph_and_environment_abi() {
    with_case("cap-hidden", |module| {
        let (decl, owner) = module.declarations.iter().find(|(_, decl)| decl.name == "app.Owner").unwrap();
        let ir::DeclarationKind::Object { fields, .. } = &owner.kind else {
            panic!("owner layout");
        };
        assert_eq!(fields.iter().map(|field| field.name.as_str()).collect::<Vec<_>>(), ["hidden", "label"]);
        let ir::Type::Object { decl: resource, .. } = *module.types.get(fields[0].ty).unwrap() else {
            panic!("hidden resource");
        };
        assert!(matches!(module.declarations.get(resource).unwrap().kind,
            ir::DeclarationKind::Object { destroy: Some(_), .. }));
        let mut makes = 0;
        for (_, function) in module.functions.iter() {
            for (_, inst) in function.body.as_ref().unwrap().insts.iter() {
                if let ir::Operation::Core(ir::CoreOp::CapMake { owner, ty, packet }) = inst.op {
                    makes += 1;
                    assert_eq!(packet, None);
                    let body = function.body.as_ref().unwrap();
                    let owner_ty = body.values.get(owner).unwrap().ty;
                    assert_eq!(module.types.get(owner_ty).unwrap(), &ir::Type::Object { decl, args: vec![] });
                    let ir::Type::Cap { decl: cap, .. } = *module.types.get(ty).unwrap() else {
                        panic!("cap type");
                    };
                    let ir::DeclarationKind::Cap { owner: environment, fields } =
                        &module.declarations.get(cap).unwrap().kind else {
                        panic!("cap layout");
                    };
                    assert_eq!(*environment, owner_ty);
                    let computed = module.functions.get(fields[0].function).unwrap();
                    assert_eq!(computed.signature.params, [owner_ty], "owner is environment parameter 0");
                    let body = computed.body.as_ref().unwrap();
                    assert!(body.insts.iter().all(|(_, inst)| !matches!(inst.op,
                        ir::Operation::Core(ir::CoreOp::ObjGet { field, .. }) if field.index == 0)),
                        "retention must include the resource even though the view never reads it");
                }
            }
        }
        assert_eq!(makes, 1);
    });
}

#[test]
fn oir2_c5_snapshot_and_repeated_read_operands() {
    with_case("cap-snapshot", |module| {
        let body = main_body(module);
        let owners: Vec<_> = body.insts.iter().filter_map(|(_, inst)| match inst.op {
            ir::Operation::Core(ir::CoreOp::CapMake { owner, packet: None, .. }) => Some(owner),
            _ => None,
        }).collect();
        assert_eq!(owners.len(), 2);
        let (binding, rebound) = body.insts.iter().find_map(|(_, inst)| match inst.op {
            ir::Operation::Lexical(ir::LexicalOp::Rebind { binding, value, .. }) => Some((binding, value)),
            _ => None,
        }).unwrap();
        assert_ne!(owners[0], rebound);
        assert_eq!(owners[1], rebound);
        assert!(body.insts.iter().any(|(_, inst)| matches!(inst.op,
            ir::Operation::Lexical(ir::LexicalOp::Bind { binding: b, value })
                if b == binding && value == owners[0])));
    });
    with_case("cap-recompute", |module| {
        let body = main_body(module);
        let mut reads = vec![];
        let mut makes = vec![];
        for (id, inst) in body.insts.iter() {
            match &inst.op {
                ir::Operation::Core(ir::CoreOp::CapMake { .. }) => makes.push((id, inst.results[0])),
                ir::Operation::Call(ir::CallOp::Cap { cap, field, call }) => {
                    assert!(call.args.is_empty());
                    assert_eq!(cap.convention, ir::OperandConvention::Borrow);
                    assert!(call.packet.is_none() && call.deps.is_empty() && call.summary.is_none());
                    reads.push((id, cap.value, *field));
                }
                _ => {}
            }
        }
        assert_eq!(makes.len(), 1);
        assert_eq!(reads.len(), 2);
        assert!(makes[0].0 < reads[0].0 && reads[0].0 < reads[1].0);
        assert_eq!(reads[0].1, makes[0].1);
        assert_eq!(reads[1].1, makes[0].1);
        assert_eq!(reads[0].2, reads[1].2);
        assert!(!body.insts.iter().any(|(_, inst)| matches!(inst.op,
            ir::Operation::Call(ir::CallOp::Direct { function, .. })
                if module.functions.get(function).unwrap().symbol.contains(".cap."))),
            "acquisition must not evaluate the computed field");
    });
}

#[test]
fn oir2_c5_generic_nominal_identity() {
    with_case("cap-generic", |module| {
        let caps: Vec<_> = module.declarations.iter().filter_map(|(decl, data)| match &data.kind {
            ir::DeclarationKind::Cap { owner, fields } => Some((decl, *owner, fields[0].function)),
            _ => None,
        }).collect();
        assert_eq!(caps.len(), 2);
        assert_ne!(caps[0].0, caps[1].0);
        assert_ne!(caps[0].1, caps[1].1);
        assert_ne!(caps[0].2, caps[1].2);
        for (_, owner, function) in caps {
            assert_eq!(module.functions.get(function).unwrap().signature.params, [owner]);
        }
    });
}

#[test]
fn oir2_c5_computed_callable_field_boundaries() {
    let owner = concat!("module app; object Owner { has f is () is int; Owner(:f); ",
        "cap View { action is () is int { f; } } } ");
    let main = "main() is int { Owner(f: () is int { 7; })::View::action(); }";
    super::print_oir2(&format!("{owner}{main}")).unwrap();
    let open = concat!("object Open { Open(); read(view is Owner::View) is int { ",
        "view::action(); } } ");
    // #569 C3b: an action read from a cap parameter is open in the reading body.
    let text = super::print_oir2(&format!("{owner}{open}{main}")).unwrap();
    assert!(text.contains("[intrinsic \"oir.sync_closure.cleanup\"]"), "{text}");
}

#[test]
fn oir2_c5_frontend_cap_boundaries() {
    let owner = concat!("module app; object Owner { has n is int; Owner(:n); ",
        "cap View { value is int { n; } } } ");
    for body in [
        "let view = Owner(n: 1)::View; view == view; 0;",
        "let view = Owner::View(); 0;",
        "let view = Owner(n: 1)::View; view.value; 0;",
        "let view is Owner::View = Owner(n: 1); 0;",
    ] {
        let text = format!("{owner}main() is int {{ {body} }}");
        let source = crate::SourceFile::new("cap-negative.ok", &text).unwrap();
        let program = crate::syntax::parse(&source, &Default::default()).unwrap();
        let errors = crate::analysis::analyze(vec![crate::analysis::SourceModule {
            path: "app".into(), program: &program, imports: vec![],
        }]).unwrap_err();
        assert!(errors.iter().any(|error| !error.unsupported && !error.warning), "{errors:?}");
    }
}
