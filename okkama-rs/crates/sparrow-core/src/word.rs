//! One machine word per value. The representation is chosen by the static SPB
//! type, so registers, fields and array elements carry no runtime tag:
//!
//! - `int`: `(n << 1) | 1` for 63-bit integers, otherwise a pointer to a BigInt
//!   object (pointers are 8-aligned, so the low bit tells them apart);
//! - `float`: raw IEEE bits (language floats are always finite);
//! - `bool`, `byte`, `char`: the raw value;
//! - every other type: a pointer to a heap object with a header.
//!
//! Zero is "unbound storage" (a constructor field not assigned yet): no tagged
//! integer and no pointer is zero. Raw scalars use [`UNBOUND_RAW`], a NaN bit
//! pattern for floats and outside every bool/byte/char range.

/// A value word; its meaning comes from a [`Repr`].
pub type Word = u64;

/// Unbound storage of references and integers.
pub const UNBOUND: Word = 0;
/// Unbound storage of raw scalars (a quiet NaN with a payload no finite float
/// or scalar value uses).
pub const UNBOUND_RAW: Word = 0x7ff8_dead_beef_0001;

pub use okkama_sparrow::layout::Repr;

/// The stub `undef` leaves in storage of representation `repr`: [`UNBOUND_RAW`] for raw scalars,
/// [`UNBOUND`] otherwise.
#[inline]
pub fn unbound(repr: Repr) -> Word {
    if repr == Repr::Raw { UNBOUND_RAW } else { UNBOUND }
}

/// Largest and smallest integers stored inline.
pub const SMALL_MAX: i64 = (1 << 62) - 1;
pub const SMALL_MIN: i64 = -(1 << 62);

/// Inline integer word, if `n` fits in 63 bits.
#[inline]
pub fn small(n: i64) -> Option<Word> {
    (SMALL_MIN..=SMALL_MAX)
        .contains(&n)
        .then_some(((n << 1) | 1) as Word)
}

/// The inline integer of a word, if it is one.
#[inline]
pub fn small_value(word: Word) -> Option<i64> {
    (word & 1 == 1).then_some((word as i64) >> 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::BuildHasher;

    /// FastHasher mixes an integer key of any width as one 64-bit word (a
    /// `u128` as two), so engine-assigned keys keep their cheap hash (#1103).
    #[test]
    fn fast_hasher_mixes_integer_keys_as_words_1103() {
        let build = FastBuild::default();
        let word = 0x517c_c1b7_2722_0a95_u64.wrapping_mul(7);
        for hash in [
            build.hash_one(7u8),
            build.hash_one(7u16),
            build.hash_one(7u32),
            build.hash_one(7u64),
            build.hash_one(7usize),
            build.hash_one(7i32),
        ] {
            assert_eq!(hash, word);
        }
        assert_eq!(build.hash_one(7u128), build.hash_one((7u64, 0u64)));
        assert_ne!(build.hash_one((1u64, 2u64)), build.hash_one((2u64, 1u64)));
    }

    /// FastHasher is predictable, so a text key (which may come from outside
    /// the process) must not reach it: debug builds refuse the byte write
    /// (#1103).
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "engine-assigned integers only")]
    fn fast_hasher_refuses_text_keys_1103() {
        FastBuild::default().hash_one("id");
    }

    #[test]
    fn small_integers_round_trip_and_never_look_like_pointers_or_unbound() {
        for n in [
            0,
            1,
            -1,
            42,
            -42,
            SMALL_MAX,
            SMALL_MIN,
            i32::MAX as i64,
            i32::MIN as i64,
        ] {
            let word = small(n).unwrap();
            assert_eq!(small_value(word), Some(n));
            assert_ne!(word, UNBOUND);
            assert_eq!(Repr::Int.pointer(word), None);
        }
        for n in [SMALL_MAX + 1, SMALL_MIN - 1, i64::MAX, i64::MIN] {
            assert_eq!(small(n), None);
        }
        assert_eq!(Repr::Int.pointer(0x1000), Some(0x1000));
        assert_eq!(Repr::Int.pointer(UNBOUND), None);
        assert_eq!(Repr::Raw.pointer(0x1000), None);
    }

    #[test]
    fn unbound_raw_is_not_a_finite_float_or_scalar() {
        let f = f64::from_bits(UNBOUND_RAW);
        assert!(f.is_nan());
        assert!(UNBOUND_RAW > u64::from(u32::MAX));
    }
}

/// Unkeyed multiplicative hashing, only for keys the engine assigns: heap
/// addresses, handle ids, function and type indices. SipHash showed up in
/// native-call profiles for these, and nobody outside the process chooses them.
///
/// It is predictable, so it must never key text or bytes that come from
/// outside the process (column names from a database server, JSON members,
/// headers): anyone can compute colliding keys, and a seed does not help, since
/// `"id"` and `"id\0"` collide under every seed. Such keys use std's keyed
/// `RandomState` (SipHash-1-3, #1103). Integer writes are mixed directly; a
/// byte write means a text or byte key, which debug builds refuse.
#[derive(Debug, Default, Clone, Copy)]
pub struct FastHasher(u64);
impl std::hash::Hasher for FastHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        debug_assert!(
            false,
            "FastHasher keys engine-assigned integers only; text and bytes take a keyed hash (#1103)"
        );
        for chunk in bytes.chunks(8) {
            let mut word = [0; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.write_u64(u64::from_le_bytes(word));
        }
    }
    fn write_u8(&mut self, n: u8) {
        self.write_u64(n.into());
    }
    fn write_u16(&mut self, n: u16) {
        self.write_u64(n.into());
    }
    fn write_u32(&mut self, n: u32) {
        self.write_u64(n.into());
    }
    fn write_usize(&mut self, n: usize) {
        self.write_u64(n as u64);
    }
    fn write_u128(&mut self, n: u128) {
        self.write_u64(n as u64);
        self.write_u64((n >> 64) as u64);
    }
    fn write_u64(&mut self, n: u64) {
        self.0 = (self.0.rotate_left(5) ^ n).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
}
pub type FastBuild = std::hash::BuildHasherDefault<FastHasher>;
pub type FastMap<K, V> = std::collections::HashMap<K, V, FastBuild>;
pub type FastSet<K> = std::collections::HashSet<K, FastBuild>;
