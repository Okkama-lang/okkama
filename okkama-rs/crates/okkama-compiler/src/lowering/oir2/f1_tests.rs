//! #601 F1 in the producer and own: async role slots, external witnesses (#773) and bound async action values
//! (ruling PR-D); `fut.spawn.bound` (ruling PR-E, #766).
use crate::oir2 as ir;

fn function<'m>(module: &'m ir::Module, symbol: &str) -> (ir::FuncId, &'m ir::Function) {
    module.functions.iter().find(|(_, function)| function.symbol == symbol)
        .unwrap_or_else(|| panic!("no function {symbol}"))
}

fn role_slot(module: &ir::Module, role: &str, slot: usize) -> ir::Signature {
    module.declarations.iter().find_map(|(_, declaration)| match &declaration.kind {
        ir::DeclarationKind::Role { methods, .. } if declaration.name.ends_with(role) => {
            Some(methods[slot].signature.clone())
        }
        _ => None,
    }).unwrap_or_else(|| panic!("no role {role}"))
}

/// The function a witness of `object` names for slot `slot`.
fn witness(module: &ir::Module, object: &str, slot: usize) -> ir::FuncId {
    module.declarations.iter().find_map(|(_, declaration)| match &declaration.kind {
        ir::DeclarationKind::Object { witnesses, .. } if declaration.name.ends_with(object) => {
            witnesses.first().map(|witness| witness.slots[slot])
        }
        _ => None,
    }).unwrap_or_else(|| panic!("no witness of {object}"))
}

fn calls(function: &ir::Function) -> Vec<&ir::CallOp> {
    let body = function.body.as_ref().expect("a body");
    body.insts.iter().filter_map(|(_, inst)| match &inst.op {
        ir::Operation::Call(call) => Some(call),
        _ => None,
    }).collect()
}

/// High → own → coro → low of a std program, under the full verifier after each pass.
fn full_low(source: &str) {
    let high = super::lower_std(source).unwrap_or_else(|error| panic!("{error}"));
    let coro = super::super::async_tests::high_to_coro(high).unwrap_or_else(|error| panic!("{error}"));
    super::super::async_tests::coro_to_low(coro).unwrap_or_else(|error| panic!("{error}"));
}

fn future_of(module: &ir::Module, ty: ir::TypeId) -> Option<ir::TypeId> {
    match module.types.get(ty).unwrap() {
        ir::Type::Future(outcome) => *outcome,
        other => panic!("expected a Future, got {other:?}"),
    }
}

const LOADER: &str = concat!(
    "module app v1.0.0; import \"io/stdout\" take Stdout; ",
    "object Res { has l is string; Res(:l); n() is int { l.length(); } destroy { Stdout().writeln(l); } } ",
    "role Loader { async load(r is Res) is int; } ",
    "object Net does Loader { Net(); async load(r is Res) is int { r.n(); } } ",
    "async main() is int { let loader is Loader = Net(); let f = loader.load(Res(l: \"x\")); await f; }",
);

/// PR-D items 1, 2 and 4: the slot returns `future<T>`; the witness names a synchronous creation thunk with the slot's
/// signature that calls the async action; the caller dispatches and awaits the Future it gets.
#[test]
fn f1_an_async_role_slot_returns_its_future_through_a_creation_thunk() {
    let module = super::lower_std(LOADER).unwrap_or_else(|error| panic!("{error}"));
    super::verify_high(&module, "async role slot");
    let slot = role_slot(&module, "Loader", 0);
    let [future] = slot.results.as_slice() else { panic!("one result") };
    assert_eq!(*module.types.get(future_of(&module, *future).unwrap()).unwrap(), ir::Type::Int);
    let thunk = witness(&module, "Net", 0);
    let (load, _) = function(&module, "app.Net.load#0");
    assert_ne!(thunk, load, "no async function in a witness table (I-F2)");
    let thunk = module.functions.get(thunk).unwrap();
    assert!(!thunk.attrs.contains(&ir::FunctionAttr::Async));
    assert_eq!(thunk.signature.results, slot.results);
    assert_eq!(&thunk.signature.params[1..], slot.params.as_slice());
    let [ir::CallOp::Direct { function: called, .. }] = calls(thunk).as_slice() else { panic!("the call") };
    assert_eq!(*called, load);
    let (_, main) = function(&module, "main");
    assert!(calls(main).iter().any(|call| matches!(call, ir::CallOp::Role { .. })), "a dispatch");
}

/// I-F1 on own's summary of the thunk: creation only (no suspension, no cancellation), a fresh result, and the fresh
/// argument taken into the result, which the cold Future retains (checklist §4.8 item 1); the whole program reaches a
/// verified Low under the full verifiers.
#[test]
fn f1_a_creation_thunk_creates_and_retains_without_suspending() {
    let (own, _) = super::own_case_tests::own_stage(LOADER, None, ir::verify::VerifyMode::Full)
        .unwrap_or_else(|error| panic!("{error}"));
    let (_, thunk) = function(&own, "app.Net.witness.app.Loader.slot0");
    let summary = own.summaries.get(thunk.summary.unwrap()).unwrap();
    assert!(!summary.contract.suspends && !summary.contract.async_destroy);
    assert!(!summary.contract.failures.contains(&ir::FailureKind::Cancelled), "{:?}", summary.contract.failures);
    let [result] = summary.results.as_slice() else { panic!("one result") };
    assert!(!result.fresh.is_empty(), "the created Future is fresh, with its cache duty");
    let resource = &summary.parameters[1];
    assert_eq!((resource.convention, &resource.destination),
        (ir::FreshConvention::Take, &ir::TransferDestination::Result));
    assert!(resource.retention.is_some());
    full_low(LOADER);
}

