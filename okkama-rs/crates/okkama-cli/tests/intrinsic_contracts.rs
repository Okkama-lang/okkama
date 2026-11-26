//! Load-time contracts of generic intrinsics (#69): a crafted SPB that pairs
//! channel endpoints of different payload types, or a scope whose action
//! yields another type than its result, is refused by `sparrow check`; the
//! compiler's own SPB is accepted.

use okkama_sparrow::{Type, TypeRef};
use std::path::Path;
use std::process::Command;

fn compiled(case: &str) -> okkama_sparrow::Program {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let source = root.join(format!("conformance/cases/{case}/main.ok"));
    // One file per build: the tests build in parallel, sometimes the same case.
    static BUILDS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let build = BUILDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let out = std::env::temp_dir().join(format!(
        "intrinsic-contracts-{}-{build}-{}.spb",
        std::process::id(),
        case.replace('/', "-")
    ));
    let built = Command::new(env!("CARGO_BIN_EXE_okkama"))
        .args(["build"])
        .arg(&source)
        .arg("-o")
        .arg(&out)
        .env("OKKAMA_STDLIB_DIR", root.join("std"))
        .output()
        .unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let program = okkama_sparrow::decode(&std::fs::read(&out).unwrap()).unwrap();
    let _ = std::fs::remove_file(out);
    program
}

fn check(program: &okkama_sparrow::Program) -> Result<(), String> {
    sparrow_core::check(&okkama_sparrow::encode(program).unwrap()).map_err(|error| error.0)
}

/// A copy of the intrinsic function `name`, never called, whose result is
/// changed to an array of what it was: a different reference type.
fn with_retyped_copy(mut program: okkama_sparrow::Program, name: &str) -> okkama_sparrow::Program {
    let original = program
        .functions
        .iter()
        .find(|f| f.intrinsic.as_ref().is_some_and(|i| i.name == name))
        .unwrap_or_else(|| panic!("no {name}"))
        .clone();
    let result = original.result.expect("a result");
    program.types.push(Type::Array(result));
    let mut copy = original;
    copy.symbol.push_str("$crafted");
    copy.result = Some(TypeRef(program.types.len() as u32 - 1));
    program.functions.push(copy);
    program
}

#[test]
fn endpoints_of_one_channel_carry_one_payload_type() {
    let program = compiled("L08-async-and-concurrency/channel.close-semantics");
    check(&program).expect("the compiler's SPB is accepted");
    let crafted = with_retyped_copy(program, "okkama.channel.Receiving.item");
    let refused = check(&crafted).expect_err("a receiving endpoint of another payload type");
    assert!(refused.contains("channel endpoints carry different payload types"), "{refused}");
}

#[test]
fn scopes_and_races_yield_the_type_they_wait_for() {
    let program = compiled("L08-async-and-concurrency/scope.future-scope-and-deadlines");
    check(&program).expect("the compiler's SPB is accepted");
    let refused = check(&with_retyped_copy(program, "okkama.scope.FutureScope.outcome"))
        .expect_err("a scope of another result type");
    assert!(refused.contains("scope action does not take the group or yield the result"), "{refused}");
    let program = compiled("L08-async-and-concurrency/collect.race-empty");
    check(&program).expect("the compiler's SPB is accepted");
    let refused = check(&with_retyped_copy(program, "okkama.collect.Raced.winner")).expect_err("a race of another result type");
    assert!(refused.contains("result does not match"), "{refused}");
}


/// std's `async/atomic` as a root module with `enum AtomicClosed is Error = (CASES);`
/// and a main that reads the payload `x` of the case `Atomic.value` throws.
fn atomic(cases: &str) -> okkama_sparrow::Program {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let text = std::fs::read_to_string(root.join("std/async/atomic.ok")).unwrap();
    let text = &text[..text.find("\ntest \"").expect("the module's tests")];
    let declared = "enum AtomicClosed is Error = (closed);";
    assert!(text.contains(declared));
    let read = if cases.starts_with("closed(") { "catch AtomicClosed.closed(x) { x; }" } else { "catch AtomicClosed.closed { 7; }" };
    let source = format!(
        "{}\nmain() is int {{ let cell = Atomic(1); cell.close(); try {{ cell.value(); }} {read}; }}\n",
        text.replace(declared, &format!("enum AtomicClosed is Error = ({cases});"))
    );
    static BUILDS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let build = BUILDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("intrinsic-contracts-atomic-{}-{build}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (file, out) = (dir.join("atomic.ok"), dir.join("atomic.spb"));
    std::fs::write(&file, source).unwrap();
    let built = Command::new(env!("CARGO_BIN_EXE_okkama"))
        .arg("build")
        .arg(&file)
        .arg("-o")
        .arg(&out)
        .env("OKKAMA_STDLIB_DIR", root.join("std"))
        .output()
        .unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let program = okkama_sparrow::decode(&std::fs::read(&out).unwrap()).unwrap();
    let _ = std::fs::remove_dir_all(dir);
    program
}

/// #1090: the runtime raises an intrinsic's error case with the payload it has
/// (`Atomic.value`: none), so a case declaring other fields is refused at load,
/// naming the intrinsic, whatever the enum's other cases.
#[test]
fn an_intrinsic_error_case_declares_the_payload_the_runtime_raises() {
    check(&atomic("closed")).expect("std's own shape");
    check(&atomic("closed, other(y is int)")).expect("another case's payload is not raised");
    for cases in ["closed(x is int)", "closed(x is int), other"] {
        let refused = check(&atomic(cases)).expect_err(cases);
        assert!(refused.contains("(okkama.atomic.Atomic.value): intrinsic contract: related object 0 throws AtomicClosed \
            with payload [], but it declares [x I]"), "{refused}");
    }
}

/// #1090: the layout before #290 (`[$tag, $0_x: array…]`) is no error case any
/// more. It used to load, and reading `x` read the tag: `x` was 0.
#[test]
fn an_error_case_in_the_old_tag_layout_is_refused() {
    let mut program = atomic("closed(x is int)");
    let case = program.objects.iter().position(|object| object.name == "AtomicClosed").expect("the case");
    let int = program.types.iter().position(|ty| matches!(ty, Type::Int)).expect("int") as u32;
    program.types.push(Type::Array(TypeRef(int)));
    let array = TypeRef(program.types.len() as u32 - 1);
    program.objects[case].fields = vec![("$tag".into(), TypeRef(int)), ("$0_x".into(), array)];
    let refused = check(&program).expect_err("a tag and payload arrays");
    assert!(refused.contains("throws AtomicClosed with payload [], but it declares [$tag I, $0_x P]"), "{refused}");
}
