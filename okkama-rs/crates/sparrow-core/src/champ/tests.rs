use super::*;
use crate::heap::Layouts;
use crate::program::Loaded;
use crate::word::{self, Repr};
use okkama_sparrow::{FuncRef, Function, Instruction, ObjectDecl, ObjectRef, Program, Type, TypeRef};

fn facts(has_destroy: bool) -> LayoutFacts {
    LayoutFacts { has_destroy, may_reach_resource: has_destroy, may_async_destroy: false }
}

fn runtime(fields: Vec<(&str, TypeRef)>) -> Runtime {
    let program = Program {
        types: vec![Type::Int, Type::Str, Type::Object(ObjectRef(0))],
        objects: vec![ObjectDecl {
            name: "Resource".into(),
            fields: fields.into_iter().map(|(name, ty)| (name.into(), ty)).collect(),
            does: vec![],
            destroy: Some(FuncRef(0)),
            builtin: false,
        }],
        functions: vec![Function {
            symbol: "destroy".into(),
            is_async: false,
            native: None,
            intrinsic: None,
            registers: vec![TypeRef(2)],
            params: vec![okkama_sparrow::Register(0)],
            result: None,
            code: vec![Instruction::Return { value: None }],
            handlers: vec![],
        }],
        ..Program::default()
    };
    Runtime::new(Loaded::new(program)).unwrap()
}

fn ids(runtime: &mut Runtime) -> (HolderId, OwnerId) {
    (runtime.champ_new_holder().unwrap(), runtime.champ_new_owner().unwrap())
}

#[test]
fn champ_layout_sizes_offsets_and_classes() {
    let plain_bytes = [16, 16, 24, 32, 40];
    let identity_bytes = [40, 48, if heap::CHAMP_SEVEN_WORD_CLASS { 56 } else { 64 }, 64, 80];
    for count in 0..=4 {
        let plain = ObjectRuntimeLayout::new(vec![Repr::Raw; count], facts(false), vec![]).unwrap();
        let own = ObjectRuntimeLayout::new(vec![Repr::Raw; count], facts(true), vec![]).unwrap();
        assert_eq!(heap::CLASSES[Heap::class_of(plain.allocation_words()).unwrap()] * 8, plain_bytes[count]);
        assert_eq!(heap::CLASSES[Heap::class_of(own.allocation_words()).unwrap()] * 8, identity_bytes[count]);

    }
}

#[test]
fn champ_plain_and_resource_container_have_no_tail_or_serial() {
    let storage = vec![vec![], vec![Repr::Ref], vec![]];
    let objects = storage.iter().enumerate().map(|(index, fields)| {
        ObjectRuntimeLayout::new(fields.clone(), LayoutFacts {
            has_destroy: index == 2,
            may_reach_resource: index != 0,
            may_async_destroy: false,
        }, vec![]).unwrap()
    }).collect();
    let mut heap = Heap::new_champ(Layouts { objects: storage, ..Layouts::default() }, objects);
    let plain = heap.object_new(0, &[]);
    let resource = heap.object_new(2, &[]);
    let container = heap.object_new(1, &[resource]);
    assert_eq!(heap.champ_identity(plain), None);
    assert_eq!(heap.champ_identity(container), None);
    assert_eq!(heap.header_of(container) & HAS_IDENTITY, 0);
    assert_eq!(heap.object_runtime_layout(1).allocation_words(), 2);
    assert_eq!(heap.champ_identity(resource), Some(Identity {
        serial: 0, holder_id: 0, owner_id: 0, epoch: 0, life: Life::Uncommitted,
    }));
    let array = heap.allocate_array(Repr::Ref, &[resource]);
    // Kind gating must ignore the overlapping HAS_IDENTITY/SHARED bit.
    unsafe {
        *(array as *mut u64) |= HAS_IDENTITY;
    }
    assert_eq!(heap.champ_identity(array), None);
}

