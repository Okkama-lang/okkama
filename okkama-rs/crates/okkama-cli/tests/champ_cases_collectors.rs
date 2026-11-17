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
///
/// Ruling Q3: a loser's outcome stays in its cache until the collector's duty settles. When the winner leaves the
/// function that awaited the race (`pick() is Res { await Raced(origins: [res("a"), res("b")]).winner(); }`), the
/// loser goes with that duty to the caller: Champ prints `inner end a / main has a / destroy b / destroy a`, Tomato
/// destroys `b` before `main has a`. That follows from Q3 and is not a defect (review of steps 3–4, C3); no test pins
/// Tomato's earlier moment.
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

/// FutureCompletions hands the origins back in the order they settle, counts what is left, and throws `NoFutures` once
/// none is (the Bible's example).
#[test]
fn completions_hand_origins_back_in_settlement_order() {
    expect_program(r#"module bible/completions v1.0.0;

// FutureCompletions hands the futures back in the order they finish, not
// the order they were given.
import "async/collect" take FutureCompletions, NoFutures;
import "async/event" take Event;
import "io/stdout" take Stdout;

object Gate {
    has event is Event;
    Gate(:event);
    async named(name is string) is string {
        await event.wait();
        name;
    }
}

object Now {
    Now();
    async named(name is string) is string { name; }
}

async main() is int {
    let out = Stdout();
    let first = Event();
    let second = Event();
    let futures is @Future<string> = [Gate(event: second).named("slow"), Now().named("quick"),
        Gate(event: first).named("middle")];
    let completions is FutureCompletions<string> = FutureCompletions(origins: futures);
    out.writeln("remaining " + completions.remaining_count().string());
    first.set();
    out.writeln(await (await completions.completion()));
    out.writeln(await (await completions.completion()));
    second.set();
    out.writeln(await (await completions.completion()));
    out.writeln("remaining " + completions.remaining_count().string());
    out.writeln(try { await (await completions.completion()); } catch e is NoFutures { "none left"; });
    0;
}
"#, "remaining 3\nquick\nmiddle\nslow\nremaining 0\nnone left\n");
}

/// The origins stay with the array that owns them: each resource outcome is made once and destroyed when the scope of
/// that array ends, after the stream that read them.
#[test]
fn completed_resources_stay_with_the_owner_of_the_origins() {
    expect_program(r#"module app v1.0.0;

import "io/stdout" take Stdout;
import "async/event" take Event;
import "async/collect" take FutureCompletions;

object Res {
    has name is string;
    Res(:name);
    label() is string { name; }
    destroy { Stdout().writeln("destroy " + name); }
}

object Gate {
    has event is Event;
    Gate(:event);
    async res(name is string) is Res {
        await event.wait();
        Stdout().writeln("make " + name);
        Res(name: name);
    }
}

async main() is int {
    let out = Stdout();
    {
        let gate = Event();
        let inputs is @Future<Res> = [Gate(event: gate).res("a"), Gate(event: gate).res("b")];
        let stream is FutureCompletions<Res> = FutureCompletions(origins: inputs);
        out.writeln("remaining " + stream.remaining_count().string());
        gate.set();
        let first = await stream.completion();
        out.writeln("first " + (await first).label());
        let second = await stream.completion();
        out.writeln("second " + (await second).label() + ", remaining " + stream.remaining_count().string());
    }
    out.writeln("after");
    0;
}
"#, "remaining 2\nmake a\nmake b\nfirst a\nsecond b, remaining 0\ndestroy b\ndestroy a\nafter\n");
}

/// A failed origin is handed back like any other, and its error is thrown where it is awaited.
#[test]
fn a_failed_origin_is_handed_back_and_throws_where_it_is_awaited() {
    expect_program(r#"module app v1.0.0;

import "io/stdout" take Stdout;
import "async/event" take Event;
import "async/collect" take FutureCompletions, NoFutures;

enum Broken is Error = (broken);

object Work {
    has event is Event;
    Work(:event);
    async fail() is int {
        Stdout().writeln("failing");
        throw Broken.broken;
    }
    async late(v is int) is int {
        await event.wait();
        v;
    }
}

async main() is int {
    let out = Stdout();
    let gate = Event();
    let inputs is @Future<int> = [Work(event: gate).late(7), Work(event: gate).fail()];
    let stream is FutureCompletions<int> = FutureCompletions(origins: inputs);
    let first = await stream.completion();
    out.writeln(try { (await first).string(); } catch e is Broken { "caught broken"; });
    gate.set();
    out.writeln("then " + (await (await stream.completion())).string());
    out.writeln(try { (await (await stream.completion())).string(); } catch e is NoFutures { "none"; });
    0;
}
"#, "failing\ncaught broken\nthen 7\nnone\n");
}

/// An empty array of origins: nothing remains, and the first request throws `NoFutures`.
#[test]
fn an_empty_stream_counts_nothing_and_throws_no_futures() {
    expect_program(r#"module app v1.0.0;

import "io/stdout" take Stdout;
import "async/collect" take FutureCompletions, NoFutures;

async main() is int {
    let out = Stdout();
    let inputs is @Future<string> = [];
    let stream is FutureCompletions<string> = FutureCompletions(origins: inputs);
    out.writeln("remaining " + stream.remaining_count().string());
    out.writeln(try { await (await stream.completion()); } catch e is NoFutures { "none"; });
    0;
}
"#, "remaining 0\nnone\n");
}

/// A stream read once and left: the origin still waiting is never forced, and the outcome already made is destroyed
/// once, with the array of origins.
#[test]
fn a_stream_read_once_leaves_the_rest_to_the_owner_of_the_origins() {
    expect_program(r#"module app v1.0.0;

import "io/stdout" take Stdout;
import "async/event" take Event;
import "async/collect" take FutureCompletions;

object Res {
    has name is string;
    Res(:name);
    label() is string { name; }
    destroy { Stdout().writeln("destroy " + name); }
}

object Maker {
    has event is Event;
    Maker(:event);
    async quick(name is string) is Res { Res(name: name); }
    async slow(name is string) is Res {
        await event.wait();
        Stdout().writeln("slow done " + name);
        Res(name: name);
    }
}

async main() is int {
    let out = Stdout();
    let never = Event();
    {
        let inputs is @Future<Res> = [Maker(event: never).slow("s"), Maker(event: never).quick("q")];
        let stream is FutureCompletions<Res> = FutureCompletions(origins: inputs);
        let first = await stream.completion();
        out.writeln("first " + (await first).label() + ", remaining " + stream.remaining_count().string());
    }
    out.writeln("after");
    0;
}
"#, "first q, remaining 1\ndestroy q\nafter\n");
}

/// A stream asked once whose two origins have both settled: the outcome that was never awaited is destroyed too.
#[test]
fn an_outcome_never_awaited_is_destroyed_with_the_origins() {
    expect_program(r#"module app v1.0.0;

import "io/stdout" take Stdout;
import "async/collect" take FutureCompletions;

object Res {
    has name is string;
    Res(:name);
    label() is string { name; }
    destroy { Stdout().writeln("destroy " + name); }
}

object Now {
    Now();
    async res(name is string) is Res { Res(name: name); }
}

async main() is int {
    {
        let inputs is @Future<Res> = [Now().res("a"), Now().res("b")];
        let stream is FutureCompletions<Res> = FutureCompletions(origins: inputs);
        let first = await stream.completion();
        Stdout().writeln("asked once, remaining " + stream.remaining_count().string());
    }
    Stdout().writeln("after");
    0;
}
"#, "asked once, remaining 1\ndestroy b\ndestroy a\nafter\n");
}

/// An async helper that borrows the stream drains it: two requests in a row, then a loop until nothing remains; the
/// stream keeps its state between the two calls.
#[test]
fn a_helper_that_borrows_the_stream_drains_it() {
    expect_program(r#"module app v1.0.0;

import "io/stdout" take Stdout;
import "async/event" take Event;
import "async/collect" take FutureCompletions;

object Gate {
    has event is Event;
    Gate(:event);
    async named(name is string) is string {
        await event.wait();
        name;
    }
}

object Now {
    Now();
    async named(name is string) is string { name; }
}

object Drain {
    Drain();
    async run(stream is FutureCompletions<string>) is int {
        loop {
            last if stream.remaining_count() == 0;
            Stdout().writeln("got " + (await (await stream.completion())));
        }
        stream.remaining_count();
    }
    async two(stream is FutureCompletions<string>) is int {
        let one = await stream.completion();
        Stdout().writeln("one " + (await one));
        let other = await stream.completion();
        Stdout().writeln("other " + (await other));
        stream.remaining_count();
    }
}

async main() is int {
    let gate = Event();
    let inputs is @Future<string> = [Gate(event: gate).named("late"), Now().named("a"), Now().named("b")];
    let stream is FutureCompletions<string> = FutureCompletions(origins: inputs);
    Stdout().writeln("left " + (await Drain().two(stream)).string());
    gate.set();
    Stdout().writeln("left " + (await Drain().run(stream)).string());
    0;
}
"#, "one a\nother b\nleft 1\ngot late\nleft 0\n");
}

/// A user `Sequencing` whose `item` makes a fresh Future each time: the stream cannot keep that duty for the time it
/// streams, so the first `item` is refused with a named program failure, after its Future was closed (review of steps
/// 3–4, Q2). Tomato streams such Futures and never destroys them (#845).
#[test]
fn a_stream_refuses_an_origin_that_comes_with_a_duty_of_its_own() {
    let run = run_source_with(r#"module app v1.0.0;

import "io/stdout" take Stdout;
import "async/collect" take FutureCompletions;
import "collections/sequence" take Sequencing;
import "collections/iteration" take Cursor;

object Maker {
    Maker();
    async make(name is string) is string {
        Stdout().writeln("make " + name);
        name;
    }
}

object Made does Cursor<Future<string>> {
    has at is int;
    has count is int;
    Made(:at, :count);
    available() is bool { at < count; }
    item() is Future<string> { Maker().make("m" + at.string()); }
    successor() is Cursor<Future<string>> { Made(at: at + 1, count: count); }
}

object Making does Sequencing<Future<string>> {
    has count is int;
    Making(:count);
    cursor() is Cursor<Future<string>> { Made(at: 0, count: count); }
    size() is int { count; }
    item(index is int) is Future<string> {
        Stdout().writeln("item " + index.string());
        Maker().make("m" + index.string());
    }
    first() is Future<string> { Maker().make("first"); }
    last() is Future<string> { Maker().make("last"); }
}

async main() is int {
    let stream is FutureCompletions<string> = FutureCompletions(origins: Making(count: 2));
    Stdout().writeln("remaining " + stream.remaining_count().string());
    Stdout().writeln(await (await stream.completion()));
    0;
}
"#, VerifyMode::Structural).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(run.stdout, "item 0\n");
    assert_eq!(run.result, Err("RuntimeError: FutureCompletions: an origin was returned with a duty of its own; only \
        origins whose duties stay with their owners can be streamed".to_string()));
}

/// A stream first read by a request inside a helper: that request listens to the origin still waiting, and when the
/// helper ends its request is released. The stream still hears the origin settle, so the next request, in `main`,
/// gets it (Champ lost it: the release cleared the request's receiver).
#[test]
fn a_stream_started_in_a_helper_still_hears_its_origins_after_the_helper_ends() {
    expect_program(r#"module app v1.0.0;

import "io/stdout" take Stdout;
import "async/event" take Event;
import "async/collect" take FutureCompletions;

object Gate {
    has event is Event;
    Gate(:event);
    async named(name is string) is string {
        await event.wait();
        name;
    }
}

object Now {
    Now();
    async named(name is string) is string { name; }
}

object Drain {
    Drain();
    async one(stream is FutureCompletions<string>) is int {
        Stdout().writeln("got " + (await (await stream.completion())));
        stream.remaining_count();
    }
}

async main() is int {
    let gate = Event();
    let inputs is @Future<string> = [Gate(event: gate).named("late"), Now().named("a")];
    let stream is FutureCompletions<string> = FutureCompletions(origins: inputs);
    Stdout().writeln("left " + (await Drain().one(stream)).string());
    gate.set();
    Stdout().writeln("main " + (await (await stream.completion())));
    0;
}
"#, "got a\nleft 1\nmain late\n");
}

/// AllFutures over byte outcomes: the `Wrap` helper takes a `bytes` value, which the bridge builds from the members'
/// byte words, the whole range included (review of steps 3–4, C3).
#[test]
fn all_futures_over_bytes_keeps_every_byte_value() {
    expect_program(r#"module app v1.0.0;

import "io/stdout" take Stdout;
import "async/collect" take AllFutures;

object Bits {
    Bits();
    async byte(value is int) is byte { value.byte(); }
}

async main() is int {
    let inputs is @Future<byte> = [Bits().byte(7), Bits().byte(200), Bits().byte(0), Bits().byte(255)];
    let values = await AllFutures(origins: inputs).results();
    Stdout().writeln(values.size().string() + ": " + values.item(0).string() + " " + values.item(1).string() + " "
        + values.item(2).string() + " " + values.item(3).string());
    0;
}
"#, "4: 7 200 0 255\n");
}
