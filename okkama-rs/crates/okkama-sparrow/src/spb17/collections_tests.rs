//! Maps, collection operations, composites and lookup in the load gate (#622).
use super::*;

const MAP: Type = Type::Map(0);
const INTS: Type = Type::Array(ArrayType { leaf: ArrayLeaf::Int, depth: 1 });
const STRS: Type = Type::Array(ArrayType { leaf: ArrayLeaf::Str, depth: 1 });

fn data(op: DataOp, inputs: Vec<u32>, output: u32) -> Instruction {
    Instruction::Data { op, inputs, output: Some(output), immediate: Immediate::None }
}

fn constant(value: Constant, output: u32) -> Instruction {
    Instruction::Data { op: DataOp::Const, inputs: vec![], output: Some(output), immediate: Immediate::Constant(value) }
}

fn with_roots(mut f: Function) -> Function {
    let live = live_locations(&f).unwrap();
    f.proofs.roots = live.into_iter().enumerate().map(|(pc, locations)| RootRequirement {
        pc: pc as u32, locations: locations.into_iter().collect(),
    }).collect();
    f
}

/// `m = ["a": 1]; m.len() + m.keys().len()`, then a lookup of "a" whose result is returned or 0.
fn fixture() -> Program {
    let mut f = tests::empty_function();
    f.registers = vec![Type::Str, Type::Int, MAP, Type::Int, STRS, Type::Int, Type::Int, Type::Int, Type::Int];
    f.results = vec![Type::Int];
    f.code = vec![
        constant(Constant::Str("a".into()), 0),
        constant(Constant::Int("1".into()), 1),
        data(DataOp::MapNew, vec![0, 1], 2),
        data(DataOp::MapLen, vec![2], 3),
        data(DataOp::MapKeys, vec![2], 4),
        data(DataOp::ArrayLen, vec![4], 5),
        data(DataOp::IntAdd, vec![3, 5], 6),
        Instruction::Champ { op: Opcode::Lookup,
            arguments: vec![Argument::Location(Location::Register(2)), Argument::Location(Location::Register(0))],
            flow: Flow::Outcomes(vec![
                Outcome { tag: Tag::Some, edge: Edge { target_pc: 8, assignments: vec![Assignment {
                    destination: Location::Register(7), source: Source::ProducedSlot(0),
                }] } },
                Outcome { tag: Tag::None, edge: Edge { target_pc: 9, assignments: vec![] } },
            ]) },
        Instruction::Ret(vec![7]),
        constant(Constant::Int("0".into()), 8),
        Instruction::Ret(vec![8]),
    ];
    f.blocks = vec![
        Block { pc: 0, params: vec![] },
        Block { pc: 8, params: vec![Location::Register(7)] },
        Block { pc: 9, params: vec![] },
    ];
    Program {
        functions: vec![with_roots(f)],
        entry: Some(0),
        composites: vec![Composite::Map { key: Type::Str, value: Type::Int }],
        ..Program::default()
    }
}

fn reject(p: &Program, rule: &str) {
    let failure = verify_load(p, &NoExternalContracts).unwrap_err();
    assert_eq!(failure.rule, rule, "{failure}");
}

fn plain_object(name: &str, fields: Vec<Type>, resource: bool) -> Object {
    let trace_layout = fields.iter().enumerate().filter_map(|(index, ty)| ty.traced().then_some(index as u32))
        .collect();
    Object {
        name: name.into(), error_cases: vec![], enum_cases: 0, variant_cases: vec![], handle: false,
        role: None, implements: vec![], any_box: false,
        fields: (0..fields.len() as u32).map(|storage| Field { name: format!("f{storage}"), storage, once: false })
            .collect(),
        layout: ObjectRuntimeLayout {
            allocation_words: (1 + fields.len() as u32).max(2), storage_fields: fields, identity_offset: None,
            has_destroy: false, may_reach_resource: resource, may_async_destroy: false, trace_layout,
            selection_layout: vec![],
        },
    }
}

#[test]
fn map_program_loads_and_round_trips() {
    let p = fixture();
    verify_load(&p, &NoExternalContracts).unwrap();
    assert_eq!(decode(&encode(&p).unwrap()).unwrap(), p);
    assert_eq!(assemble(&disassemble(&p).unwrap()).unwrap(), p);
}

