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

w1_loads! {
    oir2_w1_await_in_async_main_loads_as_spb17 => "L08.await.in-async-main",
    oir2_w1_failure_rethrown_at_every_await_loads_as_spb17 => "L08.failure.rethrown-at-every-await",
    oir2_w1_future_lazy_and_cached_loads_as_spb17 => "L08.future.lazy-and-cached",
    oir2_w1_future_never_awaited_loads_as_spb17 => "L08.future.never-awaited",
    oir2_w1_future_passed_and_awaited_elsewhere_loads_as_spb17 => "L08.future.passed-and-awaited-elsewhere",
    oir2_w1_order_across_suspension_loads_as_spb17 => "L08.order.across-suspension",
    oir2_w1_order_lazy_branches_loads_as_spb17 => "L08.order.lazy-branches",
    oir2_w1_void_await_for_effect_loads_as_spb17 => "L08.void.await-for-effect",
}

/// The full verifier is the gate profile: the largest W1 body, `main` of L08.order.lazy-branches (79.4M steps in
/// Low), builds under it within the gate's limit, above the default 50M (#693).
#[test]
fn oir2_w1_order_lazy_branches_builds_under_the_full_gate() {
    assert!(o::verify::GATE_STEPS > o::verify::VerifyLimits::default().steps);
    spb17("L08.order.lazy-branches", o::VerifyMode::Full).unwrap_or_else(|error| panic!("{error}"));
}

/// #772: an async body that passes its Future parameter to a nested async call. The nested creation retains the
/// parameter for its inputs holder, so the outer body's summary retains it too; an async body's only SPB parameter
/// is its frame, so its entry contract retains none, and the program builds and loads under both gates.
#[test]
fn oir2_772_an_async_body_passes_its_future_parameter_on() {
    let source = concat!(
        "module app; import \"io/stdout\" take Stdout; ",
        "object Child { Child(); async run() is int { 42; } } ",
        "object Worker { Worker(); async inner(child is Future<int>) is int { await child; } ",
        "async run(child is Future<int>) is int { await self.inner(child); } } ",
        "async main() is int { let child = spawn Child().run(); ",
        "Stdout().writeln((await Worker().run(child)).string()); 0; }",
    );
    for mode in [o::VerifyMode::Structural, o::VerifyMode::Full] {
        let high = super::super::tests::lower_std(source).unwrap_or_else(|error| panic!("{error}"));
        let run = high.functions.iter().find(|(_, function)| function.symbol.contains("Worker.run")).unwrap().1;
        let summary = high.summaries.get(run.async_body_summary.unwrap()).unwrap();
        assert_eq!(summary.parameters.len(), 2, "the receiver and the Future");
        let bytes = compile_high_with(high, mode).unwrap_or_else(|error| panic!("{error}"));
        let program = s::decode(&bytes).unwrap();
        let contracts = s::p1::P1Contracts::new(&program.objects).unwrap();
        s::verify_load(&program, &contracts).unwrap_or_else(|error| panic!("{error}"));
        for (symbol, _) in async_bodies(&program) {
            let function = program.functions.iter().find(|function| function.symbol == symbol).unwrap();
            assert_eq!(function.proofs.entry.as_ref().unwrap().retained_params, [] as [u32; 0], "{symbol}");
        }
    }
}

/// A Cell over a plain role (P2.4 §4.8, #764): the registered Atomic keeps its dotted name with the role's
/// declaration in its suffix, holds a role reference and its closed flag, and the program loads.
#[test]
fn oir2_pipeline_registers_an_atomic_over_a_plain_role() {
    let high = super::super::tests::lower_std(concat!("module app; import \"cell\" take Cell; ",
        "role Shape { area() is int; } object Square does Shape { has side is int; Square(:side); ",
        "area() is int { side * side; } } ",
        "main() is int { let first is Shape = Square(side: 3); let held = Cell(first); held.value().area(); }"))
        .unwrap_or_else(|error| panic!("{error}"));
    let bytes = compile_high_with(high, o::VerifyMode::Structural).unwrap_or_else(|error| panic!("{error}"));
    let program = s::decode(&bytes).unwrap();
    let role = program.objects.iter().position(|object| object.name == "app.Shape").unwrap() as u32;
    let atomic = program.objects.iter().find(|object| object.name == "async.atomic.Atomic_role_app.Shape")
        .expect("the registered Atomic over the role");
    assert_eq!(atomic.layout.storage_fields, [s::Type::Role(role), s::Type::Bool]);
    assert_eq!(atomic.layout.trace_layout, [0]);
    assert!(!atomic.layout.may_reach_resource);
}

