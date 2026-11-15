//! Source High evidence only: no runner, ownership elaboration or async execution.
use crate::analysis;
use crate::oir2 as ir;
use crate::syntax::{self, ParseMode, ParseOptions};
use crate::SourceFile;

const CASES: &[(&str, &str, bool)] = &[
    ("assertion-failure", "assertion-failure", true),
    ("async", "async", true),
    ("empty", "empty", true),
    ("fake-successor", "fake-successor", true),
    ("isolation", "isolation", true),
    ("production", "fake-successor", false),
];

fn lower(text: &str, test_mode: bool) -> super::Result<ir::Module> {
    let source = SourceFile::new("test-profile.ok", text).unwrap();
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
    super::lower_to_oir2(&analysis)
}

pub(super) fn verify(module: &ir::Module, context: &str) {
    assert_eq!(module.profile, ir::Profile::High);
    let catalog = super::native_summaries::P1NativeSummaries::for_module(module).unwrap();
    let execution = if module.tests.is_some() { ir::verify::ExecutionMode::Tests }
        else { ir::verify::ExecutionMode::Production };
    let options = ir::verify::VerifyOptions { execution, ..Default::default() };
    let context_with_catalog = ir::verify::VerifyContext { text_locations: None, external_contracts: &catalog };
    ir::verify::verify_with(module, &options, context_with_catalog)
        .unwrap_or_else(|report| panic!("{context}: {report:#?}"));
}

fn fixture(name: &str, test_mode: bool) -> ir::Module {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("src/lowering/oir2/fixtures/p2_4/tests/{name}.ok"));
    lower(&std::fs::read_to_string(path).unwrap(), test_mode).unwrap_or_else(|error| panic!("{name}: {error}"))
}

#[test]
fn oir2_c8a_source_goldens() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lowering/oir2");
    for (name, source, test_mode) in CASES {
        let first = fixture(source, *test_mode);
        verify(&first, name);
        let text = ir::text::print(&first).unwrap();
        let parsed = ir::text::parse(&text).unwrap();
        verify(&parsed, name);
        assert!(ir::text::normalized_eq(&first, &parsed).unwrap());
        let second = fixture(source, *test_mode);
        verify(&second, name);
        assert_eq!(text, ir::text::print(&second).unwrap(), "determinism: {name}");
        assert!(text.lines().count() <= 220, "keep C8a goldens small: {name}");
        let stored_text = wrap_high(&text);
        assert!(stored_text.lines().all(|line| line.len() <= 120), "line width: {name}");
        let path = root.join(format!("goldens/p2_4/tests/{name}.oir"));
        if std::env::var_os("OKKAMA_OIR2_C8A_WRITE_GOLDENS").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &stored_text).unwrap();
        }
        let stored = ir::text::parse(&std::fs::read_to_string(path).unwrap()).unwrap();
        verify(&stored, name);
        assert_eq!(text, ir::text::print(&stored).unwrap(), "stored: {name}");
    }
    for (directory, expected) in [
        ("fixtures", vec!["assertion-failure", "async", "empty", "fake-successor", "isolation"]),
        ("goldens", CASES.iter().map(|(name, _, _)| *name).collect()),
    ] {
        let mut names = std::fs::read_dir(root.join(format!("{directory}/p2_4/tests"))).unwrap()
            .map(|entry| entry.unwrap().path().file_stem().unwrap().to_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(names, expected);
    }
}

#[test]
fn oir2_c8a_core_catalogue_observes_and_continues() {
    let module = fixture("assertion-failure", true);
    verify(&module, "assertion contracts");
    let registry = ir::ContractRegistry::new(&module);
    let mut names = std::collections::BTreeSet::new();
    for (_, function) in module.functions.iter() {
        let body = function.body.as_ref().unwrap();
        for (_, inst) in body.insts.iter() {
            let name = match inst.op {
                ir::Operation::Core(ir::CoreOp::TestOk { .. }) => "ok",
                ir::Operation::Core(ir::CoreOp::TestIs { .. }) => "is",
                ir::Operation::Core(ir::CoreOp::TestIsnt { .. }) => "isnt",
                ir::Operation::Core(ir::CoreOp::TestFail { .. }) => "fail",
                _ => continue,
            };
            names.insert(name);
            let contract = registry.lookup(&inst.op).unwrap();
            assert_eq!(contract.entry_kind, ir::EntryKind::ObservationAdmissionStaysCaller);
            assert_eq!(contract.effects.failures, [ir::FailureKind::Program]);
            assert!(contract.unwind.is_some());
            assert!(contract.normal.results.is_empty());
            assert!(contract.operands.iter().all(|operand| operand.mode == ir::OperandMode::Observe));
            assert!(!contract.effects.suspends);
        }
    }
    assert_eq!(names.into_iter().collect::<Vec<_>>(), ["fail", "is", "isnt", "ok"]);
    let tests = module.tests.as_ref().unwrap();
    let body = module.functions.get(tests[0].function).unwrap().body.as_ref().unwrap();
    let block = body.blocks.get(body.entry).unwrap();
    assert!(matches!(block.term.kind, ir::TermKind::Ret { .. }));
    assert!(block.insts.iter().any(|id| matches!(body.insts.get(*id).unwrap().op,
        ir::Operation::Core(ir::CoreOp::TestOk { .. }))));
    let failed = module.functions.get(tests[1].function).unwrap().body.as_ref().unwrap();
    assert!(failed.blocks.get(failed.entry).unwrap().unwind.is_some());
}

