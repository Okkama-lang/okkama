//! PR-A (D25 tail) on Champ, in process: an Atomic over Futures borrows them. Writing a Future into it offers
//! nothing, reading gives a view, and the Future's duty stays with the scope that owns it, which settles it once.
//! Each program runs under Structural and Full verification, with Tomato's output (okkama 0.5.8).
//!
//! Only pure language cases: no processes, workers, network, outside files or test mode.
#![cfg(feature = "champ-abi")]

mod champ_support;

use champ_support::run_source_with;
use okkama_compiler::oir2::VerifyMode;

fn expect(source: &str, stdout: &str) {
    for mode in [VerifyMode::Structural, VerifyMode::Full] {
        let run = run_source_with(source, mode).unwrap_or_else(|error| panic!("{mode:?}: {error}"));
        assert_eq!((run.stdout.as_str(), run.result), (stdout, Ok(0)), "{mode:?}");
    }
}

/// A bound Future stored in the slot, awaited through the view; a second one replaces it and is cancelled through
/// the view while cold, so its body never runs; the first is awaited again through its binding.
#[test]
fn a_future_in_an_atomic_is_a_borrowed_view() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "async/atomic" take Atomic;
object Child {
    has n is int;
    Child(:n);
    async run() is int { Stdout().writeln("child " + n.string()); n; }
}
async main() is int {
    let empty is @Future<int> = [];
    let slot = Atomic(empty);
    let f = Child(n: 1).run();
    slot.replace([f]);
    Stdout().writeln("stored " + slot.value().size().string());
    let got = await slot.value()[0];
    let g = Child(n: 2).run();
    slot.replace([g]);
    slot.value()[0].cancel();
    Stdout().writeln("got " + got.string() + " end " + (await f).string());
    0;
}
"#, "stored 1\nchild 1\ngot 1 end 1\n");
}

/// The displaced Future's outcome, a resource, is destroyed once, when its owner closes; the Future that replaced it
/// is never awaited, so it never runs (I-A4).
#[test]
fn a_displaced_future_is_settled_once_by_its_owner() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "async/atomic" take Atomic;
object Res {
    has n is int;
    Res(:n);
    destroy { Stdout().writeln("destroy " + n.string()); }
    n() is int { n; }
}
object Child {
    has n is int;
    Child(:n);
    async run() is Res { Stdout().writeln("child " + n.string()); Res(n: n); }
}
async main() is int {
    {
        let empty is @Future<Res> = [];
        let slot = Atomic(empty);
        let f = Child(n: 1).run();
        slot.replace([f]);
        let r = await slot.value()[0];
        let g = Child(n: 2).run();
        slot.replace([g]);
        Stdout().writeln("read " + r.n().string() + ", stored " + slot.value().size().string());
    }
    Stdout().writeln("end");
    0;
}
"#, "child 1\nread 1, stored 1\ndestroy 1\nend\n");
}

/// PR-A item 2: a fresh Future written directly is not offered to the Atomic: publication takes nothing, its duty
/// stays with the calling scope, and it can be awaited through the view.
#[test]
fn a_fresh_future_written_into_an_atomic_stays_with_its_scope() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "async/atomic" take Atomic;
object Child {
    has n is int;
    Child(:n);
    async run() is int { Stdout().writeln("child " + n.string()); n; }
}
async main() is int {
    let empty is @Future<int> = [];
    let slot = Atomic(empty);
    slot.replace([Child(n: 3).run()]);
    Stdout().writeln("stored");
    Stdout().writeln("got " + (await slot.value()[0]).string());
    0;
}
"#, "stored\nchild 3\ngot 3\n");
}

/// The std shape (sqlite's `fetches`, tcp's slots): an object keeps an `Atomic<@Future<T>>` field, its actions write
/// and read it, and after the Future's owner has closed another task reads the Atomic and awaits the done Future
/// (I-A3). The object that keeps the Atomic carries no service entries through it, so its scopes need no dynamic
/// settle (`reaches_holder` agrees with `holds_futures`).
#[test]
fn an_object_keeps_futures_in_an_atomic_field() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "async/atomic" take Atomic;
object Child {
    has n is int;
    Child(:n);
    async run() is int { Stdout().writeln("child " + n.string()); n; }
}
// The std shape: an object keeps an optional Future in an Atomic field, written and read by its actions.
object Slot {
    has pending is Atomic<@Future<int>>;
    Slot() {
        let empty is @Future<int> = [];
        pending = Atomic(empty);
    }
    put(work is Future<int>) is void { pending.replace([work]); }
    async first() is int { await pending.value()[0]; }
}
object Reader {
    has slot is Slot;
    Reader(:slot);
    async read() is int { await slot.first(); }
}
async main() is int {
    let slot = Slot();
    {
        let work = Child(n: 4).run();
        slot.put(work);
        Stdout().writeln("put " + (await work).string());
    }
    // The owner of `work` has closed; another task reads the Atomic and awaits the done Future.
    let reader = spawn Reader(slot: slot).read();
    Stdout().writeln("read " + (await reader).string());
    0;
}
"#, "child 4\nput 4\nread 4\n");
}

/// The PR-A entry kinds apply to Futures only: a Cell over plain data holding an action value, in a module that
/// has Futures, keeps its owned publication on both sides (the summaries and SPB 17's P1 entries agree). Structural
/// only: the Full verifier refuses a holder of closures with B112/B104 before and after PR-A (#786).
#[test]
fn an_atomic_over_plain_data_keeps_its_entries_in_a_module_with_futures() {
    let run = run_source_with(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "cell" take Cell;
object Rule {
    has check is (int) is bool;
    Rule(:check);
    passes(value is int) is bool { check(value); }
}
object Judge {
    Judge();
    judged(value is int) is bool {
        let rules = Cell(Rule(check: (seen is int) is bool { seen > 1; }));
        rules.value().passes(value);
    }
}
object Child {
    has n is int;
    Child(:n);
    async run() is int { n; }
}
async main() is int {
    let n = await Child(n: 2).run();
    Stdout().writeln(Judge().judged(n).string());
    0;
}
"#, VerifyMode::Structural).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!((run.stdout.as_str(), run.result), ("true\n", Ok(0)));
}
