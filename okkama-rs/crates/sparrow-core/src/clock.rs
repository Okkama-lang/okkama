//! Sparrow's clocks (#1124): the monotonic `Instant` of timers and statistics, the wall and monotonic clocks of
//! the time natives, and the process number that names a clock domain.
//!
//! Natively these are `std::time`'s and `std::process`'s, unchanged. wasm32-unknown-unknown has no clock of its
//! own (`std::time::Instant::now` panics there) and a browser's thread must not block, so a wasm build keeps a
//! virtual timeline (design `okkama-docs/runtime/sparrow-interpreter-1079.md` §4.2–4.3): `now` is the embedder's
//! monotonic clock (`install`; none installed reads 0) plus the time the scheduler skipped. When every task
//! waits for a timer, the scheduler moves the timeline to the earliest deadline (`skip_to`) instead of waiting,
//! so a `sleep` costs nothing, the run's output can be shown on the same timeline, and time measured across
//! computation stays real. The wall clock is the embedder's wall clock at installation plus the timeline's
//! progress since then, so it moves with skipped time too.

#[cfg(not(target_family = "wasm"))]
pub use std::time::Instant;
#[cfg(target_family = "wasm")]
pub use timeline::Instant;

#[cfg(target_family = "wasm")]
pub use timeline::{Source, install, skip_to};

/// Nanoseconds since the Unix epoch (`time.wall_clock`); 0 before it.
#[cfg(not(target_family = "wasm"))]
pub fn wall_nanos() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos()
}

/// Nanoseconds since the Unix epoch (`time.wall_clock`) on the virtual timeline.
#[cfg(target_family = "wasm")]
pub fn wall_nanos() -> u128 {
    u128::from(timeline::wall())
}

/// Nanoseconds since the first read in this process (`time.monotonic_clock`).
pub fn monotonic_nanos() -> u128 {
    static ORIGIN: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    ORIGIN.get_or_init(Instant::now).elapsed().as_nanos()
}

/// The number that names this process's clock domain (`time.clock_domain`): its id natively, 1 on wasm32, where
/// one instance is one run.
pub fn process_id() -> u32 {
    #[cfg(not(target_family = "wasm"))]
    {
        std::process::id()
    }
    #[cfg(target_family = "wasm")]
    {
        1
    }
}

/// The virtual timeline of a build without a clock (wasm32). Its arithmetic is target-independent and tested
/// natively; only a wasm build reads it as its clock.
#[cfg_attr(not(target_family = "wasm"), allow(dead_code))]
mod timeline {
    use std::time::Duration;

    /// The embedder's clocks (wasm32: `performance.now()` and `Date.now()` through the page): `monotonic`
    /// nanoseconds from any fixed origin, never decreasing, and `wall` nanoseconds since the Unix epoch.
    #[derive(Debug, Clone, Copy)]
    pub struct Source {
        pub monotonic: fn() -> u64,
        pub wall: fn() -> u64,
    }

    fn zero() -> u64 {
        0
    }

    /// Without an installed source time stands still except where the scheduler skips it: a run is then
    /// deterministic, its clocks reading only the sleeps it made.
    const STILL: Source = Source { monotonic: zero, wall: zero };

    /// `now = source + skipped`, and the wall clock read at installation together with the timeline's reading then.
    #[derive(Debug)]
    pub(crate) struct Timeline {
        source: Source,
        skipped: u64,
        /// (wall, now) when the source was installed.
        origin: (u64, u64),
    }

    impl Timeline {
        pub(crate) const fn new() -> Self {
            Self { source: STILL, skipped: 0, origin: (0, 0) }
        }

        /// Read `source` from now on. The timeline goes on from where it was: a source behind it is offset forward,
        /// so `now` never decreases.
        pub(crate) fn install(&mut self, source: Source) {
            let before = self.now();
            self.source = source;
            self.skipped = before.saturating_sub((source.monotonic)());
            self.origin = ((source.wall)(), self.now());
        }

        pub(crate) fn now(&self) -> u64 {
            (self.source.monotonic)().saturating_add(self.skipped)
        }

        /// Move the timeline forward to `deadline`; one already past changes nothing.
        pub(crate) fn skip_to(&mut self, deadline: u64) {
            let now = self.now();
            if deadline > now {
                self.skipped = self.skipped.saturating_add(deadline - now);
            }
        }

        pub(crate) fn wall(&self) -> u64 {
            self.origin.0.saturating_add(self.now().saturating_sub(self.origin.1))
        }
    }

    static TIMELINE: std::sync::Mutex<Timeline> = std::sync::Mutex::new(Timeline::new());

