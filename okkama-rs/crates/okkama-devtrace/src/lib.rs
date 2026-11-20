//! Development instrumentation (owner, 2026-11-20): events, counters, timers and dumps at every step of the
//! compiler and the runtimes, to see who decided what, how many times, and what it cost.
//!
//! It is working code, not released code. A release build passes `--cfg okkama_release`; then [`Gate::on`] is a
//! constant `false` and every macro body below is removed by the optimizer at its call site. Every other build
//! (development, tests, CI) has it compiled in, and off at run time until `OKKAMA_DEV` names a category.
//!
//! ```text
//! OKKAMA_DEV=all                       every category
//! OKKAMA_DEV=lower,runtime              these categories
//! OKKAMA_DEV=lower:main,dump           lower, only for functions whose name contains "main"; plus dumps
//! OKKAMA_DEV_OUT=DIR                   output directory (default: $TMPDIR/okkama-dev/<pid>)
//! ```
//!
//! Output, written by [`finish`] (the CLI calls it before it exits) and as it goes:
//! - `events.jsonl`: one JSON object per event: `{"t_us":…,"cat":…,"name":…, fields…}`;
//! - `dumps/NNNN-<stage>/<function>.txt`: the text a [`dump!`] produced, numbered in emission order;
//! - `summary.txt` (also printed on stderr) and `counters.json`: counters and timers (count, total, max).
//!
//! Categories are free strings; the ones in use are listed by `OKKAMA_DEV=help` (see [`CATEGORIES`]).
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The categories the code base uses, for `OKKAMA_DEV=help`. A new category is added here.
pub const CATEGORIES: &[(&str, &str)] = &[
    ("lower", "lowering and IR passes: sizes, region operations by kind, per-function IR dumps"),
    ("runtime", "runtime events: identities, region helpers, resource walks, structure indexes, allocation"),
    ("jit", "JIT compilation"),
    ("gc", "collections"),
];

#[derive(Debug)]
struct Config {
    all: bool,
    /// Category → function filters (empty: every function).
    categories: BTreeMap<String, Vec<String>>,
    dumps: bool,
    out: PathBuf,
}

// A release build reads no configuration.
#[cfg_attr(okkama_release, allow(dead_code))]
impl Config {
    fn parse(spec: &str, out: Option<PathBuf>) -> Self {
        let mut config = Config { all: false, categories: BTreeMap::new(), dumps: false,
            out: out.unwrap_or_else(|| std::env::temp_dir().join("okkama-dev").join(std::process::id().to_string())) };
        for item in spec.split(',').map(str::trim).filter(|item| !item.is_empty()) {
            let (name, filter) = item.split_once(':').map_or((item, None), |(name, filter)| (name, Some(filter)));
            match name {
                "all" => config.all = true,
                "dump" | "dumps" => config.dumps = true,
                "help" => {
                    eprintln!("OKKAMA_DEV categories (comma separated, `cat:filter` limits to functions):");
                    for (name, what) in CATEGORIES {
                        eprintln!("  {name:10} {what}");
                    }
                    eprintln!("  all        every category\n  dump       also write dumps");
                }
                _ => {
                    let filters = config.categories.entry(name.to_string()).or_default();
                    if let Some(filter) = filter {
                        filters.push(filter.to_string());
                    }
                }
            }
        }
        config
    }

    fn on(&self, category: &str) -> bool {
        self.all || self.categories.contains_key(category)
    }

    fn matches(&self, category: &str, function: &str) -> bool {
        match self.categories.get(category) {
            Some(filters) => filters.is_empty() || filters.iter().any(|filter| function.contains(filter.as_str())),
            None => self.all,
        }
    }
}

#[derive(Debug, Default)]
struct Timer {
    count: u64,
    total: Duration,
    max: Duration,
}

#[derive(Debug, Default)]
struct State {
    counters: BTreeMap<(&'static str, String), u64>,
    timers: BTreeMap<(&'static str, String), Timer>,
    events: Option<std::io::BufWriter<std::fs::File>>,
    events_written: u64,
    dumps: u64,
    finished: bool,
}

// A release build reads no configuration.
#[cfg_attr(okkama_release, allow(dead_code))]
static CONFIG: OnceLock<Option<Config>> = OnceLock::new();
static STATE: Mutex<Option<State>> = Mutex::new(None);
static START: OnceLock<Instant> = OnceLock::new();
static EVENTS: AtomicU64 = AtomicU64::new(0);

fn config() -> Option<&'static Config> {
    #[cfg(okkama_release)]
    {
        None
    }
    #[cfg(not(okkama_release))]
    {
        CONFIG.get_or_init(|| {
            let spec = std::env::var("OKKAMA_DEV").ok().filter(|spec| !spec.trim().is_empty() && spec != "0")?;
            START.get_or_init(Instant::now);
            Some(Config::parse(&spec, std::env::var_os("OKKAMA_DEV_OUT").map(PathBuf::from)))
        }).as_ref()
    }
}

/// A category's switch at one call site: after the first look, one relaxed load.
#[derive(Debug)]
pub struct Gate {
    category: &'static str,
    /// 0 unknown, 1 off, 2 on.
    state: AtomicU8,
}

impl Gate {
    pub const fn new(category: &'static str) -> Self {
        Self { category, state: AtomicU8::new(0) }
    }

