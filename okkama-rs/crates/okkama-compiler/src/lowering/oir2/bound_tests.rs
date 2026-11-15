//! C4 high-only source contracts; runtime and Own evidence remain separate gates.
use crate::oir2 as ir;

const CASES: &[&str] = &[
    "bound-generic", "bound-known-join", "bound-object", "bound-parameter", "bound-pending-construction",
    "bound-pending-successor",
    "bound-role", "bound-role-generic", "bound-role-overloaded", "bound-snapshot", "bound-successor",
];

#[test]
fn oir2_c4_bound_source_goldens() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lowering/oir2");
    for directory in ["fixtures", "goldens"] {
        if directory == "goldens" && std::env::var_os("OKKAMA_OIR2_P2_WRITE_GOLDENS").is_some() {
            continue;
        }
        let mut names: Vec<_> = std::fs::read_dir(root.join(format!("{directory}/p2_4/bound"))).unwrap()
            .map(|entry| entry.unwrap().path().file_stem().unwrap().to_str().unwrap().to_owned()).collect();
        names.sort();
        assert_eq!(names, CASES);
    }
    for name in CASES {
        let source = std::fs::read_to_string(root.join(format!("fixtures/p2_4/bound/{name}.ok"))).unwrap();
        let actual = super::print_oir2(&source).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(actual, super::print_oir2(&source).unwrap(), "determinism: {name}");
        assert!(actual.lines().count() <= 200, "keep C4 goldens small: {name}");
        let directory = root.join("goldens/p2_4/bound");
        let path = directory.join(format!("{name}.oir"));
        if std::env::var_os("OKKAMA_OIR2_P2_WRITE_GOLDENS").is_some() {
            std::fs::create_dir_all(directory).unwrap();
            let wrapped = actual.replace(", read#slot", ",\n        read#slot");
            let wrapped = super::cap_tests::wrap_high(&wrapped);
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
fn oir2_c4_ordered_set_producer() {
    let source = include_str!("fixtures/p2_4/ordered-set.ok");
    let module = super::lower_std(source).unwrap();
    super::verify_high(&module, "small ordered-set producer");
    let text = ir::text::print(&module).unwrap();
    let parsed = ir::text::parse(&text).unwrap();
    super::verify_high(&parsed, "small ordered-set roundtrip");
    assert!(ir::text::normalized_eq(&module, &parsed).unwrap());
    let second = super::lower_std(source).unwrap();
    super::verify_high(&second, "small ordered-set deterministic lowering");
    assert_eq!(text, ir::text::print(&second).unwrap());
    let mut role_binds = 0;
    for (_, function) in module.functions.iter() {
        if let Some(body) = &function.body {
            for (_, inst) in body.insts.iter() {
                if let ir::Operation::Core(ir::CoreOp::FnBindRole { role, slot, packet: None, .. }) = inst.op {
                    let declaration = module.declarations.get(role).unwrap();
                    if declaration.name == "collections/set::Set<int>"
                        && function.symbol == "collections/set::Intersection<int>.cursor#overload()" {
                        assert_eq!(slot, 2, "exact Set<int>.membership slot");
                        role_binds += 1;
                    }
                }
            }
        }
    }
    assert!(role_binds > 0, "real Intersection.cursor must bind right.membership");
    eprintln!("C4 small ordered-set: verified high, {} functions, {} printed lines",
        module.functions.len(), text.lines().count());
}

#[test]
fn oir2_c4_snapshot_and_pending_guard_operands() {
    let source = include_str!("fixtures/p2_4/bound/bound-snapshot.ok");
    super::with_analysis(source, |analysis| {
        let module = super::lower_to_oir2(analysis).unwrap();
        super::verify_high(&module, "bound snapshot operands");
        let function = module.functions.iter().find(|(_, function)| function.symbol == "main").unwrap().1;
        let body = function.body.as_ref().unwrap();
        let snapshot = body.insts.iter().find_map(|(_, inst)| {
            if let ir::Operation::Core(ir::CoreOp::FnBind { receiver, packet: None, .. }) = inst.op {
                Some(receiver)
            } else {
                None
            }
        }).unwrap();
        let (binding, rebound) = body.insts.iter().find_map(|(_, inst)| {
            if let ir::Operation::Lexical(ir::LexicalOp::Rebind { binding, value, .. }) = inst.op {
                Some((binding, value))
            } else {
                None
            }
        }).unwrap();
        assert!(body.insts.iter().any(|(_, inst)| matches!(inst.op,
            ir::Operation::Lexical(ir::LexicalOp::Bind { binding: b, value }) if b == binding && value == snapshot)));
        assert_ne!(snapshot, rebound, "fn.bind retains the pre-rebind SSA receiver");
    });
    let source = include_str!("fixtures/p2_4/bound/bound-pending-construction.ok");
    super::with_analysis(source, |analysis| {
        let module = super::lower_to_oir2(analysis).unwrap();
        super::verify_high(&module, "bound pending guard");
        let (target, bound_body) = module.functions.iter().find_map(|(_, function)| {
            let body = function.body.as_ref()?;
            body.insts.iter().find_map(|(_, inst)| {
                if let ir::Operation::Core(ir::CoreOp::FnBind { function, .. }) = inst.op {
                    Some((function, body))
                } else {
                    None
                }
            })
        }).unwrap();
        assert!(!bound_body.insts.iter().any(|(_, inst)| matches!(inst.op,
            ir::Operation::Core(ir::CoreOp::ObjPending { .. }))), "creation must not invoke the guard");
        let target = module.functions.get(target).unwrap().body.as_ref().unwrap();
        let entry = target.blocks.get(target.entry).unwrap();
        assert!(matches!(target.insts.get(entry.insts[0]).unwrap().op,
            ir::Operation::Core(ir::CoreOp::ObjPending { .. })));
        assert!(target.blocks.iter().any(|(_, block)| matches!(block.term.kind,
            ir::TermKind::Trap { code: ir::TrapCode::Pending, .. })));
    });
}

fn frontend_errors(source: &str) -> Vec<crate::analysis::SemanticError> {
    let source = crate::SourceFile::new("bound.ok", source).unwrap();
    let program = crate::syntax::parse(&source, &Default::default()).unwrap();
    crate::analysis::analyze(vec![crate::analysis::SourceModule {
        path: "app".into(), program: &program, imports: vec![],
    }]).unwrap_err()
}

#[test]
fn oir2_c4_frontend_must_use_and_hidden_boundaries() {
    let counter = concat!("module app; object Counter { has n is int; Counter(:n); ",
        "read() is int { n; } bumped() is ^Counter { ^n = n + 1; } } ");
    for body in [
        "let later = Counter(n: 1).bumped(); let read is () is int = later.read; 0;",
        "let later = Counter(n: 1).bumped(); let read is () is int = later.read; let alias = read; 0;",
    ] {
        let errors = frontend_errors(&format!("{counter} main() is int {{ {body} }}"));
        assert!(errors.iter().any(|error| error.message.to_lowercase().contains("successor")), "{errors:?}");
    }
    let successor = include_str!("fixtures/p2_4/bound/bound-successor.ok");
    let text = super::print_oir2(successor).unwrap();
    assert!(text.contains("fn() -> ^app.Counting"));
    assert!(text.contains("succ.take"));
    assert!(!text.contains("succ.erase"));
    let unused = successor.replace("    later.read();", "    0;");
    let errors = frontend_errors(&unused);
    assert!(errors.iter().any(|error| error.message.to_lowercase().contains("successor")), "{errors:?}");
    let source = concat!("module app; object A { A(); hidden read() is int { 7; } ",
        "make() is () is int { self.read; } } main() is int { A().make()(); }");
    super::print_oir2(source).unwrap();
    let errors = frontend_errors(concat!("module app; object A { A(); hidden read() is int { 7; } } ",
        "main() is int { let read is () is int = A().read; read(); }"));
    assert!(errors.iter().any(|error| error.message.contains("Hidden action")), "{errors:?}");
}

/// Open ABI entities: the protocol and the closure cleanup intrinsics.
fn open_abi(text: &str) -> (usize, usize) {
    (text.matches("protocol P").count(), text.matches("[intrinsic \"oir.sync_closure.cleanup\"]").count())
}

#[test]
fn oir2_c4_closed_fields_and_open_boundaries() {
    // #569 C3b: an invocation is open exactly where own sees no finite origin in
    // the invoking body: a field read, a bound role action of an opaque receiver
    // and a parameter. Each such module carries the open callable ABI.
    for source in [
        concat!("module app; object Hold { has f is () is int; Hold(:f); call() is int { f(); } } ",
            "main() is int { Hold(f: () is int { 7; }).call(); }"),
        concat!("module app; object Hold { has f is () is int; Hold(:f); call() is int { f(); } } ",
            "object Apply { Apply(); call(h is Hold) is int { h.call(); } } ",
            "main() is int { Hold(f: () is int { 7; }).call(); }"),
        concat!("module app; role Apply { run(f is () is int) is int; } ",
            "object A does Apply { A(); run(f is () is int) is int { f(); } } ",
            "object Open { Open(); call(f is () is int) is int { let a is Apply = A(); ",
            "let run = a.run; run(f); } } main() is int { 0; }"),
    ] {
        let text = super::print_oir2(source).unwrap_or_else(|error| panic!("{error}"));
        let (protocols, cleanups) = open_abi(&text);
        assert_eq!(protocols, 1, "{text}");
        assert!(cleanups >= 1, "{text}");
    }
    // The bound action invoked where it is bound has a finite known origin: only
    // the parameter invocation inside `add` is open.
    let known = concat!("module app; object A { A(); add(f is () is int) is int { f(); } } ",
        "main() is int { let run is (() is int) is int = A().add; run(() is int { 7; }); }");
    super::with_analysis(known, |analysis| {
        let module = super::lower_to_oir2(analysis).unwrap();
        let descriptors = |symbol: &str| {
            let (_, function) = module.functions.iter().find(|(_, function)| function.symbol == symbol).unwrap();
            function.body.as_ref().unwrap().insts.iter()
                .filter(|(_, inst)| matches!(inst.op, ir::Operation::Core(ir::CoreOp::TypeOf { .. }))).count()
        };
        assert_eq!(descriptors("main"), 0);
        assert_eq!(descriptors("app.A.add#0"), 1);
    });
}

/// The actual OrderedSet<int> keeps a role-valued once cache over plain implementations: once data since
/// #717 (Bible §13.13, #520), so its High verifies.
#[test]
fn oir2_c4_ordered_set_cache_verifies() {
    let source = include_str!("fixtures/p2_4/ordered-set-cached.ok");
    let module = super::lower_std(source).unwrap();
    // The runtime catalogue trusts the open-callable cleanup of its open invocations.
    let catalog = super::P1NativeSummaries::for_module(&module).unwrap();
    let context = ir::VerifyContext { external_contracts: &catalog, ..ir::VerifyContext::default() };
    ir::verify_high_with(&module, &ir::VerifyOptions::default(), context).unwrap_or_else(|report| panic!("{report:?}"));
}

const ESCAPED_ROLE_CALL: &str = "module app v1.0.0;
role Named { name() is string; }
object Child {
    has seen is string;
    Child(parent is Named) { seen = parent.name(); }
}
object Parent does Named {
    has label is string;
    has child is Maybe<Child>;
    Parent(label is string) {
        self.label = label;
        child = Maybe.some(Child(self));
    }
    name() is string { label; }
}
object Plain does Named {
    Plain();
    name() is string { \"plain\"; }
}
main() is int {
    let n = Parent(\"p\").name();
    0;
}";

/// §11.4, #271: a role call hides the concrete receiver, so only the implementation entry can see that the
/// receiver's construction is pending; it is guarded like a bound role action. An object whose constructor never
/// lets `self` escape has no pending state and no guard.
#[test]
fn oir2_role_witness_entries_guard_a_pending_receiver() {
    super::with_analysis(ESCAPED_ROLE_CALL, |analysis| {
        let module = super::lower_to_oir2(analysis).unwrap();
        super::verify_high(&module, "role pending guard");
        let guarded = |symbol: &str| {
            let function = module.functions.iter().find(|(_, function)| function.symbol == symbol).unwrap().1;
            let body = function.body.as_ref().unwrap();
            let entry = body.blocks.get(body.entry).unwrap();
            matches!(body.insts.get(entry.insts[0]).unwrap().op, ir::Operation::Core(ir::CoreOp::ObjPending { .. }))
                && body.blocks.iter().any(|(_, block)| matches!(block.term.kind,
                    ir::TermKind::Trap { code: ir::TrapCode::Pending, .. }))
        };
        assert!(guarded("app.Parent.name#0"));
        assert!(!guarded("app.Plain.name#0"));
    });
}
