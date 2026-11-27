//! External waits: native I/O requests (Rust futures of the providers) and
//! timers. A driver polls its request with a waker that names its frame; the
//! waker may fire on any thread (SQLite workers reply through channels). When
//! no frame is ready the scheduler waits until a waker fires or the earliest
//! timer is due, then wakes those frames.
//!
//! The queue of woken frames, the requests, the wakers and the firing of due
//! timers are the same in every build; only the wait differs (#1124): with the
//! host it blocks in Tokio, turning the reactor (`wait_tokio`); natively without
//! the host it sleeps on a condition variable until a waker or the earliest
//! timer (`wait_sleeping`); on wasm32, which must not block, it moves the
//! clock to the earliest timer (`wait_skipping`, see `clock`).
use crate::clock::Instant;
use crate::runtime::Runtime;
use crate::word::Word;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Wake, Waker};

/// How long the scheduler spins for a pending native reply before sleeping.
#[cfg(feature = "host")]
const SPIN_MICROS: u64 = 50;
/// The longest a spinning scheduler goes without turning the reactor when
/// every waiting request is answered by a provider thread (#943): local
/// tasks (process reapers, capture pipes) still progress.
#[cfg(feature = "host")]
const REACTOR_AT_LEAST: std::time::Duration = std::time::Duration::from_millis(1);
/// How often a waiting scheduler looks at the interrupt flag.
#[cfg(not(target_family = "wasm"))]
const INTERRUPT_LOOK: std::time::Duration = std::time::Duration::from_millis(20);

#[derive(Debug, Default)]
struct Shared {
    woken: Mutex<Vec<Word>>,
    pending: AtomicBool,
    notify: Notify,
}

/// What a waker signals to a waiting scheduler: Tokio's notification with the host.
#[cfg(feature = "host")]
type Notify = tokio::sync::Notify;

/// Natively without the host: a condition variable the scheduler sleeps on. The waker notifies holding the
/// lock, and the scheduler reads `pending` holding it, so no wake is lost between the look and the sleep.
#[cfg(all(not(feature = "host"), not(target_family = "wasm")))]
#[derive(Debug, Default)]
struct Notify {
    lock: Mutex<()>,
    condition: std::sync::Condvar,
}

#[cfg(all(not(feature = "host"), not(target_family = "wasm")))]
impl Notify {
    fn notify_one(&self) {
        let _held = self.lock.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        self.condition.notify_one();
    }
}

/// wasm32 has no other thread to wake the scheduler: a waker only marks its frame.
#[cfg(target_family = "wasm")]
#[derive(Debug, Default)]
struct Notify;

#[cfg(target_family = "wasm")]
impl Notify {
    fn notify_one(&self) {}
}

#[derive(Debug)]
struct FrameWaker {
    frame: Word,
    shared: Arc<Shared>,
}
impl Wake for FrameWaker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.shared.woken.lock().expect("woken frames").push(self.frame);
        self.shared.pending.store(true, Ordering::Release);
        self.shared.notify.notify_one();
    }
}

#[derive(Debug, Default)]
pub struct Io {
    /// The reactor and the local tasks of providers (`spawn_local`: process
    /// reapers and capture pipes); both progress while the scheduler waits.
    #[cfg(feature = "host")]
    tokio: Option<(tokio::runtime::Runtime, tokio::task::LocalSet)>,
    shared: Arc<Shared>,
    /// Native request of each waiting driver frame (frames are roots).
    pub requests: HashMap<Word, u64>,
    /// The waiting frames of `requests` that only a provider thread wakes
    /// (`Request::off_reactor`); a subset of `requests`.
    pub off_reactor: HashSet<Word>,
    /// When the scheduler last turned the reactor while waiting.
    #[cfg(feature = "host")]
    last_turn: Option<Instant>,
    /// The buffer the last poll emptied, kept for the next one (#442), so a
    /// busy server does not allocate a vector of woken frames per poll.
    spare: Vec<Word>,
}

