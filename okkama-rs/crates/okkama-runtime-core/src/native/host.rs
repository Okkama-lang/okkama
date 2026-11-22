//! Natives часов, ожидания, случайности и сведений хоста — перенос
//! `runtime/time` (часы, сон), `runtime/system`
//! (secure_random, secure_integer, secure_float, платформа, хост, хранилище)
//! и выборки `runtime/random`.
use super::support::{byte_array, bytes, failure, int, int_value, string, string_map, text};
use super::Error;
use crate::bignum::Big;
use crate::sched;
use crate::{IntValue, Value, abi::Native};
use std::sync::OnceLock;

fn operation(native: Native) -> &'static str {
    native.name()
}

fn origin() -> std::time::Instant {
    static ORIGIN: OnceLock<std::time::Instant> = OnceLock::new();
    *ORIGIN.get_or_init(std::time::Instant::now)
}

thread_local! {
    static DOMAIN_SERIAL: std::cell::Cell<u64> = const { std::cell::Cell::new(1) };
}

fn unsigned(sample: &[u8]) -> Big {
    let base = Big::from_u64(256);
    sample
        .iter()
        .fold(Big::zero(), |number, octet| number.multiply(&base).add(&Big::from_u64(u64::from(*octet))))
}

/// Заполнить `out` криптостойкими байтами платформы (#195); отказ
/// генератора — сбой native.
fn secure_fill(native: Native, out: &mut [u8]) -> Result<(), Error> {
    okkama_native::secure::fill(out)
        .map_err(|error| failure("core.system", "unexpected", native, error.message()))
}

fn secure_bytes(native: Native, size: usize) -> Result<Vec<u8>, Error> {
    let mut out = vec![0; size];
    secure_fill(native, &mut out)?;
    Ok(out)
}

/// Несмещённое смещение в `0..span` (span > 0) из криптостойких выборок
/// (#195, #507). Выборка берёт столько 64-битных слов, сколько нужно числу
/// `span - 1`, и тянется заново, пока она меньше `2^(64·слов) mod span`. Для
/// `span` до 2^64 это одно слово в big-endian, как у random_sample_accepted и
/// random_integer_from_sample.
fn secure_offset(native: Native, span: &IntValue) -> Result<IntValue, Error> {
    if let Some(small) = span.to_i64() {
        let small = small.unsigned_abs();
        let threshold = small.wrapping_neg() % small;
        loop {
            let mut sample = [0u8; 8];
            secure_fill(native, &mut sample)?;
            let sample = u64::from_be_bytes(sample);
            if sample >= threshold {
                return Ok(IntValue::from_u128(u128::from(sample % small)));
            }
        }
    }
    let largest = crate::integer::subtract(span, &IntValue::from_i64(1));
    let words = largest.to_big().bits().div_ceil(64).max(1);
    let exponent = u32::try_from(64 * words).map_err(|_| {
        failure("core.random", "resource_exhausted", native, "Random range is too wide")
    })?;
    let modulus = IntValue::from_big(Big::from_u64(2).power(exponent));
    let threshold = crate::integer::divide(&modulus, span).expect("positive").1;
    let size = usize::try_from(words * 8).expect("fits with the exponent");
    loop {
        let sample = IntValue::from_big(unsigned(&secure_bytes(native, size)?));
        if crate::integer::compare(&sample, &threshold).is_ge() {
            return Ok(crate::integer::divide(&sample, span).expect("positive").1);
        }
    }
}

fn platform() -> &'static str {
    match std::env::consts::OS {
        "macos" | "ios" => "macos",
        "linux" | "android" => "linux",
        "windows" => "windows",
        "freebsd" => "freebsd",
        "openbsd" => "openbsd",
        "netbsd" => "netbsd",
        "dragonfly" => "dragonfly",
        "solaris" | "illumos" => "solaris",
        "aix" => "aix",
        _ => "unknown",
    }
}

fn architecture() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "x86" => "x86",
        "aarch64" => "arm64",
        "arm" => "arm",
        "powerpc64" => "ppc64",
        "powerpc" => "ppc",
        "riscv64" => "riscv64",
        "s390x" => "s390x",
        _ => "unknown",
    }
}