/// PR-D: an async slot implemented by an async intrinsic (`Cancelling.wait` by the cancellation token, D22) gets a
/// thunk that calls the intrinsic, as one implemented by a body does.
#[test]
fn f1_an_async_slot_implemented_by_an_async_intrinsic_has_a_thunk() {
    let module = super::lower_std(concat!(
        "module app v1.0.0; import \"async/cancellation\" take Cancellation, Cancelling; ",
        "async main() is int { let c = Cancellation(); let t = c.cancelling(); c.cancel(); await t.wait(); 0; }",
    )).unwrap_or_else(|error| panic!("{error}"));
    super::verify_high(&module, "intrinsic thunk");
    let slot = role_slot(&module, "Cancelling", 2);
    let thunk = module.functions.get(witness(&module, "CancellationToken", 2)).unwrap();
    assert!(!thunk.attrs.contains(&ir::FunctionAttr::Async) && thunk.body.is_some());
    assert_eq!(thunk.signature.results, slot.results);
    let [ir::CallOp::Intrinsic { function: called, .. }] = calls(thunk).as_slice() else { panic!("an intrinsic") };
    let target = module.functions.get(*called).unwrap();
    assert!(target.attrs.contains(&ir::FunctionAttr::Async) && target.body.is_none());
}

/// #773: a synchronous catalogue external that is a role slot of its object (LockGuard's `close` for `Closing`) is
/// reached through an adapter of the slot's signature; no witness table names an external.
#[test]
fn f1_an_external_witness_is_reached_through_an_adapter() {
    let module = super::lower_std(concat!(
        "module app v1.0.0; import \"io/stream\" take Closing; import \"async/lock\" take Lock, LockGuard; ",
        "async main() is int { let lock = Lock(); let guard = await lock.guard(); let c is Closing = guard; ",
        "c.close(); if c.closed() { 1; } else { 0; } }",
    )).unwrap_or_else(|error| panic!("{error}"));
    super::verify_high(&module, "external witness");
    for (_, declaration) in module.declarations.iter() {
        let ir::DeclarationKind::Object { witnesses, .. } = &declaration.kind else { continue };
        for &slot in witnesses.iter().flat_map(|witness| &witness.slots) {
            let named = module.functions.get(slot).unwrap();
            assert!(named.body.is_some() && !named.attrs.iter().any(|attr| matches!(attr,
                ir::FunctionAttr::Intrinsic { .. } | ir::FunctionAttr::Extern(_))), "{} names {}",
                declaration.name, named.symbol);
        }
    }
    let close = module.functions.get(witness(&module, "LockGuard", 1)).unwrap();
    assert!(matches!(calls(close).as_slice(), [ir::CallOp::Intrinsic { .. }]), "{}", close.symbol);
}

const BOUND: &str = concat!(
    "module app v1.0.0; ",
    "role Loading { async load(n is int) is int; } ",
    "object Net does Loading { Net(); async load(n is int) is int { n * 2; } } ",
    "object Worker { Worker(); async add(n is int) is int { n + 1; } } ",
    "async main() is int { let w is async (int) is int = Worker().add; let l is Loading = Net(); ",
    // The values are called before the first await: a call after it meets #787 under the full coro verifier.
    "let r is async (int) is int = l.load; let f = w(1); let g = r(2); await f + await g; }",
);

/// PR-D item 3: `async (A) is T` is a synchronous `Fn` returning `future<T>`; binding an async method binds its
/// creation thunk, binding an async role slot binds the slot (whose implementations are thunks), and calling the
/// value is the ordinary closure call that yields the Future.
#[test]
fn f1_a_bound_async_action_is_an_fn_that_returns_its_future() {
    let module = super::lower_std(BOUND).unwrap_or_else(|error| panic!("{error}"));
    super::verify_high(&module, "bound async action");
    let (_, main) = function(&module, "main");
    let body = main.body.as_ref().unwrap();
    let (add, _) = function(&module, "app.Worker.add#0");
    let (thunk, thunk_function) = function(&module, "app.Worker.add#0.thunk");
    assert!(!thunk_function.attrs.contains(&ir::FunctionAttr::Async));
    let mut bound = 0;
    let mut bound_roles = 0;
    for (_, inst) in body.insts.iter() {
        match &inst.op {
            ir::Operation::Core(ir::CoreOp::FnBind { function: named, ty, .. }) => {
                assert_eq!(*named, thunk, "the thunk, never the async method {add:?}");
                let ir::Type::Fn(fn_type) = module.types.get(*ty).unwrap() else { panic!("an Fn") };
                assert!(!fn_type.is_async);
                assert!(future_of(&module, fn_type.result.unwrap()).is_some());
                bound += 1;
            }
            ir::Operation::Core(ir::CoreOp::FnBindRole { .. }) => bound_roles += 1,
            _ => {}
        }
    }
    assert_eq!((bound, bound_roles), (1, 1));
    let closures = calls(main).into_iter().filter(|call| matches!(call, ir::CallOp::Closure { .. })).count();
    assert_eq!(closures, 2);
    full_low(BOUND);
}


const SPAWNS: &str = concat!(
    "module app v1.0.0; ",
    "object Job { has n is int; Job(:n); async run() is int { n; } async add(k is int) is int { n + k; } } ",
    "object Starter { Starter(); start(n is int) is Future<int> { Job(n: n).run(); } } ",
    "async main() is int { let fresh = spawn Job(n: 1).run(); let made = spawn Starter().start(4); ",
    "let action is async (int) is int = Job(n: 7).add; let valued = spawn action(3); ",
    "await fresh + await made + await valued; }",
);

/// Ruling PR-E: `spawn` of the Future its creating call just gave keeps D24.1's duty form (own forwards the owner
/// state); the Future of a synchronous call (#766) or of an action value call gets `fut.spawn.bound`, which takes and
/// gives no duty. The program reaches Low under the full verifiers (I-F4: the bound spawn changes no state).
#[test]
fn f1_a_spawn_of_a_future_not_created_on_the_spot_is_a_bound_spawn() {
    let module = super::lower_std(SPAWNS).unwrap_or_else(|error| panic!("{error}"));
    super::verify_high(&module, "spawns");
    let (_, main) = function(&module, "main");
    let spawns = main.body.as_ref().unwrap().insts.iter()
        .filter(|(_, inst)| matches!(inst.op, ir::Operation::Async(ir::AsyncOp::FutureSpawn { state: None, .. })))
        .count();
    assert_eq!(spawns, 3, "High names every spawn fut.spawn %f");
    let (own, _) = super::own_case_tests::own_stage(SPAWNS, None, ir::verify::VerifyMode::Full)
        .unwrap_or_else(|error| panic!("{error}"));
    let (_, main) = function(&own, "main");
    let body = main.body.as_ref().unwrap();
    let forms = body.insts.iter().filter_map(|(_, inst)| match &inst.op {
        ir::Operation::Async(ir::AsyncOp::FutureSpawn { state, .. }) => Some(state.is_some()),
        ir::Operation::Async(ir::AsyncOp::FutureSpawnBound { .. }) => Some(false),
        _ => None,
    }).collect::<Vec<_>>();
    assert_eq!(forms, [true, false, false], "duty form for the fresh call, the bound form for the others");
    full_low(SPAWNS);
}

