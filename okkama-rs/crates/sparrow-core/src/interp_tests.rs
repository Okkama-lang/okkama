//! The interpreter (#1120): each family of its branches, the seam and the roots, as the JIT runs the same SPB.
//! [`engines`] is the in-process comparison: one program, run by the JIT and by the interpreter (also under GC
//! stress) in this process, with the same output, result and uncaught report (category, message, trace sites)
//! required of both; each test then states what the program does.
use super::*;
use crate::budget::Limits;
use crate::program::Loaded;

/// What a run shows: `main`'s result or the report, standard output, and the uncaught failure.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Outcome {
    result: Result<i64, String>,
    output: String,
    uncaught: Option<crate::Uncaught>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(feature = "jit"), allow(dead_code))]
enum Engine {
    Jit,
    Interpreter,
}

/// The machine stack a run may use; its thread has twice as much.
const STACK: u64 = 64 << 20;

/// Run `source`'s entry on `engine`.
fn run(source: &str, engine: Engine, stress: bool, limits: Limits) -> Outcome {
    let source = source.to_owned();
    std::thread::Builder::new()
        .stack_size((STACK as usize) * 2)
        .spawn(move || {
            let program = okkama_sparrow::assemble(&source).unwrap_or_else(|error| panic!("{error}\n{source}"));
            let mut runtime = Box::new(Runtime::new(Loaded::verified(program).unwrap()).unwrap());
            // #1122: `use_engine` loads the interpreter's way too (no pointer-free proofs in frame layouts).
            match engine {
                Engine::Interpreter => runtime.use_engine(crate::Engine::Interpreter),
                #[cfg(feature = "jit")]
                Engine::Jit => runtime.use_engine(crate::Engine::Jit),
                #[cfg(not(feature = "jit"))]
                Engine::Jit => unreachable!("a build without the JIT"),
            }
            runtime.heap.set_stress(stress);
            runtime.limits = limits;
            let (result, output) = okkama_runtime_core::stdout::capture(|| runtime.run_entry(STACK));
            if engine == Engine::Interpreter {
                // One engine per runtime: every entry from Rust went through the seam to the interpreter.
                assert!(runtime.invocations.iter().all(|entry| entry.code == 0), "the interpreter's runtime compiled code");
            }
            Outcome {
                result: result.map_err(|error| error.0),
                output: String::from_utf8(output).unwrap(),
                uncaught: runtime.uncaught.take(),
            }
        })
        .unwrap()
        .join()
        .unwrap()
}

/// The comparison helper: the JIT's outcome, which the interpreter must give too, with and without GC stress.
fn engines_with(source: &str, limits: Limits) -> Outcome {
    // #1124: a build without the JIT holds the interpreter to its own outcome without GC stress.
    #[cfg(feature = "jit")]
    let jit = run(source, Engine::Jit, false, limits.clone());
    #[cfg(not(feature = "jit"))]
    let jit = run(source, Engine::Interpreter, false, limits.clone());
    for stress in [false, true] {
        let interpreted = run(source, Engine::Interpreter, stress, limits.clone());
        assert_eq!(interpreted, jit, "the interpreter (GC stress: {stress}) differs from the JIT\n{source}");
    }
    jit
}

fn engines(source: &str) -> Outcome {
    engines_with(source, Limits::default())
}

/// A program: the prelude's types t0–t4 (int, str, byte, @byte, bool) and functions f0–f3 (`write`, then `print`
/// an int, `say` a string and `flag` a bool as 1 or 0, each on a line), then the test's types, declarations and
/// functions (from t5 and f4).
fn program(types: &str, declarations: &str, functions: &str) -> String {
    format!(
        r#"sparrow 18
module "interp" "1.0.0"
type t0 int
type t1 str
type t2 byte
type t3 array t2
type t4 bool
{types}
{declarations}
function f0 "write" native "runtime/stdio@1::stdout_write/1" {{
    registers [t3]
    params [r0]
    result t0
}}
function f1 "print" {{
    registers [t0, t1, t1, t1, t3]
    params [r0]
    result void
    0: cvt r1, r0
    1: const r2, str "\n"
    2: str.concat r3, r1, r2
    3: str.utf8 r4, r3
    4: call _, f0, [r4]
    5: ret _
}}
function f2 "say" {{
    registers [t1, t1, t1, t3]
    params [r0]
    result void
    0: const r1, str "\n"
    1: str.concat r2, r0, r1
    2: str.utf8 r3, r2
    3: call _, f0, [r3]
    4: ret _
}}
function f3 "flag" {{
    registers [t4, t0]
    params [r0]
    result void
    0: br r0, @1, @3
    1: const r1, int 1
    2: jmp @4
    3: const r1, int 0
    4: call _, f1, [r1]
    5: ret _
}}
{functions}
"#
    )
}

fn lines(lines: &[&str]) -> String {
    lines.iter().map(|line| format!("{line}\n")).collect()
}

/// The sites of an uncaught failure, as (function, pc), innermost first.
fn sites(outcome: &Outcome) -> Vec<(u32, u32)> {
    outcome.uncaught.as_ref().expect("an uncaught failure").trace.iter().map(|site| (site.function, site.pc)).collect()
}

