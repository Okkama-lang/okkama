//! #1074: a closure over a function of more than 8 parameters is called like
//! any other: directly from machine code, through the runtime's helper (more
//! captures, `SPARROW_CORE_PLAIN`), and for an async function as its cold
//! Future; the helper calls such code through an adapter of its arity.
use super::*;

/// `body` takes `params` ints and folds them, in order, as `acc + acc + p`; `run` closes it over the
/// first `captures` of the values 1, 2, … and calls it with the rest (awaiting it when `asynchronous`).
fn program(params: usize, captures: usize, asynchronous: bool) -> String {
    let acc = params;
    let mut body = format!(" 0: move r{acc}, r0\n");
    for i in 1..params {
        body.push_str(&format!(" {}: int.add r{acc}, r{acc}, r{acc}\n {}: int.add r{acc}, r{acc}, r{i}\n", 2 * i - 1, 2 * i));
    }
    body.push_str(&format!(" {}: ret r{acc}\n", 2 * params - 1));
    let ints = |n: usize| vec!["t0"; n].join(", ");
    let list = |range: std::ops::Range<usize>| range.map(|i| format!("r{i}")).collect::<Vec<_>>().join(", ");
    let consts: String = (0..params).map(|i| format!(" {i}: const r{i}, int {}\n", i + 1)).collect();
    let (closure, called, result) = (params, params + 1, params + 2);
    let fn_type = format!("type t2 fn [{}] -> {}", vec!["t0"; params - captures].join(", "), if asynchronous { "t1" } else { "t0" });
    let tail = if asynchronous {
        format!(" {}: await r{result}, r{called}\n {}: ret r{result}\n", params + 2, params + 3)
    } else {
        format!(" {}: ret r{called}\n", params + 2)
    };
    let mode = if asynchronous { " async" } else { "" };
    format!(
        "sparrow 19\nmodule \"wide\" \"1.0.0\"\ntype t0 int\ntype t1 future t0\n{fn_type}\n\
         function f0 \"body\"{mode} {{\n registers [{}]\n params [{}]\n result t0\n{body}}}\n\
         function f1 \"run\"{mode} {{\n registers [{}, t2, {}, t0]\n params []\n result t0\n{consts} {}: fn.closure r{closure}, f0, [{}]\n {}: call.fn r{called}, r{closure}, [{}]\n{tail}}}\n",
        ints(params + 1), list(0..params),
        ints(params), if asynchronous { "t1" } else { "t0" },
        params, list(0..captures), params + 1, list(captures..params),
    )
}

fn expected(params: usize) -> i64 {
    (0..params).fold(0, |acc, i| acc + acc + (i as i64 + 1))
}

#[test]
fn a_closure_over_more_than_eight_parameters_is_called() {
    for (params, captures) in [(9, 0), (10, 2), (10, 3), (12, 11), (20, 5), (8, 3)] {
        for asynchronous in [false, true] {
            for plain in [false, true] {
                let text = program(params, captures, asynchronous);
                let program = okkama_sparrow::assemble(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
                let mut runtime = Box::new(Runtime::new(crate::program::Loaded::new(program)).unwrap());
                runtime.jit.plain = Chosen { all: plain, ..Chosen::default() };
                let result = runtime.run_function(1, 2 << 20);
                let case = format!("{params} parameters, {captures} captures, async {asynchronous}, plain {plain}");
                assert_eq!(result.map_err(|e| e.0), Ok(word::small(expected(params)).unwrap()), "{case}");
            }
        }
    }
}

#[test]
fn wide_calls_from_rust_share_one_adapter_per_arity() {
    let program = okkama_sparrow::assemble(&program(11, 3, false)).unwrap();
    let mut runtime = Box::new(Runtime::new(crate::program::Loaded::new(program)).unwrap());
    runtime.jit.plain = Chosen { all: true, ..Chosen::default() };
    for _ in 0..2 {
        assert_eq!(runtime.run_function(1, 2 << 20).map_err(|e| e.0), Ok(word::small(expected(11)).unwrap()));
    }
    assert_eq!(runtime.jit.spreads.keys().copied().collect::<Vec<_>>(), vec![11], "the helper's call of 11 words");
    let adapter = runtime.jit.spread(11).unwrap();
    assert_eq!(runtime.jit.spreads[&11], adapter, "compiled once");
    assert_ne!(runtime.jit.spread(12).unwrap(), adapter);
}

#[test]
fn a_collection_during_a_wide_call_passes_over_the_adapter() {
    // The body allocates until the collector runs, with the adapter's frame below it; its string parameters,
    // traced only by the body's own frame, survive.
    let names: Vec<String> = (0..9).map(|i| format!("r{i}")).collect();
    let text = format!(
        "sparrow 19\nmodule \"wide-gc\" \"1.0.0\"\ntype t0 int\ntype t1 str\ntype t2 bool\ntype t3 array t1\n\
         type t4 fn [t1, t1, t1, t1, t1, t1] -> t0\n\
         function f0 \"body\" {{\n registers [t1, t1, t1, t1, t1, t1, t1, t1, t1, t0, t0, t0, t3, t2, t1, t0]\n\
         params [{}]\n result t0\n 0: const r9, int 0\n 1: const r10, int 200000\n 2: const r11, int 1\n\
         3: int.cmp lt, r13, r9, r10\n 4: br r13, @5, @8\n 5: arr.new r12, [{}]\n 6: int.add r9, r9, r11\n 7: jmp @3\n\
         8: str.concat r14, r0, r8\n 9: str.len r15, r14\n 10: ret r15\n}}\n\
         function f1 \"run\" {{\n registers [t1, t1, t1, t1, t1, t1, t1, t1, t1, t4, t0]\n params []\n result t0\n{} 9: fn.closure r9, f0, [r0, r1, r2]\n\
         10: call.fn r10, r9, [r3, r4, r5, r6, r7, r8]\n 11: ret r10\n}}\n",
        names.join(", "),
        names.join(", "),
        (0..9).map(|i| format!(" {i}: const r{i}, str \"{}{}\"\n", (b'a' + i as u8) as char, "x".repeat(i))).collect::<String>(),
    );
    let program = okkama_sparrow::assemble(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
    let mut runtime = Box::new(Runtime::new(crate::program::Loaded::new(program)).unwrap());
    runtime.jit.plain = Chosen { all: true, ..Chosen::default() };
    let collections = runtime.heap.stats().collections;
    assert_eq!(runtime.run_function(1, 2 << 20).map_err(|e| e.0), Ok(word::small(1 + 9).unwrap()));
    assert!(runtime.heap.stats().collections > collections, "the body collected");
}
