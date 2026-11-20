//! Word representations shared by the code generator and the runtime: how a word of a static type is traced
//! and converted, and the object kind of the heap header.

/// A physical storage word that can contain a GC pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraceSlot {
    pub index: usize,
    pub repr: Repr,
}

/// The heap header's kind of an object.
pub const OBJECT_KIND: u8 = 1;

/// How a word of a given static type is traced and converted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Repr {
    /// Tagged small integer or BigInt pointer.
    Int = 0,
    /// Raw bits: float, bool, byte, char.
    Raw = 1,
    /// Pointer to a heap object (or zero).
    Ref = 2,
}

impl Repr {
    pub fn from_bits(bits: u64) -> Self {
        match bits {
            0 => Self::Int,
            1 => Self::Raw,
            _ => Self::Ref,
        }
    }
    /// The heap object a word of this representation points to, if any.
    #[inline]
    pub fn pointer(self, word: u64) -> Option<u64> {
        match self {
            Self::Int if word & 1 == 0 && word != 0 => Some(word),
            Self::Ref if word != 0 => Some(word),
            _ => None,
        }
    }
}
