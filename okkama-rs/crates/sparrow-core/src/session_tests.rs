//! #588 phase A: a session runtime grows by `Runtime::extend` within fixed capacities. Compiled code keeps
//! working, old dynamic calls reach new implementations, heap values survive, and a rejected extension changes
//! nothing.
use crate::program::Loaded;
use crate::runtime::{Capacities, Runtime};
use crate::word::{Repr, Word};

/// A program of `parts`: (types, objects, roles, functions) texts, joined in order.
fn program(parts: &[(&str, &str, &str, &str)]) -> okkama_sparrow::Program {
    let mut text = String::from("sparrow 19\nmodule \"session\" \"1.0.0\"\n");
    for section in 0..4 {
        for part in parts {
            text.push_str([part.0, part.1, part.2, part.3][section]);
        }
    }
    okkama_sparrow::assemble(&text).unwrap()
}

const BASE: (&str, &str, &str, &str) = (
    "type t0 int\ntype t1 role role0\ntype t2 object obj0\ntype t3 fn [t0] -> t0\n",
    "object obj0 \"Seven\" {\n does role0 [f0]\n}\n",
    "role role0 \"Shape\" {\n method \"area\" [] -> t0\n}\n",
    "function f0 \"Seven.area\" {\n registers [t2, t0]\n params [r0]\n result t0\n 0: const r1, int 7\n 1: ret r1\n}\n\
     function f1 \"old role call\" {\n registers [t1, t0]\n params [r0]\n result t0\n 0: call.role r1, r0, role0, 0, []\n 1: ret r1\n}\n\
     function f2 \"old closure call\" {\n registers [t3, t0, t0]\n params [r0]\n result t0\n 0: const r1, int 5\n 1: call.fn r2, r0, [r1]\n 2: ret r2\n}\n\
     function f3 \"forty-one\" {\n registers [t0]\n params []\n result t0\n 0: const r0, int 41\n 1: ret r0\n}\n\
     function f4 \"make seven\" {\n registers [t2, t1]\n params []\n result t1\n 0: obj.new r0, obj0, []\n 1: role.wrap r1, r0, role0\n 2: ret r1\n}\n\
     function f5 \"double\" {\n registers [t0, t0]\n params [r0]\n result t0\n 0: int.add r1, r0, r0\n 1: ret r1\n}\n\
     function f6 \"closure of double\" {\n registers [t3]\n params []\n result t3\n 0: fn.closure r0, f5, []\n 1: ret r0\n}\n",
);

const SECOND: (&str, &str, &str, &str) = (
    "type t4 object obj1\n",
    "object obj1 \"Nine\" {\n does role0 [f7]\n}\n",
    "",
    "function f7 \"Nine.area\" {\n registers [t4, t0]\n params [r0]\n result t0\n 0: const r1, int 9\n 1: ret r1\n}\n\
     function f8 \"make nine\" {\n registers [t4, t1]\n params []\n result t1\n 0: obj.new r0, obj1, []\n 1: role.wrap r1, r0, role0\n 2: ret r1\n}\n\
     function f9 \"triple\" {\n registers [t0, t0, t0]\n params [r0]\n result t0\n 0: const r1, int 3\n 1: int.mul r2, r0, r1\n 2: ret r2\n}\n\
     function f10 \"closure of triple\" {\n registers [t3]\n params []\n result t3\n 0: fn.closure r0, f9, []\n 1: ret r0\n}\n\
     function f11 \"new calls old\" {\n registers [t0, t0, t0]\n params []\n result t0\n 0: call r0, f3, []\n 1: const r1, int 1\n 2: int.add r2, r0, r1\n 3: ret r2\n}\n",
);

const THIRD: (&str, &str, &str, &str) = (
    "",
    "",
    "",
    "function f12 \"third uses both\" {\n registers [t1, t0]\n params []\n result t0\n 0: call r0, f8, []\n 1: call r1, f1, [r0]\n 2: ret r1\n}\n",
);

const CAPACITIES: Capacities = Capacities { functions: 64, objects: 16 };