/// Ruling PR-E: own keeps the duty form only when the creating call is the instruction just before the spawn. With
/// anything between them (here a constant), the spawn is the bound form, and coro keeps it, under the full verifiers.
#[test]
fn f1_own_keeps_the_duty_form_only_for_a_spawn_right_after_its_creation() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../okkama-oir/src/own/tests/async-high/p1-spawn-detached.oir");
    let text = std::fs::read_to_string(path).unwrap();
    let spawned = |text: &str| {
        let coro = super::super::async_tests::high_to_coro(ir::text::parse(text).unwrap())
            .unwrap_or_else(|error| panic!("{error}"));
        let (_, main) = function(&coro, "main");
        main.body.as_ref().unwrap().insts.iter().filter_map(|(_, inst)| match &inst.op {
            ir::Operation::Async(ir::AsyncOp::FutureSpawn { .. }) => Some("duty"),
            ir::Operation::Async(ir::AsyncOp::FutureSpawnBound { .. }) => Some("bound"),
            _ => None,
        }).collect::<Vec<_>>()
    };
    assert_eq!(spawned(&text), ["duty"]);
    let separated = text.replace("    fut.spawn %1\n", "    %9: int = const 1\n    fut.spawn %1\n");
    assert_ne!(separated, text);
    assert_eq!(spawned(&separated), ["bound"]);
}

/// #791 D1 (W7): an async slot whose implementation narrows a successor result (`^Walker` for `^Stepping`, which the
/// front end admits) would need the slot's Future of `^Stepping` from the body's Future of `^Walker`. No operation
/// converts the outcome of a cold Future, so the producer refuses it with a named construct, not an internal error.
#[test]
fn f1_a_covariant_successor_result_of_an_async_slot_is_refused_by_name() {
    let source = concat!(
        "module app v1.0.0; ",
        "role Stepping { async step() is ^Stepping; label() is int; } ",
        "object Walker does Stepping { has n is int = 0; Walker() {} async step() is ^Walker { ^n = n + 1; } ",
        "label() is int { n; } } ",
        "async main() is int { let s is Stepping = Walker(); let t = await s.step(); t.label(); }",
    );
    match super::lower_std(source) {
        Err(super::LowerError::Unsupported { message, span }) => {
            assert!(message.contains("covariant successor result of an async role slot"), "{message}");
            assert!(!span.is_empty());
        }
        Err(other) => panic!("expected the named refusal, got {other}"),
        Ok(_) => panic!("expected the named refusal"),
    }
}

const MANAGED: &str = concat!(
    "module app v1.0.0; import \"io/stdout\" take Stdout; ",
    "object Res { has l is string; Res(:l); n() is int { l.length(); } destroy { Stdout().writeln(l); } } ",
    "role Making { async make() is Res; } ",
    "object Maker does Making { Maker(); async make() is Res { Res(l: \"made\"); } } ",
    "async main() is int { let m is Making = Maker(); let r = await m.make(); ",
    "let a is async () is Res = Maker().make; let s = await a(); r.n() + s.n(); }",
);

/// #791 W2 (§4.8 item 2): the Future of a role call and of an action value call is a dynamic entry of the awaiting
/// scope (PR-D item 4), awaited shared. With a managed outcome own takes neither (no temporary take: no
/// `unique_cache.proof`, no `await.take`), even where the producer marks the awaited call `temp` (OIR-2 §10.3): a
/// Future this body did not create has no take capability, and its outcome is released by the dynamic step.
#[test]
fn f1_a_role_or_action_call_future_is_awaited_shared_without_a_take() {
    let module = super::lower_std(MANAGED).unwrap_or_else(|error| panic!("{error}"));
    super::verify_high(&module, "managed role call");
    let (_, main) = function(&module, "main");
    let body = main.body.as_ref().unwrap();
    let futures = body.values.iter().filter(|(_, value)| {
        matches!(module.types.get(value.ty).unwrap(), ir::Type::Future(_))
    }).map(|(id, _)| id).collect::<Vec<_>>();
    let awaited = body.insts.iter().filter(|(_, inst)| matches!(inst.op, ir::Operation::Async(ir::AsyncOp::Await {
        future, .. }) if futures.contains(&future))).count();
    assert_eq!(awaited, 2, "both Futures are awaited");
    let (own, _) = super::own_case_tests::own_stage(MANAGED, None, ir::verify::VerifyMode::Full)
        .unwrap_or_else(|error| panic!("{error}"));
    let (_, main) = function(&own, "main");
    let body = main.body.as_ref().unwrap();
    let takes = body.insts.iter().map(|(_, inst)| &inst.op)
        .chain(body.blocks.iter().filter_map(|(_, block)| match &block.term.kind {
            ir::TermKind::InvokeOwn { op, .. } => Some(&**op),
            _ => None,
        }))
        .filter(|op| matches!(op, ir::Operation::Async(ir::AsyncOp::AwaitTake { .. }
            | ir::AsyncOp::UniqueCacheProof { .. })))
        .count();
    assert_eq!(takes, 0);
    full_low(MANAGED);
}

/// Two implementations of `Cancelling`: the std token (whose `wait` is a D22 intrinsic) and a user body.
const TWO_CANCELLING: &str = concat!(
    "module app v1.0.0; import \"async/cancellation\" take Cancellation, Cancelling; ",
    "object Quiet does Cancelling { Quiet(); requested() is bool { true; } check() is void { } ",
    "async wait() is Cancelling { Quiet(); } } ",
    "object Probe { Probe(); async probe(c is Cancelling) is int { let woken = await c.wait(); ",
    "if woken.requested() { 1; } else { 0; } } } ",
    "async main() is int { let c = Cancellation(); let token = c.cancelling(); c.cancel(); ",
    "await Probe().probe(token) + await Probe().probe(Quiet()); }",
);

