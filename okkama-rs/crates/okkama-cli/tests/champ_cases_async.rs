//! The W1 cases of the async plan (#601, plan §4 V10): plain await, compiled through OIR-2 and run in Champ in
//! process, each checked in full against its case.json; and R3's async programs with resources that reach Champ.
//! With `SPARROW_CORE_GC_STRESS=1` every safepoint collects.
#![cfg(feature = "champ-abi")]

mod champ_support;

use champ_support::{case_dir, compile, run, run_source, run_source_with};
use okkama_compiler::SourceFile;
use serde_json::Value;

fn full_check(id: &str) -> Result<(), String> {
    let dir = case_dir(id);
    let case: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("case.json")).unwrap()).unwrap();
    assert_eq!(case["mode"], "run", "{id}: only run-mode cases");
    let mut sources = Vec::new();
    for name in case["sources"].as_array().unwrap() {
        let file = std::fs::canonicalize(dir.join(name.as_str().unwrap())).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        sources.push(SourceFile::new(file.to_string_lossy(), text).unwrap());
    }
    let run = run(compile(&sources)?)?;
    let stdout = run.stdout;
    let expect = &case["expect"];
    let expected_stdout = expect["stdout"]["text"].as_str().unwrap_or("");
    match (expect["outcome"].as_str().unwrap(), &run.result) {
        ("success", Ok(code)) => {
            let status = expect["exit_status"].as_i64().unwrap_or(0);
            if *code != status {
                return Err(format!("exit {code}, expected {status}; stdout={stdout:?}"));
            }
        }
        ("success", Err(error)) => return Err(format!("failed: {error}; stdout={stdout:?}")),
        (_, Ok(code)) => return Err(format!("completed with {code}, expected a failure; stdout={stdout:?}")),
        (_, Err(_)) => {}
    }
    if stdout != expected_stdout {
        return Err(format!("stdout {stdout:?}, expected {expected_stdout:?}"));
    }
    Ok(())
}

macro_rules! full_check {
    ($($name:ident => $id:literal,)*) => {
        $(
            #[test]
            fn $name() {
                full_check($id).unwrap_or_else(|error| panic!("{}: {error}", $id));
            }
        )*
    };
}

full_check! {
    w1_await_in_async_main => "L08.await.in-async-main",
    w1_failure_rethrown_at_every_await => "L08.failure.rethrown-at-every-await",
    w1_future_lazy_and_cached => "L08.future.lazy-and-cached",
    w1_future_never_awaited => "L08.future.never-awaited",
    w1_future_passed_and_awaited_elsewhere => "L08.future.passed-and-awaited-elsewhere",
    w1_order_across_suspension => "L08.order.across-suspension",
    w1_order_lazy_branches => "L08.order.lazy-branches",
    w1_void_await_for_effect => "L08.void.await-for-effect",
    k2_future_cancel_unstarted => "L08.future.cancel-unstarted",
    r3_role_restriction_through_async => "L12.role-restriction-through-async",
    b0_future_successor_obligation => "L12.future-successor-obligation",
}

/// `catch e is Cancelled` compiles through OIR-2 and SPB 17 and lets every other failure pass (#724, language spec
/// 9.5): the expected stdout is Tomato's for this program. Raising a cancellation from source needs `fut.cancel`
/// in own (K1, #688); Champ's match of a cancellation is tested in `sparrow-core` (`champ_k2_*`).
#[test]
fn k2_catch_cancelled_compiles_and_lets_other_failures_pass() {
    let run = run_source(concat!(
        "module app v1.0.0;\n",
        "import \"io/stdout\" take Stdout;\n",
        "enum Broken is Error = (broken);\n",
        "object Job {\n",
        "    has result is int;\n",
        "    Job(:result);\n",
        "    async run() is int { result; }\n",
        "    fails() is int { throw Broken.broken; }\n",
        "}\n",
        "async main() is int {\n",
        "    let out = Stdout();\n",
        "    let r = try { await Job(result: 7).run(); } catch e is Cancelled { out.writeln(e.text()); -1; };\n",
        "    out.writeln(r.string());\n",
        "    let s = try {\n",
        "        try { Job(result: 1).fails(); }\n",
        "        catch e is Cancelled { out.writeln(\"cancelled \" + e.text()); -1; };\n",
        "    } catch e is Broken {\n",
        "        -2;\n",
        "    };\n",
        "    out.writeln(s.string());\n",
        "    0;\n",
        "}\n",
    )).unwrap();
    assert_eq!((run.stdout.as_str(), run.result), ("7\n-2\n", Ok(0)));
}

/// `catch e is Error` takes a cancellation and binds Cancelled, whose `text()` is "Operation was cancelled" (#738,
/// language spec 9.5): the expected stdout is Tomato's for this program (okkama 0.5.8 in the cage). The cancelled
/// body never runs; a catch of Error around an await that completes takes nothing.
#[test]
fn k2_catch_error_takes_a_cancellation() {
    let run = run_source(concat!(
        "module app v1.0.0;\n",
        "import \"io/stdout\" take Stdout;\n",
        "object Job {\n",
        "    has result is int;\n",
        "    Job(:result);\n",
        "    async run() is int {\n",
        "        Stdout().writeln(\"running\");\n",
        "        result;\n",
        "    }\n",
        "}\n",
        "async main() is int {\n",
        "    let out = Stdout();\n",
        "    let pending = Job(result: 1).run();\n",
        "    pending.cancel();\n",
        "    let text = try {\n",
        "        (await pending).string();\n",
        "    } catch e is Error {\n",
        "        \"error: \" + e.text();\n",
        "    };\n",
        "    out.writeln(text);\n",
        "    let done = try { (await Job(result: 2).run()).string(); } catch e is Error { e.text(); };\n",
        "    out.writeln(done);\n",
        "    0;\n",
        "}\n",
    )).unwrap();
    assert_eq!((run.stdout.as_str(), run.result), ("error: Operation was cancelled\nrunning\n2\n", Ok(0)));
}

