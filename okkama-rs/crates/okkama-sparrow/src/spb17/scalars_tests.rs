//! Byte/Float/Rat/Char, plain enums and variants, value equality and typed conversion fallibility (#622).
use super::*;

fn reject(p: &Program, rule: &str) {
    let failure = verify_load(p, &NoExternalContracts).unwrap_err();
    assert_eq!(failure.rule, rule, "{failure}");
}

fn data(op: DataOp, inputs: Vec<u32>, output: u32, immediate: Immediate) -> Instruction {
    Instruction::Data { op, inputs, output: Some(output), immediate }
}

fn constant(output: u32, value: Constant) -> Instruction {
    data(DataOp::Const, vec![], output, Immediate::Constant(value))
}

/// `main` with `registers`, `code` before its `ret`, an optional proof-only service and the declared mask.
fn program(registers: Vec<Type>, code: Vec<Instruction>, service: bool, failures: u32) -> Program {
    let mut f = tests::empty_function();
    f.registers = registers;
    if service {
        f.services = vec![ServiceKind::Deps];
        f.params = vec![Location::Service(0)];
        f.blocks[0].params = f.params.clone();
    }
    f.proofs.entry = Some(EntryContract { failures, ..EntryContract::default() });
    f.code = code;
    f.code.push(Instruction::Ret(vec![]));
    let live = live_locations(&f).unwrap();
    f.proofs.roots = live.into_iter().enumerate().map(|(pc, locations)| RootRequirement {
        pc: pc as u32, locations: locations.into_iter().collect(),
    }).collect();
    Program { functions: vec![f], ..Program::default() }
}

fn plain_object(name: &str, storage_fields: Vec<Type>) -> Object {
    let count = storage_fields.len() as u32;
    let trace_layout = storage_fields.iter().enumerate()
        .filter_map(|(index, ty)| matches!(ty, Type::Str | Type::Rat | Type::Object(_)).then_some(index as u32))
        .collect();
    Object {
        name: name.into(),
        error_cases: vec![],
        enum_cases: 0,
        variant_cases: vec![], handle: false,
        fields: (0..count).map(|storage| Field { name: format!("f{storage}"), storage, once: false }).collect(),
        layout: ObjectRuntimeLayout {
            storage_fields, allocation_words: (1 + count).max(2), identity_offset: None, has_destroy: false,
            may_reach_resource: false, may_async_destroy: false, trace_layout, selection_layout: vec![],
        },
    }
}

/// Objects: 0 `Color` (a plain enum of two cases), 1 `Pair.empty`, 2 `Pair.two(int, str)`, 3 `Pair`.
fn declarations() -> Vec<Object> {
    let mut color = plain_object("Color", vec![Type::Int]);
    color.fields.clear();
    color.enum_cases = 2;
    let mut pair = plain_object("Pair", vec![]);
    pair.variant_cases = vec![1, 2];
    vec![color, plain_object("Pair.empty", vec![]), plain_object("Pair.two", vec![Type::Int, Type::Str]), pair]
}

fn with_declarations(mut p: Program) -> Program {
    p.objects = declarations();
    p
}

#[test]
fn total_conversion_is_plain_data_beside_services() {
    let code = vec![constant(0, Constant::Int("7".into())), data(DataOp::Convert, vec![0], 1, Immediate::None)];
    let p = program(vec![Type::Int, Type::Str], code, true, 0);
    verify_load(&p, &NoExternalContracts).unwrap();
}

#[test]
fn trapping_conversion_keeps_its_failure() {
    let code = || vec![constant(0, Constant::Int("300".into())), data(DataOp::Convert, vec![0], 1, Immediate::None)];
    reject(&program(vec![Type::Int, Type::Byte], code(), false, 0), "ENTRY_FAILURE_MASK");
    reject(&program(vec![Type::Int, Type::Byte], code(), true, 2), "FALLIBLE_DATA_OUTCOMES");
    verify_load(&program(vec![Type::Int, Type::Byte], code(), false, 2), &NoExternalContracts).unwrap();
}

