//! P2.5 R3 (#601) producer tests: async with resources. Async actions with resource receivers, arguments and
//! results; awaited temporaries; `defer await`; Futures returned from actions and stored in fields; the system
//! Cancelled; the High golden of the corpus program `cold-futures`. Expectations come from OIR-2 §§10.1–10.3, the
//! async ABI (C-6), Bible §§11, 20, the corpus programs and the cases' `case.json`, never from current output.
use super::async_tests::{assert_names, async_ops, function, instructions, lower, summary, ty};
use super::tests::verify_high;
use crate::oir2 as ir;
use std::path::{Path, PathBuf};

/// `Res` of the corpus programs: a resource whose destroy appends its name to a shared log.
const RES: &str = concat!(
    "module app; import \"async/atomic\" take Atomic; ",
    "object Res { has name is string; has log is Atomic<string>; ",
    "Res(name is string, log is Atomic<string>) { self.name = name; self.log = log; } ",
    "label() is string { name; } destroy { log.replace(log.value() + name); } } ",
);

fn source(rest: &str) -> String {
    format!("{RES}{rest}")
}

/// High -> verify_high (Full) -> own -> verify_own (Full): own accepts what the producer emits.
fn high_to_own(high: ir::Module) -> ir::Module {
    let verify = ir::verify::VerifyOptions { mode: ir::verify::VerifyMode::Full, ..Default::default() };
    let catalog = super::native_summaries::P1NativeSummaries::for_module(&high).unwrap();
    let context = ir::verify::VerifyContext { text_locations: None, external_contracts: &catalog };
    ir::verify::verify_high_with(&high, &verify, context).unwrap_or_else(|report| panic!("verify_high: {report:#?}"));
    let options = ir::own::OwnOptions { verify: verify.clone(), ..ir::own::OwnOptions::default() };
    let inferred = ir::own::elaborate_ownership(high, &catalog, &options)
        .unwrap_or_else(|error| panic!("own_elaborate: {error:?}"));
    let own = ir::own::rewrite_subset(inferred, &options).unwrap_or_else(|error| panic!("own_rewrite: {error:?}"));
    let catalog = super::native_summaries::P1NativeSummaries::for_module(&own.module).unwrap();
    let context = ir::verify::VerifyContext { text_locations: None, external_contracts: &catalog };
    ir::verify::verify_own_with(&own.module, &verify, context)
        .unwrap_or_else(|report| panic!("verify_own: {report:#?}"));
    own.module
}

fn body_of(function: &ir::Function) -> &ir::Body {
    function.body.as_ref().expect("source body")
}

/// The Future each await of `function` awaits, with the block of the await.
fn awaited(function: &ir::Function) -> Vec<(ir::BlockId, ir::ValueId)> {
    instructions(function).into_iter().filter_map(|(block, _, instruction)| match &instruction.op {
        ir::Operation::Async(ir::AsyncOp::Await { future, .. }) => Some((block, *future)),
        _ => None,
    }).collect()
}

/// The `temp %value, S` marks of `function`, in block order, with their block.
fn temps(function: &ir::Function) -> Vec<(ir::BlockId, ir::ValueId, ir::ScopeId)> {
    instructions(function).into_iter().filter_map(|(block, _, instruction)| match &instruction.op {
        ir::Operation::Lexical(ir::LexicalOp::Temp { value, scope }) => Some((block, *value, *scope)),
        _ => None,
    }).collect()
}

/// The values of `function` whose type is a Future.
fn futures(module: &ir::Module, function: &ir::Function) -> Vec<ir::ValueId> {
    body_of(function).values.iter()
        .filter(|(_, value)| matches!(ty(module, value.ty), ir::Type::Future(_)))
        .map(|(id, _)| id).collect()
}

