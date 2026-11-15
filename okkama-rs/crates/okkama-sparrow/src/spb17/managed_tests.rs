//! Managed arrays, maps and variants (resource references they do not own), and possibly unbound
//! construction placeholders, in the load gate (#622).
use super::*;

/// 0 = Res (destroy), 1 = Some { value: Res }, 2 = None, 3 = Maybe<Res> (cases 1, 2),
/// 4 = Holder { items: @Res } (selected), 5 = Point { x: int }.
fn objects() -> Vec<Object> {
    let object = |name: &str, fields: Vec<Type>, destroy: bool, reaches: bool, selected: Vec<u32>| {
        let trace_layout = fields.iter().enumerate().filter_map(|(index, ty)| ty.traced().then_some(index as u32))
            .collect();
        Object {
            name: name.into(), error_cases: vec![], enum_cases: 0, variant_cases: vec![], handle: false,
            role: None, implements: vec![], any_box: false,
            fields: (0..fields.len() as u32).map(|storage| Field { name: format!("f{storage}"), storage, once: false })
                .collect(),
            layout: ObjectRuntimeLayout {
                allocation_words: (1 + fields.len() as u32 + 4 * u32::from(destroy)).max(2),
                identity_offset: destroy.then_some(8 * (1 + fields.len() as u32)),
                storage_fields: fields, has_destroy: destroy, may_reach_resource: reaches, may_async_destroy: false,
                trace_layout, selection_layout: selected,
            },
        }
    };
    let res = Type::Object(0);
    vec![
        object("Res", vec![], true, true, vec![]),
        object("Some", vec![res], false, true, vec![0]),
        object("None", vec![], false, false, vec![]),
        Object { variant_cases: vec![1, 2], ..object("Maybe", vec![], false, true, vec![]) },
        object("Holder", vec![Type::Array(ArrayType { leaf: ArrayLeaf::Object(0), depth: 1 })], false, true, vec![0]),
        object("Point", vec![Type::Int], false, false, vec![]),
    ]
}

const RES_ARRAY: Type = Type::Array(ArrayType { leaf: ArrayLeaf::Object(0), depth: 1 });

fn program() -> Program {
    let mut f = tests::empty_function();
    f.registers = vec![RES_ARRAY, Type::Variant(3)];
    f.params = vec![Location::Register(0), Location::Register(1)];
    f.blocks[0].params = f.params.clone();
    Program {
        objects: objects(),
        functions: vec![f],
        composites: vec![Composite::Map { key: Type::Str, value: Type::Object(0) }],
        ..Program::default()
    }
}

fn reject(p: &Program, rule: &str) {
    let failure = verify(p).unwrap_err();
    assert_eq!(failure.rule, rule, "{failure}");
}

#[test]
fn managed_collections_and_variants_are_admitted() {
    verify(&program()).unwrap();
}

#[test]
fn managed_array_leaves_are_resource_objects_or_plain_data() {
    let mut p = program();
    p.functions[0].registers[0] = Type::Array(ArrayType { leaf: ArrayLeaf::Variant(3), depth: 1 });
    reject(&p, "ARRAY_PLAIN");
}

#[test]
fn managed_map_values_are_resource_objects_or_plain_data() {
    let mut p = program();
    p.composites[0] = Composite::Map { key: Type::Str, value: Type::Variant(3) };
    reject(&p, "MAP_PLAIN");
}

#[test]
fn fields_holding_resource_references_are_selected() {
    let mut p = program();
    p.objects[4].layout.selection_layout.clear();
    reject(&p, "COLLECTION_RETENTION");
    let mut p = program();
    p.objects[4].layout.may_reach_resource = false;
    reject(&p, "COLLECTION_RETENTION");
    // A variant field that may hold a resource reference is selected too.
    let mut p = program();
    p.objects[5].layout.storage_fields = vec![Type::Variant(3)];
    p.objects[5].layout.trace_layout = vec![0];
    reject(&p, "COLLECTION_RETENTION");
}

#[test]
fn variant_cases_are_not_identities_and_declare_their_retention() {
    let mut p = program();
    p.objects[3].layout.may_reach_resource = false;
    reject(&p, "VARIANT_CASES");
    let mut p = program();
    p.objects[3].variant_cases = vec![0, 2];
    reject(&p, "VARIANT_CASES");
}

