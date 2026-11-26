//! #541: a call of a field accessor is compiled as the load itself; every
//! other call stays a call, and `SPARROW_CORE_PLAIN` keeps them all.
use super::*;
use cranelift_codegen::ir::Opcode;

const PROGRAM: &str = r#"
sparrow 18
module "accessors" "1.0.0"
type t0 int
type t1 object obj0
object obj0 "P" { field "x" t0 field "y" t0 }
function f0 "P.x" {
 registers [t1, t0]
 params [r0]
 result t0
 0: obj.get r1, r0, 0
 1: ret r1
}
function f1 "P.y_moved" {
 registers [t1, t0, t0]
 params [r0]
 result t0
 0: obj.get r1, r0, 1
 1: move r2, r1
 2: ret r2
}
function f2 "caller" {
 registers [t0, t0, t1, t0, t0, t0]
 params []
 result t0
 0: const r0, int 40
 1: const r1, int 2
 2: obj.new r2, obj0, [r0, r1]
 3: call r3, f0, [r2]
 4: call r4, f1, [r2]
 5: int.add r5, r3, r4
 6: ret r5
}
"#;

fn runtime(plain: bool) -> Box<Runtime> {
    let program = okkama_sparrow::assemble(PROGRAM).unwrap();
    let mut runtime = Box::new(Runtime::new(crate::program::Loaded::new(program)).unwrap());
    runtime.jit.plain = Chosen { all: plain, ..Chosen::default() };
    // A non-accessor keeps its call here: general inlining (#926) is off, so only the accessor rule applies.
    runtime.jit.no_inline = Chosen { all: true, ..Chosen::default() };
    runtime
}

fn calls(runtime: &Runtime, caller: usize) -> usize {
    let func = &runtime.jit.emitted[&caller];
    func.layout.blocks().flat_map(|block| func.layout.block_insts(block))
        // Okkama calls are indirect (a code address); runtime helpers are direct.
        .filter(|&inst| func.dfg.insts[inst].opcode() == Opcode::CallIndirect)
        .count()
}

#[test]
fn only_the_exact_accessor_shape_qualifies() {
    let program = okkama_sparrow::assemble(PROGRAM).unwrap();
    assert_eq!(accessor(&program.functions[0]), Some((0, 0)));
    assert_eq!(accessor(&program.functions[1]), None, "a move after the load");
    assert_eq!(accessor(&program.functions[2]), None);
    let mut handled = program.functions[0].clone();
    handled.is_async = true;
    assert_eq!(accessor(&handled), None, "async");
}

#[test]
fn an_accessor_call_becomes_its_load_with_the_same_result() {
    for plain in [false, true] {
        let mut runtime = runtime(plain);
        compile(&mut runtime, 0).unwrap();
        compile(&mut runtime, 1).unwrap();
        assert_eq!(runtime.run_function(2, 2 << 20).unwrap(), word::small(42).unwrap(), "plain {plain}");
        assert_eq!(calls(&runtime, 2), if plain { 2 } else { 1 }, "plain {plain}");
    }
}
