//! G1 step 2 (#810) on Champ, in process: the timeouts `Within` and `Before`. The driver reads the clock and the limit
//! through the program's helpers (`<entry>.helper<i>`, checked by the loader rule `HOLDER_HELPER`), called through
//! the driver call bridge, which keeps the timer Future's duty in the driver's inputs (memo §6.4, I-G5). The origin
//! that loses is cancelled and its cleanup awaited before `TimedOut`; the timer that loses is cancelled too.
//!
//! Each program runs under Structural and Full verification, with the output of Tomato built from the rebuild tree
//! (the reference since #794). Only pure language cases: no processes, workers, network, outside files or test mode.
#![cfg(feature = "champ-abi")]

mod champ_support;

use champ_support::{compile, run, run_source_with};
use okkama_compiler::oir2::VerifyMode;
use okkama_compiler::SourceFile;
use okkama_sparrow::spb17 as s;

fn expect(source: &str, stdout: &str) {
    for mode in [VerifyMode::Structural, VerifyMode::Full] {
        let run = run_source_with(source, mode).unwrap_or_else(|error| panic!("{mode:?}: {error}"));
        assert_eq!((run.stdout.as_str(), run.result), (stdout, Ok(0)), "{mode:?}");
    }
}

const PRE: &str = r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "async/timeout" take Within, Before, TimedOut;
import "async/event" take Event;
import "time/clock" take SystemClock;
import "time/duration" take Duration;
object Res { has name is string; Res(:name); label() is string { name; } destroy { Stdout().writeln("destroy " + name); } }
object Maker {
    Maker();
    async make(name is string) is Res { Stdout().writeln("make " + name); Res(name: name); }
    async held(name is string, gate is Event) is int {
        let r = Res(name: name);
        Stdout().writeln("holding " + name);
        await gate.wait();
        Stdout().writeln("released " + name);
        1;
    }
}
"#;

fn program(main: &str) -> String {
    format!("{PRE}{main}")
}