#[test]
fn managed_values_are_not_compared_or_differenced() {
    let mut p = program();
    p.functions[0].registers.push(Type::Bool);
    p.functions[0].results = vec![Type::Bool];
    p.functions[0].code = vec![
        Instruction::Data { op: DataOp::ValEq, inputs: vec![0, 0], output: Some(2), immediate: Immediate::None },
        Instruction::Ret(vec![2]),
    ];
    reject(&p, "VAL_EQ_TYPE");
    let mut p = program();
    p.functions[0].registers.push(RES_ARRAY);
    p.functions[0].code = vec![
        Instruction::Data { op: DataOp::ArrayDifference, inputs: vec![0, 0], output: Some(2),
            immediate: Immediate::None },
        Instruction::Ret(vec![]),
    ];
    reject(&p, "ARRAY_OP_TYPE");
}

#[test]
fn resource_references_reach_resources() {
    let p = program();
    assert!(verify::reaches_resource(&p, Type::Object(0)));
    assert!(verify::reaches_resource(&p, RES_ARRAY));
    assert!(verify::reaches_resource(&p, Type::Array(ArrayType { leaf: ArrayLeaf::Object(0), depth: 3 })));
    assert!(verify::reaches_resource(&p, Type::Map(0)));
    assert!(verify::reaches_resource(&p, Type::Variant(3)));
    assert!(!verify::reaches_resource(&p, Type::Object(5)));
    assert!(!verify::reaches_resource(&p, Type::Array(ArrayType { leaf: ArrayLeaf::Int, depth: 1 })));
    assert!(!verify::reaches_resource(&p, Type::Str));
}

#[test]
fn selected_identities_never_lose_unproved_elements() {
    let p = program();
    let mut evidence = evidence::Evidence::new(&p, &p.functions[0]);
    // A managed array or resource variant whose elements no array.new proves has no static inventory.
    assert_eq!(evidence.selected_identities(Location::Register(0)), None);
    assert_eq!(evidence.selected_identities(Location::Register(1)), None);
    let mut f = p.functions[0].clone();
    f.registers.push(Type::Array(ArrayType { leaf: ArrayLeaf::Int, depth: 1 }));
    f.params.push(Location::Register(2));
    let p = Program { functions: vec![f], ..p };
    let mut evidence = evidence::Evidence::new(&p, &p.functions[0]);
    assert_eq!(evidence.selected_identities(Location::Register(2)), Some(Default::default()));
}

/// A native handle declaration (no fields, no resources), appended as object 6.
fn handle_object() -> Object {
    Object {
        name: "Native".into(), error_cases: vec![], enum_cases: 0, variant_cases: vec![], handle: true, role: None,
        implements: vec![], any_box: false,
        fields: vec![],
        layout: ObjectRuntimeLayout {
            allocation_words: 2, storage_fields: vec![], identity_offset: None, has_destroy: false,
            may_reach_resource: false, may_async_destroy: false, trace_layout: vec![], selection_layout: vec![],
        },
    }
}

/// A map value that declares neither a destroy nor a resource is plain only if its whole graph is, as an array
/// element is (`ARRAY_PLAIN`): a handle or a failure in it is not plain data (#692).
#[test]
fn managed_map_values_follow_the_array_leaf_rule() {
    for field in [Type::Failure, Type::Handle(6)] {
        let mut p = program();
        p.objects.push(handle_object());
        p.objects[5].layout.storage_fields = vec![field];
        p.objects[5].layout.trace_layout = vec![0];
        p.composites.push(Composite::Map { key: Type::Int, value: Type::Object(5) });
        reject(&p, "MAP_PLAIN");
        p.composites.pop();
        p.functions[0].registers.push(Type::Array(ArrayType { leaf: ArrayLeaf::Object(5), depth: 1 }));
        reject(&p, "ARRAY_PLAIN");
    }
    // A closed plain object and a resource object are map values.
    let mut p = program();
    p.composites.push(Composite::Map { key: Type::Int, value: Type::Object(5) });
    p.composites.push(Composite::Map { key: Type::Int, value: Type::Object(0) });
    verify(&p).unwrap();
}

/// An object field that refers to a resource object or successor raises the object's retention.
#[test]
fn object_fields_declare_the_retention_of_their_targets() {
    verify(&program()).unwrap();
    let mut p = program();
    p.objects[1].layout.may_reach_resource = false;
    reject(&p, "OBJECT_RETENTION");
    let mut p = program();
    p.objects[1].layout.storage_fields = vec![Type::Successor(0)];
    p.objects[1].layout.may_reach_resource = false;
    reject(&p, "OBJECT_RETENTION");
}