fn session(parts: &[(&str, &str, &str, &str)], capacities: Capacities) -> Box<Runtime> {
    let mut runtime = Box::new(Runtime::session(Loaded::new(program(parts)), capacities).unwrap());
    runtime.heap.set_stress(true);
    runtime
}

fn int(runtime: &Runtime, word: Word) -> i64 {
    match runtime.heap.integer(word) {
        crate::heap::Integer::Small(n) => n,
        crate::heap::Integer::Big(n) => panic!("unexpected big integer {n}"),
    }
}

fn call(runtime: &mut Box<Runtime>, function: usize, arguments: &[Word]) -> Word {
    runtime.run_function_with(function, arguments, crate::STACK_BYTES).unwrap()
}

/// Call `function` and read its integer result.
fn number(runtime: &mut Box<Runtime>, function: usize, arguments: &[Word]) -> i64 {
    let word = call(runtime, function, arguments);
    int(runtime, word)
}

/// A value kept across extensions, rooted as a session variable will be.
fn keep(runtime: &mut Box<Runtime>, function: usize) -> Word {
    let value = call(runtime, function, &[]);
    runtime.roots.push((value, Repr::Ref));
    value
}

#[test]
fn compiled_code_and_old_values_survive_extensions_and_reach_new_implementations() {
    let mut runtime = session(&[BASE], CAPACITIES);
    let seven = keep(&mut runtime, 4);
    let double = keep(&mut runtime, 6);
    assert_eq!(number(&mut runtime, 1, &[seven]), 7);
    assert_eq!(number(&mut runtime, 2, &[double]), 10);
    let compiled = runtime.invocations[1].code;
    #[cfg(feature = "jit")]
    assert_ne!(compiled, 0);
    // #1124: without the JIT the interpreter runs it and publishes no code.
    #[cfg(not(feature = "jit"))]
    assert_eq!(compiled, 0);
    let table = runtime.invocations.as_ptr();

    runtime.extend(program(&[BASE, SECOND])).unwrap();
    assert_eq!(runtime.invocations.as_ptr(), table, "the invocation table never moves");
    assert_eq!(runtime.invocations[1].code, compiled, "compiled code is kept");
    // Old compiled code, an old value, and new implementations behind old dynamic calls.
    assert_eq!(number(&mut runtime, 1, &[seven]), 7);
    let nine = keep(&mut runtime, 8);
    assert_eq!(number(&mut runtime, 1, &[nine]), 9, "an old role call reaches a new object");
    let triple = keep(&mut runtime, 10);
    assert_eq!(number(&mut runtime, 2, &[triple]), 15, "an old closure call reaches a new function");
    assert_eq!(number(&mut runtime, 11, &[]), 42, "a new function calls an old one");

    runtime.extend(program(&[BASE, SECOND, THIRD])).unwrap();
    assert_eq!(number(&mut runtime, 12, &[]), 9, "a second extension");
    assert_eq!(number(&mut runtime, 1, &[seven]), 7);
    assert_eq!(number(&mut runtime, 2, &[double]), 10);
}

/// Whatever the reason, a rejected extension leaves the runtime as it was, and it keeps running.
fn rejected(runtime: &mut Box<Runtime>, program: okkama_sparrow::Program, reason: &str) {
    let functions = runtime.loaded.program.functions.len();
    let layouts = runtime.heap.layouts().objects.len();
    let error = runtime.extend(program).expect_err("rejected");
    assert!(error.0.contains(reason), "{}", error.0);
    assert_eq!(runtime.loaded.program.functions.len(), functions);
    assert_eq!(runtime.heap.layouts().objects.len(), layouts);
    assert_eq!(number(runtime, 3, &[]), 41);
}

