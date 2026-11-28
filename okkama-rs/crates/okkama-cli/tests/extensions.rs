//! ABI-1 libraries built with the system C compiler, without host linker symbols.
#![cfg(unix)]
mod tree;

use std::{path::{Path, PathBuf}, process::{Command, Output}, sync::OnceLock};

fn sample(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/extensions/c").join(name)
}

fn libraries() -> Option<&'static (PathBuf, PathBuf)> {
    static BUILT: OnceLock<Option<(PathBuf, PathBuf)>> = OnceLock::new();
    BUILT.get_or_init(|| {
        match Command::new("cc").arg("--version").output() {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("no cc on PATH: native extension integration tests skipped");
                return None;
            }
            Err(e) => panic!("cannot run cc: {e}"),
            Ok(output) => assert!(output.status.success(), "cc --version failed"),
        }
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("extensions-c-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let build = |name: &str, abi: &str| {
            let path = dir.join(format!("{name}.{}", std::env::consts::DLL_EXTENSION));
            let output = Command::new("cc").args(["-shared", "-fPIC", "-std=c11", "-Wall", "-Wextra", "-Werror"])
                .arg(format!("-DSAMPLE_ABI={abi}"))
                .arg("-I").arg(tree::root().join("okkama-rs/crates/okkama-runtime-abi/include"))
                .arg(sample("sample.c")).arg("-o").arg(&path).output().unwrap();
            assert!(output.status.success(), "C sample: {}", String::from_utf8_lossy(&output.stderr));
            path
        };
        Some((build("sample", "1"), build("wrong-abi", "999")))
    }).as_ref()
}

fn error(output: Output) -> String {
    assert!(!output.status.success(), "unexpected success: {}", String::from_utf8_lossy(&output.stdout));
    String::from_utf8(output.stderr).unwrap()
}

fn run(library: &Path, extra: &[&str]) -> Output {
    tree::okkama().args(["run", "--ext"]).arg(library).args(extra).arg(sample("prog.ok")).output().unwrap()
}

#[test]
fn a_missing_provider_keeps_the_unregistered_native_text() {
    let message = error(tree::okkama().arg("run").arg(sample("prog.ok")).output().unwrap());
    assert!(message.contains("Native symbol \"sample@0::add/2\" is not registered"), "{message}");
}

#[test]
fn c_scalars_views_state_and_async_cross_the_shared_library_boundary() {
    let Some((library, _)) = libraries() else { return };
    for check in ["0", "1", "OFF", "yes"] {
        let output = tree::okkama().args(["run", "--ext"]).arg(library).arg(sample("prog.ok"))
            .env("OKKAMA_EXT_CHECK", check).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(String::from_utf8(output.stdout).unwrap(), "46\ntrue\nhello 🐦\n42\nASYNC BIRD\n");
        assert_eq!(String::from_utf8(output.stderr).unwrap(), "sample state dropped\n");
    }
}

#[test]
fn loading_refuses_wrong_abi_and_missing_paths() {
    let Some((_, wrong)) = libraries() else { return };
    let message = error(run(wrong, &[]));
    assert!(message.contains("wrong-abi") && message.contains("ABI"), "{message}");
    let message = error(run(&wrong.with_file_name("missing-extension.so"), &[]));
    assert!(message.contains("missing-extension.so") && message.contains("native extension"), "{message}");
}

#[test]
fn no_native_suppresses_loading_in_either_option_order() {
    let Some((_, wrong)) = libraries() else { return };
    for output in [run(wrong, &["--no-native"]),
        tree::okkama().args(["run", "--no-native", "--ext"]).arg(wrong).arg(sample("prog.ok")).output().unwrap()] {
        let message = error(output);
        assert!(message.contains("Native symbol \"sample@0::add/2\" is not registered"), "{message}");
        assert!(!message.contains("ABI") && !message.contains("constructor ran"), "{message}");
    }
}

#[test]
fn extensions_are_available_to_inline_tests() {
    let Some((library, _)) = libraries() else { return };
    let output = tree::okkama().args(["run", "-t"]).arg("--ext").arg(library).arg(sample("prog.ok")).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(String::from_utf8_lossy(&output.stdout).contains("C extension"));
}

