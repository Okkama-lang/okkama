//! PR-P (#749) on Champ, in process, under the Full ownership verifier: a result keeps a carrier-free field of a
//! parameter (no obligation and no Future reachable: an Atomic, or an object holding only one) by declaring it as an
//! alias. Each program's output is Tomato's (okkama 0.5.8).
//!
//! Only pure language cases: no processes, workers, network, outside files or test mode.
#![cfg(feature = "champ-abi")]

mod champ_support;

use champ_support::run_source_with;
use okkama_compiler::oir2::VerifyMode;

fn expect_full(source: &str, stdout: &str, status: i64) {
    let run = run_source_with(source, VerifyMode::Full).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!((run.stdout.as_str(), run.result), (stdout, Ok(status)));
}

/// A taken receiver lends its log to the step it returns while the step takes its resource; a borrowed receiver
/// (no obligation) lends its log on both arms of a branch; the caller's receivers are temporaries that their callee
/// settles, and the steps outlive them.
#[test]
fn a_result_keeps_lent_fields_of_taken_and_borrowed_receivers() {
    expect_full(r#"module app v1.0.0;

import "io/stdout" take Stdout;
import "async/atomic" take Atomic;

object Log {
    has events is Atomic<@string>;
    Log() { events = Atomic(@string()); }
    add(event is string) is void { events.replace(events.value() + [event]); }
    text() is string { events.value().join(" "); }
}

object Res {
    has name is string;
    has log is Log;
    Res(name is string, log is Log) { self.name = name; self.log = log; log.add("+" + name); }
    destroy { log.add("-" + name); }
    name() is string { name; }
}

// A taken receiver: the step keeps the receiver's resource (take retention) and its log (lent, PR-P).
object Source {
    has res is Res;
    has log is Log;
    Source(log is Log) { self.log = log; res = Res("src", log); }
    first() is Step { Step(log, res, 0); }
}

object Step {
    has log is Log;
    has source is Res;
    has at is int;
    Step(log is Log, source is Res, at is int) { self.log = log; self.source = source; self.at = at; }
    seen() is void { log.add("i" + at.string() + "(" + source.name() + ")"); }
    next() is Step { Step(log, source, at + 1); }
}

// A borrowed receiver (no obligation): the tick keeps its log, read on both arms of a branch.
object Counter {
    has log is Log;
    has size is int;
    Counter(log is Log, size is int) { self.log = log; self.size = size; }
    first() is Tick {
        if size > 1 { Tick(log, 0); } else { Tick(log, 1); }
    }
}

object Tick {
    has log is Log;
    has at is int;
    Tick(log is Log, at is int) { self.log = log; self.at = at; }
    seen() is void { log.add("t" + at.string()); }
}

main() is int {
    let log = Log();
    let step = Source(log).first().next();
    step.seen();
    Counter(log, 2).first().seen();
    Stdout().writeln(log.text());
    0;
}
"#, "+src i1(src) t0\n", 0);
}

/// The same with an Atomic field, the result made on either arm of a branch (it leaves through the exit's commit),
/// and a result that keeps only the lent counter (one alias: the caller sees the projection itself) from a
/// temporary receiver that its callee settles.
#[test]
fn a_result_made_in_a_branch_keeps_a_lent_atomic() {
    expect_full(r#"module app v1.0.0;

import "async/atomic" take Atomic;

object Res {
    has n is int;
    Res(:n);
    destroy { }
    n() is int { n; }
}

// The step keeps the receiver's resource (a take retention) and its counter, an Atomic: lent (PR-P).
object Source {
    has res is Res;
    has count is Atomic<int>;
    Source(count is Atomic<int>) { self.count = count; res = Res(n: 2); }
    tag() is Tag { Tag(count); }
    first(late is bool) is Step {
        if late { Step(count, res); } else { Step(count, res); }
    }
}

// Keeps only the lent counter: its result relation has one alias, so the caller sees the projection itself.
object Tag {
    has count is Atomic<int>;
    Tag(count is Atomic<int>) { self.count = count; }
    at() is int { count.value(); }
}

object Step {
    has count is Atomic<int>;
    has source is Res;
    Step(count is Atomic<int>, source is Res) { self.count = count; self.source = source; }
    at() is int { count.value() + source.n(); }
}

main() is int {
    let count = Atomic(1);
    Source(count).first(true).at() + Source(count).tag().at();
}
"#, "", 4);
}
