//! #933: where machine code lands. Every function starts on a page of its
//! own, at the configured shift, whatever was compiled before it.
use super::*;

const THREE: &str = r#"
sparrow 19
module "placement" "1.0.0"
type t0 int
function f0 "identity" {
 registers [t0]
 params [r0]
 result t0
 0: ret r0
}
function f1 "double" {
 registers [t0, t0]
 params [r0]
 result t0
 0: int.add r1, r0, r0
 1: ret r1
}
function f2 "square" {
 registers [t0, t0]
 params [r0]
 result t0
 0: int.mul r1, r0, r0
 1: ret r1
}
"#;

/// The in-page offset of each function, compiled in `order`, by function index.
fn offsets(order: &[usize], shift: usize) -> Vec<usize> {
    let program = okkama_sparrow::assemble(THREE).unwrap();
    let mut runtime = Box::new(Runtime::new(crate::program::Loaded::new(program)).unwrap());
    // Nothing is compiled yet: the JIT can be replaced by one with another shift.
    runtime.jit = Jit::with_code_shift(shift).unwrap();
    let mut placed = vec![0; 3];
    for &function in order {
        let address = compile(&mut runtime, function).unwrap() as usize;
        placed[function] = address % crate::code_memory::page();
    }
    placed
}

#[test]
fn functions_start_at_the_shift_past_a_page_in_any_compile_order() {
    for shift in [0, 64, 1024] {
        let forward = offsets(&[0, 1, 2], shift);
        assert_eq!(forward, vec![shift; 3], "shift {shift}");
        assert_eq!(offsets(&[2, 0, 1], shift), forward, "shift {shift}: the compile order moved code within its page");
    }
}
