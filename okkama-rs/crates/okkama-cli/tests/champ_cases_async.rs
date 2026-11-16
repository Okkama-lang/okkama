//! The W1 cases of the async plan (#601, plan §4 V10): plain await, compiled through OIR-2 and run in Champ in
//! process, each checked in full against its case.json; and R3's async programs with resources that reach Champ.
//! With `SPARROW_CORE_GC_STRESS=1` every safepoint collects.
#![cfg(feature = "champ-abi")]

mod champ_support;

use champ_support::{case_dir, compile, run, run_source};
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