#[test]
fn oir2_c8a_fake_layout_witnesses_and_fresh_cases() {
    let module = fixture("isolation", true);
    verify(&module, "isolation");
    let tests = module.tests.as_ref().unwrap();
    assert_eq!(tests.iter().map(|test| test.name.as_str()).collect::<Vec<_>>(), ["first", "second"]);
    assert_ne!(tests[0].function, tests[1].function);
    let (decl, fake) = module.declarations.iter().find(|(_, decl)| decl.name.contains("fake")).unwrap();
    let ir::DeclarationKind::Object { fields, witnesses, .. } = &fake.kind else {
        panic!("ordinary fake object");
    };
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].name, "count");
    assert!(fields[0].default.is_some());
    assert_eq!(witnesses.len(), 1);
    let mut ctor = None;
    for test in tests {
        let function = module.functions.get(test.function).unwrap();
        assert!(function.signature.params.is_empty() && function.signature.results.is_empty());
        assert!(function.attrs.is_empty());
        let body = function.body.as_ref().unwrap();
        let root = body.blocks.get(body.entry).unwrap().lexical_scope;
        assert_eq!(module.scopes.get(root).unwrap().function, test.function);
        assert_eq!(module.scopes.get(root).unwrap().parent, None);
        let calls: Vec<_> = body.insts.iter().filter_map(|(_, inst)| match inst.op {
            ir::Operation::Call(ir::CallOp::Direct { function, .. })
                if module.functions.get(function).unwrap().attrs.contains(&ir::FunctionAttr::Ctor(decl)) => {
                Some(function)
            }
            _ => None,
        }).collect();
        assert_eq!(calls.len(), 1, "each case constructs its own fake");
        assert_eq!(*ctor.get_or_insert(calls[0]), calls[0]);
    }
    let constructor = module.functions.get(ctor.unwrap()).unwrap().body.as_ref().unwrap();
    assert!(constructor.insts.iter().any(|(_, inst)| matches!(inst.op,
        ir::Operation::Core(ir::CoreOp::ObjNew { declaration, .. }) if declaration == decl)));
    assert!(constructor.insts.iter().any(|(_, inst)| matches!(inst.op,
        ir::Operation::Call(ir::CallOp::Direct { function, .. }) if Some(function) == fields[0].default)));
    assert!(module.functions.iter().flat_map(|(_, function)| function.body.iter())
        .flat_map(|body| body.insts.iter()).any(|(_, inst)| matches!(inst.op,
            ir::Operation::Core(ir::CoreOp::ObjNext { .. }))));
    assert!(!module.functions.iter().flat_map(|(_, function)| function.body.iter())
        .flat_map(|body| body.insts.iter()).any(|(_, inst)| matches!(inst.op,
            ir::Operation::Core(ir::CoreOp::FakeNew { .. }))));
    assert!(module.declarations.iter().all(|(_, declaration)| !matches!(declaration.kind,
        ir::DeclarationKind::Global { .. })));
}

#[test]
fn oir2_c8a_async_identity_empty_suite_and_production() {
    let asynchronous = fixture("async", true);
    verify(&asynchronous, "async High");
    let test = &asynchronous.tests.as_ref().unwrap()[0];
    let function = asynchronous.functions.get(test.function).unwrap();
    assert_eq!(function.attrs, [ir::FunctionAttr::Async]);
    assert!(function.async_creation_summary.is_some() && function.async_body_summary.is_some());
    let empty = fixture("empty", true);
    verify(&empty, "empty suite");
    assert_eq!(empty.tests, Some(vec![]));
    assert!(empty.requires.contains(&ir::Feature::Test));
    let production = fixture("fake-successor", false);
    verify(&production, "production");
    assert_eq!(production.tests, None);
    assert!(!production.requires.contains(&ir::Feature::Test));
    assert!(production.functions.iter().all(|(_, function)| !function.symbol.starts_with("$test")));
    assert!(production.declarations.iter().all(|(_, declaration)| !declaration.name.contains("fake")));
    let production_async = fixture("async", false);
    verify(&production_async, "production async stripped");
    assert_eq!(production_async.tests, None);
    assert!(production_async.functions.is_empty());
    assert!(!production_async.requires.contains(&ir::Feature::Async));
    let with_main = fixture("fake-successor", true);
    assert!(with_main.entry.is_some());
    assert!(with_main.tests.as_ref().unwrap().iter().all(|test| Some(test.function) != with_main.entry));
    assert!(ir::verify::verify_high(&with_main).is_err(), "test module rejected in production mode");
}

