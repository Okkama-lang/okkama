//! Persistent collections (#246, #252).
//!
//! - [`PMap`]: the ordered persistent dictionary behind Sparrow maps — a
//!   B-tree of `Rc` nodes whose versions share every untouched subtree.
//! - [`Trie`]: a 32-way trie with a tail buffer (Bagwell; Clojure's
//!   `PersistentVector`), measured for arrays in 0.4.3; `push` and `set`
//!   copy one path and reads walk ⌈log₃₂ n⌉ levels. Sparrow's arrays and
//!   strings are persistent through its own heap; the trie stays as the
//!   measured alternative (`examples/pbench.rs`).
//!
//! `slots` is the fixed-capacity node storage of `PMap`, the crate's only
//! `unsafe` code. The shared append buffer `Frontier` of the 0.4.3 prototype
//! was removed in 0.6.4: its `fold` held a reference into the buffer while a
//! foreign closure could reallocate it (#1015).

pub mod pmap;
mod slots;
pub mod trie;

pub use pmap::PMap;
pub use trie::Trie;