    pub fn category(&self) -> &'static str {
        self.category
    }

    /// Whether the category is on. A constant `false` in a release build.
    #[inline(always)]
    pub fn on(&self) -> bool {
        if cfg!(okkama_release) {
            return false;
        }
        match self.state.load(Ordering::Relaxed) {
            1 => false,
            2 => true,
            _ => self.decide(),
        }
    }

    #[cold]
    fn decide(&self) -> bool {
        let on = config().is_some_and(|config| config.on(self.category));
        self.state.store(if on { 2 } else { 1 }, Ordering::Relaxed);
        on
    }

    /// Whether the category is on for this function (its `cat:filter` entries).
    pub fn on_for(&self, function: &str) -> bool {
        self.on() && config().is_some_and(|config| config.matches(self.category, function))
    }

    /// Whether dumps of this category are written for this function.
    pub fn dumps_for(&self, function: &str) -> bool {
        self.on_for(function) && config().is_some_and(|config| config.dumps)
    }
}

/// Whether any instrumentation is on (`OKKAMA_DEV` set, not a release build).
pub fn active() -> bool {
    config().is_some()
}

fn with_state<T>(f: impl FnOnce(&mut State, &Config) -> T) -> Option<T> {
    let config = config()?;
    let mut guard = STATE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let state = guard.get_or_insert_with(State::default);
    Some(f(state, config))
}

/// Adds `by` to a counter.
pub fn count(category: &'static str, name: &str, by: u64) {
    with_state(|state, _| *state.counters.entry((category, name.to_string())).or_default() += by);
}

/// Adds one measured interval to a timer.
pub fn time(category: &'static str, name: &str, elapsed: Duration) {
    with_state(|state, _| {
        let timer = state.timers.entry((category, name.to_string())).or_default();
        timer.count += 1;
        timer.total += elapsed;
        timer.max = timer.max.max(elapsed);
    });
}

/// Measures until dropped ([`span!`]).
#[derive(Debug)]
pub struct Span {
    category: &'static str,
    name: String,
    start: Instant,
}

impl Span {
    pub fn new(category: &'static str, name: impl Into<String>) -> Self {
        Self { category, name: name.into(), start: Instant::now() }
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        time(self.category, &self.name, self.start.elapsed());
    }
}

/// JSON text of a string.
pub fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Writes one event: `fields` are (name, value as text).
pub fn event(category: &'static str, name: &str, fields: &[(&str, String)]) {
    let t = START.get().map_or(0, |start| start.elapsed().as_micros());
    with_state(|state, config| {
        if state.events.is_none() && !state.finished {
            let _ = std::fs::create_dir_all(&config.out);
            state.events = std::fs::File::create(config.out.join("events.jsonl")).ok().map(std::io::BufWriter::new);
        }
        let mut line = format!("{{\"t_us\":{t},\"cat\":{},\"name\":{}", json_string(category), json_string(name));
        for (key, value) in fields {
            let _ = write!(line, ",{}:{}", json_string(key), json_string(value));
        }
        line.push_str("}\n");
        if let Some(events) = state.events.as_mut() {
            let _ = events.write_all(line.as_bytes());
        }
        state.events_written += 1;
    });
    EVENTS.fetch_add(1, Ordering::Relaxed);
}

/// Writes a dump file `dumps/NNNN-<stage>/<function>.txt`.
pub fn dump(stage: &str, function: &str, text: &str) {
    with_state(|state, config| {
        state.dumps += 1;
        let directory = config.out.join("dumps").join(format!("{:04}-{}", state.dumps, file_name(stage)));
        let _ = std::fs::create_dir_all(&directory);
        let _ = std::fs::write(directory.join(format!("{}.txt", file_name(function))), text);
    });
}

fn file_name(text: &str) -> String {
    let name: String = text.chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '#') { c } else { '_' })
        .collect();
    if name.len() > 120 { format!("{}…{:x}", &name[..100], fxhash(text)) } else { name }
}

fn fxhash(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325, |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3))
}

