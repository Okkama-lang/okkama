//! #588 phase B: a recompiled session translates into the published numbering; new items are appended, a
//! changed published item is an error.
use okkama_sparrow::session::extend;
use okkama_sparrow::{assemble, FuncRef, Instruction, ObjectRef, Program};

const PUBLISHED: &str = "sparrow 16\nmodule \"repl\" \"1.0.0\"\n\
type t0 int\ntype t1 object obj0\n\
object obj0 \"A\" {\n field \"n\" t0\n}\n\
function f0 \"a\" {\n registers [t0]\n params []\n result t0\n 0: const r0, int 1\n 1: ret r0\n}\n\
function f1 \"make a\" {\n registers [t0, t1]\n params []\n result t1\n 0: call r0, f0, []\n 1: obj.new r1, obj0, [r0]\n 2: ret r1\n}\n";

/// The same session recompiled with one more cell, numbered differently: the new object and function come
/// first, the types in another order.
const FRESH: &str = "sparrow 16\nmodule \"repl\" \"1.0.0\"\n\
type t0 object obj0\ntype t1 object obj1\ntype t2 int\n\
object obj0 \"B\" {\n field \"a\" t1\n}\n\
object obj1 \"A\" {\n field \"n\" t2\n}\n\
function f0 \"cell 2\" {\n registers [t1, t0]\n params []\n result t0\n 0: call r0, f2, []\n 1: obj.new r1, obj0, [r0]\n 2: ret r1\n}\n\
function f1 \"a\" {\n registers [t2]\n params []\n result t2\n 0: const r0, int 1\n 1: ret r0\n}\n\
function f2 \"make a\" {\n registers [t2, t1]\n params []\n result t1\n 0: call r0, f1, []\n 1: obj.new r1, obj1, [r0]\n 2: ret r1\n}\n";

fn program(text: &str) -> Program {
    assemble(text).unwrap()
}

#[test]
fn a_recompiled_session_keeps_the_published_numbering_and_appends_the_rest() {
    let published = program(PUBLISHED);
    let merged = extend(&published, &program(FRESH)).unwrap();
    merged.verify().unwrap();
    assert!(merged.functions.starts_with(&published.functions));
    assert!(merged.objects.starts_with(&published.objects));
    assert!(merged.types.starts_with(&published.types));
    assert_eq!(merged.functions.len(), 3);
    assert_eq!(merged.objects[1].name, "B");
    let cell = &merged.functions[2];
    assert_eq!(cell.symbol, "cell 2");
    assert!(matches!(cell.code[0], Instruction::Call { function: FuncRef(1), .. }), "make a keeps index 1");
    assert!(matches!(cell.code[1], Instruction::ObjNew { object: ObjectRef(1), .. }), "B is appended as obj1");
}

#[test]
fn an_unchanged_session_is_the_published_program() {
    let published = program(PUBLISHED);
    let merged = extend(&published, &published).unwrap();
    assert_eq!(merged, published);
    // And extending twice with the same fresh program adds nothing the second time.
    let once = extend(&published, &program(FRESH)).unwrap();
    let twice = extend(&once, &program(FRESH)).unwrap();
    assert_eq!(twice, once);
}

#[test]
fn a_changed_published_item_is_an_error() {
    let published = program(PUBLISHED);
    let changed = FRESH.replace("const r0, int 1", "const r0, int 2");
    let error = extend(&published, &program(&changed)).unwrap_err();
    assert!(error.0.contains("changes the published function a"), "{}", error.0);
    let field = FRESH.replace("object obj1 \"A\" {\n field \"n\" t2", "object obj1 \"A\" {\n field \"m\" t2");
    let error = extend(&published, &program(&field)).unwrap_err();
    assert!(error.0.contains("changes the published object A"), "{}", error.0);
}