#[test]
fn assembly_without_composites_keeps_its_text() {
    let p = Program { functions: vec![tests::empty_function()], ..Program::default() };
    let text = disassemble(&p).unwrap();
    assert!(!text.contains("composites"), "{text}");
}

#[test]
fn composite_references_must_point_backwards() {
    let mut p = fixture();
    p.composites = vec![
        Composite::Map { key: Type::Str, value: Type::Int },
        Composite::Map { key: Type::Str, value: Type::Map(1) },
    ];
    reject(&p, "COMPOSITE_ORDER");
    p.composites[1] = Composite::Map { key: Type::Str, value: Type::Map(0) };
    verify_load(&p, &NoExternalContracts).unwrap();
}

#[test]
fn composite_types_name_their_kind_of_entry() {
    let mut p = fixture();
    p.composites = vec![Composite::Builder { array: INTS }];
    reject(&p, "COMPOSITE_TYPE");
    let mut p = fixture();
    p.functions[0].registers.push(Type::Builder(0));
    reject(&p, "COMPOSITE_TYPE");
    let mut p = fixture();
    p.composites.push(Composite::Builder { array: INTS });
    p.functions[0].registers.push(Type::Builder(1));
    p.functions[0].registers.push(Type::MapBuilder(0));
    verify(&p).unwrap();
}

#[test]
fn map_keys_follow_the_key_object_rule() {
    let mut p = fixture();
    p.composites[0] = Composite::Map { key: INTS, value: Type::Int };
    reject(&p, "MAP_KEY");
    // A key object of key fields, an enum and a never key (uninferred `[:]`) are keys.
    for key in [Type::Object(0), Type::Object(1), Type::Never, Type::Rat, Type::Float, Type::Char] {
        let mut p = fixture();
        p.objects = vec![plain_object("Point", vec![Type::Int, Type::Str], false),
            Object { enum_cases: 2, fields: vec![], ..plain_object("Color", vec![Type::Int], false) }];
        p.composites.push(Composite::Map { key, value: Type::Int });
        verify(&p).unwrap();
    }
    // A key object may not hold a non-key field, contain itself or reach a resource.
    for (fields, resource) in [(vec![INTS], false), (vec![Type::Object(0)], false), (vec![Type::Int], true)] {
        let mut p = fixture();
        p.objects = vec![plain_object("Bad", fields, resource)];
        p.composites.push(Composite::Map { key: Type::Object(0), value: Type::Int });
        reject(&p, "MAP_KEY");
    }
}

#[test]
fn map_values_and_builder_arrays_are_plain_or_resource_references() {
    // A map may refer to resource objects as values (a managed map, managed_tests); its keys stay plain.
    let mut p = fixture();
    p.objects = vec![plain_object("Res", vec![], true)];
    p.composites.push(Composite::Map { key: Type::Str, value: Type::Object(0) });
    verify(&p).unwrap();
    p.composites.push(Composite::Map { key: Type::Str, value: INTS });
    p.composites.push(Composite::Map { key: Type::Str, value: Type::Map(2) });
    verify(&p).unwrap();
    let mut p = fixture();
    p.composites.push(Composite::Builder { array: Type::Int });
    reject(&p, "BUILDER_ARRAY");
    let mut p = fixture();
    p.composites.push(Composite::Builder { array: Type::Bytes });
    verify(&p).unwrap();
}

#[test]
fn map_operations_check_operand_and_result_types() {
    let mut p = fixture();
    p.functions[0].code[2] = data(DataOp::MapNew, vec![0, 0], 2);
    reject(&p, "MAP_OP_TYPE");
    let mut p = fixture();
    p.functions[0].registers[4] = INTS;
    reject(&p, "MAP_OP_TYPE");
    let mut p = fixture();
    p.functions[0].code[3] = data(DataOp::MapHas, vec![2, 1], 3);
    reject(&p, "MAP_OP_TYPE");
    let mut p = fixture();
    p.functions[0].code[3] = data(DataOp::MapGet, vec![2, 1], 3);
    reject(&p, "MAP_OP_TYPE");
    // A failing map.get in a function without services is plain data with a program failure.
    let mut p = fixture();
    p.functions[0].code[3] = data(DataOp::MapGet, vec![2, 0], 3);
    p.functions[0].proofs.entry = Some(EntryContract { failures: 2, ..EntryContract::default() });
    verify_load(&p, &NoExternalContracts).unwrap();
}

