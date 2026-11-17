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
        role: None, implements: vec![], any_box: false, enum_names: vec![],
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

/// Arrays, builders and maps of native handles (#751). A handle word is the reference to its registry cell,
/// which the native bridge watches and releases with its handle when the collector frees it (`host::released`):
/// a collection of handles keeps every cell alive, and a collection collected releases them.
#[test]
fn handle_collections_keep_their_registry_cells() {
    use s::Opcode as O;
    let nodes = s::Type::Array(s::ArrayType { leaf: s::ArrayLeaf::Handle(0), depth: 1 });
    let mut node = object("runtime.json.JsonNode", vec![], 0);
    node.handle = true;
    let program = s::Program {
        objects: vec![node],
        composites: vec![
            s::Composite::Builder { array: nodes },
            s::Composite::Map { key: s::Type::Str, value: s::Type::Handle(0) },
        ],
        ..s::Program::default()
    };
    let mut runtime = Runtime::new_champ(program.clone(), vec![]).unwrap();
    // A registry cell as the native bridge makes one: `[header, id]`, watched.
    let cell = |runtime: &mut Runtime, id: u64| {
        let cell = runtime.heap.allocate_array(Repr::Raw, &[id]);
        runtime.heap.watch(cell);
        cell
    };
    let ids = |runtime: &Runtime, cells: &[Word]| -> Vec<u64> {
        cells.iter().map(|&cell| runtime.heap.elements(cell)[0]).collect()
    };
    let (first, second) = (cell(&mut runtime, 7), cell(&mut runtime, 9));
    let handle = s::Type::Handle(0);
    let array = collection(&mut runtime, &program, s::DataOp::ArrayNew, &[handle, handle], nodes, &[first, second])
        .unwrap();
    assert_eq!(runtime.heap.elements(array).to_vec(), vec![first, second]);
    runtime.heap.collect(|visit| visit(array, Repr::Ref));
    assert!(runtime.heap.take_released().is_empty(), "an array traces its handle elements");
    assert_eq!(ids(&runtime, &[first, second]), vec![7, 9]);
    // A builder holds what it was given until it finishes into an array of the same handles.
    let builder_type = s::Type::Builder(0);
    let buffer = collection(&mut runtime, &program, s::DataOp::BuilderNew, &[], builder_type, &[]).unwrap();
    let third = cell(&mut runtime, 11);
    builder(&mut runtime, &program, O::BuilderPush, false, builder_type, &[buffer, third]).unwrap();
    builder(&mut runtime, &program, O::BuilderAppend, false, builder_type, &[buffer, array]).unwrap();
    runtime.heap.collect(|visit| visit(buffer, Repr::Ref));
    assert!(runtime.heap.take_released().is_empty(), "a builder traces its handle items");
    let finished = builder(&mut runtime, &program, O::BuilderFinish, false, builder_type, &[buffer]).unwrap();
    assert_eq!(runtime.heap.elements(finished).to_vec(), vec![third, first, second]);
    // A map value holds its handle.
    let name = runtime.heap.allocate_text("root");
    let map = collection(&mut runtime, &program, s::DataOp::MapNew, &[], s::Type::Map(1), &[name, third]).unwrap();
    runtime.heap.collect(|visit| visit(map, Repr::Ref));
    let mut released = runtime.heap.take_released();
    released.sort_unstable();
    let mut held = vec![first, second];
    held.sort_unstable();
    assert_eq!(released, held, "a map traces its handle values, and nothing else holds the others");
    assert_eq!(lookup(&runtime, &program, map, name, s::Type::Str), Some(third));
    assert_eq!(ids(&runtime, &[third]), vec![11]);
    // An unreachable map no longer keeps its cell.
    runtime.heap.collect(|_| {});
    assert_eq!(runtime.heap.take_released(), vec![third]);
}

fn ints_array(runtime: &mut Runtime, program: &s::Program, values: std::ops::Range<i64>) -> Word {
    let words: Vec<Word> = values.map(|value| int(runtime, value)).collect();
    collection(runtime, program, s::DataOp::ArrayNew, &vec![s::Type::Int; words.len()], INTS, &words).unwrap()
}

fn concat(runtime: &mut Runtime, program: &s::Program, left: Word, right: Word) -> Word {
    collection(runtime, program, s::DataOp::ArrayConcat, &[INTS, INTS], INTS, &[left, right]).unwrap()
}

