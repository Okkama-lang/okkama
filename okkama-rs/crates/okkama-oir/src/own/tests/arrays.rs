use super::*;
use crate::own::analyze::analyze_function;
use crate::own::domain::Shape;

#[test]
fn plain_arrays_preserve_positions_empty_arrays_and_observation_failures() {
    for element in [Type::Byte, Type::Char, Type::Int, Type::Str] {
        let mut module = ModuleBuilder::new("arrays", Profile::High);
        let element = module.intern_type(element).unwrap();
        let array = module.intern_type(Type::Array(element)).unwrap();
        let f = module.declare_function(function("array", vec![element], vec![])).unwrap();
        let root = scope(&mut module, f);
        let mut builder = module.function_builder(f, root).unwrap();
        with_unwind(&mut builder, root);
        let input = builder.block_params(builder.entry()).unwrap()[0];
        builder.emit_schema(Operation::Collection(CollectionOp::ArrayNew {
            values: vec![], ty: array, packet: None,
        })).unwrap();
        let values = builder.emit_schema(Operation::Collection(CollectionOp::ArrayNew {
            values: vec![input, input], ty: array, packet: None,
        })).unwrap()[0];
        builder.emit_schema(Operation::Collection(CollectionOp::ArrayLen { array: values })).unwrap();
        let index = builder.emit_schema(Operation::Const(Literal::Int("0".into()))).unwrap()[0];
        builder.emit_schema(Operation::Collection(CollectionOp::ArrayGet {
            array: values, index,
        })).unwrap();
        builder.terminate(TermKind::Ret { values: vec![] }).unwrap();
        let body = builder.finish().unwrap();
        module.define_function(f, body).unwrap();
        let module = module.finish().unwrap();
        verify_high(&module).unwrap();
        let options = OwnOptions::default();
        let output = elaborate_ownership(module, &TestNatives::default(), &options).unwrap();
        let flow = analyze_function(&output.module, f, &output.analysis, &options).unwrap();
        assert_eq!(flow.equations.nodes, vec![Shape::Empty]);
        for state in flow.before_instructions.values().chain(flow.before_edges.values()) {
            assert!(state.journals.is_empty());
            assert!(state.values.iter().all(|value| value.deps.is_empty()));
        }
        let output = rewrite_synchronous(output, &options).unwrap();
        verify_own(&output.module).unwrap();
        let body = output.module.functions.get(f).unwrap().body.as_ref().unwrap();
        let operations = body.insts.iter().map(|(_, inst)| &inst.op)
            .chain(body.blocks.iter().filter_map(|(_, block)| match &block.term.kind {
                TermKind::InvokeOwn { op, .. } => Some(op.as_ref()),
                _ => None,
            })).collect::<Vec<_>>();
        assert_eq!(operations.iter().filter(|op| matches!(op,
            Operation::Collection(CollectionOp::ArrayNew { packet: None, .. }))).count(), 2);
        assert!(operations.iter().any(|op| matches!(op,
            Operation::Collection(CollectionOp::ArrayNew { values, .. })
                if values.len() == 2 && values[0] == values[1])));
        assert!(body.blocks.iter().any(|(_, block)| matches!(&block.term.kind,
            TermKind::InvokeOwn { op, .. }
                if matches!(op.as_ref(), Operation::Collection(CollectionOp::ArrayGet { .. })))));
        assert!(operations.iter().all(|op| !matches!(op, Operation::Ownership(_))));
        assert!(body.values.iter().all(|(_, value)| {
            !matches!(output.module.types.get(value.ty), Ok(Type::Array(_)))
                || value.facts == ValueFacts::Plain { certificate: PlainCertificate::ClosedGraph { ty: value.ty } }
        }));
    }
}

// Managed and residual element graphs take the collection rules: no packet,
// full deps through the retention formula, and observations only borrow.
#[test]
fn managed_and_residual_element_graphs_follow_the_collection_rules_for_all_three_operations() {
    for residual in [false, true] {
        for operation in 0..3 {
            let mut module = ModuleBuilder::new("not_plain", Profile::High);
            let f = module.declare_function(function("f", vec![], vec![])).unwrap();
            let element = if residual {
                let ty = module.intern_type(Type::Parameter(TypeParameterRef {
                    owner: GenericOwner::Function(f), index: 0,
                })).unwrap();
                module.raw_mut().functions.get_mut(f).unwrap().type_params.push(TypeParameter {
                    name: "T".into(), bounds: vec![],
                });
                ty
            } else {
                let (ty, destroy) = resource(&mut module, "R");
                empty_body(&mut module, destroy);
                ty
            };
            let array = module.intern_type(Type::Array(element)).unwrap();
            module.raw_mut().functions.get_mut(f).unwrap().signature.params = vec![element, array];
            let root = scope(&mut module, f);
            let mut builder = module.function_builder(f, root).unwrap();
            with_unwind(&mut builder, root);
            let params = builder.block_params(builder.entry()).unwrap().to_vec();
            let index = builder.emit_schema(Operation::Const(Literal::Int("0".into()))).unwrap()[0];
            let op = match operation {
                0 => CollectionOp::ArrayNew { values: vec![params[0]], ty: array, packet: None },
                1 => CollectionOp::ArrayLen { array: params[1] },
                _ => CollectionOp::ArrayGet { array: params[1], index },
            };
            builder.emit_schema(Operation::Collection(op)).unwrap();
            builder.terminate(TermKind::Ret { values: vec![] }).unwrap();
            let body = builder.finish().unwrap();
            module.define_function(f, body).unwrap();
            let module = module.finish().unwrap();
            let options = OwnOptions::default();
            let analysis = OwnAnalysis { types: classify_test(&module, &options), ..OwnAnalysis::default() };
            assert_ne!(analysis.types[&array].class, Classification::Plain);
            let flow = analyze_function(&module, f, &analysis, &options).unwrap();
            let body = module.functions.get(f).unwrap().body.as_ref().unwrap();
            let (_, inst) = body.insts.iter().find(|(_, inst)| matches!(inst.op, Operation::Collection(_))).unwrap();
            let after = flow.before_edges.values().next().unwrap();
            let element_deps = &after.values[params[0].as_u32() as usize].deps;
            let array_deps = &after.values[params[1].as_u32() as usize].deps;
            if let Some(result) = inst.results.first().filter(|_| operation != 1) {
                let expected = if operation == 0 { element_deps } else { array_deps };
                assert_eq!(&after.values[result.as_u32() as usize].deps, expected);
            }
            if residual {
                continue;
            }
            verify_high(&module).unwrap();
            let output = elaborate_ownership(module, &TestNatives::default(), &options).unwrap();
            let output = rewrite_synchronous(output, &options).unwrap();
            verify_own(&output.module).unwrap();
            let body = output.module.functions.get(f).unwrap().body.as_ref().unwrap();
            assert!(body.values.iter().all(|(_, value)| {
                !matches!(output.module.types.get(value.ty), Ok(Type::Array(_)))
                    || matches!(value.facts, ValueFacts::Managed { .. })
            }));
        }
    }
}

