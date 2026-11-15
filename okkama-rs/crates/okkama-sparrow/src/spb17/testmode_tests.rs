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

#[test]
fn a_test_is_a_body_of_the_program_not_an_external_entry() {
    // An external test: the host would run it, not the program. Its body stays, so only the external
    // marker differs from a valid test.
    let mut p = profile(is_ints());
    p.functions[1].proofs.entry.as_mut().unwrap().external = Some("app@1::check/0".into());
    reject(&p, "TEST_ENTRY");
    // A test function without code.
    let mut p = profile(is_ints());
    let f = &mut p.functions[1];
    f.code.clear();
    f.blocks.clear();
    f.proofs.roots.clear();
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
    let failure = verify_load(&p, &contracts).unwrap_err();
    assert!(failure.message.contains("external signature/summary mismatch"), "{failure}");
    assert_eq!(failure.rule, "EXTERNAL_SIGNATURE", "{failure}");
}

/// A declaration with `storage` fields named `f<i>`; `destroy` makes it a resource object (an identity).
fn object(name: &str, storage: Vec<Type>, destroy: bool, handle: bool) -> Object {
    let count = storage.len() as u32;
    Object {
        name: name.into(), error_cases: vec![], enum_cases: 0, variant_cases: vec![], handle,
        fields: (0..count).map(|index| Field { name: format!("f{index}"), storage: index, once: false }).collect(),
        layout: ObjectRuntimeLayout {
            allocation_words: (1 + count + if destroy { 4 } else { 0 }).max(2),
            identity_offset: destroy.then_some(8 * (1 + count)),
            trace_layout: (0..count).filter(|&index| storage[index as usize].traced()).collect(),
            storage_fields: storage, has_destroy: destroy, may_reach_resource: destroy, may_async_destroy: false,
            selection_layout: vec![],
        },
    }
}

/// `test.is`/`test.isnt` of the two `ty` parameters of a helper beside the test, with the declarations
/// `objects`.
fn compare_objects(op: DataOp, ty: Type, objects: Vec<Object>) -> Program {
    let mut helper = tests::empty_function();
    helper.symbol = "same".into();
    helper.registers = vec![ty, ty, Type::Bool];
    helper.params = vec![Location::Register(0), Location::Register(1)];
    helper.blocks[0].params = helper.params.clone();
    helper.proofs.entry = Some(EntryContract { failures: 2, ..EntryContract::default() });
    helper.code = vec![data(op, vec![0, 1], 2, Immediate::None)];
    // A successor parameter carries a must-use permission: take both after the check.
    if let Type::Successor(id) = ty {
        helper.registers.extend([Type::Object(id), Type::Object(id)]);
        helper.code.push(data(DataOp::Move, vec![0], 3, Immediate::None));
        helper.code.push(data(DataOp::Move, vec![1], 4, Immediate::None));
    }
    helper.code.push(Instruction::Ret(vec![]));
    let live = live_locations(&helper).unwrap();
    helper.proofs.roots = live.into_iter().enumerate().map(|(pc, locations)| RootRequirement {
        pc: pc as u32, locations: locations.into_iter().collect(),
    }).collect();
    let mut p = profile(is_ints());
    p.functions.push(helper);
    // Every resource declaration has its destroy entry, so the program is complete apart from the check.
    for (id, object) in objects.iter().enumerate().filter(|(_, object)| object.layout.has_destroy) {
        let mut destroy = tests::empty_function();
        destroy.symbol = format!("{}.destroy", object.name);
        destroy.registers = vec![Type::Object(id as u32)];
        destroy.params = vec![Location::Register(0)];
        destroy.blocks[0].params = destroy.params.clone();
        destroy.proofs.entry = Some(EntryContract { destroy: Some(id as u32), ..EntryContract::default() });
        destroy.proofs.roots = vec![RootRequirement { pc: 0, locations: vec![] }];
        p.functions.push(destroy);
    }
    p.objects = objects;
    p
}

#[test]
fn test_checks_compare_closed_plain_objects() {
    let point = || object("Point", vec![Type::Int, Type::Str], false, false);
    accept(&compare_objects(DataOp::TestIs, Type::Object(0), vec![point()]));
    accept(&compare_objects(DataOp::TestIsnt, Type::Successor(0), vec![point()]));
    // A plain object holding a plain object stays comparable.
    let line = object("Line", vec![Type::Object(0), Type::Object(0)], false, false);
    accept(&compare_objects(DataOp::TestIs, Type::Object(1), vec![point(), line]));
}

#[test]
fn test_checks_reject_objects_that_value_equality_rejects() {
    // A resource object has an identity: `val.eq` refuses it, and so does `test.is`.
    let file = || object("File", vec![Type::Int], true, false);
    reject(&compare_objects(DataOp::TestIs, Type::Object(0), vec![file()]), "TEST_OPERAND");
    reject(&compare_objects(DataOp::TestIsnt, Type::Successor(0), vec![file()]), "TEST_OPERAND");
    // A native handle field cannot be compared.
    let state = object("runtime.buffer.BufferState", vec![], false, true);
    let holder = object("Holder", vec![Type::Handle(0)], false, false);
    reject(&compare_objects(DataOp::TestIs, Type::Object(1), vec![state, holder]), "TEST_OPERAND");
    // Neither can a failure payload.
    let carrier = object("Carrier", vec![Type::Error], false, false);
    reject(&compare_objects(DataOp::TestIsnt, Type::Object(0), vec![carrier]), "TEST_OPERAND");
}