#[cfg(feature = "host")]
impl Io {
    fn reactor(&mut self) -> &(tokio::runtime::Runtime, tokio::task::LocalSet) {
        self.tokio.get_or_insert_with(|| {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("Tokio runtime");
            (runtime, tokio::task::LocalSet::new())
        })
    }

    pub fn tokio(&mut self) -> &tokio::runtime::Runtime {
        &self.reactor().0
    }

    /// The set `spawn_local` of providers puts its tasks into (see `enter`).
    pub fn local(&mut self) -> &tokio::task::LocalSet {
        &self.reactor().1
    }

    /// Turn the reactor and run local tasks until `future` completes.
    pub fn block_on<F: std::future::Future>(&mut self, future: F) -> F::Output {
        let (runtime, local) = self.reactor();
        runtime.block_on(local.run_until(future))
    }

    /// Whether a spin starting at `now` turns the reactor: some waiting
    /// request needs it, or it has not turned for REACTOR_AT_LEAST.
    fn spin_turns_reactor(&self, now: Instant) -> bool {
        self.requests.len() > self.off_reactor.len()
            || self.last_turn.is_none_or(|last| now.duration_since(last) >= REACTOR_AT_LEAST)
    }
}

impl Io {
    pub fn waker(&self, frame: Word) -> Waker {
        Waker::from(Arc::new(FrameWaker { frame, shared: self.shared.clone() }))
    }

    /// The frames woken since the last poll, in the order their wakers
    /// fired and each once (#442): a socket can fire its waker several times
    /// before the scheduler looks. The vectors swap, so neither side
    /// allocates on a busy loop.
    fn take_woken(&mut self) -> Vec<Word> {
        let mut woken = std::mem::take(&mut self.spare);
        if self.shared.pending.swap(false, Ordering::Acquire) {
            std::mem::swap(&mut *self.shared.woken.lock().expect("woken frames"), &mut woken);
            dedupe_in_order(&mut woken);
        }
        woken
    }

    /// Keep an emptied buffer for the next poll.
    fn recycle(&mut self, mut buffer: Vec<Word>) {
        buffer.clear();
        self.spare = buffer;
    }
}

impl Runtime {
    /// Wake frames whose wakers fired, without blocking.
    pub fn poll_io(&mut self) {
        let woken = self.io.take_woken();
        for &frame in &woken {
            self.wake(frame);
        }
        self.io.recycle(woken);
    }

    /// Wait until a native request or a timer can make progress, then wake
    /// those frames; false when nothing external is pending (or, on wasm32,
    /// when nothing pending can progress: only requests, none answered).
    pub fn wait_external(&mut self) -> bool {
        let earliest = self.timers.iter().map(|&(deadline, _)| deadline).min();
        if self.io.requests.is_empty() && earliest.is_none() {
            return false;
        }
        #[cfg(feature = "host")]
        self.wait_tokio(earliest);
        #[cfg(all(not(feature = "host"), not(target_family = "wasm")))]
        self.wait_sleeping(earliest);
        #[cfg(target_family = "wasm")]
        if !skipping(self.io.shared.pending.load(Ordering::Acquire), earliest, crate::clock::skip_to) {
            return false;
        }
        self.poll_io();
        let now = Instant::now();
        let mut due = Vec::new();
        self.timers.retain(|&(deadline, frame)| {
            let fired = deadline <= now;
            if fired {
                due.push((deadline, frame));
            }
            !fired
        });
        due.sort_by_key(|&(deadline, _)| deadline);
        for (_, frame) in due {
            self.wake(frame);
        }
        true
    }

    /// The earliest moment a waiting scheduler wakes: the earliest timer, and,
    /// in an interruptible run, `INTERRUPT_LOOK` from now to look at its flag.
    #[cfg(not(target_family = "wasm"))]
    fn wake_by(&self, earliest: Option<Instant>) -> Option<Instant> {
        match self.limits.interrupt {
            Some(_) => {
                let look = Instant::now() + INTERRUPT_LOOK;
                Some(earliest.map_or(look, |deadline| deadline.min(look)))
            }
            None => earliest,
        }
    }

