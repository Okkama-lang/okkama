//! C3a high-only source evidence; no Own/runtime acceptance is implied.
use crate::oir2 as ir;

const CASES: &[&str] = &[
    "closure-borrowed", "closure-captured-callable", "closure-defer", "closure-empty", "closure-equality",
    "closure-failure", "closure-fresh", "closure-known", "closure-known-join", "closure-known-loop",
    "closure-known-parameter", "closure-known-phi", "closure-loop", "closure-nested", "closure-parameter",
    "closure-receiver", "closure-resource-result", "closure-return", "closure-snapshot", "closure-successor",
];

#[test]
fn oir2_c3a_anonymous_source_goldens() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lowering/oir2");
    let destination = root.join("goldens/p2_4/anonymous");
    for name in CASES {
        let source = std::fs::read_to_string(root.join(format!("fixtures/p2_4/anonymous/{name}.ok"))).unwrap();
        let actual = super::print_oir2(&source).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(actual, super::print_oir2(&source).unwrap(), "determinism: {name}");
        let path = destination.join(format!("{name}.oir"));
        if std::env::var_os("OKKAMA_OIR2_P2_WRITE_GOLDENS").is_some() {
            std::fs::create_dir_all(&destination).unwrap();
            let wrapped = actual.replace(", replacements=[", ",\n        replacements=[");
            std::fs::write(&path, wrapped).unwrap();
        }
        let stored = std::fs::read_to_string(path).unwrap();
        let parsed = ir::text::parse(&stored).unwrap();
        super::verify_high(&parsed, name);
        assert_eq!(actual, ir::text::print(&parsed).unwrap(), "{name}");
        assert!(stored.lines().all(|line| line.len() <= 120), "stored line width: {name}");
    }
}

#[test]
fn oir2_c3a_open_invocations_remain_explicit() {
    for source in [
        "module app; object Apply { Apply(); call(f is () is int) is int { f(); } } main() is int { 0; }",
        concat!("module app; object Hold { has f is () is int; Hold(:f); call() is int { f(); } } ",
            "main() is int { Hold(f: () is int { 1; }).call(); }"),
        concat!("module app; object Hold { has f is () is int; Hold(:f); ",
            "call() is int { let g = if true { () is int { 1; }; } else { f; }; g(); } } ",
            "main() is int { Hold(f: () is int { 1; }).call(); }"),
    ] {
        let error = super::print_oir2(source).unwrap_err();
        assert!(matches!(error, super::LowerError::Unsupported { span, .. } if !span.is_empty()), "{error}");
        assert!(error.to_string().contains("open action invocation"), "{error}");
        assert!(error.to_string().contains("O1"), "{error}");
    }
}

#[test]
fn oir2_c3a_successor_flow_proof_precedes_capture_conversion() {
    let prelude = concat!("module app; object Counter { has n is int; Counter(:n); ",
        "bumped() is ^Counter { ^n = n + 1; } read() is int { n; } } ");
    for body in [
        "let f = () is int { later.read(); }; let alias = f; 0;",
        "let wanted = Counter(n: 4).read() > 0; \
        let f = () is int { if wanted { later.read(); } else { 0; } }; f();",
    ] {
        let source = format!("{prelude} main() is int {{ let later = Counter(n: 1).bumped(); {body} }}");
        let source = crate::SourceFile::new("successor.ok", source).unwrap();
        let program = crate::syntax::parse(&source, &Default::default()).unwrap();
        let errors = crate::analysis::analyze(vec![crate::analysis::SourceModule {
            path: "app".into(), program: &program, imports: vec![],
        }]).unwrap_err();
        assert!(errors.iter().any(|error| error.message.to_lowercase().contains("successor")), "{errors:?}");
    }
    let source = format!("{prelude} main() is int {{ let later = Counter(n: 1).bumped(); \
        let f = () is int {{ later.read(); }}; let alias = f; alias(); }}");
    let text = super::print_oir2(&source).unwrap();
    assert!(text.contains("succ.take"));
    assert!(!text.contains("succ.erase"));
}

#[test]
fn oir2_c3a_known_higher_order_and_defer_boundaries() {
    for source in [
        include_str!("fixtures/p2_4/nested-callables.ok"),
        include_str!("fixtures/p2_4/generic-callables.ok"),
        concat!("module app; object Box<T> { has value is T; Box(:value); } ",
            "main() is int { Box(value: () is int { 1; }); 0; }"),
        concat!("module app; main() is int { let f = (x is int) is int { x; }; ",
            "let g = (h is (int) is int) is int { h(7); }; g(f); }"),
        concat!("module app; main() is int { let f = () is int { 1; }; ",
            "let g = () is int { defer f(); 7; }; g(); }"),
        concat!("module app; object A { has n is int; A(:n); ",
            "make() is () is int { () is int { defer self.n; 7; }; } } main() is int { A(n: 1).make()(); }"),
        concat!("module app; enum Stop is Error = (now); main() is int { ",
            "let f = () is int { defer 1; throw Stop.now; }; try { f(); } catch e is Stop { 7; } }"),
    ] {
        super::print_oir2(source).unwrap_or_else(|error| panic!("{source}: {error}"));
    }
}

#[test]
fn oir2_c3a_golden_inventory_and_snapshot_operands() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lowering/oir2");
    for directory in ["fixtures", "goldens"] {
        let mut actual: Vec<_> = std::fs::read_dir(root.join(format!("{directory}/p2_4/anonymous"))).unwrap()
            .map(|entry| entry.unwrap().path().file_stem().unwrap().to_str().unwrap().to_owned()).collect();
        actual.sort();
        assert_eq!(actual, CASES);
    }
    let source = std::fs::read_to_string(root.join("fixtures/p2_4/anonymous/closure-snapshot.ok")).unwrap();
    super::with_analysis(&source, |analysis| {
        let module = super::lower_to_oir2(analysis).unwrap();
        super::verify_high(&module, "snapshot operands");
        let entry = module.functions.iter().find(|(_, function)| function.symbol == "main").unwrap().1;
        let body = entry.body.as_ref().unwrap();
        let captured = body.insts.iter().find_map(|(_, inst)| {
            if let ir::Operation::Core(ir::CoreOp::FnClosure { captures, packet: None, .. }) = &inst.op {
                Some(captures[0])
            } else {
                None
            }
        }).unwrap();
        let rebound = body.insts.iter().find_map(|(_, inst)| {
            if let ir::Operation::Lexical(ir::LexicalOp::Rebind { value, .. }) = inst.op {
                Some(value)
            } else {
                None
            }
        }).unwrap();
        assert_ne!(captured, rebound, "closure must retain the pre-rebind SSA snapshot");
    });
}