#[test]
fn binding_refuses_both_signatures_before_any_callback() {
    let Some((library, _)) = libraries() else { return };
    let source = "module sample; native add(a is int, b is float) is int; main() is int { add(a: 1, b: 2.0); }";
    let path = library.with_file_name("mismatch.ok");
    std::fs::write(&path, source).unwrap();
    let message = error(tree::okkama().args(["run", "--ext"]).arg(library).arg(&path).output().unwrap());
    assert!(message.contains("signature mismatch") && message.contains("(int, float) -> int")
        && message.contains("(int, int) -> int"), "{message}");
    assert!(!message.contains("sample state dropped"));
}

#[test]
fn repeated_and_canonical_paths_share_one_descriptor() {
    let Some((library, _)) = libraries() else { return };
    let alias = library.with_file_name("sample-alias.so");
    let _ = std::fs::remove_file(&alias);
    std::os::unix::fs::symlink(library, &alias).unwrap();
    let output = tree::okkama().args(["run", "--ext"]).arg(library).arg("--ext").arg(&alias)
        .arg(sample("prog.ok")).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stderr).matches("sample state dropped").count(), 1);
}

#[test]
fn exec_binds_extensions_and_honours_no_native() {
    let Some((library, wrong)) = libraries() else { return };
    let bytecode = library.with_file_name("exec.spb");
    let build = tree::okkama().arg("build").arg(sample("prog.ok")).arg("-o").arg(&bytecode).output().unwrap();
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let output = tree::okkama().args(["exec", "--ext"]).arg(library).arg(&bytecode).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "46\ntrue\nhello 🐦\n42\nASYNC BIRD\n");
    let message = error(tree::okkama().args(["exec", "--ext"]).arg(wrong).arg("--no-native").arg(&bytecode).output().unwrap());
    assert!(message.contains("is not registered"), "{message}");
}

#[test]
fn repl_keeps_extensions_across_cells_and_reset() {
    use std::io::Write;
    let Some((library, _)) = libraries() else { return };
    let mut child = tree::okkama().args(["repl", "--ext"]).arg(library).current_dir(sample(""))
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped())
        .spawn().unwrap();
    child.stdin.take().unwrap().write_all(b"import \"sample\" take add;\nadd(a: 17, b: 29)\n:reset\nimport \"sample\" take add;\nadd(a: 1, b: 2)\n").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(output.stderr.is_empty(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "46 (int)\n3 (int)\n");
}

