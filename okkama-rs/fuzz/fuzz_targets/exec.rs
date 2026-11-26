//! Programs that compile run on the Sparrow core — machine code — under
//! budgets and with no host access: every outcome is a result, a failure
//! report or an exhausted budget, never a crash. Programs link against the
//! repository's std tree (#1091), so std's async intrinsics run too.
#![no_main]
#[path = "support/linking.rs"]
mod linking;
#[path = "support/lowering.rs"]
mod lowering;

use libfuzzer_sys::fuzz_target;
use sparrow_core::budget::{Counted, counted_bytes, set_memory_probe};

#[global_allocator]
static ALLOCATOR: Counted<std::alloc::System> = Counted(std::alloc::System);

fuzz_target!(|data: &[u8]| {
    set_memory_probe(counted_bytes);
    let _ = linking::run(data);
});
