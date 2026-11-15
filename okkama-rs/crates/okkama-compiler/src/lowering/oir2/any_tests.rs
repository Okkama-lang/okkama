//! #569 C6: `any` boxing, transport and its checked runtime descriptor, from
//! source through High, own and Low (the structural build gate and the full verifier).
use crate::oir2 as ir;

fn high(source: &str) -> ir::Module {
    super::with_analysis(source, |analysis| {
        let module = super::lower_to_oir2(analysis).unwrap_or_else(|error| panic!("{error}"));
        super::verify_high(&module, source);
        module
    })
}

fn boxes(module: &ir::Module) -> usize {
    super::own_case_tests::operations(module).into_iter()
        .filter(|op| matches!(op, ir::Operation::Core(ir::CoreOp::AnyBox { .. }))).count()
}

/// Functions binding the `any` dictionary with the registered `oir.any.cleanup`.
fn any_descriptors(module: &ir::Module) -> Vec<String> {
    let mut functions = vec![];
    for (_, function) in module.functions.iter() {
        let Some(body) = &function.body else { continue };
        for (_, inst) in body.insts.iter() {
            let ir::Operation::Core(ir::CoreOp::TypeOf { ty, descriptor }) = &inst.op else { continue };
            if module.types.get(*ty) != Ok(&ir::Type::Any) {
                continue;
            }
            let cleanup = module.functions.get(descriptor.cleanup).unwrap();
            assert!(cleanup.attrs.iter().any(|attr| {
                matches!(attr, ir::FunctionAttr::Intrinsic { name, .. } if name == ir::catalogue::ANY_CLEANUP)
            }));
            assert_eq!(descriptor.managed, ir::DescriptorManaged::Runtime);
            assert_eq!(descriptor.retention, ir::SummarySelection::OwnerGraph(0));
            functions.push(function.symbol.clone());
        }
    }
    functions
}

fn through_low(source: &str) -> ir::Module {
    super::own_case_tests::full(source)
}

#[test]
fn oir2_c6_box_at_a_binding_and_no_box_of_a_box() {
    let module = high("module app; main() is int { let value is any = 42; let again is any = value; 0; }");
    // `any -> any` keeps the existing value.
    assert_eq!(boxes(&module), 1);
    assert_eq!(any_descriptors(&module), ["main"]);
    through_low("module app; main() is int { let value is any = 42; let again is any = value; 0; }");
}

#[test]
fn oir2_c6_heterogeneous_literals_box_every_element() {
    for (body, expected) in [
        ("let mixed is @any = [1, \"two\", 3.0]; mixed.size();", 3),
        ("let row is %string:any = [\"id\": 7, \"name\": \"Ada\"]; row.size();", 2),
        ("let nested is @@any = [[1, \"a\"], [2.0], []]; nested.size();", 3),
        ("let columns is %string:@any = [\"a\": [1, \"b\"], \"c\": [true]]; columns.size();", 3),
    ] {
        let source = format!("module app; main() is int {{ {body} }}");
        let module = high(&source);
        assert_eq!(boxes(&module), expected, "{body}");
        let element = module.types.iter().find_map(|(id, ty)| (*ty == ir::Type::Any).then_some(id)).unwrap();
        // The collection holds the boxes: one retag of the literal never replaces them.
        assert!(module.types.iter().any(|(_, ty)| {
            matches!(ty, ir::Type::Array(inner) | ir::Type::Map(_, inner) if *inner == element)
        }), "{body}");
        through_low(&source);
    }
}

#[test]
fn oir2_c6_parameters_results_iteration_and_unpacking() {
    let source = concat!(
        "module app; object Relay { Relay(); echo(value is any) is any { value; } ",
        "count(values is @any) is int { values.size(); } } ",
        "main() is int { let mixed is @any = [1, \"two\"]; let echoed = Relay().echo(mixed[0]); ",
        "for mixed -> item { Relay().echo(item); } let copy is @any = mixed...; ",
        "Relay().count(copy) + Relay().count([3, \"x\"]); }",
    );
    let module = high(source);
    // Two elements of `mixed`, two of the argument literal; items and spread stay boxed.
    assert_eq!(boxes(&module), 4);
    let descriptors = any_descriptors(&module);
    for function in ["main", "app.Relay.echo", "app.Relay.count"] {
        assert!(descriptors.iter().any(|symbol| symbol.starts_with(function)), "{function}: {descriptors:?}");
    }
    through_low(source);
}

#[test]
fn oir2_c6_a_known_boxed_resource_keeps_its_owner() {
    let source = concat!(
        "module app; object Res { Res(); destroy { } } ",
        "object Relay { Relay(); echo(value is any) is any { value; } } ",
        "main() is int { let value is any = Res(); let again = Relay().echo(value); 0; }",
    );
    let module = through_low(source);
    assert_eq!(boxes(&module), 1);
}

#[test]
fn oir2_c6_functions_without_any_bind_no_descriptor() {
    let source = concat!("module app; object Taker { Taker(); ints(values is @int) is int { values.size(); } } ",
        "main() is int { Taker().ints([1, 2]); }");
    let module = high(source);
    assert!(any_descriptors(&module).is_empty());
    assert!(!module.functions.iter().any(|(_, function)| function.symbol == ir::catalogue::ANY_CLEANUP));
}

#[test]
fn oir2_c6_any_inside_a_generic_object() {
    let source = concat!("module app; object Box<T> { has value is T; Box(:value); get() is T { value; } } ",
        "main() is int { let one is any = 1; let boxed = Box(value: one); let inner = boxed.get(); 0; }");
    let module = high(source);
    assert!(boxes(&module) >= 1);
    assert!(any_descriptors(&module).iter().any(|symbol| symbol == "main"));
    through_low(source);
}

#[test]
fn oir2_c6_narrowing_out_of_any_stays_a_rejection() {
    let text = "module app; main() is int { let value is any = 1; let n is int = value; n; }";
    let source = crate::SourceFile::new("narrow.ok", text).unwrap();
    let program = crate::syntax::parse(&source, &Default::default()).unwrap();
    let errors = crate::analysis::analyze(vec![crate::analysis::SourceModule {
        path: "app".into(), program: &program, imports: vec![],
    }]).unwrap_err();
    assert!(errors.iter().any(|error| !error.unsupported && !error.warning), "{errors:?}");
}