#[test]
fn worker_processes_receive_extensions_including_pooled_items_and_tests() {
    let Some((library, _)) = libraries() else { return };
    let path = library.with_file_name("workers.ok");
    std::fs::write(&path, r#"module sample;
        import "parallel/map" take ParallelMapped;
        native add(a is int, b is int) is int;
        object Worker { Worker(); run(value is int) is int { add(a: value, b: 10); } }
        async main() is int {
            let values = await ParallelMapped(origins: [1, 2, 3], action: Worker().run, workers: 1).results();
            confirm values.item(0) == 11 && values.item(1) == 12 && values.item(2) == 13;
            0;
        }
        test "native in test worker" { confirm add(a: 1, b: 2) == 3; }
    "#).unwrap();
    let checked = tree::okkama().arg("check").arg(&path).output().unwrap();
    assert!(checked.status.success(), "{}", String::from_utf8_lossy(&checked.stderr));
    let bytecode = path.with_extension("spb");
    let built = tree::okkama().arg("build").arg(&path).arg("-o").arg(&bytecode).output().unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let embedding = sparrow_core::Embedding {
        limits: sparrow_core::budget::Limits { extension_paths: vec![library.clone()], ..Default::default() },
        ..Default::default()
    };
    let result = sparrow_core::run_embedded(std::fs::read(&bytecode).unwrap(), embedding).unwrap();
    assert_eq!(result.result, Ok(0));
    if !sparrow_core::env_switch::enabled("CI") {
        eprintln!("process worker extension tests run on CI runners only; emulated workers passed");
        return;
    }
    for options in [vec!["run"], vec!["run", "-t", "--workers", "1"]] {
        let output = tree::okkama().args(options).arg("--ext").arg(library).arg(&path).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    }
    let message = error(tree::okkama().args(["run", "--no-native", "--ext"]).arg(library).arg(&path).output().unwrap());
    assert!(message.contains("is not registered"), "{message}");
}

#[test]
fn malformed_libraries_report_the_file_and_reason_without_callbacks() {
    let Some((library, _)) = libraries() else { return };
    for (name, tables, descriptor, reason) in [
        ("size", "", "{1, 0, \"bad\", NULL, 0, NULL, 0}", "size"),
        ("null-name", "", "{1, sizeof(okk_extension), NULL, NULL, 0, NULL, 0}", "null string"),
        ("utf8", "", "{1, sizeof(okk_extension), \"\\xff\", NULL, 0, NULL, 0}", "UTF-8"),
        ("symbol", "static const okk_export e[] = {{\"bad\", \"() -> int\", \"\", callback, NULL}};",
            "{1, sizeof(okk_extension), \"bad\", e, 1, NULL, 0}", "invalid extension symbol"),
        ("signature", "static const okk_export e[] = {{\"bad@0::f/0\", \"bad signature\", \"\", callback, NULL}};",
            "{1, sizeof(okk_extension), \"bad\", e, 1, NULL, 0}", "signature"),
        ("async-mismatch", "static const okk_export e[] = {{\"bad@0::f/0\", \"async () -> int\", \"\", callback, NULL}};",
            "{1, sizeof(okk_extension), \"bad\", e, 1, NULL, 0}", "async callback"),
        ("null-async", "static const okk_async a = {NULL, NULL, NULL, NULL}; static const okk_export e[] = {{\"bad@0::f/0\", \"async () -> int\", \"\", NULL, &a}};",
            "{1, sizeof(okk_extension), \"bad\", e, 1, NULL, 0}", "null async callback"),
        ("duplicate", "static const okk_export e[] = {{\"bad@0::f/0\", \"() -> int\", \"\", callback, NULL}, {\"bad@0::f/0\", \"() -> int\", \"\", callback, NULL}};",
            "{1, sizeof(okk_extension), \"bad\", e, 2, NULL, 0}", "duplicate extension symbol"),
        ("state-drop", "static const okk_state_type t[] = {{\"State\", NULL}};",
            "{1, sizeof(okk_extension), \"bad\", NULL, 0, t, 1}", "null state drop"),
        ("state-duplicate", "static const okk_state_type t[] = {{\"State\", drop}, {\"State\", drop}};",
            "{1, sizeof(okk_extension), \"bad\", NULL, 0, t, 2}", "duplicate extension state"),
    ] {
        let source = format!(r#"#include "okkama_ext.h"
#include <stdlib.h>
static uint32_t callback(okk_call *c, const okk_slot *a, okk_slot *out) {{ abort(); }}
static void drop(void *p) {{ abort(); }}
{tables}
const okk_extension okkama_extension = {descriptor};
"#);
        let file = library.with_file_name(format!("bad-{name}.c"));
        let path = file.with_extension(std::env::consts::DLL_EXTENSION);
        std::fs::write(&file, source).unwrap();
        let output = Command::new("cc").args(["-shared", "-fPIC", "-std=c11"])
            .arg("-I").arg(tree::root().join("okkama-rs/crates/okkama-runtime-abi/include"))
            .arg(&file).arg("-o").arg(&path).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let message = error(run(&path, &[]));
        assert!(message.contains(&format!("bad-{name}")) && message.contains(reason), "{name}: {message}");
    }
    let source = library.with_file_name("no-descriptor.c");
    let path = source.with_extension(std::env::consts::DLL_EXTENSION);
    std::fs::write(&source, "const int unrelated = 1;").unwrap();
    let output = Command::new("cc").args(["-shared", "-fPIC"]).arg(&source).arg("-o").arg(&path).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let message = error(run(&path, &[]));
    assert!(message.contains("no-descriptor") && message.contains("okkama_extension"), "{message}");
    let path = library.with_file_name("not-a-library.so");
    std::fs::write(&path, "not a shared library").unwrap();
    let message = error(run(&path, &[]));
    assert!(message.contains("not-a-library.so") && !message.contains("cannot resolve path"), "{message}");
}

#[test]
fn declarations_from_c_compile_as_a_module() {
    let Some((library, _)) = libraries() else { return };
    check_declarations(library, "sample", "native async uppercase(value is string) is string;");
}

fn check_declarations(library: &Path, module: &str, declaration: &str) {
    let output = tree::okkama().args(["ext", "declare"]).arg(library).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let source = String::from_utf8(output.stdout).unwrap();
    assert!(source.starts_with(&format!("module {module};\n")), "{source}");
    assert!(source.contains(declaration), "{source}");
    let file = library.with_file_name(format!("{module}-declarations.ok"));
    std::fs::write(&file, source).unwrap();
    let checked = tree::okkama().arg("check").arg(file).output().unwrap();
    assert!(checked.status.success(), "{}", String::from_utf8_lossy(&checked.stderr));
}

fn rust_library() -> &'static PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT.get_or_init(|| {
        let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("extensions-rust");
        let manifest = sample("../rust/Cargo.toml");
        // The isolated sample lock must remain a subset of the workspace resolution.
        let identities = |lock: String| lock.split("[[package]]").skip(1)
            .map(|p| p.split("dependencies =").next().unwrap().trim().to_owned())
            .collect::<std::collections::HashSet<_>>();
        let workspace = identities(std::fs::read_to_string(tree::root().join("okkama-rs/Cargo.lock")).unwrap());
        let sample_lock = identities(std::fs::read_to_string(manifest.with_file_name("Cargo.lock")).unwrap());
        for package in sample_lock {
            assert!(package.starts_with("name = \"okkama-ext-sample\"") || workspace.contains(&package),
                "sample dependency is not pinned by the workspace lock: {package}");
        }
        let output = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
            .args(["build", "--offline", "--locked", "-j", "4", "--manifest-path"])
            .arg(manifest).arg("--target-dir").arg(&target).output().unwrap();
        assert!(output.status.success(), "Rust sample: {}", String::from_utf8_lossy(&output.stderr));
        target.join("debug").join(format!("{}okkama_ext_sample.{}", std::env::consts::DLL_PREFIX, std::env::consts::DLL_EXTENSION))
    })
}