#[test]
fn oir2_r3_async_action_with_resource_receiver_arguments_and_result() {
    // OIR-2 §10.1: creation retains the receiver and the arguments; the body's result is a fresh resource that
    // the cache owns (§10.2). High states neither: the call is an ordinary unelaborated call (H2), and own derives
    // the cold inputs and the outcome packet.
    let module = lower(&source(concat!(
        "object Receiver { has name is string; has log is Atomic<string>; ",
        "Receiver(name is string, log is Atomic<string>) { self.name = name; self.log = log; } ",
        "async read(arg is Res) is Res { Res(name + arg.label(), log); } ",
        "destroy { log.replace(log.value() + name); } } ",
        "async main() is int { let log = Atomic(\"\"); defer log.close(); ",
        "{ let r = Receiver(\"R\", log); let a = Res(\"A\", log); let f = r.read(a); let s = await f; ",
        "s.label(); } 0; }",
    )));
    let read = function(&module, "app.Receiver.read#0");
    assert!(read.attrs.contains(&ir::FunctionAttr::Async));
    let names: Vec<_> = read.signature.params.iter().chain(&read.signature.results).map(|id| match ty(&module, *id) {
        ir::Type::Object { decl, .. } => module.declarations.get(*decl).unwrap().name.clone(),
        other => format!("{other:?}"),
    }).collect();
    assert_eq!(names, ["app.Receiver", "app.Res", "app.Res"], "receiver, argument, result");
    let res = read.signature.results[0];
    assert_eq!(summary(&module, read.async_body_summary).contract.results, [res]);
    let future = summary(&module, read.async_creation_summary).contract.results[0];
    assert_eq!(ty(&module, future), &ir::Type::Future(Some(res)));
    let main = function(&module, "main");
    let (_, _, call) = instructions(main).into_iter().find(|(_, _, instruction)| matches!(&instruction.op,
        ir::Operation::Call(ir::CallOp::Direct { function, .. }) if module.functions.get(*function).unwrap().symbol
            == "app.Receiver.read#0")).expect("the async call");
    let ir::Operation::Call(ir::CallOp::Direct { call: site, .. }) = &call.op else { unreachable!() };
    assert!(site.packet.is_none() && site.deps.is_empty() && site.summary.is_none(), "High has no packets (H2)");
    assert_eq!(site.args.len(), 2, "the receiver and the argument");
    assert_eq!(awaited(main).iter().map(|(_, future)| *future).collect::<Vec<_>>(), call.results);
    assert!(temps(main).iter().all(|(_, value, _)| !call.results.contains(value)), "a bound Future is no temporary");
    verify_high(&module, "resource receiver, arguments and result");
}

#[test]
fn oir2_r3_own_accepts_resource_receiver_arguments_and_result() {
    // The same shape with resources that hold no borrowed value: own creates the cold Future of the call from the
    // unelaborated High call, under the full verifier.
    let module = lower(concat!(
        "module app; object Lease { has name is string; Lease(name is string) { self.name = name; } ",
        "label() is string { name; } destroy { } } ",
        "object Reader { has name is string; Reader(name is string) { self.name = name; } ",
        "async read(arg is Lease) is Lease { Lease(name + arg.label()); } destroy { } } ",
        "async main() is int { let r = Reader(\"R\"); let a = Lease(\"A\"); let f = r.read(a); let s = await f; ",
        "s.label().length(); }",
    ));
    let own = high_to_own(module);
    let creations = own.functions.iter().filter_map(|(_, function)| function.body.as_ref())
        .flat_map(|body| body.insts.iter().map(|(_, instruction)| &instruction.op).collect::<Vec<_>>())
        .filter(|op| matches!(op, ir::Operation::Async(ir::AsyncOp::FutureNew { .. }))).count();
    assert_eq!(creations, 1, "own creates the cold Future of the call");
}

/// A temporary Future awaited in the inner block, a bound one, a field and a cancellation request.
fn temporaries() -> ir::Module {
    lower(&source(concat!(
        "object Work { Work(); async make(log is Atomic<string>) is Res { Res(\"R\", log); } } ",
        "object Holder { has pending is Future<Res>; Holder(pending is Future<Res>) { self.pending = pending; } ",
        "async get() is string { (await pending).label(); } } ",
        "async main() is int { let log = Atomic(\"\"); defer log.close(); ",
        "{ await Work().make(log); } ",
        "let f = Work().make(log); let kept = await f; ",
        "let g = Work().make(log); ",
        "let cancelled = try { (await g.cancel()).label(); } catch e is Cancelled { \"c\"; }; ",
        "kept.label().length() + cancelled.length(); }",
    )))
}

