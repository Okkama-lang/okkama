//! Host state belongs to one run (#40). A program reads the working directory
//! of its run, which starts as the process's, and has no way to change it:
//! `chdir` went with #1022, as nothing in std called it. The run's host
//! session, where a worker's directory lives, is tested in
//! okkama-runtime-core (`host::tests`). Programs read the environment as an
//! immutable snapshot (os/environment) and cannot change it.
#[path = "sparrow_support/mod.rs"]
mod support;
use okkama_compiler::{SourceFile, ir, lowering, syntax};

const FS: &str = "native cwd() is @byte; native chdir(path is @byte) is bool;";

fn lower(module: &str, natives: &str, body: &str) -> ir::Module {
    let source = SourceFile::new("host.ok", format!("module {module} v1.0.0; {natives} main() is int {{ {body} }}")).unwrap();
    lowering::lower_program(&syntax::parse(&source, &syntax::ParseOptions::default()).unwrap()).unwrap()
}

/// Result of `main` on Sparrow, or `None` when it failed.
fn run(module: &ir::Module) -> Option<String> {
    match support::sparrow(module) {
        support::Ending::Value(value) => Some(value.to_string()),
        _ => None,
    }
}

#[test]
fn a_run_reads_the_process_working_directory_and_cannot_change_it() {
    let process = std::env::current_dir().unwrap().as_os_str().len().to_string();
    assert_ne!(process, "1");
    let cwd = lower("runtime/fs", FS, "cwd().size();");
    let chdir = lower("runtime/fs", FS, "if chdir([47.byte()]) { cwd().size(); } else { 0; }");
    for _ in 0..2 {
        assert_eq!(run(&cwd), Some(process.clone()));
        assert_eq!(run(&chdir), None, "runtime/fs has no chdir provider (#1022)");
    }
    assert_eq!(std::env::current_dir().unwrap().as_os_str().len().to_string(), process);
}
