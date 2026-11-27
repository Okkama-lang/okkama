//! Execution budgets (hardening H-04): steps, memory, output and external
//! interruption. Every budget is off by default; machine code polls only when
//! one is set, so an unbounded run compiles to the same code as before.
//!
//! - Steps: one per function entry and per loop back-edge. Machine code
//!   decrements `Ctx::fuel` there and calls `rt_poll` when a slice is used up;
//!   the poll charges the slice, refills, and checks the other budgets.
//! - Memory: bytes the process allocator holds beyond the start of the run,
//!   read by the embedder's [`MemoryProbe`] (the CLIs read mimalloc). It covers
//!   the core heap, whose pages and large objects are allocator blocks, and
//!   Rust-side values (BigInt, builders, maps). Polls check it; machine-code
//!   allocation is refused before it would cross it; large heap allocations
//!   from Rust probe it ([`Watch`]); a BigInt product is checked before the
//!   multiplication. [`MemoryScope::Runtime`] measures this runtime's heap
//!   instead (#269): its pages and large objects, and the Rust memory its
//!   objects own, so that runtimes in one process keep budgets of their own.
//! - Output: bytes written to stdout and stderr by the program.
//! - Interruption: a flag the host sets from any thread ([`Limits::interrupt`]);
//!   polls and the scheduler's waits see it.
//!
//! Exhaustion stops the run with its own status (`EXHAUSTED`): handlers do not
//! catch it, destroys do not run, and the runtime is discarded with its heap.
#![allow(unsafe_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub use okkama_runtime_core::budget::{Counted, MemoryScope, counted_bytes, thread_counted_bytes};

/// Steps between two polls when no step budget is nearer.
pub const SLICE: u64 = 1 << 16;

/// Budgets of one run; `None` is unbounded.
#[derive(Debug, Clone, Default)]
pub struct Limits {
    /// Function entries and loop back-edges.
    pub steps: Option<u64>,
    /// Allocator bytes beyond the start of the run, or the runtime's own
    /// heap ([`MemoryScope`]); also caps the machine stack, so deep recursion
    /// fails as a stack overflow within it.
    pub memory: Option<u64>,
    /// What `memory` measures.
    pub memory_scope: MemoryScope,
    /// Bytes written to stdout and stderr.
    pub output: Option<u64>,
    /// Set by the host to stop the run.
    pub interrupt: Option<Arc<AtomicBool>>,
    /// Native groups the program may use; `None` allows every native (the
    /// CLIs). Natives outside every group (pure computation, stdout and
    /// stderr) are always allowed. Process workers need `Process`.
    pub natives: Option<Vec<NativeGroup>>,
    /// The natives an embedding offers, by name (#1125, [`Profile`]); `None` is every native. It is checked
    /// before `natives`, with its own refusal text.
    pub profile: Option<&'static Profile>,
    /// Steps between two polls (#1136); `None` is [`SLICE`]. An embedding that batches output hands it over at each
    /// poll, so this bounds how long a write waits while the program computes.
    pub slice: Option<u64>,
}

/// What an embedding of Sparrow offers a program, as data (#1125): the host groups it allows, the natives it
/// allows inside the groups it refuses, and the pure natives it lacks. The same profile refuses the same
/// natives with the same text natively and in wasm, so a native run under [`Limits::playground`] shows what the
/// playground shows. Natives are named by module and the beginning of their name (`""`: the whole module).
#[derive(Debug, PartialEq, Eq)]
pub struct Profile {
    /// The refusal's "Native X is not available in the {name}".
    pub name: &'static str,
    pub groups: &'static [NativeGroup],
    /// Allowed although their group is not.
    pub allowed: &'static [(&'static str, &'static str)],
    /// Pure, and refused: their provider is not in the embedding.
    pub refused: &'static [(&'static str, &'static str)],
    pub steps: u64,
    pub memory: u64,
    pub output: u64,
    /// Steps between two polls ([`Limits::slice`]).
    pub slice: u64,
}

