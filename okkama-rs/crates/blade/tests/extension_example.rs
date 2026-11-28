//! The shipped PNG example, including its deterministic output and native refusal.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Example(PathBuf);
impl Drop for Example {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if ["target", "pkg", "output", "blade.lock"].iter().any(|skip| name == *skip) { continue; }
        if entry.file_type().unwrap().is_dir() {
            copy(&entry.path(), &to.join(name));
        } else {
            std::fs::copy(entry.path(), to.join(name)).unwrap();
        }
    }
}

fn success(output: Output) -> String {
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn thumbs_build_run_and_refuse_native() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("extension-example-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let example = Example(root);
    let extensions = example.0.join("examples/extensions");
    let source = repo.join("examples/extensions");
    for name in ["imagekit", "thumbs"] { copy(&source.join(name), &extensions.join(name)); }
    // Keep the development SDK path valid before and after Blade copies the package.
    std::os::unix::fs::symlink(repo.join("okkama-rs"), example.0.join("okkama-rs")).unwrap();
    std::os::unix::fs::symlink(repo.join("okkama-rs"), extensions.join("okkama-rs")).unwrap();
    let app = extensions.join("thumbs");
    let target = app.join("target/native/imagekit");
    let build = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args(["build", "--offline", "--locked", "-j", "4", "--release", "--manifest-path"])
        .arg(extensions.join("imagekit/native/Cargo.toml"))
        .arg("--target-dir").arg(&target).output().unwrap();
    if !build.status.success() {
        let error = String::from_utf8_lossy(&build.stderr);
        let unavailable = error.contains("no matching package named")
            || error.contains("attempting to make an HTTP request, but --offline was specified")
            || error.contains("failed to download");
        if unavailable && std::env::var_os("CI").is_none() {
            eprintln!("imagekit example skipped: dependencies unavailable offline: {error}");
            return;
        }
        panic!("imagekit offline build: {error}");
    }
    let bin = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().to_path_buf();
    let blade = || {
        let mut command = Command::new(bin.join("blade"));
        command.current_dir(&app).env("BLADE_OKKAMA", bin.join("okkama"))
            .env("OKKAMA_STDLIB_DIR", repo.join("std"))
            .env("CARGO_NET_OFFLINE", "true").env("CARGO_BUILD_JOBS", "4");
        command
    };
    success(blade().arg("sync").output().unwrap());
    success(blade().arg("build").output().unwrap());
    std::fs::write(app.join("input/ignored.txt"), "not a PNG").unwrap();
    std::fs::create_dir(app.join("input/directory.png")).unwrap();
    let errors = Command::new(bin.join("okkama")).current_dir(&app)
        .env("OKKAMA_STDLIB_DIR", repo.join("std"))
        .env("OKKAMA_EXT_CHECK", "1")
        .args(["run", "--ext"]).arg(target.join(format!("extension.{}", std::env::consts::DLL_EXTENSION)))
        .arg(app.join("pkg/imagekit/tests/errors.ok")).arg(app.join("input/landscape.png"))
        .output().unwrap();
    assert!(errors.stderr.is_empty(), "{}", String::from_utf8_lossy(&errors.stderr));
    assert_eq!(success(errors), "");
    for checked in ["0", "1"] {
        let output = blade().arg("run").env("OKKAMA_EXT_CHECK", checked).output().unwrap();
        assert!(output.stderr.is_empty(), "{}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(success(output), include_str!("../../../../examples/extensions/thumbs/expected.txt"));
        for (name, width, height) in [("landscape", 8_u32, 5_u32), ("portrait", 3, 8), ("square", 8, 8)] {
            let png = std::fs::read(app.join(format!("output/{name}.png"))).unwrap();
            assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
            assert_eq!(&png[16..20], &width.to_be_bytes());
            assert_eq!(&png[20..24], &height.to_be_bytes());
            let digest = sha256(&png);
            let expected = include_str!("../../../../examples/extensions/thumbs/expected.txt");
            assert!(expected.lines().any(|line| line == format!("{name}.png {width}x{height} {digest}")));
        }
    }
    let refused = blade().args(["run", "--no-native"]).env("CARGO", "/no-cargo").output().unwrap();
    assert!(!refused.status.success());
    assert!(refused.stdout.is_empty());
    let error = String::from_utf8(refused.stderr).unwrap();
    let message = error.split("Stack trace (Sparrow,").next().unwrap();
    assert_eq!(message, include_str!("../../../../examples/extensions/thumbs/no-native.expected.txt"));
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}