#[test]
fn oir2_r3_awaited_temporary_future_is_temp_in_the_awaiting_scope() {
    // OIR-2 §10.3: a temporary Future's cleanup boundary is the awaiting scope. The producer marks it `temp` there,
    // right after the call and before the await, which is how own finds the temporary take (R1).
    let module = temporaries();
    let main = function(&module, "main");
    let body = body_of(main);
    let marks = temps(main);
    let marked: Vec<_> = marks.iter().filter(|(_, value, _)| futures(&module, main).contains(value)).collect();
    assert_eq!(marked.len(), 1, "only the temporary Future is marked: {marks:?}");
    let &&(block, future, scope) = marked.first().unwrap();
    assert_eq!(scope, body.blocks.get(block).unwrap().lexical_scope, "the awaiting scope");
    assert_ne!(scope, body.blocks.get(body.entry).unwrap().lexical_scope, "the inner block, not main's scope");
    let ordered: Vec<_> = body.blocks.get(block).unwrap().insts.iter()
        .map(|id| &body.insts.get(*id).unwrap().op).collect();
    let position = |test: &dyn Fn(&ir::Operation) -> bool| ordered.iter().position(|op| test(op)).unwrap();
    let call = position(&|op| matches!(op, ir::Operation::Call(ir::CallOp::Direct { function, .. })
        if module.functions.get(*function).unwrap().symbol == "app.Work.make#0"));
    let temp = position(&|op| matches!(op,
        ir::Operation::Lexical(ir::LexicalOp::Temp { value, .. }) if *value == future));
    let wait = position(&|op| matches!(op, ir::Operation::Async(ir::AsyncOp::Await { future: f, .. }) if *f == future));
    assert!(call < temp && temp < wait, "call, temp, await in one block");
    verify_high(&module, "awaited temporaries");
}

/// The value bound to the binding `name` in `function`.
fn bound(function: &ir::Function, name: &str) -> ir::ValueId {
    let body = body_of(function);
    let (binding, _) = body.bindings.iter().find(|(_, binding)| binding.name == name).expect("binding");
    instructions(function).into_iter().find_map(|(_, _, instruction)| match &instruction.op {
        ir::Operation::Lexical(ir::LexicalOp::Bind { binding: bound, value }) if *bound == binding => Some(*value),
        _ => None,
    }).expect("bind")
}

#[test]
fn oir2_r3_bound_field_and_cancelled_futures_are_not_temporaries() {
    let module = temporaries();
    // A Future read from a field belongs to its object.
    let get = function(&module, "app.Holder.get#0");
    assert_eq!(awaited(get).len(), 1);
    assert!(temps(get).is_empty(), "{:?}", temps(get));
    // `f` is bound; `g.cancel()` is `g` itself (Bible §20.1), bound too: of the three awaited Futures only the
    // temporary is marked.
    let main = function(&module, "main");
    let futures: Vec<_> = awaited(main).into_iter().map(|(_, future)| future).collect();
    assert_eq!(futures.len(), 3);
    let marked: Vec<_> = temps(main).into_iter().map(|(_, value, _)| value)
        .filter(|value| futures.contains(value)).collect();
    assert_eq!(marked.len(), 1, "{marked:?} of {futures:?}");
    assert_ne!(marked[0], bound(main, "f"));
}

