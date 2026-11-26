//! #1076: a stub of `undef` that `obj.new` or `obj.init` stores in a field (a
//! constructor's field not bound yet, #271) fails the program when a read of
//! that field meets it, instead of handing machine code the stub's word as a
//! value; the stub of a `bool` field reads as `true` (the compiler's
//! construction check reads its unbound `$pending` so); fields no stub
//! reaches are read by a bare load.
use super::*;

const PROGRAM: &str = r#"
sparrow 18
module "stub-fields" "1.0.0"
type t0 int
type t1 str
type t2 object obj0
type t3 bool
type t4 variant variant0
type t5 map t2 t0
type t6 object obj1
type t7 failure
type t8 float
object obj0 "Box" { field "name" t1 field "size" t0 field "ready" t3 field "weight" t8 }
object obj1 "Plain" { field "x" t1 }
variant variant0 "Shape" { case "box" obj0 }
function f0 "name" {
 registers [t1, t0, t3, t2, t1, t0, t8]
 params []
 result t0
 0: undef r0
 1: const r1, int 1
 2: const r2, bool true
 3: const r6, float 0x3ff8000000000000
 4: obj.new r3, obj0, [r0, r1, r2, r6]
 5: obj.get r4, r3, 0
 6: str.len r5, r4
 7: ret r5
}
function f1 "size" {
 registers [t1, t0, t3, t2, t0, t8]
 params []
 result t0
 0: undef r0
 1: const r1, int 1
 2: const r2, bool true
 3: const r5, float 0x3ff8000000000000
 4: obj.new r3, obj0, [r0, r1, r2, r5]
 5: obj.get r4, r3, 1
 6: ret r4
}
function f2 "bound_later" {
 registers [t1, t0, t3, t2, t1, t0, t1, t8]
 params []
 result t0
 0: undef r0
 1: const r1, int 1
 2: const r2, bool true
 3: const r7, float 0x3ff8000000000000
 4: obj.new r3, obj0, [r0, r1, r2, r7]
 5: const r6, str "ok"
 6: obj.init r3, 0, r6
 7: obj.get r4, r3, 0
 8: str.len r5, r4
 9: ret r5
}
function f3 "variant" {
 registers [t1, t0, t3, t2, t4, t1, t0, t8]
 params []
 result t0
 0: undef r0
 1: const r1, int 1
 2: const r2, bool true
 3: const r7, float 0x3ff8000000000000
 4: obj.new r3, obj0, [r0, r1, r2, r7]
 5: variant.of r4, r3, variant0
 6: variant.get r5, r4, 0, 0
 7: str.len r6, r5
 8: ret r6
}
function f4 "Box.name" {
 registers [t2, t1]
 params [r0]
 result t1
 0: obj.get r1, r0, 0
 1: ret r1
}
function f5 "accessor" {
 registers [t1, t0, t3, t2, t1, t0, t8]
 params []
 result t0
 0: undef r0
 1: const r1, int 1
 2: const r2, bool true
 3: const r6, float 0x3ff8000000000000
 4: obj.new r3, obj0, [r0, r1, r2, r6]
 5: call r4, f4, [r3]
 6: str.len r5, r4
 7: ret r5
}
function f6 "caught" {
 registers [t1, t0, t3, t2, t1, t0, t7, t8]
 params []
 result t0
 handler 5 7 @8 r6
 0: undef r0
 1: const r1, int 1
 2: const r2, bool true
 3: const r7, float 0x3ff8000000000000
 4: obj.new r3, obj0, [r0, r1, r2, r7]
 5: obj.get r4, r3, 0
 6: str.len r5, r4
 7: ret r5
 8: const r5, int 7
 9: ret r5
}
function f7 "ready" {
 registers [t1, t0, t3, t2, t3, t0, t8, t3, t3]
 params []
 result t0
 0: const r0, str "box"
 1: const r1, int 1
 2: undef r2
 3: const r6, float 0x3ff8000000000000
 4: obj.new r3, obj0, [r0, r1, r2, r6]
 5: obj.get r4, r3, 2
 6: const r7, bool true
 7: bool.cmp eq, r8, r4, r7
 8: br r8, @9, @11
 9: const r5, int 1
 10: ret r5
 11: const r5, int 0
 12: ret r5
}
function f8 "key" {
 registers [t1, t0, t3, t2, t5, t0, t8]
 params []
 result t0
 0: undef r0
 1: const r1, int 1
 2: const r2, bool true
 3: const r6, float 0x3ff8000000000000
 4: obj.new r3, obj0, [r0, r1, r2, r6]
 5: map.new r4, [r3: r1]
 6: map.len r5, r4
 7: ret r5
}
function f9 "plain" {
 registers [t1, t6, t1, t0]
 params []
 result t0
 0: const r0, str "abc"
 1: obj.new r1, obj1, [r0]
 2: obj.get r2, r1, 0
 3: str.len r3, r2
 4: ret r3
}
function f10 "weight" {
 registers [t1, t0, t3, t2, t8, t0, t8]
 params []
 result t0
 0: const r0, str "box"
 1: const r1, int 1
 2: const r2, bool true
 3: undef r6
 4: obj.new r3, obj0, [r0, r1, r2, r6]
 5: obj.get r4, r3, 3
 6: const r5, int 0
 7: ret r5
}
"#;