#[test]
fn evaluate_refuses_a_total_conversion() {
    let mut p = program(vec![Type::Int, Type::Str, Type::Failure], vec![], false, 2);
    let f = &mut p.functions[0];
    let produced = |destination| Assignment { destination, source: Source::ProducedSlot(0) };
    let evaluation = Evaluation { op: DataOp::Convert, result: Type::Str, immediate: Immediate::None };
    f.code = vec![
        constant(0, Constant::Int("7".into())),
        Instruction::Champ { op: Opcode::Evaluate,
            arguments: vec![Argument::Evaluation(evaluation), Argument::Location(Location::Register(0))],
            flow: Flow::Outcomes(vec![
                Outcome { tag: Tag::Normal, edge: Edge { target_pc: 2,
                    assignments: vec![produced(Location::Register(1))] } },
                Outcome { tag: Tag::Unwind, edge: Edge { target_pc: 3,
                    assignments: vec![produced(Location::Register(2))] } },
            ]) },
        Instruction::Ret(vec![]),
        Instruction::Raise(2),
    ];
    f.blocks = vec![
        Block { pc: 0, params: vec![] },
        Block { pc: 2, params: vec![Location::Register(1)] },
        Block { pc: 3, params: vec![Location::Register(2)] },
    ];
    reject(&p, "EVALUATE_OPERATION");
}

#[test]
fn conversion_pairs_are_closed() {
    let code = vec![constant(0, Constant::Str("7".into())), data(DataOp::Convert, vec![0], 1, Immediate::None)];
    reject(&program(vec![Type::Str, Type::Int], code, false, 2), "CONVERT_TYPE");
    let code = vec![constant(0, Constant::Bool(true)), data(DataOp::Convert, vec![0], 1, Immediate::None)];
    reject(&program(vec![Type::Bool, Type::Float], code, false, 2), "CONVERT_TYPE");
}

#[test]
fn scalar_constants_are_canonical() {
    reject(&program(vec![Type::Byte], vec![constant(0, Constant::Byte(256))], false, 0), "CONST_BYTE");
    reject(&program(vec![Type::Char], vec![constant(0, Constant::Char(0xd800))], false, 0), "CONST_CHAR");
    reject(&program(vec![Type::Float], vec![constant(0, Constant::Float("01".into()))], false, 0), "CONST_FLOAT");
    reject(&program(vec![Type::Float], vec![constant(0, Constant::Float("x".into()))], false, 0), "CONST_FLOAT");
    let code = vec![
        constant(0, Constant::Byte(255)),
        constant(1, Constant::Char(0xe9)),
        constant(2, Constant::Float(1.5f64.to_bits().to_string())),
        constant(3, Constant::Float(2.5f64.to_bits().to_string())),
        data(DataOp::FloatLt, vec![2, 3], 4, Immediate::None),
    ];
    let p = program(vec![Type::Byte, Type::Char, Type::Float, Type::Float, Type::Bool], code, false, 0);
    verify_load(&p, &NoExternalContracts).unwrap();
}

#[test]
fn scalar_comparisons_keep_their_family() {
    let code = vec![
        constant(0, Constant::Byte(1)),
        constant(1, Constant::Char(1)),
        data(DataOp::ByteEq, vec![0, 1], 2, Immediate::None),
    ];
    reject(&program(vec![Type::Byte, Type::Char, Type::Bool], code, false, 0), "SPB_STRUCTURE");
}

#[test]
fn enum_comparison_needs_one_plain_enum() {
    let code = |a, b| vec![
        data(DataOp::EnumCase, vec![], 0, Immediate::Updates(vec![0, a])),
        data(DataOp::EnumCase, vec![], 1, Immediate::Updates(vec![0, b])),
        data(DataOp::EnumEq, vec![0, 1], 2, Immediate::None),
    ];
    let enums = vec![Type::Object(0), Type::Object(0), Type::Bool];
    verify_load(&with_declarations(program(enums.clone(), code(0, 1), false, 0)), &NoExternalContracts).unwrap();
    reject(&with_declarations(program(enums, code(0, 2), false, 0)), "ENUM_CASE");
    let code = vec![
        data(DataOp::ObjNew, vec![], 0, Immediate::Object(1)),
        data(DataOp::ObjNew, vec![], 1, Immediate::Object(1)),
        data(DataOp::EnumEq, vec![0, 1], 2, Immediate::None),
    ];
    reject(&with_declarations(program(vec![Type::Object(1), Type::Object(1), Type::Bool], code, false, 0)),
        "ENUM_EQ_TYPE");
}

