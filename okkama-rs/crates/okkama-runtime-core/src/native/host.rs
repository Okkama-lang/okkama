//! Host natives Sparrow routes here (`host/native_origin.rs`): `runtime/system`
//! (host name, CPU count, storage; the `secure_*` randomness is `secure`'s, #1125). Clocks, sleep, platform, architecture
//! and byte order are Sparrow's own (`host/mod.rs`, `natives.rs`).
use super::support::{failure, int_value, string, string_map, text};
use super::Error;
use crate::{IntValue, Value, abi::Native};

fn operation(native: Native) -> &'static str {
    native.name()
}

/// gethostname(2): the host's name as the system knows it (#950; it ran `hostname` before).
#[allow(unsafe_code)]
fn system_host_name() -> Option<String> {
    let mut buffer = [0u8; 256];
    // SAFETY: the buffer is writable for its whole length; gethostname writes at most that many bytes.
    if unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) } != 0 {
        return None;
    }
    let end = buffer.iter().position(|byte| *byte == 0).unwrap_or(buffer.len());
    Some(String::from_utf8_lossy(&buffer[..end]).trim().to_string())
}

fn host_name(native: Native) -> Result<String, Error> {
    system_host_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| failure("core.system", "unexpected", native, "Host name is unavailable"))
}

/// Processors available to the runtime, at least one. Only the system decides:
/// the Windows variable `NUMBER_OF_PROCESSORS`, which the Java port read on
/// every platform, does not override it (#1013).
fn cpu_count() -> i64 {
    std::thread::available_parallelism().map_or(1, |count| count.get().max(1) as i64)
}

/// The size of a file system's block and its counts of blocks: all of them
/// and those an unprivileged writer may still use.
struct BlockCounts {
    block: u128,
    total: u128,
    available: u128,
}