#[test]
fn array_operations_check_operand_and_result_types() {
    let mut p = fixture();
    p.functions[0].code[5] = data(DataOp::ArrayConcat, vec![4, 4], 5);
    reject(&p, "ARRAY_OP_TYPE");
    let mut p = fixture();
    p.functions[0].registers[5] = Type::Str;
    p.functions[0].code[5] = data(DataOp::ArrayJoin, vec![4, 0], 5);
    p.functions[0].code[6] = data(DataOp::StrLen, vec![5], 6);
    verify(&p).unwrap();
    p.functions[0].code[5] = data(DataOp::ArrayJoin, vec![4, 1], 5);
    reject(&p, "ARRAY_OP_TYPE");
}

#[test]
fn builder_new_must_produce_a_builder() {
    let mut p = fixture();
    p.functions[0].code[3] = data(DataOp::BuilderNew, vec![], 3);
    reject(&p, "BUILDER_NEW_TYPE");
}

/// A function of `code` over `registers` that returns its last register, an int.
fn straight(registers: Vec<Type>, code: Vec<Instruction>) -> Program {
    let mut f = tests::empty_function();
    let result = registers.len() as u32 - 1;
    f.registers = registers;
    f.results = vec![Type::Int];
    f.code = code;
    f.code.push(Instruction::Ret(vec![result]));
    Program { functions: vec![with_roots(f)], entry: Some(0), ..Program::default() }
}

#[test]
fn byte_arrays_are_built_from_bytes() {
    let code = |element| vec![
        constant(element, 0),
        data(DataOp::ArrayNew, vec![0], 1),
        data(DataOp::ArrayLen, vec![1], 2),
    ];
    let p = straight(vec![Type::Byte, Type::Bytes, Type::Int], code(Constant::Byte(7)));
    verify_load(&p, &NoExternalContracts).unwrap();
    let p = straight(vec![Type::Int, Type::Bytes, Type::Int], code(Constant::Int("7".into())));
    reject(&p, "ARRAY_NEW_TYPE");
}

#[test]
fn value_equality_compares_plain_collections_of_one_type() {
    let mut p = fixture();
    p.functions[0].registers[3] = Type::Bool;
    p.functions[0].code[3] = data(DataOp::ValEq, vec![2, 2], 3);
    p.functions[0].code[6] = data(DataOp::IntAdd, vec![5, 5], 6);
    p.functions[0] = with_roots(p.functions[0].clone());
    verify(&p).unwrap();
    p.functions[0].code[3] = data(DataOp::ValEq, vec![2, 4], 3);
    reject(&p, "VAL_EQ_TYPE");
}

#[test]
fn never_shapes_retag_only_to_their_own_shape() {
    let empty = Type::Array(ArrayType { leaf: ArrayLeaf::Never, depth: 1 });
    let composites = [Composite::Map { key: Type::Never, value: Type::Never },
        Composite::Map { key: Type::Str, value: Type::Int }];
    assert!(never_retag(&composites, empty, INTS));
    assert!(never_retag(&composites, Type::Map(0), Type::Map(1)));
    assert!(!never_retag(&composites, empty, Type::Bytes));
    assert!(!never_retag(&composites, INTS, STRS));
    assert!(!never_retag(&composites, Type::Map(1), Type::Map(0)));
    let code = vec![
        data(DataOp::ArrayNew, vec![], 0),
        data(DataOp::Move, vec![0], 1),
        data(DataOp::ArrayLen, vec![1], 2),
    ];
    let p = straight(vec![empty, INTS, Type::Int], code.clone());
    verify_load(&p, &NoExternalContracts).unwrap();
    // A non-empty shape cannot be reinterpreted.
    let p = straight(vec![INTS, STRS, Type::Int], code);
    assert!(verify(&p).unwrap_err().message.contains("invalid move"));
}