    fn with<T>(body: impl FnOnce(&mut Timeline) -> T) -> T {
        body(&mut TIMELINE.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
    }

    /// Install the embedder's clocks (wasm32: before the first run).
    pub fn install(source: Source) {
        with(|timeline| timeline.install(source));
    }

    /// The scheduler's wait on wasm32: every task waits for a timer, the earliest due at `deadline`.
    pub fn skip_to(deadline: Instant) {
        with(|timeline| timeline.skip_to(deadline.0));
    }

    pub(crate) fn wall() -> u64 {
        with(|timeline| timeline.wall())
    }

    /// A moment of the timeline, in nanoseconds; `std::time::Instant`'s interface as Sparrow uses it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct Instant(u64);

    impl Instant {
        pub fn now() -> Self {
            Self(with(|timeline| timeline.now()))
        }

        pub fn duration_since(&self, earlier: Self) -> Duration {
            Duration::from_nanos(self.0.saturating_sub(earlier.0))
        }

        pub fn elapsed(&self) -> Duration {
            Self::now().duration_since(*self)
        }
    }

    impl std::ops::Add<Duration> for Instant {
        type Output = Self;
        fn add(self, duration: Duration) -> Self {
            Self(self.0.saturating_add(u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{Instant, Source, Timeline};
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::Duration;

        // One fake host clock per test, each read through its own function.
        static HOST_A: AtomicU64 = AtomicU64::new(0);
        static HOST_B: AtomicU64 = AtomicU64::new(0);
        fn host_a() -> u64 {
            HOST_A.load(Ordering::Relaxed)
        }
        fn host_b() -> u64 {
            HOST_B.load(Ordering::Relaxed)
        }
        fn wall() -> u64 {
            1_700_000_000_000_000_000
        }

        /// The scheduler's fast-forward: a skip moves `now` to the deadline exactly, a deadline already past
        /// changes nothing, and the host's own progress adds to the skipped time.
        #[test]
        fn skipping_moves_now_to_the_deadline_and_never_back() {
            let mut timeline = Timeline::new();
            assert_eq!(timeline.now(), 0, "no source: time stands still");
            timeline.skip_to(2_000_000_000);
            assert_eq!(timeline.now(), 2_000_000_000);
            timeline.skip_to(1_000);
            assert_eq!(timeline.now(), 2_000_000_000, "a deadline in the past");
            timeline.install(Source { monotonic: host_a, wall });
            assert_eq!(timeline.now(), 2_000_000_000, "installing keeps the timeline where it was");
            HOST_A.store(500, Ordering::Relaxed);
            assert_eq!(timeline.now(), 2_000_000_500, "real time passes on top of skipped time");
            timeline.skip_to(3_000_000_000);
            assert_eq!(timeline.now(), 3_000_000_000);
            HOST_A.store(700, Ordering::Relaxed);
            assert_eq!(timeline.now(), 3_000_000_200);
        }

        /// The wall clock is the host's at installation plus the timeline's progress, skips included.
        #[test]
        fn the_wall_clock_follows_the_timeline() {
            let mut timeline = Timeline::new();
            HOST_B.store(10, Ordering::Relaxed);
            timeline.install(Source { monotonic: host_b, wall });
            assert_eq!(timeline.wall(), wall());
            timeline.skip_to(timeline.now() + 1_000_000_000);
            assert_eq!(timeline.wall(), wall() + 1_000_000_000);
            HOST_B.store(15, Ordering::Relaxed);
            assert_eq!(timeline.wall(), wall() + 1_000_000_005);
        }

        /// Instants order by the timeline and add durations as `std::time::Instant` does.
        #[test]
        fn instants_compare_and_add_durations() {
            let early = Instant(5);
            let late = early + Duration::from_micros(2);
            assert!(late > early);
            assert_eq!(late, Instant(2_005));
            assert_eq!(late.duration_since(early), Duration::from_nanos(2_000));
            assert_eq!(early.duration_since(late), Duration::ZERO);
            assert_eq!(Instant(u64::MAX - 1) + Duration::from_secs(1), Instant(u64::MAX));
        }
    }
}

#[cfg(all(test, not(target_family = "wasm")))]
mod tests {
    /// Natively the clocks are the system's: the wall clock reads `SystemTime`, the monotonic one never goes back.
    #[test]
    fn the_native_clocks_are_the_systems() {
        let system = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let wall = super::wall_nanos();
        assert!(wall.abs_diff(system) < 1_000_000_000, "wall {wall}, system {system}");
        let first = super::monotonic_nanos();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let second = super::monotonic_nanos();
        assert!(second >= first + 2_000_000, "{first} then {second}");
        assert_eq!(super::process_id(), std::process::id());
    }
}