/// A resource that prints its destroy, so the run shows when each one closes.
const LEASE: &str = concat!(
    "module app; import \"io/stdout\" take Stdout; ",
    "object Lease { has name is string; Lease(name is string) { self.name = name; } label() is string { name; } ",
    "destroy { Stdout().writeln(\"destroy \" + name); } } ",
);

fn lease_run(rest: &str) -> String {
    let run = run_source(&format!("{LEASE}{rest}")).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(run.result, Ok(0), "stdout={:?}", run.stdout);
    run.stdout
}

/// OIR-2 §10.2: the cache owns the resource result; both awaits see it, the body's local closes before the
/// result is published, and the Future's owner destroys the result once (corpus `async-results` checks 2–4).
#[test]
fn r3_resource_result_is_cached_and_destroyed_once() {
    let stdout = lease_run(concat!(
        "object Work { Work(); async make() is Lease { let local = Lease(\"L\"); Lease(\"R\"); } } ",
        "async main() is int { { let f = Work().make(); let r = await f; Stdout().writeln(\"first \" + r.label()); ",
        "let again = await f; Stdout().writeln(\"again \" + again.label()); } Stdout().writeln(\"end\"); 0; }",
    ));
    assert_eq!(stdout, "destroy L\nfirst R\nagain R\ndestroy R\nend\n");
}

/// #732: an async action with a resource receiver and a resource argument. Both are bound, so the split that would
/// take them into the cold Future's inputs selects nothing (an empty packet: they are lent), and its static
/// relocation is empty (EVIDENCE_SELECTIONS). Tomato (okkama 0.5.8) prints the same.
#[test]
fn r3_a_resource_receiver_and_argument_are_lent_to_an_async_action() {
    let stdout = lease_run(concat!(
        "object Reader { has name is string; Reader(name is string) { self.name = name; } ",
        "async read(arg is Lease) is string { name + arg.label(); } destroy { Stdout().writeln(\"destroy \" + name); } } ",
        "async main() is int { { let r = Reader(\"R\"); let a = Lease(\"A\"); let f = r.read(a); ",
        "Stdout().writeln(\"created\"); let s = await f; Stdout().writeln(\"awaited \" + s); } ",
        "Stdout().writeln(\"end\"); 0; }",
    ));
    assert_eq!(stdout, "created\nawaited RA\ndestroy A\ndestroy R\nend\n");
}

