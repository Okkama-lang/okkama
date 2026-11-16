//! #601 F1: async role slots, bound async action values and variadic role slots on Champ (OIR-2 async ABI, ruling
//! PR-D; `fut.spawn.bound`, ruling PR-E). Conformance cases are checked in full against their case.json; the inline
//! programs' expected output is Tomato's for the same program (okkama 0.5.8, run in the cage).
//! The file runs twice: as is, and with `SPARROW_CORE_GC_STRESS=1`, which collects at every safepoint.
#![cfg(feature = "champ-abi")]

mod champ_support;

use champ_support::{case_dir, run_case, run_source};
use serde_json::Value;
use std::path::Path;
use std::process::Command;

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
    f1_variadic_slot_stderr_basic => "L00.stderr.basic",
    f1_role_shape_async_dispatch => "L08.role-shape.async-dispatch",
}

fn expect(source: &str, stdout: &str) {
    let run = run_source(source).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!((run.stdout.as_str(), run.result), (stdout, Ok(0)));
}

/// PR-D: an async slot of a generic role implemented by a generic object, at two instantiations. A role call creates
/// the cold Future; its body runs only when awaited.
#[test]
fn f1_an_async_slot_of_a_generic_role_returns_a_cold_future() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
role Reading<T> { async next() is Maybe<T>; }
object Counter<T> does Reading<T> {
    has value is T;
    Counter(:value);
    async next() is Maybe<T> { Stdout().writeln("next runs"); Maybe.some(value); }
}
async main() is int {
    let r is Reading<string> = Counter(value: "x");
    let pending = r.next();
    Stdout().writeln("created");
    let got = match await pending { case Maybe.some(v) { v; } case Maybe.none { "none"; } };
    let n is Reading<int> = Counter(value: 4);
    let four = match await n.next() { case Maybe.some(v) { v; } case Maybe.none { 0; } };
    Stdout().writeln("next " + got + " " + four.string());
    0;
}
"#, "created\nnext runs\nnext runs\nnext x 4\n");
}

/// #773: a role call that reaches a std primitive's intrinsic member (LockGuard as Closing) goes through an adapter of
/// the slot's ABI.
#[test]
fn f1_a_role_call_reaches_an_intrinsic_member_through_its_adapter() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "io/stream" take Closing;
import "async/lock" take Lock, LockGuard;
async main() is int {
    let lock = Lock();
    let guard = await lock.guard();
    let c is Closing = guard;
    c.close();
    Stdout().writeln("closed " + c.closed().string() + " locked " + lock.locked().string());
    0;
}
"#, "closed true locked false\n");
}

/// PR-D item 3: a bound async action value, held in a local and in a field, called and awaited; and an async role slot
/// bound as a value. Calling it creates the cold Future.
#[test]
fn f1_a_bound_async_action_creates_its_future_when_called() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Worker {
    has base is int;
    Worker(:base);
    async add(n is int) is int { Stdout().writeln("adding " + n.string()); base + n; }
}
object Runner {
    has action is async (int) is int;
    Runner(:action);
    async run(n is int) is int {
        let f = action(n);
        Stdout().writeln("created");
        await f;
    }
}
async main() is int {
    let w = Worker(base: 10);
    let action is async (int) is int = w.add;
    let direct = await action(1);
    let r = Runner(action: w.add);
    let via = await r.run(5);
    Stdout().writeln(direct.string() + " " + via.string());
    0;
}
"#, "adding 1\ncreated\nadding 5\n11 15\n");
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
role Loading { async load(n is int) is int; }
object Net does Loading {
    Net();
    async load(n is int) is int { Stdout().writeln("load " + n.string()); n * 2; }
}
async main() is int {
    let l is Loading = Net();
    let action is async (int) is int = l.load;
    let f = action(3);
    Stdout().writeln("bound");
    Stdout().writeln((await f).string());
    0;
}
"#, "bound\nload 3\n6\n");
}

/// A case that works with files, run by the okkama binary through OIR-2 in a fresh copy of its directory, so that
/// its files stay out of the repository; checked in full against its case.json.
fn run_in_copy(id: &str) -> Result<(), String> {
    let source = case_dir(id);
    let copy = std::env::temp_dir().join(format!("okkama-f1-{}-{id}", std::process::id()));
    let _ = std::fs::remove_dir_all(&copy);
    std::fs::create_dir_all(&copy).unwrap();
    for entry in std::fs::read_dir(&source).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            std::fs::copy(&path, copy.join(path.file_name().unwrap())).unwrap();
        }
    }
    let case: Value = serde_json::from_str(&std::fs::read_to_string(source.join("case.json")).unwrap()).unwrap();
    let main = case["sources"][0].as_str().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_okkama"))
        .args(["run", main])
        .current_dir(&copy)
        .env("OKKAMA_PIPELINE", "oir2")
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&copy);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let expect = &case["expect"];
    assert_eq!(expect["outcome"], "success", "{id}: only successful cases");
    let status = expect["exit_status"].as_i64().unwrap_or(0) as i32;
    if output.status.code() != Some(status) {
        return Err(format!("exit {:?}, expected {status}; stdout={stdout:?} stderr={stderr:?}", output.status.code()));
    }
    let expected = expect["stdout"]["text"].as_str().unwrap_or("");
    if stdout != expected {
        return Err(format!("stdout {stdout:?}, expected {expected:?}; stderr={stderr:?}"));
    }
    Ok(())
}

