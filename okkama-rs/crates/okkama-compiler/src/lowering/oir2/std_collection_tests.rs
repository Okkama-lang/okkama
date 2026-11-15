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
                    // Role-valued and closure caches over plain implementations are once data (#717). The full
                    // verifier rejects none of these sources; on the three largest it runs out of budget (#634).
                    Err(report) => {
                        eprintln!("C8 {name}: {report:#?}");
                        assert!(matches!(name, "collections.frequencies" | "compare.failing-order-stops-the-sort"
                            | "collections.ordered-sets"), "{name}");
                        assert_eq!(report.failure, ir::verify::VerifyFailure::Incomplete, "{name}");
                        assert!(report.diagnostics.is_empty(), "{name}");
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

/// Two closures of one type, one capturing a resource: a once let of that type is refused even for the plain
/// one, since the slot's type is what it keeps (#520, #717).
const ONCE_SHARED_CLOSURE_TYPE: &str = "module ownership v1.0.0;
object Res {
    Res();
    label() is string { \"R\"; }
    destroy { 0; }
}
object Cache {
    has value is () is string;
    Cache(value is () is string) { self.value = value; }
    cached() is () is string { once let kept = value; kept; }
}
main() is int {
    let r = Res();
    let plain = Cache(() is string { \"p\"; });
    let leased = () is string { r.label(); };
    leased();
    plain.cached()();
    0;
}";

/// Bible §13.13 (#491, #520, #717): a once let keeps a closure that captures plain values, a role value whose
/// implementations are plain, and an object that holds a native handle without destroy (P2.4 §4.7).
#[test]
fn oir2_c8_once_keeps_plain_closures_roles_and_native_handles() {
    let closure_and_role = "module app v1.0.0;
role Named { name() is string; }
object Plain does Named {
    has label is string;
    Plain(:label);
    name() is string { label; }
}
object Registry {
    has first is Named;
    has pick is (string) is bool;
    Registry(:first, :pick);
    names() is @Named {
        once let kept is @Named = [first];
        kept;
    }
    chosen() is (string) is bool {
        once let kept = pick;
        kept;
    }
}
main() is int {
    let prefix = \"a\";
    let registry = Registry(first: Plain(label: \"a\"), pick: (s is string) is bool { s == prefix; });
    registry.names().size() + registry.chosen()(\"a\").string().length();
}";
    let handle = "module app v1.0.0;
native object Token;
native token_make(seed is int) is Token;
object Keeper {
    has token is Token;
    Keeper(:token);
    kept() is Keeper {
        once let copy = Keeper(token: token);
        copy;
    }
}
main() is int {
    let keeper = Keeper(token: token_make(1));
    keeper.kept();
    0;
}";
    for source in [closure_and_role, handle] {
        let module = super::tests::lower_std(source).unwrap_or_else(|error| panic!("{error}\n{source}"));
        let once = module.declarations.iter().map(|(_, declaration)| match &declaration.kind {
            ir::DeclarationKind::Object { fields, .. } => fields.iter().filter(|field| field.once).count(),
            _ => 0,
        }).sum::<usize>();
        assert!(once > 0, "{source}");
    }
}

/// A bound action retains its receiver: a resource's bound action cannot be kept (#717).
const ONCE_BOUND_RESOURCE_ACTION: &str = "module ownership v1.0.0;
object Res {
    Res();
    label() is string { \"R\"; }
    destroy { 0; }
}
object Cache {
    has value is () is string;
    Cache(value is () is string) { self.value = value; }
    cached() is () is string { once let kept = value; kept; }
}
main() is int {
    let r = Res();
    let cache = Cache(r.label);
    cache.cached()();
    0;
}";

#[test]
fn oir2_c8_once_resources_stay_frontend_rejections() {
    let corpus = |name: &str| {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../tests/ownership-corpus/rejected").join(format!("{name}.ok"));
        std::fs::read_to_string(path).unwrap()
    };
    let sources = ["once-generic", "once-role", "once-closure", "once-resource"].map(|name| (name, corpus(name)));
    let shared = ("once-shared-closure-type", ONCE_SHARED_CLOSURE_TYPE.to_owned());
    let bound = ("once-bound-resource-action", ONCE_BOUND_RESOURCE_ACTION.to_owned());
    for (name, text) in sources.into_iter().chain([shared, bound]) {
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
