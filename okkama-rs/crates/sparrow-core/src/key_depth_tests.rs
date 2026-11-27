//! #1130: map keys walk their depth on explicit stacks. A key nested deeper than a small machine stack (wasm32's
//! is about 1 MiB) is checked, built, compared, sized and dropped on a thread with such a stack; and on shallow
//! keys of every kind the walks give what the recursive definitions they replaced give.
use super::{Fields, Key};
use crate::program::Loaded;
use crate::runtime::Runtime;
use okkama_sparrow::TypeRef;
use std::cmp::Ordering;
use std::rc::Rc;

/// `Node` keys by its one field, typed `Node`; a `Leaf` (an int field) ends a chain. The walks follow each
/// object's own declaration, so a chain of nodes ending in a leaf keys like nested value objects.
const PROGRAM: &str = r#"sparrow 18
module "keys" "1.0.0"
type t0 int
type t1 object obj0
type t2 object obj1
object obj0 "Node" { field "inner" t1 }
object obj1 "Leaf" { field "n" t0 }
function f0 "main" {
    registers [t0]
    params []
    result t0
    0: const r0, int 0
    1: ret r0
}
entry f0
"#;

/// Deeper than a 1 MiB stack holds frames of the recursive walks (each well over 10 bytes).
const DEPTH: usize = 100_000;

/// A machine stack like wasm32's.
const SMALL_STACK: usize = 1 << 20;

fn chain(runtime: &mut Runtime, depth: usize, leaf: i64) -> crate::word::Word {
    let mut word = runtime.heap.object_new(1, &[crate::word::small(leaf).unwrap()]);
    runtime.roots.push((word, crate::word::Repr::Ref));
    for _ in 0..depth {
        word = runtime.heap.object_new(0, &[word]);
        runtime.roots.push((word, crate::word::Repr::Ref));
    }
    word
}

#[test]
fn a_key_deeper_than_a_small_stack_is_checked_built_compared_sized_and_dropped() {
    std::thread::Builder::new()
        .stack_size(SMALL_STACK)
        .spawn(|| {
            let program = okkama_sparrow::assemble(PROGRAM).unwrap();
            let mut runtime = Box::new(Runtime::new(Loaded::verified(program).unwrap()).unwrap());
            let (kind, ty) = (runtime.kind_of(TypeRef(1)), runtime.loaded.program.ty(TypeRef(1)).cloned());
            let (seven, again, eight) = (chain(&mut runtime, DEPTH, 7), chain(&mut runtime, DEPTH, 7), chain(&mut runtime, DEPTH, 8));
            assert!(runtime.is_key(seven, kind, ty.as_ref()));
            let key = runtime.checked_key(seven, kind, ty.as_ref()).unwrap();
            let same = runtime.key(again, kind, ty.as_ref());
            let larger = runtime.key(eight, kind, ty.as_ref());
            assert_eq!(key.cmp(&same), Ordering::Equal);
            assert_eq!(key, same);
            assert_eq!(key.cmp(&larger), Ordering::Less, "the leaves decide, at the bottom");
            assert_eq!(larger.cmp(&key), Ordering::Greater);
            let level = 16 + std::mem::size_of::<Key>();
            assert_eq!(key.data_bytes(), (DEPTH + 1) * level);
            assert_eq!(key.owned_bytes(), (DEPTH + 1) * level);
            let shared = key.clone();
            assert_eq!(key.owned_bytes(), (DEPTH + 1) * level / 2, "the top level, and all it holds, is shared by two");
            drop((key, same, larger));
            assert_eq!(shared.data_bytes(), (DEPTH + 1) * level, "a copy outlives the original");
            drop(shared);
            // A chain whose leaf is not bound yet (#1076) keys by nothing.
            let unbound = runtime.heap.object_new(1, &[crate::word::unbound(crate::word::Repr::Int)]);
            let mut top = unbound;
            for _ in 0..DEPTH {
                top = runtime.heap.object_new(0, &[top]);
            }
            assert!(!runtime.is_key(top, kind, ty.as_ref()));
        })
        .unwrap()
        .join()
        .unwrap();
}

// ---- the recursive definitions the walks replaced (before #1130) ----

fn reference_cmp(a: &Key, b: &Key) -> Ordering {
    match (a, b) {
        (Key::Object(x, xs), Key::Object(y, ys)) => {
            x.cmp(y).then_with(|| xs.iter().zip(ys.iter()).map(|(p, q)| reference_cmp(p, q)).find(|o| o.is_ne()).unwrap_or(xs.len().cmp(&ys.len())))
        }
        _ => a.cmp(b),
    }
}

fn reference_data_bytes(key: &Key) -> usize {
    match key {
        Key::Object(_, fields) => 16 + fields.len() * std::mem::size_of::<Key>() + fields.iter().map(reference_data_bytes).sum::<usize>(),
        other => other.data_bytes(),
    }
}

fn reference_owned_bytes(key: &Key) -> usize {
    match key {
        Key::Object(_, fields) => {
            (16 + fields.len() * std::mem::size_of::<Key>() + fields.iter().map(reference_owned_bytes).sum::<usize>())
                / Rc::strong_count(&fields.0)
        }
        other => other.owned_bytes(),
    }
}

/// Shallow keys of every kind, objects up to three deep, some levels shared: a small LCG picks them.
fn keys() -> Vec<Key> {
    let mut seed = 0x9e37_79b9_u64;
    let mut next = move |bound: u64| {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (seed >> 33) % bound
    };
    fn make(next: &mut dyn FnMut(u64) -> u64, depth: u32, shared: &mut Vec<Fields>) -> Key {
        match next(if depth == 0 { 6 } else { 10 }) {
            0 => Key::Small(next(5) as i64 - 2),
            1 => Key::int(crate::heap::Integer::Big(&(num_bigint::BigInt::from(1u8) << (64 + next(3) as usize)))),
            2 => Key::Float(next(4) as f64 / 2.0),
            3 => Key::Raw(next(3)),
            4 => Key::text(Rc::from(["", "a", "abcdefgh", "abcdefghi", "b"][next(5) as usize])),
            5 => Key::Rat(Rc::new(okkama_runtime_core::rat::Rat::new(
                &okkama_runtime_core::IntValue::from_i64(next(3) as i64 + 1),
                &okkama_runtime_core::IntValue::from_i64(2),
            ).unwrap())),
            6 if !shared.is_empty() => Key::Object(next(2) as u32, shared[next(shared.len() as u64) as usize].clone()),
            _ => {
                let count = next(4);
                let fields: Fields = (0..count).map(|_| make(next, depth - 1, shared)).collect::<Vec<_>>().into();
                shared.push(fields.clone());
                Key::Object(next(2) as u32, fields)
            }
        }
    }
    let mut shared = Vec::new();
    (0..300).map(|_| make(&mut next, 3, &mut shared)).collect()
}

#[test]
fn the_walks_give_what_the_recursive_definitions_give() {
    let keys = keys();
    assert!(keys.iter().filter(|key| matches!(key, Key::Object(_, fields) if fields.iter().any(|f| matches!(f, Key::Object(..))))).count() > 20);
    for a in &keys {
        assert_eq!(a.data_bytes(), reference_data_bytes(a), "{a:?}");
        assert_eq!(a.owned_bytes(), reference_owned_bytes(a), "{a:?}");
        for b in &keys {
            assert_eq!(a.cmp(b), reference_cmp(a, b), "{a:?} against {b:?}");
        }
    }
}
