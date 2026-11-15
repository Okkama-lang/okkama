//! Native handles, trusted native entries and the unresolved-provider adapter at load (#569 C7).
use super::*;
use super::p1::{P1Contracts, NATIVE_FAILURES};

fn object(name: &str, handle: bool, storage_fields: Vec<Type>) -> Object {
    let trace_layout = storage_fields.iter().enumerate()
        .filter(|(_, ty)| matches!(ty, Type::Str)).map(|(index, _)| index as u32).collect();
    Object {
        name: name.into(), fields: vec![], error_cases: vec![], enum_cases: 0, variant_cases: vec![], handle,
        role: None, implements: vec![], any_box: false,
        layout: ObjectRuntimeLayout {
            allocation_words: (1 + storage_fields.len() as u32).max(2),
            storage_fields, identity_offset: None, has_destroy: false, may_reach_resource: false,
            may_async_destroy: false, trace_layout, selection_layout: vec![],
        },
    }
}

fn buffer() -> Vec<Object> {
    vec![object("runtime.buffer.BufferState", true, vec![])]
}

fn program(objects: Vec<Object>, functions: Vec<Function>) -> Program {
    Program { objects, functions, ..Program::default() }
}

fn declaration(symbol: &str, registers: Vec<Type>, results: Vec<Type>) -> Function {
    let mut params = (0..registers.len() as u32).map(Location::Register).collect::<Vec<_>>();
    params.push(Location::Service(0));
    Function {
        symbol: symbol.into(), entry_abi: EntryAbi::Owned, registers, services: vec![ServiceKind::Packet],
        params, results, blocks: vec![], code: vec![],
        proofs: FunctionProofs {
            entry: Some(EntryContract {
                external: Some(symbol.into()), failures: NATIVE_FAILURES, ..EntryContract::default()
            }),
            ..FunctionProofs::default()
        },
    }
}

fn load(p: &Program) -> Result<(), Error> {
    verify_load(p, &P1Contracts::new(&p.objects).unwrap())
}

#[test]
fn table_natives_are_trusted_entries_from_the_table() {
    let size = declaration("runtime/buffer@1::buffer_state_size/1", vec![Type::Handle(0)], vec![Type::Int]);
    let create = declaration("runtime/buffer@1::buffer_state_create/1", vec![Type::Bytes], vec![Type::Handle(0)]);
    let hex = declaration("runtime/hex@1::hex_encode/1", vec![Type::Bytes], vec![Type::Str]);
    load(&program(buffer(), vec![size.clone(), create, hex])).unwrap();
    let contracts = P1Contracts::new(&buffer()).unwrap();
    assert_eq!(contracts.native("runtime/buffer@1::buffer_state_size/1"), Some(&size));
    // Not admitted by the companion catalogue: `any` transport and an asynchronous provider.
    assert_eq!(contracts.native("runtime/debug@1::data_dump/1"), None);
    assert_eq!(contracts.native("runtime/application@1::terminal_standard/1"), None);
    // A handle of the table needs its declaring module's handle object.
    assert_eq!(P1Contracts::new(&[]).unwrap().native("runtime/buffer@1::buffer_state_size/1"), None);
}

#[test]
fn a_table_native_cannot_change_its_signature() {
    let hex = declaration("runtime/hex@1::hex_encode/1", vec![Type::Str], vec![Type::Str]);
    let failure = load(&program(vec![], vec![hex])).unwrap_err();
    assert!(failure.message.contains("external signature/summary mismatch"), "{failure}");
    assert_eq!(failure.rule, "EXTERNAL_SIGNATURE", "{failure}");
    // A table native never falls back to the adapter, even with data transport.
    let mut size = declaration("runtime/buffer@1::buffer_state_size/1", vec![Type::Handle(0)], vec![Type::Int]);
    size.proofs.entry.as_mut().unwrap().failures = 0;
    assert!(load(&program(buffer(), vec![size])).is_err());
}

