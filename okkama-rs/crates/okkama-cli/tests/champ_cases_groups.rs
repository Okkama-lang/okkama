//! G1 step 1 (#810) on Champ, in process: FutureGroup. `start` registers a fresh Future's duty (its packet holds it)
//! and only retains a bound one; the group's own duty settles in its owner's dynamic step: its registered members'
//! outcomes are released in the owner's plan, and a member that still runs is detached until shutdown. Only `wait`
//! and `cancel` seal a group (memo OIR-2-async-remaining §§6.3–6.10).
//!
//! Each program runs under Structural and Full verification. The expected output is Tomato's from the rebuild tree,
//! the reference since #794 (a default build, 2026-11-16); it is the memo's too (§6.8: "register, member done, owner
//! closes: destroyed in the owner's plan"). The 0.5.8 release destroyed a registered outcome never, and an awaited
//! one when the awaiting expression ended: the old behaviour.
//!
//! Only pure language cases: no processes, workers, network, outside files or test mode.
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

const RES: &str = r#"object Res {
    has name is string;
    Res(:name);
    label() is string { name; }
    destroy { Stdout().writeln("destroy " + name); }
}
object Maker {
    Maker();
    async make(name is string) is Res { Stdout().writeln("make " + name); Res(name: name); }
}
"#;

fn program(imports: &str, body: &str) -> String {
    format!("module app v1.0.0;\nimport \"io/stdout\" take Stdout;\n{imports}\n{RES}{body}")
}

/// `start` spawns a fresh and a bound Future at once; `wait` waits for both and returns the group; the bound one is
/// awaited again through its binding.
#[test]
fn a_group_starts_its_members_and_waits_for_them() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "async/scope" take FutureGroup;
object Child {
    has n is int;
    Child(:n);
    async run() is int { Stdout().writeln("child " + n.string()); n; }
}
async main() is int {
    let group is FutureGroup<int> = FutureGroup();
    group.start(Child(n: 1).run());
    let bound = Child(n: 2).run();
    group.start(bound);
    group.start(bound);
    Stdout().writeln("active " + group.active().string());
    await group.wait();
    Stdout().writeln("waited " + (await bound).string());
    0;
}
"#, "child 1\nchild 2\nactive 0\nwaited 2\n");
}

