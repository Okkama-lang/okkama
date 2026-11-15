//! The source pipeline of plain async (#601 V7, plan §4 V10): own → coro → verify_coro → low, then SPB 17 and
//! its load check, for the eight W1 cases of the plan.
use super::*;
use super::super::case_tests::case_high;

/// A W1 case from its source through the build pipeline in `mode`, then its SPB 17 decoded and loaded again.
fn spb17(id: &str, mode: o::VerifyMode) -> Result<s::Program, String> {
    let high = case_high(id).map_err(|error| format!("{id}: {error}"))?;
    let bytes = compile_high_with(high, mode).map_err(|error| format!("{id}: {error}"))?;
    let program = s::decode(&bytes).map_err(|error| format!("{id}: decode: {error}"))?;
    let contracts = s::p1::P1Contracts::new(&program.objects).map_err(|error| format!("{id}: {error}"))?;
    s::verify_load(&program, &contracts).map_err(|error| format!("{id}: verify_load: {error}"))?;
    Ok(program)
}

/// The async bodies of a program: their symbols, with the frame layout each names.
fn async_bodies(program: &s::Program) -> Vec<(&str, &s::FrameLayout)> {
    program.functions.iter().filter_map(|function| match function.entry_abi {
        s::EntryAbi::AsyncBody { layout } => Some((function.symbol.as_str(), &program.tables.frames[layout as usize])),
        _ => None,
    }).collect()
}

/// An async action reaches SPB 17 only through coro: low takes no async body from own.
#[test]
fn oir2_pipeline_lowers_async_bodies_through_coro() {
    for mode in [o::VerifyMode::Structural, o::VerifyMode::Full] {
        let program = spb17("L08.future.never-awaited", mode).unwrap_or_else(|error| panic!("{error}"));
        let bodies = async_bodies(&program);
        assert_eq!(bodies.len(), 1, "Exploder.run is the one async body: {bodies:?}");
        let (_, layout) = bodies[0];
        // The receiver, then the inputs packet (C-2).
        assert_eq!(layout.continuations[0].saved.len(), 2);
        assert_eq!(program.tables.future_results, [Some(s::Type::Int)]);
        // main stays synchronous: it creates the Future and settles its cache duty at its close.
        let main = &program.functions[program.entry.unwrap() as usize];
        assert_eq!(main.entry_abi, s::EntryAbi::Observation);
        let settles = main.code.iter().filter(|instruction| matches!(instruction,
            s::Instruction::Champ { op: s::Opcode::CleanupSettle, arguments, .. } if arguments.len() == 3)).count();
        assert!(settles >= 1, "the cache duty is settled at main's close");
    }
}

macro_rules! w1_loads {
    ($($test:ident => $id:literal,)*) => {
        $(
            #[test]
            fn $test() {
                spb17($id, o::VerifyMode::Structural).unwrap_or_else(|error| panic!("{error}"));
            }
        )*
    };
}

/// The W1 cases whose async `main` needs the loader to accept a fixed root's start without an input packet and a
/// deferred cancellation delivered after the first plan of a final close (V7 report).
macro_rules! w1_loads_blocked {
    ($($test:ident => $id:literal,)*) => {
        $(
            #[test]
            #[ignore = "loader: async root start without input packet; deliver_cancel after the first plan (V7 report)"]
            fn $test() {
                spb17($id, o::VerifyMode::Structural).unwrap_or_else(|error| panic!("{error}"));
            }
        )*
    };
}

w1_loads! {
    oir2_w1_future_never_awaited_loads_as_spb17 => "L08.future.never-awaited",
}

w1_loads_blocked! {
    oir2_w1_await_in_async_main_loads_as_spb17 => "L08.await.in-async-main",
    oir2_w1_failure_rethrown_at_every_await_loads_as_spb17 => "L08.failure.rethrown-at-every-await",
    oir2_w1_future_lazy_and_cached_loads_as_spb17 => "L08.future.lazy-and-cached",
    oir2_w1_future_passed_and_awaited_elsewhere_loads_as_spb17 => "L08.future.passed-and-awaited-elsewhere",
    oir2_w1_order_across_suspension_loads_as_spb17 => "L08.order.across-suspension",
    oir2_w1_order_lazy_branches_loads_as_spb17 => "L08.order.lazy-branches",
    oir2_w1_void_await_for_effect_loads_as_spb17 => "L08.void.await-for-effect",
}
