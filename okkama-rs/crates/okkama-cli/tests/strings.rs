//! Code point positions of long strings (#52): `okkama run` slices them one
//! code point at a time in either direction.

use std::path::Path;
use std::process::Command;

#[test]
fn long_strings_are_sliced_by_code_point() {
    let program = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/strings/positions.ok");
    let output = Command::new(env!("CARGO_BIN_EXE_okkama")).args(["run"]).arg(&program).output().unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", String::from_utf8_lossy(&output.stderr));
}
