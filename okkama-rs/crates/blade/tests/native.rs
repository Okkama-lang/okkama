//! Native packages require root consent; sync is inert and run binds the built cdylib.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use blade::{install, lock::Lock};

struct Tree(PathBuf);

impl Tree {
    fn new(name: &str, allow: bool) -> Self {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("blade-native-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let tree = Self(root);
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/native");
        for file in ["blade.toml", "src/native.ok", "native/src/lib.rs", "native/Cargo.lock"] {
            tree.write(&format!("bird/{file}"), &std::fs::read_to_string(fixture.join(file)).unwrap());
        }
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
        let cargo = std::fs::read_to_string(fixture.join("native/Cargo.toml")).unwrap()
            .replace("../../../../../okkama-ext", workspace.join("crates/okkama-ext").to_str().unwrap())
            .replace("../../../../../../vendor/num-bigint", workspace.join("vendor/num-bigint").to_str().unwrap());
        tree.write("bird/native/Cargo.toml", &cargo);
        tree.write("cargo", r#"#!/bin/sh
if [ "$1" = build ]; then
    printf '%s\n' "$*" >> "$BLADE_TEST_CARGO_LOG"
    shift
    exec "$BLADE_TEST_REAL_CARGO" build --offline -j 4 "$@"
fi
if [ "$1" = --version ] && [ -n "$BLADE_TEST_CARGO_VERSION" ]; then
    printf '%s\n' "$BLADE_TEST_CARGO_VERSION"
    exit 0
fi
exec "$BLADE_TEST_REAL_CARGO" "$@"
"#);
        std::fs::set_permissions(tree.0.join("cargo"), std::fs::Permissions::from_mode(0o755)).unwrap();
        tree.root_manifest(allow);
        tree.write("app/src/main.ok", "module app;\nimport \"pkg/bird/src/native\" take answer;\nmain() is int { confirm answer() == 42; 0; }\n");
        tree
    }

    fn root_manifest(&self, allow: bool) {
        self.write("app/blade.toml", &format!("[module]\nname = \"app\"\n[dependencies]\nbird = {{ path = \"../bird\", version = \"1.0.0\" }}\n[native]\nallow = [{}]\n", if allow { "\"bird\"" } else { "" }));
    }

    fn write(&self, path: &str, text: &str) {
        let file = self.0.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }

    fn app(&self) -> PathBuf { self.0.join("app") }

    fn builds(&self) -> usize {
        std::fs::read_to_string(self.0.join("cargo.log")).unwrap_or_default().lines().count()
    }

    fn blade(&self) -> Command {
        let bin = bin_dir();
        let mut cmd = Command::new(bin.join(format!("blade{}", std::env::consts::EXE_SUFFIX)));
        cmd.current_dir(self.app())
            .env("BLADE_OKKAMA", bin.join(format!("okkama{}", std::env::consts::EXE_SUFFIX)))
            .env("OKKAMA_STDLIB_DIR", Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../std"))
            .env("CARGO_NET_OFFLINE", "true").env("CARGO_BUILD_JOBS", "4")
            .env("CARGO", self.0.join("cargo"))
            .env("BLADE_TEST_REAL_CARGO", std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .env("BLADE_TEST_CARGO_LOG", self.0.join("cargo.log"));
        cmd
    }
}

impl Drop for Tree {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

fn success(output: Output) -> String {
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn unmarked_packages_still_cannot_declare_natives() {
    let tree = Tree::new("unmarked", true);
    tree.write("bird/blade.toml", "[module]\nname = \"bird\"\nversion = \"1.0.0\"\n");
    let error = install::sync(&tree.app()).unwrap_err();
    assert!(error.contains("declares a native"), "{error}");
}

#[test]
fn native_modules_stay_in_the_package_namespace() {
    let tree = Tree::new("namespace", true);
    tree.write("bird/src/native.ok", "module runtime/foreign; native answer() is int;\n");
    let error = install::sync(&tree.app()).unwrap_err();
    assert!(error.contains("modules of package \"bird\""), "{error}");
}

#[test]
fn direct_and_transitive_natives_require_root_allow() {
    for transitive in [false, true] {
        let tree = Tree::new(if transitive { "transitive" } else { "direct" }, false);
        if transitive {
            tree.write("middle/blade.toml", "[module]\nname = \"middle\"\nversion = \"1.0.0\"\n[dependencies]\nbird = { path = \"../bird\", version = \"1.0.0\" }\n[native]\nallow = [\"bird\"]\n");
            tree.write("middle/src/main.ok", "module middle;");
            tree.write("app/blade.toml", "[dependencies]\nmiddle = { path = \"../middle\", version = \"1.0.0\" }\n");
        }
        let error = install::sync(&tree.app()).unwrap_err();
        assert!(error.contains("bird 1.0.0 carries native code") && error.contains("allow"), "{error}");
        assert!(!tree.app().join("blade.lock").exists());
        assert!(!tree.app().join("pkg/bird").exists());
        let root = std::fs::read_to_string(tree.app().join("blade.toml")).unwrap();
        let allowed = if transitive { format!("{root}\n[native]\nallow = [\"bird\"]\n") }
            else { root.replace("allow = []", "allow = [\"bird\"]") };
        tree.write("app/blade.toml", &allowed);
        assert!(install::sync(&tree.app()).unwrap().lines.iter().any(|line| line == "bird 1.0.0 carries native code"));
    }
}

#[test]
fn allowed_native_sync_reports_even_when_unchanged() {
    let tree = Tree::new("sync", true);
    for _ in 0..2 {
        let report = install::sync(&tree.app()).unwrap();
        assert!(report.lines.iter().any(|line| line == "bird 1.0.0 carries native code"));
    }
    assert!(std::fs::read_to_string(tree.app().join("blade.lock")).unwrap().contains("native = true"));
    assert!(!tree.app().join("target").exists());
    tree.root_manifest(false);
    assert!(install::sync(&tree.app()).unwrap_err().contains("allow"));
}

#[test]
fn fixture_lock_is_pinned_by_the_workspace() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let identities = |text: String| text.split("[[package]]").skip(1)
        .map(|p| p.split("dependencies =").next().unwrap().trim().to_owned())
        .collect::<std::collections::HashSet<_>>();
    let pinned = identities(std::fs::read_to_string(workspace.join("Cargo.lock")).unwrap());
    for package in identities(include_str!("fixtures/native/native/Cargo.lock").to_owned()) {
        assert!(package.starts_with("name = \"blade-native-fixture\"") || pinned.contains(&package), "{package}");
    }
}

#[test]
fn cli_build_run_and_no_native() {
    let tree = Tree::new("cli", true);
    let output = success(tree.blade().arg("sync").env("CARGO", "/no-cargo").output().unwrap());
    assert!(output.contains("bird 1.0.0 carries native code"), "{output}");
    assert_eq!(tree.builds(), 0);
    let locked = Lock::read(&tree.app()).unwrap();
    let disabled = tree.blade().args(["run", "--no-native"]).env("CARGO", "/no-cargo").output().unwrap();
    let error = String::from_utf8_lossy(&disabled.stderr);
    assert!(!disabled.status.success() && error.contains("bird/native@0::answer/0\" is not registered"), "{error}");
    assert!(!tree.app().join("target/native").exists());
    success(tree.blade().arg("build").output().unwrap());
    success(tree.blade().arg("run").output().unwrap());
    assert_eq!(tree.builds(), 1);
    assert_eq!(Lock::read(&tree.app()).unwrap(), locked);
    let invoked = std::fs::read_to_string(tree.0.join("cargo.log")).unwrap();
    assert!(invoked.contains("build --release --locked") && invoked.contains("--target-dir"), "{invoked}");
    let target = tree.app().join("target/native/bird");
    let stamp_path = target.join(".blade-stamp");
    let stamp = std::fs::read(&stamp_path).unwrap();
    let modified = std::fs::metadata(&stamp_path).unwrap().modified().unwrap();
    success(tree.blade().arg("build").output().unwrap());
    assert_eq!(std::fs::metadata(&stamp_path).unwrap().modified().unwrap(), modified);
    assert_eq!(std::fs::read(&stamp_path).unwrap(), stamp);
    assert_eq!(tree.builds(), 1);
    let source = std::fs::read_to_string(tree.0.join("bird/native/src/lib.rs")).unwrap();
    tree.write("bird/native/src/lib.rs", &source.replace("42", "43"));
    success(tree.blade().arg("sync").output().unwrap());
    tree.write("app/src/main.ok", "module app; import \"pkg/bird/src/native\" take answer; main() is int { confirm answer() == 43; 0; }");
    success(tree.blade().arg("run").output().unwrap());
    assert_ne!(std::fs::read(&stamp_path).unwrap(), stamp);
    assert_eq!(tree.builds(), 2);
    let current = Lock::read(&tree.app()).unwrap();
    assert_ne!(current.packages["bird"].checksum, locked.packages["bird"].checksum);
    let library = target.join(format!("extension.{}", std::env::consts::DLL_EXTENSION));
    std::fs::write(&library, "corrupted library").unwrap();
    success(tree.blade().arg("run").output().unwrap());
    assert_eq!(Lock::read(&tree.app()).unwrap(), current);
    assert_eq!(tree.builds(), 3);
    success(tree.blade().arg("build").env("BLADE_TEST_CARGO_VERSION", "cargo changed").output().unwrap());
    assert_eq!(tree.builds(), 4);
    let disabled = tree.blade().args(["run", "--no-native"]).env("CARGO", "/no-cargo").output().unwrap();
    assert!(String::from_utf8_lossy(&disabled.stderr).contains("is not registered"));
    assert_eq!(tree.builds(), 4);
    tree.root_manifest(false);
    let refused = tree.blade().arg("build").env("CARGO", "/no-cargo").output().unwrap();
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("allow"));
    tree.root_manifest(true);
    let mut tampered = current;
    tampered.packages.get_mut("bird").unwrap().native = false;
    tampered.write(&tree.app()).unwrap();
    let refused = tree.blade().arg("run").env("CARGO", "/no-cargo").output().unwrap();
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("native mark differs"));
    let report = success(tree.blade().arg("sync").output().unwrap());
    assert!(!report.contains("files changed"), "{report}");
    tree.write("app/pkg/bird/native/src/lib.rs", "invalid Rust");
    let failed = tree.blade().arg("run").output().unwrap();
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("cargo build --release --locked failed"));
    assert!(!stamp_path.exists());
    let help = success(tree.blade().args(["run", "--help"]).output().unwrap());
    assert!(help.contains("--no-native"));
}

#[test]
fn native_crates_must_be_inside_the_package_and_build_a_cdylib() {
    let tree = Tree::new("crate-validation", true);
    for crate_path in ["../bird/native", "/tmp/native"] {
        tree.write("bird/blade.toml", &format!("[module]\nname = \"bird\"\nversion = \"1.0.0\"\n[native]\ncrate = \"{crate_path}\"\n"));
        assert!(install::sync(&tree.app()).unwrap_err().contains("inside the package"));
    }
    tree.write("bird/blade.toml", include_str!("fixtures/native/blade.toml"));
    tree.write("bird/native/Cargo.toml", "[package]\nname = \"bird\"\nversion = \"1.0.0\"\n[lib]\ncrate-type = [\"rlib\"]\n");
    assert!(install::sync(&tree.app()).unwrap_err().contains("cdylib"));
    tree.write("bird/blade.toml", "[module]\nname = \"bird\"\nversion = \"1.0.0\"\n[native]\nallow = [\"bird\"]\n");
    assert!(install::sync(&tree.app()).unwrap_err().contains("declares a native"));
}

/// The directory with the built `blade` and `okkama`: OKKAMA_TEST_BIN_DIR, or the nearest ancestor of this test
/// binary that holds both (cargo test puts it in target/<profile>/deps; other runners may nest it deeper).
fn bin_dir() -> std::path::PathBuf {
    let exe = |dir: &std::path::Path, name: &str| dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    if let Some(dir) = std::env::var_os("OKKAMA_TEST_BIN_DIR") {
        let dir = std::path::PathBuf::from(dir);
        let listing: Vec<_> = std::fs::read_dir(&dir).map(|entries| entries.flatten().map(|e| e.file_name()).collect()).unwrap_or_default();
        assert!(exe(&dir, "blade").is_file() && exe(&dir, "okkama").is_file(), "OKKAMA_TEST_BIN_DIR {} holds {listing:?}", dir.display());
        return dir;
    }
    let me = std::env::current_exe().unwrap();
    let found = me.ancestors().skip(1).find(|dir| exe(dir, "blade").is_file() && exe(dir, "okkama").is_file());
    found.map(std::path::Path::to_path_buf).unwrap_or_else(|| {
        panic!("no built blade and okkama above {}: build blade-cli and okkama-cli first, or set OKKAMA_TEST_BIN_DIR", me.display())
    })
}