/// #791 W3 (§4.8 item 3): an async slot implemented by an async intrinsic and by an async body. Both witnesses are
/// creation thunks with the slot's signature, and own's Full verifier admits both: each thunk returns the fresh
/// Future its call creates (I-F1: no suspension, no Cancelled). The SPB 17 half of W3 is in
/// `codegen/sparrow_oir2/roles_tests.rs`.
#[test]
fn f1_two_implementations_of_an_async_slot_share_its_thunk_signature() {
    let (own, _) = super::own_case_tests::own_stage(TWO_CANCELLING, None, ir::verify::VerifyMode::Full)
        .unwrap_or_else(|error| panic!("{error}"));
    let slot = role_slot(&own, "Cancelling", 2);
    for object in ["CancellationToken", "Quiet"] {
        let thunk = own.functions.get(witness(&own, object, 2)).unwrap();
        assert!(!thunk.attrs.contains(&ir::FunctionAttr::Async), "{}", thunk.symbol);
        assert_eq!((&thunk.signature.results, &thunk.signature.params[1..]), (&slot.results, &slot.params[..]));
        let summary = own.summaries.get(thunk.summary.unwrap()).unwrap();
        assert!(!summary.contract.suspends && !summary.contract.async_destroy, "{}", thunk.symbol);
        assert!(!summary.contract.failures.contains(&ir::FailureKind::Cancelled), "{}: {:?}", thunk.symbol,
            summary.contract.failures);
        let [result] = summary.results.as_slice() else { panic!("one result") };
        assert!(!result.fresh.is_empty(), "{}: the created Future is fresh, with its cache duty", thunk.symbol);
    }
}

/// The Full verifier's report on an own module that a test has changed.
fn full_report(own: &ir::Module) -> ir::verify::VerifyReport {
    let verify = ir::verify::VerifyOptions { mode: ir::verify::VerifyMode::Full, ..Default::default() };
    let catalog = super::P1NativeSummaries::for_module(own).unwrap();
    let context = ir::verify::VerifyContext { text_locations: None, external_contracts: &catalog };
    ir::verify::verify_own_with(own, &verify, context).expect_err("the changed module is refused")
}

fn reports(report: &ir::verify::VerifyReport, rule: ir::verify::RuleId) -> bool {
    report.diagnostics.iter().any(|diagnostic| diagnostic.rule == rule)
}

/// The export rule of a callee body (B5.1), which the creation-thunk allowance relaxes.
fn exports(report: &ir::verify::VerifyReport, function: ir::FuncId) -> bool {
    report.diagnostics.iter().any(|diagnostic| diagnostic.rule == ir::verify::RuleId::B51
        && diagnostic.message == "body exports accepted inputs beyond its declared take retention"
        && matches!(diagnostic.primary.entity, ir::verify::EntityLocation::Terminator { function: f, .. }
            | ir::verify::EntityLocation::Instruction { function: f, .. } if f == function))
}

/// Ruling PR-N2 (#860): the creation thunk of `Loader.load` forwards its entry packet into `fut.new`, so the fresh
/// `Res` leaves with the Future. Full admits that export only for an adapter of an async target whose owning
/// parameter is taken into the result.
#[test]
fn f1_a_creation_thunk_exports_its_entry_only_as_a_taken_input_of_its_future() {
    let (own, _) = super::own_case_tests::own_stage(LOADER, None, ir::verify::VerifyMode::Full)
        .unwrap_or_else(|error| panic!("{error}"));
    let thunk = witness(&own, "Net", 0);
    let summary = own.functions.get(thunk).unwrap().summary.unwrap();
    let taken = own.summaries.get(summary).unwrap().parameters.iter()
        .position(|effect| effect.convention == ir::FreshConvention::Take)
        .expect("the thunk takes the fresh Res");
    assert_eq!(own.summaries.get(summary).unwrap().parameters[taken].destination, ir::TransferDestination::Result);
    assert_eq!(own.summaries.get(summary).unwrap().parameters[taken].retention,
        Some(ir::SummarySelection::Parameter(taken as u16)), "the Res is kept whole");

    // Retaining a field of the Res does not cover the whole entry piece that leaves with the Future.
    let mut field = own.clone();
    let res = field.declarations.iter().find(|(_, declaration)| declaration.name.ends_with(".Res"))
        .map(|(id, _)| id).expect("Res");
    field.summaries.get_mut(summary).unwrap().parameters[taken].retention =
        Some(ir::SummarySelection::Field { parameter: taken as u16,
            field: ir::FieldRef { declaration: res, index: 0 } });
    let report = full_report(&field);
    assert!(exports(&report, thunk), "a field retention exports the whole Res: {report:?}");

    let mut borrowed = own.clone();
    let effect = &mut borrowed.summaries.get_mut(summary).unwrap().parameters[taken];
    effect.convention = ir::FreshConvention::Borrow;
    effect.retention = None;
    effect.destination = ir::TransferDestination::None;
    let report = full_report(&borrowed);
    assert!(exports(&report, thunk), "a borrowed Res leaves with the Future: {report:?}");

    let mut plain = own.clone();
    plain.functions.get_mut(thunk).unwrap().attrs.retain(|attr| !matches!(attr, ir::FunctionAttr::Adapter { .. }));
    let report = full_report(&plain);
    assert!(exports(&report, thunk), "only an adapter exports its entry: {report:?}");
}

/// A synchronous slot that takes a Future and returns it: the caller offers the Future with its cache duty.
const KEEPER: &str = concat!(
    "module app v1.0.0; import \"io/stdout\" take Stdout; ",
    "object Res { has l is string; Res(:l); n() is int { l.length(); } destroy { Stdout().writeln(l); } } ",
    "object Maker { Maker(); async make(l is string) is Res { Res(l: l); } } ",
    "role Keeping { keep(f is Future<Res>) is Future<Res>; } ",
    "object K does Keeping { K(); keep(f is Future<Res>) is Future<Res> { f; } } ",
    "async main() is int { let k is Keeping = K(); (await k.keep(Maker().make(\"x\"))).n(); }",
);