/// #732 (#791 W5): a bound resource borrowed by an awaited async action, under the Full ownership verifier too;
/// the resource is destroyed once, at the end of its scope (Tomato's output).
#[test]
fn r3_a_bound_resource_is_lent_to_an_awaited_async_action() {
    for mode in [okkama_compiler::oir2::VerifyMode::Structural, okkama_compiler::oir2::VerifyMode::Full] {
        let run = run_source_with(r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Res { has label is string; Res(:label); name() is string { label; } destroy { Stdout().writeln("destroy " + label); } }
object U { U(); async use(r is Res) is int { r.name().length(); } }
async main() is int { let kept = Res(label: "kept"); let n = await U().use(kept); Stdout().writeln("n " + n.string()); 0; }
"#, mode).unwrap_or_else(|error| panic!("{mode:?}: {error}"));
        assert_eq!((run.stdout.as_str(), run.result), ("n 4\ndestroy kept\n", Ok(0)), "{mode:?}");
    }
}

/// #742: resources that keep the caller's Atomic (a borrowed capability, no obligation) as a receiver and an argument
/// of an async action. The Atomic field reaches no resource, so a static selection of the resource stays a
/// singleton. Tomato prints `RA:` then `AR`.
#[test]
fn r3_resources_holding_a_borrowed_atomic_are_lent_to_an_async_action() {
    for mode in [okkama_compiler::oir2::VerifyMode::Structural, okkama_compiler::oir2::VerifyMode::Full] {
        let run = run_source_with(r#"module app v1.0.0;
import "async/atomic" take Atomic;
import "io/stdout" take Stdout;
object Res {
    has name is string;
    has log is Atomic<string>;
    Res(name is string, log is Atomic<string>) { self.name = name; self.log = log; }
    label() is string { name; }
    destroy { log.replace(log.value() + name); }
}
object Receiver {
    has name is string;
    has log is Atomic<string>;
    Receiver(name is string, log is Atomic<string>) { self.name = name; self.log = log; }
    async read(arg is Res) is string { name + arg.label(); }
    destroy { log.replace(log.value() + name); }
}
async main() is int {
    let log = Atomic("");
    defer log.close();
    {
        let r = Receiver("R", log);
        let a = Res("A", log);
        let f = r.read(a);
        let s = await f;
        Stdout().writeln(s + ":" + log.value());
    }
    Stdout().writeln(log.value());
    0;
}
"#, mode).unwrap_or_else(|error| panic!("{mode:?}: {error}"));
        assert_eq!((run.stdout.as_str(), run.result), ("RA:\nAR\n", Ok(0)), "{mode:?}");
    }
}

/// OIR-2 §§10.2–10.3: an awaited temporary's fresh result belongs to the awaiting scope, a discarded one too
/// (corpus `async-results` checks 5–6); a temporary inside an expression closes with its statement.
#[test]
fn r3_awaited_temporary_result_closes_with_its_awaiting_scope() {
    let stdout = lease_run(concat!(
        "object Work { Work(); async make(n is string) is Lease { Lease(n); } } ",
        "async main() is int { { await Work().make(\"T\"); Stdout().writeln(\"in block\"); ",
        "let kept = (await Work().make(\"K\")).label(); Stdout().writeln(\"kept \" + kept); } ",
        "Stdout().writeln(\"end\"); 0; }",
    ));
    assert_eq!(stdout, "in block\ndestroy K\nkept K\ndestroy T\nend\n");
}

/// #601 D25, OIR-2 §10.1 `make()`: a synchronous action returns a cold Future through `cache.rehome ... target
/// caller`; the caller's scope admits it with its entry, awaits it, and settles it dynamically at its close, where
/// the cached outcome Q is destroyed once (OIR-2 §10.2: the cache owns the outcome until its owner closes).
#[test]
fn d25_a_returned_future_is_settled_by_the_scope_that_admits_it() {
    let source = r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Res {
    has label is string;
    Res(:label);
    show() is string { label; }
    destroy { Stdout().writeln("destroy " + label); }
}
object Work {
    Work();
    async run() is Res {
        Stdout().writeln("running");
        Res(label: "Q");
    }
}
object Maker {
    Maker();
    make() is Future<Res> {
        Work().run();
    }
}
async main() is int {
    let f = Maker().make();
    Stdout().writeln("made");
    let r = await f;
    Stdout().writeln("got " + r.show());
    0;
}
"#;
    let run = champ_support::run_source(source).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(run.result, Ok(0), "{}", run.stdout);
    assert_eq!(run.stdout, "made\nrunning\ngot Q\ndestroy Q\n");
}

/// #741 (ruling PR-M): a `try` with a payload `catch` in an async body over a throwing constructor and a total
/// action. Codegen lowers the action as total and drops the unreachable unwind handler with the async body's frame
/// layout; the loader stays strict. The expected stdout is Tomato's for the same programs (okkama 0.5.8).
#[test]
fn b0_payload_catch_in_async_main_over_a_total_action() {
    let source = r#"module app v1.0.0;
import "io/stdout" take Stdout;
enum Invalid is Error = (count(count is int));
object Thrower {
    has amount is int;
    Thrower(n is int) { confirm n > 0 else throw Invalid.count(n); amount = n; }
    value() is int { amount; }
}
async main() is int {
    let bad = try { Thrower(0).value(); } catch Invalid.count(p) { p - 1; };
    Stdout().writeln("bad " + bad.string());
    0;
}
"#;
    let run = run_source(source).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!((run.stdout.as_str(), run.result), ("bad -1\n", Ok(0)));
    let source = r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "async/semaphore" take Semaphore, InvalidPermitCount;
async main() is int {
    let n = try { Semaphore(permits: 0 - 1).permits(); } catch InvalidPermitCount.permits(p) { p * 10; };
    Stdout().writeln("permits " + n.string());
    0;
}
"#;
    let run = run_source(source).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!((run.stdout.as_str(), run.result), ("permits -10\n", Ok(0)));
}

/// #747: a generic async action, awaited at two instantiations. Tomato prints `5s` (okkama 0.5.8).
#[test]
fn b0_generic_async_action() {
    let source = r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Box<T> {
    has v is T;
    Box(:v);
    async get() is T { v; }
}
async main() is int {
    let b = Box(v: 5);
    let x = await b.get();
    let s = await Box(v: "s").get();
    Stdout().writeln(x.string() + s);
    0;
}
"#;
    let run = run_source(source).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!((run.stdout.as_str(), run.result), ("5s\n", Ok(0)));
}

/// PR-B1 (#784): a binding marks a Future bound, so an aggregate that stores it only borrows it. `f` is created
/// here and bound (`fut.new … binding`); `g` is admitted with `make`'s result packet and bound by `own.bind`. Neither
/// `Wrapper` takes the duty into `helper`'s packet, so `helper`'s return settles nothing and both Futures run when
/// `main` awaits them. Tomato's output (okkama 0.5.8, the program of #784).
#[test]
fn pr_b1_a_bound_future_stored_in_an_argument_stays_with_its_binding() {
    let source = r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Child {
    has n is int;
    Child(:n);
    async run() is int { Stdout().writeln("child " + n.string()); n; }
}
object Wrapper {
    has pending is Future<int>;
    Wrapper(:pending);
}
object Helper {
    Helper();
    take(w is Wrapper) is int { 1; }
}
object Maker {
    Maker();
    make(n is int) is Future<int> { Child(n: n).run(); }
}
async main() is int {
    let f = Child(n: 4).run();
    Stdout().writeln("helper " + Helper().take(Wrapper(pending: f)).string());
    Stdout().writeln((await f).string());
    let g = Maker().make(9);
    Stdout().writeln("helper " + Helper().take(Wrapper(pending: g)).string());
    Stdout().writeln((await g).string());
    0;
}
"#;
    let run = run_source(source).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(run.result, Ok(0), "{}", run.stdout);
    assert_eq!(run.stdout, "helper 1\nchild 4\n4\nhelper 1\nchild 9\n9\n");
}

/// PR-B1 (#784), review of D25 (F1): an alias of `f` in a nested scope does not unbind `f`. High passes `f` into the
/// branch through a block parameter; after the branch closes `f` is still bound in `main`, so `Wrapper` borrows it,
/// `helper`'s return settles nothing, and `f` runs when `main` awaits it. Tomato's output (okkama 0.5.8).
#[test]
fn pr_b1_an_alias_in_a_nested_scope_keeps_a_future_bound() {
    let source = r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Child {
    has n is int;
    Child(:n);
    async run() is int { Stdout().writeln("child " + n.string()); n; }
}
object Wrapper {
    has pending is Future<int>;
    Wrapper(:pending);
}
object Helper {
    Helper();
    take(w is Wrapper) is int { 1; }
}
async main() is int {
    let f = Child(n: 4).run();
    if true { let g = f; }
    Stdout().writeln("helper " + Helper().take(Wrapper(pending: f)).string());
    Stdout().writeln((await f).string());
    0;
}
"#;
    let run = run_source(source).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(run.result, Ok(0), "{}", run.stdout);
    assert_eq!(run.stdout, "helper 1\nchild 4\n4\n");
}

/// F1 with retained nested scopes (a resource lives there): the alias of a created `f` and of an admitted `h` is
/// stored and passed inside the branch, then the original outside it. Every `Wrapper` borrows; each Future runs once,
/// when `main` awaits it, after the branch's resource is destroyed. Tomato's output (okkama 0.5.8).
#[test]
fn pr_b1_aliases_in_retained_nested_scopes_borrow() {
    let source = r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Res {
    has name is string;
    Res(:name);
    destroy { Stdout().writeln("destroy " + name); }
}
object Child {
    has n is int;
    Child(:n);
    async run() is int { Stdout().writeln("child " + n.string()); n; }
}
object Wrapper {
    has pending is Future<int>;
    Wrapper(:pending);
}
object Helper {
    Helper();
    take(w is Wrapper) is int { 1; }
}
object Maker {
    Maker();
    make(n is int) is Future<int> { Child(n: n).run(); }
}
async main() is int {
    let f = Child(n: 4).run();
    if true {
        let r = Res(name: "r");
        let g = f;
        Stdout().writeln("inner " + Helper().take(Wrapper(pending: g)).string());
    }
    Stdout().writeln("helper " + Helper().take(Wrapper(pending: f)).string());
    Stdout().writeln((await f).string());
    let h = Maker().make(9);
    if true {
        let r = Res(name: "s");
        let k = h;
        Stdout().writeln("inner " + Helper().take(Wrapper(pending: k)).string());
    }
    Stdout().writeln("helper " + Helper().take(Wrapper(pending: h)).string());
    Stdout().writeln((await h).string());
    0;
}
"#;
    let run = run_source(source).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(run.result, Ok(0), "{}", run.stdout);
    assert_eq!(run.stdout,
        "inner 1\ndestroy r\nhelper 1\nchild 4\n4\ninner 1\ndestroy s\nhelper 1\nchild 9\n9\n");
}

/// PR-B1 (#784), §2.2a runtime row: `pass` binds an admitted Future (`own.bind` moves its entry to the bound
/// journal) and returns it, so the state form of `cache.rehome` takes the entry from the bound journal; the Future
/// runs once when `main` awaits it.
#[test]
fn pr_b1_a_bound_admitted_future_is_rehomed_to_the_caller() {
    let source = r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Child {
    has n is int;
    Child(:n);
    async run() is int { Stdout().writeln("child " + n.string()); n; }
}
object Maker {
    Maker();
    make(n is int) is Future<int> { Child(n: n).run(); }
    pass(n is int) is Future<int> {
        let g = Maker().make(n);
        g;
    }
}
async main() is int {
    let h = Maker().pass(9);
    Stdout().writeln("passed");
    Stdout().writeln((await h).string());
    0;
}
"#;
    let run = run_source(source).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(run.result, Ok(0), "{}", run.stdout);
    assert_eq!(run.stdout, "passed\nchild 9\n9\n");
}

/// #776: a cold Future that reads a Future created beside it in a block leaves the block with that Future's duty:
/// both run after the block, as on Tomato (okkama 0.5.8: `escaped`, `seed`, `read 1 2`, `3`).
#[test]
fn a_future_leaves_its_block_with_the_future_it_reads() {
    let source = r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Res {
    has name is string;
    Res(:name);
    label() is string { name; }
    destroy { Stdout().writeln("destroy " + name); }
}
object Seed {
    Seed();
    async run() is int { Stdout().writeln("seed"); 2; }
}
object Reader {
    Reader();
    async read(r is int, e is Future<int>) is int {
        let n = await e;
        Stdout().writeln("read " + r.string() + " " + n.string());
        n + 1;
    }
}
async main() is int {
    let f = if true {
        let r = 1;
        let e = Seed().run();
        Reader().read(r, e);
    } else {
        Reader().read(5, Seed().run());
    };
    Stdout().writeln("escaped");
    Stdout().writeln((await f).string());
    0;
}
"#;
    let run = run_source(source).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(run.result, Ok(0), "{}", run.stdout);
    assert_eq!(run.stdout, "escaped\nseed\nread 1 2\n3\n");
}

/// #759: Futures created in a `for` body with `next` before and `last` after the creation; each iteration awaits
/// or abandons its own (okkama 0.5.8: `job 1`, `got 1`, `job 3`, `got 3`, `main`).
#[test]
fn a_loop_body_future_with_next_and_last() {
    let source = r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Job {
    has n is int;
    Job(:n);
    async run() is int { Stdout().writeln("job " + n.string()); n; }
}
async main() is int {
    for 1..=4 -> i {
        next if i == 2;
        let f = Job(n: i).run();
        last if i == 4;
        Stdout().writeln("got " + (await f).string());
    }
    Stdout().writeln("main");
    0;
}
"#;
    let run = run_source(source).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(run.result, Ok(0), "{}", run.stdout);
    assert_eq!(run.stdout, "job 1\ngot 1\njob 3\ngot 3\nmain\n");
}

/// #849: Futures in the body of a `map`, `reduce` or `filter`, whose builder lives across the body's awaits. The
/// outputs are Tomato's, built from the rebuild tree. Bodies whose Future has a destroyable outcome wait for #851.
const MAP_BODY: &str = r#"module app v1.0.0;

import "io/stdout" take Stdout;

object Res {
    has name is string;
    Res(:name);
    label() is string { name; }
    destroy { Stdout().writeln("destroy " + name); }
}

object Now {
    Now();
    async named(name is string) is string { name; }
    async res(name is string) is Res {
        Stdout().writeln("make " + name);
        Res(name: name);
    }
}

enum Broken is Error = (broken);
"#;

/// Under Structural verification and, when `full`, under Full too.
fn map_body(main: &str, stdout: &str, full: bool) {
    let source = format!("{MAP_BODY}\n{main}");
    let modes: &[okkama_compiler::oir2::VerifyMode] = if full {
        &[okkama_compiler::oir2::VerifyMode::Structural, okkama_compiler::oir2::VerifyMode::Full]
    } else {
        &[okkama_compiler::oir2::VerifyMode::Structural]
    };
    for &mode in modes {
        let run = champ_support::run_source_with(&source, mode).unwrap_or_else(|error| panic!("{mode:?}: {error}"));
        assert_eq!((run.stdout.as_str(), run.result), (stdout, Ok(0)), "{mode:?}");
    }
}

/// A Future created and awaited in a `map` body (#849): each round's duty is settled by its own close.
#[test]
fn a_map_body_awaits_a_fresh_future_each_round() {
    map_body(r#"async main() is int {
    let names = (0..3).map() -> index { await Now().named("x" + index.string()); };
    Stdout().writeln(names.size().string() + ": " + names[0] + " " + names[1] + " " + names[2]);
    0;
}
"#, "3: x0 x1 x2\n", true);
}

/// A `reduce` whose accumulator lives in a builder (#831) and whose block awaits.
#[test]
fn a_builder_reduce_awaits_in_its_block() {
    map_body(r#"async main() is int {
    let all is @string = (0..3).reduce([]) -> acc, index { acc + [await Now().named("p" + index.string())]; };
    Stdout().writeln(all.size().string() + ": " + all[0] + all[1] + all[2]);
    0;
}
"#, "3: p0p1p2\n", true);
}

/// A `filter` whose predicate awaits.
/// Structural only: Full verification runs out of its budget on this body (Incomplete, #634).
#[test]
fn a_filter_awaits_in_its_predicate() {
    map_body(r#"async main() is int {
    let kept = (0..4).filter() -> index { (await Now().named(index.string())) != "2"; };
    Stdout().writeln("kept " + kept.size().string());
    0;
}
"#, "kept 3\n", false);
}

/// Nested `map`s, the inner body awaiting: two builders live across the await.
/// Structural only: Full verification runs out of its budget on this body (Incomplete, #634).
#[test]
fn nested_maps_await_in_the_inner_body() {
    map_body(r#"async main() is int {
    let grid = (0..2).map() -> row { (0..2).map() -> column { await Now().named(row.string() + column.string()); }; };
    Stdout().writeln(grid[0][0] + " " + grid[0][1] + " " + grid[1][0] + " " + grid[1][1]);
    0;
}
"#, "00 01 10 11\n", false);
}

/// #851: a Future whose outcome has a destroy, awaited in a `for` body: each outcome is destroyed once, right after
/// the body uses it, and main's close, which settles nothing, releases nothing.
#[test]
fn a_for_body_awaits_a_destroyable_outcome() {
    map_body(r#"async main() is int {
    for ["a", "b"] -> name { Stdout().writeln((await Now().res(name)).label() + "!"); }
    0;
}
"#, "make a\ndestroy a\na!\nmake b\ndestroy b\nb!\n", true);
}

/// #851: the same Future never awaited in a `for` body: it never runs, so nothing is made or destroyed.
#[test]
fn a_for_body_leaves_a_destroyable_future_unawaited() {
    map_body(r#"async main() is int {
    for 0..2 -> index {
        let pending = Now().res("r" + index.string());
        Stdout().writeln("round " + index.string());
    }
    0;
}
"#, "round 0\nround 1\n", true);
}

/// #805: a `for` loop awaits a Future created outside it. Coro carries the cache duty round the loop as a block
/// parameter, and Low settles it once at the owner's close. Tomato (okkama 0.5.8) prints the same.
#[test]
fn a_loop_awaits_a_future_created_outside_it() {
    let source = r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Job {
    has n is int;
    Job(:n);
    async run() is int { Stdout().writeln("job " + n.string()); n; }
}
async main() is int {
    let a = Job(n: 1).run();
    for 1..=2 -> k {
        Stdout().writeln("a " + (await a).string());
    }
    Stdout().writeln("main");
    0;
}
"#;
    for mode in [okkama_compiler::oir2::VerifyMode::Structural, okkama_compiler::oir2::VerifyMode::Full] {
        let run = run_source_with(source, mode).unwrap_or_else(|error| panic!("{mode:?}: {error}"));
        assert_eq!(run.result, Ok(0), "{mode:?}: stdout={:?}", run.stdout);
        assert_eq!(run.stdout, "job 1\na 1\na 1\nmain\n", "{mode:?}");
    }
}

/// Duty slots, case 3 of the memo (#858): Okkama has no `while`, so a loop's condition is evaluated inside its body
/// (`loop { last if !(await …); … }`), and the Future it creates belongs to the body's scope, created and settled
/// once per iteration. Tomato (tree build) prints the same.
#[test]
fn a_loop_awaits_in_its_exit_condition() {
    let source = r#"module app v1.0.0;
import "io/stdout" take Stdout;
import "async/atomic" take Atomic;
object Src {
    has left is Atomic<int>;
    Src() { left = Atomic(3); }
    async more() is bool { let v = left.value(); left.replace(v - 1); v > 0; }
}
async main() is int {
    let s = Src();
    loop { last if !(await s.more()); Stdout().writeln("tick"); }
    Stdout().writeln("done");
    0;
}
"#;
    for mode in [okkama_compiler::oir2::VerifyMode::Structural, okkama_compiler::oir2::VerifyMode::Full] {
        let run = run_source_with(source, mode).unwrap_or_else(|error| panic!("{mode:?}: {error}"));
        assert_eq!(run.result, Ok(0), "{mode:?}: stdout={:?}", run.stdout);
        assert_eq!(run.stdout, "tick\ntick\ntick\ndone\n", "{mode:?}");
    }
}

fn both_modes(source: &str, stdout: &str) {
    for mode in [okkama_compiler::oir2::VerifyMode::Structural, okkama_compiler::oir2::VerifyMode::Full] {
        let run = run_source_with(source, mode).unwrap_or_else(|error| panic!("{mode:?}: {error}"));
        assert_eq!(run.result, Ok(0), "{mode:?}: stdout={:?}", run.stdout);
        assert_eq!(run.stdout, stdout, "{mode:?}");
    }
}

/// #860 (ruling PR-N2): a call through an async role slot is a creation. The slot's creation thunk passes its entry
/// packet to `fut.new` as the inputs packet, so the fresh resource argument reaches the body, which destroys it when
/// it ends, as Tomato (tree build) prints.
#[test]
fn a_fresh_resource_through_an_async_role_slot() {
    both_modes(r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Res {
    has name is string;
    Res(:name);
    label() is string { name; }
    destroy { Stdout().writeln("destroy " + name); }
}
role Feeding { async feed(r is Res) is int; }
object Feeder does Feeding {
    Feeder();
    async feed(r is Res) is int { Stdout().writeln("feed " + r.label()); 1; }
}
async main() is int {
    let f is Feeding = Feeder();
    let n = await f.feed(Res(name: "fresh"));
    Stdout().writeln("fed " + n.string());
    Stdout().writeln("main end");
    0;
}
"#, "feed fresh\ndestroy fresh\nfed 1\nmain end\n");
}

/// #860 (ruling PR-N2): a fresh Future argument through an async role slot is borrowed, as on a direct call
/// (PR-B2, OIR-2 §10.2): its cache stays with main's root and its outcome is destroyed at main's close, through the
/// role and directly alike. Tomato destroys each right after its call; the owner kept PR-B2's order (#861).
#[test]
fn a_fresh_future_through_an_async_role_slot_is_borrowed_as_on_a_direct_call() {
    both_modes(r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Res {
    has name is string;
    Res(:name);
    label() is string { name; }
    destroy { Stdout().writeln("destroy " + name); }
}
object Maker {
    Maker();
    async make(name is string) is Res { Stdout().writeln("make " + name); Res(name: name); }
}
role Taking { async take(f is Future<Res>) is string; }
object Taker does Taking {
    Taker();
    async take(f is Future<Res>) is string { (await f).label(); }
}
async main() is int {
    let t is Taking = Taker();
    Stdout().writeln("took " + (await t.take(Maker().make("via role"))));
    Stdout().writeln("took " + (await Taker().take(Maker().make("direct"))));
    Stdout().writeln("main end");
    0;
}
"#, "make via role\ntook via role\nmake direct\ntook direct\nmain end\n\
        destroy direct\ndestroy via role\n");
}

/// #863 (ruling PR-N2, third amendment): two fresh resources through an async role slot. The creation thunk depends
/// on the union of their deps (`deps.join`) and passes its entry packet whole to `fut.new`; the callee's body
/// destroys both. Tomato's output.
#[test]
fn two_fresh_resources_through_an_async_role_slot() {
    both_modes(r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Res {
    has name is string;
    Res(:name);
    label() is string { name; }
    destroy { Stdout().writeln("destroy " + name); }
}
role Pairing { async pair(a is Res, b is Res, n is int) is string; }
object T does Pairing {
    T();
    async pair(a is Res, b is Res, n is int) is string {
        Stdout().writeln("pair " + a.label() + " " + b.label());
        a.label() + n.string();
    }
}
async main() is int {
    let p is Pairing = T();
    Stdout().writeln("got " + (await p.pair(Res(name: "one"), Res(name: "two"), 3)));
    Stdout().writeln("main end");
    0;
}
"#, "pair one two\ndestroy two\ndestroy one\ngot one3\nmain end\n");
}

/// #862 (OIR-2-async-remaining §6.3c): a synchronous function returns a cold Future over its parameter. The
/// parameter is a binding of `start`, never moved into the Future: the Future borrows it, and it leaves beside the
/// Future in the result packet, so the caller destroys it at its own close. Tomato's output.
#[test]
fn a_returned_future_over_a_parameter_takes_the_parameter_back_beside_it() {
    both_modes(r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Res {
    has name is string;
    Res(:name);
    label() is string { name; }
    destroy { Stdout().writeln("destroy " + name); }
}
object Feeder { Feeder(); async feed(r is Res) is int { Stdout().writeln("feed " + r.label()); 1; } }
object Starter { Starter(); start(r is Res) is Future<int> { Feeder().feed(r); } }
async main() is int {
    let f = Starter().start(Res(name: "via start"));
    Stdout().writeln("fed " + (await f).string());
    Stdout().writeln("main end");
    0;
}
"#, "feed via start\nfed 1\nmain end\ndestroy via start\n");
}

/// #862 (§6.3c): two Futures over one parameter, both returned and both awaited. The resource is read twice and
/// destroyed once, at the caller's close: a Future that owned it would destroy it at the end of its body.
#[test]
fn two_returned_futures_over_one_parameter_read_it_and_the_caller_destroys_it() {
    both_modes(r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Res {
    has name is string;
    Res(:name);
    label() is string { name; }
    destroy { Stdout().writeln("destroy " + name); }
}
object Feeder { Feeder(); async feed(r is Res) is int { Stdout().writeln("feed " + r.label()); 1; } }
object Starter { Starter(); start(r is Res) is Future<int> { Feeder().feed(r); } }
async main() is int {
    let r = Res(name: "shared");
    let f = Starter().start(r);
    let g = Starter().start(r);
    Stdout().writeln("got " + (await f + await g).string());
    Stdout().writeln("main end");
    0;
}
"#, "feed shared\nfeed shared\ngot 2\nmain end\ndestroy shared\n");
}

/// #862, ruling (b) of §6.3c: a direct creation over a fresh resource after a call that returned a Future (its duty
/// came back into main). `create_future` offers the fresh resource and takes `deps.of` it, without an `own.split`
/// whose static inventory the loader could not prove beside the returned duty. Tomato's output.
#[test]
fn a_direct_creation_after_a_returned_future_offers_its_fresh_input() {
    both_modes(r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Res {
    has name is string;
    Res(:name);
    label() is string { name; }
    destroy { Stdout().writeln("destroy " + name); }
}
object Feeder {
    Feeder();
    async feed(r is Res) is int { Stdout().writeln("feed " + r.label()); 1; }
}
object Starter { Starter(); start(r is Res) is Future<int> { Feeder().feed(r); } }
async main() is int {
    let f = Starter().start(Res(name: "via start"));
    Stdout().writeln("fed " + (await f).string());
    let direct = await Feeder().feed(Res(name: "direct"));
    Stdout().writeln("main end");
    0;
}
"#, "feed via start\nfed 1\nfeed direct\ndestroy direct\nmain end\ndestroy via start\n");
}

/// #860 case 1 with #862: an async role slot called with a fresh resource, then a direct creation over another in
/// the same main. Tomato's output.
#[test]
fn a_direct_creation_after_an_async_role_call_offers_its_fresh_input() {
    both_modes(r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Res {
    has name is string;
    Res(:name);
    label() is string { name; }
    destroy { Stdout().writeln("destroy " + name); }
}
role Feeding { async feed(r is Res) is int; }
object Feeder does Feeding {
    Feeder();
    async feed(r is Res) is int { Stdout().writeln("feed " + r.label()); 1; }
}
async main() is int {
    let f is Feeding = Feeder();
    let n = await f.feed(Res(name: "fresh"));
    Stdout().writeln("fed " + n.string());
    let direct = await Feeder().feed(Res(name: "direct"));
    Stdout().writeln("main end");
    0;
}
"#, "feed fresh\ndestroy fresh\nfed 1\nfeed direct\ndestroy direct\nmain end\n");
}

/// #869 (Fable, ruling (ii)): an async body that awaits and then returns a managed value. Its final close delivers a
/// deferred cancellation, so low runs a second plan; the result is not carried through that close as an SSA value
/// (its packet is in the pending exit), so no root of a plan binds it. Tomato's output.
#[test]
fn an_async_body_that_awaits_returns_a_resource() {
    both_modes(r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Res {
    has name is string;
    Res(:name);
    label() is string { name; }
    destroy { Stdout().writeln("destroy " + name); }
}
object Waiter { Waiter(); async wait() is int { 1; } }
object Relay { Relay(); async relay() is Res { let n = await Waiter().wait(); Res(name: "made " + n.string()); } }
async main() is int {
    let r = await Relay().relay();
    Stdout().writeln("got " + r.label());
    Stdout().writeln("main end");
    0;
}
"#, "got made 1\nmain end\ndestroy made 1\n");
}

/// #869: a function returns a resource while its close destroys a local one whose destroy may fail. A failure there
/// aborts the reserve into the plan; the returned value carried through the close would then be a root of a destroyed
/// identity. Synchronous and async alike; Tomato's output.
#[test]
fn a_returned_resource_beside_a_destroyed_local() {
    both_modes(r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Res {
    has name is string;
    Res(:name);
    label() is string { name; }
    destroy { Stdout().writeln("destroy " + name); }
}
object M {
    M();
    make() is Res { let a = Res(name: "local"); Stdout().writeln("have " + a.label()); Res(name: "made"); }
}
object A {
    A();
    async make() is Res { let a = Res(name: "alocal"); Stdout().writeln("have " + a.label()); Res(name: "amade"); }
}
async main() is int {
    let r = M().make();
    Stdout().writeln("got " + r.label());
    let q = await A().make();
    Stdout().writeln("got " + q.label());
    Stdout().writeln("main end");
    0;
}
"#, "have local\ndestroy local\ngot made\nhave alocal\ndestroy alocal\ngot amade\nmain end\n\
    destroy amade\ndestroy made\n");
}

/// #867 (Fable): an outcome taken from a temporary Future is owned by the awaiting scope, not a view of the cache, so
/// it leaves the function without a rehome and its unique-cache proof holds. It does not inherit what the Future
/// owned (the fresh input, destroyed by the body). Both forms, the await as the result and through a binding; Tomato's
/// output.
#[test]
fn a_taken_outcome_leaves_its_function_owned() {
    for relay in [
        "async relay() is Res { await Wrapper().wrap(Res(name: \"in\")); }",
        "async relay() is Res { let r = await Wrapper().wrap(Res(name: \"in\")); r; }",
    ] {
        both_modes(&format!(r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Res {{
    has name is string;
    Res(:name);
    label() is string {{ name; }}
    destroy {{ Stdout().writeln("destroy " + name); }}
}}
object Wrapper {{ Wrapper(); async wrap(r is Res) is Res {{ Res(name: "wrapped " + r.label()); }} }}
object Relay {{ Relay(); {relay} }}
async main() is int {{
    let r = await Relay().relay();
    Stdout().writeln("got " + r.label());
    Stdout().writeln("main end");
    0;
}}
"#), "destroy in\ngot wrapped in\nmain end\ndestroy wrapped in\n");
    }
}

/// #865 (OIR-2-async-remaining §6.3d): a synchronous function returns a cold Future over its local binding. The
/// binding stays beside the Future in the result packet, declared `beside` in the relation with its type; the caller
/// admits it and destroys it at its own close. Tomato's output.
#[test]
fn a_returned_future_over_a_local_takes_the_local_back_beside_it() {
    both_modes(r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Res {
    has l is string;
    Res(:l);
    n() is int { l.length(); }
    label() is string { l; }
    destroy { Stdout().writeln("destroy " + l); }
}
object U { U(); async use(r is Res) is int { Stdout().writeln("use " + r.label()); r.n(); } }
object Mk { Mk(); make() is Future<int> { let r = Res(l: "local"); U().use(r); } }
async main() is int {
    let f = Mk().make();
    Stdout().writeln("made");
    Stdout().writeln("got " + (await f).string());
    Stdout().writeln("main end");
    0;
}
"#, "made\nuse local\ngot 5\nmain end\ndestroy local\n");
}

/// #866 (PR-B2, OIR-2 §10.2): a synchronous function returns a cold Future over a Future it borrows. A creation
/// borrows a bare Future argument, so the returned Future depends on `g`, which main keeps; `chain` declares that
/// dependency and takes nothing. Tomato's output.
#[test]
fn a_returned_future_over_a_borrowed_future_depends_on_it() {
    both_modes(r#"module app v1.0.0;
import "io/stdout" take Stdout;
object Res { has l is string; Res(:l); n() is int { l.length(); } destroy { Stdout().writeln("destroy " + l); } }
object M { M(); async make(l is string) is Res { Stdout().writeln("make " + l); Res(l: l); } }
object U { U(); async take(f is Future<Res>) is int { (await f).n(); } }
object C { C(); chain(f is Future<Res>) is Future<int> { U().take(f); } }
async main() is int {
    let g = M().make("x");
    let h = C().chain(g);
    Stdout().writeln("got " + (await h).string());
    Stdout().writeln("main end");
    0;
}
"#, "make x\ngot 1\nmain end\ndestroy x\n");
}

/// #851 with #849: a `map` over an array whose body awaits a resource.
#[test]
fn a_map_body_awaits_a_destroyable_outcome() {
    map_body(r#"async main() is int {
    let labels = ["a", "b"].map() -> name { (await Now().res(name)).label() + "!"; };
    Stdout().writeln(labels[0] + " " + labels[1]);
    0;
}
"#, "make a\ndestroy a\nmake b\ndestroy b\na! b!\n", true);
}

/// #851 with #849: a throw after an await in a `map` body abandons the builder, and every outcome made so far is
/// destroyed once. Structural only: Full verification runs out of its budget here (Incomplete, #634).
#[test]
fn a_throw_after_an_await_in_a_map_body_destroys_what_was_made() {
    map_body(r#"async main() is int {
    let out = Stdout();
    let result = try {
        let names = (0..3).map() -> index {
            let made = await Now().res("m" + index.string());
            if index == 1 { throw Broken.broken; }
            made.label();
        };
        names.size().string();
    } catch e is Broken { "broken"; };
    out.writeln(result);
    0;
}
"#, "make m0\ndestroy m0\nmake m1\ndestroy m1\nbroken\n", false);
}