#[test]
fn integers_take_the_inline_path_and_the_exact_one() {
    let outcome = engines(&program("", "", r#"
function f4 "main" {
    registers [t0, t0, t0, t0, t4]
    params []
    result t0
    0: const r0, int 4611686018427387903
    1: const r1, int 1
    2: int.add r2, r0, r1
    3: call _, f1, [r2]
    4: int.sub r3, r2, r1
    5: call _, f1, [r3]
    6: int.mul r2, r0, r0
    7: call _, f1, [r2]
    8: int.cmp gt, r4, r2, r0
    9: call _, f3, [r4]
    10: int.cmp lt, r4, r2, r1
    11: call _, f3, [r4]
    12: const r2, int -7
    13: const r3, int 3
    14: int.mod r3, r2, r3
    15: call _, f1, [r3]
    16: const r3, int -1
    17: int.mod r3, r0, r3
    18: call _, f1, [r3]
    19: int.sub r3, r1, r0
    20: int.sub r3, r3, r0
    21: call _, f1, [r3]
    22: const r3, int 6
    23: const r2, int 7
    24: int.mul r3, r3, r2
    25: int.cmp le, r4, r3, r3
    26: call _, f3, [r4]
    27: int.cmp lt, r4, r1, r0
    28: call _, f3, [r4]
    29: int.cmp gt, r4, r1, r0
    30: call _, f3, [r4]
    31: ret r3
}
entry f4
"#));
    assert_eq!(outcome.result, Ok(42));
    assert_eq!(outcome.output, lines(&[
        "4611686018427387904",
        "4611686018427387903",
        "21267647932558653957237540927630737409",
        "1",
        "0",
        "-1",
        "0",
        "-9223372036854775805",
        "1",
        "1",
        "0",
    ]));
}

#[test]
fn scalars_compare_negate_and_compute() {
    let outcome = engines(&program("type t5 float\ntype t6 char", "", r#"
function f4 "main" {
    registers [t4, t4, t4, t2, t2, t6, t6, t5, t5, t5, t1, t0]
    params []
    result t0
    0: const r0, bool true
    1: bool.not r1, r0
    2: bool.cmp ne, r2, r0, r1
    3: call _, f3, [r2]
    4: call _, f3, [r1]
    5: const r3, byte 200
    6: const r4, byte 7
    7: byte.cmp gt, r2, r3, r4
    8: call _, f3, [r2]
    9: const r5, char 1078
    10: const r6, char 97
    11: char.cmp lt, r2, r6, r5
    12: call _, f3, [r2]
    13: const r7, float 0x3ff8000000000000
    14: const r8, float 0x4004000000000000
    15: float.mul r9, r7, r8
    16: float.sub r9, r9, r8
    17: float.div r9, r9, r7
    18: float.neg r9, r9
    19: float.abs r8, r9
    20: float.add r8, r8, r7
    21: cvt r10, r8
    22: call _, f2, [r10]
    23: float.cmp ge, r2, r8, r7
    24: call _, f3, [r2]
    25: float.cmp eq, r2, r9, r8
    26: call _, f3, [r2]
    27: const r11, int 0
    28: ret r11
}
entry f4
"#));
    assert_eq!(outcome.result, Ok(0));
    assert_eq!(outcome.output, lines(&["1", "0", "1", "1", "2.3333333333333335", "1", "0"]));
}

#[test]
fn objects_successors_and_variants() {
    let types = "type t5 object obj0\ntype t6 object obj1\ntype t7 variant variant0\ntype t8 object obj2\ntype t9 object obj3\ntype t10 role role0";
    let declarations = r#"
role role0 "Named" {
    method "name" [] -> t1
}
object obj0 "Pair" {
    field "left" t0
    field "right" t1
    does role0 [f5]
}
object obj1 "Flag" { field "pending" t4 }
object obj2 "Shape.box" { field "side" t0 }
object obj3 "Shape.dot" { field "x" t0 }
variant variant0 "Shape" { case "box" obj2 case "dot" obj3 }
"#;
    let outcome = engines(&program(types, declarations, r#"
function f4 "main" {
    registers [t0, t1, t5, t0, t5, t1, t4, t6, t4, t7, t7, t10, t1]
    params []
    result t0
    0: const r0, int 5
    1: const r1, str "five"
    2: obj.new r2, obj0, [r0, r1]
    3: obj.get r3, r2, 0
    4: call _, f1, [r3]
    5: const r0, int 6
    6: obj.next r4, r2, [0: r0]
    7: obj.get r3, r4, 0
    8: call _, f1, [r3]
    9: obj.get r5, r4, 1
    10: call _, f2, [r5]
    11: obj.get r3, r2, 0
    12: call _, f1, [r3]
    13: undef r6
    14: obj.new r7, obj1, [r6]
    15: obj.get r8, r7, 0
    16: call _, f3, [r8]
    17: const r6, bool false
    18: obj.init r7, 0, r6
    19: obj.get r8, r7, 0
    20: call _, f3, [r8]
    21: variant.new r9, variant0, 0, [r0]
    22: variant.tag r3, r9
    23: call _, f1, [r3]
    24: variant.get r3, r9, 0, 0
    25: call _, f1, [r3]
    26: variant.new r10, variant0, 1, [r0]
    27: variant.tag r3, r10
    28: call _, f1, [r3]
    29: role.wrap r11, r4, role0
    30: call.role r12, r11, role0, 0, []
    31: call _, f2, [r12]
    32: undef r6
    33: obj.new r7, obj1, [r6]
    34: obj.get r8, r7, 0
    35: bool.not r8, r8
    36: call _, f3, [r8]
    37: ret r3
}
function f5 "Pair.name" {
    registers [t5, t1]
    params [r0]
    result t1
    0: obj.get r1, r0, 1
    1: ret r1
}
entry f4
"#));
    assert_eq!(outcome.result, Ok(1));
    assert_eq!(outcome.output, lines(&["5", "6", "five", "5", "1", "0", "0", "6", "1", "five", "0"]));
}

#[test]
fn arrays_lookups_and_maps() {
    let outcome = engines(&program("type t5 array t0\ntype t6 map t1 t0", "", r#"
function f4 "main" {
    registers [t0, t0, t0, t5, t0, t0, t3, t2, t6, t1, t1, t0]
    params []
    result t0
    0: const r0, int 10
    1: const r1, int 20
    2: const r2, int 30
    3: arr.new r3, [r0, r1, r2]
    4: arr.len r4, r3
    5: call _, f1, [r4]
    6: const r5, int 2
    7: arr.get r4, r3, r5
    8: call _, f1, [r4]
    9: arr.lookup r3, r5, r4, @10, @12
    10: call _, f1, [r4]
    11: const r5, int 7
    12: arr.lookup r3, r5, r4, @13, @15
    13: call _, f1, [r4]
    14: jmp @16
    15: call _, f1, [r5]
    16: const r9, str "hé"
    17: str.utf8 r6, r9
    18: const r5, int 1
    19: arr.get r7, r6, r5
    20: cvt r4, r7
    21: call _, f1, [r4]
    22: const r10, str "b"
    23: map.new r8, [r9: r0, r10: r1]
    24: map.lookup r8, r10, r11, @25, @26
    25: call _, f1, [r11]
    26: map.lookup r8, r9, r11, @27, @28
    27: call _, f1, [r11]
    28: map.len r4, r8
    29: ret r4
}
entry f4
"#));
    assert_eq!(outcome.result, Ok(2));
    assert_eq!(outcome.output, lines(&["3", "30", "30", "7", "195", "20", "10"]));
}

/// Parallel edge moves (a swap), back-edges and a loop's result.
#[test]
fn jumps_move_in_parallel_and_loop() {
    let outcome = engines(&program("", "", r#"
function f4 "main" {
    registers [t0, t0, t0, t0, t4, t0]
    params []
    result t0
    0: const r0, int 1
    1: const r1, int 2
    2: jmp @3(r0 <- r1, r1 <- r0)
    3: call _, f1, [r0]
    4: call _, f1, [r1]
    5: const r2, int 0
    6: const r3, int 0
    7: const r5, int 1
    8: int.cmp lt, r4, r2, r0
    9: br r4, @10, @13
    10: int.add r3, r3, r2
    11: int.add r2, r2, r5
    12: jmp @8
    13: const r5, int 100
    14: int.add r3, r3, r5
    15: ret r3
}
entry f4
"#));
    assert_eq!(outcome.output, lines(&["2", "1"]));
    // 0 + 1 over the loop, then 100.
    assert_eq!(outcome.result, Ok(101));
}

#[test]
fn calls_of_every_kind() {
    let types = "type t5 fn [t0] -> t0\ntype t6 object obj0\ntype t7 cap cap0\ntype t8 role role0\ntype t9 object obj1";
    let declarations = r#"
role role0 "Shape" {
    method "area" [t0] -> t0
}
object obj0 "Square" {
    field "side" t0
    does role0 [f6]
}
object obj1 "Rect" {
    field "w" t0
    field "h" t0
    does role0 [f7]
}
cap cap0 "Side" obj0 { field "side" f8 }
"#;
    let outcome = engines(&program(types, declarations, r#"
function f4 "add3" {
    registers [t0, t0, t0, t0, t0]
    params [r0, r1, r2]
    result t0
    0: int.sub r3, r0, r1
    1: int.add r4, r3, r2
    2: ret r4
}
function f5 "fact" {
    registers [t0, t0, t0, t4, t0]
    params [r0]
    result t0
    0: const r1, int 1
    1: int.cmp le, r3, r0, r1
    2: br r3, @3, @4
    3: ret r1
    4: int.sub r2, r0, r1
    5: call r4, f5, [r2]
    6: int.mul r4, r4, r0
    7: ret r4
}
function f6 "Square.area" {
    registers [t6, t0, t0, t0]
    params [r0, r1]
    result t0
    0: obj.get r2, r0, 0
    1: int.mul r3, r2, r2
    2: int.add r3, r3, r1
    3: ret r3
}
function f7 "Rect.area" {
    registers [t9, t0, t0, t0, t0]
    params [r0, r1]
    result t0
    0: obj.get r2, r0, 0
    1: obj.get r3, r0, 1
    2: int.mul r4, r2, r3
    3: int.add r4, r4, r1
    4: ret r4
}
function f8 "Square.side" {
    registers [t6, t0]
    params [r0]
    result t0
    0: obj.get r1, r0, 0
    1: ret r1
}
function f9 "main" {
    registers [t0, t0, t0, t5, t0, t6, t9, t8, t8, t7, t0]
    params []
    result t0
    0: const r0, int 1
    1: const r1, int 20
    2: fn.closure r3, f4, [r0, r1]
    3: const r2, int 300
    4: call.fn r4, r3, [r2]
    5: call _, f1, [r4]
    6: const r2, int 20
    7: call r4, f5, [r2]
    8: call _, f1, [r4]
    9: const r0, int 3
    10: obj.new r5, obj0, [r0]
    11: obj.new r6, obj1, [r0, r1]
    12: role.wrap r7, r5, role0
    13: role.wrap r8, r6, role0
    14: call.role r4, r7, role0, 0, [r0]
    15: call _, f1, [r4]
    16: call.role r4, r8, role0, 0, [r0]
    17: call _, f1, [r4]
    18: move r9, r5
    19: call.cap r10, r9, cap0, 0
    20: call _, f1, [r10]
    21: ret r10
}
entry f9
"#));
    assert_eq!(outcome.result, Ok(3));
    assert_eq!(outcome.output, lines(&["281", "2432902008176640000", "12", "63", "3"]));
}

/// A closure over a native (the code table's trampoline in the JIT) and one whose captures are all of its
/// parameters.
#[test]
fn closures_of_natives_and_of_full_captures() {
    let outcome = engines(&program("type t5 fn [t3] -> t0\ntype t6 fn [] -> t0", "", r#"
function f4 "seven" {
    registers [t0, t0]
    params [r0]
    result t0
    0: const r1, int 7
    1: int.add r1, r1, r0
    2: ret r1
}
function f5 "main" {
    registers [t5, t1, t3, t0, t6, t0]
    params []
    result t0
    0: fn.closure r0, f0, []
    1: const r1, str "native\n"
    2: str.utf8 r2, r1
    3: call.fn r3, r0, [r2]
    4: const r5, int 35
    5: fn.closure r4, f4, [r5]
    6: call.fn r3, r4, []
    7: ret r3
}
entry f5
"#));
    assert_eq!(outcome.output, "native\n");
    assert_eq!(outcome.result, Ok(42));
}

/// The failure of an instruction in a callee's callee: the same report, and the trace records each activation
/// once, at the instruction the failure reached it.
#[test]
fn failures_reach_the_top_with_their_trace() {
    let failing = |body: &str| program("type t5 float", "", &format!(r#"
function f4 "inner" {{
    registers [t0, t0, t0, t5, t5]
    params [r0]
    result t0
{body}
}}
function f5 "middle" {{
    registers [t0, t0]
    params [r0]
    result t0
    0: call r1, f4, [r0]
    1: ret r1
}}
function f6 "main" {{
    registers [t0, t0]
    params []
    result t0
    0: const r0, int 5
    1: call r1, f5, [r0]
    2: ret r1
}}
entry f6
"#));
    let modulo = engines(&failing("    0: const r1, int 0\n    1: int.mod r2, r0, r1\n    2: ret r2"));
    assert!(modulo.result.as_ref().unwrap_err().contains("Modulo by zero"), "{modulo:?}");
    assert_eq!(sites(&modulo), vec![(4, 1), (5, 0), (6, 1)]);
    let float = engines(&failing("    0: const r3, float 0x7fefffffffffffff\n    1: float.add r4, r3, r3\n    2: ret r0"));
    assert!(float.result.as_ref().unwrap_err().contains("finite"), "{float:?}");
    assert_eq!(sites(&float), vec![(4, 1), (5, 0), (6, 1)]);
    let trap = engines(&failing("    0: trap range, \"no way\""));
    assert!(trap.result.as_ref().unwrap_err().contains("no way"), "{trap:?}");
    let unreachable = engines(&failing("    0: unreachable"));
    assert!(unreachable.result.as_ref().unwrap_err().contains(crate::texts::UNREACHABLE), "{unreachable:?}");
    assert_eq!(sites(&unreachable), vec![(4, 0), (5, 0), (6, 1)]);
}

#[test]
fn checked_reads_fail_with_the_emitter_texts() {
    let types = "type t5 array t0\ntype t6 variant variant0\ntype t7 object obj0\ntype t8 object obj1\ntype t9 object obj2";
    let declarations = r#"
object obj0 "Shape.box" { field "side" t0 }
object obj1 "Shape.dot" { field "x" t0 }
object obj2 "Box" { field "name" t1 }
variant variant0 "Shape" { case "box" obj0 case "dot" obj1 }
"#;
    let index = engines(&program(types, declarations, r#"
function f4 "main" {
    registers [t0, t5, t0]
    params []
    result t0
    0: const r0, int 3
    1: arr.new r1, [r0, r0]
    2: arr.get r2, r1, r0
    3: ret r2
}
entry f4
"#));
    assert!(index.result.as_ref().unwrap_err().contains(crate::texts::ARRAY_INDEX), "{index:?}");
    let case = engines(&program(types, declarations, r#"
function f4 "main" {
    registers [t0, t6, t0]
    params []
    result t0
    0: const r0, int 3
    1: variant.new r1, variant0, 1, [r0]
    2: variant.get r2, r1, 0, 0
    3: ret r2
}
entry f4
"#));
    assert!(case.result.as_ref().unwrap_err().contains(crate::texts::INACTIVE_CASE), "{case:?}");
    let stub = engines(&program(types, declarations, r#"
function f4 "main" {
    registers [t1, t9, t1, t0]
    params []
    result t0
    0: undef r0
    1: obj.new r1, obj2, [r0]
    2: obj.get r2, r1, 0
    3: str.len r3, r2
    4: ret r3
}
entry f4
"#));
    assert!(stub.result.as_ref().unwrap_err().contains("Field `name` of `Box` is read before it is bound"), "{stub:?}");
    assert_eq!(sites(&stub), vec![(4, 2)]);
}

/// A thrown object's report takes its text from its `Error.text`, which the runtime runs through the seam.
#[test]
fn a_thrown_error_is_described_by_its_text_method() {
    let declarations = r#"
role role0 "Error" builtin {
    method "text" [] -> t1
}
object obj0 "Oops" {
    field "code" t0
    does role0 [f4]
}
"#;
    let outcome = engines(&program("type t5 object obj0", declarations, r#"
function f4 "Oops.text" {
    registers [t5, t0, t1, t1, t1]
    params [r0]
    result t1
    0: obj.get r1, r0, 0
    1: cvt r2, r1
    2: const r3, str "oops "
    3: str.concat r4, r3, r2
    4: ret r4
}
function f5 "main" {
    registers [t0, t5]
    params []
    result t0
    0: const r0, int 17
    1: obj.new r1, obj0, [r0]
    2: throw r1
}
entry f5
"#));
    assert!(outcome.result.as_ref().unwrap_err().contains("oops 17"), "{outcome:?}");
    assert_eq!(sites(&outcome), vec![(5, 2)]);
}

/// Leaving a region runs the destroys of what it owns, newest first: the runtime calls each destroy body
/// through the seam.
#[test]
fn a_region_runs_destroys_through_the_seam() {
    let declarations = r#"
object obj0 "Handle" {
    field "name" t1
    destroy f4
}
"#;
    let outcome = engines(&program("type t5 object obj0\ntype t6 region", declarations, r#"
function f4 "Handle.destroy" {
    registers [t5, t1, t1, t1]
    params [r0]
    result void
    0: const r1, str "closing "
    1: obj.get r2, r0, 0
    2: str.concat r3, r1, r2
    3: call _, f2, [r3]
    4: ret _
}
function f5 "main" {
    registers [t6, t1, t5, t0, t1]
    params []
    result t0
    0: region.enter r0
    1: const r1, str "a"
    2: obj.new r2, obj0, [r1]
    3: region.track r0, r2
    4: region.adopt r0, r2
    5: const r4, str "b"
    6: obj.new r2, obj0, [r4]
    7: region.track r0, r2
    8: region.adopt r0, r2
    9: const r3, int 0
    10: region.leave r0, _
    11: ret r3
}
entry f5
"#));
    assert_eq!(outcome.result, Ok(0));
    assert_eq!(outcome.output, lines(&["closing b", "closing a"]));
}

/// The shared helper (`rt_exec`): bitwise operations, text, conversions and maps take the JIT's plain path.
#[test]
fn generic_instructions_run_through_the_shared_helper() {
    let outcome = engines(&program("type t5 map t0 t1\ntype t6 array t1", "", r#"
function f4 "main" {
    registers [t0, t0, t0, t1, t1, t0, t5, t4, t6, t1]
    params []
    result t0
    0: const r0, int 12
    1: const r1, int 10
    2: int.and r2, r0, r1
    3: call _, f1, [r2]
    4: int.shl r2, r0, r1
    5: call _, f1, [r2]
    6: const r3, str "a,bc,d"
    7: str.len r5, r3
    8: call _, f1, [r5]
    9: const r4, str ","
    10: str.split r8, r3, r4
    11: arr.len r5, r8
    12: call _, f1, [r5]
    13: map.new r6, [r0: r3, r1: r4]
    14: map.has r7, r6, r1
    15: call _, f3, [r7]
    16: map.get r9, r6, r0
    17: ret r5
}
entry f4
"#));
    assert_eq!(outcome.result, Ok(3));
    assert_eq!(outcome.output, lines(&["8", "12288", "6", "3", "1"]));
}

/// A step budget stops both engines at the same step (the comparison holds the output, hence the point):
/// function entries and back-edges count, natives do not.
#[test]
fn a_step_budget_stops_at_the_same_step() {
    let source = program("", "", r#"
function f4 "tick" {
    registers [t0, t0]
    params [r0]
    result t0
    0: const r1, int 1
    1: int.add r1, r0, r1
    2: ret r1
}
function f5 "main" {
    registers [t0, t0, t4, t0]
    params []
    result t0
    0: const r0, int 0
    1: const r3, int 1000000
    2: call r0, f4, [r0]
    3: call _, f1, [r0]
    4: int.cmp lt, r2, r0, r3
    5: br r2, @2, @6
    6: ret r0
}
entry f5
"#);
    for (steps, printed) in [(1, 0), (2, 0), (3, 1), (4, 1), (7, 2), (100, 33), (1001, 333)] {
        let outcome = engines_with(&source, Limits { steps: Some(steps), ..Limits::default() });
        assert!(outcome.result.as_ref().unwrap_err().contains("Execution budget exhausted: steps"), "{outcome:?}");
        // main's entry, then per line: tick's entry, print's entry, the back-edge.
        assert_eq!(outcome.output.lines().count(), printed, "{steps} steps");
    }
}

/// Deep recursion: the register stack runs out as the machine stack does, with the same report text (the depth
/// it is reached at differs, a known divergence of the design, §6.3).
#[test]
fn deep_recursion_is_a_stack_overflow() {
    let source = program("", "", r#"
function f4 "down" {
    registers [t0, t0, t0]
    params [r0]
    result t0
    0: const r1, int 1
    1: int.add r1, r0, r1
    2: call r2, f4, [r1]
    3: ret r2
}
function f5 "main" {
    registers [t0, t0]
    params []
    result t0
    0: const r0, int 0
    1: call r1, f4, [r0]
    2: ret r1
}
entry f5
"#);
    #[cfg(feature = "jit")]
    let jit = run(&source, Engine::Jit, false, Limits::default());
    let interpreted = run(&source, Engine::Interpreter, false, Limits::default());
    #[cfg(not(feature = "jit"))]
    let jit = interpreted.clone();
    for outcome in [&jit, &interpreted] {
        assert!(outcome.result.as_ref().unwrap_err().contains("Stack overflow"), "{outcome:?}");
        let sites = sites(outcome);
        // The innermost activation is recorded at its call, the outermost is main's.
        assert_eq!((sites[0], *sites.last().unwrap()), ((4, 2), (5, 1)), "{:?}", &sites[..4]);
    }
    let message = |outcome: &Outcome| outcome.uncaught.as_ref().map(|uncaught| (uncaught.category, uncaught.message.clone()));
    assert_eq!(message(&interpreted), message(&jit));
}

/// #1121: a failure at an instruction a handler covers goes to the handler, with the failure in its register;
/// one outside every range leaves the function.
#[test]
fn a_handler_takes_the_failures_of_its_range() {
    let outcome = engines(&program("type t5 failure", "", r#"
function f4 "main" {
    registers [t0, t0, t0, t5, t0]
    params []
    result t0
    handler 1 3 @6 r3
    0: const r0, int 1
    1: const r1, int 0
    2: int.mod r2, r0, r1
    3: const r4, int 5
    4: int.mod r2, r4, r1
    5: ret r2
    6: const r4, int 9
    7: call _, f1, [r4]
    8: jmp @3(r1 <- r0)
}
entry f4
"#));
    assert_eq!(outcome.output, "9\n");
    assert_eq!(outcome.result, Ok(0));
    // A range ends before its end: the failure at pc 2 of `handler 1 2` is not the handler's.
    let outside = engines(&program("type t5 failure", "", r#"
function f4 "main" {
    registers [t0, t0, t0, t5]
    params []
    result t0
    handler 1 2 @4 r3
    0: const r0, int 1
    1: const r1, int 0
    2: int.mod r2, r0, r1
    3: ret r2
    4: const r2, int 9
    5: ret r2
}
entry f4
"#));
    assert!(outside.result.as_ref().unwrap_err().contains("Modulo by zero"), "{outside:?}");
}

/// #1121: a handler in a caller takes a callee's failure, the callee's activation left; raised again outside every
/// handler, the failure reports each activation once (the caller at the instruction it first reached).
#[test]
fn a_caller_takes_a_callee_failure_and_records_each_activation_once() {
    let outcome = engines(&program("type t5 failure", "", r#"
function f4 "inner" {
    registers [t0, t0, t0]
    params [r0]
    result t0
    0: const r1, int 0
    1: int.mod r2, r0, r1
    2: ret r2
}
function f5 "main" {
    registers [t0, t0, t5]
    params []
    result t0
    handler 1 2 @3 r2
    0: const r0, int 4
    1: call r1, f4, [r0]
    2: ret r1
    3: call _, f1, [r0]
    4: raise r2
}
entry f5
"#));
    assert_eq!(outcome.output, "4\n");
    assert!(outcome.result.as_ref().unwrap_err().contains("Modulo by zero"), "{outcome:?}");
    assert_eq!(sites(&outcome), vec![(4, 1), (5, 1)]);
}

/// #1121: a handler's edge moves read the registers as they were, the failure is written last, and an edge that goes
/// back is a step: a failure caught over and over stops at the same step on both engines.
#[test]
fn a_handler_edge_moves_in_parallel_and_steps_when_it_goes_back() {
    let source = program("type t5 failure", "", r#"
function f4 "main" {
    registers [t0, t0, t0, t5, t0]
    params []
    result t0
    handler 3 4 @1 r3
    0: const r0, int 1
    1: const r1, int 0
    2: call _, f1, [r0]
    3: int.mod r2, r0, r1
    4: ret r2
}
entry f4
"#);
    let swap = engines(&program("type t5 failure", "", r#"
function f4 "fail" {
    registers [t0, t0, t0]
    params [r0, r1]
    result t0
    0: const r2, int 0
    1: int.mod r2, r0, r2
    2: ret r2
}
function f5 "main" {
    registers [t0, t0, t5, t0]
    params []
    result t0
    handler 2 3 @4(r0 <- r1, r1 <- r0) r2
    0: const r0, int 1
    1: const r1, int 2
    2: call r3, f4, [r0, r1]
    3: ret r3
    4: call _, f1, [r0]
    5: call _, f1, [r1]
    6: ret r0
}
entry f5
"#));
    assert_eq!(swap.output, lines(&["2", "1"]));
    for (steps, printed) in [(1, 0), (2, 1), (3, 1), (4, 2), (50, 25)] {
        let outcome = engines_with(&source, Limits { steps: Some(steps), ..Limits::default() });
        assert!(outcome.result.as_ref().unwrap_err().contains("Execution budget exhausted: steps"), "{outcome:?}");
        // main's entry, then per line: print's entry and the handler's edge back to @1.
        assert_eq!(outcome.output.lines().count(), printed, "{steps} steps");
    }
}

/// #1121: the handler register keeps the failure (a heap word) across what the handler does next, a collection
/// at every safepoint included.
#[test]
fn a_caught_failure_survives_collections_in_its_register() {
    let outcome = engines(&program("type t5 failure\ntype t6 object obj0", r#"object obj0 "Cell" { field "text" t1 }"#, r#"
function f4 "main" {
    registers [t0, t0, t5, t1, t6, t0]
    params []
    result t0
    handler 2 3 @4 r2
    0: const r0, int 3
    1: const r1, int 0
    2: int.mod r5, r0, r1
    3: ret r5
    4: const r3, str "after"
    5: obj.new r4, obj0, [r3]
    6: str.concat r3, r3, r3
    7: call _, f2, [r3]
    8: raise r2
}
entry f4
"#));
    assert_eq!(outcome.output, "afterafter\n");
    assert!(outcome.result.as_ref().unwrap_err().contains("Modulo by zero"), "{outcome:?}");
}

/// #1121: the program of every failure path (`tests/mnemonics/failures.spa`): each failure where the JIT takes it,
/// the cleanups on the failure path, and the uncaught report with its secondary.
#[test]
fn the_failure_paths_take_each_failure_where_the_jit_does() {
    let outcome = engines(include_str!("../tests/mnemonics/failures.spa"));
    assert_eq!(outcome.output, lines(&[
        "7",
        "oops",
        "0",
        "oops",
        "overflow taken",
        "closing b",
        "closing a",
        "region unwound",
        "closing pinned",
        "resource unwound",
        "narrow refused",
        "trap taken",
        "closing kept",
        "3",
    ]));
    let uncaught = outcome.uncaught.as_ref().expect("an uncaught failure");
    assert_eq!(uncaught.message, crate::texts::UNREACHABLE);
    assert!(outcome.result.as_ref().unwrap_err().contains("Secondary cleanup failure"), "{outcome:?}");
}

/// Every mnemonic runs, on both engines alike, in the Sparrow assembly of the repository: the SparrowBook's
/// examples, sparrow-core's and sparrow-cli's `.spa` and `tests/mnemonics` (#1121; async ones since #1122). A new
/// instruction needs a program here too.
#[test]
fn every_mnemonic_runs_on_both_engines() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let directories = [
        root.join("../../../docs/SparrowBook/examples"),
        root.join("tests"),
        root.join("tests/mnemonics"),
        root.join("../sparrow-cli/tests/handlers"),
    ];
    let mut covered = std::collections::BTreeSet::new();
    let mut programs = 0;
    for directory in directories {
        let mut files: Vec<_> = std::fs::read_dir(&directory).unwrap().map(|entry| entry.unwrap().path()).collect();
        files.sort();
        for path in files.into_iter().filter(|path| path.extension().is_some_and(|extension| extension == "spa")) {
            let source = std::fs::read_to_string(&path).unwrap();
            // A fixture its own test completes (a template with `$` holes, a part of a program) is not a program.
            let Ok(program) = okkama_sparrow::assemble(&source) else { continue };
            if program.entry.is_none() {
                continue;
            }
            engines(&source);
            covered.extend(run_covering(&source));
            programs += 1;
        }
    }
    let missing: Vec<_> =
        okkama_sparrow::Instruction::MNEMONICS.iter().filter(|m| !covered.contains(*m)).collect();
    assert!(missing.is_empty(), "{programs} programs leave {} mnemonics unexecuted: {missing:?}", missing.len());
}

thread_local! {
    /// The mnemonics the interpreter executed on this thread (test builds only).
    static EXECUTED: std::cell::RefCell<std::collections::BTreeSet<&'static str>> = const { std::cell::RefCell::new(std::collections::BTreeSet::new()) };
}

/// The interpreter's loop reports each instruction it starts (test builds only).
pub(super) fn executed(mnemonic: &'static str) {
    EXECUTED.with(|executed| executed.borrow_mut().insert(mnemonic));
}

/// The mnemonics an interpreted run of `source` executes.
fn run_covering(source: &str) -> std::collections::BTreeSet<&'static str> {
    let source = source.to_owned();
    std::thread::Builder::new()
        .stack_size((STACK as usize) * 2)
        .spawn(move || {
            let program = okkama_sparrow::assemble(&source).unwrap();
            let mut runtime = Box::new(Runtime::new(Loaded::verified(program).unwrap()).unwrap());
            runtime.use_engine(crate::Engine::Interpreter);
            EXECUTED.with(|executed| executed.borrow_mut().clear());
            let _ = okkama_runtime_core::stdout::capture(|| runtime.run_entry(STACK));
            EXECUTED.with(|executed| executed.take())
        })
        .unwrap()
        .join()
        .unwrap()
}

/// #1122: async bodies on both engines (`tests/mnemonics/async.spa`): awaits, a spawned task, async destroys
/// settled by a region and a static resource, one suspended until another task opens its gate, and a failure that
/// unwinds through a suspended `region.settle_unwind`: the body is in the trace once, at the call the failure first
/// reached, though it resumed at another stack depth (design #1079 §6.4: the JIT recorded it twice, by frame
/// pointer, until it keyed async bodies by their heap frame).
#[test]
fn async_bodies_suspend_resume_and_settle_alike() {
    let outcome = engines(include_str!("../tests/mnemonics/async.spa"));
    assert_eq!(outcome.output, lines(&["25", "36", "gate closed", "settled", "gate closed", "gate closed", "1", "gate closed"]));
    assert!(outcome.result.as_ref().unwrap_err().contains("oops"), "{outcome:?}");
    assert_eq!(sites(&outcome), vec![(11, 1), (13, 11), (14, 26)]);
    // Every resumption is a step, as every entry of the JIT's body: a budget stops both at one point.
    for steps in [3, 12, 40, 400] {
        engines_with(include_str!("../tests/mnemonics/async.spa"), Limits { steps: Some(steps), ..Limits::default() });
    }
    // `await.take` consumes the Future: a second await of it is a panic.
    let taken = engines(&program("type t5 future t0", "", r#"
function f4 "square" async {
    registers [t0, t0]
    params [r0]
    result t0
    0: int.mul r1, r0, r0
    1: ret r1
}
function f5 "main" async {
    registers [t0, t5, t0, t0]
    params []
    result t0
    0: const r0, int 7
    1: call r1, f4, [r0]
    2: await.take r2, r1
    3: call _, f1, [r2]
    4: await r3, r1
    5: ret r3
}
entry f5
"#));
    assert_eq!(taken.output, "49\n");
    assert!(taken.result.as_ref().unwrap_err().contains("await of a consumed Future"), "{taken:?}");
    let simple = program("", "", "function f4 \"main\" async {\n registers [t0]\n params []\n result t0\n 0: const r0, int 3\n 1: ret r0\n}\nentry f4\n");
    assert_eq!(engines(&simple).result, Ok(3));
}

/// The collector reads every frame's registers: a chain of objects held only by the registers of callers below
/// survives a collection at every safepoint, and its fields read back.
#[test]
fn frames_root_their_registers_under_gc_stress() {
    let outcome = engines(&program("type t5 object obj0", r#"object obj0 "Cell" { field "text" t1 field "next" t5 }"#, r#"
function f4 "build" {
    registers [t0, t5, t1, t5, t0, t4, t0]
    params [r0, r1]
    result t5
    0: const r4, int 0
    1: int.cmp eq, r5, r0, r4
    2: br r5, @3, @4
    3: ret r1
    4: cvt r2, r0
    5: obj.new r3, obj0, [r2, r1]
    6: const r6, int 1
    7: int.sub r6, r0, r6
    8: call r3, f4, [r6, r3]
    9: ret r3
}
function f5 "main" {
    registers [t0, t5, t5, t1, t1, t0, t5]
    params []
    result t0
    0: const r3, str "end"
    1: undef r6
    2: obj.new r1, obj0, [r3, r6]
    3: const r0, int 40
    4: call r2, f4, [r0, r1]
    5: obj.get r4, r2, 0
    6: call _, f2, [r4]
    7: obj.get r2, r2, 1
    8: obj.get r4, r2, 0
    9: call _, f2, [r4]
    10: str.len r5, r4
    11: ret r5
}
entry f5
"#));
    assert_eq!(outcome.output, lines(&["1", "2"]));
    assert_eq!(outcome.result, Ok(1));
}

#[test]
fn the_register_stack_keeps_frames_in_place_and_refuses_past_its_limit() {
    let mut stack = Stack { limit: 8 * (CHUNK as u64 * 3), ..Stack::default() };
    let (first, at_first) = stack.push(10).unwrap();
    let (second, _) = stack.push(CHUNK - 10).unwrap();
    assert_eq!(second, first.wrapping_add(10), "frames follow each other in a chunk");
    let (third, at_third) = stack.push(CHUNK + 5).unwrap();
    assert!(third != first && third != second, "a frame bigger than a chunk gets a chunk of its own");
    assert!(stack.push(CHUNK).is_none(), "past the limit");
    stack.pop(at_third, (CHUNK + 5) as u32);
    let (again, _) = stack.push(4).unwrap();
    assert_eq!(again, third, "the chunk above is reused");
    stack.pop(at_first, 0);
    assert_eq!((stack.chunk, stack.top), (0, 0));
}

/// The Rust stack bounds the runtime's own re-entries: an entry below the context's limit is a stack overflow.
#[test]
fn an_entry_past_the_rust_stack_limit_is_a_stack_overflow() {
    let source = program("", "", "function f4 \"main\" {\n registers [t0]\n params []\n result t0\n 0: const r0, int 1\n 1: ret r0\n}\nentry f4\n");
    let program = okkama_sparrow::assemble(&source).unwrap();
    let mut runtime = Box::new(Runtime::new(Loaded::verified(program).unwrap()).unwrap());
    runtime.interp = Some(Box::new(Interp::new(runtime.loaded.program.functions.len())));
    let mut ctx = Ctx { status: OK, stack_limit: u64::MAX, failure: word::UNBOUND, runtime: &mut *runtime, entry_frame: 0, scratch: word::UNBOUND };
    assert_eq!(invoke(&mut ctx, 4, &[]), 0);
    assert_eq!(ctx.status, FAILED);
    let failure = runtime.failure(ctx.failure).expect("a failure");
    assert!(matches!(&failure.kind, FailureKind::Program(message) if message == "Stack overflow"), "{failure:?}");
    let mut ctx = Ctx { status: OK, stack_limit: 0, failure: word::UNBOUND, runtime: &mut *runtime, entry_frame: 0, scratch: word::UNBOUND };
    assert_eq!(invoke(&mut ctx, 4, &[]), word::small(1).unwrap());
    assert_eq!(ctx.status, OK);
}

/// Two evaluations on one runtime (a session's cells), the second under other budgets: its outcome.
/// It compares the engines, so it needs the JIT (#1124).
#[cfg(feature = "jit")]
fn evaluate_twice(source: &str, engine: Engine, first: Limits, second: Limits) -> (Outcome, Outcome) {
    let source = source.to_owned();
    std::thread::Builder::new()
        .stack_size((STACK as usize) * 2)
        .spawn(move || {
            let program = okkama_sparrow::assemble(&source).unwrap();
            let mut runtime = Box::new(Runtime::new(Loaded::verified(program).unwrap()).unwrap());
            runtime.use_engine(match engine {
                Engine::Jit => crate::Engine::Jit,
                Engine::Interpreter => crate::Engine::Interpreter,
            });
            let mut evaluate = |limits: Limits| {
                runtime.limits = limits;
                let (result, output) = okkama_runtime_core::stdout::capture(|| runtime.run_entry(STACK));
                Outcome { result: result.map_err(|error| error.0), output: String::from_utf8(output).unwrap(), uncaught: runtime.uncaught.take() }
            };
            let first = evaluate(first);
            (first, evaluate(second))
        })
        .unwrap()
        .join()
        .unwrap()
}

/// #1129: whether code polls the fuel cell is fixed at a runtime's first run, for the JIT (compiled into each
/// function) and the interpreter (read at each step) alike. A later run with other step budgets polls as the first
/// did; a run that would turn polling on or off is refused, with one text on both engines.
#[cfg(feature = "jit")]
#[test]
fn the_polling_of_a_runtime_is_fixed_at_its_first_run_on_both_engines() {
    let source = program("", "", r#"
function f4 "main" {
    registers [t0, t0, t4, t0]
    params []
    result t0
    0: const r0, int 0
    1: const r3, int 300
    2: const r1, int 1
    3: int.add r0, r0, r1
    4: call _, f1, [r0]
    5: int.cmp lt, r2, r0, r3
    6: br r2, @2, @7
    7: ret r0
}
entry f4
"#);
    let steps = |n| Limits { steps: Some(n), ..Limits::default() };
    for (first, second) in [(Limits::default(), steps(100)), (steps(100_000), steps(100)), (steps(100), Limits::default())] {
        let jit = evaluate_twice(&source, Engine::Jit, first.clone(), second.clone());
        let interpreted = evaluate_twice(&source, Engine::Interpreter, first.clone(), second.clone());
        assert_eq!(interpreted, jit, "budgets {:?} then {:?}", first.steps, second.steps);
        match (first.polled(), second.polled()) {
            (a, b) if a != b => {
                let error = jit.1.result.as_ref().unwrap_err();
                assert!(error.contains("set the budgets before the first run"), "{error}");
                assert!(jit.1.output.is_empty(), "a refused run runs nothing");
            }
            _ => {
                // The second run stops on its own budget: main's entry, then a print and a back-edge per line.
                assert!(jit.1.result.as_ref().unwrap_err().contains("Execution budget exhausted: steps"), "{jit:?}");
                assert_eq!(jit.1.output.lines().count(), 50);
            }
        }
    }
}

/// #1122: entries from Rust nest only so deep (`REENTRIES`, 200 on wasm32, where the Rust stack's overflow is a trap):
/// the entry past the limit is a stack overflow, as the machine stack's limit is for the JIT. Here a region's
/// destroy, which the runtime calls from Rust, is the second entry.
#[test]
fn entries_from_rust_nest_only_so_deep() {
    let source = program("type t5 object obj0\ntype t6 region", "object obj0 \"Handle\" {\n    field \"name\" t1\n    destroy f4\n}", r#"
function f4 "Handle.destroy" {
    registers [t5, t1]
    params [r0]
    result void
    0: obj.get r1, r0, 0
    1: call _, f2, [r1]
    2: ret _
}
function f5 "main" {
    registers [t6, t1, t5, t0]
    params []
    result t0
    0: region.enter r0
    1: const r1, str "closed"
    2: obj.new r2, obj0, [r1]
    3: region.track r0, r2
    4: region.adopt r0, r2
    5: region.leave r0, _
    6: const r3, int 0
    7: ret r3
}
entry f5
"#);
    let run_with = |reentries: u32| {
        let source = source.clone();
        std::thread::spawn(move || {
            let program = okkama_sparrow::assemble(&source).unwrap();
            let mut runtime = Box::new(Runtime::new(Loaded::verified(program).unwrap()).unwrap());
            runtime.use_engine(crate::Engine::Interpreter);
            runtime.interp.as_mut().unwrap().reentries = reentries;
            let (result, output) = okkama_runtime_core::stdout::capture(|| runtime.run_entry(STACK));
            (result.map_err(|error| error.0), String::from_utf8(output).unwrap())
        })
        .join()
        .unwrap()
    };
    assert_eq!(run_with(2), (Ok(0), "closed\n".to_owned()));
    let (result, output) = run_with(1);
    assert!(result.as_ref().unwrap_err().contains("Stack overflow"), "{result:?}");
    assert_eq!(output, "");
}

/// #1122: the interpreter's runtime proves no register pointer-free, so a suspended frame traces every register of
/// a traced kind (design #1079 §2.4: the oracle does not rest on the analysis it checks); the JIT's keeps the proofs.
#[test]
fn the_interpreter_loads_without_pointer_free_proofs() {
    let source = "sparrow 18\nmodule \"ranges\" \"1\"\ntype t0 int\nfunction f0 \"rest\" async {\n registers [t0, t0, t0]\n \
        params [r0]\n result t0\n 0: const r1, int 3\n 1: int.mod r2, r0, r1\n 2: ret r2\n}\n";
    let program = okkama_sparrow::assemble(source).unwrap();
    let mut runtime = Box::new(Runtime::new(Loaded::verified(program).unwrap()).unwrap());
    if std::env::var_os("SPARROW_CORE_POINTER_FREE").is_none() && runtime.interp.is_none() {
        assert!(runtime.loaded.functions[0].pointer_free.contains(&true), "the JIT's load proves r2 small");
    }
    runtime.use_engine(crate::Engine::Interpreter);
    assert!(runtime.loaded.functions[0].pointer_free.iter().all(|free| !free));
    // The frame's registers follow the header's seven fixed words; every int register is traced.
    let frame = &runtime.heap.layouts().frames[0];
    assert!(frame[crate::tasks::FIXED_WORDS..crate::tasks::FIXED_WORDS + 3].iter().all(|repr| *repr == Repr::Int), "{frame:?}");
}

/// #1167: array literals up to 16 elements are allocated inline by the JIT, longer ones by the helper; both must
/// give what the interpreter's heap builds, for int (a BigInt among them), string and byte elements, sizes around
/// the packing and inline limits, with and without GC stress on the JIT too.
#[test]
fn inline_array_literals_match_the_interpreter() {
    for (element, ty) in [("int", "t0"), ("str", "t1"), ("byte", "t2")] {
        for size in [0usize, 1, 7, 8, 9, 15, 16, 17] {
            let array_type = if element == "byte" { "t3".to_string() } else { "t5".to_string() };
            let types = if element == "byte" { String::new() } else { format!("type t5 array {ty}") };
            // Registers: elements r0..size-1, the array, its length, an index, an element, a scratch int.
            let mut registers: Vec<String> = (0..size).map(|_| ty.to_string()).collect();
            let (array, length, index, item, scratch) = (size, size + 1, size + 2, size + 3, size + 4);
            registers.extend([array_type.clone(), "t0".into(), "t0".into(), ty.into(), "t0".into()]);
            let mut code = Vec::new();
            for i in 0..size {
                code.push(match element {
                    "int" if i == 3 => format!("const r{i}, int 1234567890123456789012345"),
                    "int" => format!("const r{i}, int {}", i as i64 * 7 - 20),
                    "str" => format!("const r{i}, str \"s{i}\""),
                    _ => format!("const r{scratch}, int {}\n    byte.from r{i}, r{scratch}, _", (i * 37 + 5) % 256),
                });
            }
            let list: Vec<String> = (0..size).map(|i| format!("r{i}")).collect();
            code.push(format!("arr.new r{array}, [{}]", list.join(", ")));
            code.push(format!("arr.len r{length}, r{array}"));
            code.push(format!("call _, f1, [r{length}]"));
            for i in 0..size {
                code.push(format!("const r{index}, int {i}"));
                code.push(format!("arr.get r{item}, r{array}, r{index}"));
                code.push(match element {
                    "int" => format!("call _, f1, [r{item}]"),
                    "str" => format!("call _, f2, [r{item}]"),
                    _ => format!("cvt r{scratch}, r{item}\n    call _, f1, [r{scratch}]"),
                });
            }
            code.push(format!("ret r{length}"));
            let body: String = code.join("\n").lines().enumerate().map(|(pc, line)| format!("    {pc}: {}\n", line.trim())).collect();
            let source = program(&types, "", &format!(
                "function f4 \"main\" {{\n    registers [{}]\n    params []\n    result t0\n{body}}}\nentry f4\n",
                registers.join(", ")
            ));
            let outcome = engines(&source);
            assert_eq!(outcome.result, Ok(size as i64), "{element} × {size}: {outcome:?}");
            assert_eq!(outcome.output.lines().count(), size + 1, "{element} × {size}");
            #[cfg(feature = "jit")]
            assert_eq!(run(&source, Engine::Jit, true, Limits::default()), outcome, "{element} × {size} under GC stress");
        }
    }
}

/// #1167: closures are allocated inline by the JIT: captures fill a prefix of the function's layout and the rest
/// stays unbound until the call supplies it; a closure built while collections run keeps its captures.
#[test]
fn inline_closures_keep_their_captures_under_collection() {
    let source = program("type t5 fn [t0, t1] -> t0\ntype t6 fn [t1] -> t0\ntype t7 fn [] -> t0\ntype t8 array t1", "", r#"
function f4 "mix" {
    registers [t0, t0, t0, t1, t0]
    params [r0, r1, r2, r3]
    result t0
    0: int.mul r4, r0, r1
    1: int.add r4, r4, r2
    2: str.len r0, r3
    3: int.add r4, r4, r0
    4: ret r4
}
function f5 "main" {
    registers [t0, t0, t0, t1, t5, t6, t7, t0, t8, t0]
    params []
    result t0
    0: const r0, int 1000000000000000000000
    1: const r1, int 3
    2: const r3, str "four"
    3: fn.closure r4, f4, [r0, r1]
    4: fn.closure r5, f4, [r1, r1, r0]
    5: fn.closure r6, f4, [r1, r0, r1, r3]
    6: arr.new r8, [r3, r3, r3]
    7: const r2, int 5
    8: call.fn r7, r4, [r2, r3]
    9: call _, f1, [r7]
    10: call.fn r7, r5, [r3]
    11: call _, f1, [r7]
    12: call.fn r7, r6, []
    13: call _, f1, [r7]
    14: ret r1
}
entry f5
"#);
    let outcome = engines(&source);
    assert_eq!(outcome.output, lines(&["3000000000000000000009", "1000000000000000000013", "3000000000000000000007"]));
    #[cfg(feature = "jit")]
    assert_eq!(run(&source, Engine::Jit, true, Limits::default()), outcome);
}
