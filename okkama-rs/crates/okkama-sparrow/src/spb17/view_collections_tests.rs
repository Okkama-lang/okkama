//! Managed collections of role values and `any` values (#711): their leaves are references to implementing
//! objects or any boxes, so an array or map of them refers to resources when one of those objects or boxes
//! may reach one, and is closed plain data otherwise. Fields that hold such values are selected.
use super::*;

const SOURCES: Type = Type::Array(ArrayType { leaf: ArrayLeaf::Role(1), depth: 1 });
const SHAPES: Type = Type::Array(ArrayType { leaf: ArrayLeaf::Role(4), depth: 1 });
const VALUES: Type = Type::Array(ArrayType { leaf: ArrayLeaf::Any, depth: 1 });

fn object(name: &str, fields: Vec<Type>, selected: Vec<u32>) -> Object {
    let destroy = name == "Res";
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
            storage_fields: fields, has_destroy: destroy, may_reach_resource: destroy || !selected.is_empty(),
            may_async_destroy: false, trace_layout, selection_layout: selected,
        },
    }
}

fn role(name: &str) -> Object {
    Object { role: Some(RoleDecl { slots: vec![] }), ..object(name, vec![], vec![]) }
}

fn implementing(object: Object, role: u32) -> Object {
    Object { implements: vec![RoleImpl { role, slots: vec![] }], ..object }
}

/// 0 `Res` (destroy); 1 role `Source`, implemented by 2 `Owning { res }` (reaches) and 3 `Plain { value }`;
/// 4 role `Shape`, implemented only by 5 `Square { side }`; 6 `Holder { sources: @Source, by_name:
/// %str:Source, one: Source, shapes: @Shape }`; 7 the any box of `Res` and 8 the any box of `int`;
/// 9 `AnyHolder { values: @any, one: any }`.
fn program() -> Program {
    let mut res_box = object("any#res", vec![Type::Object(0)], vec![0]);
    res_box.any_box = true;
    let mut int_box = object("any#int", vec![Type::Int], vec![]);
    int_box.any_box = true;
    let objects = vec![
        object("Res", vec![], vec![]),
        role("Source"),
        implementing(object("Owning", vec![Type::Object(0)], vec![0]), 1),
        implementing(object("Plain", vec![Type::Int], vec![]), 1),
        role("Shape"),
        implementing(object("Square", vec![Type::Int], vec![]), 4),
        object("Holder", vec![SOURCES, Type::Map(0), Type::Role(1), SHAPES], vec![0, 1, 2]),
        res_box,
        int_box,
        object("AnyHolder", vec![VALUES, Type::Any], vec![0, 1]),
    ];
    let mut f = tests::empty_function();
    f.registers = vec![SOURCES, SHAPES, Type::Map(0), VALUES];
    f.params = (0..4).map(Location::Register).collect();
    f.blocks[0].params = f.params.clone();
    Program {
        objects,
        functions: vec![f],
        composites: vec![Composite::Map { key: Type::Str, value: Type::Role(1) }],
        ..Program::default()
    }
}

fn reject(p: &Program, rule: &str) {
    let failure = verify(p).unwrap_err();
    assert_eq!(failure.rule, rule, "{failure}");
}

#[test]
fn managed_role_and_any_collections_are_admitted() {
    verify(&program()).unwrap();
}

#[test]
fn role_and_any_leaves_reach_what_their_objects_reach() {
    let p = program();
    assert!(verify::reaches_resource(&p, Type::Role(1)));
    assert!(verify::reaches_resource(&p, SOURCES));
    assert!(verify::reaches_resource(&p, Type::Map(0)));
    assert!(verify::reaches_resource(&p, Type::Any));
    assert!(verify::reaches_resource(&p, VALUES));
    // A role whose implementations are all plain keeps its collections plain.
    assert!(!verify::reaches_resource(&p, Type::Role(4)));
    assert!(!verify::reaches_resource(&p, SHAPES));
    assert!(verify::closed_plain(&p, SHAPES));
    assert!(!verify::closed_plain(&p, SOURCES));
}

#[test]
fn without_a_reaching_box_any_collections_are_plain() {
    let mut p = program();
    // Drop the box of `Res`: every remaining box holds plain data.
    p.objects[7] = object("Unused", vec![], vec![]);
    p.objects[9].layout.selection_layout.clear();
    p.objects[9].layout.may_reach_resource = false;
    verify(&p).unwrap();
    assert!(!verify::reaches_resource(&p, VALUES));
    assert!(verify::closed_plain(&p, VALUES));
}

#[test]
fn fields_holding_reaching_role_or_any_values_are_selected() {
    // A managed collection of role values.
    let mut p = program();
    p.objects[6].layout.selection_layout = vec![1, 2];
    reject(&p, "COLLECTION_RETENTION");
    // A role field whose implementations reach a resource: the selection walk must descend into it (#697).
    let mut p = program();
    p.objects[6].layout.selection_layout = vec![0, 1];
    reject(&p, "COLLECTION_RETENTION");
    // An `any` field and a managed array of `any`.
    let mut p = program();
    p.objects[9].layout.selection_layout = vec![0];
    reject(&p, "COLLECTION_RETENTION");
    let mut p = program();
    p.objects[9].layout.selection_layout = vec![1];
    reject(&p, "COLLECTION_RETENTION");
}

#[test]
fn a_role_leaf_names_a_role_declaration() {
    let mut p = program();
    p.functions[0].registers[0] = Type::Array(ArrayType { leaf: ArrayLeaf::Role(0), depth: 1 });
    reject(&p, "ROLE_TYPE");
}

/// Selection evidence follows the managed collections of resource objects: an array of reaching role or `any`
/// values whose elements no `array.new` proves has no static inventory; one of plain role values has none to
/// prove.
#[test]
fn selected_identities_never_lose_unproved_role_or_any_elements() {
    let p = program();
    let mut evidence = evidence::Evidence::new(&p, &p.functions[0]);
    assert_eq!(evidence.selected_identities(Location::Register(0)), None);
    assert_eq!(evidence.selected_identities(Location::Register(2)), None);
    assert_eq!(evidence.selected_identities(Location::Register(3)), None);
    assert_eq!(evidence.selected_identities(Location::Register(1)), Some(Default::default()));
}
