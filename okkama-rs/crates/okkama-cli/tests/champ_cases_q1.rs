//! #601 Q1: the std async primitives on Champ (OIR-2 async ABI D22): Event, Lock, Semaphore, Barrier and the
//! cancellation token, compiled through OIR-2 and run in process. Conformance cases are checked in full against their
//! case.json; the inline programs' expected output is Tomato's for the same program (okkama 0.5.8, run in the cage).
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
    q1_wake_event_set => "L08.wake.event-set",
    q1_wake_already_settled => "L08.wake.already-settled",
    q1_wake_multiple_waiters => "L08.wake.multiple-waiters",
    q1_wake_cancel_defers_cleanup => "L08.wake.cancel-defers-cleanup",
    q1_spawn_event_barrier => "L08.spawn.event-barrier",
    q1_deadlock_two_tasks => "L08.deadlock.two-tasks",
    q1_shutdown_outstanding_task_cleanup => "L08.shutdown.outstanding-task-cleanup",
    q1_cancel_awaited_cleanup_settles => "L08.cancel.awaited-cleanup-settles",
    q1_cancel_awaited_spawned => "L08.cancel.awaited-spawned",
    q1_cancel_final_defer_await_interrupted => "L08.cancel.final-defer-await-interrupted",
    q1_cancellation_runs_cleanup => "L12.cancellation-runs-cleanup",
}

fn expect(source: &str, stdout: &str, status: i64) {
    let run = run_source(source).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!((run.stdout.as_str(), run.result), (stdout, Ok(status)));
}

/// #739, K2: `L08.cancel.late-after-last-await` with the child Future passed as an argument (the case keeps it in a
/// field, #771): a cancellation that arrives after the last await settled is not delivered at the final exit.
#[test]
fn k2_cancel_after_the_last_await_keeps_the_result() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "async/event" take Event;
object Child {
    has go is Event;
    Child(:go);
    async run() is int { await go.wait(); 42; }
}
object Worker {
    has ready is Event;
    has go is Event;
    Worker(:ready, :go);
    async run(child is Future<int>) is int {
        defer Stdout().writeln("worker cleanup");
        await ready.wait();
        go.set();
        let v = await child;
        Stdout().writeln("worker observed " + v.string());
        v;
    }
}
async main() is int {
    let ready = Event();
    let go = Event();
    let child = spawn Child(go: go).run();
    let worker = spawn Worker(ready: ready, go: go).run(child);
    ready.set();
    Stdout().writeln("main observed " + (await child).string());
    worker.cancel();
    Stdout().writeln("cancel requested");
    let r = try { await worker; } catch e is Cancelled { -1; };
    Stdout().writeln("worker returned " + r.string() + ", cancelled " + worker.cancelled().string());
    0;
}
"#, "main observed 42\ncancel requested\nworker observed 42\nworker cleanup\nworker returned 42, cancelled false\n", 0);
}

/// #739: `L08.cancel.after-await-settled` with the child Future passed as an argument (#771): the worker observes
/// the value its finished await delivered and is cancelled at its next await (Bible §20.5).
#[test]
fn k2_cancel_after_a_settled_await_reaches_the_next_await() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "async/event" take Event;
object Child {
    has go is Event;
    Child(:go);
    async run() is int { await go.wait(); 42; }
}
object Worker {
    has ready is Event;
    has go is Event;
    has never is Event;
    Worker(:ready, :go, :never);
    async run(child is Future<int>) is int {
        defer Stdout().writeln("worker cleanup");
        await ready.wait();
        go.set();
        let v = await child;
        Stdout().writeln("worker observed " + v.string());
        await never.wait();
        Stdout().writeln("worker not cancelled");
        v;
    }
}
async main() is int {
    let ready = Event();
    let go = Event();
    let never = Event();
    let child = spawn Child(go: go).run();
    let worker = spawn Worker(ready: ready, go: go, never: never).run(child);
    ready.set();
    let v = await child;
    Stdout().writeln("main observed " + v.string());
    worker.cancel();
    Stdout().writeln("cancel requested");
    let r = try { await worker; } catch e is Cancelled { -1; };
    Stdout().writeln(r.string());
    0;
}
"#, "main observed 42\ncancel requested\nworker observed 42\nworker cleanup\n-1\n", 0);
}

/// #739: `L08.cancel.shared-awaited-survives` with the shared Future passed as an argument (#771): cancelling one
/// awaiter leaves the Future another frame awaits running.
#[test]
fn k2_a_shared_awaited_future_survives_its_cancelled_awaiter() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "async/event" take Event;
object Child {
    has started is Event;
    has never is Event;
    has release is Event;
    Child(:started, :never, :release);
    async run() is int {
        defer await self.settle();
        started.set();
        await never.wait();
        1;
    }
    async settle() is void {
        await release.wait();
        Stdout().writeln("child settled");
    }
}
object Parent {
    Parent();
    async run(pending is Future<int>) is int { await pending; }
}
object Watcher {
    Watcher();
    async run(pending is Future<int>) is int { await pending; }
}
async main() is int {
    let started = Event();
    let never = Event();
    let release = Event();
    let child = spawn Child(started: started, never: never, release: release).run();
    let watcher = spawn Watcher().run(child);
    let parent = spawn Parent().run(child);
    await started.wait();
    parent.cancel();
    Stdout().writeln("cancel requested");
    let r = try { await parent; } catch e is Cancelled { -1; };
    Stdout().writeln(r.string());
    release.set();
    never.set();
    Stdout().writeln((await watcher).string());
    Stdout().writeln((await child).string());
    0;
}
"#, "cancel requested\n-1\nchild settled\n1\n1\n", 0);
}

