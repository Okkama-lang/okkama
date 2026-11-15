//! The test profile in SPB 17 (#569): the tests table, test operations and test intrinsics.
use super::*;

fn reject(p: &Program, rule: &str) {
    let contracts = p1::P1Contracts::new(&p.objects).unwrap();
    let failure = verify_load(p, &contracts).unwrap_err();
    assert_eq!(failure.rule, rule, "{failure}");
}

fn accept(p: &Program) {
    let contracts = p1::P1Contracts::new(&p.objects).unwrap();
    verify_load(p, &contracts).unwrap();
}

fn data(op: DataOp, inputs: Vec<u32>, output: u32, immediate: Immediate) -> Instruction {
    Instruction::Data { op, inputs, output: Some(output), immediate }
}

fn constant(output: u32, value: Constant) -> Instruction {
    data(DataOp::Const, vec![], output, Immediate::Constant(value))
}

/// An observation function with `registers` and `code` before its `ret`, declaring `failures`.
fn function(symbol: &str, registers: Vec<Type>, code: Vec<Instruction>, failures: u32) -> Function {
    let mut f = tests::empty_function();
    f.symbol = symbol.into();
    f.registers = registers;
    f.proofs.entry = Some(EntryContract { failures, ..EntryContract::default() });
    f.code = code;
    f.code.push(Instruction::Ret(vec![]));
    let live = live_locations(&f).unwrap();
    f.proofs.roots = live.into_iter().enumerate().map(|(pc, locations)| RootRequirement {
        pc: pc as u32, locations: locations.into_iter().collect(),
    }).collect();
    f
}

/// `test.is(1, 1)` in data form: a fallible check in a function without service locations.
fn check(op: DataOp, registers: Vec<Type>, values: Vec<Constant>) -> Function {
    let mut code = values.into_iter().enumerate().map(|(index, value)| constant(index as u32, value))
        .collect::<Vec<_>>();
    let inputs = (0..code.len() as u32).collect::<Vec<_>>();
    let output = registers.len() as u32 - 1;
    code.push(data(op, inputs, output, Immediate::None));
    function("$test0", registers, code, 2)
}

fn int(value: &str) -> Constant {
    Constant::Int(value.into())
}

/// `main` (0) and the test `$test0` (1) in a test profile.
fn profile(test: Function) -> Program {
    Program {
        module: "app".into(),
        functions: vec![tests::empty_function(), test],
        entry: Some(0),
        tests: Some(vec![Test { name: "sum".into(), function: 1 }]),
        ..Program::default()
    }
}

fn is_ints() -> Function {
    check(DataOp::TestIs, vec![Type::Int, Type::Int, Type::Bool], vec![int("1"), int("1")])
}

#[test]
fn tests_table_roundtrips_in_binary_and_assembly() {
    let p = profile(is_ints());
    accept(&p);
    assert_eq!(decode(&encode(&p).unwrap()).unwrap(), p);
    let text = disassemble(&p).unwrap();
    assert!(text.contains("tests") && text.contains("test.check"), "{text}");
    assert_eq!(assemble(&text).unwrap(), p);
    // An empty suite stays a test profile; a production program prints no tests table.
    let empty = Program { tests: Some(vec![]), ..p.clone() };
    assert_eq!(decode(&encode(&empty).unwrap()).unwrap(), empty);
    assert_eq!(assemble(&disassemble(&empty).unwrap()).unwrap(), empty);
    let production = Program { functions: vec![tests::empty_function()], tests: None, ..p };
    assert!(!disassemble(&production).unwrap().contains("tests"));
}

#[test]
fn test_operations_need_the_test_profile() {
    let mut p = profile(is_ints());
    p.tests = None;
    reject(&p, "TEST_PROFILE");
    let fail = check(DataOp::TestFail, vec![Type::Str, Type::Bool], vec![Constant::Str("no".into())]);
    accept(&profile(fail.clone()));
    let mut p = profile(fail);
    p.tests = None;
    reject(&p, "TEST_PROFILE");
}