/// Ruling PR-N2 (#860): a call of an async callee is a creation, which borrows a bare Future argument. The same role
/// call that consumes a Future into a synchronous slot is refused once the slot is async (B9.1).
#[test]
fn f1_a_call_of_an_async_slot_offers_no_futures_duty() {
    let (own, _) = super::own_case_tests::own_stage(KEEPER, None, ir::verify::VerifyMode::Full)
        .unwrap_or_else(|error| panic!("{error}"));
    let mut changed = own.clone();
    let role = changed.declarations.iter().find(|(_, declaration)| declaration.name.ends_with("Keeping"))
        .map(|(id, _)| id).expect("the role");
    let ir::DeclarationKind::Role { methods, .. } = &mut changed.declarations.get_mut(role).unwrap().kind else {
        panic!("a role")
    };
    methods[0].is_async = true;
    let report = full_report(&changed);
    assert!(reports(&report, ir::verify::RuleId::B91), "{report:?}");
}

/// An async slot that takes a Future: own borrows the bare Future argument (PR-N2).
const TAKING: &str = concat!(
    "module app v1.0.0; import \"io/stdout\" take Stdout; ",
    "object Res { has l is string; Res(:l); n() is int { l.length(); } destroy { Stdout().writeln(l); } } ",
    "object Maker { Maker(); async make(l is string) is Res { Res(l: l); } } ",
    "role Taking { async take(f is Future<Res>) is int; } ",
    "object Taker does Taking { Taker(); async take(f is Future<Res>) is int { (await f).n(); } } ",
    "async main() is int { let t is Taking = Taker(); await t.take(Maker().make(\"x\")); }",
);

/// Ruling PR-N2 (#860), the packet side of B9.1: in coro a Future's duty reaches a packet through `service.absorb`.
/// The call of the async slot `Taking.take` borrows its bare Future, so a call packet that absorbed that Future's
/// cache-state is refused, although the argument stays a borrow.
#[test]
fn f1_an_async_slot_call_packet_holds_no_duty_of_its_bare_future() {
    let mut coro = super::super::async_tests::high_to_coro(super::lower_std(TAKING).unwrap())
        .unwrap_or_else(|error| panic!("{error}"));
    let main = coro.functions.iter().find(|(_, function)| function.symbol == "main").map(|(id, _)| id).unwrap();
    let body = coro.functions.get_mut(main).unwrap().body.as_mut().unwrap();
    let (call_block, packet) = body.blocks.iter().find_map(|(id, block)| match &block.term.kind {
        ir::TermKind::InvokeOwn { op, .. } => match &**op {
            ir::Operation::Call(ir::CallOp::Role { call, .. }) => Some((id, call.packet.unwrap())),
            _ => None,
        },
        _ => None,
    }).expect("the role call");
    let ir::Definition::InstResult { inst: empty, .. } = body.values.get(packet).unwrap().def else {
        panic!("the call packet is an instruction result")
    };
    let cache = body.insts.iter().find_map(|(_, instruction)| match instruction.op {
        ir::Operation::Async(ir::AsyncOp::FutureNew { .. }) => Some(instruction.results[1]),
        _ => None,
    }).expect("the Future's cache-state");
    let ty = body.values.get(packet).unwrap().ty;
    let absorb = body.insts.alloc(body.insts.get(empty).unwrap().clone()).unwrap();
    let absorbed = body.values.alloc(ir::ValueData { ty, def: ir::Definition::InstResult { inst: absorb, index: 0 },
        facts: body.values.get(packet).unwrap().facts.clone() }).unwrap();
    let instruction = body.insts.get_mut(absorb).unwrap();
    instruction.results = vec![absorbed];
    instruction.op = ir::Operation::Async(ir::AsyncOp::ServiceAbsorb { packet, token: cache });
    let block = body.blocks.get_mut(call_block).unwrap();
    let at = block.insts.iter().position(|inst| *inst == empty).expect("the empty packet in the call's block") + 1;
    block.insts.insert(at, absorb);
    let ir::TermKind::InvokeOwn { op, .. } = &mut block.term.kind else { unreachable!() };
    let ir::Operation::Call(ir::CallOp::Role { call, .. }) = &mut **op else { unreachable!() };
    call.packet = Some(absorbed);
    let verify = ir::verify::VerifyOptions { mode: ir::verify::VerifyMode::Full, ..Default::default() };
    let catalog = super::P1NativeSummaries::for_module(&coro).unwrap();
    let context = ir::verify::VerifyContext { text_locations: None, external_contracts: &catalog };
    let report = ir::verify::verify_coro_with(&coro, &verify, context).expect_err("the absorbed duty is refused");
    assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.rule == ir::verify::RuleId::B91
        && diagnostic.message.contains("in its packet")), "{report:?}");
}

/// An async slot with two fresh resource inputs.
const PAIRING: &str = concat!(
    "module app v1.0.0; import \"io/stdout\" take Stdout; ",
    "object Res { has l is string; Res(:l); n() is int { l.length(); } destroy { Stdout().writeln(l); } } ",
    "role Pairing { async pair(a is Res, b is Res) is int; } ",
    "object P does Pairing { P(); async pair(a is Res, b is Res) is int { a.n() + b.n(); } } ",
    "async main() is int { let p is Pairing = P(); await p.pair(Res(l: \"one\"), Res(l: \"two\")); }",
);

/// The `deps.join` of a creation thunk and the `fut.new` that names it.
fn thunk_join(own: &ir::Module, thunk: ir::FuncId) -> (ir::InstId, ir::InstId) {
    let body = own.functions.get(thunk).unwrap().body.as_ref().unwrap();
    let find = |wanted: fn(&ir::Operation) -> bool| body.insts.iter().find(|(_, inst)| wanted(&inst.op))
        .map(|(id, _)| id);
    let join = find(|op| matches!(op, ir::Operation::Ownership(ir::OwnershipOp::DepsJoin { .. }))).expect("deps.join");
    let creation = find(|op| matches!(op, ir::Operation::Async(ir::AsyncOp::FutureNew { .. }))).expect("fut.new");
    (join, creation)
}