/// A builder's owner is the scope of its `builder.new`: transitions and edge
/// arguments carry it to every buffer version, however deep the transition.
#[test]
fn builder_owner_is_the_scope_of_builder_new_on_every_version() {
    let module = crate::text::parse(r#"oir 2 high
module owners
scope S action in @f
scope SI iteration in @f parent S
scope SB block in @f parent SI
func @f(%c: bool) -> void {
^entry in S:
    br ^iteration
^iteration in SI:
    %initial: builder<int> = builder.new element int
    br ^body(%initial)
^body(%current: builder<int>) in SB:
    %item: int = const 1
    %next: builder<int> = builder.push %current, %item
    br ^finish(%next)
^finish(%last: builder<int>) in SI:
    %array: array<int> = builder.finish %last
    cond %c, ^iteration, ^out
^out in S:
    ret
}
"#).unwrap();
    let (f, function) = module.functions.iter().next().unwrap();
    let body = function.body.as_ref().unwrap();
    // Scopes in declaration order: S, SI (parent S), SB (parent SI).
    let scopes: Vec<_> = module.scopes.iter().map(|(id, _)| id).collect();
    assert_eq!(module.scopes.get(scopes[2]).unwrap().parent, Some(scopes[1]));
    assert_eq!(module.scopes.get(scopes[1]).unwrap().parent, Some(scopes[0]));
    let scope = |name: &str| match name {
        "S" => scopes[0],
        "SI" => scopes[1],
        _ => scopes[2],
    };
    let owners = crate::own::arrays::builder_owners(&module, body).unwrap();
    let buffers: Vec<_> = body.values.iter().filter(|(_, value)| {
        matches!(module.types.get(value.ty), Ok(Type::Builder(_)))
    }).map(|(id, _)| id).collect();
    assert_eq!(buffers.len(), 4);
    for buffer in &buffers {
        assert_eq!(owners.get(buffer), Some(&scope("SI")), "{f:?} {buffer:?}");
    }
    let inner = crate::own::arrays::inner_scopes(&module, scope("SB"), scope("SI")).unwrap();
    assert_eq!(inner, vec![scope("SB")]);
    assert!(crate::own::arrays::inner_scopes(&module, scope("S"), scope("SI")).is_err());
}

/// Versions of builders from sibling scopes meeting at one parameter belong
/// to the nearest common scope, which outlives both.
#[test]
fn builder_owner_join_takes_the_nearest_common_scope() {
    let module = crate::text::parse(r#"oir 2 high
module owner_join
scope S action in @f
scope SA block in @f parent S
scope SB block in @f parent S
func @f(%c: bool) -> void {
^entry in S:
    cond %c, ^left, ^right
^left in SA:
    %a: builder<int> = builder.new element int
    br ^join(%a)
^right in SB:
    %b: builder<int> = builder.new element int
    br ^join(%b)
^join(%merged: builder<int>) in S:
    builder.abandon %merged
    ret
}
"#).unwrap();
    let (_, function) = module.functions.iter().next().unwrap();
    let body = function.body.as_ref().unwrap();
    let owners = crate::own::arrays::builder_owners(&module, body).unwrap();
    let root = body.blocks.get(body.entry).unwrap().lexical_scope;
    let merged = body.blocks.iter().flat_map(|(_, block)| block.params.iter().copied()).find(|value| {
        matches!(module.types.get(body.values.get(*value).unwrap().ty), Ok(Type::Builder(_)))
    }).unwrap();
    assert_eq!(owners.get(&merged), Some(&root));
    for (_, block) in body.blocks.iter() {
        for inst in &block.insts {
            let instruction = body.insts.get(*inst).unwrap();
            if matches!(instruction.op, Operation::Collection(CollectionOp::BuilderNew { .. })) {
                assert_eq!(owners.get(&instruction.results[0]), Some(&block.lexical_scope));
            }
        }
    }
}
