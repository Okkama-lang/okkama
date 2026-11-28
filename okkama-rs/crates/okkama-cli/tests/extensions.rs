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
    for command in [vec!["run", "-t"]] {
        let output = tree::okkama().args(command).arg("--ext").arg(library).arg(sample("prog.ok")).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(String::from_utf8_lossy(&output.stdout).contains("C extension"));
    }
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