/// #863 (ruling PR-N2, third amendment): a creation thunk with two managed inputs depends on the union of their deps,
/// a `deps.join` of the two the prologue gives, and passes its entry packet to `fut.new` unchanged.
#[test]
fn f1_a_creation_thunk_with_two_inputs_depends_on_their_joined_deps() {
    let (own, _) = super::own_case_tests::own_stage(PAIRING, None, ir::verify::VerifyMode::Full)
        .unwrap_or_else(|error| panic!("{error}"));
    let thunk = witness(&own, "P", 0);
    let body = own.functions.get(thunk).unwrap().body.as_ref().unwrap();
    let (join, creation) = thunk_join(&own, thunk);
    let ir::Operation::Ownership(ir::OwnershipOp::DepsJoin { deps }) = &body.insts.get(join).unwrap().op else {
        unreachable!()
    };
    assert_eq!(deps.len(), 2, "the deps of both inputs");
    let joined = body.insts.get(join).unwrap().results[0];
    let ir::Operation::Async(ir::AsyncOp::FutureNew { deps: named, .. }) = &body.insts.get(creation).unwrap().op else {
        unreachable!()
    };
    assert_eq!(*named, joined, "the Future depends on the union");
    assert!(!body.insts.iter().any(|(_, inst)| matches!(inst.op, ir::Operation::Ownership(
        ir::OwnershipOp::Split { .. } | ir::OwnershipOp::Join { .. }))), "the entry packet is not restructured");
    let text = ir::text::print(&own).unwrap();
    assert!(text.contains(" = deps.join [%"), "deps.join prints by name");
    assert!(ir::text::normalized_eq(&own, &ir::text::parse(&text).unwrap()).unwrap(), "and parses back");
    full_low(PAIRING);
}

/// #863: `deps.join` takes deps values only (operand typing).
#[test]
fn f1_a_deps_join_joins_deps_values_only() {
    let (own, _) = super::own_case_tests::own_stage(PAIRING, None, ir::verify::VerifyMode::Full)
        .unwrap_or_else(|error| panic!("{error}"));
    let thunk = witness(&own, "P", 0);
    let (join, _) = thunk_join(&own, thunk);
    let mut typed = own.clone();
    let body = typed.functions.get_mut(thunk).unwrap().body.as_mut().unwrap();
    let receiver = body.blocks.get(body.entry).unwrap().params[0];
    let ir::Operation::Ownership(ir::OwnershipOp::DepsJoin { deps }) = &mut body.insts.get_mut(join).unwrap().op else {
        unreachable!()
    };
    deps[1] = receiver;
    let report = full_report(&typed);
    assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.rule == ir::verify::RuleId::T1), "{report:?}");
}

/// #863 (ruling PR-N2, third amendment): a creation thunk never restructures its unaccepted entry packet. A split of
/// it before `fut.new`, joined straight back, is evaluation before acceptance (B5.1); only `deps.of` and `deps.join`
/// may come first.
#[test]
fn f1_a_creation_thunk_does_not_split_its_entry() {
    let (own, _) = super::own_case_tests::own_stage(PAIRING, None, ir::verify::VerifyMode::Full)
        .unwrap_or_else(|error| panic!("{error}"));
    let thunk = witness(&own, "P", 0);
    let (join, creation) = thunk_join(&own, thunk);
    let mut split = own.clone();
    let body = split.functions.get_mut(thunk).unwrap().body.as_mut().unwrap();
    let ir::Operation::Async(ir::AsyncOp::FutureNew { packet: entry, .. }) = body.insts.get(creation).unwrap().op
    else { unreachable!() };
    let template = body.insts.get(join).unwrap().clone();
    let packet = body.values.get(entry).unwrap().clone();
    let deps = body.values.get(template.results[0]).unwrap().clone();
    let selection = body.selections.alloc(ir::SelectionExpr::Union(vec![])).unwrap();
    let parts = body.insts.alloc(template.clone()).unwrap();
    let joined = body.insts.alloc(template).unwrap();
    let mut value = |data: &ir::ValueData, inst, index| body.values.alloc(ir::ValueData {
        ty: data.ty, def: ir::Definition::InstResult { inst, index }, facts: data.facts.clone() }).unwrap();
    let results = vec![value(&packet, parts, 0), value(&packet, parts, 1), value(&deps, parts, 2)];
    let whole = value(&packet, joined, 0);
    let instruction = body.insts.get_mut(parts).unwrap();
    instruction.op = ir::Operation::Ownership(ir::OwnershipOp::Split { packet: entry, selection });
    instruction.results = results.clone();
    let instruction = body.insts.get_mut(joined).unwrap();
    instruction.op = ir::Operation::Ownership(ir::OwnershipOp::Join { packets: results[..2].to_vec() });
    instruction.results = vec![whole];
    let ir::Operation::Async(ir::AsyncOp::FutureNew { packet, .. }) = &mut body.insts.get_mut(creation).unwrap().op
    else { unreachable!() };
    *packet = whole;
    let (_, block) = body.blocks.iter_mut().find(|(_, block)| block.insts.contains(&creation)).unwrap();
    let at = block.insts.iter().position(|inst| *inst == creation).unwrap();
    block.insts.splice(at..at, [parts, joined]);
    let report = full_report(&split);
    assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.rule == ir::verify::RuleId::B51
        && diagnostic.message == "callee must accept fresh inputs before evaluating its body"), "{report:?}");
}

/// #864, the counterpart of B32 at a call: `fut.new`'s deps cover the full deps of every argument its target's creation
/// summary takes. A two-input thunk whose Future names the deps of its first input only is refused.
#[test]
fn f1_a_future_names_the_deps_of_every_managed_argument() {
    let (own, _) = super::own_case_tests::own_stage(PAIRING, None, ir::verify::VerifyMode::Full)
        .unwrap_or_else(|error| panic!("{error}"));
    let thunk = witness(&own, "P", 0);
    let (join, creation) = thunk_join(&own, thunk);
    let mut one = own.clone();
    let body = one.functions.get_mut(thunk).unwrap().body.as_mut().unwrap();
    let ir::Operation::Ownership(ir::OwnershipOp::DepsJoin { deps }) = &body.insts.get(join).unwrap().op else {
        unreachable!()
    };
    let first = deps[0];
    let creation = &mut body.insts.get_mut(creation).unwrap().op;
    let ir::Operation::Async(ir::AsyncOp::FutureNew { deps: named, .. }) = creation else { unreachable!() };
    *named = first;
    let report = full_report(&one);
    assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.rule == ir::verify::RuleId::B32
        && diagnostic.message == "fut.new deps omit part of a managed argument's full dependencies"), "{report:?}");
}