/// The variadic slots of `WritingText` (PR-D item 5) reached through std's file and directory objects.
#[test]
fn f1_variadic_slot_file_cases() {
    assert!(Path::new(env!("CARGO_BIN_EXE_okkama")).is_file());
    let failures = ["L10.fs.directory-entries-sorted", "L10.fs.line-reading", "L10.fs.missing-file-message",
        "L10.fs.text-lines-in-batches", "L10.fs.write-then-read"].iter()
        .filter_map(|id| run_in_copy(id).err().map(|error| format!("{id}: {error}")))
        .collect::<Vec<_>>();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Ruling PR-E, #766: `spawn` of a synchronous action that returns a Future, and of a bound async action value's call,
/// start the Future at once, as Tomato's `spawn` of a cold Future.
#[test]
fn f1_a_spawn_of_a_future_not_created_on_the_spot_starts_it() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Job {
    has n is int;
    Job(:n);
    async run() is int { Stdout().writeln("job " + n.string()); n; }
    async add(k is int) is int { Stdout().writeln("add " + k.string()); n + k; }
}
object Starter {
    Starter();
    start(n is int) is Future<int> { Job(n: n).run(); }
}
role Running { async go(n is int) is int; }
object Runner does Running {
    Runner();
    async go(n is int) is int { Stdout().writeln("go " + n.string()); n * 10; }
}
async main() is int {
    let f = spawn Starter().start(4);
    Stdout().writeln("after spawn");
    let r is Running = Runner();
    let g = r.go(2);
    let action is async (int) is int = Job(n: 7).add;
    let h = spawn action(3);
    Stdout().writeln("after value spawn");
    Stdout().writeln("sum " + (await f + await g + await h).string());
    0;
}
"#, "job 4\nafter spawn\nadd 3\nafter value spawn\ngo 2\nsum 34\n");
}

/// Ruling PR-E: a bound-spawned Future whose synchronous owner closes while it waits detaches and still runs; one that
/// fails unobserved is reported at shutdown and the run fails, as on Tomato.
#[test]
fn f1_a_bound_spawned_future_detaches_and_its_unobserved_failure_is_reported() {
    let run = run_source(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "async/event" take Event;
enum Broken is Error = (broken);
object Job {
    has gate is Event;
    Job(:gate);
    async run() is int { await gate.wait(); Stdout().writeln("job ran"); 1; }
    async fail() is int { Stdout().writeln("failing"); throw Broken.broken; }
}
object Starter {
    has gate is Event;
    Starter(:gate);
    start() is Future<int> { Job(gate: gate).run(); }
    failing() is Future<int> { Job(gate: gate).fail(); }
}
object Scope {
    Scope();
    detach(gate is Event) is void {
        let f = spawn Starter(gate: gate).start();
        Stdout().writeln("detached " + f.done().string());
    }
}
async main() is int {
    let gate = Event();
    Scope().detach(gate);
    Stdout().writeln("closed");
    let failing = spawn Starter(gate: gate).failing();
    Stdout().writeln("spawned failing");
    gate.set();
    let waiter = Starter(gate: gate).start();
    Stdout().writeln("waited " + (await waiter).string());
    0;
}
"#).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(run.stdout, "detached false\nclosed\nfailing\nspawned failing\njob ran\nwaited 1\n");
    let report = run.result.unwrap_err();
    assert!(report.starts_with("Broken: Broken.broken"), "{report}");
}

/// #791 W5: a borrowed resource through an async role slot. The creation thunk borrows `kept` into the cold Future,
/// the body uses it when awaited, and `kept` is destroyed once, at the end of its scope (Tomato's output).
#[test]
#[ignore = "#732: a resource argument of an async action fails the SPB 17 static selection check"]
fn f1_a_borrowed_resource_goes_through_an_async_slot() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Res {
    has label is string;
    Res(:label);
    name() is string { label; }
    destroy { Stdout().writeln("destroy " + label); }
}
role User { async use(r is Res) is int; }
object U does User {
    U();
    async use(r is Res) is int { Stdout().writeln("use " + r.name()); r.name().length(); }
}
async main() is int {
    let u is User = U();
    {
        let kept = Res(label: "kept");
        let g = u.use(kept);
        Stdout().writeln("created");
        let n = await g;
        Stdout().writeln("n " + n.string());
    }
    Stdout().writeln("end");
    0;
}
"#, "created\nuse kept\nn 4\ndestroy kept\nend\n");
}

/// #791 W6 (S1): `spawn` of a D22 driver: an Event's `wait` and a Lock's `guard`, created on the spot (the duty form)
/// and through a bound action value (the bound form, PR-E), against Tomato's output.
#[test]
fn f1_a_spawn_of_an_event_wait_and_a_lock_guard() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "async/event" take Event;
async main() is int {
    let open = spawn Event(ready: true).wait();
    Stdout().writeln("open " + (await open).ready().string());
    let gate = Event();
    let wait is async () is Event = gate.wait;
    let waiting = spawn wait();
    Stdout().writeln("spawned " + gate.ready().string());
    gate.set();
    let woken = await waiting;
    Stdout().writeln("woken " + woken.ready().string());
    0;
}
"#, "open true\nspawned false\nwoken true\n");
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "async/lock" take Lock, LockGuard;
async main() is int {
    let fresh = spawn Lock().guard();
    let held = await fresh;
    Stdout().writeln("fresh " + held.closed().string());
    held.close();
    let lock = Lock();
    let first = await lock.guard();
    let guard is async () is LockGuard = lock.guard;
    let second = spawn guard();
    Stdout().writeln("locked " + lock.locked().string());
    first.close();
    let g = await second;
    Stdout().writeln("second " + g.closed().string() + " " + lock.locked().string());
    g.close();
    Stdout().writeln("after " + lock.locked().string());
    0;
}
"#, "fresh false\nlocked true\nsecond false true\nafter false\n");
}
