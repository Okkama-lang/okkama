//! Memory for machine code: one reserved region per kind, filled in order.
//!
//! Cranelift's default provider maps fresh pages for every finalized batch
//! (each lazily compiled function) and, on AArch64 Linux, follows each batch
//! with `membarrier(PRIVATE_EXPEDITED_SYNC_CORE)`: an IPI to every core of the
//! process, and several milliseconds on the first call to register it. The
//! barrier is for code that other threads may run; machine code here runs on
//! the VM thread only, which wrote it. Cleaning the caches for the new range
//! on this core is enough (`clear_cache` ends with an ISB, and a migration to
//! another core is a context synchronization event).
//!
//! Placement (#933): every function starts on a page of its own, `shift`
//! bytes past the page start (0 unless `SPARROW_CORE_CODE_SHIFT` says
//! otherwise). So the offset of every instruction within its page, cache line
//! and fetch block is a property of the function's own code, the same
//! whatever was compiled before it; only the page number follows the compile
//! order. The shift is the layout perturbation of the release micro A/B: the
//! same build under two or three shifts shows how much of a cell is layout.
#![allow(unsafe_code)]

use cranelift_jit::{BranchProtection, JITMemoryKind, JITMemoryProvider};
use cranelift_module::{ModuleError, ModuleResult};
use std::io;

/// Virtual reservation per kind; pages are committed as they are used.
const RESERVE: usize = 1 << 30;

/// The largest `SPARROW_CORE_CODE_SHIFT`: a perturbation, not a layout.
pub const MAX_SHIFT: usize = 1 << 16;

pub(crate) fn page() -> usize {
    // SAFETY: sysconf has no preconditions.
    unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }
}

fn ceil(value: usize, unit: usize) -> usize {
    value.div_ceil(unit) * unit
}

fn floor(value: usize, unit: usize) -> usize {
    value / unit * unit
}

/// One reserved region: `[0, finalized)` has its final protection, new
/// allocations start on the page after it and are writable until finalized.
struct Region {
    base: *mut u8,
    top: usize,
    committed: usize,
    finalized: usize,
    protection: libc::c_int,
    /// The first page written since the last finalization: a shift of a page
    /// or more leaves the pages before it untouched and inaccessible.
    written: Option<usize>,
}

impl Region {
    fn new(protection: libc::c_int) -> Self {
        Self { base: std::ptr::null_mut(), top: 0, committed: 0, finalized: 0, protection, written: None }
    }

    /// `shift`: the bytes the first allocation of a batch starts past its page.
    fn allocate(&mut self, size: usize, align: u64, shift: usize) -> io::Result<*mut u8> {
        if self.base.is_null() {
            // SAFETY: an anonymous reservation without access.
            let base = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    RESERVE,
                    libc::PROT_NONE,
                    libc::MAP_PRIVATE | libc::MAP_ANON | libc::MAP_NORESERVE,
                    -1,
                    0,
                )
            };
            if base == libc::MAP_FAILED {
                return Err(io::Error::last_os_error());
            }
            self.base = base.cast();
        }
        // A batch starts on the page after the last finalized one (`finalize`).
        let first = self.written.is_none();
        let from = if first { self.finalized + shift } else { self.top };
        let start = ceil(from, align.max(1) as usize);
        let end = start.checked_add(size).filter(|&end| end <= RESERVE).ok_or_else(|| {
            io::Error::new(io::ErrorKind::OutOfMemory, "machine code region is full")
        })?;
        let needed = ceil(end, page());
        if needed > self.committed {
            let from = self.committed.max(floor(start, page()));
            // SAFETY: pages inside the reservation, not yet finalized.
            let result = unsafe {
                libc::mprotect(self.base.add(from).cast(), needed - from, libc::PROT_READ | libc::PROT_WRITE)
            };
            if result != 0 {
                return Err(io::Error::last_os_error());
            }
            self.committed = needed;
        }
        self.top = end;
        if first {
            self.written = Some(floor(start, page()));
        }
        okkama_devtrace::count!("jit", "code memory: bytes requested", size);
        // SAFETY: inside the committed part of the reservation.
        Ok(unsafe { self.base.add(start) })
    }

    fn finalize(&mut self, code: bool) -> io::Result<()> {
        let Some(first) = self.written else { return Ok(()) };
        if self.protection == libc::PROT_READ | libc::PROT_WRITE {
            return Ok(());
        }
        let end = ceil(self.top, page());
        okkama_devtrace::count!("jit", "code memory: finalizations");
        okkama_devtrace::count!("jit", "code memory: bytes finalized (whole pages)", end - first);
        // SAFETY: the pages written since the last finalization.
        unsafe {
            let start = self.base.add(first);
            if code {
                wasmtime_jit_icache_coherence::clear_cache(start.cast(), end - first)
                    .map_err(|e| io::Error::other(e.to_string()))?;
            }
            if libc::mprotect(start.cast(), end - first, self.protection) != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        // Later allocations start on a fresh page: finalized pages stay read-only.
        self.finalized = end;
        self.top = end;
        self.written = None;
        Ok(())
    }

    fn free(&mut self) {
        if !self.base.is_null() {
            // SAFETY: the whole reservation made in `allocate`.
            unsafe { libc::munmap(self.base.cast(), RESERVE) };
            *self = Self::new(self.protection);
        }
    }
}