fn host_name(native: Native) -> Result<String, Error> {
    let from_command = std::process::Command::new("hostname")
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|name| !name.is_empty());
    from_command.ok_or_else(|| failure("core.system", "unexpected", native, "Host name is unavailable"))
}

/// statvfs(3) of `path`.
#[allow(unsafe_code)]
fn file_system_stats(path: &std::ffi::CStr) -> std::io::Result<libc::statvfs> {
    // SAFETY: an all-zero statvfs is a valid value of this plain C struct.
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `path` is NUL-terminated and `stats` is a writable statvfs.
    if unsafe { libc::statvfs(path.as_ptr(), &mut stats) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(stats)
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
    // statvfs(3), as df reports it: capacity is every block, available what an
    // unprivileged writer may still use (#944; it ran `df -Pk` before).
    let unexpected = || failure("core.system", "unexpected", native, "Cannot inspect storage usage");
    let text = std::ffi::CString::new(absolute.as_os_str().as_encoded_bytes()).map_err(|_| unexpected())?;
    let stats = file_system_stats(&text)
        .map_err(|error| failure("core.system", crate::host::code(&error), native, "Cannot inspect storage usage"))?;
    // The field widths differ between platforms (32-bit block counts on macOS).
    fn wide(value: impl Into<u128>) -> u128 {
        value.into()
    }
    let fragment = wide(stats.f_frsize);
    let bytes = |blocks: u128| IntValue::from_decimal_digits(&(blocks * fragment).to_string()).expect("digits");
    let capacity = bytes(wide(stats.f_blocks));
    let available = bytes(wide(stats.f_bavail));
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

fn sleep(native: Native, source: &str) -> Value {
    let invalid = |message: &str| {
        let Error::Failure(failure) = failure("core.time", "invalid_input", native, message) else {
            unreachable!("failure builds NativeFailure")
        };
        crate::failure::Failure::new(crate::failure::Kind::Native(std::rc::Rc::new(failure)))
    };
    let canonical = source == "0"
        || (!source.is_empty() && !source.starts_with('0') && source.bytes().all(|b| b.is_ascii_digit()));
    if !canonical {
        return Value::Future(sched::resolved(Err(invalid(
            "Wait duration must be a canonical non-negative integer",
        ))));
    }
    let Some(nanoseconds) = source.parse::<i64>().ok() else {
        return Value::Future(sched::resolved(Err(invalid(
            "Wait duration exceeds the runtime timer range",
        ))));
    };
    if nanoseconds == 0 {
        return Value::Future(sched::resolved(Ok(Value::Bool(true))));
    }
    let promise = sched::promise();
    let target = promise.clone();
    // A timer on the event loop, not a sleeping thread per wait (#503).
    let (op, id) = sched::timer(std::time::Duration::from_nanos(nanoseconds as u64), move || {
        sched::settle(&target, Ok(Value::Bool(true)))
    });
    // Отменённое ожидание не держит программу и не держит продолжение.
    promise.listen(move || {
        op.detach();
        sched::forget(id);
    });
    Value::Future(promise)
}

pub fn execute(native: Native, args: &[Value]) -> Result<Value, Error> {
    match operation(native) {
        "wall_clock" => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default();
            Ok(Value::Int(IntValue::from_u128(now.as_nanos())))
        }
        "monotonic_clock" => Ok(Value::Int(IntValue::from_u128(origin().elapsed().as_nanos()))),
        "clock_domain" => {
            let pid = std::process::id();
            if !args[0].as_bool()? {
                return Ok(text(format!("system:{pid}:1")));
            }
            if !crate::testing::active() {
                return Err(failure(
                    "core.time",
                    "permission_denied",
                    native,
                    "Manual clock domains require a running test case",
                ));
            }
            let serial = DOMAIN_SERIAL.with(|serial| {
                serial.set(serial.get() + 1);
                serial.get()
            });
            Ok(text(format!("manual:{pid}:{serial}")))
        }
        "sleep_nanoseconds" => Ok(sleep(native, string(&args[0])?)),
        "secure_random" => {
            let size = int(&args[0])?;
            if size.is_negative() {
                return Err(failure(
                    "core.system",
                    "invalid_input",
                    native,
                    "Secure random size must be non-negative",
                ));
            }
            let size = size
                .to_i64()
                .filter(|size| *size <= 16 * 1024 * 1024)
                .ok_or_else(|| {
                    failure(
                        "core.system",
                        "resource_exhausted",
                        native,
                        "Secure random request exceeds 16 MiB",
                    )
                })?;
            Ok(byte_array(secure_bytes(native, size as usize)?))
        }
        "secure_integer" => {
            let (start, stop) = (int(&args[0])?, int(&args[1])?);
            if crate::integer::compare(start, stop).is_ge() {
                return Err(failure(
                    "core.random",
                    "invalid_input",
                    native,
                    "Random range must be non-empty",
                ));
            }
            let span = crate::integer::subtract(stop, start);
            Ok(Value::Int(crate::integer::add(start, &secure_offset(native, &span)?)))
        }
        "secure_float" => {
            // 53 старших бита из семи байт, как у random_float_from_sample.
            let mut sample = [0u8; 8];
            secure_fill(native, &mut sample[1..])?;
            let value = u64::from_be_bytes(sample) >> 3;
            Ok(Value::Float(value as f64 / 9_007_199_254_740_992.0))
        }
        "random_sample_accepted" | "random_integer_from_sample" => {
            let sample = bytes(&args[0])?;
            if sample.len() != 8 {
                return Err(failure(
                    "core.random",
                    "invalid_input",
                    native,
                    "Random integer sample must have eight bytes",
                ));
            }
            let value = IntValue::from_big(unsigned(&sample));
            let (start, stop) = (int(&args[1])?, int(&args[2])?);
            let span = crate::integer::subtract(stop, start);
            if operation(native) == "random_sample_accepted" {
                if span.signum() <= 0 {
                    return Ok(Value::Bool(false));
                }
                let modulus = IntValue::from_big(Big::from_u64(2).power(64));
                let threshold = crate::integer::divide(&modulus, &span).expect("positive").1;
                return Ok(Value::Bool(crate::integer::compare(&value, &threshold).is_ge()));
            }
            if span.is_zero() {
                // Math::BigInt: x mod 0 = x.
                return Ok(Value::Int(crate::integer::add(start, &value)));
            }
            let magnitude = if span.is_negative() { span.negated() } else { span.clone() };
            let mut rest = crate::integer::divide(&value, &magnitude).expect("non-zero").1;
            // Остаток со знаком делителя, как у Math::BigInt.
            if span.is_negative() && !rest.is_zero() {
                rest = crate::integer::add(&rest, &span);
            }
            Ok(Value::Int(crate::integer::add(start, &rest)))
        }
        "random_float_from_sample" => {
            let sample = bytes(&args[0])?;
            if sample.len() != 7 {
                return Err(failure(
                    "core.random",
                    "invalid_input",
                    native,
                    "Random float sample must have seven bytes",
                ));
            }
            let value = sample.iter().fold(0u64, |acc, b| (acc << 8) | u64::from(*b)) >> 3;
            Ok(Value::Float(value as f64 / 9_007_199_254_740_992.0))
        }
        "system_platform" => Ok(text(platform())),
        "system_architecture" => Ok(text(architecture())),
        "system_byte_order" => Ok(text(if cfg!(target_endian = "little") { "little" } else { "big" })),
        "system_host_name" => Ok(text(host_name(native)?)),
        "system_cpu_count" => {
            if let Ok(environment) = std::env::var("NUMBER_OF_PROCESSORS") {
                let trimmed = environment.trim();
                if !trimmed.is_empty() && !trimmed.starts_with('0') && trimmed.bytes().all(|b| b.is_ascii_digit()) {
                    return Ok(Value::Int(IntValue::from_decimal_digits(trimmed).expect("digits")));
                }
            }
            let count = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
            Ok(int_value(count.max(1) as i64))
        }
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
        let Ok(entries) = super::super::support::entries(&value) else { panic!("a map") };
        entries
            .into_iter()
            .map(|(key, value)| {
                let Ok(text) = value.as_string() else { panic!("text") };
                (key, text.parse().expect("number"))
            })
            .collect()
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
