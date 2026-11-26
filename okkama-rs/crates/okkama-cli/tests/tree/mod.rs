//! The tree these tests check (#1101): its std, and the `okkama` binary set to use it. `okkama` finds the
//! repository's std by itself only when it runs from `okkama-rs/target`; built into another `CARGO_TARGET_DIR`
//! it fell back to an installed std (or none), and a program importing std failed or ran against the wrong one.
//! Every test file runs the binary through [`okkama`], or, where another program starts it, passes [`OKKAMA`]
//! and [`std_dir`] itself; `cli.rs` checks that no file names the binary any other way.
#![allow(dead_code)]

use std::path::PathBuf;
use std::process::Command;

/// The `okkama` binary under test.
pub const OKKAMA: &str = env!("CARGO_BIN_EXE_okkama");

/// The repository root.
pub fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// The std of this tree.
pub fn std_dir() -> PathBuf {
    root().join("std")
}

/// `okkama` with `OKKAMA_STDLIB_DIR` set to this tree's std; a test of another std sets the variable again.
pub fn okkama() -> Command {
    let mut command = Command::new(OKKAMA);
    command.env("OKKAMA_STDLIB_DIR", std_dir());
    command
}