#[test]
fn handle_types_name_handle_declarations() {
    let size = declaration("runtime/buffer@1::buffer_state_size/1", vec![Type::Handle(0)], vec![Type::Int]);
    let plain = vec![object("runtime.buffer.BufferState", false, vec![])];
    assert_eq!(load(&program(plain, vec![size])).unwrap_err().rule, "HANDLE_TYPE");
    let lookup = declaration("app@1::lookup/1", vec![Type::Object(0)], vec![Type::Int]);
    assert_eq!(verify(&program(buffer(), vec![lookup])).unwrap_err().rule, "HANDLE_TYPE");
    let mut successor = declaration("app@1::lookup/1", vec![Type::Int], vec![Type::Successor(0)]);
    successor.registers[0] = Type::Int;
    assert_eq!(verify(&program(buffer(), vec![successor])).unwrap_err().rule, "HANDLE_TYPE");
    let shape = ArrayType { leaf: ArrayLeaf::Object(0), depth: 1 };
    let array = declaration("app@1::lookup/1", vec![Type::Array(shape)], vec![Type::Int]);
    assert_eq!(verify(&program(buffer(), vec![array])).unwrap_err().rule, "HANDLE_TYPE");
}

#[test]
fn an_external_entry_carries_no_code() {
    let hex = declaration("runtime/hex@1::hex_encode/1", vec![Type::Bytes], vec![Type::Str]);
    load(&program(vec![], vec![hex.clone()])).unwrap();
    let mut body = hex.clone();
    body.code = vec![Instruction::Ret(vec![])];
    body.blocks = vec![Block { pc: 0, params: body.params.clone() }];
    assert_eq!(verify(&program(vec![], vec![body])).unwrap_err().rule, "EXTERNAL_SIGNATURE");
    let mut blocks = hex;
    blocks.blocks = vec![Block { pc: 0, params: blocks.params.clone() }];
    assert_eq!(verify(&program(vec![], vec![blocks])).unwrap_err().rule, "EXTERNAL_SIGNATURE");
}

#[test]
fn a_handle_type_out_of_range_names_no_handle() {
    let size = declaration("runtime/buffer@1::buffer_state_size/1", vec![Type::Handle(1)], vec![Type::Int]);
    assert_eq!(verify(&program(buffer(), vec![size])).unwrap_err().rule, "HANDLE_TYPE");
    let create = declaration("runtime/buffer@1::buffer_state_create/1", vec![Type::Bytes], vec![Type::Handle(7)]);
    assert_eq!(verify(&program(buffer(), vec![create])).unwrap_err().rule, "HANDLE_TYPE");
}

#[test]
fn handle_declarations_have_distinct_names() {
    let two = vec![object("runtime.buffer.BufferState", true, vec![]), object("app.Port", true, vec![])];
    verify(&program(two.clone(), vec![])).unwrap();
    P1Contracts::new(&two).unwrap();
    let twins = vec![object("runtime.buffer.BufferState", true, vec![]), object("runtime.buffer.BufferState", true,
        vec![])];
    assert_eq!(verify(&program(twins.clone(), vec![])).unwrap_err().rule, "HANDLE_NAME");
    // The trusted catalogue refuses them on its own: it never picks one of the two.
    assert_eq!(P1Contracts::new(&twins).unwrap_err().rule, "HANDLE_NAME");
    // A plain object may share a handle's name without making it a handle.
    let plain = vec![object("runtime.buffer.BufferState", true, vec![]), object("runtime.buffer.BufferState", false,
        vec![])];
    verify(&program(plain.clone(), vec![])).unwrap();
    P1Contracts::new(&plain).unwrap();
}

#[test]
fn handle_declarations_hold_nothing() {
    verify(&program(buffer(), vec![])).unwrap();
    let held = vec![object("runtime.buffer.BufferState", true, vec![Type::Str])];
    assert_eq!(verify(&program(held, vec![])).unwrap_err().rule, "HANDLE_DECLARATION");
    let mut cases = buffer();
    cases[0].enum_cases = 1;
    assert_eq!(verify(&program(cases, vec![])).unwrap_err().rule, "HANDLE_DECLARATION");
}