#[test]
fn champ_completion_is_explicit_and_serial_is_per_runtime() {
    let mut first = runtime(vec![]);
    let mut second = runtime(vec![]);
    let (holder, owner) = ids(&mut first);
    assert_eq!((holder.get(), owner.get()), (1, 1));
    let incomplete = first.heap.object_new(0, &[]);
    let a = first.heap.object_new(0, &[]);
    first.created(0, a);
    assert_eq!(first.heap.champ_identity(a).unwrap().life, Life::Uncommitted);
    let ka = first.champ_create_commit(a, holder, owner).unwrap().unwrap();
    assert_eq!(ka.serial(), 1);
    assert_eq!(ka.object(), a);
    assert!(first.champ_create_commit(a, holder, owner).is_err());
    let b = first.heap.object_new(0, &[]);
    assert_eq!(first.champ_create_commit(b, holder, owner).unwrap().unwrap().serial(), 2);
    assert_eq!(first.heap.champ_identity(incomplete).unwrap().serial, 0);
    let (other_holder, other_owner) = ids(&mut second);
    let c = second.heap.object_new(0, &[]);
    assert_eq!(second.champ_create_commit(c, other_holder, other_owner).unwrap().unwrap().serial(), 1);
    assert_eq!(first.champ_new_holder().unwrap().get(), 2);
    assert_eq!(first.champ_new_owner().unwrap().get(), 2);
}

#[test]
fn champ_life_api_preserves_tail_and_header_marks_and_rejects_invalid_transitions() {
    let mut runtime = runtime(vec![("value", TypeRef(0))]);
    let (holder, owner) = ids(&mut runtime);
    let object = runtime.heap.object_new(0, &[word::small(17).unwrap()]);
    let key = runtime.champ_create_commit(object, holder, owner).unwrap().unwrap();
    runtime.heap.visit(object);
    let before = runtime.heap.champ_identity(object).unwrap();
    assert_eq!(before.epoch, 1);
    assert!(runtime.champ_finish_destroy(key, holder, owner).is_err());
    let foreign = runtime.champ_new_holder().unwrap();
    assert!(runtime.champ_begin_destroy(key, foreign, owner).is_err());
    let stale = KeyRef { serial: key.serial + 1, ..key };
    assert!(runtime.champ_begin_destroy(stale, holder, owner).is_err());
    assert_eq!(runtime.heap.champ_identity(object), Some(before));
    runtime.champ_begin_destroy(key, holder, owner).unwrap();
    assert_eq!(runtime.heap.champ_identity(object).unwrap().life, Life::Destroying);
    assert!(runtime.champ_begin_destroy(key, holder, owner).is_err());
    // Ordinary access has no life-state check (m6), including destroying/dead aliases.
    assert_eq!(runtime.heap.field(object, 0), word::small(17).unwrap());
    runtime.heap.collect(|visit| visit(object, Repr::Ref));
    runtime.champ_finish_destroy(key, holder, owner).unwrap();
    assert!(runtime.champ_finish_destroy(key, holder, owner).is_err());
    assert_eq!(runtime.heap.champ_identity(object), Some(Identity { life: Life::Dead, ..before }));
    assert_ne!(runtime.heap.header_of(object) & heap::VISITED, 0);
    assert_eq!(runtime.heap.header_of(object) & (7 << 18), 0);
    runtime.heap.unvisit(object);
    runtime.heap.collect(|visit| visit(object, Repr::Ref));
    assert_eq!(runtime.heap.field(object, 0), word::small(17).unwrap());
    assert_eq!(runtime.heap.champ_identity(object).unwrap().life, Life::Dead);
}

#[test]
fn champ_counter_overflow_has_no_partial_commit() {
    let mut runtime = runtime(vec![]);
    let (holder, owner) = ids(&mut runtime);
    let object = runtime.heap.object_new(0, &[]);
    let before = runtime.heap.champ_identity(object);
    runtime.champ.serial = u64::MAX;
    assert!(runtime.champ_create_commit(object, holder, owner).is_err());
    assert_eq!(runtime.heap.champ_identity(object), before);
    runtime.champ.holder = u64::MAX;
    runtime.champ.owner = u64::MAX;
    assert!(runtime.champ_new_holder().is_err());
    assert!(runtime.champ_new_owner().is_err());
    assert_eq!(runtime.champ.holder, u64::MAX);
    assert_eq!(runtime.champ.owner, u64::MAX);
}