pub struct CodeMemory {
    code: Region,
    writable: Region,
    read_only: Region,
    /// Bytes each function's code starts past its page (#933).
    shift: usize,
}

// SAFETY: the regions are owned and only touched through `&mut self`.
unsafe impl Send for CodeMemory {}

impl CodeMemory {
    /// `shift` at most [`MAX_SHIFT`]; it is rounded up to the function's alignment.
    pub fn new(shift: usize) -> Self {
        debug_assert!(shift <= MAX_SHIFT);
        Self {
            code: Region::new(libc::PROT_READ | libc::PROT_EXEC),
            writable: Region::new(libc::PROT_READ | libc::PROT_WRITE),
            read_only: Region::new(libc::PROT_READ),
            shift,
        }
    }
}

impl JITMemoryProvider for CodeMemory {
    fn allocate(&mut self, size: usize, align: u64, kind: JITMemoryKind) -> io::Result<*mut u8> {
        match kind {
            JITMemoryKind::Executable => self.code.allocate(size, align, self.shift),
            JITMemoryKind::Writable => self.writable.allocate(size, align, 0),
            JITMemoryKind::ReadOnly => self.read_only.allocate(size, align, 0),
        }
    }

    unsafe fn free_memory(&mut self) {
        self.code.free();
        self.writable.free();
        self.read_only.free();
    }

    fn finalize(&mut self, _branch_protection: BranchProtection) -> ModuleResult<()> {
        // Branch target identification is not enabled for this ISA (see `jit`).
        let allocation = |err| ModuleError::Allocation { err };
        self.code.finalize(true).map_err(allocation)?;
        self.read_only.finalize(false).map_err(allocation)?;
        Ok(())
    }
}

impl Drop for CodeMemory {
    fn drop(&mut self) {
        // SAFETY: the JIT module that used this memory is gone with it.
        unsafe { self.free_memory() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile `sizes` as functions, one finalized batch each, and give each
    /// function's offset from the region's base.
    fn place(memory: &mut CodeMemory, sizes: &[usize]) -> Vec<usize> {
        sizes
            .iter()
            .map(|&size| {
                let code = memory.allocate(size, 4, JITMemoryKind::Executable).unwrap();
                // SAFETY: the allocation is writable until finalized.
                unsafe { code.write_bytes(0xd5, size) };
                memory.finalize(BranchProtection::None).unwrap();
                code as usize - memory.code.base as usize
            })
            .collect()
    }

    const SIZES: [usize; 6] = [100, 5000, 4, 4096, 20000, 4092];

    #[test]
    fn every_function_starts_on_a_page_of_its_own() {
        let mut memory = CodeMemory::new(0);
        let offsets = place(&mut memory, &SIZES);
        for (offset, size) in offsets.iter().zip(SIZES) {
            assert_eq!(offset % page(), 0, "{offset:#x} ({size} bytes)");
        }
        for pair in offsets.windows(2) {
            assert!(pair[1] >= ceil(pair[0] + 1, page()), "{pair:x?}: two functions share a page");
        }
    }

    #[test]
    fn the_offset_within_the_page_does_not_depend_on_compile_order() {
        for shift in [0, 64, 192, 1000, 4096 + 128] {
            let (mut forward, mut backward) = (CodeMemory::new(shift), CodeMemory::new(shift));
            let mut reversed = SIZES;
            reversed.reverse();
            let a = place(&mut forward, &SIZES);
            let mut b = place(&mut backward, &reversed);
            b.reverse();
            // A shift is rounded up to the function's alignment (4 here).
            let expected = ceil(shift, 4) % page();
            for (x, y) in a.iter().zip(&b) {
                assert_eq!((x % page(), y % page()), (expected, expected), "shift {shift}");
            }
        }
    }

    #[test]
    fn a_batch_of_several_allocations_is_shifted_once() {
        let mut memory = CodeMemory::new(256);
        let first = memory.allocate(100, 16, JITMemoryKind::Executable).unwrap() as usize;
        let second = memory.allocate(100, 16, JITMemoryKind::Executable).unwrap() as usize;
        assert_eq!(first - memory.code.base as usize, 256);
        assert_eq!(second - first, 112, "the second allocation follows the first");
        memory.finalize(BranchProtection::None).unwrap();
        let third = memory.allocate(8, 16, JITMemoryKind::Executable).unwrap() as usize;
        assert_eq!((third - memory.code.base as usize) % page(), 256);
        // Data is not code: its regions are never shifted.
        let data = memory.allocate(8, 8, JITMemoryKind::ReadOnly).unwrap() as usize;
        assert_eq!(data - memory.read_only.base as usize, 0);
    }

    #[test]
    fn a_shift_past_a_page_leaves_the_skipped_pages_untouched() {
        let shift = 3 * page() + 64;
        let mut memory = CodeMemory::new(shift);
        let offsets = place(&mut memory, &[100, 100]);
        assert_eq!(offsets[0], shift);
        assert_eq!(offsets[1], 4 * page() + shift);
        let mut info = 0u8;
        // SAFETY: mincore on the reservation's first page, which was never committed.
        let status = unsafe { libc::mincore(memory.code.base.cast(), page(), (&raw mut info).cast()) };
        assert_eq!((status, info & 1), (0, 0), "a skipped page became resident");
    }
}
