//! Runtime tables (`okkama-runtime-core/data/*.dat`): the core's providers read the same tables as
//! okkama-runtime-core's, so they are embedded and decompressed once (#1109: a copy of this module embedded
//! `unicode.dat`, `idna.dat` and `psl.dat` a second time, ~349 KB).
pub use okkama_runtime_core::data::*;
