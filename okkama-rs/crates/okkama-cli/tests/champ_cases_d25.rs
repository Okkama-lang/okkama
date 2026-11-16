//! #771, #758 (OIR-2 async ABI D25.2, revision of 2026-11-16, and the ruling "How a stored Future leaves"): Futures
//! stored in object fields and in arrays on Champ. Conformance cases are checked in full against their case.json;
//! the inline programs' expected output is Tomato's for the same program (okkama 0.5.8, run in the cage).
//! The file runs twice: as is, and with `SPARROW_CORE_GC_STRESS=1`, which collects at every safepoint.
#![cfg(feature = "champ-abi")]

mod champ_support;

use champ_support::{run_case, run_source};

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
    d25_collect_race_empty => "L08.collect.race-empty",
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
