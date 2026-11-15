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
            role: None, implements: vec![],
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
