use super::*;

fn fixture() -> Program {
    let mut f = tests::empty_function();
    let array = Type::Array(ArrayType { leaf: ArrayLeaf::Str, depth: 1 });
    f.registers = vec![Type::Str, array, Type::Int, Type::Str, Type::Failure];
    f.results = vec![Type::Str];
    f.blocks.extend([
        Block { pc: 4, params: vec![Location::Register(3)] },
        Block { pc: 5, params: vec![Location::Register(4)] },
    ]);
    f.proofs.entry = Some(EntryContract { failures: 2, ..EntryContract::default() });
    f.code = vec![
        Instruction::Data { op: DataOp::Const, inputs: vec![], output: Some(0),
            immediate: Immediate::Constant(Constant::Str("rooted".into())) },
        Instruction::Data { op: DataOp::ArrayNew, inputs: vec![0], output: Some(1), immediate: Immediate::None },
        Instruction::Data { op: DataOp::Const, inputs: vec![], output: Some(2),
            immediate: Immediate::Constant(Constant::Int("0".into())) },
        Instruction::Champ { op: Opcode::ArrayGet,
            arguments: vec![Argument::Location(Location::Register(1)), Argument::Location(Location::Register(2))],
            flow: Flow::Outcomes(vec![
                Outcome { tag: Tag::Normal, edge: Edge { target_pc: 4, assignments: vec![Assignment {
                    destination: Location::Register(3), source: Source::ProducedSlot(0),
                }] } },
                Outcome { tag: Tag::Unwind, edge: Edge { target_pc: 5, assignments: vec![Assignment {
                    destination: Location::Register(4), source: Source::ProducedSlot(0),
                }] } },
            ]) },
        Instruction::Ret(vec![3]),
        Instruction::Raise(4),
    ];
    let live = live_locations(&f).unwrap();
    f.proofs.roots = live.into_iter().enumerate().map(|(pc, locations)| RootRequirement {
        pc: pc as u32, locations: locations.into_iter().collect(),
    }).collect();
    Program { functions: vec![f], entry: Some(0), ..Program::default() }
}

#[test]
fn plain_array_roundtrip_and_load() {
    let p = fixture();
    verify_load(&p, &NoExternalContracts).unwrap();
    assert_eq!(decode(&encode(&p).unwrap()).unwrap(), p);
    assert_eq!(assemble(&disassemble(&p).unwrap()).unwrap(), p);
    for depth in [1, 2, u32::MAX] {
        let mut p = fixture();
        p.functions[0].registers.push(Type::Array(ArrayType { leaf: ArrayLeaf::Int, depth }));
        verify(&p).unwrap();
    }
}

#[test]
fn array_loader_rejects_shape_operands_results_and_missing_bounds_outcome() {
    let reject = |p: &Program, rule| {
        assert_eq!(verify_load(p, &NoExternalContracts).unwrap_err().rule, rule);
    };
    let mut p = fixture();
    p.functions[0].registers[1] = Type::Array(ArrayType { leaf: ArrayLeaf::Str, depth: 0 });
    reject(&p, "ARRAY_SHAPE");
    let mut p = fixture();
    p.functions[0].registers[1] = Type::Array(ArrayType { leaf: ArrayLeaf::Object(77), depth: 1 });
    reject(&p, "ARRAY_ELEMENT");
    let mut p = fixture();
    p.functions[0].registers[0] = Type::Int;
    p.functions[0].code[0] = Instruction::Data { op: DataOp::Const, inputs: vec![], output: Some(0),
        immediate: Immediate::Constant(Constant::Int("0".into())) };
    reject(&p, "ARRAY_NEW_TYPE");
    let mut p = fixture();
    p.functions[0].code[2] = Instruction::Data { op: DataOp::ArrayLen, inputs: vec![0], output: Some(2),
        immediate: Immediate::None };
    reject(&p, "ARRAY_LEN_TYPE");
    let mut p = fixture();
    let Instruction::Champ { arguments, .. } = &mut p.functions[0].code[3] else { unreachable!() };
    arguments[1] = Argument::Location(Location::Register(0));
    reject(&p, "ARRAY_GET_OPERANDS");
    let mut p = fixture();
    let Instruction::Champ { flow: Flow::Outcomes(outcomes), .. } = &mut p.functions[0].code[3] else {
        unreachable!()
    };
    outcomes.pop();
    reject(&p, "ARRAY_GET_OUTCOMES");
}

