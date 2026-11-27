//! The `runtime/system` `secure_*` natives (#1125): the platform's cryptographically secure random bytes, an
//! unbiased integer in a range and a float in [0, 1), through `okkama_native::secure` (the platform's generator
//! natively, the JavaScript host's `crypto.getRandomValues` on wasm32). Pure of the host otherwise, so a build
//! without it (the playground) has them.
use super::support::{byte_array, failure, int};
use super::Error;
use crate::bignum::Big;
use crate::{IntValue, Value, abi::Native};

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
/// `span` до 2^64 это одно слово в big-endian.
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

pub fn execute(native: Native, args: &[Value]) -> Result<Value, Error> {
    match native.name() {
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
            // 53 старших бита из семи байт.
            let mut sample = [0u8; 8];
            secure_fill(native, &mut sample[1..])?;
            let value = u64::from_be_bytes(sample) >> 3;
            Ok(Value::Float(value as f64 / 9_007_199_254_740_992.0))
        }
        _ => Err(Error::Unsupported(format!("native provider is not implemented: {}", native.symbol()))),
    }
}
