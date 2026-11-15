//! C8 tests keep full conformance inputs separate from small contract goldens.
use crate::analysis;
use crate::oir2 as ir;
use crate::syntax::{self, ParseOptions};
use crate::SourceFile;

const GOLDENS: &[&str] = &[
    "array-cursor", "frequency-amount", "list-size", "once-failure", "once-plain", "once-successor",
    "scalar-order", "sort-failure",
];

fn checked(module: &ir::Module) -> Result<(), ir::verify::VerifyReport> {
    assert_eq!(module.profile, ir::Profile::High);
    if module.functions.iter().all(|(_, function)| function.body.is_some()) {
        return ir::verify::verify_high(module).map(|_| ());
    }
    let catalog = super::native_summaries::P1NativeSummaries::for_module(module).unwrap();
    let context = ir::verify::VerifyContext { text_locations: None, external_contracts: &catalog };
    ir::verify::verify_with(module, &ir::verify::VerifyOptions::default(), context).map(|_| ())
}

fn roundtrip(module: &ir::Module, name: &str) -> String {
    checked(module).unwrap_or_else(|report| panic!("{name}: {report:#?}"));
    let text = ir::text::print(module).unwrap();
    let parsed = ir::text::parse(&text).unwrap();
    checked(&parsed).unwrap();
    assert!(ir::text::normalized_eq(module, &parsed).unwrap());
    assert_eq!(text, ir::text::print(&parsed).unwrap());
    text
}

#[test]
fn oir2_c8_std_goldens() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lowering/oir2");
    let source_names = |directory: &str| {
        let mut names: Vec<_> = std::fs::read_dir(root.join(directory)).unwrap().map(|entry| {
            entry.unwrap().path().file_stem().unwrap().to_str().unwrap().to_owned()
        }).collect();
        names.sort();
        names
    };
    assert_eq!(source_names("fixtures/p2_3/std"), GOLDENS);
    if std::env::var_os("OKKAMA_OIR2_P2_WRITE_GOLDENS").is_none() {
        assert_eq!(source_names("goldens/p2_3/std"), GOLDENS);
    }
    for name in GOLDENS {
        let source = std::fs::read_to_string(root.join("fixtures/p2_3/std").join(format!("{name}.ok"))).unwrap();
        let module = super::tests::lower_std(&source).unwrap_or_else(|error| panic!("{name}: {error}"));
        let actual = roundtrip(&module, name);
        assert!(actual.lines().count() <= 400, "{name}: keep contract fixtures small (#634)");
        let second = super::tests::lower_std(&source).unwrap();
        assert_eq!(actual, roundtrip(&second, name));
        let path = root.join("goldens/p2_3/std").join(format!("{name}.oir"));
        if std::env::var_os("OKKAMA_OIR2_P2_WRITE_GOLDENS").is_some() {
            std::fs::write(&path, &actual).unwrap();
        }
        let expected = std::fs::read_to_string(path).unwrap();
        checked(&ir::text::parse(&expected).unwrap()).unwrap();
        assert_eq!(actual, expected, "{name}");
        check_once(&module, name);
        eprintln!("C8 golden {name}: {} lines", actual.lines().count());
    }
}