#[test]
fn array_loader_checks_transitive_element_graph_independently() {
    let mut p = fixture();
    let layout = |fields, resource| ObjectRuntimeLayout {
        storage_fields: fields, allocation_words: 2, identity_offset: None,
        has_destroy: false, may_reach_resource: resource, may_async_destroy: false,
        trace_layout: vec![], selection_layout: vec![],
    };
    p.objects = vec![
        Object { name: "plain_lie".into(), error_cases: vec![], enum_cases: 0, variant_cases: vec![],
            handle: false, fields: vec![],
            layout: layout(vec![Type::Object(1)], false) },
        Object { name: "resource".into(), error_cases: vec![], enum_cases: 0, variant_cases: vec![],
            handle: false, fields: vec![],
            layout: layout(vec![], true) },
    ];
    p.functions[0].registers[1] = Type::Array(ArrayType { leaf: ArrayLeaf::Object(0), depth: 1 });
    p.objects[0].layout.storage_fields = vec![p.functions[0].registers[1], Type::Object(1)];
    p.objects[0].layout.allocation_words = 3;
    p.objects[0].layout.trace_layout = vec![0, 1];
    // #622: an array may refer to resource objects (managed arrays), so the array's element graph is not
    // walked here. The array is classified by its leaf's retention flag, which the loader checks object by
    // object: the leaf that hides a resource field is rejected on its own.
    assert_eq!(verify_load(&p, &NoExternalContracts).unwrap_err().rule, "OBJECT_RETENTION");
}

#[test]
fn scalar_outcome_extensions_reject_wrong_kinds_and_service_operands() {
    let cases = [
        (Opcode::Fail, vec![Argument::Label(3), Argument::Location(Location::Register(0))], "FAIL_KIND"),
        (Opcode::Fail, vec![Argument::Label(0), Argument::Location(Location::Register(0))], "FAIL_OPERAND"),
        (Opcode::Catch, vec![Argument::Location(Location::Register(2)), Argument::Flag(true)], "CATCH_FAILURE"),
        (Opcode::Catch, vec![Argument::Location(Location::Register(4)), Argument::Flag(false)], "CATCH_TYPE"),
        (Opcode::Evaluate, vec![Argument::Evaluation(Evaluation {
            op: DataOp::ObjNew, result: Type::Str, immediate: Immediate::None,
        })], "EVALUATE_OPERATION"),
    ];
    for (op, arguments, rule) in cases {
        let mut p = fixture();
        let Instruction::Champ { op: old, arguments: inputs, .. } = &mut p.functions[0].code[3] else {
            unreachable!()
        };
        *old = op;
        *inputs = arguments;
        assert_eq!(verify_load(&p, &NoExternalContracts).unwrap_err().rule, rule);
    }
    let mut p = fixture();
    let f = &mut p.functions[0];
    f.services.push(ServiceKind::Deps);
    f.params.push(Location::Service(0));
    f.blocks[0].params.push(Location::Service(0));
    let Instruction::Champ { op, arguments, .. } = &mut f.code[3] else { unreachable!() };
    *op = Opcode::Evaluate;
    *arguments = vec![Argument::Evaluation(Evaluation {
        op: DataOp::RangeEnd, result: Type::Int, immediate: Immediate::None,
    }), Argument::Location(Location::Service(0))];
    assert_eq!(verify_load(&p, &NoExternalContracts).unwrap_err().rule, "EVALUATE_OPERAND");
}

