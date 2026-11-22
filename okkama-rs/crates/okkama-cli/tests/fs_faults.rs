//! Atomic replace under injected filesystem faults (#934): a failed write,
//! file fsync or rename leaves the old file whole and no temporary file
//! behind; a failed directory fsync after the rename leaves the new content
//! and reports not_durable. Faults come from SPARROW_CORE_FS_FAULTS.
#![cfg(unix)]

use std::path::Path;
use std::process::Command;

fn replace_with(fault: &str) -> String {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scratch = std::env::temp_dir().join(format!("okkama-fs-faults-{}-{}", std::process::id(), fault.replace(':', "-")));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).unwrap();
    std::fs::write(scratch.join("notes.txt"), "old").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_okkama"))
        .arg("run")
        .arg(here.join("tests/fs_faults/replace.ok"))
        .arg(&scratch)
        .env("OKKAMA_STDLIB_DIR", here.join("../../../std"))
        .env("SPARROW_CORE_FS_FAULTS", fault)
        .output()
        .expect("run okkama");
    let _ = std::fs::remove_dir_all(&scratch);
    assert!(output.status.success(), "{fault}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn failures_before_the_rename_keep_the_old_file_and_leave_no_temporary() {
    for (fault, code) in [("write:ENOSPC", "resource_exhausted"), ("file_sync:EIO", "unexpected"), ("rename:EXDEV", "unsupported")] {
        let out = replace_with(fault);
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines[0].starts_with("refused: ") && lines[0].contains(code), "{fault}: {out}");
        assert_eq!(lines[1], "content: old", "{fault}: {out}");
        assert_eq!(lines[2], "entries: 1", "{fault}: {out}");
    }
}

#[test]
fn a_failed_directory_sync_after_the_rename_is_not_durable_with_the_new_content() {
    let out = replace_with("directory_sync:EIO");
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines[0].starts_with("refused: ") && lines[0].contains("not_durable"), "{out}");
    assert_eq!(lines[1], "content: new", "{out}");
    assert_eq!(lines[2], "entries: 1", "{out}");
}

#[test]
fn without_faults_the_replace_goes_through() {
    assert_eq!(replace_with(""), "replaced\ncontent: new\nentries: 1\n");
}