/// statvfs(3) of `path`.
#[cfg(not(target_os = "macos"))]
#[allow(unsafe_code)]
fn file_system_stats(path: &std::ffi::CStr) -> std::io::Result<BlockCounts> {
    // SAFETY: an all-zero statvfs is a valid value of this plain C struct.
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `path` is NUL-terminated and `stats` is a writable statvfs.
    if unsafe { libc::statvfs(path.as_ptr(), &mut stats) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(BlockCounts {
        block: u128::from(stats.f_frsize),
        total: u128::from(stats.f_blocks),
        available: u128::from(stats.f_bavail),
    })
}

/// statfs(2) of `path`. On macOS `statvfs` is a wrapper over `statfs` with
/// 32-bit block counts, wrong for a volume of more than 2^32 blocks (#1013);
/// `statfs` counts in 64 bits.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn file_system_stats(path: &std::ffi::CStr) -> std::io::Result<BlockCounts> {
    // SAFETY: an all-zero statfs is a valid value of this plain C struct.
    let mut stats: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: `path` is NUL-terminated and `stats` is a writable statfs.
    if unsafe { libc::statfs(path.as_ptr(), &mut stats) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(BlockCounts {
        block: u128::from(stats.f_bsize),
        total: u128::from(stats.f_blocks),
        available: u128::from(stats.f_bavail),
    })
}

fn storage_usage(native: Native, path: &str) -> Result<Value, Error> {
    if path.contains('\0') {
        return Err(failure(
            "core.system",
            "invalid_input",
            native,
            "Storage path contains a zero byte",
        ));
    }
    let absolute = std::fs::canonicalize(crate::host::path(path)).map_err(|error| {
        failure(
            "core.system",
            crate::host::code(&error),
            native,
            "Cannot resolve storage path",
        )
    })?;
    // statvfs(3) (statfs(2) on macOS), as df reports it: capacity is every
    // block, available what an unprivileged writer may still use (#944; it ran
    // `df -Pk` before).
    let unexpected = || failure("core.system", "unexpected", native, "Cannot inspect storage usage");
    let text = std::ffi::CString::new(absolute.as_os_str().as_encoded_bytes()).map_err(|_| unexpected())?;
    let stats = file_system_stats(&text)
        .map_err(|error| failure("core.system", crate::host::code(&error), native, "Cannot inspect storage usage"))?;
    let bytes = |blocks: u128| IntValue::from_decimal_digits(&(blocks * stats.block).to_string()).expect("digits");
    let capacity = bytes(stats.total);
    let available = bytes(stats.available);
    if crate::integer::compare(&available, &capacity).is_gt() {
        return Err(failure(
            "core.system",
            "invalid_data",
            native,
            "Inconsistent storage usage result",
        ));
    }
    Ok(string_map([
        ("capacity".to_string(), capacity.to_string()),
        ("used".to_string(), crate::integer::subtract(&capacity, &available).to_string()),
        ("available".to_string(), available.to_string()),
    ]))
}

pub fn execute(native: Native, args: &[Value]) -> Result<Value, Error> {
    match operation(native) {
        "system_host_name" => Ok(text(host_name(native)?)),
        "system_cpu_count" => Ok(int_value(cpu_count())),
        "storage_usage" => storage_usage(native, string(&args[0])?),
        _ => Err(Error::Unsupported(format!(
            "native provider is not implemented: {}",
            native.symbol()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(path: &str) -> std::collections::HashMap<String, u128> {
        let Ok(value) = storage_usage(Native::SystemStorageUsage, path) else { panic!("usage of {path}") };
        let Ok(entries) = crate::heap::maps::entries(&value) else { panic!("a map") };
        entries
            .iter()
            .map(|(key, value)| {
                let (Value::Str(key), Ok(text)) = (key.to_value(), value.as_string()) else { panic!("text") };
                (key.to_string(), text.parse().expect("number"))
            })
            .collect()
    }

    /// #1013: `NUMBER_OF_PROCESSORS` (a Windows variable) does not override the
    /// system's count. The check runs in a child copy of this test binary, so
    /// the variable is set without touching this process's environment.
    #[test]
    fn cpu_count_ignores_number_of_processors() {
        let name = "native::host::tests::cpu_count_ignores_number_of_processors";
        if !crate::env_switch::enabled("OKKAMA_TEST_CPU_CHILD") {
            let status = std::process::Command::new(std::env::current_exe().expect("the test binary"))
                .args(["--exact", name, "--test-threads=1"])
                .env("OKKAMA_TEST_CPU_CHILD", "1")
                .env("NUMBER_OF_PROCESSORS", "999")
                .status()
                .expect("the child test");
            assert!(status.success(), "the child saw a wrong count");
            return;
        }
        assert_eq!(std::env::var("NUMBER_OF_PROCESSORS").as_deref(), Ok("999"));
        let expected = std::thread::available_parallelism().map_or(1, |count| count.get()) as i64;
        let Ok(Value::Int(count)) = execute(Native::SystemSystemCpuCount, &[]) else { panic!("an int") };
        assert_eq!(count.to_i64(), Some(expected));
    }

    /// #950: the host name comes from gethostname and agrees with the `hostname` tool where it exists.
    #[test]
    fn host_name_reads_gethostname_like_the_hostname_tool() {
        let Ok(name) = host_name(Native::SystemSystemHostName) else { panic!("a host name") };
        assert!(!name.is_empty());
        if let Ok(output) = std::process::Command::new("hostname").output()
            && output.status.success()
        {
            assert_eq!(name, String::from_utf8_lossy(&output.stdout).trim());
        }
    }

    /// #944: usage comes from statvfs, consistent with itself and with `df -Pk`
    /// (kilobyte blocks) where the tool exists, and fails for a missing path.
    #[test]
    fn storage_usage_reads_statvfs_like_df() {
        let root = usage("/");
        assert!(root["capacity"] > 0);
        assert!(root["available"] <= root["capacity"]);
        assert_eq!(root["used"], root["capacity"] - root["available"]);
        if let Ok(output) = std::process::Command::new("df").args(["-Pk", "/"]).env("LC_ALL", "C").output() {
            let listing = String::from_utf8_lossy(&output.stdout);
            let fields: Vec<&str> = listing.lines().last().unwrap_or("").split_whitespace().collect();
            if let Some(blocks) = fields.get(1).and_then(|text| text.parse::<u128>().ok()) {
                let difference = (root["capacity"] / 1024).abs_diff(blocks);
                assert!(difference <= 1, "statvfs {} KiB, df {blocks} KiB", root["capacity"] / 1024);
            }
        }
        assert!(storage_usage(Native::SystemStorageUsage, "/definitely/missing/okkama-944").is_err());
        assert!(storage_usage(Native::SystemStorageUsage, "a\0b").is_err());
    }
}
