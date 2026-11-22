//! External waits: native I/O requests (Rust futures of the providers) and
//! timers. A driver polls its request with a waker that names its frame; the
//! waker may fire on any thread (SQLite workers reply through channels). When
//! no frame is ready the scheduler blocks in Tokio until a waker fires or the
//! earliest timer is due, then wakes those frames.
use crate::runtime::Runtime;
use crate::word::Word;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Wake, Waker};

/// How long the scheduler spins for a pending native reply before sleeping.
const SPIN_MICROS: u64 = 50;
/// The longest a spinning scheduler goes without turning the reactor when
/// every waiting request is answered by a provider thread (#943): local
/// tasks (process reapers, capture pipes) still progress.
const REACTOR_AT_LEAST: std::time::Duration = std::time::Duration::from_millis(1);
/// How often a waiting scheduler looks at the interrupt flag.
const INTERRUPT_LOOK: std::time::Duration = std::time::Duration::from_millis(20);

#[derive(Debug, Default)]
struct Shared {
    woken: Mutex<Vec<Word>>,
    pending: AtomicBool,
    notify: tokio::sync::Notify,
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
    tokio: Option<(tokio::runtime::Runtime, tokio::task::LocalSet)>,
    shared: Arc<Shared>,
    /// Native request of each waiting driver frame (frames are roots).
    pub requests: HashMap<Word, u64>,
    /// The waiting frames of `requests` that only a provider thread wakes
    /// (`Request::off_reactor`); a subset of `requests`.
    pub off_reactor: HashSet<Word>,
    /// When the scheduler last turned the reactor while waiting.
    last_turn: Option<std::time::Instant>,
    /// The buffer the last poll emptied, kept for the next one (#442), so a
    /// busy server does not allocate a vector of woken frames per poll.
    spare: Vec<Word>,
}

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

    /// Whether a spin starting at `now` turns the reactor: some waiting
    /// request needs it, or it has not turned for REACTOR_AT_LEAST.
    fn spin_turns_reactor(&self, now: std::time::Instant) -> bool {
        self.requests.len() > self.off_reactor.len()
            || self.last_turn.is_none_or(|last| now.duration_since(last) >= REACTOR_AT_LEAST)
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

    /// Block until a native request or a timer can make progress; false when
    /// nothing external is pending.
    pub fn wait_external(&mut self) -> bool {
        let earliest = self.timers.iter().map(|&(deadline, _)| deadline).min();
        if self.io.requests.is_empty() && earliest.is_none() {
            return false;
        }
        // Native replies (SQLite workers) usually arrive within microseconds:
        // spin briefly before paying for a sleep and a wake-up. Sockets make
        // progress only when this thread turns the reactor, so every 64 turns
        // the spin also polls it without blocking. A turn costs a kevent
        // (about 12 microseconds on macOS, #943), so a spin for requests that
        // only provider threads wake turns it only when it has not turned
        // for REACTOR_AT_LEAST.
        if !self.io.requests.is_empty() {
            let spin = std::time::Instant::now();
            let turn = self.io.spin_turns_reactor(spin);
            let mut turns = 0u32;
            while !self.io.shared.pending.load(Ordering::Acquire) {
                std::hint::spin_loop();
                turns += 1;
                // Reading the clock costs more than a turn; look every 64.
                if turns.is_multiple_of(64) {
                    if turn {
                        self.io.block_on(tokio::task::yield_now());
                        self.io.last_turn = Some(std::time::Instant::now());
                    }
                    if spin.elapsed() >= std::time::Duration::from_micros(SPIN_MICROS) {
                        break;
                    }
                }
            }
        }
        if !self.io.shared.pending.load(Ordering::Acquire) {
            self.count(|_| "scheduler idle wait".into());
            let started = std::time::Instant::now();
            let shared = self.io.shared.clone();
            // An interruptible run wakes to look at its interrupt flag.
            let earliest = match self.limits.interrupt {
                Some(_) => {
                    let look = std::time::Instant::now() + INTERRUPT_LOOK;
                    Some(earliest.map_or(look, |deadline| deadline.min(look)))
                }
                None => earliest,
            };
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
            self.io.last_turn = Some(std::time::Instant::now());
            if let Some(stats) = &mut self.stats {
                *stats.entry("scheduler idle wait, microseconds".into()).or_default() += started.elapsed().as_micros() as u64;
            }
        }
        self.poll_io();
        let now = std::time::Instant::now();
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
    use super::{Io, REACTOR_AT_LEAST, dedupe_in_order};
    use std::time::{Duration, Instant};

    #[test]
    fn spin_turns_the_reactor_only_when_needed() {
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
}
