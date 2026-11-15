//! Contracts of the SPB 17 map, collection and builder helpers on Tomato's payloads.
use super::*;
use okkama_runtime_core::IntValue;
use okkama_runtime_core::rat::Rat;

const INTS: s::Type = s::Type::Array(s::ArrayType { leaf: s::ArrayLeaf::Int, depth: 1 });

fn object(name: &str, fields: Vec<s::Type>, enum_cases: u32) -> s::Object {
    let source = if enum_cases > 0 { vec![] } else {
        (0..fields.len() as u32).map(|storage| s::Field { name: format!("f{storage}"), storage, once: false }).collect()
    };
    s::Object {
        name: name.into(), error_cases: vec![], enum_cases, variant_cases: vec![], handle: false, fields: source,
        role: None, implements: vec![],
        layout: s::ObjectRuntimeLayout {
            allocation_words: (1 + fields.len() as u32).max(2),
            trace_layout: fields.iter().enumerate().filter_map(|(index, ty)| ty.traced().then_some(index as u32))
                .collect(),
            storage_fields: fields, identity_offset: None, has_destroy: false, may_reach_resource: false,
            may_async_destroy: false, selection_layout: vec![],
        },
    }
}

/// Composites: 0 = map<str, int>, 1 = builder of @int, 2 = builder of @byte, 3 = map<Point, int>.
fn runtime() -> (Box<Runtime>, s::Program) {
    let program = s::Program {
        objects: vec![object("Color", vec![s::Type::Int], 2), object("Point", vec![s::Type::Int, s::Type::Str], 0)],
        composites: vec![
            s::Composite::Map { key: s::Type::Str, value: s::Type::Int },
            s::Composite::Builder { array: INTS },
            s::Composite::Builder { array: s::Type::Bytes },
            s::Composite::Map { key: s::Type::Object(1), value: s::Type::Int },
        ],
        ..s::Program::default()
    };
    (Runtime::new_champ(program.clone(), vec![]).unwrap(), program)
}

fn int(runtime: &mut Runtime, value: i64) -> Word {
    runtime.heap.int(value)
}

fn ints(runtime: &Runtime, array: Word) -> Vec<i64> {
    runtime.heap.elements(array).iter().map(|&word| runtime.small_or_big(word).unwrap()).collect()
}

fn rat(runtime: &mut Runtime, numerator: i64, denominator: i64) -> Word {
    let value = Rat::new(&IntValue::from_i64(numerator), &IntValue::from_i64(denominator)).unwrap();
    runtime.rational_new(value)
}

#[test]
fn keys_are_structural_values() {
    let (mut runtime, program) = runtime();
    let half = rat(&mut runtime, 1, 2);
    let two_quarters = rat(&mut runtime, 2, 4);
    assert_eq!(key(&runtime, &program, half, s::Type::Rat), key(&runtime, &program, two_quarters, s::Type::Rat));
    let a = runtime.heap.allocate_text("a");
    let b = runtime.heap.allocate_text("b");
    let other_a = runtime.heap.allocate_text("a");
    assert!(key(&runtime, &program, a, s::Type::Str) < key(&runtime, &program, b, s::Type::Str));
    assert_eq!(key(&runtime, &program, a, s::Type::Str), key(&runtime, &program, other_a, s::Type::Str));
    let one = int(&mut runtime, 1);
    let red = runtime.heap.object_new(0, &[one]);
    let also_red = runtime.heap.object_new(0, &[one]);
    let zero = int(&mut runtime, 0);
    let blue = runtime.heap.object_new(0, &[zero]);
    let color = s::Type::Object(0);
    assert_eq!(key(&runtime, &program, red, color), key(&runtime, &program, also_red, color));
    assert!(key(&runtime, &program, blue, color) < key(&runtime, &program, red, color));
    let p = runtime.heap.object_new(1, &[one, a]);
    let q = runtime.heap.object_new(1, &[one, other_a]);
    let r = runtime.heap.object_new(1, &[one, b]);
    let point = s::Type::Object(1);
    assert_eq!(key(&runtime, &program, p, point), key(&runtime, &program, q, point));
    assert_ne!(key(&runtime, &program, p, point), key(&runtime, &program, r, point));
}

