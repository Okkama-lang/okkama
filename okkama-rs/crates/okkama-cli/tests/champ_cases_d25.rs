//! #771, #758 (OIR-2 async ABI D25.2, revision of 2026-11-16, and the ruling "How a stored Future leaves"): Futures
//! stored in object fields and in arrays on Champ. Conformance cases are checked in full against their case.json;
//! the inline programs' expected output is Tomato's for the same program (okkama 0.5.8, run in the cage).
//! The file runs twice: as is, and with `SPARROW_CORE_GC_STRESS=1`, which collects at every safepoint.
#![cfg(feature = "champ-abi")]

mod champ_support;

use champ_support::{run_case, run_source, run_source_with};

macro_rules! cases {
    ($($name:ident => $id:literal,)*) => {
        $(
            #[test]
            fn $name() {
                run_case($id).unwrap_or_else(|error| panic!("{}: {error}", $id));
            }
        )*
    };
}

cases! {
    d25_cancel_after_await_settled => "L08.cancel.after-await-settled",
    d25_cancel_late_after_last_await => "L08.cancel.late-after-last-await",
    d25_cancel_shared_awaited_survives => "L08.cancel.shared-awaited-survives",
    d25_cancel_nested_frame_next_await => "L08.cancel.nested-frame-next-await",
    d25_cancel_final_exit_window => "L08.cancel.final-exit-window",
}

/// A race of no Futures fails with NoFutures, as on Tomato. `Raced` is a D23 holder of slice G1: the producer refuses
/// its members by their catalogue row until the slice brings the runtime (#810).
#[test]
#[ignore = "#810: Raced.winner is a G1 holder member, refused until the slice"]
fn d25_collect_race_empty() {
    run_case("L08.collect.race-empty").unwrap_or_else(|error| panic!("L08.collect.race-empty: {error}"));
}

fn expect(source: &str, stdout: &str, status: i64) {
    let run = run_source(source).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!((run.stdout.as_str(), run.result), (stdout, Ok(status)));
}

/// A cold Future stored in a field is not cancelled when the scope that created it closes: it leaves `make` with
/// its holder, runs when awaited through the holder, and a holder never awaited settles it, cold, when its owner
/// closes (D24.2): that child never runs.
#[test]
fn a_cold_future_in_a_field_lives_with_its_holder() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Child {
    has n is int;
    Child(:n);
    async run() is int { Stdout().writeln("child runs"); n; }
}
object Holder {
    has f is Future<int>;
    Holder(:f);
    async get() is int { await f; }
}
object Maker {
    Maker();
    make(n is int) is Holder {
        let c = Child(n: n).run();
        Holder(f: c);
    }
}
async main() is int {
    let h = Maker().make(7);
    Stdout().writeln("made");
    Stdout().writeln((await h.get()).string());
    let k = Maker().make(8);
    Stdout().writeln("never awaited");
    0;
}
"#, "made\nchild runs\n7\nnever awaited\n", 0);
}

/// A spawned Future stored in a field moves its task duty and its cache duty together: both children run, and
/// the holder that is never awaited still sees its Future done.
#[test]
fn a_spawned_future_in_a_field_moves_task_and_cache_together() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "async/event" take Event;
object Child {
    has go is Event;
    has n is int;
    Child(:go, :n);
    async run() is int { await go.wait(); Stdout().writeln("child " + n.string()); n; }
}
object Holder {
    has f is Future<int>;
    Holder(:f);
    async get() is int { await f; }
    finished() is bool { f.done(); }
}
object Maker {
    Maker();
    make(go is Event, n is int) is Holder {
        Holder(f: spawn Child(go: go, n: n).run());
    }
}
async main() is int {
    let go = Event();
    let h = Maker().make(go, 5);
    let lost = Maker().make(go, 6);
    Stdout().writeln("made");
    go.set();
    Stdout().writeln((await h.get()).string());
    Stdout().writeln("done " + lost.finished().string());
    0;
}
"#, "made\nchild 5\nchild 6\n5\ndone true\n", 0);
}

