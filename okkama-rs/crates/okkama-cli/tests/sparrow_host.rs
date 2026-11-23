//! Host state belongs to one run (#40): a second program on the same thread
//! does not see the working directory the first one changed. Programs read the
//! environment as an immutable snapshot (os/environment) and cannot change it.
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
fn a_working_directory_change_does_not_outlive_its_run() {
    let process = std::env::current_dir().unwrap().as_os_str().len().to_string();
    assert_ne!(process, "1");
    let chdir = lower("runtime/fs", FS, "if chdir([47.byte()]) { cwd().size(); } else { 0; }");
    let cwd = lower("runtime/fs", FS, "cwd().size();");
    for _ in 0..2 {
        assert_eq!(run(&chdir), Some("1".into()));
        assert_eq!(run(&cwd), Some(process.clone()), "the next run starts in /");
    }
    assert_eq!(std::env::current_dir().unwrap().as_os_str().len().to_string(), process);
}