#[test]
fn oir2_r3_own_takes_the_temporary_and_shares_the_bound_future() {
    // R1 (OIR-2 §10.3): the temporary's managed outcome is taken by its awaiting scope; a bound Future is awaited
    // shared. The producer's `temp` mark is what tells them apart.
    let module = lower(concat!(
        "module app; object Lease { Lease(); destroy { } } ",
        "object Work { Work(); async make() is Lease { Lease(); } } ",
        "async main() is int { { await Work().make(); } let f = Work().make(); let kept = await f; 0; }",
    ));
    let own = high_to_own(module);
    let main = own.functions.iter().find(|(_, function)| function.symbol == "main").unwrap().1;
    let mut takes = 0;
    let mut shared = 0;
    for (_, block) in body_of(main).blocks.iter() {
        if let ir::TermKind::InvokeOwn { op, .. } = &block.term.kind {
            match **op {
                ir::Operation::Async(ir::AsyncOp::AwaitTake { .. }) => takes += 1,
                ir::Operation::Async(ir::AsyncOp::Await { .. }) => shared += 1,
                _ => {}
            }
        }
    }
    assert_eq!((takes, shared), (1, 1));
}

/// `defer await c.close();` in an inner block of async main.
const DEFER_AWAIT: &str = concat!(
    "object Conn { has log is Atomic<string>; Conn(log is Atomic<string>) { self.log = log; } ",
    "async close() is void { log.replace(log.value() + \"C\"); } } ",
    "async main() is int { let log = Atomic(\"\"); defer log.close(); ",
    "{ let c = Conn(log); defer await c.close(); log.replace(log.value() + \"B\"); } 0; }",
);

#[test]
fn oir2_r3_defer_await_is_an_async_defer_entry() {
    // Async ABI C-6: a defer entry whose expression awaits is an async body, `[defer(), async]`, with no
    // replacement. It creates no Future, so in High it carries the body envelope only (H6, #730: the
    // coordinator's decision on the question of #730).
    let module = lower(&source(DEFER_AWAIT));
    let close = function(&module, "main.defer#0");
    assert!(!close.attrs.contains(&ir::FunctionAttr::Async), "log.close() does not await");
    let entry = function(&module, "main.defer#1");
    assert!(entry.attrs.contains(&ir::FunctionAttr::DeferEntry { replacements: vec![] }));
    assert!(entry.attrs.contains(&ir::FunctionAttr::Async));
    let body = summary(&module, entry.async_body_summary);
    assert!(body.contract.suspends && body.contract.results.is_empty());
    assert!(body.contract.failures.contains(&ir::FailureKind::Cancelled));
    assert_eq!(entry.async_creation_summary, None, "a defer entry creates no Future");
    // Its body awaits the temporary Future of `c.close()` in the entry's scope.
    let ops = async_ops(entry);
    let [ir::AsyncOp::Await { future, result: None }] = ops.as_slice() else { panic!("{ops:?}") };
    assert_eq!(temps(entry).iter().map(|(_, value, _)| *value).collect::<Vec<_>>(), [*future]);
    let main = function(&module, "main");
    let registered: Vec<_> = instructions(main).into_iter().filter_map(|(_, _, instruction)| match &instruction.op {
        ir::Operation::Lexical(ir::LexicalOp::DeferRegister { action, .. }) => Some(action.function),
        _ => None,
    }).map(|id| module.functions.get(id).unwrap().symbol.clone()).collect();
    assert_eq!(registered, ["main.defer#0", "main.defer#1"]);
    verify_high(&module, "defer await");
}

#[test]
fn oir2_r6_defer_await_reaches_verified_low() {
    // #730, async ABI C-6: own runs the async defer entry through `invoke.defer` with its input packet at the
    // block's exit (nothing after the registration can fail); coro and low take it under the full verifiers.
    let module = lower(concat!(
        "module app; object Conn { Conn(); async close() is void { } } ",
        "async main() is int { { let c = Conn(); defer await c.close(); } 0; }",
    ));
    let coro = super::async_tests::high_to_coro(module).unwrap_or_else(|error| panic!("{error}"));
    let entry = function(&coro, "main.defer#0");
    assert!(entry.attrs.contains(&ir::FunctionAttr::Async) && entry.async_creation_summary.is_none());
    let main = function(&coro, "main");
    let invokes = body_of(main).blocks.iter().filter(|(_, block)| matches!(&block.term.kind,
        ir::TermKind::InvokeDefer { call, .. } if call.packet.is_some())).count();
    assert_eq!(invokes, 1, "the one exit of the block after the registration");
    super::async_tests::coro_to_low(coro).unwrap_or_else(|error| panic!("{error}"));
}