/// The playground (#1079 §5.2, #1125): Sparrow without `jit` and `host`, in a browser. Clocks and secure
/// randomness come from the page; standard input is a buffer given before the run; workers are emulated in the same
/// instance (#1127, owner's decision 8); what reaches files, sockets, processes, the environment or the terminal is
/// refused, and so are the pure providers left out of the wasm build: zstd and Brotli (`compression` codecs 3 and 4
/// fail inside the codec instead), collation (the system's ICU) and X.509 (aws-lc). RSA and ECDSA verification, also
/// aws-lc's, are not refused by the profile: they fail inside the operation as unsupported, while the rest of the
/// cryptography (digests, AEAD, ECDSA signing) runs on RustCrypto (#1124). Its budgets are
/// the starting ones of §5.5. The owner's decisions 4, 6, 7, 9 and 10 of §9 are taken as recommended there and
/// await the owner; 8 (workers) and 3 (time fast-forward) are decided.
pub const PLAYGROUND: Profile = Profile {
    name: "playground",
    groups: &[NativeGroup::Clock, NativeGroup::Random],
    allowed: &[
        ("runtime/stdio", "stdin_"),
        // The processors workers can use (#1127): one in the playground, whose workers are emulated one at a time.
        ("runtime/system", "system_cpu_count"),
    ],
    refused: &[("runtime/collation", ""), ("runtime/x509", "")],
    steps: 50_000_000,
    memory: 256 << 20,
    output: 1 << 20,
    slice: SLICE_PLAYGROUND,
};

/// The playground's steps between two polls (#1136): its output waits at most this long while a program computes.
/// Measured in wasm (Node, release): 65 536 steps take 1.3–5 ms, so a write waits at most about 6 ms and 60 000 lines
/// arrive in 11 chunks; 16 384 and 4 096 steps gave 41 and 162 chunks for under 2 ms less wait (decisions.md, #1136).
pub const SLICE_PLAYGROUND: u64 = SLICE;

impl Profile {
    /// Does the profile offer `native`?
    pub fn offers(&self, native: okkama_runtime_abi::Native) -> bool {
        let symbol = native.symbol();
        let module = symbol.split('@').next().unwrap_or(symbol);
        let name = symbol.split("::").nth(1).unwrap_or("");
        let named = |list: &[(&str, &str)]| list.iter().any(|&(m, prefix)| m == module && name.starts_with(prefix));
        match Reach::of(native) {
            Reach::Group(group) => self.groups.contains(&group) || named(self.allowed),
            Reach::Pure => !named(self.refused),
            Reach::Unclassified => false,
        }
    }
}

/// Natives that reach the host, by what they reach (H-14).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeGroup {
    /// Files, directories, file snapshots, SQLite databases.
    Fs,
    /// Sockets, TLS, PostgreSQL.
    Net,
    /// Child processes (and process workers).
    Process,
    /// Environment variables, users and groups, host facts (host name, CPUs, storage), the preferred languages
    /// and the system's certificates.
    Env,
    /// Wall and monotonic clocks, sleeping.
    Clock,
    /// Secure random bytes.
    Random,
    /// The controlling terminal and standard input.
    Terminal,
}

/// What a native reaches: the host through one group, nothing beyond the
/// program (computation, stdout and stderr), or unknown: a module this table
/// does not name, refused while the groups are restricted (#361).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    Group(NativeGroup),
    Pure,
    Unclassified,
}

/// The natives of `runtime/system` that state what the build is (its platform, architecture and byte order):
/// constants of the binary, not facts of the host it runs on (#1133).
const SYSTEM_CONSTANTS: &[&str] = &["system_platform", "system_architecture", "system_byte_order"];