/// #864 (ruling (ii), OIR-2-async-remaining §6.3c): a creation thunk's Future depends on exactly the inputs its
/// target's creation summary calls managed. `Net.load` takes only plain inputs, so the thunk names the empty
/// `deps.join []`, not the deps of its receiver, whose witness entry declares it managed for transport.
#[test]
fn f1_a_thunk_over_plain_inputs_names_no_deps() {
    let (own, _) = super::own_case_tests::own_stage(BOUND, None, ir::verify::VerifyMode::Full)
        .unwrap_or_else(|error| panic!("{error}"));
    let load = |target: &ir::FuncId| own.functions.get(*target).unwrap().symbol == "app.Net.load#0";
    let thunks: Vec<_> = own.functions.iter().filter(|(_, function)| function.attrs.iter().any(|attr| {
        matches!(attr, ir::FunctionAttr::Adapter { target } if load(target))
    })).collect();
    assert!(!thunks.is_empty(), "the witness thunks of Net.load");
    for (_, function) in thunks {
        let body = function.body.as_ref().unwrap();
        let named = body.insts.iter().find_map(|(_, inst)| match inst.op {
            ir::Operation::Async(ir::AsyncOp::FutureNew { deps, .. }) => Some(deps),
            _ => None,
        }).expect("the thunk's fut.new");
        let ir::Definition::InstResult { inst, .. } = body.values.get(named).unwrap().def else {
            panic!("{}: the deps are an instruction's", function.symbol)
        };
        assert!(matches!(&body.insts.get(inst).unwrap().op,
            ir::Operation::Ownership(ir::OwnershipOp::DepsJoin { deps }) if deps.is_empty()), "{}", function.symbol);
    }
}

/// A function that returns a cold Future of its parameter (#862's shape).
const STARTER: &str = concat!(
    "module app v1.0.0; import \"io/stdout\" take Stdout; ",
    "object Res { has l is string; Res(:l); n() is int { l.length(); } destroy { Stdout().writeln(l); } } ",
    "object Feeder { Feeder(); async feed(r is Res) is int { r.n(); } } ",
    "object Starter { Starter(); start(r is Res) is Future<int> { Feeder().feed(r); } } ",
    "async main() is int { let f = Starter().start(Res(l: \"s\")); await f; }",
);

/// Parameters that only may leave with the result.
const PICKS: &str = concat!(
    "module app v1.0.0; import \"io/stdout\" take Stdout; ",
    "object Res { has l is string; Res(:l); n() is int { l.length(); } destroy { Stdout().writeln(l); } } ",
    "object P { P(); pick(a is Res, b is Res, c is bool) is Res { if c { a; } else { b; } } ",
    "maybe(r is Res, c is bool) is Res { if c { r; } else { Res(l: \"new\"); } } } ",
    "main() is int { P().pick(Res(l: \"a\"), Res(l: \"b\"), true).n() + P().maybe(Res(l: \"x\"), false).n(); }",
);

/// #862 (V2, OIR-2-async-remaining §6.3c, narrowed): a parameter declared taken into the result that only may leave
/// with it, as in `pick` and `maybe`, stays in the callee on the other exits; the results do not depend on it there.
#[test]
fn f1_a_parameter_that_may_leave_with_the_result_stays_on_other_exits() {
    super::own_case_tests::own_stage(PICKS, None, ir::verify::VerifyMode::Full)
        .unwrap_or_else(|error| panic!("{error}"));
}

/// #862 (V2 and ruling (a), §6.3c): `Starter.start` returns a Future of `feed(r)`, which depends on `r`, and `r`
/// leaves beside it. An exit that names only the Future and extends nothing leaves `r` in the callee, which closes it
/// while the Future depends on it.
#[test]
fn f1_a_returned_future_takes_back_the_parameter_it_depends_on() {
    let (own, _) = super::own_case_tests::own_stage(STARTER, None, ir::verify::VerifyMode::Full)
        .unwrap_or_else(|error| panic!("{error}"));
    let (start, _) = function(&own, "app.Starter.start#0");
    let mut cut = own.clone();
    let body = cut.functions.get_mut(start).unwrap().body.as_mut().unwrap();
    let empty = body.insts.iter()
        .find(|(_, inst)| matches!(inst.op, ir::Operation::Ownership(ir::OwnershipOp::Empty {})))
        .map(|(id, inst)| (id, inst.clone())).expect("an own.empty");
    let prepare = body.insts.iter().find_map(|(id, inst)| match &inst.op {
        ir::Operation::Exit(ir::ExitOp::Prepare { exit }) => Some((id, exit.value.unwrap())),
        _ => None,
    }).expect("the exit");
    let only = body.selections.alloc(ir::SelectionExpr::Value(prepare.1)).unwrap();
    let ir::Operation::Exit(ir::ExitOp::Prepare { exit }) = &mut body.insts.get_mut(prepare.0).unwrap().op else {
        unreachable!()
    };
    exit.selection = only;
    let extend = body.insts.iter().find(|(_, inst)| matches!(inst.op, ir::Operation::Exit(ir::ExitOp::Extend { .. })))
        .map(|(id, _)| id).expect("the extension");
    let fresh = body.insts.alloc(empty.1.clone()).unwrap();
    let data = body.values.get(empty.1.results[0]).unwrap().clone();
    let packet = body.values.alloc(ir::ValueData { def: ir::Definition::InstResult { inst: fresh, index: 0 }, ..data })
        .unwrap();
    body.insts.get_mut(fresh).unwrap().results = vec![packet];
    let ir::Operation::Exit(ir::ExitOp::Extend { packet: extended, .. }) = &mut body.insts.get_mut(extend).unwrap().op
    else { unreachable!() };
    *extended = packet;
    let (_, block) = body.blocks.iter_mut().find(|(_, block)| block.insts.contains(&extend)).unwrap();
    let at = block.insts.iter().position(|inst| *inst == extend).unwrap();
    block.insts.insert(at, fresh);
    let _ = empty.0;
    let report = full_report(&cut);
    assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.rule == ir::verify::RuleId::B51
        && diagnostic.message == "a parameter declared taken into the result stays in the callee"), "{report:?}");
}

