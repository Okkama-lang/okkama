//! Decoding any bytes as CBOR (#1082) ends in a value or a decode error within the limits, never a panic; a
//! decoded value's preferred and deterministic encodings decode again to the same deterministic encoding, the
//! deterministic encoding is a fixed point, and diagnostic notation renders it.
#![no_main]
use libfuzzer_sys::fuzz_target;
use sparrow_core::cbor::{self, Limits};

fuzz_target!(|data: &[u8]| {
    let limits = Limits { depth: 64, items: 1 << 16, size: 1 << 20 };
    if let Ok((value, used)) = cbor::decode_first(data, limits) {
        assert!(used <= data.len());
        let deterministic = cbor::encode(&value, true);
        let preferred = cbor::encode(&value, false);
        let open = Limits { depth: 64, items: 1 << 20, size: 1 << 24 };
        let again = cbor::decode(&preferred, open).expect("a preferred encoding decodes");
        assert_eq!(cbor::encode(&again, true), deterministic, "preferred and deterministic encodings agree");
        let fixed = cbor::decode(&deterministic, open).expect("a deterministic encoding decodes");
        assert_eq!(cbor::encode(&fixed, true), deterministic, "the deterministic encoding is a fixed point");
        let _ = cbor::diagnostic(&value);
    }
});