/// Modules whose natives compute without reaching the host. A module added
/// to the runtime is in no list until someone puts it in one, and the test
/// `every_native_is_classified` fails until then.
const PURE: &[&str] = &[
    "runtime/bigint", "runtime/binary", "runtime/buffer", "runtime/bytes", "runtime/checksum", "runtime/cldr", "runtime/collation", "runtime/compression",
    "runtime/collections", "runtime/crypto", "runtime/db", "runtime/debug",
    "runtime/decimal", "runtime/diff", "runtime/format", "runtime/hex", "runtime/host_crypto",
    "runtime/hpack", "runtime/idna", "runtime/input_state", "runtime/io_decorators", "runtime/json",
    "runtime/layout", "runtime/math_aggregate", "runtime/net", "runtime/number_text", "runtime/pem", "runtime/postgres",
    "runtime/public_suffix", "runtime/quoted_printable", "runtime/random_distribution",
    "runtime/regex", "runtime/secret", "runtime/single_byte", "runtime/statistics",
    "runtime/template", "runtime/testing", "runtime/time_zone",
    "runtime/unicode_character", "runtime/unicode_normalization",
    "runtime/utf", "runtime/uuid", "runtime/stdio", "runtime/base32",
    "runtime/base64", "runtime/base85", "runtime/x509",
];

impl NativeGroup {
    /// The group of `native`, if it reaches the host.
    pub fn of(native: okkama_runtime_abi::Native) -> Option<Self> {
        match Reach::of(native) {
            Reach::Group(group) => Some(group),
            Reach::Pure | Reach::Unclassified => None,
        }
    }
}

impl Reach {
    pub fn of(native: okkama_runtime_abi::Native) -> Self {
        static REACH: std::sync::OnceLock<Vec<Reach>> = std::sync::OnceLock::new();
        let reach = REACH.get_or_init(|| {
            let size = okkama_runtime_abi::Native::ALL.iter().map(|&n| n as usize + 1).max().unwrap_or(0);
            let mut reach = vec![Reach::Unclassified; size];
            for &native in okkama_runtime_abi::Native::ALL {
                reach[native as usize] = Self::classify(native);
            }
            reach
        });
        reach.get(native as usize).copied().unwrap_or(Reach::Unclassified)
    }

    fn classify(native: okkama_runtime_abi::Native) -> Self {
        let symbol = native.symbol();
        let module = symbol.split('@').next().unwrap_or(symbol);
        let name = symbol.split("::").nth(1).unwrap_or("");
        Self::Group(match module {
            "runtime/fs" | "runtime/fs_root" | "runtime/snapshot" | "runtime/database" => NativeGroup::Fs,
            "runtime/network" | "runtime/tls_transport" => NativeGroup::Net,
            "runtime/process" | "runtime/signal" => NativeGroup::Process,
            "runtime/system" if name.starts_with("secure_") => NativeGroup::Random,
            // What the build is, a constant of the binary (#1133).
            "runtime/system" if SYSTEM_CONSTANTS.iter().any(|constant| name.split('/').next() == Some(constant)) => return Self::Pure,
            "runtime/environment" | "runtime/account" | "runtime/system" => NativeGroup::Env,
            // The host's language preferences and trust anchors (#361).
            "runtime/locale" => NativeGroup::Env,
            "runtime/x509" if name.starts_with("system_certificates") => NativeGroup::Env,
            "runtime/time" => NativeGroup::Clock,
            "runtime/random" => NativeGroup::Random,
            "runtime/application" => NativeGroup::Terminal,
            "runtime/stdio" if name.starts_with("stdin") => NativeGroup::Terminal,
            _ if PURE.contains(&module) => return Self::Pure,
            _ => return Self::Unclassified,
        })
    }
}

impl Limits {
    /// Is `native` allowed? The refusal names its group.
    #[inline]
    pub fn permits(&self, native: okkama_runtime_abi::Native) -> Result<(), String> {
        if let Some(profile) = self.profile
            && !profile.offers(native)
        {
            return Err(format!("Native {} is not available in the {}", native.symbol(), profile.name));
        }
        let Some(allowed) = &self.natives else {
            return Ok(());
        };
        match Reach::of(native) {
            Reach::Group(group) if !allowed.contains(&group) => {
                Err(format!("Native {} needs the {group:?} permission", native.symbol()))
            }
            Reach::Unclassified => Err(format!(
                "Native {} is in no native group; it is refused while the native groups are restricted",
                native.symbol()
            )),
            Reach::Group(_) | Reach::Pure => Ok(()),
        }
    }
}