    /// The host's wait: block in Tokio until a waker fires or `earliest`.
    #[cfg(feature = "host")]
    fn wait_tokio(&mut self, earliest: Option<Instant>) {
        // Native replies (SQLite workers) usually arrive within microseconds:
        // spin briefly before paying for a sleep and a wake-up. Sockets make
        // progress only when this thread turns the reactor, so every 64 turns
        // the spin also polls it without blocking. A turn costs a kevent
        // (about 12 microseconds on macOS, #943), so a spin for requests that
        // only provider threads wake turns it only when it has not turned
        // for REACTOR_AT_LEAST.
        if !self.io.requests.is_empty() {
            let spin = Instant::now();
            let turn = self.io.spin_turns_reactor(spin);
            let mut turns = 0u32;
            while !self.io.shared.pending.load(Ordering::Acquire) {
                std::hint::spin_loop();
                turns += 1;
                // Reading the clock costs more than a turn; look every 64.
                if turns.is_multiple_of(64) {
                    if turn {
                        self.io.block_on(tokio::task::yield_now());
                        self.io.last_turn = Some(Instant::now());
                    }
                    if spin.elapsed() >= std::time::Duration::from_micros(SPIN_MICROS) {
                        break;
                    }
                }
            }
        }
        if !self.io.shared.pending.load(Ordering::Acquire) {
            self.count(|_| "scheduler idle wait".into());
            let started = Instant::now();
            let shared = self.io.shared.clone();
            // An interruptible run wakes to look at its interrupt flag.
            let earliest = self.wake_by(earliest);
            self.io.block_on(async move {
                match earliest {
                    Some(deadline) => {
                        tokio::select! {
                            _ = shared.notify.notified() => {}
                            _ = tokio::time::sleep_until(deadline.into()) => {}
                        }
                    }
                    None => shared.notify.notified().await,
                }
            });
            self.io.last_turn = Some(Instant::now());
            if let Some(stats) = &mut self.stats {
                *stats.entry("scheduler idle wait, microseconds".into()).or_default() += started.elapsed().as_micros() as u64;
            }
        }
    }

    /// Natively without the host: sleep until a waker fires or `earliest`.
    #[cfg(all(not(feature = "host"), not(target_family = "wasm")))]
    fn wait_sleeping(&mut self, earliest: Option<Instant>) {
        if self.io.shared.pending.load(Ordering::Acquire) {
            return;
        }
        self.count(|_| "scheduler idle wait".into());
        let started = Instant::now();
        let earliest = self.wake_by(earliest);
        let notify = &self.io.shared.notify;
        let mut held = notify.lock.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        while !self.io.shared.pending.load(Ordering::Acquire) {
            held = match earliest {
                Some(deadline) => {
                    let now = Instant::now();
                    if now >= deadline {
                        break;
                    }
                    notify.condition.wait_timeout(held, deadline - now).unwrap_or_else(std::sync::PoisonError::into_inner).0
                }
                None => notify.condition.wait(held).unwrap_or_else(std::sync::PoisonError::into_inner),
            };
        }
        drop(held);
        if let Some(stats) = &mut self.stats {
            *stats.entry("scheduler idle wait, microseconds".into()).or_default() += started.elapsed().as_micros() as u64;
        }
    }
}

/// The wait of a build that must not block (wasm32): a fired waker needs no
/// wait; otherwise time skips to the earliest timer. With neither nothing can
/// progress (no thread answers a request there): false.
#[cfg_attr(not(target_family = "wasm"), allow(dead_code))]
fn skipping<T>(woken: bool, earliest: Option<T>, skip_to: impl FnOnce(T)) -> bool {
    if woken {
        return true;
    }
    match earliest {
        Some(deadline) => {
            skip_to(deadline);
            true
        }
        None => false,
    }
}

