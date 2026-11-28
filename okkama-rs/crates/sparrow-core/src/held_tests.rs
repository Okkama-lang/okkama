//! #956: an object whose state the runtime holds compares by identity.
use super::*;

const PROGRAM: &str = r#"sparrow 19
module "held-956" "1.0.0"
type t0 int
type t1 object obj0
type t2 object obj1
type t3 bool
type t4 object obj3
type t5 object obj4
object obj0 "Atomic<int>" {}
object obj1 "Marker" {}
object obj2 "AtomicClosed" {}
object obj3 "Channel<int>" {}
object obj4 "Atomic<Marker>" {}
function f0 "value" intrinsic "okkama.atomic.Atomic.value" [obj2] [] {
    registers [t1]
    params [r0]
    result t0
}
function f1 "new_value" intrinsic "okkama.atomic.Atomic.new_value" [obj0] [] {
    registers [t0]
    params [r0]
    result t1
}
function f2 "new" intrinsic "okkama.channel.Channel.new" [obj3] [] {
    registers [t0]
    params [r0]
    result t4
}
function f3 "new_value" intrinsic "okkama.atomic.Atomic.new_value" [obj4] [] {
    registers [t2]
    params [r0]
    result t5
}
"#;

fn runtime() -> Box<Runtime> {
    let program = okkama_sparrow::assemble(PROGRAM).unwrap();
    Box::new(Runtime::new(crate::program::Loaded::new(program)).unwrap())
}

fn object_type(runtime: &Runtime, name: &str) -> okkama_sparrow::TypeRef {
    let program = &runtime.loaded.program;
    let index = program.objects.iter().position(|o| o.name == name).unwrap();
    let position = program
        .types
        .iter()
        .position(|t| matches!(t, okkama_sparrow::Type::Object(id) if id.0 as usize == index))
        .unwrap();
    okkama_sparrow::TypeRef(position as u32)
}

#[test]
fn atomics_compare_by_identity() {
    let mut runtime = runtime();
    let atomic = object_type(&runtime, "Atomic<int>");
    let descriptor = runtime.loaded.program.objects.iter().position(|o| o.name == "Atomic<int>").unwrap() as u32;
    let a = runtime.heap.object_new(descriptor, &[1, 0]);
    let b = runtime.heap.object_new(descriptor, &[1, 0]);
    assert!(runtime.held[descriptor as usize], "the receiver of an object intrinsic is held by the runtime");
    assert!(!crate::ops::equal(&runtime, a, b, atomic).unwrap(), "two atomics are two objects");
    assert!(crate::ops::equal(&runtime, a, a, atomic).unwrap(), "an atomic equals itself");
}

#[test]
fn a_value_object_without_fields_still_compares_by_value() {
    let mut runtime = runtime();
    let marker = object_type(&runtime, "Marker");
    let descriptor = runtime.loaded.program.objects.iter().position(|o| o.name == "Marker").unwrap() as u32;
    let a = runtime.heap.object_new(descriptor, &[]);
    let b = runtime.heap.object_new(descriptor, &[]);
    assert!(!runtime.held[descriptor as usize]);
    assert!(crate::ops::equal(&runtime, a, b, marker).unwrap(), "fieldless value objects are equal");
    // The error an intrinsic raises and a constructor's argument are values, not held objects.
    let closed = runtime.loaded.program.objects.iter().position(|o| o.name == "AtomicClosed").unwrap();
    assert!(!runtime.held[closed]);
}

#[test]
fn an_object_with_only_an_intrinsic_constructor_is_held() {
    let runtime = runtime();
    let position = |name: &str| runtime.loaded.program.objects.iter().position(|o| o.name == name).unwrap();
    assert!(runtime.held[position("Channel<int>")], "a channel is made by the runtime");
    assert!(runtime.held[position("Atomic<Marker>")]);
    assert!(!runtime.held[position("Marker")], "the value an atomic is made with stays a value");
}