/// A case object (`some(value: R)`) or an ordinary object whose field refers to a resource object or successor
/// selects that field: the runtime walk reaches the identity only through selected fields (#692).
#[test]
fn object_fields_referring_to_resources_are_selected() {
    let mut p = program();
    p.objects[1].layout.selection_layout.clear();
    reject(&p, "COLLECTION_RETENTION");
    let mut p = program();
    p.objects[1].layout.storage_fields = vec![Type::Successor(0)];
    verify(&p).unwrap();
    p.objects[1].layout.selection_layout.clear();
    reject(&p, "COLLECTION_RETENTION");
    // A plain object field is not selected.
    let mut p = program();
    p.objects[1].layout.storage_fields = vec![Type::Object(5)];
    p.objects[1].layout.selection_layout.clear();
    p.objects[1].layout.may_reach_resource = false;
    p.objects[3].layout.may_reach_resource = false;
    verify(&p).unwrap();
}

fn champ(op: Opcode, arguments: Vec<Argument>, outputs: Vec<Location>) -> Instruction {
    Instruction::Champ { op, arguments, flow: Flow::Next(outputs) }
}

/// `r = Res()` admitted to a scope, then `value` bound in it, then a split of `r` filtered by admission.
/// Registers: r0 = r, r1 = `[r]`, r2 = an int.
fn bound_then_split(value: u32) -> Program {
    use Argument::Location as At;
    use Location::{Register as R, Service as S};
    let mut f = tests::empty_function();
    f.entry_abi = EntryAbi::Owned;
    f.registers = vec![Type::Object(0), RES_ARRAY, Type::Int];
    f.services = vec![ServiceKind::Packet, ServiceKind::ScopeState, ServiceKind::Packet, ServiceKind::ScopeState,
        ServiceKind::Packet, ServiceKind::ScopeState];
    f.params = vec![S(0)];
    f.blocks[0].params = f.params.clone();
    f.proofs.selections = vec![Selection { locations: vec![R(0)] }];
    let stop = Edge { target_pc: 9, assignments: vec![] };
    f.code = vec![
        champ(Opcode::ScopeBegin, vec![Argument::Scope(0), Argument::Boundary(0)], vec![S(1)]),
        Instruction::Data { op: DataOp::ObjNew, inputs: vec![], output: Some(0), immediate: Immediate::Object(0) },
        champ(Opcode::OwnCreateCommit, vec![At(R(0)), Argument::Object(0), Argument::Boundary(0)], vec![S(2)]),
        champ(Opcode::OwnAdmit, vec![At(S(1)), At(S(2))], vec![S(3)]),
        Instruction::Data { op: DataOp::ArrayNew, inputs: vec![0], output: Some(1), immediate: Immediate::None },
        Instruction::Data { op: DataOp::Const, inputs: vec![], output: Some(2),
            immediate: Immediate::Constant(Constant::Int("0".into())) },
        champ(Opcode::OwnEmpty, vec![], vec![S(4)]),
        champ(Opcode::OwnBind,
            vec![At(S(3)), Argument::Binding(0), Argument::Scope(0), At(R(value)), At(S(4))], vec![S(5)]),
        Instruction::Champ { op: Opcode::ScopeSplit,
            arguments: vec![At(S(5)), Argument::Selection(0), Argument::Flag(true)],
            flow: Flow::Outcomes(vec![
                Outcome { tag: Tag::Normal, edge: stop.clone() },
                Outcome { tag: Tag::Unwind, edge: stop },
            ]) },
        Instruction::Unreachable,
    ];
    Program { objects: objects(), functions: vec![f], ..Program::default() }
}

/// The runtime binds every identity a bound value reaches, the elements of a managed array included: once `[r]`
/// is bound, `r` is no longer admissible, so the admission filter of a later split is not redundant (#692).
#[test]
fn binding_a_managed_array_removes_its_elements_from_admission() {
    let redundant = |value| {
        let p = bound_then_split(value);
        evidence::redundant_admission_filters(&p, &p.functions[0]).unwrap()
    };
    // Binding an int binds nothing: `r` stays admissible and the filter selects it either way.
    assert_eq!(redundant(2), vec![8]);
    // Binding `r` itself, or an array of it, binds `r`: the filtered split selects nothing.
    assert_eq!(redundant(0), Vec::<usize>::new());
    assert_eq!(redundant(1), Vec::<usize>::new());
}