#[test]
fn map_operations_keep_the_first_key_and_the_last_value() {
    use s::DataOp as D;
    let (mut runtime, program) = runtime();
    let (a, b, other_a) = (runtime.heap.allocate_text("a"), runtime.heap.allocate_text("b"),
        runtime.heap.allocate_text("a"));
    let (one, two, three) = (int(&mut runtime, 1), int(&mut runtime, 2), int(&mut runtime, 3));
    let map = s::Type::Map(0);
    let pairs = [b, one, a, two, other_a, three];
    let m = collection(&mut runtime, &program, D::MapNew, &[], map, &pairs).unwrap();
    let len = collection(&mut runtime, &program, D::MapLen, &[map], s::Type::Int, &[m]).unwrap();
    assert_eq!(runtime.small_or_big(len), Some(2));
    let keys = collection(&mut runtime, &program, D::MapKeys, &[map], s::Type::Array(s::ArrayType {
        leaf: s::ArrayLeaf::Str, depth: 1 }), &[m]).unwrap();
    // Key order, with the first word of an equal key.
    assert_eq!(runtime.heap.elements(keys).to_vec(), vec![a, b]);
    let got = collection(&mut runtime, &program, D::MapGet, &[map, s::Type::Str], s::Type::Int, &[m, other_a]);
    assert_eq!(got.unwrap(), three);
    let z = runtime.heap.allocate_text("z");
    let missing = collection(&mut runtime, &program, D::MapGet, &[map, s::Type::Str], s::Type::Int, &[m, z]);
    assert_eq!(missing.unwrap_err(), "Map key not found: \"z\"");
    let has = collection(&mut runtime, &program, D::MapHas, &[map, s::Type::Str], s::Type::Bool, &[m, z]).unwrap();
    assert_eq!(has, 0);
    let far = int(&mut runtime, 2);
    assert!(collection(&mut runtime, &program, D::MapKeyAt, &[map, s::Type::Int], s::Type::Str, &[m, far]).is_err());
    let right = collection(&mut runtime, &program, D::MapNew, &[], map, &[other_a, one]).unwrap();
    let merged = collection(&mut runtime, &program, D::MapMerge, &[map, map], map, &[m, right]).unwrap();
    assert_eq!(lookup(&runtime, &program, merged, a, s::Type::Str), Some(one));
    let difference = collection(&mut runtime, &program, D::MapDifference, &[map, map], map, &[m, right]).unwrap();
    let keys = collection(&mut runtime, &program, D::MapKeys, &[map], s::Type::Array(s::ArrayType {
        leaf: s::ArrayLeaf::Str, depth: 1 }), &[difference]).unwrap();
    assert_eq!(runtime.heap.elements(keys).to_vec(), vec![b]);
}

#[test]
fn array_operations_copy_and_check_bounds() {
    use s::DataOp as D;
    let (mut runtime, program) = runtime();
    let words: Vec<Word> = (1..=4).map(|value| int(&mut runtime, value)).collect();
    let array = collection(&mut runtime, &program, D::ArrayNew, &[s::Type::Int; 4], INTS, &words).unwrap();
    let (one, three, nine) = (int(&mut runtime, 1), int(&mut runtime, 3), int(&mut runtime, 9));
    let slice = collection(&mut runtime, &program, D::ArraySlice, &[INTS, s::Type::Int, s::Type::Int], INTS,
        &[array, one, three]).unwrap();
    assert_eq!(ints(&runtime, slice), vec![2, 3]);
    let past = collection(&mut runtime, &program, D::ArraySlice, &[INTS, s::Type::Int, s::Type::Int], INTS,
        &[array, one, nine]);
    assert_eq!(past.unwrap_err(), "Slice end must not exceed length");
    let removed = collection(&mut runtime, &program, D::ArrayRemove, &[INTS, s::Type::Int], INTS, &[array, one])
        .unwrap();
    assert_eq!(ints(&runtime, removed), vec![1, 3, 4]);
    let replaced = collection(&mut runtime, &program, D::ArrayReplace, &[INTS, s::Type::Int, s::Type::Int], INTS,
        &[array, three, nine]).unwrap();
    assert_eq!(ints(&runtime, replaced), vec![1, 2, 3, 9]);
    assert_eq!(ints(&runtime, array), vec![1, 2, 3, 4]);
    let outside = collection(&mut runtime, &program, D::ArrayRemove, &[INTS, s::Type::Int], INTS, &[array, nine]);
    assert_eq!(outside.unwrap_err(), "Array index out of bounds");
    let concat = collection(&mut runtime, &program, D::ArrayConcat, &[INTS, INTS], INTS, &[slice, removed]).unwrap();
    assert_eq!(ints(&runtime, concat), vec![2, 3, 1, 3, 4]);
    let difference = collection(&mut runtime, &program, D::ArrayDifference, &[INTS, INTS], INTS, &[concat, slice])
        .unwrap();
    assert_eq!(ints(&runtime, difference), vec![1, 4]);
    let bytes = collection(&mut runtime, &program, D::ArrayNew, &[s::Type::Byte], s::Type::Bytes, &[7]).unwrap();
    let both = collection(&mut runtime, &program, D::ArrayConcat, &[s::Type::Bytes, s::Type::Bytes], s::Type::Bytes,
        &[bytes, bytes]).unwrap();
    assert_eq!(runtime.heap.bytes(both), &[7, 7]);
}