/// A Cell over a role one of whose implementations is a resource is the slice P2.4 §4.8 leaves out: refused by
/// name before SPB 17.
#[test]
fn oir2_pipeline_refuses_an_atomic_over_a_role_with_a_resource() {
    let high = super::super::tests::lower_std(concat!("module app; import \"cell\" take Cell; ",
        "role Named { name() is string; } object Res does Named { has n is string; Res(:n); ",
        "name() is string { n; } destroy { n; } } ",
        "main() is int { let first is Named = Res(n: \"a\"); let held = Cell(first); held.value().name().length(); }"))
        .unwrap_or_else(|error| panic!("{error}"));
    let error = compile_high_with(high, o::VerifyMode::Structural).unwrap_err().to_string();
    // #778 words the refusal for every closed plain value; the role is named by its suffix.
    assert!(error.contains("Atomic/Cell over role_app.Named") && error.contains("closed plain data only"), "{error}");
}

/// #723: an object that holds an action value whose closures capture only plain data is no identity. Own gives
/// its construction no fresh creation, the full verifier counts the function type's obligations by the same
/// closures, and the loader finds no fresh result without a creation (FRESH_ORIGIN).
#[test]
fn oir2_pipeline_a_holder_of_plain_closures_is_no_identity() {
    let source = concat!("module app; object Filter { has keep is (int) is bool; Filter(:keep); ",
        "passes(value is int) is bool { keep(value); } } ",
        "object Sets { Sets(); ",
        "above(limit is int) is Filter { Filter(keep: (value is int) is bool { value > limit; }); } } ",
        "main() is int { let f = Sets().above(3); if f.passes(5) { 1; } else { 0; } }");
    for mode in [o::VerifyMode::Structural, o::VerifyMode::Full] {
        let high = super::super::tests::lower_std(source).unwrap_or_else(|error| panic!("{error}"));
        compile_high_with(high, mode).unwrap_or_else(|error| panic!("{mode:?}: {error}"));
    }
}


/// #778: a Cell over a map of plain data registers an Atomic named by the encoded type, whose storage holds the
/// map and the closed flag, and the program loads.
#[test]
fn oir2_pipeline_registers_an_atomic_over_a_plain_map() {
    let high = super::super::tests::lower_std(concat!("module app; import \"cell\" take Cell; ",
        "main() is int { let counts = Cell([\"a\": 1]); counts.replace([\"a\": 1, \"b\": 2]); ",
        "counts.value().size(); }")).unwrap_or_else(|error| panic!("{error}"));
    let bytes = compile_high_with(high, o::VerifyMode::Structural).unwrap_or_else(|error| panic!("{error}"));
    let program = s::decode(&bytes).unwrap();
    let atomic = program.objects.iter().find(|object| object.name.starts_with("async.atomic.Atomic_t_"))
        .expect("the registered Atomic over the map");
    assert!(matches!(atomic.layout.storage_fields.as_slice(), [s::Type::Map(_), s::Type::Bool]), "{atomic:?}");
}

/// #778 leaves Futures out (D25): an Atomic over an array of Futures stays a producer refusal.
#[test]
fn oir2_pipeline_refuses_an_atomic_over_futures() {
    let source = concat!("module app; import \"async/atomic\" take Atomic; ",
        "object Job { Job(); async run() is int { 1; } } ",
        "main() is int { let pending is @Future<int> = []; let held = Atomic(pending); held.close(); 0; }");
    let error = super::super::tests::lower_std(source).map(|_| ()).unwrap_err().to_string();
    assert!(error.contains("std object async/atomic.Atomic"), "{error}");
}

/// #777: an action value's role is implemented by the backend's closure layouts, so a holder of a closure that
/// captures a resource reaches that resource; its layout says so (the loader's OBJECT_RETENTION). A holder of a
/// closure that captures plain data does not.
#[test]
fn oir2_pipeline_a_holder_reaches_what_its_closure_captures() {
    let holder = |captured: &str| {
        let source = format!(concat!("module app; ",
            "object Res {{ has n is string; Res(:n); size() is int {{ n.length(); }} ",
            "destroy {{ n; }} }} object Plain {{ has n is string; Plain(:n); size() is int {{ n.length(); }} }} ",
            "object Filter {{ has keep is (int) is bool; Filter(:keep); ",
            "passes(value is int) is bool {{ keep(value); }} }} ",
            "main() is int {{ {} let f = Filter(keep: (value is int) is bool {{ value > r.size(); }}); ",
            "if f.passes(5) {{ 1; }} else {{ 0; }} }}"), captured);
        let high = super::super::tests::lower_std(&source).unwrap_or_else(|error| panic!("{error}"));
        let bytes = compile_high_with(high, o::VerifyMode::Structural).unwrap_or_else(|error| panic!("{error}"));
        let program = s::decode(&bytes).unwrap();
        program.objects.iter().find(|object| object.name == "app.Filter").unwrap().layout.may_reach_resource
    };
    assert!(holder("let r = Res(n: \"abc\");"));
    // The plain capture: a `Res`-free value with the same `size()` shape.
    assert!(!holder("let r = Plain(n: \"abc\");"));
}