#[test]
fn champ_successor_copies_storage_only_and_gets_new_identity() {
    let mut runtime = runtime(vec![("a", TypeRef(0)), ("b", TypeRef(0))]);
    let (holder, owner) = ids(&mut runtime);
    let source = runtime.heap.object_new(0, &[11, 13]);
    let first = runtime.champ_create_commit(source, holder, owner).unwrap().unwrap();
    runtime.champ_begin_destroy(first, holder, owner).unwrap();
    runtime.champ_finish_destroy(first, holder, owner).unwrap();
    runtime.heap.visit(source);
    // Simulate a collector-marked predecessor: successor must start with neither mark.
    unsafe {
        *(source as *mut u64) |= 1 << 8;
    }
    let successor = runtime.heap.object_successor(source, &[(1, 19)]);
    assert_eq!(runtime.heap.field(successor, 0), 11);
    assert_eq!(runtime.heap.field(successor, 1), 19);
    assert_eq!(runtime.heap.header_of(successor), runtime.heap.object_runtime_layout(0).header(0));
    assert_eq!(runtime.heap.champ_identity(successor).unwrap().serial, 0);
    assert_eq!(runtime.heap.champ_identity(successor).unwrap().life, Life::Uncommitted);
    let second = runtime.champ_create_commit(successor, holder, owner).unwrap().unwrap();
    assert_eq!(second.serial(), 2);
    assert_ne!(first, second);
    assert_eq!(runtime.heap.champ_identity(source).unwrap().serial, 1);
}

#[test]
fn champ_gc_traces_fields_and_bigints_but_never_any_tail_word() {
    let mut runtime = runtime(vec![("text", TypeRef(1)), ("number", TypeRef(0))]);
    runtime.heap.set_stress(true);
    let text = runtime.heap.allocate_text("kept");
    let big = runtime.heap.big(num_bigint::BigInt::from(1u64) << 100);
    let garbage = runtime.heap.allocate_text("tail is raw");
    runtime.heap.watch(garbage);
    let object = runtime.heap.object_new(0, &[text, big]);
    // Adversarial bit patterns in ALL four metadata words: each looks like a valid Ref.
    runtime.heap.identity_write(object, Identity {
        serial: garbage, holder_id: garbage, owner_id: garbage, epoch: garbage, life: Life::Live,
    });
    runtime.roots.push((object, Repr::Ref));
    for _ in 0..3 {
        runtime.heap.collect(|visit| {
            for &(word, repr) in &runtime.roots {
                visit(word, repr);
            }
        });
        assert_eq!(runtime.heap.text(runtime.heap.field(object, 0)), "kept");
        assert_eq!(runtime.heap.field(object, 1), big);
        assert_eq!(runtime.heap.integer(big).to_string(), "1267650600228229401496703205376");
        assert_eq!(runtime.heap.champ_identity(object).unwrap().epoch, garbage);
    }
    assert_eq!(runtime.heap.take_released(), vec![garbage]);
    runtime.heap.identity_life(object, Life::Dead);
    runtime.roots.clear();
    runtime.heap.collect(|_| {});
    assert_eq!(runtime.heap.stats().live_bytes, 0);
}

#[test]
fn champ_sweep_rejects_lost_live_and_destroying_small_and_large_identities() {
    for count in [0, 128] {
        for life in [Life::Live, Life::Destroying] {
            let descriptor = ObjectRuntimeLayout::new(vec![Repr::Raw; count], facts(true), vec![]).unwrap();
            let mut heap = Heap::new_champ(
                Layouts { objects: vec![vec![Repr::Raw; count]], ..Layouts::default() },
                vec![descriptor].into_boxed_slice(),
            );
            heap.set_stress(true);
            let object = heap.object_new(0, &[]);
            heap.identity_write(object, Identity { serial: 1, holder_id: 1, owner_id: 1, epoch: 1, life });
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| heap.collect(|_| {})));
            assert!(result.is_err());
        }
    }
}

