//! Collections that cross or hold native state on Champ: arrays, builders and maps of native handles (#751,
//! json's parsed nodes).
//!
//! Pure language plus natives, one case per test so the slow debug runs go in parallel.
#![cfg(feature = "champ-abi")]

mod champ_support;

use champ_support::check;

#[test]
fn json_typed_reading() {
    check(&["L09.json.typed-reading"]);
}

#[test]
fn json_member_readers() {
    check(&["L09.json.member-readers"]);
}

#[test]
fn json_parsed_reading() {
    check(&["L09.json.parsed-reading"]);
}

#[test]
fn json_rejections() {
    check(&["L09.json.rejections"]);
}

/// A builder of handles (`Builder(Handle(_))`, #751).
#[test]
fn json_codec() {
    check(&["L09.json.codec"]);
}

#[test]
fn json_large_objects() {
    check(&["L09.json.large-objects"]);
}