impl Limits {
    /// Steps between two polls: [`Limits::slice`] or [`SLICE`], at least one.
    pub fn slice(&self) -> u64 {
        self.slice.unwrap_or(SLICE).max(1)
    }

    /// The playground's limits (#1125): its [`PLAYGROUND`] profile and budgets, memory measured on the runtime's
    /// own heap (one runtime per page worker, no process allocator to read).
    pub fn playground() -> Self {
        Self {
            steps: Some(PLAYGROUND.steps),
            memory: Some(PLAYGROUND.memory),
            memory_scope: MemoryScope::Runtime,
            output: Some(PLAYGROUND.output),
            profile: Some(&PLAYGROUND),
            slice: Some(PLAYGROUND.slice),
            ..Self::default()
        }
    }

    /// May the program start a worker? With the `Process` group when the groups are restricted. The playground's
    /// profile allows them: its workers are emulated in the same instance (owner's decision 8 of #1079, #1127).
    pub fn permits_workers(&self) -> Result<(), String> {
        match &self.natives {
            Some(allowed) if !allowed.contains(&NativeGroup::Process) => Err("Process workers need the Process permission".to_string()),
            _ => Ok(()),
        }
    }

    /// Does machine code poll (any budget or interruption set)?
    pub fn polled(&self) -> bool {
        self.steps.is_some() || self.memory.is_some() || self.interrupt.is_some()
    }

    pub fn interrupted(&self) -> bool {
        self.interrupt.as_ref().is_some_and(|flag| flag.load(Ordering::Relaxed))
    }

    /// Apply a command-line budget option: `--max-steps N`, `--max-memory
    /// SIZE`, `--max-output SIZE` (bytes, or with a K/M/G suffix of 1024s) or
    /// `--timeout SECONDS` (wall clock; interrupts the run). Ok(false) when
    /// `option` is not one of them.
    pub fn option(&mut self, option: &str, value: &str) -> Result<bool, String> {
        let invalid = || format!("invalid value `{value}` for {option}");
        match option {
            "--max-steps" => self.steps = Some(value.parse().map_err(|_| invalid())?),
            "--max-memory" => self.memory = Some(size(value).ok_or_else(invalid)?),
            "--max-output" => self.output = Some(size(value).ok_or_else(invalid)?),
            "--timeout" => {
                let seconds: f64 = value.parse().ok().filter(|s: &f64| s.is_finite() && *s >= 0.0).ok_or_else(invalid)?;
                let flag = self.interrupt.get_or_insert_with(Arc::default).clone();
                std::thread::Builder::new()
                    .name("sparrow-timeout".into())
                    .spawn(move || {
                        std::thread::sleep(std::time::Duration::from_secs_f64(seconds));
                        flag.store(true, Ordering::Relaxed);
                    })
                    .map_err(|e| format!("{option}: {e}"))?;
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}

/// Command-line help of the budget options.
pub const OPTIONS_HELP: &str = "\
    --max-steps N      stop after N steps (function entries and loop iterations)
    --max-memory SIZE  stop when the run holds more than SIZE bytes (K/M/G suffixes)
    --max-output SIZE  stop when stdout and stderr exceed SIZE bytes
    --timeout SECONDS  interrupt the run after SECONDS of wall-clock time
";

/// Bytes with an optional K, M or G suffix (powers of 1024).
fn size(text: &str) -> Option<u64> {
    let (digits, unit) = match text.char_indices().last()? {
        (at, 'K' | 'k') => (&text[..at], 1u64 << 10),
        (at, 'M' | 'm') => (&text[..at], 1 << 20),
        (at, 'G' | 'g') => (&text[..at], 1 << 30),
        _ => (text, 1),
    };
    digits.parse::<u64>().ok()?.checked_mul(unit)
}

/// What ran out; the text of the failure report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exhausted {
    Steps,
    Memory,
    Output,
    Interrupted,
}

impl Exhausted {
    pub fn message(self) -> &'static str {
        match self {
            Exhausted::Steps => "Execution budget exhausted: steps",
            Exhausted::Memory => "Execution budget exhausted: memory",
            Exhausted::Output => "Execution budget exhausted: output",
            Exhausted::Interrupted => "Execution interrupted",
        }
    }
}

/// The bytes the process allocator holds now; set by the embedder (the CLIs
/// read mimalloc's committed memory). A memory budget needs one.
pub type MemoryProbe = fn() -> usize;

static PROBE: std::sync::OnceLock<MemoryProbe> = std::sync::OnceLock::new();

/// Install the memory probe once per process; later calls are ignored.
pub fn set_memory_probe(probe: MemoryProbe) {
    let _ = PROBE.set(probe);
}

pub fn memory_probe() -> Option<MemoryProbe> {
    PROBE.get().copied()
}

/// mimalloc's committed bytes, a probe for processes whose allocator is
/// mimalloc (the CLIs, feature `mimalloc`); it needs mimalloc linked.
#[cfg(feature = "mimalloc")]
pub fn mimalloc_committed() -> usize {
    unsafe extern "C" {
        fn mi_process_info(
            elapsed: *mut usize,
            user: *mut usize,
            system: *mut usize,
            rss: *mut usize,
            peak_rss: *mut usize,
            commit: *mut usize,
            peak_commit: *mut usize,
            faults: *mut usize,
        );
    }
    let null = std::ptr::null_mut();
    let mut commit = 0;
    // SAFETY: mimalloc writes only the non-null outputs.
    unsafe { mi_process_info(null, null, null, null, null, &mut commit, null, null) };
    commit
}

/// The fuel cell of a runtime: machine code decrements it at every step (its
/// address is a constant of the code) and polls below zero. It is touched only
/// through its raw pointer, never borrowed.
#[derive(Debug)]
pub struct Fuel(*mut i64);

impl Default for Fuel {
    fn default() -> Self {
        Self(Box::into_raw(Box::new(0)))
    }
}

impl Fuel {
    pub fn address(&self) -> i64 {
        self.0 as i64
    }

