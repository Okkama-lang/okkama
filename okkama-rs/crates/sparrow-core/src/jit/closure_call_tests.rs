//! #928: a closure call goes straight to the body for 0, 1 or 2 captures; more captures, an async body,
//! `SPARROW_CORE_PLAIN` and statistics keep the runtime's helper. Results are the same either way.
use super::*;
use cranelift_codegen::ir::{ExternalName, InstructionData, Opcode};

/// `make(k)` returns a closure over k captures that adds them to its argument; `run(k, x)` calls it.
fn program(captures: usize) -> String {
    let regs: Vec<String> = (0..captures + 1).map(|_| "t0".into()).collect();
    let params: Vec<String> = (0..captures + 1).map(|i| format!("r{i}")).collect();
    let mut body = String::new();
    let mut acc = captures; // the argument is the last parameter
    for (n, i) in (0..captures).enumerate() {
        body.push_str(&format!(" {n}: int.add r{}, r{acc}, r{i}\n", captures + 1 + n));
        acc = captures + 1 + n;
    }
    let total = captures + 1 + captures;
    let body_regs: Vec<String> = (0..total).map(|_| "t0".into()).collect();
    let caps: Vec<String> = (0..captures).map(|i| format!("r{i}")).collect();
    let consts: String = (0..captures).map(|i| format!(" {i}: const r{i}, int {}\n", 10 * (i + 1))).collect();
    format!(
        "sparrow 19\nmodule \"closures\" \"1.0.0\"\ntype t0 int\ntype t1 fn [t0] -> t0\n\
         function f0 \"body\" {{\n registers [{}]\n params [{}]\n result t0\n{body} {}: ret r{acc}\n}}\n\
         function f1 \"run\" {{\n registers [{}, t1, t0]\n params [r{captures}]\n result t0\n{consts} {c}: fn.closure r{f}, f0, [{}]\n {d}: call.fn r{g}, r{f}, [r{captures}]\n {e}: ret r{g}\n}}\n",
        body_regs.join(", "), params.join(", "), captures,
        regs.join(", "), caps.join(", "),
        c = captures, d = captures + 1, e = captures + 2, f = captures + 1, g = captures + 2,
    )
}

fn helper_calls(runtime: &Runtime, function: usize, helper: FuncId) -> usize {
    let func = &runtime.jit.emitted[&function];
    func.layout.blocks().flat_map(|block| func.layout.block_insts(block)).filter(|&inst| {
        let InstructionData::Call { func_ref, .. } = func.dfg.insts[inst] else { return false };
        let ExternalName::User(name) = func.dfg.ext_funcs[func_ref].name else { return false };
        func.params.user_named_funcs()[name].index == helper.as_u32()
    }).count()
}

fn indirect_calls(runtime: &Runtime, function: usize) -> usize {
    let func = &runtime.jit.emitted[&function];
    func.layout.blocks().flat_map(|block| func.layout.block_insts(block))
        .filter(|&inst| func.dfg.insts[inst].opcode() == Opcode::CallIndirect).count()
}

#[test]
fn closures_with_up_to_two_captures_are_called_directly_and_answer_the_same() {
    for captures in 0..=3 {
        for plain in [false, true] {
            let program = okkama_sparrow::assemble(&program(captures)).unwrap_or_else(|e| panic!("{e:?}\n{}", program(captures)));
            let mut runtime = Box::new(Runtime::new(crate::program::Loaded::new(program)).unwrap());
            runtime.jit.plain = Chosen { all: plain, ..Chosen::default() };
            let argument = word::small(5).unwrap();
            let result = runtime.run_function_with(1, &[argument], 2 << 20).unwrap();
            let expected = 5 + (1..=captures as i64).map(|i| 10 * i).sum::<i64>();
            assert_eq!(result, word::small(expected).unwrap(), "{captures} captures, plain {plain}");
            assert_eq!(helper_calls(&runtime, 1, runtime.jit.helpers.call_closure), 1, "the helper is always the fallback");
            // one direct call per capture count 0..=2 the site can take, and one through the closure's entry
            // word (#1110); none when plain
            assert_eq!(indirect_calls(&runtime, 1), if plain { 0 } else { 4 }, "{captures} captures, plain {plain}");
        }
    }
}