fn runtime() -> Box<Runtime> {
    let program = okkama_sparrow::assemble(PROGRAM).unwrap();
    Box::new(Runtime::new(crate::program::Loaded::new(program)).unwrap())
}

fn run(function: usize) -> Result<Word, String> {
    runtime().run_function(function, 2 << 20).map_err(|error| error.0)
}

fn unbound(failure: Result<Word, String>, field: &str) {
    let message = failure.expect_err("the read meets the stub");
    assert!(message.contains(&format!("Field `{field}` of `Box` is read before it is bound")), "{message}");
}

#[test]
fn a_field_read_that_meets_the_stub_fails_the_program() {
    unbound(run(0), "name");
    // A raw scalar's stub is not 0, and is found as well.
    unbound(run(10), "weight");
    // Through a variant of the case object.
    unbound(run(3), "name");
    // The failure is the program's: a handler catches it.
    assert_eq!(run(6), Ok(word::small(7).unwrap()));
}

#[test]
fn an_accessor_of_a_field_a_stub_reaches_stays_a_call_that_checks() {
    unbound(run(5), "name");
    // #541 would compile the call as the bare load; this field's load can fail, so the call stays.
    let mut runtime = runtime();
    runtime.jit.no_inline = Chosen { all: true, ..Chosen::default() };
    compile(&mut runtime, 4).unwrap();
    compile(&mut runtime, 5).unwrap();
    let func = &runtime.jit.emitted[&5];
    let calls = func.layout.blocks().flat_map(|block| func.layout.block_insts(block))
        .filter(|&inst| func.dfg.insts[inst].opcode() == cranelift_codegen::ir::Opcode::CallIndirect)
        .count();
    assert_eq!(calls, 1);
    assert!(runtime.run_function(5, 2 << 20).is_err());
}

#[test]
fn a_bound_field_and_a_field_no_stub_reaches_read_as_values() {
    assert_eq!(run(1), Ok(word::small(1).unwrap()));
    assert_eq!(run(2), Ok(word::small(2).unwrap()));
    assert_eq!(run(9), Ok(word::small(3).unwrap()));
    let runtime = runtime();
    let loaded = &runtime.loaded;
    // Only the fields an `undef` reaches pay for a check.
    assert!(loaded.may_hold_stub(0, 0) && loaded.may_hold_stub(0, 2) && loaded.may_hold_stub(0, 3));
    assert!(!loaded.may_hold_stub(0, 1), "Box.size holds only `int 1`");
    assert!(!loaded.may_hold_stub(1, 0), "Plain.x holds only a string");
}

#[test]
fn the_stub_of_a_bool_field_reads_as_true() {
    // The compiler's construction check: `$pending` unbound means the object is still under construction (#271).
    assert_eq!(run(7), Ok(word::small(1).unwrap()));
}

#[test]
fn an_object_with_a_field_not_bound_is_no_map_key() {
    let message = run(8).expect_err("refused");
    assert!(message.contains("map key must be"), "{message}");
}
