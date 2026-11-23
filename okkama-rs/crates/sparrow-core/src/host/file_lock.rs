//! Whole-file locks that belong to one open file description, not to the
//! process (#1010): a POSIX `fcntl` record lock is dropped when the process
//! closes *any* descriptor of the file, and the host opens one for every
//! `File`, so reading the same path through another `File` used to release a
//! lock another process relied on. Linux uses open file description locks
//! (`F_OFD_SETLK`), which conflict with the POSIX locks of other processes
//! (`lockf`, `fcntl`) as before. macOS has no OFD locks; there `flock(2)`
//! locks belong to the description too, and the kernel makes them conflict
//! with `fcntl` locks and shows them to `F_GETLK`. A process-wide reservation
//! prevents overlapping locks even when independent VM hosts use different
//! threads: two descriptions of one process would otherwise block each other.
use crate::host::{
    ProviderError,
    native_fs::{fail, host},
};
use okkama_runtime_abi::Native;
use std::{
    collections::HashMap,
    fs::File,
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    sync::{Mutex, OnceLock},
};
/// The locks of this process, and whether each is shared.
static LOCKS: OnceLock<Mutex<HashMap<(u64, u64), bool>>> = OnceLock::new();
#[derive(Debug)]
pub(crate) struct Lock {
    key: (u64, u64),
}
fn table() -> &'static Mutex<HashMap<(u64, u64), bool>> {
    LOCKS.get_or_init(Mutex::default)
}
/// Take (`F_RDLCK`, `F_WRLCK`) or drop (`F_UNLCK`) the lock of `file`'s open
/// file description over the whole file.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn command(file: &File, kind: i16, wait: bool) -> std::io::Result<()> {
    let record = libc::flock {
        l_start: 0,
        l_len: 0,
        // Required for open file description locks.
        l_pid: 0,
        l_type: kind,
        l_whence: libc::SEEK_SET as _,
    };
    let arg = if wait {
        nix::fcntl::FcntlArg::F_OFD_SETLKW(&record)
    } else {
        nix::fcntl::FcntlArg::F_OFD_SETLK(&record)
    };
    nix::fcntl::fcntl(file.as_raw_fd(), arg)
        .map(|_| ())
        .map_err(Into::into)
}
/// Take (`F_RDLCK`, `F_WRLCK`) or drop (`F_UNLCK`) the lock of `file`'s open
/// file description over the whole file.
// F_RDLCK and F_WRLCK are c_short on macOS and c_int on Linux.
#[allow(clippy::unnecessary_cast)]
#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn command(file: &File, kind: i16, wait: bool) -> std::io::Result<()> {
    let operation = match i32::from(kind) {
        k if k == libc::F_RDLCK as i32 => libc::LOCK_SH,
        k if k == libc::F_WRLCK as i32 => libc::LOCK_EX,
        _ => libc::LOCK_UN,
    } | if wait { 0 } else { libc::LOCK_NB };
    // SAFETY: `flock` reads only its two integer arguments; the descriptor is
    // owned by `file` and open for the whole call.
    if unsafe { libc::flock(file.as_raw_fd(), operation) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}
