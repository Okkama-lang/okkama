//! Hostile but memory-valid native calls must fail cleanly, never panic or crash.
#![no_main]
mod ext_body;
libfuzzer_sys::fuzz_target!(|data: &[u8]| ext_body::exercise(data));
