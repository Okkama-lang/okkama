//! Bundles the repository's std (#1078): every `std/**/*.ok`, embedded read-only, so imports resolve without a
//! file system. The wasm gets the stack of a native main thread: analysis and lowering recurse.

use std::path::{Path, PathBuf};

fn main() {
    let std = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../../std");
    let std = std.canonicalize().expect("the repository's std");
    println!("cargo:rerun-if-changed={}", std.display());
    let mut files = Vec::new();
    collect(&std, &std, &mut files);
    files.sort();
    let mut out = String::from("/// The std modules: path under std (`io/stdout.ok`) and text.\npub static FILES: &[(&str, &str)] = &[\n");
    for (relative, path) in &files {
        out.push_str(&format!("    ({relative:?}, include_str!({:?})),\n", path.display().to_string()));
    }
    out.push_str("];\n");
    let target = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("std_files.rs");
    std::fs::write(target, out).expect("write the std bundle");
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        println!("cargo:rustc-link-arg-cdylib=-zstack-size=8388608");
    }
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    for entry in std::fs::read_dir(dir).expect("read std").flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, out);
        } else if path.extension().is_some_and(|extension| extension == "ok") {
            let relative = path.strip_prefix(root).unwrap();
            let parts: Vec<String> = relative.components().map(|part| part.as_os_str().to_string_lossy().into_owned()).collect();
            out.push((parts.join("/"), path));
        }
    }
}
