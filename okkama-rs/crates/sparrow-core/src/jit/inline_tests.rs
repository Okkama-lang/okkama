//! #926/#927: a small static callee is compiled into its caller (no call left), recursion one level deep; a
//! callee with a handler and `SPARROW_CORE_NO_INLINE` keep the call.
use super::*;
use cranelift_codegen::ir::Opcode;

const PROGRAM: &str = r#"
sparrow 16
module "inline" "1.0.0"
type t0 int
type t1 bool
type t2 failure
function f0 "twice" {
 registers [t0, t0]
 params [r0]
 result t0
 0: int.add r1, r0, r0
 1: ret r1
}
function f1 "caller" {
 registers [t0, t0]
 params [r0]
 result t0
 0: call r1, f0, [r0]
 1: ret r1
}
function f2 "fib" {
 registers [t0, t0, t1, t0, t0, t0]
 params [r0]
 result t0
 0: const r1, int 2
 1: int.cmp lt, r2, r0, r1
 2: br r2, @3, @4
 3: ret r0
 4: const r1, int 1
 5: int.sub r3, r0, r1
 6: call r4, f2, [r3]
 7: const r1, int 2
 8: int.sub r3, r0, r1
 9: call r5, f2, [r3]
 10: int.add r4, r4, r5
 11: ret r4
}
function f3 "guarded" {
 registers [t0, t2]
 params [r0]
 result t0
 handler 0 1 @2 r1
 0: int.add r0, r0, r0
 1: ret r0
 2: const r0, int 0
 3: ret r0
}
function f4 "calls guarded" {
 registers [t0, t0]
 params [r0]
 result t0
 0: call r1, f3, [r0]
 1: ret r1
}
"#;

fn runtime(no_inline: bool) -> Box<Runtime> {
    let program = okkama_sparrow::assemble(PROGRAM).unwrap();
    let mut runtime = Box::new(Runtime::new(crate::program::Loaded::new(program)).unwrap());
    runtime.jit.no_inline = Chosen { all: no_inline, ..Chosen::default() };
    runtime
}

fn calls(runtime: &mut Runtime, function: usize) -> usize {
    compile(runtime, function).unwrap();
    let func = &runtime.jit.emitted[&function];
    func.layout.blocks().flat_map(|block| func.layout.block_insts(block))
        .filter(|&inst| func.dfg.insts[inst].opcode() == Opcode::CallIndirect)
        .count()
}

#[test]
fn a_small_callee_leaves_no_call() {
    assert_eq!(calls(&mut runtime(false), 1), 0);
    assert_eq!(calls(&mut runtime(true), 1), 1, "inlining off keeps the call");
}

#[test]
fn recursion_is_inlined_one_level_deep() {
    assert_eq!(calls(&mut runtime(true), 2), 2);
    assert_eq!(calls(&mut runtime(false), 2), 4, "each inlined body keeps its own two calls");
}

#[test]
fn a_callee_with_a_handler_keeps_its_call() {
    assert_eq!(calls(&mut runtime(false), 4), 1);
}