#[test]
fn enum_case_checks_its_closed_catalog() {
    let mut p = fixture();
    p.objects.push(Object { name: "E".into(), fields: vec![], error_cases: vec!["E.one".into()], enum_cases: 0,
        variant_cases: vec![], handle: false,
        layout: ObjectRuntimeLayout {
            storage_fields: vec![Type::Int], allocation_words: 2, identity_offset: None,
            has_destroy: false, may_reach_resource: false, may_async_destroy: false,
            trace_layout: vec![], selection_layout: vec![],
        } });
    p.functions[0].registers[0] = Type::Object(0);
    p.functions[0].code[0] = Instruction::Data { op: DataOp::EnumCase, inputs: vec![], output: Some(0),
        immediate: Immediate::Updates(vec![0, 1]) };
    assert_eq!(verify_load(&p, &NoExternalContracts).unwrap_err().rule, "ENUM_CASE");
}

/// The fixture with one unused array register whose element is object 0 with `fields`.
fn element_array(fields: Vec<Type>, has_destroy: bool) -> Program {
    let mut p = fixture();
    let untraced = |ty: &Type| matches!(ty, Type::Int | Type::Bool | Type::Range | Type::Descriptor);
    let traced = fields.iter().enumerate().filter(|(_, ty)| !untraced(ty))
        .map(|(index, _)| index as u32).collect();
    let count = fields.len() as u32;
    p.objects.push(Object { name: "Element".into(), fields: vec![], error_cases: vec![], enum_cases: 0,
        variant_cases: vec![], handle: false,
        layout: ObjectRuntimeLayout {
            storage_fields: fields,
            allocation_words: (1 + count + 4 * u32::from(has_destroy)).max(2),
            identity_offset: has_destroy.then_some(8 * (1 + count)),
            has_destroy,
            may_reach_resource: has_destroy,
            may_async_destroy: false,
            trace_layout: traced,
            selection_layout: vec![],
        } });
    if has_destroy {
        let mut destroy = tests::empty_function();
        destroy.symbol = "destroy".into();
        destroy.registers = vec![Type::Object(0)];
        destroy.params = vec![Location::Register(0)];
        destroy.blocks[0].params = destroy.params.clone();
        destroy.proofs.entry = Some(EntryContract { destroy: Some(0), ..EntryContract::default() });
        p.functions.push(destroy);
    }
    p.functions[0].registers.push(Type::Array(ArrayType { leaf: ArrayLeaf::Object(0), depth: 1 }));
    p
}

#[test]
fn array_of_closed_plain_objects_loads() {
    let nested = Type::Array(ArrayType { leaf: ArrayLeaf::Str, depth: 2 });
    let p = element_array(vec![Type::Int, Type::Str, Type::Bool, Type::Range, Type::Bytes, nested], false);
    verify_load(&p, &NoExternalContracts).unwrap();
}

#[test]
fn array_element_fields_must_be_closed_plain_data() {
    for field in [Type::Failure, Type::Error, Type::Descriptor] {
        let p = element_array(vec![Type::Int, field], false);
        let failure = verify_load(&p, &NoExternalContracts).unwrap_err();
        assert_eq!(failure.rule, "ARRAY_PLAIN", "{field:?}: {failure}");
        assert!(failure.message.contains("not closed plain data"), "{field:?}: {failure}");
    }
}

#[test]
fn array_element_cannot_have_a_destroy() {
    // A destroy layout always reaches resources (verify: "destroy layout must reach self"), so this
    // element fails both conditions of the same ARRAY_PLAIN check.
    let p = element_array(vec![Type::Int], true);
    let failure = verify_load(&p, &NoExternalContracts).unwrap_err();
    assert_eq!(failure.rule, "ARRAY_PLAIN", "{failure}");
    assert!(failure.message.contains("reaches resources"), "{failure}");
}