#[test]
fn unresolved_natives_use_the_checked_adapter() {
    let objects = vec![object("app.Port", true, vec![])];
    let lookup = declaration("app@1::lookup/3", vec![Type::Str, Type::Handle(0), Type::Float],
        vec![Type::Array(ArrayType { leaf: ArrayLeaf::Str, depth: 1 })]);
    load(&program(objects.clone(), vec![lookup.clone()])).unwrap();
    // Unknown runtime symbols are unresolved providers too.
    let runtime = declaration("runtime/hex@1::hex_unknown/1", vec![Type::Str], vec![]);
    load(&program(vec![], vec![runtime])).unwrap();
    // An unknown symbol of a table module may carry that module's handles; it resolves at run time.
    let unknown = declaration("runtime/buffer@1::unknown/0", vec![], vec![Type::Handle(0)]);
    load(&program(buffer(), vec![unknown])).unwrap();
    let rejected = |function: Function, objects: Vec<Object>| {
        assert_eq!(load(&program(objects, vec![function])).unwrap_err().rule, "EXTERNAL_UNRESOLVED");
    };
    rejected(declaration("app@1::lookup/1", vec![Type::Failure], vec![]), vec![]);
    rejected(declaration("app@1::lookup/1", vec![Type::Range], vec![]), vec![]);
    rejected(declaration("app@1::lookup/2", vec![Type::Int], vec![]), vec![]);
    rejected(declaration("app@1::lookup", vec![], vec![]), vec![]);
    rejected(declaration("App@1::lookup/0", vec![], vec![]), vec![]);
    rejected(declaration("app@1::lookup/0", vec![], vec![Type::Int, Type::Int]), vec![]);
    rejected(declaration("app@1::lookup/1", vec![Type::Handle(0)], vec![]), vec![object("other.Port", true, vec![])]);
    let plain = vec![object("app.Plain", false, vec![Type::Int])];
    rejected(declaration("app@1::lookup/1", vec![Type::Object(0)], vec![]), plain);
    // The adapter is recomputed: its effect contract is not the declaration's to choose.
    let mut observed = lookup.clone();
    observed.proofs.entry.as_mut().unwrap().failures = 0;
    assert!(load(&program(objects.clone(), vec![observed])).is_err());
    let mut retained = lookup;
    retained.proofs.entry.as_mut().unwrap().retained_params = vec![0];
    assert!(load(&program(objects, vec![retained])).is_err());
}

#[test]
fn atomic_entries_follow_the_registered_value_layout() {
    use super::p1::atomic_value;
    let strings = Type::Array(ArrayType { leaf: ArrayLeaf::Str, depth: 1 });
    assert_eq!(atomic_value("array_string"), Some(strings));
    assert_eq!(atomic_value("array_array_int"), Some(Type::Array(ArrayType { leaf: ArrayLeaf::Int, depth: 2 })));
    assert_eq!(atomic_value("array_byte"), Some(Type::Bytes));
    for suffix in ["array_float", "array_char", "range", "array_", "map_int"] {
        assert_eq!(atomic_value(suffix), None, "{suffix}");
    }
    let closed = Object { error_cases: vec!["AtomicClosed.closed".into()], ..object("async.atomic.AtomicClosed", false,
        vec![Type::Int]) };
    let mut objects = vec![object("async.atomic.Atomic_array_string", false, vec![strings, Type::Bool]), closed];
    objects[0].layout.trace_layout = vec![0];
    let contracts = P1Contracts::new(&objects).unwrap();
    let names = contracts.entries().iter().map(|f| f.symbol.as_str()).collect::<Vec<_>>();
    for op in ["new_value", "value", "replace", "compare_and_replace", "closed", "close"] {
        assert!(names.contains(&format!("okkama.atomic.Atomic.{op}$array_string").as_str()), "{op}");
    }
    let cas = contracts.entries().iter().find(|f| f.symbol == "okkama.atomic.Atomic.compare_and_replace$array_string")
        .unwrap();
    assert_eq!((cas.entry_abi.clone(), cas.registers.clone(), cas.results.clone()),
        (EntryAbi::Owned, vec![Type::Object(0), strings, strings], vec![Type::Bool]));
    // A layout that differs from the registered value is rejected, not repaired.
    let mut wrong = objects.clone();
    wrong[0].layout.storage_fields = vec![Type::Str, Type::Bool];
    assert!(P1Contracts::new(&wrong).is_err());
}

#[test]
fn handles_round_trip_in_binary_and_assembly() {
    let size = declaration("runtime/buffer@1::buffer_state_size/1", vec![Type::Handle(0)], vec![Type::Int]);
    let p = program(buffer(), vec![size]);
    assert_eq!(decode(&encode(&p).unwrap()).unwrap(), p);
    let text = disassemble(&p).unwrap();
    assert_eq!(assemble(&text).unwrap(), p);
    // `handle` is an optional field: an object that is no handle encodes as before.
    let plain = program(vec![object("app.Plain", false, vec![])], vec![]);
    assert!(!disassemble(&plain).unwrap().contains("handle"));
}