/// The summary text: counters and timers by category.
pub fn summary() -> Option<String> {
    with_state(|state, config| {
        let mut out = format!("okkama-dev: {} events, {} dumps in {}\n", state.events_written, state.dumps,
            config.out.display());
        let mut category = "";
        for ((cat, name), value) in &state.counters {
            if *cat != category {
                category = cat;
                let _ = writeln!(out, "[{cat}] counters");
            }
            let _ = writeln!(out, "  {name:56} {value:>12}");
        }
        category = "";
        for ((cat, name), timer) in &state.timers {
            if *cat != category {
                category = cat;
                let _ = writeln!(out, "[{cat}] timers                                              count     total ms       max ms");
            }
            let _ = writeln!(out, "  {name:56} {:>8} {:>12.3} {:>12.3}", timer.count,
                timer.total.as_secs_f64() * 1000.0, timer.max.as_secs_f64() * 1000.0);
        }
        out
    })
}

/// Flushes the events and writes `summary.txt` and `counters.json`; prints the summary on stderr. Called once
/// before the process exits (the CLI); later events still go to the counters but no longer to files.
pub fn finish() {
    let Some(text) = summary() else { return };
    with_state(|state, config| {
        if let Some(mut events) = state.events.take() {
            let _ = events.flush();
        }
        state.finished = true;
        let _ = std::fs::create_dir_all(&config.out);
        let _ = std::fs::write(config.out.join("summary.txt"), &text);
        let mut json = String::from("{\"counters\":{");
        for (index, ((cat, name), value)) in state.counters.iter().enumerate() {
            let _ = write!(json, "{}{}:{value}", if index > 0 { "," } else { "" }, json_string(&format!("{cat}.{name}")));
        }
        json.push_str("},\"timers_ms\":{");
        for (index, ((cat, name), timer)) in state.timers.iter().enumerate() {
            let _ = write!(json, "{}{}:{{\"count\":{},\"total\":{:.3},\"max\":{:.3}}}", if index > 0 { "," } else { "" },
                json_string(&format!("{cat}.{name}")), timer.count, timer.total.as_secs_f64() * 1000.0,
                timer.max.as_secs_f64() * 1000.0);
        }
        json.push_str("}}\n");
        let _ = std::fs::write(config.out.join("counters.json"), json);
    });
    eprint!("{text}");
}

/// Adds to a counter when the category is on: `count!("own", "splits")`, `count!("own", "work", n)`.
#[macro_export]
macro_rules! count {
    ($cat:literal, $name:expr) => { $crate::count!($cat, $name, 1) };
    ($cat:literal, $name:expr, $by:expr) => {{
        static GATE: $crate::Gate = $crate::Gate::new($cat);
        if GATE.on() {
            $crate::count($cat, &$name, ($by) as u64);
        }
    }};
}

/// A timer guard when the category is on: `let _span = span!("stages", "own");`.
#[macro_export]
macro_rules! span {
    ($cat:literal, $name:expr) => {{
        static GATE: $crate::Gate = $crate::Gate::new($cat);
        GATE.on().then(|| $crate::Span::new($cat, $name))
    }};
}

/// One event when the category is on (and, with `fn = …` first, its filter matches the function):
/// `event!("own", "dynamic owner", fn = name, scope = id, reason = "offered bind")`. Values print with `Display`
/// (write `format!("{:?}", x)` for `Debug`).
#[macro_export]
macro_rules! event {
    ($cat:literal, $name:expr, fn = $function:expr $(, $key:ident = $value:expr)* $(,)?) => {{
        static GATE: $crate::Gate = $crate::Gate::new($cat);
        if GATE.on() && GATE.on_for(&$function) {
            $crate::event($cat, &$name, &[("fn", ($function).to_string()) $(, (stringify!($key), ($value).to_string()))*]);
        }
    }};
    ($cat:literal, $name:expr $(, $key:ident = $value:expr)* $(,)?) => {{
        static GATE: $crate::Gate = $crate::Gate::new($cat);
        if GATE.on() {
            $crate::event($cat, &$name, &[$((stringify!($key), ($value).to_string())),*]);
        }
    }};
}

/// A dump when the category is on with `dump`, for a matching function; the text is built only then:
/// `dump!("own", "after own", name, || print(&function))`.
#[macro_export]
macro_rules! dump {
    ($cat:literal, $stage:expr, $function:expr, $text:expr) => {{
        static GATE: $crate::Gate = $crate::Gate::new($cat);
        if GATE.on() && GATE.dumps_for(&$function) {
            let text: String = ($text)();
            $crate::dump(&$stage, &$function, &text);
        }
    }};
}

#[cfg(test)]
mod tests;