/// A key-less map keeps its value type unless that is never too: no value word is read as another type.
#[test]
fn never_keyed_maps_keep_their_value_type() {
    let composites = [
        Composite::Map { key: Type::Never, value: Type::Never },
        Composite::Map { key: Type::Never, value: Type::Float },
        Composite::Map { key: Type::Int, value: Type::Float },
        Composite::Map { key: Type::Int, value: Type::Object(0) },
    ];
    assert!(never_retag(&composites, Type::Map(0), Type::Map(3)));
    assert!(never_retag(&composites, Type::Map(1), Type::Map(2)));
    assert!(!never_retag(&composites, Type::Map(1), Type::Map(3)));
    // `map.new` with no entries, then a retag of the empty map.
    let retag = |from: u32, to: u32| {
        let mut p = straight(vec![Type::Map(from), Type::Map(to), Type::Int], vec![
            data(DataOp::MapNew, vec![], 0),
            data(DataOp::Move, vec![0], 1),
            data(DataOp::MapLen, vec![1], 2),
        ]);
        p.objects = vec![plain_object("P", vec![Type::Int], false)];
        p.composites = composites.to_vec();
        p
    };
    verify_load(&retag(0, 3), &NoExternalContracts).unwrap();
    verify_load(&retag(1, 2), &NoExternalContracts).unwrap();
    reject(&retag(1, 3), "NEVER_RETAG");
    // An array of never leaves keeps its depth.
    let empty = Type::Array(ArrayType { leaf: ArrayLeaf::Never, depth: 1 });
    let deep = Type::Array(ArrayType { leaf: ArrayLeaf::Int, depth: 2 });
    let p = straight(vec![empty, deep, Type::Int], vec![
        data(DataOp::ArrayNew, vec![], 0),
        data(DataOp::Move, vec![0], 1),
        data(DataOp::ArrayLen, vec![1], 2),
    ]);
    reject(&p, "NEVER_RETAG");
}

/// The audit's type confusion: an `undef` stored in a never field and read back is a never value; a map of it
/// with a float value, retagged to a map of objects, would read the float's word as an object reference.
fn never_map_confusion() -> Program {
    let mut f = tests::empty_function();
    f.registers = vec![
        Type::Never,
        Type::Object(0),
        Type::Never,
        Type::Float,
        Type::Map(0),
        Type::Map(1),
        Type::Int,
        Type::Object(1),
        Type::Int,
    ];
    f.results = vec![Type::Int];
    f.proofs.entry = Some(EntryContract { failures: 2, ..EntryContract::default() });
    let forged = Constant::Float(4_702_111_234_474_983_745u64.to_string());
    f.code = vec![
        Instruction::Data { op: DataOp::Undef, inputs: vec![], output: Some(0), immediate: Immediate::None },
        Instruction::Data { op: DataOp::ObjNew, inputs: vec![0], output: Some(1), immediate: Immediate::Object(0) },
        Instruction::Data { op: DataOp::ObjGet, inputs: vec![1], output: Some(2), immediate: Immediate::Field(0) },
        constant(forged, 3),
        data(DataOp::MapNew, vec![2, 3], 4),
        data(DataOp::Move, vec![4], 5),
        constant(Constant::Int("0".into()), 6),
        data(DataOp::MapValueAt, vec![5, 6], 7),
        Instruction::Data { op: DataOp::ObjGet, inputs: vec![7], output: Some(8), immediate: Immediate::Field(0) },
        Instruction::Ret(vec![8]),
    ];
    Program {
        objects: vec![plain_object("Holder", vec![Type::Never], false), plain_object("P", vec![Type::Int], false)],
        functions: vec![with_roots(f)],
        entry: Some(0),
        composites: vec![
            Composite::Map { key: Type::Never, value: Type::Float },
            Composite::Map { key: Type::Int, value: Type::Object(1) },
        ],
        ..Program::default()
    }
}

