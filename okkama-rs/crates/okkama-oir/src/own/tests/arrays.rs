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
