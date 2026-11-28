//! Root consent and source builds for native packages. Installation never runs Cargo.

use std::path::{Component, Path, PathBuf};
use std::process::Command;

use crate::install::{checksum_of, files};
use crate::lock::Lock;
use crate::manifest::{Manifest, PKG};

pub(crate) fn check_crate_path(path: &str) -> Result<(), String> {
    if path.is_empty() || path.contains('\\')
        || !Path::new(path).components().all(|part| matches!(part, Component::Normal(_)))
    {
        return Err("[native] crate must be a directory inside the package".into());
    }
    Ok(())
}

pub(crate) fn crate_dir(package: &Path, path: &str) -> Result<PathBuf, String> {
    check_crate_path(path)?;
    let root = package.canonicalize().map_err(|error| format!("{}: {error}", package.display()))?;
    let dir = root.join(path).canonicalize().map_err(|error| format!("[native] crate {path}: {error}"))?;
    if !dir.starts_with(&root) {
        return Err("[native] crate must stay inside the package".into());
    }
    let manifest = dir.join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest).map_err(|error| format!("{}: {error}", manifest.display()))?;
    let cargo: toml::Table = text.parse().map_err(|error| format!("{}: {error}", manifest.display()))?;
    let cdylib = cargo.get("lib").and_then(|lib| lib.get("crate-type"))
        .and_then(toml::Value::as_array)
        .is_some_and(|types| types.iter().any(|kind| kind.as_str() == Some("cdylib")));
    if !cdylib {
        return Err(format!("{}: [lib] crate-type must include cdylib", manifest.display()));
    }
    Ok(dir)
}

pub(crate) fn consent(manifest: &Manifest, lock: &Lock) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();
    let mut refused = Vec::new();
    for (name, package) in &lock.packages {
        if package.native {
            lines.push(format!("{name} {} carries native code", package.version));
            if !manifest.native_allow.contains(name) {
                refused.push(name.as_str());
            }
        }
    }
    if !refused.is_empty() {
        return Err(format!("{}\nnative packages missing from root [native] allow: {}", lines.join("\n"), refused.join(", ")));
    }
    Ok(lines)
}

fn target(root: &Path, name: &str) -> PathBuf {
    root.join("target/native").join(name)
}

fn library(target: &Path) -> PathBuf {
    target.join(format!("extension.{}", std::env::consts::DLL_EXTENSION))
}

/// Check every package before starting any build scripts.
pub(crate) fn build(root: &Path) -> Result<Vec<PathBuf>, String> {
    let root = root.canonicalize().map_err(|error| format!("{}: {error}", root.display()))?;
    let root = root.as_path();
    let manifest = Manifest::read(root)?;
    let lock = Lock::read(root)?;
    consent(&manifest, &lock)?;
    let mut crates = Vec::new();
    for (name, locked) in &lock.packages {
        crate::manifest::check_package_name(name)?;
        let package = root.join(PKG).join(name);
        let installed = Manifest::read_package(&package)?;
        if locked.native != installed.native.is_some() {
            return Err(format!("{name}: native mark differs from blade.lock; run `blade sync`"));
        }
        if let Some(path) = &installed.native {
            crates.push((name, package.clone(), crate_dir(&package, path)?));
        }
    }
    let mut libraries = Vec::new();
    for (name, package, dir) in crates {
        libraries.push(build_crate(root, name, &package, &dir)
            .map_err(|error| format!("native package {name}: {error}"))?);
    }
    Ok(libraries)
}