#[test]
fn value_equality_compares_objects_of_one_declaration() {
    let code = |left, right| vec![
        data(DataOp::ObjNew, vec![], 0, Immediate::Object(1)),
        data(DataOp::ObjNew, vec![], 1, Immediate::Object(1)),
        data(DataOp::ValEq, vec![left, right], 2, Immediate::None),
    ];
    let registers = vec![Type::Object(1), Type::Object(1), Type::Bool];
    verify_load(&with_declarations(program(registers.clone(), code(0, 1), false, 2)), &NoExternalContracts)
        .unwrap();
    reject(&with_declarations(program(registers.clone(), code(0, 1), false, 0)), "ENTRY_FAILURE_MASK");
    let code = vec![
        constant(0, Constant::Int("1".into())),
        constant(1, Constant::Int("1".into())),
        data(DataOp::ValEq, vec![0, 1], 2, Immediate::None),
    ];
    reject(&with_declarations(program(vec![Type::Int, Type::Int, Type::Bool], code, false, 2)), "VAL_EQ_TYPE");
}

/// A `Pair.two(7, "x")` value, its tag and its payload field `storage` of case `case`.
fn variant_code(case: u32, storage: u32) -> Vec<Instruction> {
    vec![
        constant(0, Constant::Int("7".into())),
        constant(1, Constant::Str("x".into())),
        data(DataOp::ObjNew, vec![0, 1], 2, Immediate::Object(2)),
        data(DataOp::Move, vec![2], 3, Immediate::None),
        data(DataOp::VariantTag, vec![3], 4, Immediate::None),
        data(DataOp::VariantGet, vec![3], 5, Immediate::Updates(vec![case, storage])),
    ]
}

fn variant_registers(field: Type) -> Vec<Type> {
    vec![Type::Int, Type::Str, Type::Object(2), Type::Variant(3), Type::Int, field]
}

#[test]
fn variants_load_with_closed_cases_tags_and_payloads() {
    let p = with_declarations(program(variant_registers(Type::Str), variant_code(1, 1), false, 2));
    verify_load(&p, &NoExternalContracts).unwrap();
    assert_eq!(decode(&encode(&p).unwrap()).unwrap(), p);
    assert_eq!(assemble(&disassemble(&p).unwrap()).unwrap(), p);
    reject(&with_declarations(program(variant_registers(Type::Str), variant_code(1, 1), true, 2)),
        "FALLIBLE_DATA_OUTCOMES");
}

#[test]
fn variant_rules_reject_wrong_cases_fields_and_types() {
    reject(&with_declarations(program(variant_registers(Type::Str), variant_code(2, 1), false, 2)), "VARIANT_GET");
    reject(&with_declarations(program(variant_registers(Type::Str), variant_code(0, 0), false, 2)), "VARIANT_GET");
    reject(&with_declarations(program(variant_registers(Type::Int), variant_code(1, 1), false, 2)), "VARIANT_GET");
    let mut p = with_declarations(program(variant_registers(Type::Str), variant_code(1, 1), false, 2));
    p.functions[0].registers[3] = Type::Variant(2);
    reject(&p, "VARIANT_TYPE");
    let mut p = with_declarations(program(variant_registers(Type::Str), variant_code(1, 1), false, 2));
    p.objects[3].variant_cases = vec![1, 1];
    reject(&p, "VARIANT_CASES");
    let mut p = with_declarations(program(variant_registers(Type::Str), variant_code(1, 1), false, 2));
    p.objects[3].variant_cases = vec![1, 3];
    reject(&p, "VARIANT_CASES");
    let mut code = variant_code(1, 1);
    code[4] = data(DataOp::VariantTag, vec![2], 4, Immediate::None);
    reject(&with_declarations(program(variant_registers(Type::Str), code, false, 2)), "VARIANT_TAG");
    let mut code = variant_code(1, 1);
    code[3] = data(DataOp::Move, vec![0], 3, Immediate::None);
    reject(&with_declarations(program(variant_registers(Type::Str), code, false, 2)), "SPB_STRUCTURE");
}

#[test]
fn optional_object_fields_stay_out_of_assembly_until_used() {
    let p = with_declarations(program(vec![], vec![], false, 0));
    let text = disassemble(&p).unwrap();
    assert!(text.contains("enum_cases 2") && text.contains("variant_cases 2 1 2"), "{text}");
    assert_eq!(text.matches("enum_cases").count(), 1, "{text}");
    assert_eq!(assemble(&text).unwrap(), p);
    let explicit = text.replacen("error_cases 0 }", "error_cases 0 enum_cases 0 }", 1);
    assert_ne!(explicit, text);
    assert_eq!(assemble(&explicit).unwrap_err().rule, "SPB_DECODE");
}