/// #826: a chain of appends continues in a shared buffer, as on Tomato (#246): from the second append to a long
/// array every version is a view, and an append at the buffer's frontier writes in place, so 168 appends take two
/// buffers (capacities 128 and 512). Every version keeps its elements; an append to an older version, which does
/// not end at the frontier, copies into a buffer of its own.
#[test]
fn appends_continue_in_a_shared_buffer_and_keep_every_version() {
    use crate::heap::{CHAIN_MIN, SHARED};
    let (mut runtime, program) = runtime();
    let mut versions = vec![ints_array(&mut runtime, &program, 0..CHAIN_MIN as i64)];
    for value in CHAIN_MIN as i64..200 {
        let item = ints_array(&mut runtime, &program, value..value + 1);
        let next = concat(&mut runtime, &program, *versions.last().unwrap(), item);
        versions.push(next);
    }
    let mut buffers = std::collections::HashSet::new();
    for &version in &versions[2..] {
        assert_ne!(runtime.heap.header_of(version) & SHARED, 0, "a version of the chain is a view");
        buffers.insert(runtime.heap.shared(version).0);
    }
    assert_eq!(buffers.len(), 2);
    for (index, &version) in versions.iter().enumerate() {
        assert_eq!(ints(&runtime, version), (0..CHAIN_MIN as i64 + index as i64).collect::<Vec<_>>());
    }
    let item = ints_array(&mut runtime, &program, 999..1000);
    let branch = concat(&mut runtime, &program, versions[50], item);
    let mut expected: Vec<i64> = (0..CHAIN_MIN as i64 + 50).collect();
    expected.push(999);
    assert_eq!(ints(&runtime, branch), expected);
    assert!(!buffers.contains(&runtime.heap.shared(branch).0), "a branch writes into a buffer of its own");
    assert_eq!(ints(&runtime, versions[51]), (0..CHAIN_MIN as i64 + 51).collect::<Vec<_>>());
}

/// #826: a chain of prepends (`[x] + items`, #247) continues in a shared buffer with room in front.
#[test]
fn prepends_continue_in_a_shared_buffer() {
    use crate::heap::{CHAIN_MIN, PREPENDED, SHARED};
    let (mut runtime, program) = runtime();
    let mut version = ints_array(&mut runtime, &program, 0..CHAIN_MIN as i64);
    let mut buffers = std::collections::HashSet::new();
    for value in 1..=100 {
        let item = ints_array(&mut runtime, &program, -value..-value + 1);
        version = concat(&mut runtime, &program, item, version);
        let header = runtime.heap.header_of(version);
        if value == 1 {
            assert_ne!(header & PREPENDED, 0, "the first prepend to a long array starts a chain");
        } else {
            assert_ne!(header & SHARED, 0, "a later version of the chain is a view");
            buffers.insert(runtime.heap.shared(version).0);
        }
    }
    assert!(buffers.len() <= 2, "{} buffers for 99 prepends", buffers.len());
    assert_eq!(ints(&runtime, version), (-100..CHAIN_MIN as i64).collect::<Vec<_>>());
}

/// #826: building an array by appends allocates linearly, not quadratically: 10⁴ one-element appends to a growing
/// array allocate well under the 400 MB a copy per append would take.
#[test]
fn appending_in_a_loop_allocates_linearly() {
    let (mut runtime, program) = runtime();
    let mut array = ints_array(&mut runtime, &program, 0..0);
    let before = runtime.heap.stats().allocated_bytes;
    for value in 0..10_000 {
        let item = ints_array(&mut runtime, &program, value..value + 1);
        array = concat(&mut runtime, &program, array, item);
    }
    let spent = runtime.heap.stats().allocated_bytes - before;
    assert!(spent < 2_000_000, "{spent} bytes for 10⁴ appends");
    assert_eq!(ints(&runtime, array), (0..10_000).collect::<Vec<_>>());
}

/// #826: a chain of replacements in a large array becomes a trie (#250): the first replacement copies and marks
/// the copy, the next ones build and update a trie. Every version keeps its elements.
#[test]
fn replacements_in_a_large_array_become_a_trie() {
    use crate::heap::{REPLACED, TRIE, TRIE_MIN};
    let (mut runtime, program) = runtime();
    let original = ints_array(&mut runtime, &program, 0..TRIE_MIN as i64);
    let replace = |runtime: &mut Runtime, array: Word, at: i64, value: i64| {
        let (at, value) = (int(runtime, at), int(runtime, value));
        collection(runtime, &program, s::DataOp::ArrayReplace, &[INTS, s::Type::Int, s::Type::Int], INTS,
            &[array, at, value]).unwrap()
    };
    let first = replace(&mut runtime, original, 0, 100);
    assert_ne!(runtime.heap.header_of(first) & REPLACED, 0);
    assert_eq!(runtime.heap.header_of(first) & TRIE, 0);
    let second = replace(&mut runtime, first, 1, 101);
    let third = replace(&mut runtime, second, 63, 163);
    assert_ne!(runtime.heap.header_of(second) & TRIE, 0);
    assert_ne!(runtime.heap.header_of(third) & TRIE, 0);
    let with = |changes: &[(usize, i64)]| {
        let mut values: Vec<i64> = (0..TRIE_MIN as i64).collect();
        for &(at, value) in changes {
            values[at] = value;
        }
        values
    };
    assert_eq!(ints(&runtime, original), with(&[]));
    assert_eq!(ints(&runtime, first), with(&[(0, 100)]));
    assert_eq!(ints(&runtime, second), with(&[(0, 100), (1, 101)]));
    assert_eq!(ints(&runtime, third), with(&[(0, 100), (1, 101), (63, 163)]));
}