#[test]
fn champ_dump_and_destroy_trace_use_creation_ordinal_per_runtime() {
    for _ in 0..2 {
        let mut runtime = runtime(vec![("value", TypeRef(0))]);
        let (holder, owner) = ids(&mut runtime);
        let a = runtime.heap.object_new(0, &[word::small(42).unwrap()]);
        let b = runtime.heap.object_new(0, &[word::small(73).unwrap()]);
        let ka = runtime.champ_create_commit(a, holder, owner).unwrap().unwrap();
        let kb = runtime.champ_create_commit(b, holder, owner).unwrap().unwrap();
        let mut trace = Vec::new();
        runtime.champ_trace_destroy(kb, &mut trace).unwrap();
        runtime.champ_trace_destroy(ka, &mut trace).unwrap();
        assert_eq!(String::from_utf8(trace).unwrap(), "Resource 2\nResource 1\n");
        let dump = runtime.heap.champ_dump_object(a);
        assert!(dump.contains("fields=[85] identity serial=1 holder=1 owner=1 epoch=1 life=Live"));
        assert!(!dump.contains(&format!("fields=[85, {}", ka.serial())));
    }
}

#[test]
fn champ_intrinsic_overrides_define_allocation_and_hidden_storage() {
    for (name, storage) in [
        ("okkama.atomic.Atomic.new_value", vec![Repr::Int, Repr::Raw]),
        ("okkama.event.Event.new", vec![Repr::Raw, Repr::Raw, Repr::Ref]),
    ] {
        let mut program = runtime(vec![]).loaded.program;
        let atomic = name.contains("atomic");
        program.functions.push(Function {
            symbol: "new".into(),
            is_async: false,
            native: None,
            intrinsic: Some(okkama_sparrow::Intrinsic {
                name: name.into(), objects: vec![ObjectRef(0)], helpers: vec![],
            }),
            registers: if atomic { vec![TypeRef(0)] } else { vec![] },
            params: if atomic { vec![okkama_sparrow::Register(0)] } else { vec![] },
            result: Some(TypeRef(2)),
            code: vec![],
            handlers: vec![],
        });
        let mut runtime = Runtime::new(Loaded::new(program)).unwrap();
        let descriptor = runtime.heap.object_runtime_layout(0);
        assert_eq!(descriptor.storage_fields(), storage);
        assert_eq!(descriptor.identity_offset(), Some(8 * (storage.len() + 1)));
        assert_eq!(descriptor.allocation_words(), storage.len() + 5);
        // Constructor supplies fewer fields than physical storage; tail stays beyond ALL storage.
        let object = runtime.heap.object_new(0, &[11]);
        assert_eq!(runtime.heap.field(object, 0), 11);
        for index in 1..storage.len() {
            assert_eq!(runtime.heap.field(object, index), 0);
        }
        assert_eq!(runtime.heap.champ_identity(object).unwrap().serial, 0);
        let successor = runtime.heap.object_successor(object, &[]);
        assert_eq!(runtime.heap.field(successor, 0), 11);
        assert_eq!(runtime.heap.champ_identity(successor).unwrap().epoch, 0);
    }
}

#[test]
fn champ_plain_completion_does_not_consume_creation_ordinal() {
    let mut program = runtime(vec![]).loaded.program;
    program.objects[0].destroy = None;
    let mut runtime = Runtime::new(Loaded::new(program)).unwrap();
    let (holder, owner) = ids(&mut runtime);
    let object = runtime.heap.object_new(0, &[]);
    assert_eq!(runtime.champ_create_commit(object, holder, owner).unwrap(), None);
    assert_eq!(runtime.champ.serial, 0);
    assert_eq!(runtime.heap.champ_dump_object(object), "object 0 fields=[]");
}


#[test]
fn champ_uncommitted_self_can_be_collected_while_constructor_inputs_stay_rooted() {
    let mut runtime = runtime(vec![("text", TypeRef(1))]);
    runtime.heap.set_stress(true);
    let text = runtime.heap.allocate_text("constructor input");
    let incomplete = runtime.heap.object_new(0, &[text]);
    runtime.heap.watch(incomplete);
    runtime.roots.push((text, Repr::Ref));
    runtime.heap.collect(|visit| {
        for &(word, repr) in &runtime.roots {
            visit(word, repr);
        }
    });
    assert_eq!(runtime.heap.take_released(), vec![incomplete]);
    assert_eq!(runtime.heap.text(text), "constructor input");
    assert_eq!(runtime.champ.serial, 0);
    let fresh = runtime.heap.object_new(0, &[]);
    assert_eq!(runtime.heap.field(fresh, 0), 0);
    assert_eq!(runtime.heap.champ_identity(fresh), Some(Identity {
        serial: 0, holder_id: 0, owner_id: 0, epoch: 0, life: Life::Uncommitted,
    }));
}