/// Registration is decided by the packet (§6.3): the fresh member's duty is the group's, so its outcome is destroyed
/// when the group's owner closes (§6.8). The bound member is only retained: its binding's scope settles it, and its
/// outcome is the awaited value, destroyed once.
#[test]
fn a_registered_outcome_is_destroyed_in_the_owners_plan() {
    expect(&program("import \"async/scope\" take FutureGroup;", r#"object Helper {
    Helper();
    async go() is void {
        let group is FutureGroup<Res> = FutureGroup();
        group.start(Maker().make("fresh"));
        let bound = Maker().make("bound");
        group.start(bound);
        group.start(bound);
        Stdout().writeln("active " + group.active().string());
        await group.wait();
        let r = await bound;
        Stdout().writeln("got " + r.label());
    }
}
async main() is int {
    await Helper().go();
    Stdout().writeln("main end");
    0;
}
"#), "make fresh\nmake bound\nactive 0\ngot bound\ndestroy bound\ndestroy fresh\nmain end\n");
}

/// A member that still runs when the group's owner closes is detached to the root (§2.4): it is not cancelled by
/// the close, and shutdown cancels it, so the resource its body holds is destroyed after `main`.
#[test]
fn a_running_member_is_detached_when_its_owner_closes() {
    expect(&program("import \"async/scope\" take FutureGroup;\nimport \"async/event\" take Event;", r#"object Holding {
    Holding();
    async hold(name is string, gate is Event) is Res {
        let r = Res(name: name);
        Stdout().writeln("holding " + r.label());
        await gate.wait();
        Stdout().writeln("released " + r.label());
        r;
    }
}
object Helper {
    Helper();
    async go(gate is Event) is void {
        let group is FutureGroup<Res> = FutureGroup();
        group.start(Holding().hold("running", gate));
        Stdout().writeln("leaving, active " + group.active().string());
    }
}
async main() is int {
    let gate = Event();
    await Helper().go(gate);
    Stdout().writeln("main end");
    0;
}
"#), "holding running\nleaving, active 1\nmain end\ndestroy running\n");
}

/// After `wait` the group is sealed: `start` of a Future that is not a member fails with `FinishedFutureGroup` before
/// it registers anything. A fresh Future offered to it settles Cancelled and never runs (§6.7). The registered
/// member's outcome is destroyed when `main` closes (§6.8).
#[test]
fn a_sealed_group_refuses_a_fresh_member() {
    expect(&program("import \"async/scope\" take FutureGroup, FinishedFutureGroup;", r#"async main() is int {
    let group is FutureGroup<Res> = FutureGroup();
    group.start(Maker().make("first"));
    await group.wait();
    let text = try {
        group.start(Maker().make("late"));
        "started";
    } catch error is FinishedFutureGroup {
        "finished";
    };
    Stdout().writeln(text + ", active " + group.active().string());
    0;
}
"#), "make first\nfinished, active 0\ndestroy first\n");
}

/// A sealed group refuses a bound Future too; the Future stays with its binding and, never awaited, never runs.
#[test]
fn a_sealed_group_refuses_a_bound_member() {
    expect(&program("import \"async/scope\" take FutureGroup, FinishedFutureGroup;", r#"async main() is int {
    let group is FutureGroup<Res> = FutureGroup();
    let first = Maker().make("first");
    group.start(first);
    await group.wait();
    let late = Maker().make("late");
    let text = try { group.start(late); "started"; } catch error is FinishedFutureGroup { "finished"; };
    Stdout().writeln(text + ", active " + group.active().string() + ", got " + (await first).label());
    0;
}
"#), "make first\nfinished, active 0, got first\ndestroy first\n");
}

/// `start` of a member the group already has is a no-op, also after the group is sealed (§6.7, I-G1).
#[test]
fn a_sealed_group_accepts_its_own_member_again() {
    expect(&program("import \"async/scope\" take FutureGroup;", r#"async main() is int {
    let group is FutureGroup<Res> = FutureGroup();
    let first = Maker().make("first");
    group.start(first);
    await group.wait();
    group.start(first);
    Stdout().writeln("active " + group.active().string() + ", got " + (await first).label());
    0;
}
"#), "make first\nactive 0, got first\ndestroy first\n");
}

/// Only `wait` and `cancel` seal a group (std `async/scope`); its owner's close does not. A member that outlived the
/// owner starts another one into the group: it is accepted and runs.
#[test]
fn an_owners_close_does_not_seal_the_group() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "async/scope" take FutureGroup, Supervising, FinishedFutureGroup;
import "async/event" take Event;
object Child {
    Child();
    async run() is int { Stdout().writeln("child ran"); 1; }
}
object Late {
    Late();
    async run(group is Supervising<int>, gate is Event, done is Event) is int {
        await gate.wait();
        let text = try { group.start(Child().run()); "started"; } catch error is FinishedFutureGroup { "finished"; };
        Stdout().writeln("late start: " + text);
        done.set();
        0;
    }
}
object Helper {
    Helper();
    async go(gate is Event, done is Event) is void {
        let group is FutureGroup<int> = FutureGroup();
        group.start(Late().run(group, gate, done));
        Stdout().writeln("owner closes");
    }
}
async main() is int {
    let gate = Event();
    let done = Event();
    await Helper().go(gate, done);
    gate.set();
    await done.wait();
    Stdout().writeln("end");
    0;
}
"#, "owner closes\nchild ran\nlate start: started\nend\n");
}

/// After the owner closed there is no group duty to join: a member started then settles at once by its rows. A
/// finished one releases its outcome at the start; a running one is detached and released at shutdown, like the member
/// that was registered and still ran when the owner closed (`late body`). Each resource is destroyed once.
/// Intended behaviour (memo §2.4, #794); Tomato from the rebuild tree never destroys `late done` and `late body`.
#[test]
fn a_member_started_after_the_owner_closed_settles_on_its_own() {
    expect(&program("import \"async/scope\" take FutureGroup;\nimport \"async/event\" take Event;", r#"object Holding {
    Holding();
    async held(name is string, gate is Event) is Res {
        let r = Res(name: name);
        await gate.wait();
        r;
    }
}
object Late {
    Late();
    async run(group is FutureGroup<Res>, gate is Event, hold is Event, done is Event) is Res {
        await gate.wait();
        group.start(Maker().make("late done"));
        Stdout().writeln("after late done");
        group.start(Holding().held("late running", hold));
        Stdout().writeln("after late running, active " + group.active().string());
        done.set();
        Res(name: "late body");
    }
}
object Helper {
    Helper();
    async go(gate is Event, hold is Event, done is Event) is void {
        let group is FutureGroup<Res> = FutureGroup();
        group.start(Late().run(group, gate, hold, done));
        Stdout().writeln("owner closes");
    }
}
async main() is int {
    let gate = Event();
    let hold = Event();
    let done = Event();
    await Helper().go(gate, hold, done);
    gate.set();
    await done.wait();
    Stdout().writeln("end");
    0;
}
"#), concat!("owner closes\nmake late done\ndestroy late done\nafter late done\nafter late running, active 2\nend\n",
        "destroy late running\ndestroy late body\n"));
}

/// `wait` reports the first member failure; `cancel` seals the group and cancels what still runs, and a member the
/// group cancelled is quiet.
#[test]
fn member_failures_and_group_cancellation() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "async/scope" take FutureGroup;
import "async/event" take Event;
enum Broken is Error = (first, second);
object Fail {
    has which is int;
    Fail(:which);
    async run() is int {
        Stdout().writeln("fail " + which.string());
        if which == 1 { throw Broken.first; }
        throw Broken.second;
    }
}
object Never {
    has gate is Event;
    Never(:gate);
    async run() is int { await gate.wait(); 0; }
}
async main() is int {
    let group is FutureGroup<int> = FutureGroup();
    group.start(Fail(which: 1).run());
    group.start(Fail(which: 2).run());
    let text = try { await group.wait(); "ok"; } catch error is Broken {
        match error { case Broken.first { "caught first"; } case Broken.second { "caught second"; } };
    };
    Stdout().writeln(text);
    let other is FutureGroup<int> = FutureGroup();
    let gate = Event();
    other.start(Never(gate: gate).run());
    Stdout().writeln("active " + other.active().string());
    other.cancel();
    await other.wait();
    Stdout().writeln("cancelled quietly, active " + other.active().string());
    0;
}
"#, "fail 1\nfail 2\ncaught first\nactive 1\ncancelled quietly, active 0\n");
}

/// The group's own duty travels with the value that holds it (PR-C): a group returned from a function brings its
/// registered member, and a group in a field is waited through an async action of its holder, whose summary
/// retains the holder whole. Every registered outcome is destroyed when `main` closes (§6.8).
#[test]
fn a_group_moves_with_the_value_that_holds_it() {
    expect(&program("import \"async/scope\" take FutureGroup;", r#"object Box {
    has group is FutureGroup<Res>;
    Box(:group);
    start(name is string) is void { group.start(Maker().make(name)); }
    async wait() is void { await group.wait(); }
}
object Factory {
    Factory();
    made() is FutureGroup<Res> {
        let group is FutureGroup<Res> = FutureGroup();
        group.start(Maker().make("made"));
        group;
    }
}
async main() is int {
    let group = Factory().made();
    group.start(Maker().make("more"));
    await group.wait();
    Stdout().writeln("first waited");
    let box = Box(group: FutureGroup());
    box.start("boxed");
    await box.wait();
    Stdout().writeln("end");
    0;
}
"#), "make made\nmake more\nfirst waited\nmake boxed\nend\ndestroy boxed\ndestroy more\ndestroy made\n");
}

/// A fresh temporary group comes to `start` with its own duty, which never joins its own record (review F1): it
/// closes in the call, as a fresh argument does, so the members' outcomes are destroyed at the call. Tomato destroys
/// them when `main` ends; the call-boundary rule is the memo's (§6.10).
#[test]
fn a_fresh_temporary_group_settles_in_the_start_call() {
    expect(&program("import \"async/scope\" take FutureGroup;", r#"object Factory {
    Factory();
    made(n is string) is FutureGroup<Res> {
        let group is FutureGroup<Res> = FutureGroup();
        group.start(Maker().make(n));
        group;
    }
}
async main() is int {
    Factory().made("m1").start(Maker().make("temp"));
    Stdout().writeln("main end");
    0;
}
"#), "make m1\nmake temp\ndestroy temp\ndestroy m1\nmain end\n");
}

/// A fresh Future is offered to `start` also in a function that calls a closure, where other offers are elided
/// (review F2): the group registers it, and its owner's close destroys it.
#[test]
fn a_function_with_a_closure_call_registers_a_fresh_member() {
    expect(&program("import \"async/scope\" take FutureGroup;", r#"object Starter {
    Starter();
    plain(g is FutureGroup<Res>) is void { g.start(Maker().make("plain")); Stdout().writeln("plain end"); }
    mixed(g is FutureGroup<Res>) is void {
        let hello = () is void { Stdout().writeln("hello"); };
        hello();
        g.start(Maker().make("mixed"));
        Stdout().writeln("mixed end");
    }
}
async main() is int {
    let group is FutureGroup<Res> = FutureGroup();
    Starter().plain(group);
    Starter().mixed(group);
    await group.wait();
    Stdout().writeln("main end");
    0;
}
"#), "make plain\nplain end\nhello\nmake mixed\nmixed end\nmain end\ndestroy mixed\ndestroy plain\n");
}

/// A group returned as its role keeps its duty with the value (review F3: the verifier's `holds_futures` sees the
/// group behind `Supervising`): the registered outcome is destroyed when the caller's binding closes.
#[test]
fn a_group_returned_as_its_role_keeps_its_duty() {
    expect(&program("import \"async/scope\" take FutureGroup, Supervising;", r#"object Factory {
    Factory();
    direct() is Supervising<Res> {
        let fg is FutureGroup<Res> = FutureGroup();
        fg.start(Maker().make("pre"));
        Stdout().writeln("direct end");
        fg;
    }
}
object User {
    User();
    use() is void {
        let s = Factory().direct();
        Stdout().writeln("active " + s.active().string());
        Stdout().writeln("use end");
    }
}
async main() is int {
    User().use();
    Stdout().writeln("main end");
    0;
}
"#), "make pre\ndirect end\nactive 0\nuse end\ndestroy pre\nmain end\n");
}

/// The simplest groups compile (review F4): the close's envelope and the loader's demand both come from the member
/// type, so a group of plain members demands nothing and a group of resources demands their destroys, wherever the
/// members are started.
#[test]
fn a_groups_settle_effects_follow_its_member_type() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "async/scope" take FutureGroup;
object Child { Child(); async run() is int { Stdout().writeln("child"); 1; } }
object S { S(); go(g is FutureGroup<int>) is void { g.start(Child().run()); } }
async main() is int {
    let g is FutureGroup<int> = FutureGroup();
    g.start(Child().run());
    let h is FutureGroup<int> = FutureGroup();
    S().go(h);
    await h.wait();
    Stdout().writeln("main end");
    0;
}
"#, "child\nchild\nmain end\n");
    expect(&program("import \"async/scope\" take FutureGroup;", r#"object Starter {
    Starter();
    plain(g is FutureGroup<Res>) is void { g.start(Maker().make("plain")); Stdout().writeln("plain end"); }
}
async main() is int {
    let group is FutureGroup<Res> = FutureGroup();
    Starter().plain(group);
    Stdout().writeln("main end");
    0;
}
"#), "make plain\nplain end\nmain end\ndestroy plain\n");
    // An async action that makes its own group of resources, called with a group of plain members: its creations
    // stay with it, so the caller's close demands only what the inputs it may release reach.
    expect(&program("import \"async/scope\" take FutureGroup;", r#"object Child {
    Child();
    async run(r is FutureGroup<Res>) is int { Stdout().writeln("child " + r.active().string()); 1; }
}
object S {
    S();
    async go(g is FutureGroup<int>) is void {
        let inner is FutureGroup<Res> = FutureGroup();
        inner.start(Maker().make("done"));
        g.start(Child().run(inner));
    }
}
async main() is int {
    let g is FutureGroup<int> = FutureGroup();
    await S().go(g);
    await g.wait();
    Stdout().writeln("main end");
    0;
}
"#), "make done\nchild 0\ndestroy done\nmain end\n");
}

/// The loader's side of the same rule (review F4): the group's owner's close must admit the destroy failures of the
/// group's member type. With `main`'s envelopes emptied, the program is refused, though nothing in `main` itself
/// creates the resource: only the group reaches it.
#[test]
fn the_loader_demands_the_destroys_a_group_may_release() {
    let text = program("import \"async/scope\" take FutureGroup;", r#"object Starter {
    Starter();
    plain(g is FutureGroup<Res>) is void { g.start(Maker().make("plain")); }
}
async main() is int {
    let group is FutureGroup<Res> = FutureGroup();
    Starter().plain(group);
    0;
}
"#);
    let name = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/champ_support/inline.ok");
    let source = SourceFile::new(name.to_string_lossy(), text).unwrap();
    let bytes = compile(&[source]).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(run(bytes.clone()).map(|run| run.stdout).as_deref(), Ok("make plain\ndestroy plain\n"));
    let mut program = s::decode(&bytes).unwrap();
    let main = program.functions.iter_mut().find(|function| function.symbol == "main").expect("main");
    for envelope in &mut main.proofs.envelopes {
        envelope.failures = 0;
    }
    let refused = run(s::encode(&program).unwrap()).err().expect("an envelope without the member's destroys");
    assert!(refused.contains("D18_EXECUTOR"), "{refused}");
}

/// A group of groups: the element type is generic itself (`FutureGroup_t_object(…FutureGroup<…>)`), and a temporary
/// group's `wait` is registered in the outer group while it runs; the member's outcome is destroyed once.
#[test]
fn a_group_of_groups() {
    expect(&program("import \"async/scope\" take FutureGroup;\nimport \"async/event\" take Event;", r#"object Gated {
    Gated();
    async make(name is string, gate is Event) is Res { await gate.wait(); Stdout().writeln("make " + name); Res(name: name); }
}
object F {
    F();
    made(gate is Event) is FutureGroup<Res> {
        let g is FutureGroup<Res> = FutureGroup();
        g.start(Gated().make("member", gate));
        g;
    }
}
async main() is int {
    let gate = Event();
    let outer is FutureGroup<FutureGroup<Res>> = FutureGroup();
    outer.start(F().made(gate).wait());
    gate.set();
    await outer.wait();
    Stdout().writeln("main end");
    0;
}
"#), "make member\nmain end\ndestroy member\n");
}

/// Not a group: a fresh Future offered from a nested scope together with a value of an enclosing one. The inner
/// split runs first, so the enclosing split can fail with the Future's entry in flight; the Future's owner settles
/// entries at run time (D25.3). The callee refuses and closes the packet: the cold Future never runs.
#[test]
fn a_fresh_future_offered_from_a_nested_scope() {
    expect(&program("", r#"enum Broken is Error = (broken);
object Keeper {
    has res is Res;
    Keeper(:res);
    take(f is Future<Res>) is void { Stdout().writeln("refusing, keeping " + res.label()); throw Broken.broken; }
}
async main() is int {
    let keeper = Keeper(res: Res(name: "kept"));
    let text = try {
        keeper.take(Maker().make("late"));
        "taken";
    } catch error is Broken {
        "broken";
    };
    Stdout().writeln(text);
    0;
}
"#), "refusing, keeping kept\nbroken\ndestroy kept\n");
}
