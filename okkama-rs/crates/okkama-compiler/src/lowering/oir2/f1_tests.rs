//! #601 F1 in the producer and own: async role slots and bound async action values (ruling PR-D).
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