/// Time wins: the origin is cancelled, and its cleanup (the resource its body holds) is awaited before `TimedOut`.
#[test]
fn time_wins_and_the_origin_is_cancelled_first() {
    expect(&program(r#"async main() is int {
    let gate = Event();
    let w is Within<int> = Within(origin: Maker().held("origin", gate), limit: Duration(milliseconds: 20), clock: SystemClock());
    let text = try { (await w.outcome()).string(); } catch e is TimedOut { "timed out"; };
    Stdout().writeln(text);
    0;
}
"#), "holding origin\ndestroy origin\ntimed out\n");
}

/// The origin wins: the outcome is a view of the origin's cache, which its owner (`main`, through the Within)
/// settles.
#[test]
fn the_origin_wins_and_its_outcome_stays_with_its_owner() {
    expect(&program(r#"async main() is int {
    let w is Within<Res> = Within(origin: Maker().make("won"), limit: Duration(milliseconds: 50), clock: SystemClock());
    let r = await w.outcome();
    Stdout().writeln("got " + r.label());
    Stdout().writeln("end");
    0;
}
"#), "make won\ngot won\nend\ndestroy won\n");
}

/// A deadline already passed: the timer has won without waiting, and the origin is cancelled and cleaned up.
#[test]
fn a_deadline_in_the_past_has_already_won() {
    expect(&program(r#"async main() is int {
    let clock = SystemClock();
    let gate = Event();
    let past = clock.monotonic_instant().minus(Duration(milliseconds: 5));
    let b is Before<int> = Before(origin: Maker().held("late", gate), deadline: past, clock: clock);
    let text = try { (await b.outcome()).string(); } catch e is TimedOut { "timed out"; };
    Stdout().writeln("past: " + text);
    0;
}
"#), "holding late\ndestroy late\npast: timed out\n");
}

/// Cancelling the outcome before it starts: Cancelled, and the origin never runs.
#[test]
fn cancelling_the_outcome_cancels_the_origin() {
    expect(&program(r#"async main() is int {
    let gate = Event();
    let w is Within<int> = Within(origin: Maker().held("cancelled", gate), limit: Duration(seconds: 30), clock: SystemClock());
    let f = w.outcome();
    f.cancel();
    let text = try { (await f).string(); } catch e is Cancelled { "cancelled"; };
    Stdout().writeln(text);
    0;
}
"#), "cancelled\n");
}

/// An origin that awaits a call of its own: the call it awaits runs in the origin's task.
#[test]
fn an_origin_awaits_a_call_of_its_own() {
    expect(&program(r#"object Chain {
    Chain();
    async run() is int {
        let inner = await Maker().make("inner");
        Stdout().writeln("chain got " + inner.label());
        inner.label().length();
    }
}
async main() is int {
    let w is Within<int> = Within(origin: Chain().run(), limit: Duration(seconds: 5), clock: SystemClock());
    Stdout().writeln("length " + (await w.outcome()).string());
    0;
}
"#), "make inner\nchain got inner\ndestroy inner\nlength 5\n");
}

/// A clock whose timer holds a resource and wakes the origin: the origin wins, and the timer (its duty kept in the
/// driver's inputs by the bridge) is cancelled and its resource destroyed once, before the outcome.
#[test]
fn a_losing_timer_is_cancelled_and_released_once() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "async/timeout" take Within, TimedOut;
import "async/event" take Event;
import "time/clock" take SystemClock, Timing, Instant, MonotonicInstant;
import "time/duration" take Duration;
object Res { has name is string; Res(:name); label() is string { name; } destroy { Stdout().writeln("destroy " + name); } }
object FakeClock does Timing {
    has wake is Event;
    has fire is Event;
    FakeClock(:wake, :fire);
    instant() is Instant { SystemClock().instant(); }
    monotonic_instant() is MonotonicInstant { SystemClock().monotonic_instant(); }
    async wait(duration is Duration) is void {
        let r = Res(name: "timer");
        Stdout().writeln("timer waits");
        wake.set();
        await fire.wait();
        Stdout().writeln("timer fired");
    }
}
object Origin {
    Origin();
    async run(gate is Event) is int { await gate.wait(); Stdout().writeln("origin done"); 7; }
}
async main() is int {
    let wake = Event();
    let fire = Event();
    let w is Within<int> = Within(origin: Origin().run(wake), limit: Duration(seconds: 5), clock: FakeClock(wake: wake, fire: fire));
    let text = try { (await w.outcome()).string(); } catch e is TimedOut { "timed out"; };
    Stdout().writeln("got " + text);
    0;
}
"#, "timer waits\norigin done\ndestroy timer\ngot 7\n");
}

/// A clock whose timer fires at once: the origin is cancelled and cleaned up, then `TimedOut`.
#[test]
fn a_winning_timer_cancels_the_origin() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "async/timeout" take Within, TimedOut;
import "async/event" take Event;
import "time/clock" take SystemClock, Timing, Instant, MonotonicInstant;
import "time/duration" take Duration;
object Res { has name is string; Res(:name); label() is string { name; } destroy { Stdout().writeln("destroy " + name); } }
object FakeClock does Timing {
    has wake is Event;
    has fire is Event;
    FakeClock(:wake, :fire);
    instant() is Instant { SystemClock().instant(); }
    monotonic_instant() is MonotonicInstant { SystemClock().monotonic_instant(); }
    async wait(duration is Duration) is void {
        let r = Res(name: "timer");
        Stdout().writeln("timer fires");
    }
}
object Origin {
    Origin();
    async run(gate is Event) is int { let r = Res(name: "origin"); await gate.wait(); Stdout().writeln("origin done"); 7; }
}
async main() is int {
    let wake = Event();
    let fire = Event();
    let w is Within<int> = Within(origin: Origin().run(wake), limit: Duration(seconds: 5), clock: FakeClock(wake: wake, fire: fire));
    let text = try { (await w.outcome()).string(); } catch e is TimedOut { "timed out"; };
    Stdout().writeln("got " + text);
    0;
}
"#, "timer fires\ndestroy timer\ndestroy origin\ngot timed out\n");
}

/// `HOLDER_HELPER`: the driver trusts its helpers' shapes. A program whose helper is renamed away, or whose helpers
/// are swapped, is refused at load.
#[test]
fn the_loader_checks_the_timeout_helpers() {
    let name = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/champ_support/inline.ok");
    let source = SourceFile::new(name.to_string_lossy(), program(r#"async main() is int {
    let gate = Event();
    let w is Within<int> = Within(origin: Maker().held("origin", gate), limit: Duration(milliseconds: 20), clock: SystemClock());
    let text = try { (await w.outcome()).string(); } catch e is TimedOut { "timed out"; };
    Stdout().writeln(text);
    0;
}
"#)).unwrap();
    let bytes = compile(&[source]).unwrap_or_else(|error| panic!("{error}"));
    assert!(run(bytes.clone()).is_ok());
    let program = s::decode(&bytes).unwrap();
    let helper = |program: &s::Program, index: usize| -> usize {
        program.functions.iter().position(|function| function.symbol.starts_with("okkama.timeout.Within.outcome$")
            && function.symbol.ends_with(&format!(".helper{index}"))).expect("helper")
    };
    let mut renamed = program.clone();
    let wait = helper(&renamed, 1);
    renamed.functions[wait].symbol.push_str(".gone");
    let refused = run(s::encode(&renamed).unwrap()).err().expect("a missing helper");
    assert!(refused.contains("HOLDER_HELPER"), "{refused}");
    let mut swapped = program.clone();
    let (seconds, wait) = (helper(&swapped, 0), helper(&swapped, 1));
    let symbols = (swapped.functions[seconds].symbol.clone(), swapped.functions[wait].symbol.clone());
    (swapped.functions[seconds].symbol, swapped.functions[wait].symbol) = (symbols.1, symbols.0);
    let refused = run(s::encode(&swapped).unwrap()).err().expect("helpers of other shapes");
    assert!(refused.contains("HOLDER_HELPER"), "{refused}");
}

/// A running timeout whose duty moves (into a group) keeps its inputs with it: the fresh receiver's origin, whose
/// outcome is destroyed once (review T1: a driver's INPUTS take every owner transition).
#[test]
fn a_running_timeout_moved_to_a_group_keeps_its_inputs() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "async/timeout" take Within;
import "async/scope" take FutureGroup;
import "async/event" take Event;
import "time/clock" take SystemClock;
import "time/duration" take Duration;
object Res { has name is string; Res(:name); label() is string { name; } destroy { Stdout().writeln("destroy " + name); } }
object Maker {
    Maker();
    async gated(name is string, gate is Event) is Res { await gate.wait(); Stdout().writeln("make " + name); Res(name: name); }
}
object F {
    F();
    started(g is FutureGroup<Res>, gate is Event) is Future<Res> {
        let f = Within(origin: Maker().gated("x", gate), limit: Duration(seconds: 5), clock: SystemClock()).outcome();
        g.start(f);
        Stdout().writeln("started");
        f;
    }
}
async main() is int {
    let gate = Event();
    let g is FutureGroup<Res> = FutureGroup();
    let f = F().started(g, gate);
    gate.set();
    let r = await f;
    Stdout().writeln("got " + r.label());
    0;
}
"#, "started\nmake x\ngot x\ndestroy x\n");
}

/// A running timeout detached when its owner closes keeps its inputs until the root closer releases them at shutdown
/// (review T1, memo §2.4: a detached driver is retained while its INPUTS are not empty).
#[test]
fn a_detached_timeout_releases_its_inputs_at_shutdown() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "async/timeout" take Within;
import "async/scope" take FutureGroup;
import "async/event" take Event;
import "time/clock" take SystemClock;
import "time/duration" take Duration;
object Res { has name is string; Res(:name); label() is string { name; } destroy { Stdout().writeln("destroy " + name); } }
object Maker {
    Maker();
    async gated(name is string, gate is Event) is Res { await gate.wait(); Stdout().writeln("make " + name); Res(name: name); }
}
object H {
    H();
    async go(g is FutureGroup<Res>, gate is Event) is void {
        let f = Within(origin: Maker().gated("x", gate), limit: Duration(seconds: 5), clock: SystemClock()).outcome();
        g.start(f);
        Stdout().writeln("go end");
    }
}
async main() is int {
    let gate = Event();
    let g is FutureGroup<Res> = FutureGroup();
    await H().go(g, gate);
    gate.set();
    await g.wait();
    Stdout().writeln("main end");
    0;
}
"#, "go end\nmake x\nmain end\ndestroy x\n");
}

/// #840: a timeout on a fresh temporary receiver, awaited at once (`await Within(…).outcome()`). Full refused it with
/// B91; it passes since an earlier fix of the release (found while closing #877). Tomato's output.
#[test]
fn a_timeout_on_a_fresh_temporary_receiver() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "async/timeout" take Within;
import "time/clock" take SystemClock;
import "time/duration" take Duration;
object Res { has name is string; Res(:name); label() is string { name; } destroy { Stdout().writeln("destroy " + name); } }
object Maker { Maker(); async make(name is string) is Res { Stdout().writeln("make " + name); Res(name: name); } }
async main() is int {
    let r = await Within(origin: Maker().make("x"), limit: Duration(seconds: 5), clock: SystemClock()).outcome();
    Stdout().writeln("got " + r.label());
    0;
}
"#, "make x\ngot x\ndestroy x\n");
}

/// #824: a `Before` over a clock role that no object implements, in a method of a used object that is never called
/// (std's `SqliteConnection.backup` in every SQLite program). The timeout driver's helpers call the role's slots;
/// with no implementation the role has no value, so they are unreachable, as an ordinary role call is (#757).
/// Before, the helper's empty dispatch stopped `own` at an unregistered open callable. Structural only: Full still
/// rejects a value of a role without implementations (#885).
#[test]
fn a_timeout_over_a_clock_role_without_implementations_compiles() {
    let source = r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "async/timeout" take Before;
import "time/clock" take Timing;
import "time/duration" take Duration;
object Waiter {
    Waiter();
    label() is string { "waiter"; }
    async slow() is int { 1; }
    async bounded(clock is Timing, limit is Duration) is int {
        let deadline = clock.monotonic_instant().plus(limit);
        let bounded is Before<int> = Before(origin: self.slow(), deadline: deadline, clock: clock);
        await bounded.outcome();
    }
}
async main() is int {
    Stdout().writeln(Waiter().label());
    0;
}
"#;
    let run = run_source_with(source, VerifyMode::Structural).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!((run.stdout.as_str(), run.result), ("waiter\n", Ok(0)));
}
