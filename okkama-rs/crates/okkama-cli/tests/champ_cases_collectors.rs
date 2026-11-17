//! #810 G1 step 3 on Champ, in process: `AllFutures.results`, `Raced.winner` and `FirstSuccessful.outcome` run
//! Tomato's collector driver, which reads the origins through their `Sequencing` role with the driver call bridge;
//! AllFutures' result is made by the program's `Wrap` helper (ruling Q1). The outputs are those of
//! Tomato built from the rebuild tree. The programs run under Structural verification: an array of Futures behind
//! `Sequencing` goes through the builtin array-role adapter, which Full verification refuses until #796 (B51).
#![cfg(feature = "champ-abi")]

mod champ_support;

use champ_support::{check, run_source_with};
use okkama_compiler::oir2::VerifyMode;

const PRE: &str = r#"module app v1.0.0;

import "io/stdout" take Stdout;
import "async/event" take Event;
import "async/collect" take Raced, FirstSuccessful, NoFutures;

object Res {
    has name is string;
    Res(:name);
    label() is string { name; }
    destroy { Stdout().writeln("destroy " + name); }
}

object Waiter {
    has event is Event;
    Waiter(:event);
    async value(v is int) is int {
        await event.wait();
        v;
    }
}

object Values {
    Values();
    async v(label is string, value is int) is int {
        Stdout().writeln(label);
        value;
    }
    async res(name is string) is Res {
        Stdout().writeln("make " + name);
        Res(name: name);
    }
}

enum Broken is Error = (broken, again);

object Failing {
    Failing();
    async run(which is int) is int {
        Stdout().writeln("failing " + which.string());
        if which == 1 { throw Broken.broken; } else { throw Broken.again; }
    }
}
"#;

fn expect(main: &str, stdout: &str) {
    expect_program(&format!("{PRE}\n{main}"), stdout);
}

fn expect_program(source: &str, stdout: &str) {
    let run = run_source_with(source, VerifyMode::Structural).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!((run.stdout.as_str(), run.result), (stdout, Ok(0)));
}

/// An empty race throws `NoFutures`; AllFutures keeps input order and wakes what waits on it (conformance).
#[test]
fn collector_conformance_cases() {
    check(&["L08.collect.race-empty", "L08.collect.all-futures-order", "L08.wake.transitive",
        "L12.concurrent-cas-and-dedup"]);
}

/// The first origin to finish wins; the one still waiting on an event is cancelled, and the event stays unset.
#[test]
fn a_race_returns_the_first_outcome_and_cancels_the_rest() {
    expect(r#"async main() is int {
    let out = Stdout();
    let never = Event();
    let race is @Future<int> = [Waiter(event: never).value(1), Values().v("fast", 2)];
    out.writeln("winner " + (await Raced(origins: race).winner()).string());
    out.writeln("set " + never.ready().string());
    0;
}
"#, "fast\nwinner 2\nset false\n");
}

/// FirstSuccessful passes over failures to the first success; when every origin fails, the first failure is the
/// outcome; without origins it throws `NoFutures`.
#[test]
fn first_successful_skips_failures_until_one_succeeds() {
    expect(r#"async main() is int {
    let out = Stdout();
    let mixed is @Future<int> = [Failing().run(1), Values().v("ok", 3), Failing().run(2)];
    out.writeln("first " + (await FirstSuccessful(origins: mixed).outcome()).string());
    let bad is @Future<int> = [Failing().run(1), Failing().run(2)];
    out.writeln(try { (await FirstSuccessful(origins: bad).outcome()).string(); } catch Broken.broken { "failed broken"; } catch Broken.again { "failed again"; });
    let nothing is @Future<int> = [];
    out.writeln(try { (await FirstSuccessful(origins: nothing).outcome()).string(); } catch e is NoFutures { "none"; });
    0;
}
"#, "failing 1\nok\nfailing 2\nfirst 3\nfailing 1\nfailing 2\nfailed broken\nnone\n");
}

/// A race on a temporary receiver whose origins return resources: the winner is the awaiting scope's, and both
/// outcomes are destroyed once, when that scope ends, the loser first, as on Tomato.
#[test]
fn a_race_on_a_temporary_receiver_keeps_resource_outcomes_to_the_scope() {
    expect(r#"async main() is int {
    let out = Stdout();
    {
        let won = await Raced(origins: [Values().res("a"), Values().res("b")]).winner();
        out.writeln("won " + won.label());
    }
    out.writeln("after");
    0;
}
"#, "make a\nmake b\nwon a\ndestroy b\ndestroy a\nafter\n");
}

