//! Host paths and directory snapshots, scoped to each VM's working directory.
use crate::host::{
    Error,
    ProviderError,
    native_fs::{fail, host},
    *,
};
use okkama_runtime_abi::Native;
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::{fs, path::Path};
fn names(path: &Path) -> std::io::Result<Vec<String>> {
    let mut out = fs::read_dir(path)?
        .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect::<Result<Vec<_>, _>>()?;
    out.sort();
    Ok(out)
}
pub(crate) fn handles(n: Native) -> bool {
    use Native::*;
    matches!(
        n,
        FsCwd
            | FsChdir
            | FsMkdir
            | FsMkdirAll
            | FsRmdir
            | FsUnlink
            | FsRename
            | FsRenameNoreplace
            | FsCopyFile
            | FsLink
            | FsSymlink
            | FsReadlink
            | FsChmod
            | FsStat
            | FsLstat
            | FsDirectoryEntries
            | FsDirectoryRemoveTree
            | FsFileContent
            | FsDirectorySync
    )
}
/// fsync of the directory at `path`, so that a name made or replaced in it
/// survives a crash (#122). The directory is opened with O_DIRECTORY, so a
/// file is refused; `sync_all` is F_FULLFSYNC on macOS. Hosts without POSIX
/// directories answer `Unsupported`.
fn sync_directory(path: &Path) -> std::io::Result<()> {
    crate::host::native_fs::injected_fault("directory_sync")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY)
            .open(path)?
            .sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
    }
}
impl Heap {
    pub(crate) fn directory_execute(
        &mut self,
        n: Native,
        a: &[Slot],
    ) -> Result<Slot, ProviderError> {
        use Native::*;
        let yes = Slot::boolean(true);
        if n == FsCwd {
            let path = self.working_directory()?;
            return Ok(self.native_byte_array(path.to_string_lossy().as_bytes().to_vec())?);
        }
        let path = self.fs_path(n, a[0])?;
        match n {
            FsChdir => {
                let real = fs::canonicalize(path).map_err(|e| host(n, e))?;
                if !real.is_dir() {
                    return Err(fail(
                        n,
                        "invalid_input",
                        "Working directory must be a directory",
                    ));
                }
                #[cfg(unix)]
                nix::unistd::access(&real, nix::unistd::AccessFlags::X_OK)
                    .map_err(|e| host(n, e.into()))?;
                okkama_runtime_core::host::set_cwd(real);
            }
            FsMkdir | FsMkdirAll | FsChmod => {
                let mode = self
                    .int64(a[1])?
                    .filter(|v| (0..=0o777).contains(v))
                    .ok_or_else(|| fail(n, "invalid_input", "Invalid permission mode"))?
                    as u32;
                #[cfg(unix)]
                {
                    if n == FsMkdir || n == FsMkdirAll {
                        // mkdir_all makes every missing parent; an existing
                        // directory is then no failure (#125).
                        fs::DirBuilder::new().recursive(n == FsMkdirAll).mode(mode).create(path)
                    } else {
                        fs::set_permissions(path, fs::Permissions::from_mode(mode))
                    }
                    .map_err(|e| host(n, e))?;
                }
                #[cfg(not(unix))]
                {
                    let _ = (mode, path);
                    return Err(fail(
                        n,
                        "unsupported",
                        "Filesystem permission mode unavailable",
                    ));
                }
            }
            FsRmdir => {
                if fs::symlink_metadata(&path).is_ok_and(|m| !m.is_dir()) {
                    return Err(fail(n, "invalid_input", "Not a directory"));
                }
                fs::remove_dir(path).map_err(|e| host(n, e))?;
            }
            FsUnlink => {
                if fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
                    return Err(fail(n, "permission_denied", "Cannot unlink directory"));
                }
                fs::remove_file(path).map_err(|e| host(n, e))?;
            }
            FsRename | FsRenameNoreplace | FsLink => {
                let target = self.fs_path(n, a[1])?;
                if n == FsRename {
                    fs::rename(path, target)
                } else if n == FsRenameNoreplace {
                    okkama_runtime_core::native::fs::rename_noreplace(&path, &target)
                } else {
                    fs::hard_link(path, target)
                }
                .map_err(|e| host(n, e))?;
            }
            FsCopyFile => {
                let target = self.fs_path(n, a[1])?;
                okkama_runtime_core::native::fs::copy_file(&path, &target, a[2].bits != 0)
                    .map_err(|e| host(n, e))?;
            }
            FsSymlink => {
                let raw = self.native_bytes(a[0])?;
                let target = String::from_utf8_lossy(&raw);
                let link = self.fs_path(n, a[1])?;
                #[cfg(unix)]
                std::os::unix::fs::symlink(target.as_ref(), link).map_err(|e| host(n, e))?;
                #[cfg(not(unix))]
                {
                    let _ = (target, link);
                    return Err(fail(n, "unsupported", "Symlink creation unavailable"));
                }
            }
            FsReadlink => {
                let target = fs::read_link(path).map_err(|e| host(n, e))?;
                return Ok(self.native_byte_array(target.to_string_lossy().as_bytes().to_vec())?);
            }
            FsDirectoryRemoveTree => {
                let meta = fs::symlink_metadata(&path).map_err(|e| host(n, e))?;
                if !meta.is_dir() {
                    return Err(fail(n, "invalid_input", "Tree root must be a directory"));
                }
                fs::remove_dir_all(path).map_err(|e| host(n, e))?;
            }
            FsDirectorySync => sync_directory(&path).map_err(|e| host(n, e))?,
            FsFileContent => {
                return Ok(self.native_byte_array(fs::read(path).map_err(|e| host(n, e))?)?);
            }
            FsDirectoryEntries => {
                let names = names(&path).map_err(|e| host(n, e))?;
                let values = names
                    .into_iter()
                    .map(|s| self.allocate_text(s))
                    .collect::<Result<Vec<_>, _>>()?;
                return Ok(self.allocate_array(values)?);
            }
            FsStat | FsLstat => {
                let meta = if n == FsStat {
                    fs::metadata(path)
                } else {
                    fs::symlink_metadata(path)
                }
                .map_err(|e| host(n, e))?;
                #[cfg(unix)]
                {
                    let kind = match meta.mode() & 0o170000 {
                        0o100000 => 1,
                        0o040000 => 2,
                        0o120000 => 3,
                        0o060000 => 4,
                        0o020000 => 5,
                        0o010000 => 6,
                        0o140000 => 7,
                        _ => 0,
                    };
                    let pairs = [
                        ("kind", kind),
                        ("size", meta.size() as i64),
                        ("permissions", i64::from(meta.mode() & 0o777)),
                        ("accessed_sec", meta.atime()),
                        ("modified_sec", meta.mtime()),
                        ("changed_sec", meta.ctime()),
                        ("created_sec", -1),
                        ("device", meta.dev() as i64),
                        ("inode", meta.ino() as i64),
                    ];
                    let entries = pairs
                        .into_iter()
                        .map(|(k, v)| self.allocate_text(k.into()).map(|k| (k, Slot::small(v))))
                        .collect::<Result<Vec<_>, _>>()?;
                    return Ok(self.map_new(entries.into_iter())?);
                }
                #[cfg(not(unix))]
                {
                    let _ = meta;
                    return Err(fail(n, "unsupported", "POSIX metadata unavailable"));
                }
            }
            _ => return Err(Error("unsupported directory native".into()).into()),
        }
        Ok(yes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #122: a directory is synchronized; a file or a missing path is
    /// refused with the host's failure.
    #[test]
    fn sync_directory_takes_a_directory_only() {
        let root = std::env::temp_dir().join(format!("sparrow-directory-sync-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        fs::write(root.join("file"), b"x").unwrap();
        sync_directory(&root).unwrap();
        let file = sync_directory(&root.join("file")).unwrap_err();
        assert_eq!(file.kind(), std::io::ErrorKind::NotADirectory);
        let missing = sync_directory(&root.join("missing")).unwrap_err();
        assert_eq!(missing.kind(), std::io::ErrorKind::NotFound);
        fs::remove_dir_all(&root).unwrap();
    }
}
