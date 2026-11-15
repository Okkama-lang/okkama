//! Opt-in exclusive charged-work counters. Never used by production verification.
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::Write;

#[derive(Default)]
struct Counters {
    stack: Vec<&'static str>,
    totals: BTreeMap<&'static str, (usize, usize)>,
}
thread_local! {
    static COUNTERS: RefCell<Vec<Counters>> = const { RefCell::new(Vec::new()) };
}
pub(super) struct Operation;
impl Operation {
    pub fn new(name: &'static str) -> Self {
        COUNTERS.with_borrow_mut(|counters| {
            if let Some(counters) = counters.last_mut() {
                counters.totals.entry(name).or_default().0 += 1;
                counters.stack.push(name);
            }
        });
        Self
    }
}
impl Drop for Operation {
    fn drop(&mut self) {
        COUNTERS.with_borrow_mut(|counters| {
            if let Some(counters) = counters.last_mut() {
                counters.stack.pop();
            }
        });
    }
}
pub(super) fn charge(work: usize) {
    COUNTERS.with_borrow_mut(|counters| {
        if let Some(counters) = counters.last_mut() {
            let name = counters.stack.last().copied().unwrap_or("solver");
            counters.totals.entry(name).or_default().1 += work;
        }
    });
}
pub(super) fn begin() {
    if std::env::var_os("OKKAMA_634_WORK").is_some() {
        COUNTERS.with_borrow_mut(|counters| counters.push(Counters::default()));
    }
}
pub(super) fn finish(profile: crate::Profile, symbol: &str) {
    let counters = COUNTERS.with_borrow_mut(|active| {
        let counters = active.pop()?;
        if let Some(parent) = active.last_mut() {
            let work = counters.totals.values().map(|(_, work)| work).sum::<usize>();
            let nested = parent.totals.entry("nested_solver").or_default();
            nested.0 += 1;
            nested.1 += work;
        }
        Some(counters)
    });
    let Some(counters) = counters else {
        return;
    };
    let path = std::env::var_os("OKKAMA_634_WORK").unwrap();
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
    for (name, (calls, work)) in counters.totals {
        writeln!(file, "{profile:?}\t{symbol}\t{name}\t{calls}\t{work}").unwrap();
    }
}