/// A user `Sequencing` whose `item` makes a fresh Future each time: those duties join the collector's inputs (review
/// of steps 3–4, Q2), so both outcomes are destroyed once, when the awaiting scope ends. Tomato never destroys them
/// (#845).
#[test]
fn a_user_sequence_that_makes_futures_gives_their_outcomes_to_the_awaiting_scope() {
    expect_program(r#"module app v1.0.0;

import "io/stdout" take Stdout;
import "async/collect" take Raced;
import "collections/sequence" take Sequencing, EmptySequence;
import "collections/iteration" take Cursor;

object Res {
    has name is string;
    Res(:name);
    label() is string { name; }
    destroy { Stdout().writeln("destroy " + name); }
}

object Maker {
    Maker();
    async make(name is string) is Res {
        Stdout().writeln("make " + name);
        Res(name: name);
    }
}

object Made does Cursor<Future<Res>> {
    has at is int;
    has count is int;
    Made(:at, :count);
    available() is bool { at < count; }
    item() is Future<Res> { Maker().make("m" + at.string()); }
    successor() is Cursor<Future<Res>> { Made(at: at + 1, count: count); }
}

object Making does Sequencing<Future<Res>> {
    has count is int;
    Making(:count);
    cursor() is Cursor<Future<Res>> { Made(at: 0, count: count); }
    size() is int { count; }
    item(index is int) is Future<Res> {
        Stdout().writeln("item " + index.string());
        Maker().make("m" + index.string());
    }
    first() is Future<Res> { Maker().make("first"); }
    last() is Future<Res> { Maker().make("last"); }
}

async main() is int {
    {
        let won = await Raced(origins: Making(count: 2)).winner();
        Stdout().writeln("won " + won.label());
    }
    Stdout().writeln("after");
    0;
}
"#, "item 0\nitem 1\nmake m0\nmake m1\nwon m0\ndestroy m1\ndestroy m0\nafter\n");
}

/// A running race whose duty moves into a group keeps the fresh receiver's origins with it, and both outcomes are
/// destroyed once (the shape of review T1).
#[test]
fn a_running_race_moved_to_a_group_keeps_its_inputs() {
    expect_program(r#"module app v1.0.0;

import "io/stdout" take Stdout;
import "async/collect" take Raced;
import "async/scope" take FutureGroup;
import "async/event" take Event;

object Res { has name is string; Res(:name); label() is string { name; } destroy { Stdout().writeln("destroy " + name); } }
object Maker {
    Maker();
    async gated(name is string, gate is Event) is Res { await gate.wait(); Stdout().writeln("make " + name); Res(name: name); }
}
object F {
    F();
    started(g is FutureGroup<Res>, gate is Event) is Future<Res> {
        let f = Raced(origins: [Maker().gated("x", gate), Maker().gated("y", gate)]).winner();
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
"#, "started\nmake x\nmake y\ngot x\ndestroy y\ndestroy x\n");
}

/// A running race detached when its owner closes keeps its inputs until the root closer releases them at shutdown
/// (the shape of review T1).
#[test]
fn a_detached_race_releases_its_inputs_at_shutdown() {
    expect_program(r#"module app v1.0.0;

import "io/stdout" take Stdout;
import "async/collect" take Raced;
import "async/scope" take FutureGroup;
import "async/event" take Event;

object Res { has name is string; Res(:name); label() is string { name; } destroy { Stdout().writeln("destroy " + name); } }
object Maker {
    Maker();
    async gated(name is string, gate is Event) is Res { await gate.wait(); Stdout().writeln("make " + name); Res(name: name); }
}
object H {
    H();
    async go(g is FutureGroup<Res>, gate is Event) is void {
        let f = Raced(origins: [Maker().gated("x", gate), Maker().gated("y", gate)]).winner();
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
"#, "go end\nmake x\nmake y\nmain end\ndestroy y\ndestroy x\n");
}

/// AllFutures: results in input order, not finishing order; the first failure is thrown once every origin has
/// settled; no origins give an empty result; resource outcomes are the awaiting scope's and are destroyed once,
/// when it ends. `AllFutures<string>` needs an array adapter for `Sequencing<string>` the program never converts
/// to itself: the producer declares it for the `Wrap` helper.
#[test]
fn all_futures_keeps_input_order_failures_and_resources() {
    expect_program(r#"module app v1.0.0;

import "io/stdout" take Stdout;
import "async/event" take Event;
import "async/collect" take AllFutures;

object Res {
    has name is string;
    Res(:name);
    label() is string { name; }
    destroy { Stdout().writeln("destroy " + name); }
}

object Gate {
    has event is Event;
    Gate(:event);
    async word(text is string) is string {
        await event.wait();
        text;
    }
}

object Values {
    Values();
    async word(text is string) is string { text; }
    async res(name is string) is Res {
        Stdout().writeln("make " + name);
        Res(name: name);
    }
}

enum Broken is Error = (broken, again);

object Failing {
    Failing();
    async run(which is int) is string {
        Stdout().writeln("failing " + which.string());
        if which == 1 { throw Broken.broken; } else { throw Broken.again; }
    }
}

async main() is int {
    let out = Stdout();
    let late = Event();
    let words is @Future<string> = [Gate(event: late).word("second"), Values().word("first")];
    let all is AllFutures<string> = AllFutures(origins: words);
    late.set();
    let results = await all.results();
    out.writeln(results.size().string() + ": " + results.item(0) + ", " + results.item(1));
    let failing is @Future<string> = [Values().word("ok"), Failing().run(1), Failing().run(2)];
    out.writeln(try { (await AllFutures(origins: failing).results()).size().string(); }
        catch Broken.broken { "broken"; } catch Broken.again { "again"; });
    let nothing is @Future<string> = [];
    out.writeln("empty " + (await AllFutures(origins: nothing).results()).size().string());
    {
        let made = await AllFutures(origins: [Values().res("a"), Values().res("b")]).results();
        out.writeln("made " + made.item(0).label() + made.item(1).label());
    }
    out.writeln("after");
    0;
}
"#,
        "2: second, first\nfailing 1\nfailing 2\nbroken\nempty 0\nmake a\nmake b\nmade ab\ndestroy b\ndestroy a\nafter\n");
}
