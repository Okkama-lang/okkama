//! Native handles, trusted native entries and the unresolved-provider adapter at load (#569 C7).
use super::*;
use super::p1::{P1Contracts, NATIVE_FAILURES};

fn object(name: &str, handle: bool, storage_fields: Vec<Type>) -> Object {
    let trace_layout = storage_fields.iter().enumerate()
        .filter(|(_, ty)| matches!(ty, Type::Str)).map(|(index, _)| index as u32).collect();
    Object {
        name: name.into(), fields: vec![], error_cases: vec![], enum_cases: 0, variant_cases: vec![], handle,
        role: None, implements: vec![], any_box: false, enum_names: vec![],
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

/// Float arrays cross the native bridge as raw binary64 elements (`statistics.exact-samples`, #710).
#[test]
fn table_natives_take_and_return_float_arrays() {
    let floats = Type::Array(ArrayType { leaf: ArrayLeaf::Float, depth: 1 });
    let mean = declaration("runtime/statistics@1::statistics_mean/1", vec![floats], vec![Type::Float]);
    let quantiles = declaration("runtime/statistics@1::statistics_quantiles/3",
        vec![floats, Type::Int, Type::Str], vec![floats]);
    load(&program(vec![], vec![mean.clone(), quantiles.clone()])).unwrap();
    let contracts = P1Contracts::new(&[]).unwrap();
    assert_eq!(contracts.native("runtime/statistics@1::statistics_mean/1"), Some(&mean));
    assert_eq!(contracts.native("runtime/statistics@1::statistics_quantiles/3"), Some(&quantiles));
}

/// Arrays of a module's handles cross the native bridge as arrays of registry cells (json's parsed nodes, #751):
/// the trusted entry is the table's, with the handle leaf of the module's handle declaration.
#[test]
fn table_natives_take_and_return_handle_arrays() {
    let json = || vec![object("runtime.json.JsonNodeState", true, vec![]), object("app.Node", true, vec![])];
    let nodes = |id| Type::Array(ArrayType { leaf: ArrayLeaf::Handle(id), depth: 1 });
    let items = declaration("runtime/json@1::json_node_items/1", vec![Type::Handle(0)], vec![nodes(0)]);
    let array = declaration("runtime/json@1::json_array_node/1", vec![nodes(0)], vec![Type::Handle(0)]);
    let object_node = declaration("runtime/json@1::json_object_node/2",
        vec![Type::Array(ArrayType { leaf: ArrayLeaf::Str, depth: 1 }), nodes(0)], vec![Type::Handle(0)]);
    load(&program(json(), vec![items.clone(), array.clone(), object_node.clone()])).unwrap();
    let contracts = P1Contracts::new(&json()).unwrap();
    assert_eq!(contracts.native("runtime/json@1::json_node_items/1"), Some(&items));
    assert_eq!(contracts.native("runtime/json@1::json_array_node/1"), Some(&array));
    assert_eq!(contracts.native("runtime/json@1::json_object_node/2"), Some(&object_node));
    // The leaf is the table's: another module's handles, or a single handle for the array, differ from it.
    let other = declaration("runtime/json@1::json_node_items/1", vec![Type::Handle(0)], vec![nodes(1)]);
    assert_eq!(load(&program(json(), vec![other])).unwrap_err().rule, "EXTERNAL_SIGNATURE");
    let single = declaration("runtime/json@1::json_array_node/1", vec![Type::Handle(0)], vec![Type::Handle(0)]);
    assert_eq!(load(&program(json(), vec![single])).unwrap_err().rule, "EXTERNAL_SIGNATURE");
}

fn with_maps(objects: Vec<Object>, functions: Vec<Function>, maps: &[(Type, Type)]) -> Program {
    let composites = maps.iter().map(|&(key, value)| Composite::Map { key, value }).collect();
    Program { composites, ..program(objects, functions) }
}

/// Table natives with a map parameter or result have a data adapter (#752): the table's signature, where each map
/// is a composite of the program with the table's key and value types.
#[test]
fn table_natives_take_and_return_maps() {
    let regex = || vec![object("runtime.regex.RegexState", true, vec![])];
    let prepare = declaration("runtime/regex@1::regex_state_prepare/1", vec![Type::Handle(0)], vec![Type::Map(0)]);
    load(&with_maps(regex(), vec![prepare.clone()], &[(Type::Str, Type::Int)])).unwrap();
    let normalize = declaration("runtime/environment@1::environment_normalize/1", vec![Type::Map(1)],
        vec![Type::Map(1)]);
    let maps = [(Type::Str, Type::Int), (Type::Str, Type::Str)];
    load(&with_maps(vec![], vec![normalize.clone()], &maps)).unwrap();
    // The adapter is the trusted signature over the program's composites, recomputed from the table.
    let contracts = P1Contracts::new(&regex()).unwrap();
    let composites = [Composite::Map { key: Type::Str, value: Type::Int }];
    assert_eq!(contracts.native("runtime/regex@1::regex_state_prepare/1"), None);
    assert_eq!(contracts.adapter(&prepare, &composites), Some(prepare.clone()));
    // A map of other key or value types, or another collection, is not the table's.
    let unresolved = |p: &Program| assert_eq!(load(p).unwrap_err().rule, "EXTERNAL_UNRESOLVED");
    unresolved(&with_maps(regex(), vec![prepare.clone()], &[(Type::Str, Type::Str)]));
    unresolved(&with_maps(regex(), vec![prepare.clone()], &[(Type::Int, Type::Int)]));
    unresolved(&with_maps(vec![], vec![normalize.clone()], &[(Type::Str, Type::Int), (Type::Str, Type::Int)]));
    let mut builder = with_maps(regex(), vec![prepare.clone()], &[]);
    builder.composites = vec![Composite::Builder { array: Type::Array(ArrayType { leaf: ArrayLeaf::Int, depth: 1 }) }];
    builder.functions[0].results = vec![Type::Builder(0)];
    unresolved(&builder);
    let mut array = prepare.clone();
    array.results = vec![Type::Array(ArrayType { leaf: ArrayLeaf::Int, depth: 1 })];
    unresolved(&with_maps(regex(), vec![array], &[(Type::Str, Type::Int)]));
    // Arity and the rest of the transport stay the table's.
    let mut extra = normalize.clone();
    extra.registers.push(Type::Int);
    extra.params.insert(1, Location::Register(1));
    unresolved(&with_maps(vec![], vec![extra], &maps));
    let mut port = prepare;
    port.registers = vec![Type::Int];
    unresolved(&with_maps(regex(), vec![port], &[(Type::Str, Type::Int)]));
    // Its effect contract is not the declaration's to choose either.
    let mut observed = normalize;
    observed.proofs.entry.as_mut().unwrap().failures = 0;
    assert_eq!(load(&with_maps(vec![], vec![observed], &maps)).unwrap_err().rule, "EXTERNAL_SIGNATURE");
}

/// An unresolved provider may carry maps of plain data the adapter carries (#752), and no map of anything else.
#[test]
fn unresolved_natives_carry_maps_of_plain_data() {
    let objects = || vec![object("app.Port", true, vec![])];
    let lookup = declaration("app@1::lookup/1", vec![Type::Map(0)], vec![Type::Map(1)]);
    let strings = Type::Array(ArrayType { leaf: ArrayLeaf::Str, depth: 1 });
    load(&with_maps(objects(), vec![lookup.clone()], &[(Type::Str, Type::Int), (Type::Int, strings)])).unwrap();
    let unresolved = |maps: &[(Type, Type)]| {
        let failure = load(&with_maps(objects(), vec![lookup.clone()], maps)).unwrap_err();
        assert_eq!(failure.rule, "EXTERNAL_UNRESOLVED", "{maps:?}: {failure}");
    };
    // A handle is no plain data, even of the provider's module; neither is a map.
    unresolved(&[(Type::Str, Type::Int), (Type::Str, Type::Handle(0))]);
    unresolved(&[(Type::Str, Type::Int), (Type::Str, Type::Map(0))]);
    unresolved(&[(Type::Str, Type::Int), (Type::Str, Type::Range)]);
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

/// A native entry fails with the provider's `NativeFailure` or a program failure of the call (a missing provider,
/// a refused permission), never with a thrown value or a panic (#719, language spec 9.5); a table native and an
/// unresolved provider declare exactly that mask, and a declaration that narrows it to native failures is refused.
#[test]
fn native_entries_declare_native_and_program_failures() {
    let hex = declaration("runtime/hex@1::hex_encode/1", vec![Type::Bytes], vec![Type::Str]);
    let lookup = declaration("app@1::lookup/1", vec![Type::Int], vec![Type::Int]);
    for function in [hex, lookup] {
        load(&program(vec![], vec![function.clone()])).unwrap();
        for mask in [8, 2, 1 | 2 | 8, 2 | 4 | 8] {
            let mut narrowed = function.clone();
            narrowed.proofs.entry.as_mut().unwrap().failures = mask;
            let failure = load(&program(vec![], vec![narrowed])).unwrap_err();
            assert_eq!(failure.rule, "EXTERNAL_SIGNATURE", "{} mask {mask}: {failure}", function.symbol);
        }
    }
}

#[test]
fn atomic_entries_follow_the_registered_value_layout() {
    use super::p1::atomic_value;
    let strings = Type::Array(ArrayType { leaf: ArrayLeaf::Str, depth: 1 });
    assert_eq!(atomic_value("array_string"), Some(strings));
    assert_eq!(atomic_value("array_array_int"), Some(Type::Array(ArrayType { leaf: ArrayLeaf::Int, depth: 2 })));
    assert_eq!(atomic_value("array_byte"), Some(Type::Bytes));
    // Every concrete plain specialization is registered (P2.4 §4.8), arrays of every plain scalar included.
    let arrays = [("array_float", ArrayLeaf::Float), ("array_char", ArrayLeaf::Char), ("array_rat", ArrayLeaf::Rat)];
    for (suffix, leaf) in arrays {
        assert_eq!(atomic_value(suffix), Some(Type::Array(ArrayType { leaf, depth: 1 })), "{suffix}");
    }
    for suffix in ["range", "array_", "map_int"] {
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

/// An Error case `name` with payload fields of `types`, one storage word each.
fn error_case(name: &str, types: Vec<Type>) -> Object {
    let fields = (0..types.len() as u32).map(|storage| Field { name: format!("f{storage}"), storage, once: false })
        .collect();
    Object { error_cases: vec![name.into()], fields, ..object(name, false, types) }
}

fn p1(objects: Vec<Object>) -> Result<P1Contracts, Error> {
    P1Contracts::new(&objects)
}

#[test]
fn p1_error_text_admits_the_layouts_of_error_variants() {
    // An Error enum, and a case of an Error variant without payload (no storage).
    p1(vec![Object { error_cases: vec!["E.a".into(), "E.b".into()], ..object("E", false, vec![Type::Int]) }])
        .unwrap();
    p1(vec![error_case("F.plain", vec![])]).unwrap();
    // Scalar payloads.
    p1(vec![error_case("F.all", vec![Type::Str, Type::Int, Type::Bool, Type::Byte, Type::Float, Type::Rat])])
        .unwrap();
    // A plain enum that names its cases, an Error enum, and an Error variant.
    let limit = Object { enum_cases: 2, enum_names: vec!["L.a".into(), "L.b".into()],
        ..object("L", false, vec![Type::Int]) };
    p1(vec![limit, error_case("F.kind", vec![Type::Object(0)])]).unwrap();
    let inner = Object { error_cases: vec!["I.a".into()], ..object("I", false, vec![Type::Int]) };
    p1(vec![inner, error_case("F.cause", vec![Type::Object(0)])]).unwrap();
    let variant = Object { variant_cases: vec![1], ..object("V", false, vec![]) };
    p1(vec![variant, error_case("V.one", vec![]), error_case("F.nested", vec![Type::Variant(0)])]).unwrap();
}

/// An Error payload may be a plain variant (language spec: enums whose payloads satisfy the same rule; Tomato
/// shows `Fault.major(1, Shape.rect(2, 3))`, #763): its cases are named and their payloads shown, at any depth,
/// a recursive variant included.
#[test]
fn p1_error_text_admits_a_named_plain_variant_payload() {
    let payload = |name: &str, types: Vec<Type>| {
        let fields = (0..types.len() as u32).map(|storage| Field { name: format!("f{storage}"), storage, once: false })
            .collect();
        Object { fields, ..object(name, false, types) }
    };
    let shape = Object { variant_cases: vec![1, 2], enum_names: vec!["Shape.dot".into(), "Shape.rect".into()],
        ..object("Shape", false, vec![]) };
    p1(vec![shape.clone(), object("Shape.dot", false, vec![]), payload("Shape.rect", vec![Type::Int, Type::Int]),
        error_case("Fault.major", vec![Type::Int, Type::Variant(0)])]).unwrap();
    // A variant whose case holds the variant itself (a list of shapes).
    let list = Object { variant_cases: vec![1, 2], enum_names: vec!["List.end".into(), "List.cons".into()],
        ..object("List", false, vec![]) };
    p1(vec![list, object("List.end", false, vec![]), payload("List.cons", vec![Type::Str, Type::Variant(0)]),
        error_case("Fault.list", vec![Type::Variant(0)])]).unwrap();
}

#[test]
fn p1_error_text_refuses_payloads_it_cannot_show() {
    let refused = |objects: Vec<Object>| {
        let failure = p1(objects).unwrap_err();
        assert!(failure.message.contains("invalid P1 plain Error layout"), "{failure}");
    };
    // A plain enum without its case names has no text.
    let unnamed = Object { enum_cases: 2, ..object("L", false, vec![Type::Int]) };
    refused(vec![unnamed, error_case("F.kind", vec![Type::Object(0)])]);
    // Neither has an Error role value (as on Tomato), a plain variant, a plain object or a char.
    refused(vec![error_case("F.any", vec![Type::Error])]);
    // A plain variant without case names (#763: one with names is shown).
    let plain = Object { variant_cases: vec![1], ..object("V", false, vec![]) };
    refused(vec![plain, object("V.one", false, vec![]), error_case("F.v", vec![Type::Variant(0)])]);
    // A named plain variant whose case holds a payload the text cannot show (a char).
    let named = Object { variant_cases: vec![1], enum_names: vec!["V.one".into()], ..object("V", false, vec![]) };
    let char_case = Object { fields: vec![Field { name: "c".into(), storage: 0, once: false }],
        ..object("V.one", false, vec![Type::Char]) };
    refused(vec![named, char_case, error_case("F.v", vec![Type::Variant(0)])]);
    // Names that do not cover every case.
    let short = Object { variant_cases: vec![1, 2], enum_names: vec!["V.one".into()],
        ..object("V", false, vec![]) };
    refused(vec![short, object("V.one", false, vec![]), object("V.two", false, vec![]),
        error_case("F.v", vec![Type::Variant(0)])]);
    refused(vec![object("P", false, vec![Type::Int]), error_case("F.p", vec![Type::Object(0)])]);
    refused(vec![error_case("F.c", vec![Type::Char])]);
    // A case without payload holds no storage; a payload case's storage words are its fields.
    refused(vec![Object { fields: vec![], ..error_case("F.tag", vec![Type::Str]) }]);
    refused(vec![Object { fields: vec![Field { name: "s".into(), storage: 0, once: false }],
        ..error_case("F.extra", vec![Type::Str, Type::Str]) }]);
    // An Error enum keeps its case number; only a single case may hold no storage.
    refused(vec![Object { error_cases: vec!["E.a".into(), "E.b".into()], ..object("E", false, vec![]) }]);
    // A payload case has one case.
    refused(vec![Object { error_cases: vec!["F.a".into(), "F.b".into()], ..error_case("F", vec![Type::Str]) }]);
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
