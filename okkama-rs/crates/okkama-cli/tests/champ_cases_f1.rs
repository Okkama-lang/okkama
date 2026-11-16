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

/// PR-D, §4.8 item 1: a fresh resource passed through an async role slot is retained by the cold Future, used by the
/// body and destroyed once, at the awaiting scope; a bound one stays with its binding.
#[test]
#[ignore = "#735: SPB 17 static selection of fresh inputs offered to fut.new (also for direct calls)"]
fn f1_a_fresh_argument_of_an_async_slot_is_retained_by_its_future() {
    expect(r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Res {
    has label is string;
    Res(:label);
    label() is string { label; }
    destroy { Stdout().writeln("destroy " + label); }
}
role Using { async use(r is Res) is int; }
object Impl does Using {
    Impl();
    async use(r is Res) is int { Stdout().writeln("use " + r.label()); r.label().length(); }
}
async main() is int {
    let u is Using = Impl();
    let f = u.use(Res(label: "fresh"));
    Stdout().writeln("created");
    let n = await f;
    Stdout().writeln("got " + n.string());
    let kept = Res(label: "kept");
    let g = u.use(kept);
    Stdout().writeln("awaiting " + (await g).string());
    Stdout().writeln("temp " + (await u.use(Res(label: "temp"))).string());
    0;
}
"#, concat!("created\nuse fresh\ndestroy fresh\ngot 5\nuse kept\nawaiting 4\nuse temp\ndestroy temp\ntemp 4\n",
        "destroy kept\n"));
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
