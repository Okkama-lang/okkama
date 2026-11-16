//! Arrays, builders and maps of native handles (#751). A native handle is plain data without an Okkama destroy
//! (P2.4 §4.7, #717), so collections of handles are plain; like a handle they have no structural value.
use super::*;

/// 0 = a native handle declaration (json's parsed node), 1 = Point { x: int }.
fn objects() -> Vec<Object> {
    let object = |name: &str, handle: bool, fields: Vec<Type>| Object {
        name: name.into(), error_cases: vec![], enum_cases: 0, variant_cases: vec![], handle, role: None,
        implements: vec![], any_box: false, enum_names: vec![],
        fields: (0..fields.len() as u32).map(|storage| Field { name: format!("f{storage}"), storage, once: false })
            .collect(),
        layout: ObjectRuntimeLayout {
            allocation_words: (1 + fields.len() as u32).max(2), identity_offset: None, has_destroy: false,
            may_reach_resource: false, may_async_destroy: false, trace_layout: vec![], selection_layout: vec![],
            storage_fields: fields,
        },
    };
    vec![object("runtime.json.JsonNode", true, vec![]), object("Point", false, vec![Type::Int])]
}

const NODES: Type = Type::Array(ArrayType { leaf: ArrayLeaf::Handle(0), depth: 1 });
const NODE_ROWS: Type = Type::Array(ArrayType { leaf: ArrayLeaf::Handle(0), depth: 2 });

/// A function holding an array of handles, a nested one, a builder of handles and a map from strings to handles.
fn program() -> Program {
    let mut f = tests::empty_function();
    f.registers = vec![NODES, NODE_ROWS, Type::Map(1), Type::Builder(0)];
    f.params = vec![Location::Register(0), Location::Register(1), Location::Register(2)];
    f.blocks[0].params = f.params.clone();
    Program {
        objects: objects(),
        functions: vec![f],
        composites: vec![
            Composite::Builder { array: NODES },
            Composite::Map { key: Type::Str, value: Type::Handle(0) },
        ],
        ..Program::default()
    }
}

fn reject(p: &Program, rule: &str) {
    let failure = verify(p).unwrap_err();
    assert_eq!(failure.rule, rule, "{failure}");
}

#[test]
fn a_handle_is_an_array_leaf() {
    let handles = Type::Array(ArrayType { leaf: ArrayLeaf::Handle(3), depth: 1 });
    assert_eq!(ArrayType::of(Type::Handle(3)), Some(handles));
    assert_eq!(ArrayType::of(NODES), Some(NODE_ROWS));
    assert_eq!(element_type(NODES), Some(Type::Handle(0)));
    assert_eq!(element_type(NODE_ROWS), Some(NODES));
    // A handle element is the reference to its registry cell: the collector traces it.
    assert!(Type::Handle(0).traced());
}

#[test]
fn arrays_builders_and_maps_of_handles_load_as_plain_data() {
    let p = program();
    verify(&p).unwrap();
    for ty in [NODES, NODE_ROWS, Type::Map(1)] {
        assert!(verify::closed_plain(&p, ty), "{ty:?}");
        assert!(!verify::reaches_resource(&p, ty), "{ty:?}");
    }
}

#[test]
fn handle_collections_round_trip_through_the_codec_and_the_text() {
    let p = program();
    assert_eq!(decode(&encode(&p).unwrap()).unwrap(), p);
    assert_eq!(assemble(&disassemble(&p).unwrap()).unwrap(), p);
}

/// The leaf names a native handle declaration, as a handle type does (`HANDLE_TYPE`): a plain object or an
/// unknown declaration is not a handle.
#[test]
fn an_array_handle_leaf_names_a_handle_declaration() {
    for leaf in [ArrayLeaf::Handle(1), ArrayLeaf::Handle(7)] {
        for depth in [1, 2] {
            let mut p = program();
            p.functions[0].registers.push(Type::Array(ArrayType { leaf, depth }));
            reject(&p, "HANDLE_TYPE");
        }
        let mut p = program();
        p.composites[0] = Composite::Builder { array: Type::Array(ArrayType { leaf, depth: 1 }) };
        reject(&p, "HANDLE_TYPE");
    }
    let mut p = program();
    p.composites[1] = Composite::Map { key: Type::Str, value: Type::Handle(1) };
    reject(&p, "HANDLE_TYPE");
}

/// A handle has no structural value, so it is no map key, and its collections are not compared.
#[test]
fn handle_collections_have_no_structural_value() {
    let p = program();
    for ty in [Type::Handle(0), NODES, NODE_ROWS, Type::Map(1)] {
        assert!(!verify::comparable(&p, ty), "{ty:?}");
    }
    let mut p = program();
    p.composites[1] = Composite::Map { key: Type::Handle(0), value: Type::Int };
    reject(&p, "MAP_KEY");
    for (ty, op, rule) in [
        (NODES, DataOp::ValEq, "VAL_EQ_TYPE"),
        (NODE_ROWS, DataOp::ValNe, "VAL_EQ_TYPE"),
        (Type::Map(1), DataOp::ValEq, "VAL_EQ_TYPE"),
        (NODES, DataOp::ArrayDifference, "ARRAY_OP_TYPE"),
    ] {
        let mut p = program();
        let result = if op == DataOp::ArrayDifference { ty } else { Type::Bool };
        let f = &mut p.functions[0];
        f.registers = vec![ty, ty, result];
        f.params = vec![Location::Register(0), Location::Register(1)];
        f.blocks[0].params = f.params.clone();
        f.code = vec![
            Instruction::Data { op, inputs: vec![0, 1], output: Some(2), immediate: Immediate::None },
            Instruction::Ret(vec![]),
        ];
        reject(&p, rule);
    }
}
