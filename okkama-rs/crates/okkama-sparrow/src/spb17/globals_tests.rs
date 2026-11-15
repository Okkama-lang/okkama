//! Module constants (#709): `Program::globals` laid out as data and read by `global.get`.
use super::*;

fn int(value: &str) -> Datum {
    Datum { ty: Type::Int, value: DatumValue::Constant(Constant::Int(value.into())) }
}

fn ints() -> Type {
    Type::Array(ArrayType { leaf: ArrayLeaf::Int, depth: 1 })
}

fn nested() -> Type {
    Type::Array(ArrayType { leaf: ArrayLeaf::Int, depth: 2 })
}

/// Constants of every shape the producer writes: a scalar beyond 64 bits, an array, a nested array with an
/// empty typed element, a map and an enum case; `main` reads the global named by `read`.
fn program(read: u32, result: Type) -> Program {
    let mut f = tests::empty_function();
    f.registers = vec![result];
    f.results = vec![result];
    f.code = vec![
        Instruction::Data { op: DataOp::GlobalGet, inputs: vec![], output: Some(0),
            immediate: Immediate::Global(read) },
        Instruction::Ret(vec![0]),
    ];
    let live = live_locations(&f).unwrap();
    f.proofs.roots = live.into_iter().enumerate().map(|(pc, locations)| RootRequirement {
        pc: pc as u32, locations: locations.into_iter().collect(),
    }).collect();
    let color = Object {
        name: "Color".into(), error_cases: vec![], enum_cases: 2, variant_cases: vec![], handle: false,
        role: None, implements: vec![], any_box: false, fields: vec![],
        layout: ObjectRuntimeLayout {
            storage_fields: vec![Type::Int], allocation_words: 2, identity_offset: None, has_destroy: false,
            may_reach_resource: false, may_async_destroy: false, trace_layout: vec![], selection_layout: vec![],
        },
    };
    Program {
        objects: vec![color],
        composites: vec![Composite::Map { key: Type::Int, value: Type::Str }],
        globals: vec![
            Global { name: "app.const.BIG".into(), ty: Type::Int, data: vec![int("18446744073709551614")] },
            Global { name: "app.const.PRIMES".into(), ty: ints(), data: vec![
                int("2"), int("3"), Datum { ty: ints(), value: DatumValue::Array(vec![0, 1]) },
            ] },
            Global { name: "app.const.NESTED".into(), ty: nested(), data: vec![
                Datum { ty: ints(), value: DatumValue::Array(vec![]) },
                int("11"),
                Datum { ty: ints(), value: DatumValue::Array(vec![1]) },
                Datum { ty: nested(), value: DatumValue::Array(vec![0, 2]) },
            ] },
            Global { name: "app.const.NAMES".into(), ty: Type::Map(0), data: vec![
                int("1"), Datum { ty: Type::Str, value: DatumValue::Constant(Constant::Str("one".into())) },
                Datum { ty: Type::Map(0), value: DatumValue::Map(vec![0, 1]) },
            ] },
            Global { name: "app.const.DEFAULT".into(), ty: Type::Object(0), data: vec![
                Datum { ty: Type::Object(0), value: DatumValue::Case(1) },
            ] },
        ],
        functions: vec![f],
        entry: Some(0),
        ..Program::default()
    }
}

#[test]
fn globals_load_and_roundtrip() {
    for (read, ty) in [(0, Type::Int), (1, ints()), (2, nested()), (3, Type::Map(0)), (4, Type::Object(0))] {
        let p = program(read, ty);
        verify_load(&p, &NoExternalContracts).unwrap_or_else(|error| panic!("{read}: {error}"));
        assert_eq!(decode(&encode(&p).unwrap()).unwrap(), p);
        assert_eq!(assemble(&disassemble(&p).unwrap()).unwrap(), p);
    }
}

fn rule(p: &Program) -> &'static str {
    verify_load(p, &NoExternalContracts).unwrap_err().rule
}

/// GLOBAL_GET: the read names a constant and produces exactly its type.
#[test]
fn global_get_rejects_another_type_or_a_missing_constant() {
    assert_eq!(rule(&program(1, nested())), "GLOBAL_GET");
    assert_eq!(rule(&program(0, ints())), "GLOBAL_GET");
    assert_eq!(rule(&program(9, Type::Int)), "GLOBAL_GET");
    let mut p = program(1, ints());
    let Instruction::Data { immediate, .. } = &mut p.functions[0].code[0] else { unreachable!() };
    *immediate = Immediate::Field(1);
    assert_eq!(rule(&p), "GLOBAL_GET");
}

/// GLOBAL_DATA: every datum builds a value of its declared type from earlier data of its element, key and
/// value types.
#[test]
fn global_data_rejects_ill_typed_or_forward_data() {
    let reject = |edit: &dyn Fn(&mut Vec<Global>)| {
        let mut p = program(1, ints());
        edit(&mut p.globals);
        assert_eq!(rule(&p), "GLOBAL_DATA");
    };
    // An array element of another type: a str among ints.
    reject(&|g| g[1].data[1] = Datum { ty: Type::Str, value: DatumValue::Constant(Constant::Str("3".into())) });
    // A constant that is not of its datum's type.
    reject(&|g| g[1].data[1].value = DatumValue::Constant(Constant::Bool(true)));
    // A part named after its whole, and a well-typed part that does not precede its whole.
    reject(&|g| g[1].data[2].value = DatumValue::Array(vec![0, 2]));
    reject(&|g| g[2].data[0].value = DatumValue::Array(vec![1]));
    // A nested array whose element is not an int array.
    reject(&|g| g[2].data[3].value = DatumValue::Array(vec![1]));
    // A map with an unpaired key, and with a value of the key type.
    reject(&|g| g[3].data[2].value = DatumValue::Map(vec![0]));
    reject(&|g| g[3].data[2].value = DatumValue::Map(vec![0, 0]));
    // A descriptor is no constant data; an enum case outside the enum; an array datum of a scalar type.
    reject(&|g| g[0].data[0].value = DatumValue::Constant(Constant::Descriptor(0)));
    reject(&|g| g[4].data[0].value = DatumValue::Case(2));
    reject(&|g| g[0].data[0].value = DatumValue::Array(vec![]));
}

/// GLOBAL_TYPE: a constant is closed plain data and its last datum is its value.
#[test]
fn global_type_rejects_a_mismatched_value_or_non_plain_type() {
    let mut p = program(1, ints());
    p.globals[1].data.push(int("5"));
    assert_eq!(rule(&p), "GLOBAL_TYPE");
    let mut p = program(1, ints());
    p.globals[1].data.clear();
    assert_eq!(rule(&p), "GLOBAL_TYPE");
    let mut p = program(1, ints());
    p.objects[0].layout.has_destroy = true;
    p.objects[0].layout.may_reach_resource = true;
    p.objects[0].layout.allocation_words = 6;
    p.objects[0].layout.identity_offset = Some(16);
    p.objects[0].enum_cases = 0;
    p.globals.truncate(4);
    p.globals.push(Global { name: "app.const.RESOURCE".into(), ty: Type::Object(0), data: vec![
        Datum { ty: Type::Object(0), value: DatumValue::Case(0) },
    ] });
    assert_eq!(rule(&p), "GLOBAL_TYPE");
}
