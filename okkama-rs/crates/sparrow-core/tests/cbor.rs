//! The CBOR codec (#1082) against RFC 8949: every Appendix A example decodes to its diagnostic notation and
//! encodes back (preferred serialization), the non-well-formed examples of Appendix F fail with their offset, and
//! the limits, duplicate keys, tags and the deterministic encoding hold.
use sparrow_core::cbor::{self, DecodeError, Failure, Limits, Map, Value};
use std::rc::Rc;

fn bytes(hex: &str) -> Vec<u8> {
    (0..hex.len()).step_by(2).map(|at| u8::from_str_radix(&hex[at..at + 2], 16).expect("hex")).collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn decoded(hex: &str) -> Rc<Value> {
    cbor::decode(&bytes(hex), Limits::default()).unwrap_or_else(|error| panic!("{hex}: {error:?}"))
}

fn refused(hex: &str) -> DecodeError {
    cbor::decode(&bytes(hex), Limits::default()).expect_err(hex)
}

/// Appendix A: the examples that encode back byte for byte.
const EXACT: &[(&str, &str)] = &[
    ("00", "0"),
    ("01", "1"),
    ("0a", "10"),
    ("17", "23"),
    ("1818", "24"),
    ("1819", "25"),
    ("1864", "100"),
    ("1903e8", "1000"),
    ("1a000f4240", "1000000"),
    ("1b000000e8d4a51000", "1000000000000"),
    ("1bffffffffffffffff", "18446744073709551615"),
    ("c249010000000000000000", "18446744073709551616"),
    ("3bffffffffffffffff", "-18446744073709551616"),
    ("c349010000000000000000", "-18446744073709551617"),
    ("20", "-1"),
    ("29", "-10"),
    ("3863", "-100"),
    ("3903e7", "-1000"),
    ("f90000", "0.0"),
    ("f98000", "-0.0"),
    ("f93c00", "1.0"),
    ("fb3ff199999999999a", "1.1"),
    ("f93e00", "1.5"),
    ("f97bff", "65504.0"),
    ("fa47c35000", "100000.0"),
    ("fa7f7fffff", "3.4028234663852886e+38"),
    ("fb7e37e43c8800759c", "1.0e+300"),
    ("f90001", "5.960464477539063e-8"),
    ("f90400", "0.00006103515625"),
    ("f9c400", "-4.0"),
    ("fbc010666666666666", "-4.1"),
    ("f97c00", "Infinity"),
    ("f97e00", "NaN"),
    ("f9fc00", "-Infinity"),
    ("f4", "false"),
    ("f5", "true"),
    ("f6", "null"),
    ("f7", "undefined"),
    ("f0", "simple(16)"),
    ("f8ff", "simple(255)"),
    ("c074323031332d30332d32315432303a30343a30305a", "0(\"2013-03-21T20:04:00Z\")"),
    ("c11a514b67b0", "1(1363896240)"),
    ("c1fb41d452d9ec200000", "1(1363896240.5)"),
    ("d74401020304", "23(h'01020304')"),
    ("d818456449455446", "24(h'6449455446')"),
    ("d82076687474703a2f2f7777772e6578616d706c652e636f6d", "32(\"http://www.example.com\")"),
    ("40", "h''"),
    ("4401020304", "h'01020304'"),
    ("60", "\"\""),
    ("6161", "\"a\""),
    ("6449455446", "\"IETF\""),
    ("62225c", "\"\\\"\\\\\""),
    ("62c3bc", "\"ü\""),
    ("63e6b0b4", "\"水\""),
    ("64f0908591", "\"𐅑\""),
    ("80", "[]"),
    ("83010203", "[1, 2, 3]"),
    ("8301820203820405", "[1, [2, 3], [4, 5]]"),
    (
        "98190102030405060708090a0b0c0d0e0f101112131415161718181819",
        "[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25]",
    ),
    ("a0", "{}"),
    ("a201020304", "{1: 2, 3: 4}"),
    ("a26161016162820203", "{\"a\": 1, \"b\": [2, 3]}"),
    ("826161a161626163", "[\"a\", {\"b\": \"c\"}]"),
    ("a56161614161626142616361436164614461656145", "{\"a\": \"A\", \"b\": \"B\", \"c\": \"C\", \"d\": \"D\", \"e\": \"E\"}"),
];

/// Appendix A: the examples whose preferred serialization is another encoding of the same value (wider floats
/// that a half holds, indefinite lengths), with that encoding.
const PREFERRED: &[(&str, &str, &str)] = &[
    ("fa7f800000", "Infinity", "f97c00"),
    ("fa7fc00000", "NaN", "f97e00"),
    ("faff800000", "-Infinity", "f9fc00"),
    ("fb7ff0000000000000", "Infinity", "f97c00"),
    ("fb7ff8000000000000", "NaN", "f97e00"),
    ("fbfff0000000000000", "-Infinity", "f9fc00"),
    ("5f42010243030405ff", "h'0102030405'", "450102030405"),
    ("7f657374726561646d696e67ff", "\"streaming\"", "6973747265616d696e67"),
    ("9fff", "[]", "80"),
    ("9f018202039f0405ffff", "[1, [2, 3], [4, 5]]", "8301820203820405"),
    ("9f01820203820405ff", "[1, [2, 3], [4, 5]]", "8301820203820405"),
    ("83018202039f0405ff", "[1, [2, 3], [4, 5]]", "8301820203820405"),
    ("83019f0203ff820405", "[1, [2, 3], [4, 5]]", "8301820203820405"),
    (
        "9f0102030405060708090a0b0c0d0e0f101112131415161718181819ff",
        "[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25]",
        "98190102030405060708090a0b0c0d0e0f101112131415161718181819",
    ),
    ("bf61610161629f0203ffff", "{\"a\": 1, \"b\": [2, 3]}", "a26161016162820203"),
    ("826161bf61626163ff", "[\"a\", {\"b\": \"c\"}]", "826161a161626163"),
    ("bf6346756ef563416d7421ff", "{\"Fun\": true, \"Amt\": -2}", "a26346756ef563416d7421"),
];

#[test]
fn appendix_a_examples_decode_to_their_diagnostic_and_encode_back() {
    for &(encoded, diagnostic) in EXACT {
        let value = decoded(encoded);
        assert_eq!(cbor::diagnostic(&value), diagnostic, "{encoded}");
        assert_eq!(hex(&cbor::encode(&value, false)), encoded, "{encoded} preferred");
        assert_eq!(hex(&cbor::encode(&value, true)), encoded, "{encoded} deterministic");
    }
}

#[test]
fn appendix_a_examples_of_other_widths_and_indefinite_lengths_encode_in_the_preferred_form() {
    for &(encoded, diagnostic, preferred) in PREFERRED {
        let value = decoded(encoded);
        assert_eq!(cbor::diagnostic(&value), diagnostic, "{encoded}");
        assert_eq!(hex(&cbor::encode(&value, false)), preferred, "{encoded}");
        assert_eq!(hex(&cbor::encode(&decoded(preferred), false)), preferred, "{preferred}");
    }
}

#[test]
fn appendix_f_examples_are_not_well_formed() {
    let cases: &[(&str, Failure, usize)] = &[
        // End of input in a head.
        ("18", Failure::Truncated, 0),
        ("19ff", Failure::Truncated, 0),
        ("1bffffffffffffff", Failure::Truncated, 0),
        ("f8", Failure::Truncated, 0),
        // Definite-length strings with short data.
        ("41", Failure::Truncated, 0),
        ("61", Failure::Truncated, 0),
        ("5affffffff00", Failure::Truncated, 0),
        ("7b7fffffffffffffff00", Failure::Truncated, 0),
        // Definite-length maps and arrays not closed with enough items.
        ("81", Failure::Truncated, 0),
        ("818181818181818181", Failure::Truncated, 8),
        ("8200", Failure::Truncated, 0),
        ("a1", Failure::Truncated, 0),
        ("a20102", Failure::Truncated, 0),
        ("a100", Failure::Truncated, 0),
        // A tag number not followed by a tag content.
        ("c0", Failure::Truncated, 0),
        // Indefinite-length strings not closed by a break.
        ("5f4100", Failure::Truncated, 0),
        ("7f6100", Failure::Truncated, 0),
        // Indefinite-length maps and arrays not closed by a break.
        ("9f", Failure::Truncated, 0),
        ("9f0102", Failure::Truncated, 0),
        ("bf", Failure::Truncated, 0),
        ("bf01020102", Failure::Truncated, 0),
        ("819f", Failure::Truncated, 1),
        ("9f8000", Failure::Truncated, 0),
        ("9f9f9f9f9fffffffff", Failure::Truncated, 0),
        ("9f819f819f9fffffff", Failure::Truncated, 0),
        // Reserved additional information.
        ("1c", Failure::Malformed("reserved additional information (28-30)"), 0),
        ("3e", Failure::Malformed("reserved additional information (28-30)"), 0),
        ("fd", Failure::Malformed("reserved additional information (28-30)"), 0),
        // Indefinite length on major types 0, 1, 6 and simple values in two bytes below 32.
        ("1f", Failure::Malformed("an indefinite length on an integer"), 0),
        ("3f", Failure::Malformed("an indefinite length on an integer"), 0),
        ("df", Failure::Malformed("an indefinite-length tag"), 0),
        ("f800", Failure::Malformed("a simple value below 32 in an extra byte"), 0),
        ("f818", Failure::Malformed("a simple value below 32 in an extra byte"), 0),
        ("f81f", Failure::Malformed("a simple value below 32 in an extra byte"), 0),
        // Chunks of the wrong type or of indefinite length inside an indefinite-length string.
        ("5f00ff", Failure::Malformed("a chunk of an indefinite-length string is not a definite string of its type"), 1),
        ("5f21ff", Failure::Malformed("a chunk of an indefinite-length string is not a definite string of its type"), 1),
        ("5f6100ff", Failure::Malformed("a chunk of an indefinite-length string is not a definite string of its type"), 1),
        ("5f5fffff", Failure::Malformed("a chunk of an indefinite-length string is not a definite string of its type"), 1),
        ("7f4100ff", Failure::Malformed("a chunk of an indefinite-length string is not a definite string of its type"), 1),
        ("7f7f6100ffff", Failure::Malformed("a chunk of an indefinite-length string is not a definite string of its type"), 1),
        // Break where none may be.
        ("ff", Failure::Malformed("a break outside an indefinite-length item"), 0),
        ("81ff", Failure::Malformed("a break outside an indefinite-length item"), 1),
        ("8200ff", Failure::Malformed("a break outside an indefinite-length item"), 2),
        // A map of one pair needs two bytes more: refused as cut off before its break is read.
        ("a1ff", Failure::Truncated, 0),
        ("a1ff00", Failure::Malformed("a break outside an indefinite-length item"), 1),
        ("a100ff", Failure::Malformed("a break outside an indefinite-length item"), 2),
        ("a20000ff", Failure::Truncated, 0),
        ("9f81ff", Failure::Malformed("a break outside an indefinite-length item"), 2),
        ("9f829f819f9fffffffff", Failure::Malformed("a break outside an indefinite-length item"), 9),
        ("bf00ff", Failure::Malformed("a break after a map key, before its value"), 2),
        ("bf000000ff", Failure::Malformed("a break after a map key, before its value"), 4),
    ];
    for (encoded, failure, offset) in cases {
        assert_eq!(refused(encoded), DecodeError { failure: failure.clone(), offset: *offset }, "{encoded}");
    }
}

#[test]
fn text_must_be_utf8_in_every_chunk() {
    assert_eq!(refused("62c328"), DecodeError { failure: Failure::InvalidUtf8, offset: 0 });
    // "ü" split between two chunks: each chunk must be UTF-8 on its own.
    assert_eq!(refused("7f61c361bcff"), DecodeError { failure: Failure::InvalidUtf8, offset: 1 });
    assert_eq!(cbor::diagnostic(&decoded("7f62c3bc6161ff")), "\"üa\"");
}

#[test]
fn repeated_map_keys_are_refused_at_the_repeated_key() {
    assert_eq!(refused("a2616101616102"), DecodeError { failure: Failure::DuplicateKey, offset: 4 });
    // 1 and the bignum 1 are one integer; 1 and 1.0 are different values.
    assert_eq!(refused("a201f5c2410102"), DecodeError { failure: Failure::DuplicateKey, offset: 3 });
    assert_eq!(cbor::diagnostic(&decoded("a201f5f93c00f4")), "{1: true, 1.0: false}");
    // A large map is checked through a set.
    let mut many = vec![0xb8, 20];
    for key in 0..19u8 {
        many.extend_from_slice(&[key, 0xf6]);
    }
    many.extend_from_slice(&[0x05, 0xf6]);
    assert_eq!(cbor::decode(&many, Limits::default()).expect_err("repeated 5"), DecodeError {
        failure: Failure::DuplicateKey,
        offset: 40
    });
}

#[test]
fn bignum_and_rational_tags_are_read_and_checked() {
    assert_eq!(cbor::diagnostic(&decoded("c24100")), "0");
    assert_eq!(hex(&cbor::encode(&decoded("c243000001"), false)), "01", "a small bignum is a plain integer");
    assert_eq!(cbor::diagnostic(&decoded("d81e820203")), "30([2, 3])");
    assert_eq!(cbor::diagnostic(&decoded("d81e822204")), "30([-3, 4])");
    assert_eq!(cbor::diagnostic(&decoded("d81e820204")), "30([1, 2])", "reduced");
    assert_eq!(cbor::diagnostic(&decoded("d81e82c249010000000000000000c249020000000000000000")), "30([1, 2])");
    let tag = |number, reason| Failure::InvalidTag(number, reason);
    assert_eq!(refused("c201"), DecodeError { failure: tag(2, "a bignum's content must be a byte string"), offset: 0 });
    assert_eq!(refused("81c36161"), DecodeError { failure: tag(3, "a bignum's content must be a byte string"), offset: 1 });
    let rational = "a rational is an array of an integer and a positive integer";
    for encoded in ["d81e820200", "d81e820220", "d81e8102", "d81e83010203", "d81e82f93c0002", "d81e01"] {
        assert_eq!(refused(encoded), DecodeError { failure: tag(30, rational), offset: 0 }, "{encoded}");
    }
}

#[test]
fn limits_stop_hostile_input_where_it_passes_them() {
    let limits = Limits { depth: 2, items: 5, size: 8 };
    assert!(cbor::decode(&bytes("828100f6"), limits).is_ok());
    assert_eq!(cbor::decode(&bytes("82818100"), limits).expect_err("depth"), DecodeError {
        failure: Failure::TooDeep(2),
        offset: 2
    });
    assert_eq!(cbor::decode(&bytes("c1c1c100"), limits).expect_err("tags nest"), DecodeError {
        failure: Failure::TooDeep(2),
        offset: 2
    });
    assert_eq!(cbor::decode(&bytes("8401020304"), Limits { items: 4, ..limits }).expect_err("items"), DecodeError {
        failure: Failure::TooManyItems(4),
        offset: 4
    });
    assert_eq!(cbor::decode(&bytes("480102030405060708"), limits).expect_err("size"), DecodeError {
        failure: Failure::TooLarge(8),
        offset: 8
    });
    // A declared length or count beyond the input is refused before anything is allocated for it.
    assert_eq!(refused("5bffffffffffffffff"), DecodeError { failure: Failure::Truncated, offset: 0 });
    assert_eq!(refused("9bffffffffffffffff"), DecodeError { failure: Failure::Truncated, offset: 0 });
    assert_eq!(refused("bbffffffffffffffff"), DecodeError { failure: Failure::Truncated, offset: 0 });
    // Deep nesting within the limit is read without recursion and dropped without it.
    let depth = 200_000;
    let mut deep = vec![0x81; depth];
    deep.push(0x00);
    let value = cbor::decode(&deep, Limits { depth, items: depth + 1, size: depth + 1 }).expect("deep");
    assert_eq!(cbor::encode(&value, true), deep);
    drop(value);
    let open = vec![0x9f; 100_000];
    assert_eq!(cbor::decode(&open, Limits { depth: 1_000_000, ..Limits::default() }).expect_err("open").failure, Failure::Truncated);
}

#[test]
fn bytes_after_the_item_are_trailing_and_a_sequence_reads_its_first_item() {
    assert_eq!(refused("0102"), DecodeError { failure: Failure::Trailing, offset: 1 });
    let (first, used) = cbor::decode_first(&bytes("8201020304"), Limits::default()).expect("first");
    assert_eq!((cbor::diagnostic(&first), used), ("[1, 2]".to_string(), 3));
    assert_eq!(cbor::decode_first(&bytes("8201"), Limits::default()).expect_err("cut").failure, Failure::Truncated);
    let limits = Limits { size: 4, ..Limits::default() };
    assert_eq!(cbor::decode_first(&bytes("4401020304"), limits).expect_err("past the size"), DecodeError {
        failure: Failure::TooLarge(4),
        offset: 4
    });
    assert!(cbor::decode_first(&bytes("430102030405"), limits).is_ok(), "a whole item within the size");
}

fn text(t: &str) -> Rc<Value> {
    Rc::new(Value::Text(t.into()))
}

fn int(n: i128) -> Rc<Value> {
    Rc::new(Value::Integer(n))
}

#[test]
fn the_deterministic_encoding_sorts_keys_by_their_encoded_bytes() {
    // RFC 8949 4.2.1's example order: 10, 100, -1, "z", "aa", [100], [-1], false.
    let keys = vec![
        Rc::new(Value::Bool(false)),
        text("aa"),
        Rc::new(Value::Array(vec![int(-1)])),
        int(100),
        text("z"),
        int(-1),
        Rc::new(Value::Array(vec![int(100)])),
        int(10),
    ];
    let values = (0..keys.len() as i128).map(int).collect();
    let map = Value::Map(Map::new(keys, values).expect("distinct"));
    let sorted = cbor::decode(&cbor::encode(&map, true), Limits::default()).expect("decodes");
    assert_eq!(cbor::diagnostic(&sorted), "{10: 7, 100: 3, -1: 5, \"z\": 4, \"aa\": 1, [100]: 6, [-1]: 2, false: 0}");
    assert_eq!(cbor::diagnostic(&cbor::decode(&cbor::encode(&map, false), Limits::default()).expect("decodes")),
        "{false: 0, \"aa\": 1, [-1]: 2, 100: 3, \"z\": 4, -1: 5, [100]: 6, 10: 7}", "preferred keeps the order");
}

#[test]
fn integers_floats_and_rationals_take_their_shortest_exact_form() {
    let encoded = |value: Value| hex(&cbor::encode(&value, true));
    assert_eq!(encoded(cbor::integer(num_bigint::BigInt::from(u64::MAX))), "1bffffffffffffffff");
    assert_eq!(encoded(cbor::integer(num_bigint::BigInt::from(u64::MAX) + 1)), "c249010000000000000000");
    assert_eq!(encoded(cbor::integer(-num_bigint::BigInt::from(u64::MAX) - 1)), "3bffffffffffffffff");
    assert_eq!(encoded(cbor::integer(-num_bigint::BigInt::from(u64::MAX) - 2)), "c349010000000000000000");
    assert_eq!(encoded(Value::Float(0.5)), "f93800");
    assert_eq!(encoded(Value::Float(65505.0)), "fa477fe100", "past the half range");
    assert_eq!(encoded(Value::Float(2f64.powi(-24))), "f90001", "the smallest half subnormal");
    assert_eq!(encoded(Value::Float(2f64.powi(-25))), "fa33000000", "below it: a single");
    assert_eq!(encoded(Value::Float(3.0 * 2f64.powi(-24))), "f90003");
    assert_eq!(encoded(Value::Float(1.0 + 2f64.powi(-10))), "f93c01");
    assert_eq!(encoded(Value::Float(1.0 + 2f64.powi(-11))), "fa3f801000", "eleven fraction bits: a single");
    assert_eq!(encoded(Value::Float(f64::from(f32::MIN_POSITIVE))), "fa00800000");
    assert_eq!(encoded(Value::Float(f64::MIN_POSITIVE)), "fb0010000000000000");
    assert_eq!(encoded(cbor::rational(1.into(), 3.into()).expect("1/3")), "d81e820103");
    assert_eq!(encoded(cbor::rational(6.into(), (-4).into()).expect("-3/2")), "d81e822202");
    assert!(cbor::rational(1.into(), 0.into()).is_none());
}

#[test]
fn every_half_decodes_and_encodes_back_to_itself() {
    for bits in 0..=u16::MAX {
        let encoded = [0xf9, (bits >> 8) as u8, bits as u8];
        let value = cbor::decode(&encoded, Limits::default()).expect("a half");
        let is_nan = matches!(&*value, Value::Float(f) if f.is_nan());
        let expected = if is_nan { vec![0xf9, 0x7e, 0x00] } else { encoded.to_vec() };
        assert_eq!(cbor::encode(&value, false), expected, "{bits:04x}");
    }
}

#[test]
fn diagnostic_floats_follow_appendix_a() {
    for (value, text) in [
        (1363896240.5, "1363896240.5"),
        (1e21, "1.0e+21"),
        (1e20, "100000000000000000000.0"),
        (0.000001, "0.000001"),
        (1e-7, "1.0e-7"),
        (-2.5e-10, "-2.5e-10"),
        (123.456, "123.456"),
    ] {
        assert_eq!(cbor::float_text(value), text);
    }
    assert_eq!(cbor::diagnostic(&Value::Text("\u{1}\n\"".into())), "\"\\u0001\\n\\\"\"");
}

#[test]
fn map_lookups_find_keys_by_value_small_or_indexed() {
    for size in [3i128, 40] {
        let keys: Vec<Rc<Value>> = (0..size).map(|n| if n % 2 == 0 { int(n) } else { text(&format!("k{n}")) }).collect();
        let values = (0..size).map(int).collect();
        let map = Map::new(keys, values).expect("distinct");
        assert_eq!(map.position(&Value::Integer(2)), Some(2));
        assert_eq!(map.text_position("k1"), Some(1));
        assert_eq!(map.text_position("k2"), None);
        assert_eq!(map.position(&Value::Float(2.0)), None, "2.0 is not the key 2");
        assert_eq!(map.position(&cbor::integer(num_bigint::BigInt::from(2))), Some(2));
    }
    assert_eq!(Map::new(vec![text("a"), text("a")], vec![int(1), int(2)]).err(), Some(1));
}

/// The fuzz target's property (okkama-rs/fuzz/fuzz_targets/cbor.rs) over its seeds and every truncation and
/// one-byte change of them: no panic, and a decoded value's encodings decode again to one deterministic form.
#[test]
fn the_fuzz_property_holds_on_the_seeds_and_their_mutations() {
    let seeds = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/seeds/cbor");
    let mut inputs: Vec<Vec<u8>> = Vec::new();
    for entry in std::fs::read_dir(&seeds).expect("fuzz seeds") {
        let seed = std::fs::read(entry.expect("a seed").path()).expect("a seed's bytes");
        for cut in 0..=seed.len() {
            inputs.push(seed[..cut].to_vec());
        }
        for at in 0..seed.len() {
            for flip in [0x01, 0x10, 0x1f, 0x80, 0xff] {
                let mut changed = seed.clone();
                changed[at] ^= flip;
                inputs.push(changed);
            }
        }
    }
    assert!(inputs.len() > 1000, "{}", inputs.len());
    let limits = Limits { depth: 64, items: 1 << 16, size: 1 << 20 };
    let open = Limits { depth: 64, items: 1 << 20, size: 1 << 24 };
    for input in inputs {
        if let Ok((value, used)) = cbor::decode_first(&input, limits) {
            assert!(used <= input.len());
            let deterministic = cbor::encode(&value, true);
            let again = cbor::decode(&cbor::encode(&value, false), open).expect("a preferred encoding decodes");
            assert_eq!(cbor::encode(&again, true), deterministic, "{}", hex(&input));
            let fixed = cbor::decode(&deterministic, open).expect("a deterministic encoding decodes");
            assert_eq!(cbor::encode(&fixed, true), deterministic, "{}", hex(&input));
            let _ = cbor::diagnostic(&value);
        }
    }
}
