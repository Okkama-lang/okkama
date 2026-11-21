//! `runtime/uuid` и `runtime/unicode_normalization` — перенос части
//! `runtime/decimal` — в `numbers`.
use super::Error;
use super::support::{byte_array, bytes, failed, failure, int, string, text};
use crate::{Value, abi::Native};
use unicode_normalization::UnicodeNormalization;

fn invalid(native: Native, message: &str) -> Error {
    failure("core.uuid", "invalid_input", native, message)
}

fn versioned(mut octets: Vec<u8>, version: u8) -> Value {
    octets[6] = (octets[6] & 0x0f) | (version << 4);
    octets[8] = (octets[8] & 0x3f) | 0x80;
    byte_array(octets)
}

pub fn execute(native: Native, args: &[Value]) -> Result<Value, Error> {
    let name = native.name();
    match name {
        "normalized_text" => {
            let origin = string(&args[0])?;
            Ok(text(match string(&args[1])? {
                "NFC" => origin.nfc().collect::<String>(),
                "NFD" => origin.nfd().collect(),
                "NFKC" => origin.nfkc().collect(),
                "NFKD" => origin.nfkd().collect(),
                _ => return Err(failed(native)),
            }))
        }
        "uuid_named" => {
            let namespace = bytes(&args[0])?;
            if namespace.len() != 16 {
                return Err(invalid(native, "UUID namespace must contain sixteen bytes"));
            }
            let data = bytes(&args[1])?;
            let version = int(&args[2])?.to_i64();
            let hash: Vec<u8> = match version {
                Some(3) => {
                    use md5::Digest;
                    let mut digest = md5::Md5::new();
                    digest.update(&namespace);
                    digest.update(&data);
                    digest.finalize().to_vec()
                }
                Some(5) => {
                    use sha1::Digest;
                    let mut digest = sha1::Sha1::new();
                    digest.update(&namespace);
                    digest.update(&data);
                    digest.finalize().to_vec()
                }
                _ => return Err(invalid(native, "Named UUID version must be three or five")),
            };
            Ok(versioned(hash[..16].to_vec(), version.expect("checked") as u8))
        }
        _ => Err(Error::Unsupported(format!(
            "native provider is not implemented: {}",
            native.symbol()
        ))),
    }
}