/// #758: an array of cold Futures in a field; the awaited element runs, the others settle with their owner.
#[test]
fn cold_futures_in_an_array_field() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Child {
    has n is int;
    Child(:n);
    async run() is int { Stdout().writeln("child " + n.string()); n; }
}
object Batch {
    has fs is @Future<int>;
    Batch(:fs);
    async first() is int { await fs[0]; }
}
object Maker {
    Maker();
    make(n is int) is Batch {
        let a = Child(n: n).run();
        let b = Child(n: n + 1).run();
        Batch(fs: [a, b]);
    }
}
async main() is int {
    let h = Maker().make(1);
    Stdout().writeln("made");
    Stdout().writeln((await h.first()).string());
    let k = Maker().make(10);
    Stdout().writeln("never awaited");
    0;
}
"#, "made\nchild 1\n1\nnever awaited\n", 0);
}

/// #758: an array of spawned Futures in a field.
#[test]
fn spawned_futures_in_an_array_field() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "async/event" take Event;
object Child {
    has go is Event;
    has n is int;
    Child(:go, :n);
    async run() is int { await go.wait(); Stdout().writeln("child " + n.string()); n; }
}
object Batch {
    has fs is @Future<int>;
    Batch(:fs);
    async first() is int { await fs[0]; }
    finished() is bool { fs[1].done(); }
}
object Maker {
    Maker();
    make(go is Event, n is int) is Batch {
        let a = spawn Child(go: go, n: n).run();
        let b = spawn Child(go: go, n: n + 1).run();
        Batch(fs: [a, b]);
    }
}
async main() is int {
    let go = Event();
    let h = Maker().make(go, 5);
    Stdout().writeln("made");
    go.set();
    Stdout().writeln((await h.first()).string());
    Stdout().writeln("done " + h.finished().string());
    0;
}
"#, "made\nchild 5\nchild 6\n5\ndone true\n", 0);
}

/// A duty is never settled twice, from a schedule and from a packet: the outcome of a stored Future is destroyed
/// exactly once. `await f` in `get` reads the cached outcome without taking it (a Future keeps its outcome), so the
/// outcome lives with the Future, whose duty travels with the holder `h` to `main`, and it is destroyed when `main`
/// closes (ABI D24.2, Bible §11.4). Ruling on #794 (2026-11-16): the rebuild tree's Tomato and Champ agree on this;
/// the 0.5.8 release destroyed it when `get` ended.
#[test]
fn a_stored_done_future_releases_its_outcome_once() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Res {
    has n is int;
    Res(:n);
    value() is int { n; }
    destroy { Stdout().writeln("destroy " + n.string()); }
}
object Child {
    has n is int;
    Child(:n);
    async run() is Res { Res(n: n); }
}
object Holder {
    has f is Future<Res>;
    Holder(:f);
    async get() is int { (await f).value(); }
}
object Maker {
    Maker();
    make(n is int) is Holder { Holder(f: Child(n: n).run()); }
}
async main() is int {
    let h = Maker().make(3);
    Stdout().writeln((await h.get()).string());
    Stdout().writeln("end");
    0;
}
"#, "3\nend\ndestroy 3\n", 0);
}

/// #783: a bound Future stored on some paths only (PR-B1: each store is an alias, the duty stays with `make`'s
/// binding and leaves by `cache.rehome` with the returned Holder). Tomato (okkama 0.5.8): `child 7`, `7`.
#[test]
fn a_bound_future_stored_on_some_paths_only() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Child {
    has n is int;
    Child(:n);
    async run() is int { Stdout().writeln("child " + n.string()); n; }
}
object Holder {
    has f is Future<int>;
    Holder(:f);
    first() is Future<int> { f; }
}
object Maker {
    Maker();
    make(n is int) is Holder {
        let c = Child(n: n).run();
        if n > 0 { let first = Holder(f: c); }
        Holder(f: c);
    }
}
async main() is int {
    let h = Maker().make(7);
    let g = h.first();
    Stdout().writeln((await g).string());
    0;
}
"#, "child 7\n7\n", 0);
}