/// Event: waiters released by one set, a ready-made Event, a cancelled waiter and a cold wait cancelled unstarted.
#[test]
fn q1_an_event_releases_every_waiter_once_set() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "async/event" take Event;
object Waiter {
    has event is Event;
    has name is string;
    Waiter(:event, :name);
    async run() is int {
        let e = await event.wait();
        Stdout().writeln(name + " woke " + e.ready().string());
        1;
    }
}
async main() is int {
    let event = Event();
    Stdout().writeln("ready " + event.ready().string());
    let a = spawn Waiter(event: event, name: "a").run();
    let b = spawn Waiter(event: event, name: "b").run();
    let pending = event.wait();
    event.set();
    event.set();
    Stdout().writeln("set " + event.ready().string());
    await a;
    await b;
    Stdout().writeln("pending " + (await pending).ready().string());
    Stdout().writeln("ready-made " + (await Event(ready: true).wait()).ready().string());
    let never = Event();
    let w = spawn Waiter(event: never, name: "c").run();
    w.cancel();
    let r = try { await w; } catch e is Cancelled { -1; };
    Stdout().writeln("cancelled " + r.string() + " " + w.cancelled().string());
    let cold = never.wait();
    cold.cancel();
    Stdout().writeln("cold " + cold.cancelled().string());
    0;
}
"#, concat!("ready false\nset true\na woke true\nb woke true\npending true\nready-made true\n",
        "cancelled -1 true\ncold true\n"), 0);
}

/// Lock: a second guard waits in a spawned task until the first is closed; close is idempotent.
#[test]
fn q1_a_lock_admits_one_guard_at_a_time() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "async/lock" take Lock, LockGuard;
import "async/semaphore" take Semaphore, Permit, InvalidPermitCount;
object Holder {
    has lock is Lock;
    has name is string;
    Holder(:lock, :name);
    async run() is int {
        let guard = await lock.guard();
        Stdout().writeln(name + " holds " + lock.locked().string());
        guard.close();
        Stdout().writeln(name + " released " + guard.closed().string());
        1;
    }
}
async main() is int {
    let lock = Lock();
    Stdout().writeln("locked " + lock.locked().string());
    let first = await lock.guard();
    let waiter = spawn Holder(lock: lock, name: "b").run();
    Stdout().writeln("main holds " + lock.locked().string());
    first.close();
    first.close();
    await waiter;
    Stdout().writeln("after " + lock.locked().string());
    0;
}
"#, "locked false\nmain holds true\nb holds true\nb released true\nafter false\n", 0);
}

/// Semaphore: permits are taken and given back; close is idempotent.
#[test]
fn q1_a_semaphore_counts_its_permits() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "async/lock" take Lock, LockGuard;
import "async/semaphore" take Semaphore, Permit, InvalidPermitCount;
object Holder {
    has lock is Lock;
    has name is string;
    Holder(:lock, :name);
    async run() is int {
        let guard = await lock.guard();
        Stdout().writeln(name + " holds " + lock.locked().string());
        guard.close();
        Stdout().writeln(name + " released " + guard.closed().string());
        1;
    }
}
async main() is int {
    let semaphore = Semaphore(permits: 2);
    let a = await semaphore.permit();
    let b = await semaphore.permit();
    Stdout().writeln("permits " + semaphore.permits().string());
    a.close();
    Stdout().writeln("permits " + semaphore.permits().string() + " " + a.closed().string());
    b.close();
    Stdout().writeln("permits " + semaphore.permits().string());
    0;
}
"#, "permits 0\npermits 1 true\npermits 2\n", 0);
}

/// Barrier: arrival indices, its state, abort with BrokenBarrier, reset; a Cancellation cancelled twice.
#[test]
fn q1_a_barrier_releases_its_parties_and_breaks() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "async/barrier" take Barrier, BarrierState, BrokenBarrier;
import "async/cancellation" take Cancellation, CancellationToken;
object Party {
    has barrier is Barrier;
    Party(:barrier);
    async arrive() is int { await barrier.wait(); }
}
async main() is int {
    let barrier = Barrier(parties: 2);
    let first = spawn Party(barrier: barrier).arrive();
    let second = await barrier.wait();
    Stdout().writeln("arrivals " + (await first).string() + " " + second.string());
    let state = barrier.state();
    Stdout().writeln("state " + state.parties().string() + " " + state.waiters().string() + " "
        + state.generation().string() + " " + state.broken().string());
    barrier.abort();
    Stdout().writeln("broken " + barrier.state().broken().string());
    let r = try { await barrier.wait(); } catch e is BrokenBarrier { Stdout().writeln(e.text()); -1; };
    Stdout().writeln("wait " + r.string());
    barrier.reset();
    Stdout().writeln("reset " + barrier.state().broken().string() + " " + barrier.state().generation().string());
    let cancellation = Cancellation();
    cancellation.cancel();
    cancellation.cancel();
    Stdout().writeln("cancelled once");
    0;
}
"#, "arrivals 0 1\nstate 2 0 1 false\nbroken true\nBrokenBarrier.broken\nwait -1\nreset false 2\ncancelled once\n", 0);
}