/// Drop repeated frames, keeping the first of each in place. A poll usually
/// sees one to a few frames, so a short list is checked pairwise.
fn dedupe_in_order(frames: &mut Vec<Word>) {
    if frames.len() < 2 {
        return;
    }
    if frames.len() <= 16 {
        let mut kept = 0;
        for index in 0..frames.len() {
            let frame = frames[index];
            if !frames[..kept].contains(&frame) {
                frames[kept] = frame;
                kept += 1;
            }
        }
        frames.truncate(kept);
    } else {
        let mut seen = std::collections::HashSet::with_capacity(frames.len());
        frames.retain(|frame| seen.insert(*frame));
    }
}

#[cfg(test)]
mod woken_tests {
    use super::{Io, dedupe_in_order, skipping};

    #[cfg(feature = "host")]
    #[test]
    fn spin_turns_the_reactor_only_when_needed() {
        use super::REACTOR_AT_LEAST;
        use std::time::{Duration, Instant};
        let now = Instant::now();
        let mut io = Io::default();
        io.requests.insert(1, 10);
        io.off_reactor.insert(1);
        assert!(io.spin_turns_reactor(now), "never turned yet");
        io.last_turn = Some(now);
        assert!(!io.spin_turns_reactor(now + Duration::from_micros(10)), "only a worker can wake frame 1");
        assert!(io.spin_turns_reactor(now + REACTOR_AT_LEAST), "local tasks still progress");
        io.requests.insert(2, 11);
        assert!(io.spin_turns_reactor(now + Duration::from_micros(10)), "frame 2 waits on the reactor");
        io.off_reactor.insert(2);
        assert!(!io.spin_turns_reactor(now + Duration::from_micros(10)));
    }

    #[test]
    fn repeated_wakes_collapse_in_first_order() {
        let mut short = vec![5, 3, 5, 9, 3, 3];
        dedupe_in_order(&mut short);
        assert_eq!(short, vec![5, 3, 9]);
        let mut long: Vec<u64> = (0..40).chain(0..40).chain([7, 99]).collect();
        dedupe_in_order(&mut long);
        let expected: Vec<u64> = (0..40).chain([99]).collect();
        assert_eq!(long, expected);
        let mut one = vec![4];
        dedupe_in_order(&mut one);
        assert_eq!(one, vec![4]);
    }

    /// #1124, the wasm32 wait: a fired waker is progress without a skip; else the timeline skips to the earliest
    /// timer; with neither (requests no thread can answer) the scheduler is told nothing can progress.
    #[test]
    fn a_wait_that_cannot_block_skips_to_the_earliest_timer() {
        let mut skipped = Vec::new();
        assert!(skipping(true, Some(7u64), |deadline| skipped.push(deadline)));
        assert!(skipped.is_empty(), "a woken frame runs first");
        assert!(skipping(false, Some(7u64), |deadline| skipped.push(deadline)));
        assert_eq!(skipped, [7]);
        assert!(!skipping(false, None::<u64>, |deadline| skipped.push(deadline)));
        assert!(skipping(true, None::<u64>, |deadline| skipped.push(deadline)));
        assert_eq!(skipped, [7]);
    }

    /// #1124, the native wait without the host: a waker on another thread ends the sleep before its timer.
    #[cfg(all(not(feature = "host"), not(target_family = "wasm")))]
    #[test]
    fn a_sleeping_wait_ends_when_a_waker_fires() {
        let io = Io::default();
        let waker = io.waker(42);
        let started = std::time::Instant::now();
        let waking = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(20));
            waker.wake();
        });
        let shared = io.shared.clone();
        let mut held = shared.notify.lock.lock().unwrap();
        while !shared.pending.load(std::sync::atomic::Ordering::Acquire) {
            held = shared.notify.condition.wait_timeout(held, std::time::Duration::from_secs(10)).unwrap().0;
        }
        drop(held);
        waking.join().unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "woken, not timed out");
        let mut io = io;
        assert_eq!(io.take_woken(), [42]);
    }
}