fn version(tool: &std::ffi::OsStr, dir: &Path) -> Result<String, String> {
    let output = Command::new(tool).arg("--version").current_dir(dir).output()
        .map_err(|error| format!("cannot run {}: {error}", tool.to_string_lossy()))?;
    if !output.status.success() {
        return Err(format!("{} --version failed: {}", tool.to_string_lossy(), String::from_utf8_lossy(&output.stderr)));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn stamp(inputs: &str, cargo: &str, rustc: &str, target: &Path) -> Result<String, String> {
    let file = library(target).file_name().unwrap().to_string_lossy().into_owned();
    let digest = checksum_of(target, &[file])?;
    Ok(format!("blade-native-stamp 1\ncargo {cargo}\nrustc {rustc}\ninputs {inputs}\nlibrary {digest}\n"))
}

fn build_crate(root: &Path, name: &str, package: &Path, dir: &Path) -> Result<PathBuf, String> {
    let target = target(root, name);
    let lib = library(&target);
    let stamp_file = target.join(".blade-stamp");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let cargo_version = version(&cargo, dir)?;
    let rustc_version = version(&rustc, dir)?;
    // Include build scripts and their data, not only .rs sources.
    let inputs = checksum_of(package, &files(package)?)?;
    if lib.is_file() && let Ok(recorded) = std::fs::read_to_string(&stamp_file)
        && recorded == stamp(&inputs, &cargo_version, &rustc_version, &target)?
    {
        return Ok(lib);
    }
    std::fs::create_dir_all(&target).map_err(|error| format!("{}: {error}", target.display()))?;
    if let Err(error) = std::fs::remove_file(&stamp_file)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(format!("{}: {error}", stamp_file.display()));
    }
    let target = target.canonicalize().map_err(|error| error.to_string())?;
    let output = Command::new(&cargo).current_dir(dir)
        .args(["build", "--release", "--locked", "--lib", "--message-format=json-render-diagnostics"])
        .arg("--target-dir").arg(&target).output()
        .map_err(|error| format!("cannot run {}: {error}", cargo.to_string_lossy()))?;
    let mut artifact = None;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if message["reason"] == "compiler-message" {
            if let Some(rendered) = message["message"]["rendered"].as_str() {
                eprint!("{rendered}");
            }
        } else if message["reason"] == "compiler-artifact"
            && message["manifest_path"].as_str().is_some_and(|path| Path::new(path) == dir.join("Cargo.toml"))
            && message["target"]["crate_types"].as_array()
                .is_some_and(|types| types.iter().any(|kind| kind == "cdylib"))
            && let Some(paths) = message["filenames"].as_array()
        {
            artifact = paths.iter().filter_map(|path| path.as_str()).map(PathBuf::from)
                .find(|path| path.extension().is_some_and(|ext| ext == std::env::consts::DLL_EXTENSION));
        }
    }
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    if !output.status.success() {
        return Err(format!("cargo build --release --locked failed ({})", output.status));
    }
    let artifact = artifact.ok_or("cargo did not produce a cdylib for the package crate")?;
    let artifact = artifact.canonicalize().map_err(|error| format!("{}: {error}", artifact.display()))?;
    if !artifact.starts_with(&target) {
        return Err("cargo produced a library outside target/native".into());
    }
    std::fs::copy(&artifact, &lib).map_err(|error| format!("{}: {error}", lib.display()))?;
    std::fs::write(&stamp_file, stamp(&inputs, &cargo_version, &rustc_version, &target)?)
        .map_err(|error| format!("{}: {error}", stamp_file.display()))?;
    Ok(lib)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_consent_is_exact_and_covers_the_entire_lock() {
        let lock = Lock::parse("[[package]]\nname = \"acme/bird\"\nversion = \"1.0.0\"\nsource = \"path+bird\"\nnative = true\n").unwrap();
        for allow in ["", "acme", "acme/Bird", "acme/bird/extra"] {
            let manifest = Manifest { native_allow: vec![allow.into()], ..Default::default() };
            let error = consent(&manifest, &lock).unwrap_err();
            assert!(error.contains("acme/bird 1.0.0 carries native code"));
            assert!(error.contains("root [native] allow"));
        }
        let manifest = Manifest { native_allow: vec!["acme/bird".into()], ..Default::default() };
        assert_eq!(consent(&manifest, &lock).unwrap(), ["acme/bird 1.0.0 carries native code"]);
        assert!(consent(&Manifest::default(), &Lock::default()).unwrap().is_empty());
    }

    #[test]
    fn native_stamp_covers_sources_both_tools_and_library_bytes() {
        let target = std::env::temp_dir().join(format!("blade-native-stamp-{}", std::process::id()));
        std::fs::create_dir_all(&target).unwrap();
        let lib = library(&target);
        std::fs::write(&lib, "first").unwrap();
        let original = stamp("sources", "cargo 1", "rustc 1", &target).unwrap();
        assert_eq!(stamp("sources", "cargo 1", "rustc 1", &target).unwrap(), original);
        for (inputs, cargo, rustc) in [("changed", "cargo 1", "rustc 1"), ("sources", "cargo 2", "rustc 1"), ("sources", "cargo 1", "rustc 2")] {
            assert_ne!(stamp(inputs, cargo, rustc, &target).unwrap(), original);
        }
        std::fs::write(&lib, "other").unwrap();
        assert_ne!(stamp("sources", "cargo 1", "rustc 1", &target).unwrap(), original);
        std::fs::remove_dir_all(target).unwrap();
    }
}