#[test]
fn builders_collect_ranges_and_finish_into_their_array() {
    use s::Opcode as O;
    let (mut runtime, program) = runtime();
    let builder_type = s::Type::Builder(1);
    let buffer = collection(&mut runtime, &program, s::DataOp::BuilderNew, &[], builder_type, &[]).unwrap();
    let (zero, two, four) = (int(&mut runtime, 0), int(&mut runtime, 2), int(&mut runtime, 4));
    assert_eq!(builder(&mut runtime, &program, O::BuilderPush, false, builder_type, &[buffer, zero]), Ok(buffer));
    assert_eq!(builder(&mut runtime, &program, O::BuilderRange, true, builder_type, &[buffer, two, four]),
        Ok(buffer));
    assert_eq!(builder(&mut runtime, &program, O::BuilderRange, false, builder_type, &[buffer, four, two])
        .unwrap_err(), "Range start must be less than or equal to range end");
    let finished = builder(&mut runtime, &program, O::BuilderFinish, false, builder_type, &[buffer]).unwrap();
    assert_eq!(ints(&runtime, finished), vec![0, 2, 3, 4]);
    let bytes_type = s::Type::Builder(2);
    let bytes = collection(&mut runtime, &program, s::DataOp::BuilderNew, &[], bytes_type, &[]).unwrap();
    builder(&mut runtime, &program, O::BuilderPush, false, bytes_type, &[bytes, 255]).unwrap();
    let packed = builder(&mut runtime, &program, O::BuilderFinish, false, bytes_type, &[bytes]).unwrap();
    assert_eq!(runtime.heap.bytes(packed), &[255]);
    let abandoned = collection(&mut runtime, &program, s::DataOp::BuilderNew, &[], builder_type, &[]).unwrap();
    assert_eq!(builder(&mut runtime, &program, O::BuilderAbandon, false, builder_type, &[abandoned]), Ok(0));
}

#[test]
fn map_builders_put_append_and_finish() {
    use s::Opcode as O;
    let (mut runtime, program) = runtime();
    let builder_type = s::Type::MapBuilder(0);
    let buffer = collection(&mut runtime, &program, s::DataOp::BuilderNew, &[], builder_type, &[]).unwrap();
    let (a, b) = (runtime.heap.allocate_text("a"), runtime.heap.allocate_text("b"));
    let (one, two) = (int(&mut runtime, 1), int(&mut runtime, 2));
    builder(&mut runtime, &program, O::BuilderPush, false, builder_type, &[buffer, a, one]).unwrap();
    let source = collection(&mut runtime, &program, s::DataOp::MapNew, &[], s::Type::Map(0), &[b, two, a, two])
        .unwrap();
    builder(&mut runtime, &program, O::BuilderAppend, false, builder_type, &[buffer, source]).unwrap();
    let map = builder(&mut runtime, &program, O::BuilderFinish, false, builder_type, &[buffer]).unwrap();
    assert_eq!(map_entries(&runtime, map).len(), 2);
    assert_eq!(lookup(&runtime, &program, map, a, s::Type::Str), Some(two));
    // An object key finds an equal object.
    let point = s::Type::Object(1);
    let p = runtime.heap.object_new(1, &[one, a]);
    let q = runtime.heap.object_new(1, &[one, a]);
    let keyed = collection(&mut runtime, &program, s::DataOp::MapNew, &[], s::Type::Map(3), &[p, two]).unwrap();
    assert_eq!(lookup(&runtime, &program, keyed, q, point), Some(two));
}