impl Lock {
    pub(crate) fn acquire(
        n: Native,
        file: &File,
        shared: bool,
        wait: bool,
    ) -> Result<Option<Self>, ProviderError> {
        let meta = file.metadata().map_err(|e| host(n, e))?;
        let key = (meta.dev(), meta.ino());
        // A failed reservation leaves the lock already in the table as it was:
        // `held` reads its kind from there (#1010).
        match table().lock().unwrap_or_else(|e| e.into_inner()).entry(key) {
            std::collections::hash_map::Entry::Occupied(_) => {
                return Err(fail(
                    n,
                    "invalid_input",
                    "File lock overlaps an existing lock",
                ));
            }
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(shared);
            }
        }
        let lock = Self { key };
        match command(
            file,
            if shared {
                libc::F_RDLCK as _
            } else {
                libc::F_WRLCK as _
            },
            wait,
        ) {
            Ok(()) => Ok(Some(lock)),
            Err(e) if !wait && matches!(e.raw_os_error(), Some(libc::EAGAIN | libc::EACCES)) => {
                Ok(None)
            }
            Err(e) => Err(host(n, e)),
        }
    }
    /// Whether `file` is locked now, `shared` or exclusively: this process's
    /// locks come from the table, another process's from F_GETLK, which
    /// reports the kind of the lock in the way and never the caller's own.
    // F_RDLCK and F_WRLCK are c_short on macOS and c_int on Linux.
    #[allow(clippy::unnecessary_cast)]
    pub(crate) fn held(n: Native, file: &File, shared: bool) -> Result<bool, ProviderError> {
        let meta = file.metadata().map_err(|e| host(n, e))?;
        let key = (meta.dev(), meta.ino());
        if let Some(own) = table().lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            return Ok(*own == shared);
        }
        let mut record = libc::flock {
            l_start: 0,
            l_len: 0,
            l_pid: 0,
            l_type: libc::F_WRLCK as _,
            l_whence: libc::SEEK_SET as _,
        };
        // Linux: F_OFD_GETLK sees the OFD and the POSIX locks of others.
        // macOS: F_GETLK also reports their `flock` locks.
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let query = nix::fcntl::FcntlArg::F_OFD_GETLK(&mut record);
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        let query = nix::fcntl::FcntlArg::F_GETLK(&mut record);
        nix::fcntl::fcntl(file.as_raw_fd(), query).map_err(|e| host(n, e.into()))?;
        let kind = i32::from(record.l_type);
        Ok(if shared { kind == libc::F_RDLCK as i32 } else { kind == libc::F_WRLCK as i32 })
    }
    pub(crate) fn release(self, file: &File) -> std::io::Result<()> {
        command(file, libc::F_UNLCK as _, false)
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        table()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.key);
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("sparrow-lock-{tag}-{}", std::process::id()));
        std::fs::write(&path, b"x").unwrap();
        path
    }

    fn open(path: &std::path::Path) -> File {
        File::options().read(true).write(true).open(path).unwrap()
    }

    fn busy(result: std::io::Result<()>) -> bool {
        matches!(result, Err(e) if matches!(e.raw_os_error(), Some(libc::EAGAIN | libc::EACCES)))
    }

    /// #1010: closing another descriptor of the same file keeps the lock;
    /// another open file description still finds it in the way.
    #[test]
    fn closing_another_descriptor_keeps_the_lock() {
        let path = scratch("close");
        let owner = open(&path);
        let lock = Lock::acquire(Native::FsLockExclusive, &owner, false, false).unwrap().expect("free");
        drop(File::open(&path).unwrap());
        let other = open(&path);
        assert!(busy(command(&other, libc::F_WRLCK as _, false)), "the lock survives the close");
        assert!(busy(command(&other, libc::F_RDLCK as _, false)));
        lock.release(&owner).unwrap();
        command(&other, libc::F_WRLCK as _, false).unwrap();
        command(&other, libc::F_UNLCK as _, false).unwrap();
        let _ = std::fs::remove_file(&path);
    }

    /// #1010: a refused lock of another kind leaves the lock in the table,
    /// so `held` keeps answering for the lock that is really held.
    #[test]
    fn a_refused_lock_leaves_the_held_one_as_it_was() {
        let path = scratch("table");
        let owner = open(&path);
        let lock = Lock::acquire(Native::FsLockExclusive, &owner, false, false).unwrap().expect("free");
        let other = open(&path);
        assert!(Lock::acquire(Native::FsLockShared, &other, true, false).is_err());
        assert!(Lock::held(Native::FsLockExclusive, &other, false).unwrap(), "still exclusive");
        assert!(!Lock::held(Native::FsLockShared, &other, true).unwrap());
        lock.release(&owner).unwrap();
        let _ = std::fs::remove_file(&path);
    }
}