#[test]
fn oir2_r3_a_return_inside_an_awaiting_defer_is_refused_by_name() {
    // Async ABI C-6: an async defer entry has no replacement in 0.6.0 (verifier rule `async-defer-replace`).
    assert_names(
        &source(concat!(
            "object Conn { Conn(); async close() is void { } } ",
            "async main() is int { let c = Conn(); defer if true { await c.close(); return 1; } else { 0; }; 0; }",
        )),
        "return inside an awaiting defer (async ABI C-6, async-defer-replace)",
        Some("if true { await c.close(); return 1; }"),
    );
    // A synchronous defer keeps its replacement (D8).
    let module = lower(concat!("module app; object C { C(); n() is int { 1; } } ",
        "main() is int { defer if true { return 1; } else { 0; }; 0; }"));
    let entry = function(&module, "main.defer#0");
    assert!(!entry.attrs.contains(&ir::FunctionAttr::Async));
    assert!(matches!(&entry.attrs[0], ir::FunctionAttr::DeferEntry { replacements } if replacements.len() == 1));
}

#[test]
fn oir2_r3_futures_returned_from_actions_and_stored_in_fields() {
    // OIR-2 §10.1: a sync action may return the cold Future it creates; own carries its fresh inputs with it
    // (escape through `cache.rehome`, R1). A Future is an ordinary field value of its object.
    let module = lower(&source(concat!(
        "object Receiver { has name is string; has log is Atomic<string>; ",
        "Receiver(name is string, log is Atomic<string>) { self.name = name; self.log = log; } ",
        "async read(arg is Res) is string { name + arg.label(); } destroy { log.replace(log.value() + name); } } ",
        "object Factory { Factory(); make(log is Atomic<string>) is Future<string> ",
        "{ Receiver(\"R\", log).read(Res(\"A\", log)); } } ",
        "object Holder { has pending is Future<string>; Holder(pending is Future<string>) { self.pending = pending; } ",
        "async get() is string { await pending; } } ",
        "async main() is int { let log = Atomic(\"\"); defer log.close(); ",
        "let h = Holder(Factory().make(log)); (await h.get()).length(); }",
    )));
    let make = function(&module, "app.Factory.make#0");
    assert!(!make.attrs.contains(&ir::FunctionAttr::Async), "creating a Future does not make an action async");
    let string = module.types.iter().find(|(_, ty)| **ty == ir::Type::Str).unwrap().0;
    assert_eq!(ty(&module, make.signature.results[0]), &ir::Type::Future(Some(string)));
    assert!(async_ops(make).is_empty(), "make neither awaits nor creates in High");
    let holder = module.declarations.iter().find(|(_, declaration)| declaration.name == "app.Holder").unwrap().1;
    let ir::DeclarationKind::Object { fields, .. } = &holder.kind else { panic!() };
    assert_eq!(ty(&module, fields[0].ty), &ir::Type::Future(Some(string)));
    verify_high(&module, "returned and stored Futures");
    high_to_own(module);
}