#[test]
fn test_operations_type_their_operands() {
    let ok = check(DataOp::TestOk, vec![Type::Bool, Type::Bool], vec![Constant::Bool(true)]);
    accept(&profile(ok));
    let ok_int = check(DataOp::TestOk, vec![Type::Int, Type::Bool], vec![int("1")]);
    reject(&profile(ok_int), "TEST_OPERAND");
    let mixed = check(DataOp::TestIsnt, vec![Type::Int, Type::Str, Type::Bool],
        vec![int("1"), Constant::Str("1".into())]);
    reject(&profile(mixed), "TEST_OPERAND");
    let descriptors = check(DataOp::TestIs, vec![Type::Descriptor, Type::Descriptor, Type::Bool],
        vec![Constant::Descriptor(0), Constant::Descriptor(0)]);
    let mut p = profile(descriptors);
    p.objects.push(Object {
        name: "Empty".into(), error_cases: vec![], enum_cases: 0, variant_cases: vec![], handle: false,
        role: None, implements: vec![],
        fields: vec![],
        layout: ObjectRuntimeLayout {
            storage_fields: vec![], allocation_words: 2, identity_offset: None, has_destroy: false,
            may_reach_resource: false, may_async_destroy: false, trace_layout: vec![], selection_layout: vec![],
        },
    });
    reject(&p, "TEST_OPERAND");
    let held_int = check(DataOp::TestIs, vec![Type::Int, Type::Int, Type::Int], vec![int("1"), int("1")]);
    reject(&profile(held_int), "TEST_OPERAND");
}

#[test]
fn a_check_fails_only_with_a_program_failure() {
    let mut test = is_ints();
    test.proofs.entry = Some(EntryContract::default());
    reject(&profile(test), "ENTRY_FAILURE_MASK");
}

#[test]
fn tests_table_names_distinct_test_bodies() {
    let mut p = profile(is_ints());
    p.tests.as_mut().unwrap().push(Test { name: "sum".into(), function: 1 });
    reject(&p, "TEST_TABLE");
    let mut p = profile(is_ints());
    p.tests.as_mut().unwrap()[0].name.clear();
    reject(&p, "TEST_TABLE");
    let mut p = profile(is_ints());
    p.tests.as_mut().unwrap()[0].function = 2;
    reject(&p, "TEST_TABLE");
    let mut p = profile(is_ints());
    p.tests.as_mut().unwrap().push(Test { name: "again".into(), function: 1 });
    reject(&p, "TEST_TABLE");
}

#[test]
fn a_test_is_a_void_observation_body_beside_the_entry() {
    let mut p = profile(is_ints());
    p.tests.as_mut().unwrap()[0].function = 0;
    reject(&p, "TEST_ENTRY");
    let mut p = profile(is_ints());
    let f = &mut p.functions[1];
    f.results = vec![Type::Int];
    f.code.pop();
    f.code.push(Instruction::Ret(vec![0]));
    reject(&p, "TEST_ENTRY");
    let mut p = profile(is_ints());
    let f = &mut p.functions[1];
    f.params = vec![Location::Register(0)];
    f.blocks[0].params = f.params.clone();
    f.code.remove(0);
    reject(&p, "TEST_ENTRY");
    let mut p = profile(is_ints());
    p.functions[1].entry_abi = EntryAbi::Owned;
    reject(&p, "TEST_ENTRY");
}

/// The bodyless declaration of a test intrinsic, as the trusted P1 catalogue states it.
fn intrinsic(name: &str, registers: Vec<Type>) -> Function {
    Function {
        symbol: name.into(),
        entry_abi: EntryAbi::Observation,
        params: (0..registers.len() as u32).map(Location::Register).collect(),
        registers,
        services: vec![],
        results: vec![],
        blocks: vec![],
        proofs: FunctionProofs {
            entry: Some(EntryContract { external: Some(name.into()), failures: 2, ..EntryContract::default() }),
            ..FunctionProofs::default()
        },
        code: vec![],
    }
}

#[test]
fn test_intrinsics_need_the_test_profile() {
    let mut p = profile(is_ints());
    for (name, registers) in [
        ("okkama.test.skip", vec![Type::Str]),
        ("okkama.test.close", vec![Type::Float, Type::Float, Type::Float]),
        ("okkama.test.text", vec![Type::Str, Type::Str]),
        ("okkama.test.bytes", vec![Type::Bytes, Type::Bytes]),
    ] {
        p.functions.push(intrinsic(name, registers));
    }
    accept(&p);
    let mut production = p.clone();
    production.tests = None;
    production.functions[1] = function("helper", vec![], vec![], 0);
    reject(&production, "TEST_PROFILE");
    // The catalogue fixes each intrinsic's operands.
    let mut p = profile(is_ints());
    p.functions.push(intrinsic("okkama.test.skip", vec![Type::Int]));
    let contracts = p1::P1Contracts::new(&p.objects).unwrap();
    assert!(verify_load(&p, &contracts).unwrap_err().message.contains("external signature/summary mismatch"));
}