    pub fn set(&self, value: i64) {
        // SAFETY: a live allocation owned by `self`.
        unsafe { self.0.write_volatile(value) }
    }

    /// One step, as machine code takes it at an entry or a back-edge (#1120: the interpreter): true when the
    /// slice is used up and `rt_poll` must run.
    #[inline]
    pub(crate) fn step(&self) -> bool {
        // SAFETY: a live allocation owned by `self`; machine code writes it only on this thread.
        unsafe {
            let left = self.0.read_volatile() - 1;
            self.0.write_volatile(left);
            left < 0
        }
    }
}

impl Drop for Fuel {
    fn drop(&mut self) {
        // SAFETY: allocated by `default`, freed once.
        drop(unsafe { Box::from_raw(self.0) });
    }
}

/// The memory of one run: the probe's reading at its start.
#[derive(Debug, Clone, Copy)]
pub struct Meter {
    probe: MemoryProbe,
    base: usize,
}

impl Meter {
    pub fn start(probe: MemoryProbe) -> Self {
        Self { probe, base: probe() }
    }

    /// Bytes held beyond the start (negative after freeing older ones). In `i64`, not `isize`: a budget's
    /// arithmetic must not depend on the pointer width (wasm32, #1124).
    pub fn used(&self) -> i64 {
        (self.probe)() as i64 - self.base as i64
    }
}

/// The heap's check of large allocations against a memory budget: past it,
/// the fuel is emptied so the next step polls and stops the run. Probing is
/// a system call, so it happens once per `PROBE_BYTES` of heap growth. A
/// budget of the runtime (`meter` None) reads the heap's own count instead.
#[derive(Debug)]
pub struct Watch {
    pub meter: Option<Meter>,
    pub limit: u64,
    pub fuel: *mut i64,
    unprobed: usize,
}

/// Heap growth between two probes of a watched run.
const PROBE_BYTES: usize = 256 << 10;

impl Watch {
    pub fn new(meter: Option<Meter>, limit: u64, fuel: &Fuel) -> Self {
        Self { meter, limit, fuel: fuel.0, unprobed: 0 }
    }

