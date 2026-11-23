//! File operations Sparrow's `runtime/fs` provider (`host/native_directory.rs`)
//! calls directly: `rename_noreplace` and `copy_file`. The `runtime/fs` natives
//! themselves are Sparrow's own providers; none of them reach
//! `native::execute`.
#![allow(unsafe_code)]

/// `rename` that refuses an existing target with `AlreadyExists` instead of
/// replacing it (#126): `renameat2(RENAME_NOREPLACE)` on Linux,
/// `renamex_np(RENAME_EXCL)` on macOS. Where neither works, a file moves by
/// link and unlink, which refuse an existing target too, and a directory is
/// checked first.
pub fn rename_noreplace(source: &std::path::Path, target: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = |path: &std::path::Path| {
        std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "path holds NUL"))
    };
    let (from, to) = (c_path(source)?, c_path(target)?);
    #[cfg(target_os = "linux")]
    {
        // The system call, not glibc's wrapper: the Linux packages are linked
        // against glibc 2.17, which has no renameat2 (2.28; #545). Kernels
        // before 3.15 answer ENOSYS and the fallback below takes over.
        // SAFETY: both paths are NUL-terminated strings that outlive the call.
        let result = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                libc::AT_FDCWD,
                from.as_ptr(),
                libc::AT_FDCWD,
                to.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if result == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if !matches!(error.raw_os_error(), Some(libc::EINVAL | libc::ENOSYS)) {
            return Err(error);
        }
    }
    #[cfg(target_os = "macos")]
    {
        // SAFETY: both paths are NUL-terminated strings that outlive the call.
        let result = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
        if result == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if !matches!(error.raw_os_error(), Some(libc::EINVAL | libc::ENOTSUP)) {
            return Err(error);
        }
    }
    let _ = (from, to);
    if !std::fs::symlink_metadata(source)?.is_dir() {
        std::fs::hard_link(source, target)?;
        return std::fs::remove_file(source);
    }
    if std::fs::symlink_metadata(target).is_ok() {
        return Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists));
    }
    std::fs::rename(source, target)
}

/// A copy of the file at `source` as `target`, with its permission bits
/// (#126). An existing target is `AlreadyExists` unless `replace`; then the
/// copy goes to a fresh sibling first and is renamed over the target, so a
/// failed copy leaves the old file whole. A failed copy leaves no new file.
pub fn copy_file(source: &std::path::Path, target: &std::path::Path, replace: bool) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut from = std::fs::File::open(source)?;
    let metadata = from.metadata()?;
    if metadata.is_dir() {
        return Err(std::io::Error::new(std::io::ErrorKind::IsADirectory, "a directory is not copied as a file"));
    }
    let written = if replace {
        static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let name = target.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let serial = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        target.with_file_name(format!(".{name}.{}-{serial}.copy", std::process::id()))
    } else {
        target.to_path_buf()
    };
    let mut to = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&written)?;
    let copied = std::io::copy(&mut from, &mut to)
        .and_then(|_| to.set_permissions(metadata.permissions()))
        .and_then(|()| if replace { std::fs::rename(&written, target) } else { Ok(()) });
    if copied.is_err() {
        drop(to);
        let _ = std::fs::remove_file(&written);
    }
    copied
}

#[cfg(test)]
mod tests {
    use super::{copy_file, rename_noreplace};
    use std::os::unix::fs::PermissionsExt;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("okkama-fs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch directory");
        dir
    }

    #[test]
    fn rename_noreplace_moves_and_refuses_an_existing_target() {
        let dir = scratch("rename");
        let (source, target) = (dir.join("a"), dir.join("b"));
        std::fs::write(&source, b"one").unwrap();
        rename_noreplace(&source, &target).expect("fresh target");
        assert!(!source.exists());
        assert_eq!(std::fs::read(&target).unwrap(), b"one");
        std::fs::write(&source, b"two").unwrap();
        let error = rename_noreplace(&source, &target).expect_err("existing target");
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&source).unwrap(), b"two");
        assert_eq!(std::fs::read(&target).unwrap(), b"one");
        let (from, to) = (dir.join("d"), dir.join("e"));
        std::fs::create_dir(&from).unwrap();
        std::fs::create_dir(&to).unwrap();
        assert!(rename_noreplace(&from, &to).is_err(), "a directory never replaces one");
        assert!(from.is_dir());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn copy_file_keeps_permissions_and_replaces_only_when_asked() {
        let dir = scratch("copy");
        let (source, target) = (dir.join("a"), dir.join("b"));
        std::fs::write(&source, b"data").unwrap();
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o640)).unwrap();
        copy_file(&source, &target, false).expect("fresh target");
        assert_eq!(std::fs::read(&target).unwrap(), b"data");
        assert_eq!(std::fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o640);
        std::fs::write(&source, b"newer").unwrap();
        let error = copy_file(&source, &target, false).expect_err("existing target");
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&target).unwrap(), b"data");
        copy_file(&source, &target, true).expect("replace");
        assert_eq!(std::fs::read(&target).unwrap(), b"newer");
        let names: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names.len(), 2, "no temporary copy is left behind: {names:?}");
        let error = copy_file(&dir, &dir.join("c"), false).expect_err("a directory");
        assert_eq!(error.kind(), std::io::ErrorKind::IsADirectory);
        assert!(!dir.join("c").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