/// #826: a slice that keeps at least half of its origin shares the origin's storage (#249); a short one copies.
#[test]
fn a_large_slice_is_a_view_and_a_short_one_a_copy() {
    use crate::heap::SHARED;
    let (mut runtime, program) = runtime();
    let array = ints_array(&mut runtime, &program, 0..64);
    let slice = |runtime: &mut Runtime, start: i64, end: i64| {
        let (start, end) = (int(runtime, start), int(runtime, end));
        collection(runtime, &program, s::DataOp::ArraySlice, &[INTS, s::Type::Int, s::Type::Int], INTS,
            &[array, start, end]).unwrap()
    };
    let large = slice(&mut runtime, 10, 50);
    assert_ne!(runtime.heap.header_of(large) & SHARED, 0);
    assert_eq!(runtime.heap.shared(large), (array, 10));
    assert_eq!(ints(&runtime, large), (10..50).collect::<Vec<_>>());
    let short = slice(&mut runtime, 1, 4);
    assert_eq!(runtime.heap.header_of(short) & SHARED, 0);
    assert_eq!(ints(&runtime, short), vec![1, 2, 3]);
}

/// #832: the site of each instruction, computed once at load, is what its helper used to decode on every call:
/// a data operation's and an `evaluate`'s operation, first operand type and result; a lookup's receiver type;
/// a builder transition's opcode, inclusive flag and buffer type. Other instructions have none.
#[test]
fn sites_hold_what_the_helpers_decoded() {
    use s::{Argument as A, Location as L};
    let map = s::Type::Map(0);
    let builder = s::Type::Builder(1);
    let function = s::Function {
        symbol: "sites".into(), entry_abi: s::EntryAbi::Observation,
        registers: vec![INTS, INTS, INTS, s::Type::Int, s::Type::Int, map, builder, s::Type::Int],
        services: vec![], params: vec![], results: vec![], blocks: vec![],
        proofs: s::FunctionProofs::default(),
        code: vec![
            s::Instruction::Data { op: s::DataOp::ArrayConcat, inputs: vec![0, 1], output: Some(2),
                immediate: s::Immediate::None },
            s::Instruction::Champ { op: s::Opcode::Evaluate, arguments: vec![
                A::Evaluation(s::Evaluation { op: s::DataOp::IntMod, result: s::Type::Int,
                    immediate: s::Immediate::None }),
                A::Location(L::Register(3)), A::Location(L::Register(4)),
            ], flow: s::Flow::Next(vec![]) },
            s::Instruction::Champ { op: s::Opcode::Lookup, arguments: vec![
                A::Location(L::Register(5)), A::Location(L::Register(3)),
            ], flow: s::Flow::Next(vec![]) },
            s::Instruction::Champ { op: s::Opcode::BuilderRange, arguments: vec![
                A::Flag(true), A::Location(L::Register(6)), A::Location(L::Register(3)), A::Location(L::Register(4)),
            ], flow: s::Flow::Next(vec![]) },
            s::Instruction::Data { op: s::DataOp::ArrayLen, inputs: vec![0], output: Some(7),
                immediate: s::Immediate::None },
            s::Instruction::Ret(vec![]),
        ],
    };
    let (_, mut program) = runtime();
    program.functions = vec![function];
    let sites = sites(&program);
    assert_eq!(sites[0].to_vec(), vec![
        Some(Site::Collection { op: s::DataOp::ArrayConcat, operand: INTS, result: INTS }),
        Some(Site::Collection { op: s::DataOp::IntMod, operand: s::Type::Int, result: s::Type::Int }),
        Some(Site::Lookup { receiver: map }),
        Some(Site::Builder { op: s::Opcode::BuilderRange, inclusive: true, buffer: builder }),
        Some(Site::Collection { op: s::DataOp::ArrayLen, operand: INTS, result: s::Type::Int }),
        None,
    ]);
}