/// `map.new` over two fresh resources whose keys are the constants `first` and `second`.
fn keyed_map(first: &str, second: &str) -> Program {
    use Argument::Location as At;
    use Location::{Register as R, Service as S};
    let mut f = tests::empty_function();
    f.registers = vec![Type::Object(0), Type::Object(0), Type::Str, Type::Str, Type::Map(0)];
    f.services = vec![ServiceKind::Packet, ServiceKind::Packet];
    let key = |value: &str, output| Instruction::Data { op: DataOp::Const, inputs: vec![], output: Some(output),
        immediate: Immediate::Constant(Constant::Str(value.into())) };
    let create = |output| Instruction::Data { op: DataOp::ObjNew, inputs: vec![], output: Some(output),
        immediate: Immediate::Object(0) };
    let commit = |identity, packet| champ(Opcode::OwnCreateCommit,
        vec![At(R(identity)), Argument::Object(0), Argument::Boundary(0)], vec![S(packet)]);
    f.code = vec![
        create(0),
        commit(0, 0),
        create(1),
        commit(1, 1),
        key(first, 2),
        key(second, 3),
        Instruction::Data { op: DataOp::MapNew, inputs: vec![2, 0, 3, 1], output: Some(4),
            immediate: Immediate::None },
        Instruction::Ret(vec![]),
    ];
    let mut p = program();
    p.functions = vec![f];
    p
}

/// A repeated key keeps only its last value: the values of `map.new` are its elements only when the keys are
/// pairwise distinct constants (#692).
#[test]
fn a_map_with_a_repeated_key_has_no_static_inventory() {
    let selected = |p: &Program| evidence::Evidence::new(p, &p.functions[0]).selected_identities(Location::Register(4));
    let distinct = keyed_map("k", "j");
    assert_eq!(selected(&distinct), Some([(1, None), (3, None)].into_iter().collect()));
    assert_eq!(selected(&keyed_map("k", "k")), None);
    // A key that is not a constant may repeat another.
    let mut p = keyed_map("k", "j");
    let f = &mut p.functions[0];
    f.code[5] = Instruction::Data { op: DataOp::Move, inputs: vec![2], output: Some(3), immediate: Immediate::None };
    assert_eq!(selected(&p), None);
}

/// `undef r0` reaches block 1 as r1, which obj.new stores; the object's field is returned.
fn construction(read_parameter: bool) -> Program {
    let mut f = tests::empty_function();
    f.registers = vec![Type::Int, Type::Int, Type::Object(5), Type::Int];
    f.results = vec![Type::Int];
    f.blocks = vec![Block { pc: 0, params: vec![] }, Block { pc: 2, params: vec![Location::Register(1)] }];
    f.code = vec![
        Instruction::Data { op: DataOp::Undef, inputs: vec![], output: Some(0), immediate: Immediate::None },
        Instruction::Br(Edge { target_pc: 2, assignments: vec![Assignment {
            destination: Location::Register(1), source: Source::Location(Location::Register(0)),
        }] }),
        Instruction::Data { op: DataOp::ObjNew, inputs: vec![1], output: Some(2), immediate: Immediate::Object(5) },
        Instruction::Data { op: DataOp::ObjGet, inputs: vec![2], output: Some(3), immediate: Immediate::Field(0) },
        Instruction::Ret(vec![if read_parameter { 1 } else { 3 }]),
    ];
    let live = live_locations(&f).unwrap();
    // The placeholder is the constant zero, never a root; the parameter it may bind is rooted.
    f.proofs.roots = live.into_iter().enumerate().map(|(pc, locations)| RootRequirement {
        pc: pc as u32, locations: locations.into_iter().filter(|&location| location != Location::Register(0)).collect(),
    }).collect();
    Program { objects: objects(), functions: vec![f], entry: Some(0), ..Program::default() }
}

#[test]
fn placeholders_may_be_forwarded_to_construction_fields() {
    let mut p = construction(false);
    p.objects.truncate(6);
    // Res has destroy: give it its destroy entry so the executable gate has a complete inventory.
    let mut destroy = tests::empty_function();
    destroy.symbol = "Res.destroy".into();
    destroy.registers = vec![Type::Object(0)];
    destroy.params = vec![Location::Register(0)];
    destroy.blocks[0].params = destroy.params.clone();
    destroy.proofs.entry = Some(EntryContract { destroy: Some(0), ..EntryContract::default() });
    destroy.proofs.roots = vec![RootRequirement { pc: 0, locations: vec![] }];
    p.functions.push(destroy);
    verify_load(&p, &NoExternalContracts).unwrap();
}

#[test]
fn a_possibly_unbound_parameter_is_read_only_by_construction() {
    let p = construction(true);
    assert_eq!(verify(&p).unwrap_err().rule, "UNBOUND_READ");
}