#[test]
fn a_changed_prefix_or_capacity_overflow_is_rejected_without_change() {
    let mut runtime = session(&[BASE], CAPACITIES);
    assert_eq!(number(&mut runtime, 3, &[]), 41);
    let changed = (BASE.0, BASE.1, BASE.2, &*BASE.3.replace("const r0, int 41", "const r0, int 40"));
    rejected(&mut runtime, program(&[changed, SECOND]), "published functions");
    let small = Capacities { functions: 9, objects: 16 };
    let mut tight = session(&[BASE], small);
    rejected(&mut tight, program(&[BASE, SECOND]), "at most 9 functions");
    let mut few = session(&[BASE], Capacities { functions: 64, objects: 1 });
    rejected(&mut few, program(&[BASE, SECOND]), "at most 1 object types");
    // After the rejections, a valid extension still works.
    runtime.extend(program(&[BASE, SECOND])).unwrap();
    assert_eq!(number(&mut runtime, 11, &[]), 42);
}

#[test]
fn an_extension_that_changes_ownership_metadata_is_rejected() {
    // Turning resources on: frames and compiled code were made without them.
    let lease = (
        "type t4 object obj1\n",
        "object obj1 \"Lease\" {\n destroy f7\n}\n",
        "",
        "function f7 \"Lease.destroy\" {\n registers [t4]\n params [r0]\n result void\n 0: ret _\n}\n",
    );
    let mut runtime = session(&[BASE], CAPACITIES);
    rejected(&mut runtime, program(&[BASE, lease]), "ownership metadata");
    // With resources on from the start, a new implementer with a destroy makes an existing holder of the role
    // reach a destroy: rejected; a new resource type alone is accepted.
    let holder = (
        "type t4 object obj1\ntype t5 object obj2\n",
        "object obj1 \"Lease\" {\n destroy f7\n}\nobject obj2 \"Holder\" {\n field \"shape\" t1\n}\n",
        "",
        "function f7 \"Lease.destroy\" {\n registers [t4]\n params [r0]\n result void\n 0: ret _\n}\n",
    );
    let mut runtime = session(&[BASE, holder], CAPACITIES);
    let bad = (
        "type t6 object obj3\n",
        "object obj3 \"Owning\" {\n does role0 [f8]\n destroy f9\n}\n",
        "",
        "function f8 \"Owning.area\" {\n registers [t6, t0]\n params [r0]\n result t0\n 0: const r1, int 1\n 1: ret r1\n}\n\
         function f9 \"Owning.destroy\" {\n registers [t6]\n params [r0]\n result void\n 0: ret _\n}\n",
    );
    rejected(&mut runtime, program(&[BASE, holder, bad]), "ownership metadata");
    let fine = (
        "type t6 object obj3\n",
        "object obj3 \"Other\" {\n destroy f8\n}\n",
        "",
        "function f8 \"Other.destroy\" {\n registers [t6]\n params [r0]\n result void\n 0: ret _\n}\n",
    );
    runtime.extend(program(&[BASE, holder, fine])).unwrap();
}

#[test]
fn only_a_session_runtime_can_be_extended() {
    let mut runtime = Box::new(Runtime::new(Loaded::new(program(&[BASE]))).unwrap());
    assert!(runtime.extend(program(&[BASE, SECOND])).unwrap_err().0.contains("only a session runtime"));
}

/// #1122: a session on the interpreter loads its extensions as it loaded itself (without the pointer-free proofs),
/// so they extend it as they extend the JIT's; a changed prefix is still refused.
#[test]
fn a_session_on_the_interpreter_extends_as_on_the_jit() {
    let mut runtime = session(&[BASE], CAPACITIES);
    runtime.use_engine(crate::Engine::Interpreter);
    let seven = keep(&mut runtime, 4);
    runtime.extend(program(&[BASE, SECOND])).unwrap();
    assert_eq!(number(&mut runtime, 1, &[seven]), 7);
    assert_eq!(number(&mut runtime, 11, &[]), 42, "a new function calls an old one");
    runtime.extend(program(&[BASE, SECOND, THIRD])).unwrap();
    assert_eq!(number(&mut runtime, 12, &[]), 9);
    let changed = (BASE.0, BASE.1, BASE.2, &*BASE.3.replace("const r0, int 41", "const r0, int 40"));
    rejected(&mut runtime, program(&[changed, SECOND, THIRD]), "published functions");
}