/// #739, K2 "direct": the scenario of `L08.cancel.final-exit-window` that made the ruling provisional, with the child
/// Future passed as an argument (the case keeps it in a field, #771). The worker's await of the child has
/// finished when the request arrives and it has no later await: the result stands, `cancelled()` stays false.
#[test]
fn k2_direct_cancel_after_the_last_await_keeps_the_result() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "async/event" take Event;
object Child {
    has go is Event;
    Child(:go);
    async run() is int { await go.wait(); 42; }
}
object Worker {
    has ready is Event;
    has go is Event;
    Worker(:ready, :go);
    async direct(child is Future<int>) is int {
        defer Stdout().writeln("direct cleanup");
        await ready.wait();
        go.set();
        let v = await child;
        Stdout().writeln("direct observed " + v.string());
        v;
    }
}
async main() is int {
    let ready2 = Event();
    let go2 = Event();
    let child2 = spawn Child(go: go2).run();
    let w2 = spawn Worker(ready: ready2, go: go2).direct(child2);
    ready2.set();
    Stdout().writeln("main observed " + (await child2).string());
    w2.cancel();
    let r2 = try { await w2; } catch e is Cancelled { -1; };
    Stdout().writeln("direct " + r2.string() + " cancelled " + w2.cancelled().string());
    0;
}
"#, "main observed 42\ndirect observed 42\ndirect cleanup\ndirect 42 cancelled false\n", 0);
}

/// #601 Q1 audit A1: Events compare as on Tomato, by their source field `initial`; the runtime's words are not
/// compared.
#[test]
fn q1_events_compare_by_their_initial_state() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "async/event" take Event;
main() is int {
    let a = Event();
    let b = Event();
    let c = Event(ready: true);
    Stdout().writeln("same " + (a == a).string());
    Stdout().writeln("fresh " + (a == b).string());
    Stdout().writeln("ready " + (a == c).string());
    0;
}
"#, "same true
fresh true
ready false
", 0);
}

/// #601 Q1 audit A1: two Events, each with a waiting task, compare by `initial` (Tomato), never by their
/// runtime list of waiters.
#[test]
fn q1_events_with_waiters_compare_by_their_initial_state() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "async/event" take Event;
object Waiter {
    has e is Event;
    Waiter(:e);
    async run() is int { await e.wait(); 1; }
}
async main() is int {
    let a = Event();
    let b = Event();
    let wa = spawn Waiter(e: a).run();
    let wb = spawn Waiter(e: b).run();
    Stdout().writeln("same " + (a == a).string());
    Stdout().writeln("other " + (a == b).string());
    a.set();
    b.set();
    await wa;
    await wb;
    Stdout().writeln("after " + (a == b).string());
    0;
}
"#, "same true
other true
after true
", 0);
}

/// #601 Q1 audit A2: an `any` that holds a resource leaves the Event cell plain (its runtime word reaches nothing),
/// and the driver's cache survives Tomato's resource bookkeeping, which never runs on Champ.
#[test]
fn q1_an_event_beside_an_any_that_holds_a_resource() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "async/event" take Event;
object Res {
    Res() {}
    destroy { Stdout().writeln("res closed"); }
}
async main() is int {
    let x is any = Res();
    let e = Event();
    e.set();
    await e.wait();
    Stdout().writeln("waited " + e.ready().string());
    0;
}
"#, "waited true
res closed
", 0);
}

/// PR-F (#775): a synchronous action that calls `check()` through `Cancelling` passes the thrown Cancelled to its
/// caller, which catches it as on Tomato (okkama 0.5.8: `2`, `cancelled`, exit 0). The direct call of `step` and
/// the role call of `check` both lower as ordinary fallible calls.
#[test]
fn q1_a_synchronous_action_passes_a_thrown_cancelled() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "async/cancellation" take Cancellation, Cancelling;
object Worker {
    has token is Cancelling;
    Worker(:token);
    step(n is int) is int {
        token.check();
        n + 1;
    }
}
async main() is int {
    let cancellation = Cancellation();
    let w = Worker(token: cancellation.cancelling());
    Stdout().writeln(w.step(1).string());
    cancellation.cancel();
    let r = try {
        w.step(2).string();
    }
    catch e is Cancelled {
        "cancelled";
    };
    Stdout().writeln(r);
    0;
}
"#, "2\ncancelled\n", 0);
}
