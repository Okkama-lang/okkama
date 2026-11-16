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