/// #783: a fresh Future that reaches a store on one branch only: it leaves the branch scope by `cache.rehome`, so
/// the store after the join takes an admitted entry, never a token. Tomato (okkama 0.5.8): `child 1`, `1`,
/// `child 2`, `2`.
#[test]
fn a_fresh_future_from_one_branch_is_stored_after_the_join() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Child {
    has n is int;
    Child(:n);
    async run() is int { Stdout().writeln("child " + n.string()); n; }
}
object Holder {
    has f is Future<int>;
    Holder(:f);
    first() is Future<int> { f; }
}
async main() is int {
    let g = Child(n: 2).run();
    let flag = true;
    let h = Holder(f: if flag { Child(n: 1).run(); } else { g; });
    let r = h.first();
    Stdout().writeln((await r).string());
    Stdout().writeln((await g).string());
    0;
}
"#, "child 1\n1\nchild 2\n2\n", 0);
}

/// #801 (S1): a fresh Future stored in a Holder and returned through its getter from the scope that stored it.
/// The getter's result is a borrowed alias: no rehome takes it, and `pick`'s exit names it, which takes its entry
/// out of the Holder. Tomato (okkama 0.5.8): `picked`, `child 1`, `1`.
#[test]
fn a_stored_future_returned_through_a_getter() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Child {
    has n is int;
    Child(:n);
    async run() is int { Stdout().writeln("child " + n.string()); n; }
}
object Holder {
    has f is Future<int>;
    Holder(:f);
    first() is Future<int> { f; }
}
object Picker {
    Picker();
    pick() is Future<int> {
        let h = Holder(f: Child(n: 1).run());
        h.first();
    }
}
async main() is int {
    let f = Picker().pick();
    Stdout().writeln("picked");
    Stdout().writeln((await f).string());
    0;
}
"#, "picked\nchild 1\n1\n", 0);
}

/// #801 (S3): a relay of a getter returns a borrowed alias of the Box's Future. Tomato (okkama 0.5.8): `child 5`, `5`.
#[test]
fn a_relay_of_a_future_getter() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Child {
    has n is int;
    Child(:n);
    async run() is int { Stdout().writeln("child " + n.string()); n; }
}
object Box {
    has f is Future<int>;
    Box(:f);
    first() is Future<int> { f; }
}
object Relay {
    Relay();
    relay(b is Box) is Future<int> { b.first(); }
}
async main() is int {
    let b = Box(f: Child(n: 5).run());
    let g = Relay().relay(b);
    Stdout().writeln((await g).string());
    0;
}
"#, "child 5\n5\n", 0);
}

/// #801 (wave/18 review): a synchronous relay of a freshly created Future (`id(Seed(n: 1).run())`) at the top of the
/// body, in each branch of an `if` whose value leaves its block, and returned from a synchronous action. The relay's
/// result is a borrowed alias, so the duty of the Future it was given leaves with it. Under the Full verifier, with
/// Tomato's output (okkama 0.5.8): the Futures run when awaited, in order.
#[test]
fn a_relay_of_a_fresh_future_under_the_full_verifier() {
    let run = run_source_with(r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Seed {
    has n is int;
    Seed(:n);
    async run() is int { Stdout().writeln("seed " + n.string()); n; }
}
object Relay {
    Relay();
    id(f is Future<int>) is Future<int> { f; }
    made(n is int) is Future<int> { self.id(Seed(n: n).run()); }
}
async main() is int {
    let relay = Relay();
    let f = relay.id(Seed(n: 1).run());
    let g = if f.done() { relay.id(Seed(n: 0).run()); } else { relay.id(Seed(n: 2).run()); };
    let h = relay.made(3);
    Stdout().writeln("relayed");
    Stdout().writeln(((await f) + (await g) + (await h)).string());
    0;
}
"#, okkama_compiler::oir2::VerifyMode::Full).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!((run.stdout.as_str(), run.result), ("relayed\nseed 1\nseed 2\nseed 3\n6\n", Ok(0)));
}