    /// `bytes` more are being allocated; `held` is what the heap holds.
    pub fn grow(&mut self, bytes: usize, held: usize) {
        self.unprobed += bytes;
        if self.unprobed < PROBE_BYTES {
            return;
        }
        self.unprobed = 0;
        let used = self.meter.map_or(held as i64, |meter| meter.used());
        if used > self.limit as i64 {
            // SAFETY: the runtime owns both the heap and its fuel cell.
            unsafe { self.fuel.write_volatile(0) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use okkama_runtime_abi::Native;

    /// Every native is in a group or among the pure ones (#361): a module
    /// added without a place in the table fails here instead of passing a
    /// host that restricted the groups.
    #[test]
    fn every_native_is_classified() {
        let unclassified: Vec<&str> = Native::ALL
            .iter()
            .filter(|&&native| Reach::of(native) == Reach::Unclassified)
            .map(|native| native.symbol())
            .collect();
        assert!(unclassified.is_empty(), "natives in no group and not pure: {unclassified:?}");
    }

    /// The compiler's companion catalogue (#569 C7) and the host permission groups agree on what
    /// reaches the host: a group exactly for a host effect, pure for computation and the standard streams.
    #[test]
    fn the_companion_catalogue_agrees_with_the_groups() {
        use okkama_runtime_abi::catalogue::{classify, Effect};
        for &native in Native::ALL {
            let effect = classify(native).expect("classified").effect;
            let agrees = match Reach::of(native) {
                Reach::Group(_) => effect == Effect::Host,
                Reach::Pure => matches!(effect, Effect::Pure | Effect::Stdio),
                Reach::Unclassified => false,
            };
            assert!(agrees, "{}: {:?} vs {effect:?}", native.symbol(), Reach::of(native));
        }
    }

    #[test]
    fn the_preferred_languages_need_env() {
        let native = *Native::ALL.iter().find(|native| native.symbol().starts_with("runtime/locale@")).unwrap();
        assert_eq!(Reach::of(native), Reach::Group(NativeGroup::Env));
        let limits = Limits { natives: Some(vec![NativeGroup::Clock]), ..Limits::default() };
        assert!(limits.permits(native).unwrap_err().contains("needs the Env permission"));
        let everything = Limits::default();
        assert!(everything.permits(native).is_ok());
    }

    fn native(symbol: &str) -> Native {
        *Native::ALL.iter().find(|native| native.symbol().starts_with(symbol)).unwrap_or_else(|| panic!("{symbol}"))
    }

    /// #1133: every native of `prefix` computes without the host: pure, so a run with every group denied runs it,
    /// while `neighbour`, of the same module or the group it was in, still needs `group`.
    fn pure_family(prefix: &str, count: usize, neighbour: &str, group: NativeGroup) {
        let family: Vec<Native> = Native::ALL.iter().copied().filter(|native| native.symbol().starts_with(prefix)).collect();
        assert_eq!(family.len(), count, "{prefix}");
        let denied = Limits { natives: Some(Vec::new()), ..Limits::default() };
        for native in family {
            assert_eq!(Reach::of(native), Reach::Pure, "{}", native.symbol());
            assert_eq!(denied.permits(native), Ok(()), "{}", native.symbol());
            assert!(PLAYGROUND.offers(native), "{}", native.symbol());
        }
        let neighbour = native(neighbour);
        assert_eq!(Reach::of(neighbour), Reach::Group(group), "{}", neighbour.symbol());
        assert!(denied.permits(neighbour).unwrap_err().ends_with(&format!("needs the {group:?} permission")));
    }

    #[test]
    fn standard_input_line_buffers_are_pure() {
        pure_family("runtime/input_state@1::", 9, "runtime/stdio@1::stdin_read/", NativeGroup::Terminal);
    }

    #[test]
    fn ip_address_text_is_pure() {
        pure_family("runtime/net@1::ip_", 2, "runtime/network@1::", NativeGroup::Net);
    }

    #[test]
    fn postgres_result_pages_are_pure() {
        pure_family("runtime/postgres@1::pg_page", 7, "runtime/tls_transport@1::", NativeGroup::Net);
    }

    #[test]
    fn the_date_times_of_a_named_zone_are_pure() {
        pure_family("runtime/time_zone@1::timezone_local_", 2, "runtime/locale@1::", NativeGroup::Env);
    }

    #[test]
    fn what_the_build_is_is_pure() {
        for name in SYSTEM_CONSTANTS {
            pure_family(&format!("runtime/system@1::{name}/"), 1, "runtime/system@1::system_host_name", NativeGroup::Env);
        }
        assert_eq!(Reach::of(native("runtime/system@1::secure_random")), Reach::Group(NativeGroup::Random));
    }

    /// #1125: the playground refuses with its own text whatever it lacks, a group it allows or a stdin exception
    /// passes, a pure native passes unless its provider is left out, and without the profile the texts are the old ones.
    #[test]
    fn the_playground_profile_refuses_with_its_own_text() {
        let limits = Limits::playground();
        let refused = |symbol: &str| limits.permits(native(symbol)).unwrap_err();
        assert_eq!(refused("runtime/fs@1::"), format!("Native {} is not available in the playground", native("runtime/fs@1::").symbol()));
        assert!(refused("runtime/collation@1::").ends_with("is not available in the playground"));
        assert!(refused("runtime/x509@1::certificate_facts").ends_with("is not available in the playground"));
        assert!(refused("runtime/system@1::system_host_name").ends_with("is not available in the playground"));
        assert!(refused("runtime/application@").ends_with("is not available in the playground"));
        assert!(refused("runtime/network@1::").ends_with("is not available in the playground"));
        for symbol in ["runtime/net@1::ip_parse", "runtime/input_state@1::", "runtime/time@1::wall_clock", "runtime/system@1::secure_random", "runtime/stdio@1::stdin_read/", "runtime/stdio@1::stdout_write", "runtime/json@", "runtime/random@"] {
            assert_eq!(limits.permits(native(symbol)), Ok(()), "{symbol}");
        }
        assert_eq!(limits.permits_workers(), Ok(()), "#1127: the playground's workers are emulated");
        let groups = Limits { natives: Some(vec![NativeGroup::Clock]), ..Limits::default() };
        assert!(groups.permits(native("runtime/fs@1::")).unwrap_err().ends_with("needs the Fs permission"));
        assert_eq!(groups.permits_workers().unwrap_err(), "Process workers need the Process permission");
        assert_eq!(Limits::default().permits_workers(), Ok(()));
        assert_eq!((limits.steps, limits.memory, limits.output), (Some(50_000_000), Some(256 << 20), Some(1 << 20)));
        assert_eq!(limits.memory_scope, MemoryScope::Runtime);
    }

    /// #1127 (design §9 #6, awaiting the owner): every native of the digests, HMAC and HKDF (`crypto`,
    /// `host_crypto`), UUIDs and checksums is in the playground's profile.
    #[test]
    fn the_playground_offers_digests_macs_uuids_and_checksums() {
        let modules = ["runtime/crypto@", "runtime/host_crypto@", "runtime/uuid@", "runtime/checksum@"];
        let natives: Vec<Native> =
            Native::ALL.iter().copied().filter(|native| modules.iter().any(|module| native.symbol().starts_with(module))).collect();
        assert!(natives.len() >= 8, "{natives:?}");
        for native in natives {
            assert!(PLAYGROUND.offers(native), "{}", native.symbol());
        }
    }

    /// #1079 §5.2: built as the playground is (no `jit`, no `host`), the profile offers a native exactly when the
    /// build has its provider, so the profile's refusal is the only one a playground program meets.
    #[cfg(not(feature = "host"))]
    #[test]
    fn the_playground_profile_is_what_the_pure_build_provides() {
        let mut offered_unprovided = Vec::new();
        let mut provided_refused = Vec::new();
        for &native in Native::ALL {
            match (PLAYGROUND.offers(native), crate::natives::supported(native)) {
                (true, false) => offered_unprovided.push(native.symbol()),
                (false, true) => provided_refused.push(native.symbol()),
                _ => {}
            }
        }
        assert!(offered_unprovided.is_empty() && provided_refused.is_empty(), "offered without a provider: {offered_unprovided:?}\nprovided but refused: {provided_refused:?}");
    }
}