fn check_once(module: &ir::Module, name: &str) {
    if !name.starts_with("once-") {
        return;
    }
    let mut slots = 0;
    for (_, declaration) in module.declarations.iter() {
        if let ir::DeclarationKind::Object { fields, .. } = &declaration.kind {
            slots += fields.iter().filter(|field| field.once).count();
        }
    }
    assert_eq!(slots, if name == "once-successor" { 2 } else { 1 });
    let mut loads = 0;
    let mut stores = 0;
    let mut computation_failure = false;
    for (_, function) in module.functions.iter() {
        let body = function.body.as_ref().unwrap();
        for (_, block) in body.blocks.iter() {
            for &id in &block.insts {
                match &body.insts.get(id).unwrap().op {
                    ir::Operation::Call(ir::CallOp::Direct { function: target, .. })
                        if name == "once-failure"
                            && module.functions.get(*target).unwrap().symbol.contains(".compute#") => {
                        let unwind = block.unwind.as_ref().expect("failure route");
                        let failure = body.blocks.get(unwind.target.block).unwrap();
                        assert!(matches!(failure.term.kind, ir::TermKind::Raise(_)));
                        assert!(!failure.insts.iter().any(|id| matches!(body.insts.get(*id).unwrap().op,
                            ir::Operation::Core(ir::CoreOp::OnceStore { .. }))));
                        computation_failure = true;
                    }
                    ir::Operation::Core(ir::CoreOp::OnceLoad { .. }) => loads += 1,
                    ir::Operation::Core(ir::CoreOp::OnceStore { value, .. }) => {
                        stores += 1;
                        let ir::Definition::InstResult { inst, .. } = body.values.get(*value).unwrap().def else {
                            panic!("only a successfully computed Some can be published");
                        };
                        assert!(matches!(body.insts.get(inst).unwrap().op,
                            ir::Operation::Core(ir::CoreOp::VariantNew { case: 0, .. })));
                    }
                    ir::Operation::Core(ir::CoreOp::ObjNew { declaration, fields: values, .. }) => {
                        let ir::DeclarationKind::Object { fields, .. } = &module.declarations
                            .get(*declaration).unwrap().kind else {
                                unreachable!("constructor owner");
                            };
                        for (field, value) in fields.iter().zip(values).filter(|(field, _)| field.once) {
                            let ir::Definition::InstResult { inst, .. } = body.values.get(*value).unwrap().def else {
                                panic!("constructor must initialize once storage");
                            };
                            assert!(matches!(body.insts.get(inst).unwrap().op,
                                ir::Operation::Core(ir::CoreOp::VariantNew { case: 1, ty, .. }) if ty == field.ty));
                        }
                    }
                    ir::Operation::Core(ir::CoreOp::ObjNext { updates, .. }) => {
                        let reset: Vec<_> = updates.iter().filter(|update| {
                            let ir::DeclarationKind::Object { fields, .. } = &module.declarations
                                .get(update.field.declaration).unwrap().kind else {
                                    unreachable!("successor owner");
                                };
                            fields[update.field.index as usize].once
                        }).collect();
                        assert_eq!(reset.len(), slots);
                        for update in reset {
                            let definition = body.values.get(update.value).unwrap().def;
                            let ir::Definition::InstResult { inst, .. } = definition else {
                                panic!("reset must be an empty slot");
                            };
                            assert!(matches!(body.insts.get(inst).unwrap().op,
                                ir::Operation::Core(ir::CoreOp::VariantNew { case: 1, .. })));
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    if name == "once-failure" {
        assert!(computation_failure);
    }
    assert_eq!(loads, slots);
    assert_eq!(stores, slots);
}

// Read the actual case.json source inventory, including every root file. This
// narrow reader deliberately accepts only the local corpus's simple path strings.
fn case_sources(json: &str) -> Vec<&str> {
    let array = json.split_once("\"sources\"").unwrap().1.split_once('[').unwrap().1;
    array.split_once(']').unwrap().0.split(',').map(|item| {
        let path = item.trim().strip_prefix('"').unwrap().strip_suffix('"').unwrap();
        assert!(!path.contains(['"', '\\']) && path.ends_with(".ok"));
        path
    }).collect()
}

fn lower_case(name: &str) -> Result<ir::Module, super::LowerError> {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../conformance/cases/L09-stdlib-pure").join(name);
    let json = std::fs::read_to_string(directory.join("case.json")).unwrap();
    let files: Vec<_> = case_sources(&json).iter().map(|path| {
        SourceFile::new(*path, std::fs::read_to_string(directory.join(path)).unwrap()).unwrap()
    }).collect();
    let programs: Vec<_> = files.iter().map(|file| syntax::parse(file, &ParseOptions::default()).unwrap()).collect();
    let roots: Vec<_> = programs.iter().zip(&files).map(|(program, file)| {
        (program, file.name(), Some(file))
    }).collect();
    let graph = crate::lowering::modules::load_graph(&roots, &mut super::tests::StdLoader).unwrap();
    let sources = crate::lowering::modules::source_modules(&graph, &roots);
    let analysis = analysis::analyze_files(sources, roots.len()).unwrap_or_else(|errors| panic!("{name}: {errors:?}"));
    super::lower_to_oir2(&analysis)
}

#[test]
fn oir2_c8_l09_sources() {
    for name in ["collections.persistent-list", "collections.frequencies", "collections.heap-and-deque",
        "compare.builtin-order", "compare.failing-order-stops-the-sort", "collections.ordered-sets"] {
        match lower_case(name) {
            Ok(module) => {
                match checked(&module) {
                    Ok(()) => {
                        roundtrip(&module, name);
                        eprintln!("C8 {name}: verified high");
                    }
                    Err(report) => {
                        eprintln!("C8 {name}: {report:#?}");
                        assert!(matches!(name, "collections.frequencies" | "collections.heap-and-deque"
                            | "compare.failing-order-stops-the-sort" | "collections.ordered-sets"));
                        assert!(report.diagnostics.iter().all(|diagnostic| {
                            diagnostic.rule == ir::verify::RuleId::B111
                        }));
                        if name == "compare.failing-order-stops-the-sort" {
                            assert_eq!(report.failure, ir::verify::VerifyFailure::Incomplete);
                        } else {
                            assert!(!report.diagnostics.is_empty(), "role-valued caches need the #565 plain proof");
                        }
                        for diagnostic in &report.diagnostics {
                            if let ir::verify::EntityLocation::Declaration(id) = diagnostic.primary.entity {
                                let declaration = module.declarations.get(id).unwrap();
                                eprintln!("C8 {name}: {} in {}", diagnostic.rule.as_str(), declaration.name);
                            }
                        }
                    }
                }
            }
            Err(error) => {
                eprintln!("C8 {name}: {error}");
                panic!("unexpected producer blocker in {name}: {error}");
            }
        }
    }
}

#[test]
fn oir2_c8_once_resources_stay_frontend_rejections() {
    for name in ["once-generic", "once-role"] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../tests/ownership-corpus/rejected").join(format!("{name}.ok"));
        let text = std::fs::read_to_string(path).unwrap();
        let file = SourceFile::new(name, &text).unwrap();
        let program = syntax::parse(&file, &ParseOptions::default()).unwrap();
        let errors = crate::lowering::check_linked(&program, name, &mut super::tests::StdLoader)
            .expect_err("public frontend check must reject resource storage")
            .expect("semantic rejection, not a loader failure");
        assert!(errors.iter().all(|error| !error.error.unsupported));
        assert!(errors.iter().any(|error| error.error.message.contains("once let cannot keep a resource")),
            "{name}: {errors:?}");
        let error = super::tests::lower_std(&text).expect_err("High also refuses known resources");
        assert!(matches!(error, super::LowerError::InvalidThir { .. }), "{error}");
        assert!(error.to_string().contains("once let cannot keep a resource"));
    }
}