#[test]
fn no_value_of_type_never_is_stored_or_read() {
    reject(&never_map_confusion(), "NEVER_FIELD");
    // The same map built from a never value that arrives as a parameter: map.new cannot read it.
    let mut p = straight(vec![Type::Never, Type::Float, Type::Map(0), Type::Int], vec![
        constant(Constant::Float(0u64.to_string()), 1),
        data(DataOp::MapNew, vec![0, 1], 2),
        data(DataOp::MapLen, vec![2], 3),
    ]);
    p.composites = vec![Composite::Map { key: Type::Never, value: Type::Float }];
    let f = &mut p.functions[0];
    f.params = vec![Location::Register(0)];
    f.blocks[0].params = f.params.clone();
    p.functions[0] = with_roots(p.functions[0].clone());
    reject(&p, "NEVER_OPERAND");
    // The empty map of the same type loads.
    let Instruction::Data { inputs, .. } = &mut p.functions[0].code[1] else { unreachable!() };
    inputs.clear();
    verify_load(&p, &NoExternalContracts).unwrap();
}

/// An array of never leaves exists only empty: `array.new` cannot read a never element into it.
#[test]
fn a_never_array_cannot_be_built_from_a_never_value() {
    let empty = Type::Array(ArrayType { leaf: ArrayLeaf::Never, depth: 1 });
    let objects = Type::Array(ArrayType { leaf: ArrayLeaf::Object(0), depth: 1 });
    let mut p = straight(vec![Type::Never, empty, objects, Type::Int], vec![
        data(DataOp::ArrayNew, vec![0], 1),
        data(DataOp::Move, vec![1], 2),
        data(DataOp::ArrayLen, vec![2], 3),
    ]);
    p.objects = vec![plain_object("P", vec![Type::Int], false)];
    let f = &mut p.functions[0];
    f.params = vec![Location::Register(0)];
    f.blocks[0].params = f.params.clone();
    p.functions[0] = with_roots(p.functions[0].clone());
    reject(&p, "NEVER_OPERAND");
    // Without its element the same program is the empty literal and loads.
    let Instruction::Data { inputs, .. } = &mut p.functions[0].code[0] else { unreachable!() };
    inputs.clear();
    verify_load(&p, &NoExternalContracts).unwrap();
}

fn lookup_arguments(p: &mut Program, receiver: u32, key: u32) {
    let Instruction::Champ { arguments, .. } = &mut p.functions[0].code[7] else { unreachable!() };
    *arguments = vec![Argument::Location(Location::Register(receiver)), Argument::Location(Location::Register(key))];
}

#[test]
fn lookup_checks_its_receiver_and_key() {
    let mut p = fixture();
    lookup_arguments(&mut p, 2, 1);
    reject(&p, "LOOKUP_OPERANDS");
    // An array lookup takes an int index and yields the element (here the str keys of the map).
    let mut p = fixture();
    lookup_arguments(&mut p, 4, 3);
    p.functions[0].registers[7] = Type::Str;
    p.functions[0].code[8] = Instruction::Ret(vec![6]);
    p.functions[0] = with_roots(p.functions[0].clone());
    verify_load(&p, &NoExternalContracts).unwrap();
    lookup_arguments(&mut p, 4, 0);
    reject(&p, "LOOKUP_OPERANDS");
    // The found edge receives the element type, not the receiver's.
    let mut p = fixture();
    lookup_arguments(&mut p, 4, 3);
    p.functions[0] = with_roots(p.functions[0].clone());
    assert!(verify(&p).unwrap_err().message.contains("edge assignment type mismatch"));
}

#[test]
fn array_shapes_compute_their_elements() {
    assert_eq!(ArrayType::of(Type::Byte), Some(Type::Bytes));
    assert_eq!(ArrayType::of(Type::Int), Some(INTS));
    assert_eq!(ArrayType::of(INTS), Some(Type::Array(ArrayType { leaf: ArrayLeaf::Int, depth: 2 })));
    assert_eq!(ArrayType::of(Type::Failure), None);
    assert_eq!(element_type(Type::Bytes), Some(Type::Byte));
    assert_eq!(element_type(Type::Array(ArrayType { leaf: ArrayLeaf::Map(3), depth: 1 })), Some(Type::Map(3)));
    assert_eq!(element_type(Type::Str), None);
    assert!(Type::Map(0).traced() && Type::Builder(0).traced() && !Type::Float.traced());
}
