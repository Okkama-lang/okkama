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
            runtime.interp = match engine {
                Engine::Jit => None,
                Engine::Interpreter => Some(Box::new(Interp::new(runtime.loaded.program.functions.len()))),
            };
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
    let jit = run(source, Engine::Jit, false, limits.clone());
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
    let jit = run(&source, Engine::Jit, false, Limits::default());
    let interpreted = run(&source, Engine::Interpreter, false, Limits::default());
    for outcome in [&jit, &interpreted] {
        assert!(outcome.result.as_ref().unwrap_err().contains("Stack overflow"), "{outcome:?}");
        let sites = sites(outcome);
        // The innermost activation is recorded at its call, the outermost is main's.
        assert_eq!((sites[0], *sites.last().unwrap()), ((4, 2), (5, 1)), "{:?}", &sites[..4]);
    }
    let message = |outcome: &Outcome| outcome.uncaught.as_ref().map(|uncaught| (uncaught.category, uncaught.message.clone()));
    assert_eq!(message(&interpreted), message(&jit));
}

#[test]
fn a_failure_a_handler_would_catch_is_refused() {
    let source = program("type t5 failure", "", r#"
function f4 "main" {
    registers [t0, t0, t0, t5]
    params []
    result t0
    handler 1 3 @4 r3
    0: const r0, int 1
    1: const r1, int 0
    2: int.mod r2, r0, r1
    3: ret r2
    4: const r2, int 9
    5: ret r2
}
entry f4
"#);
    let error = run(&source, Engine::Interpreter, false, Limits::default()).result.unwrap_err();
    assert!(error.contains("main:2: a failure handler is not supported by the interpreter yet (#1121)"), "{error}");
    // The JIT catches it.
    assert_eq!(run(&source, Engine::Jit, false, Limits::default()).result, Ok(9));
}

#[test]
fn an_async_body_is_refused() {
    let source = program("", "", r#"
function f4 "main" async {
    registers [t0]
    params []
    result t0
    0: const r0, int 3
    1: ret r0
}
entry f4
"#);
    let error = run(&source, Engine::Interpreter, false, Limits::default()).result.unwrap_err();
    assert!(error.contains("main: an async body is not supported by the interpreter yet (#1122)"), "{error}");
    assert_eq!(run(&source, Engine::Jit, false, Limits::default()).result, Ok(3));
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