#[test]
fn oir2_r3_catch_cancelled_declares_the_system_cancelled() {
    // Bible §20.5 (#724): `catch e is Cancelled` binds the system Cancelled, an object without fields whose
    // `text()` is Tomato's message.
    let module = lower(concat!(
        "module app; object J { J(); async run() is int { 1; } } ",
        "async main() is int { let f = J().run(); f.cancel(); ",
        "let t = try { await f; \"no\"; } catch e is Cancelled { e.text(); }; t.length(); }",
    ));
    let (decl, declaration) = module.declarations.iter().find(|(_, declaration)| declaration.name == "Cancelled")
        .expect("the system Cancelled");
    assert!(ir::is_cancelled(&module, decl), "{declaration:?}");
    let main = function(&module, "main");
    let body = body_of(main);
    let caught = body.blocks.iter().find_map(|(_, block)| match block.term.kind {
        ir::TermKind::Catch { ty, .. } => Some(ty),
        _ => None,
    }).expect("the catch");
    assert_eq!(ty(&module, caught), &ir::Type::Object { decl, args: vec![] });
    let text = function(&module, "Cancelled.text");
    let constants: Vec<_> = instructions(text).into_iter().filter_map(|(_, _, instruction)| match &instruction.op {
        ir::Operation::Const(ir::Literal::Str(text)) => Some(text.clone()),
        _ => None,
    }).collect();
    assert_eq!(constants, ["Operation was cancelled"]);
    verify_high(&module, "catch Cancelled");
    // A program that never names Cancelled declares none.
    let module = lower(concat!("module app; object J { J(); async run() is int { 1; } } ",
        "async main() is int { await J().run(); }"));
    assert!(module.declarations.iter().all(|(_, declaration)| declaration.name != "Cancelled"));
}

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// The High module of a corpus program; its `tap` import resolves in `t/support`, as the oracle runs it.
fn corpus_high(name: &str) -> ir::Module {
    let path = repository().join(format!("tests/ownership-corpus/{name}.ok"));
    let text = std::fs::read_to_string(&path).unwrap();
    super::case_tests::source_high(name, text, repository().join("t/support"))
        .unwrap_or_else(|error| panic!("{name}: {error}"))
}

/// The corpus programs of R3 that have a High golden (plan §4 R3).
const CORPUS: [&str; 1] = ["cold-futures"];

#[test]
fn oir2_r3_corpus_high_goldens() {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/lowering/oir2/goldens/p2_5");
    for name in CORPUS {
        let expected = std::fs::read_to_string(directory.join(format!("corpus.{name}.oir"))).unwrap();
        let parsed = ir::text::parse(&expected).unwrap_or_else(|error| panic!("{name}: {error:?}"));
        assert_eq!(ir::text::print(&parsed).unwrap(), expected, "{name}: stored golden is canonical text");
        verify_high(&parsed, name);
        let actual = corpus_high(name);
        assert_eq!(ir::text::print(&actual).unwrap(), expected, "{name}");
    }
}

#[test]
fn oir2_r3_void_capture_await_is_rejected_before_lowering() {
    // L08.void.capture-await: capturing the await of an async void action is invalid (case.json: an error, at
    // check or at run time). The checker rejects it, so no High exists to lower.
    let directory = super::case_tests::case_directory("L08.void.capture-await");
    let text = std::fs::read_to_string(directory.join("main.ok")).unwrap();
    let file = crate::SourceFile::new("main.ok", text).unwrap();
    let program = crate::syntax::parse(&file, &crate::syntax::ParseOptions::default()).unwrap();
    let roots = [(&program, file.name(), Some(&file))];
    let graph = crate::lowering::modules::load_graph(&roots, &mut super::tests::StdLoader).unwrap();
    let sources = crate::lowering::modules::source_modules(&graph, &roots);
    let Err(errors) = crate::analysis::analyze(sources) else { panic!("the checker rejects the capture") };
    assert!(errors.iter().any(|error| error.message == "Await of an async void action cannot be used as a value."),
        "{errors:?}");
}

#[test]
fn oir2_r3_a_module_without_awaits_in_defers_keeps_sync_defer_entries() {
    let module = lower(concat!("module app; object C { C(); close() is void { } } ",
        "main() is int { let c = C(); defer c.close(); 0; }"));
    assert!(!module.requires.contains(&ir::Feature::Async));
    assert!(!function(&module, "main.defer#0").attrs.contains(&ir::FunctionAttr::Async));
}

#[test]
fn oir2_r3_spawn_keeps_its_diagnostic_inside_defer_await() {
    // Spawn belongs to slice P1 in a defer entry too.
    assert_names(
        &source(concat!(
            "object W { W(); async run() is void { } } ",
            "async main() is int { defer await spawn W().run(); 0; }",
        )),
        "spawn of an async action (P2.5 slice P1, fut.spawn)",
        Some("spawn W().run()"),
    );
}
