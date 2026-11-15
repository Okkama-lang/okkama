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
        variant_cases: vec![], handle: false, role: None, implements: vec![],
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
    for bits in [f64::INFINITY.to_bits(), f64::NEG_INFINITY.to_bits(), f64::NAN.to_bits()] {
        let p = program(vec![Type::Float], vec![constant(0, Constant::Float(bits.to_string()))], false, 0);
        reject(&p, "CONST_FLOAT");
    }
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

#[test]
fn value_equality_needs_a_closed_plain_graph() {
    // 4 `Box`, outside the variant declarations of `declarations()`.
    let code = vec![
        data(DataOp::ObjNew, vec![], 0, Immediate::Object(4)),
        data(DataOp::ObjNew, vec![], 1, Immediate::Object(4)),
        data(DataOp::ValEq, vec![0, 1], 2, Immediate::None),
    ];
    let registers = vec![Type::Object(4), Type::Object(4), Type::Bool];
    let boxed = || {
        let mut p = with_declarations(program(registers.clone(), code.clone(), false, 2));
        p.objects.push(plain_object("Box", vec![]));
        p
    };
    verify_load(&boxed(), &NoExternalContracts).unwrap();
    let mut reaching = boxed();
    reaching.objects[4].layout.may_reach_resource = true;
    reject(&reaching, "VAL_EQ_TYPE");
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

/// A constant of each scalar family operand type.
fn operand_constant(ty: Type) -> Constant {
    match ty {
        Type::Int => Constant::Int("3".into()),
        Type::Byte => Constant::Byte(3),
        Type::Float => Constant::Float(1.5f64.to_bits().to_string()),
        Type::Rat => Constant::Rat(RatValue { numerator: "1".into(), denominator: "2".into() }),
        Type::Str => Constant::Str("a b".into()),
        other => panic!("no operand constant for {other:?}"),
    }
}

/// `op` on constants of its signature: registers are the operands, then the result.
fn family_program(op: DataOp, service: bool, failures: u32) -> Program {
    let (operands, result) = op.scalar_signature().unwrap();
    let mut registers = operands.to_vec();
    registers.push(result);
    let mut code = operands.iter().enumerate()
        .map(|(index, ty)| constant(index as u32, operand_constant(*ty)))
        .collect::<Vec<_>>();
    let inputs = (0..operands.len() as u32).collect();
    code.push(data(op, inputs, operands.len() as u32, Immediate::None));
    program(registers, code, service, failures)
}

fn family_operations() -> Vec<DataOp> {
    SCALAR_FAMILIES.iter().flat_map(|(_, _, ops)| ops.iter().copied()).collect()
}

#[test]
fn scalar_families_load_and_round_trip_every_operation() {
    for op in family_operations() {
        let p = family_program(op, false, 2);
        verify_load(&p, &NoExternalContracts).unwrap_or_else(|error| panic!("{op:?}: {error}"));
        assert_eq!(decode(&encode(&p).unwrap()).unwrap(), p, "{op:?}");
        assert_eq!(assemble(&disassemble(&p).unwrap()).unwrap(), p, "{op:?}");
        let code = DataOp::from_scalar_code(op.scalar_code().unwrap());
        assert_eq!(code, Some(op));
    }
}

#[test]
fn scalar_family_wire_codes_are_fixed() {
    assert_eq!(DataOp::IntAnd.family(), Some((14, 0)));
    assert_eq!(DataOp::IntRotr.family(), Some((14, 15)));
    assert_eq!(DataOp::ByteUshr.family(), Some((15, 5)));
    assert_eq!(DataOp::FloatAdd.family(), Some((16, 0)));
    assert_eq!(DataOp::FloatSqrt.family(), Some((16, 9)));
    assert_eq!(DataOp::RatDiv.family(), Some((17, 3)));
    assert_eq!(DataOp::StrSlice.family(), Some((18, 0)));
    assert_eq!(DataOp::StrUtf8.family(), Some((18, 15)));
    assert_eq!(DataOp::IntAdd.family(), None);
    // Wire codes reserved for other lanes stay free.
    for code in (56..=63).chain(19..=19).chain(21..=23).chain(27..=39).chain(73..=79) {
        assert!(SCALAR_FAMILIES.iter().all(|(family, _, _)| *family != code), "{code}");
    }
}

#[test]
fn scalar_family_decoder_refuses_unknown_sub_codes() {
    let p = family_program(DataOp::FloatAdd, false, 2);
    let text = disassemble(&p).unwrap();
    assert!(text.contains("float.ext 0"), "{text}");
    let unknown = text.replace("float.ext 0", "float.ext 10");
    assert_eq!(assemble(&unknown).unwrap_err().rule, "SPB_DECODE");
    let unknown = text.replace("float.ext 0", "str.ext 16");
    assert_eq!(assemble(&unknown).unwrap_err().rule, "SPB_DECODE");
}

#[test]
fn scalar_family_operands_and_results_are_typed() {
    let mut p = family_program(DataOp::IntShl, false, 2);
    p.functions[0].registers[2] = Type::Byte;
    reject(&p, "SCALAR_OP_TYPE");
    // byte.shl counts in int, byte.ushr in byte.
    let mut p = family_program(DataOp::ByteShl, false, 2);
    p.functions[0].registers[1] = Type::Byte;
    p.functions[0].code[1] = constant(1, Constant::Byte(1));
    reject(&p, "SCALAR_OP_TYPE");
    let mut p = family_program(DataOp::ByteUshr, false, 2);
    p.functions[0].registers[1] = Type::Int;
    p.functions[0].code[1] = constant(1, Constant::Int("1".into()));
    reject(&p, "SCALAR_OP_TYPE");
    let mut p = family_program(DataOp::StrSplit, false, 0);
    p.functions[0].registers[2] = Type::Str;
    reject(&p, "SCALAR_OP_TYPE");
    let mut p = family_program(DataOp::RatNumerator, false, 0);
    p.functions[0].registers[1] = Type::Int;
    reject(&p, "SCALAR_OP_TYPE");
    let mut p = family_program(DataOp::FloatAdd, false, 2);
    let Instruction::Data { inputs, .. } = &mut p.functions[0].code[2] else { unreachable!() };
    inputs.pop();
    reject(&p, "SCALAR_OP_TYPE");
}

#[test]
fn scalar_family_failures_follow_tomato() {
    let fallible = [
        DataOp::IntAnd, DataOp::IntOr, DataOp::IntXor, DataOp::IntShl, DataOp::IntShr, DataOp::IntUshr,
        DataOp::IntNot, DataOp::IntBitCount, DataOp::IntLeadingZeros, DataOp::IntTrailingZeros,
        DataOp::IntReverseBits, DataOp::IntSwapBytes, DataOp::IntRotl, DataOp::IntRotr,
        DataOp::ByteShl, DataOp::ByteShr, DataOp::ByteUshr, DataOp::ByteRotl, DataOp::ByteRotr,
        DataOp::FloatAdd, DataOp::FloatSub, DataOp::FloatMul, DataOp::FloatDiv, DataOp::FloatMod,
        DataOp::FloatPow, DataOp::FloatSqrt, DataOp::RatDiv, DataOp::RatMod, DataOp::StrSlice,
        DataOp::StrRepeat,
    ];
    for op in family_operations() {
        if fallible.contains(&op) {
            reject(&family_program(op, false, 0), "ENTRY_FAILURE_MASK");
            reject(&family_program(op, true, 2), "FALLIBLE_DATA_OUTCOMES");
        } else {
            let p = family_program(op, true, 0);
            verify_load(&p, &NoExternalContracts).unwrap_or_else(|error| panic!("{op:?}: {error}"));
        }
    }
}

/// `op` as an `evaluate` with normal and unwind outcomes, on constants of its signature.
fn evaluated(op: DataOp) -> Program {
    let (operands, result) = op.scalar_signature().unwrap();
    let mut registers = operands.to_vec();
    registers.extend([result, Type::Failure]);
    let count = operands.len() as u32;
    let mut p = program(registers, vec![], false, 2);
    let f = &mut p.functions[0];
    let produced = |destination| Assignment { destination, source: Source::ProducedSlot(0) };
    f.code = operands.iter().enumerate().map(|(index, ty)| constant(index as u32, operand_constant(*ty))).collect();
    let evaluation = Evaluation { op, result, immediate: Immediate::None };
    let mut arguments = vec![Argument::Evaluation(evaluation)];
    arguments.extend((0..count).map(|id| Argument::Location(Location::Register(id))));
    f.code.extend([
        Instruction::Champ { op: Opcode::Evaluate, arguments, flow: Flow::Outcomes(vec![
            Outcome { tag: Tag::Normal, edge: Edge { target_pc: count + 1,
                assignments: vec![produced(Location::Register(count))] } },
            Outcome { tag: Tag::Unwind, edge: Edge { target_pc: count + 2,
                assignments: vec![produced(Location::Register(count + 1))] } },
        ]) },
        Instruction::Ret(vec![]),
        Instruction::Raise(count + 1),
    ]);
    f.blocks = vec![
        Block { pc: 0, params: vec![] },
        Block { pc: count + 1, params: vec![Location::Register(count)] },
        Block { pc: count + 2, params: vec![Location::Register(count + 1)] },
    ];
    let live = live_locations(f).unwrap();
    f.proofs.roots = live.into_iter().enumerate().map(|(pc, locations)| RootRequirement {
        pc: pc as u32, locations: locations.into_iter().collect(),
    }).collect();
    p
}

#[test]
fn scalar_family_evaluate_takes_only_fallible_operations() {
    for op in [DataOp::FloatDiv, DataOp::IntShl, DataOp::ByteShl, DataOp::RatDiv, DataOp::StrSlice] {
        verify_load(&evaluated(op), &NoExternalContracts).unwrap_or_else(|error| panic!("{op:?}: {error}"));
    }
    for op in [DataOp::RatAdd, DataOp::ByteAnd, DataOp::FloatNeg, DataOp::StrTrim, DataOp::IntAbs] {
        reject(&evaluated(op), "EVALUATE_OPERATION");
    }
}

fn rat(numerator: &str, denominator: &str) -> Program {
    let value = Constant::Rat(RatValue { numerator: numerator.into(), denominator: denominator.into() });
    program(vec![Type::Rat], vec![constant(0, value)], false, 0)
}

#[test]
fn rat_constants_are_in_lowest_terms() {
    for (numerator, denominator) in [("1", "2"), ("-3", "4"), ("0", "1"), ("1", "1"), ("-7", "1"),
        ("12345678901234567890121", "98765432109876543210"), ("3", "10")]
    {
        let p = rat(numerator, denominator);
        verify_load(&p, &NoExternalContracts).unwrap_or_else(|error| panic!("{numerator}/{denominator}: {error}"));
        assert_eq!(decode(&encode(&p).unwrap()).unwrap(), p);
        assert_eq!(assemble(&disassemble(&p).unwrap()).unwrap(), p);
    }
    for (numerator, denominator) in [("2", "4"), ("0", "2"), ("1", "0"), ("1", "-2"), ("01", "2"), ("-0", "1"),
        ("1", "02"), ("x", "1"), ("6", "9"), ("10", "15"), ("246913578024691357802", "98765432109876543210")]
    {
        reject(&rat(numerator, denominator), "CONST_RAT");
    }
}

#[test]
fn int_constants_are_exact_beyond_64_bits() {
    let code = vec![
        constant(0, Constant::Int("1267650600228229401496703205376".into())),
        constant(1, Constant::Int("-9223372036854775809".into())),
        data(DataOp::IntAdd, vec![0, 1], 2, Immediate::None),
    ];
    let p = program(vec![Type::Int, Type::Int, Type::Int], code, false, 0);
    verify_load(&p, &NoExternalContracts).unwrap();
    assert_eq!(decode(&encode(&p).unwrap()).unwrap(), p);
}
