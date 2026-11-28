//! #1083: the native table names a `native object` with its module (`runtime.buffer::BufferState`). A std
//! runtime module checked on its own is the root module, whose declarations carry no prefix; its handles must
//! still be named with the module, or its natives no longer match the table.
mod tree;

#[test]
fn every_runtime_module_with_native_objects_checks_as_the_root() {
    let runtime = tree::std_dir().join("runtime");
    let mut checked = 0;
    for entry in std::fs::read_dir(&runtime).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|extension| extension != "ok") {
            continue;
        }
        let source = std::fs::read_to_string(&path).unwrap();
        if !source.contains("native object") {
            continue;
        }
        let output = tree::okkama().arg("check").arg(&path).output().unwrap();
        assert!(output.status.success(), "{}: {}", path.display(), String::from_utf8_lossy(&output.stderr));
        checked += 1;
    }
    assert!(checked >= 5, "only {checked} runtime modules declare native objects");
}