#[test]
fn declarations_from_rust_compile_as_a_module() {
    check_declarations(rust_library(), "rust_sample", "native async shared(image is Image, suffix is @byte) is string;");
}

fn rust_program(name: &str, body: &str) -> PathBuf {
    let library = rust_library();
    let declarations = tree::okkama().args(["ext", "declare"]).arg(library).output().unwrap();
    assert!(declarations.status.success(), "{}", String::from_utf8_lossy(&declarations.stderr));
    let file = library.with_file_name(format!("{name}.ok"));
    let declarations = String::from_utf8(declarations.stdout).unwrap()
        .replacen("module rust_sample;", "module rust_sample;\nimport \"io/stdout\" take Stdout;\nimport \"async/scope\" take FutureGroup;", 1);
    let source = format!("{declarations}\n{body}");
    std::fs::write(&file, source).unwrap();
    file
}

#[test]
fn rust_values_states_failure_and_blocking_jobs_roundtrip() {
    let path = rust_program("roundtrip", r#"
async main() is int {
    Stdout().writeln(add(a: 17, b: 29));
    confirm negate(value: false);
    confirm floating(value: 1.25) == 2.5;
    confirm byte_copy(value: byte(255)) == byte(255);
    confirm character(value: '🐦') == '🐦';
    Stdout().writeln(text_copy(value: "hello 🐦"));
    confirm bytes_copy(value: [byte(0), byte(128), byte(255)]) == [byte(0), byte(128), byte(255)];
    confirm bytes_owned(value: [byte(255)]) == [byte(255)];
    confirm integers(value: [1, -2, 9223372036854775807]) == [1, -2, 9223372036854775807];
    confirm integers(value: []) == [];
    confirm dictionary(value: ["b": 2, "a": 1]) == ["a": 1, "b": 2];
    confirm maybe(value: Maybe.some("bird")) == Maybe.some("bird");
    confirm maybe(value: Maybe<string>.none) == Maybe<string>.none;
    confirm big(value: 9223372036854775808) == 9223372036854775809;
    confirm big(value: -18446744073709551616) == -18446744073709551615;
    confirm big(value: -1) == 0;
    confirm text_copy(value: "") == "";
    confirm bytes_copy(value: []) == [];
    confirm dictionary(value: [:]) == [:];
    confirm nested(value: [["k": Maybe.some(42)], ["n": Maybe<int>.none]]) == [["k": Maybe.some(42)], ["n": Maybe<int>.none]];
    let caught = try { failure(); 0; } catch e is NativeFailure {
        confirm e.code() == "sample_failure";
        confirm e.domain() == "rust_sample";
        confirm e.operation() == "rust_sample@0::failure/0";
        confirm e.message() == "Rust failure";
        1;
    };
    confirm caught == 1;
    let counter = create(value: 41);
    confirm read(counter: counter) == 41;
    confirm increment(counter: counter) == 42;
    release(counter: counter);
    Stdout().writeln(drops());
    confirm await borrowed(value: "borrowed text") == "borrowed text";
    Stdout().writeln(await uppercase(value: "async bird"));
    Stdout().writeln(await shared(image: image(value: "shared bird"), suffix: [byte(0), byte(255)]));
    0;
}
"#);
    for checked in ["0", "1"] {
        let output = tree::okkama().args(["run", "--ext"]).arg(rust_library()).arg(&path)
            .env("OKKAMA_EXT_CHECK", checked).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(String::from_utf8_lossy(&output.stdout), "46\nhello 🐦\n1\nASYNC BIRD\nSHARED BIRD:2\n");
    }
}

#[test]
fn rust_faults_and_failures_do_not_cross_the_abi() {
    for (name, expression, expected) in [
        ("failure", "failure()", "sample_failure"),
        ("fail_async", "await fail_async()", "async_failure"),
        ("panic_sync", "panic_sync()", "status 2"),
        ("panic_async", "await panic_async()", "status 2"),
        ("not_finite", "not_finite()", "not_finite"),
        ("range", "add(a: 9223372036854775808, b: 1)", "out_of_range"),
        ("closed", "let c = create(value: 1); release(counter: c); read(counter: c)", "already_closed"),
        ("alias", "let c = create(value: 1); alias(a: c, b: c)", "state_borrowed"),
    ] {
        let path = rust_program(name, &format!("async main() is int {{ {expression}; 0; }}"));
        let message = error(tree::okkama().args(["run", "--ext"]).arg(rust_library()).arg(path).output().unwrap());
        assert!(message.contains(expected), "{name}: {message}");
    }
}

#[test]
fn extension_command_help_and_usage_errors() {
    for args in [vec!["help", "ext"], vec!["ext", "--help"], vec!["ext", "declare", "--help"]] {
        let output = tree::okkama().args(args).output().unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("okkama ext declare LIB"));
    }
    for args in [vec!["ext"], vec!["ext", "declare"], vec!["ext", "unknown"], vec!["ext", "declare", "a", "b"]] {
        assert!(error(tree::okkama().args(args).output().unwrap()).contains("expected `okkama ext declare LIB`"));
    }
}

#[test]
fn rust_shared_release_drops_once_after_the_blocking_job() {
    let path = rust_program("shared-release", r#"
object ImageOwner {
    has state is Image;
    ImageOwner(:state);
    destroy { close(image: state); }
}
async main() is int {
    let state = image(value: "still alive");
    let job = held_image(image: state);
    let group is FutureGroup<string> = FutureGroup();
    group.start(job);
    { let owner = ImageOwner(state: state); }
    confirm image_drops() == 0;
    finish_job();
    confirm await job == "still alive";
    await group.wait();
    confirm image_drops() == 1;
    let closed = try { close(image: state); false; } catch e is NativeFailure {
        confirm e.code() == "already_closed";
        true;
    };
    confirm closed;
    confirm image_drops() == 1;
    0;
}
"#);
    for checked in ["0", "1"] {
        let output = tree::okkama().args(["run", "--ext"]).arg(rust_library()).arg(&path)
            .env("OKKAMA_EXT_CHECK", checked).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    }
}