/// A synchronous function that returns a Future created over a fresh resource (#862, ruling (b) of §6.3c).
const FRESH_RETURN: &str = concat!(
    "module app v1.0.0; ",
    "object Res { has l is string; Res(:l); n() is int { l.length(); } destroy { } } ",
    "object Feeder { Feeder(); async feed(r is Res) is int { r.n(); } } ",
    "object Maker { Maker(); start() is Future<int> { Feeder().feed(Res(l: \"made\")); } } ",
    "async main() is int { await Maker().start(); }",
);

/// Ruling (b) of §6.3c (#862): `create_future` never splits a packet; a fresh input's `deps.of` names its own
/// identity, and the verifier subtracts what the Future's inputs packet owns. So a returned Future over a fresh
/// resource depends on nothing its function did not receive: the result relation of `start` declares no external
/// dependency, and Full accepts it. Without the subtraction the Future would seem to depend on the fresh creation,
/// which the relation cannot declare.
#[test]
fn f1_a_returned_future_over_a_fresh_input_declares_no_dependency_on_it() {
    let (own, _) = super::own_case_tests::own_stage(FRESH_RETURN, None, ir::verify::VerifyMode::Full)
        .unwrap_or_else(|error| panic!("{error}"));
    let (_, start) = function(&own, "app.Maker.start#0");
    let summary = own.summaries.get(start.summary.unwrap()).unwrap();
    let [relation] = summary.results.as_slice() else { panic!("{summary:?}") };
    assert!(relation.external_deps.is_empty(), "{relation:?}");
    let body = start.body.as_ref().unwrap();
    let splits = body.insts.iter().filter(|(_, instruction)| {
        matches!(instruction.op, ir::Operation::Ownership(ir::OwnershipOp::Split { .. }))
    }).count();
    assert_eq!(splits, 0, "create_future restructures no packet");
}

/// #865 (OIR-2-async-remaining §6.3d): `make` returns a Future over its local binding `r`, which stays beside the
/// Future in the result packet (a binding is never moved into a Future, ruling (iii) of §6.3c).
const LOCAL_BESIDE: &str = concat!(
    "module app v1.0.0; ",
    "object Res { has l is string; Res(:l); n() is int { l.length(); } destroy { } } ",
    "object U { U(); async use(r is Res) is int { r.n(); } } ",
    "object Mk { Mk(); make() is Future<int> { let r = Res(l: \"local\"); U().use(r); } } ",
    "async main() is int { let f = Mk().make(); await f; }",
);

fn relation_of<'m>(own: &'m ir::Module, symbol: &str) -> (ir::SummaryId, &'m ir::ResultRelation) {
    let (_, function) = function(own, symbol);
    let summary = function.summary.unwrap();
    (summary, &own.summaries.get(summary).unwrap().results[0])
}

/// #865: own declares the local's creation beside the returned Future, with its type, and Full accepts the exit that
/// carries it.
#[test]
fn f1_a_returned_future_declares_its_local_input_beside_it() {
    let (own, _) = super::own_case_tests::own_stage(LOCAL_BESIDE, None, ir::verify::VerifyMode::Full)
        .unwrap_or_else(|error| panic!("{error}"));
    let (_, relation) = relation_of(&own, "app.Mk.make#0");
    let [ir::BesideEntry::Site { ty, .. }] = relation.beside.as_slice() else { panic!("{relation:?}") };
    assert!(matches!(own.types.get(*ty), Ok(ir::Type::Object { .. })), "{relation:?}");
}

/// #865: the marker is what lets the exit carry the local beside the Future: without it, the Future's selection
/// names a creation its relation does not declare beside it.
#[test]
fn f1_a_local_beside_a_future_needs_its_marker() {
    let (own, _) = super::own_case_tests::own_stage(LOCAL_BESIDE, None, ir::verify::VerifyMode::Full)
        .unwrap_or_else(|error| panic!("{error}"));
    let (summary, _) = relation_of(&own, "app.Mk.make#0");
    let mut cut = own.clone();
    cut.summaries.get_mut(summary).unwrap().results[0].beside.clear();
    let report = full_report(&cut);
    assert!(reports(&report, ir::verify::RuleId::B101), "{report:?}");
}

/// #865: a `beside` entry belongs to a bare Future result only.
#[test]
fn f1_only_a_future_result_has_entries_beside_it() {
    let (own, _) = super::own_case_tests::own_stage(LOCAL_BESIDE, None, ir::verify::VerifyMode::Full)
        .unwrap_or_else(|error| panic!("{error}"));
    let (_, use_) = function(&own, "app.Res.n#0");
    let summary = use_.summary.unwrap();
    let mut cut = own.clone();
    let ty = cut.types.iter().find(|(_, ty)| matches!(ty, ir::Type::Object { .. })).map(|(id, _)| id).unwrap();
    let result = &mut cut.summaries.get_mut(summary).unwrap().results[0];
    result.beside.push(ir::BesideEntry::Site { site: ir::CreationSiteId::from_u32(0), ty });
    let report = full_report(&cut);
    assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.rule == ir::verify::RuleId::B51
        && diagnostic.message == "only a bare Future result declares entries beside it"), "{report:?}");
}

/// #865 (§6.3d, fail-closed): a caller that returns such a Future again has no value naming what came beside it, so
/// its exit cannot carry it, and Full refuses the return.
#[test]
fn f1_a_future_with_a_local_beside_it_is_not_returned_further() {
    let source = LOCAL_BESIDE.replace("async main() is int { let f = Mk().make(); await f; }",
        "object P { P(); pass() is Future<int> { Mk().make(); } } async main() is int { await P().pass(); }");
    let Err(error) = super::own_case_tests::own_stage(&source, None, ir::verify::VerifyMode::Full) else {
        panic!("Full refuses the second return");
    };
    // The returned Future depends on the `Res` beside it, which `pass` closes.
    assert!(error.contains("B31 use of a value whose stable dependency has been closed at app.P.pass#0"), "{error}");
}