#[test]
fn oir2_c8a_skip_and_unregistered_typed_assertions_stay_unsupported() {
    for (name, args, message) in [
        ("skip", "\"later\"", "pending audited S1 contract"),
        ("close", "1.0, 1.0, 0.1", "typed test intrinsic outside C8a"),
        ("text", "\"a\", \"a\"", "typed test intrinsic outside C8a"),
        ("bytes", "[byte(1)], [byte(1)]", "typed test intrinsic outside C8a"),
    ] {
        let source = format!("module app; test \"unsupported\" {{ test.{name}({args}); }}");
        let error = lower(&source, true).unwrap_err();
        assert!(matches!(&error, super::LowerError::Unsupported { span, message: text }
            if !span.is_empty() && text.contains(message)), "{error}");
    }
}

#[test]
fn oir2_c8a_qualified_names_generic_fake_and_successor_assertions() {
    let source = concat!(
        "module app; role Echo<T> { echo(value is T) is T; ",
        "fake { echo(value is T) is T { value; } } test \"role\" { test.ok(true); } } ",
        "object Bottle { has opened is bool = false; Bottle(); ",
        "open() is ^Bottle { ^opened = true; } ",
        "test \"owner\" { let b = Bottle(); test.is(b.open(), b); } } ",
        "test \"generic\" { let e is Echo<int> = Echo.fake(); test.is(e.echo(3), 3); }",
    );
    let module = lower(source, true).unwrap();
    verify(&module, "qualified roots and generic fake");
    let tests = module.tests.as_ref().unwrap();
    assert_eq!(tests.iter().map(|test| test.name.as_str()).collect::<Vec<_>>(),
        ["Echo :: role", "Bottle :: owner", "generic"]);
    for test in tests {
        let function = module.functions.get(test.function).unwrap();
        assert!(function.attrs.is_empty());
        assert!(function.signature.params.is_empty());
    }
    assert!(module.declarations.iter().any(|(_, declaration)| declaration.name.contains("Echo<int>")));
    let roundtrip = ir::text::parse(&ir::text::print(&module).unwrap()).unwrap();
    verify(&roundtrip, "qualified roots roundtrip");
    assert!(ir::text::normalized_eq(&module, &roundtrip).unwrap());
}

#[test]
fn oir2_c8a_imported_helpers_keep_dynamic_context_without_importing_tests() {
    let library = SourceFile::new("library.ok", concat!(
        "module library; object Helper { Helper(); check() is void { test.ok(true); } } ",
        "test \"imported case\" { test.fail(\"must not be registered\"); }",
    )).unwrap();
    let root = SourceFile::new("root.ok", concat!(
        "module app; import \"library\" take Helper; ",
        "test \"root\" { Helper().check(); } main() is int { 13; }",
    )).unwrap();
    let program = syntax::prune::desugar_test_items(syntax::parse(&library, &ParseOptions::default()).unwrap());
    let root = syntax::prune::desugar_test_items(syntax::parse(&root, &ParseOptions::default()).unwrap());
    let analysis = analysis::analyze_mode(vec![
        analysis::SourceModule { path: "library".into(), program: &program, imports: vec![] },
        analysis::SourceModule { path: "app".into(), program: &root, imports: vec![(0, &root.imports[0])] },
    ], 1, true).unwrap();
    let module = super::lower_to_oir2(&analysis).unwrap();
    verify(&module, "imported helper");
    assert_eq!(module.tests.as_ref().unwrap().iter().map(|test| test.name.as_str()).collect::<Vec<_>>(), ["root"]);
    let helper = module.functions.iter().find(|(_, function)| function.symbol.contains(".check#")).unwrap().1;
    assert!(helper.body.as_ref().unwrap().insts.iter().any(|(_, inst)| matches!(inst.op,
        ir::Operation::Core(ir::CoreOp::TestOk { .. }))));
}

// Generated fake symbols and summary envelopes can exceed the stored line width.
// Break only at whitespace outside quoted identifiers; compare canonical text after parsing.
fn wrap_high(text: &str) -> String {
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

