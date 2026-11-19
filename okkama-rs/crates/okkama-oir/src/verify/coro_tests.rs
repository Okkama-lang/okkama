//! Variants of the V1 async fixtures for the structural coro rules (D20, D21, D22).
use crate::*;
use super::super::*;

const AWAIT: &str = include_str!("../../tests/fixtures/async/valid/d20-await-triple.coro.oir");
const DESTROY: &str = include_str!("../../tests/fixtures/async/valid/d20-instruction-triple.oir");
const LAYOUT: &str = include_str!("../../tests/fixtures/async/valid/d21-layout-roundtrip.oir");
const EXTERNAL: &str = include_str!("../../tests/fixtures/async/valid/d22-external-new.oir");

fn structural(source: &str) -> Vec<RuleId> {
    let module = text::parse(source).unwrap_or_else(|error| panic!("{error}"));
    let options = VerifyOptions { mode: VerifyMode::Structural, ..VerifyOptions::default() };
    match verify_with(&module, &options, VerifyContext::default()) {
        Ok(_) => vec![],
        Err(report) => report.diagnostics.into_iter().map(|diagnostic| diagnostic.rule).collect(),
    }
}

fn edited(source: &str, from: &str, to: &str) -> String {
    assert_eq!(source.matches(from).count(), 1, "{from}");
    source.replace(from, to)
}

#[test]
fn bracket_variants_report_their_stable_codes() {
    let cases = [
        // A suspending terminator without a save in its block.
        (AWAIT, "    invoke.own await", "    %gap: own = own.empty\n    invoke.own await", RuleId::CoroMissingResume),
        // A second save names continuation 1, already used by the await site.
        (AWAIT, "frame_slot(slot=1, value=%name2, mode=borrow)], continuation 2",
            "frame_slot(slot=1, value=%name2, mode=borrow)], continuation 1", RuleId::CoroContinuationMismatch),
        // The declared site of continuation 2 is not the await.unwind instruction.
        (AWAIT, "continuation 2 at #9", "continuation 2 at ^bad", RuleId::CoroContinuationMismatch),
        // Continuation 0 resumes outside the start of the body.
        (AWAIT, "frame.resume %fr4, continuation 2", "frame.resume %fr4, continuation 0",
            RuleId::CoroContinuationMismatch),
        // A saved value of another type than its slot.
        (AWAIT, "frame_slot(slot=1, value=%name2, mode=borrow)", "frame_slot(slot=2, value=%name2, mode=borrow)",
            RuleId::CoroLayoutTypeMismatch),
        // A linear slot saved by borrow.
        (AWAIT, "frame_slot(slot=0, value=%q, mode=consume)", "frame_slot(slot=0, value=%q, mode=borrow)",
            RuleId::CoroLayoutTypeMismatch),
        // An operand slot whose type differs from the operand.
        (AWAIT, "saves [0, 1] operands [2]", "saves [0, 1] operands [1]", RuleId::CoroLayoutTypeMismatch),
        // A continuation that names an undeclared slot.
        (AWAIT, "saves [1] operands [0, 3, 4, 5]", "saves [1] operands [0, 3, 4, 6]",
            RuleId::CoroLayoutTypeMismatch),
        // The await.unwind save is separated from it, so neither is bracketed.
        (AWAIT, "    [i9] (%q3", "    %gap: own = own.empty\n    [i9] (%q3", RuleId::CoroMissingResume),
        // A may-suspend cleanup step is bracketed, but its resume is missing.
        (DESTROY, "    (%fr9: frame<@job>, %x1: str) = frame.resume %fr8,",
            "    %gap: own = own.empty\n    (%fr9: frame<@job>, %x1: str) = frame.resume %fr8,",
            RuleId::CoroMissingResume),
        // A raising async low body.
        (DESTROY, "    fut.publish %fr13, failure(%final)\n    ret\n", "    raise %final\n", RuleId::CoroBodyRaises),
    ];
    for (source, from, to, rule) in cases {
        let rules = structural(&edited(source, from, to));
        assert!(rules.contains(&rule), "{rule:?} for {from:?}: {rules:?}");
    }
}

#[test]
fn successor_of_a_suspending_terminator_has_no_other_predecessor() {
    let source = edited(AWAIT, "    %closed: closed = scope.end %q1,", "    br ^ok(%v, %d)\n^dup(%v9: int) in S:
    %closed: closed = scope.end %q1,");
    // The edit makes ^ok its own predecessor, besides the await edge.
    let rules = structural(&source);
    assert!(rules.contains(&RuleId::CoroMissingResume), "{rules:?}");
}

#[test]
fn rules_without_a_layout_keep_pre_d21_bodies_valid() {
    // An async coro body without a D21 layout predates the async ABI; only the shielded-await
    // and D22 rules apply to it.
    let start = AWAIT.find("    frame.layout {").unwrap();
    let end = AWAIT.find("^entry in S:").unwrap();
    let source = format!("{}{}", &AWAIT[..start], &AWAIT[end..]);
    let source = edited(&source, "    fut.publish %fr5, failure(%combined)\n    ret\n", "    raise %combined\n");
    let source = edited(&source, "    %fr4: frame<@main> = frame.save", "    %gap: own = own.empty
    %fr4: frame<@main> = frame.save");
    assert_eq!(structural(&source), []);
    for source in [AWAIT, DESTROY, LAYOUT] {
        assert_eq!(structural(source), []);
    }
}

#[test]
fn external_environment_requires_an_async_external() {
    let source = edited(EXTERNAL, "[async, intrinsic \"fixture.ordinary\"]", "[intrinsic \"fixture.ordinary\"]");
    assert!(structural(&source).contains(&RuleId::D22ExternalWithBody));
}

#[test]
fn layout_machine_words_follow_service_layout_and_scope_area() {
    let module = text::parse(LAYOUT).unwrap();
    let function = FuncId::from_u32(0);
    let layout = module.functions.get(function).unwrap().body.as_ref().unwrap().frame_layout.clone().unwrap();
    let words: Vec<_> = layout.slots.iter()
        .map(|slot| FrameLayout::slot_words(&module.types, slot.ty).unwrap()).collect();
    assert_eq!(words.iter().map(|slot| slot.words).collect::<Vec<_>>(), [1, 1, 2, 0]);
    assert_eq!(words[0].references, None);
    assert_eq!(words[1].references, Some(&[0][..]));
    assert_eq!(words[2].references, Some(&[0][..]));
    assert_eq!(words[3].references, Some(&[][..]));
    assert_eq!(layout.slot_area_words(&module.types), Some(4));
    assert_eq!(FrameLayout::scope_area_words(&module, function), Some(4));
    assert_eq!(layout.machine_words(&module, function), Some(8));
}

// Slice V3: the full verifier over the coro goldens (B9.5 rules and the D21 forms of B5.1, T1, T5, B4.2).
const PLAIN: &str = include_str!("../../tests/fixtures/async/coro/async-plain-await.oir");
const FAILURE: &str = include_str!("../../tests/fixtures/async/coro/async-await-failure.oir");
const REPEATED: &str = include_str!("../../tests/fixtures/async/coro/async-repeated-await.oir");
const OWN_PLAIN: &str = include_str!("../own/tests/async-goldens/async-plain-await.oir");

fn full(source: &str) -> Vec<RuleId> {
    let module = text::parse(source).unwrap_or_else(|error| panic!("{error}"));
    full_module(&module)
}

fn full_module(module: &Module) -> Vec<RuleId> {
    let options = VerifyOptions { execution: ExecutionMode::Tests, ..VerifyOptions::default() };
    match verify_with(module, &options, VerifyContext::default()) {
        Ok(_) => vec![],
        Err(report) => report.diagnostics.into_iter().map(|diagnostic| diagnostic.rule).collect(),
    }
}

#[test]
fn coro_goldens_pass_the_full_verifier() {
    for source in [PLAIN, FAILURE, REPEATED] {
        assert_eq!(full(source), []);
    }
}

#[test]
fn d21_bodies_keep_their_language_signature_in_continuation_zero() {
    let work_body = "summary M4 call-boundary parameters [parameter_effect(convention=borrow, destination=none)] \
        results";
    let cases = [
        // B5.1: the summary results disagree with the published value.
        (PLAIN, "summary M4 call-boundary parameters [parameter_effect(convention=borrow, destination=none)] results \
            [result_relation(provenance=unbound_result)] contract call_contract(results=[int],",
            "summary M4 call-boundary parameters [parameter_effect(convention=borrow, destination=none)] results \
            [result_relation(provenance=unbound_result)] contract call_contract(results=[str],", RuleId::B51),
        // B5.1: the summary has no parameter for the argument restored from continuation 0.
        (PLAIN, work_body, "summary M4 call-boundary results", RuleId::B51),
        // T1: the deferred environment no longer matches the callee's start continuation.
        (PLAIN, "function=@work, arguments=[%4]", "function=@work, arguments=[]", RuleId::T1),
        // B5.1: the published failure has a kind that the body summary does not declare.
        (FAILURE, "summary M4 call-boundary results [result_relation(provenance=unbound_result)] contract \
            call_contract(results=[int], failures=[thrown, cancelled]",
            "summary M4 call-boundary results [result_relation(provenance=unbound_result)] contract \
            call_contract(results=[int], failures=[cancelled]", RuleId::B51),
        // B5.1, ruling 3: a fixed root keeps the observation ABI that its summary's entry kind states.
        (PLAIN, "-> void [entry, async] abi observation summary(M9)", "-> void [entry, async] summary(M9)",
            RuleId::B51),
        // S3, ruling 3: the language signature of the D21 entry publishes its int.
        (PLAIN, "fut.publish %35, success(value=%29, packet=%30, deps=%17)",
            "fut.publish %35, success(packet=%30, deps=%17)", RuleId::S3),
    ];
    for (source, from, to, rule) in cases {
        let from = from.split_whitespace().collect::<Vec<_>>().join(" ");
        let to = to.split_whitespace().collect::<Vec<_>>().join(" ");
        let rules = full(&edited(source, &from, &to));
        assert!(rules.contains(&rule), "{rule:?} for {from:?}: {rules:?}");
    }
}

#[test]
fn b95_variants_report_their_stable_codes() {
    let cases = [
        // D21: the save lists its slots in another order than the declared saved set.
        (PLAIN, "continuation 1 at ^b1 saves [0, 1, 2, 3]", "continuation 1 at ^b1 saves [1, 0, 2, 3]",
            RuleId::CoroLayoutTypeMismatch),
        // D21: the declared saved set of continuation 2 omits the slot that its save fills.
        (PLAIN, "    continuation 2 at #10 saves [3] operands [1, 5, 6, 7]\n",
            "    continuation 2 at #10 saves [] operands [1, 5, 6, 7]\n", RuleId::CoroLayoutTypeMismatch),
        // A read of a saved value at an instruction after its resume.
        (REPEATED, "    br ^b4(%66, %67, %69, %14, %70, %71)",
            "    %dup: int = int.add %12, %12\n    br ^b4(%66, %67, %69, %14, %70, %71)", RuleId::CoroReadAfterSave),
    ];
    for (source, from, to, rule) in cases {
        let rules = full(&edited(source, from, to));
        assert!(rules.contains(&rule), "{rule:?} for {from:?}: {rules:?}");
    }
}

/// The coro output of plain-await whose final close of main declares a conservative suspending envelope.
fn suspending_close() -> Module {
    let close = "invoke.own scope.end %11, effects scope_effects(failures=[], suspends=false, cancel=true)";
    let suspending = "invoke.own scope.end %11, effects scope_effects(failures=[], suspends=true, cancel=true)";
    let source = edited(OWN_PLAIN, close, suspending);
    let own = text::parse(&source).unwrap();
    assert_eq!(full_module(&own), []);
    crate::coro::lower_coro(own, &crate::coro::CoroOptions::default()).unwrap()
}

#[test]
fn the_duties_of_a_suspending_close_are_its_tail_operand_slots() {
    // D20, review B4: a suspending close settles the duties of its owner, so they are operand slots of its
    // continuation after the close's own operands.
    let mut module = suspending_close();
    assert_eq!(full_module(&module), []);
    let main = module.functions.iter().find(|(_, function)| function.symbol == "main").unwrap().0;
    let body = module.functions.get_mut(main).unwrap().body.as_mut().unwrap();
    let close = body.blocks.iter().find_map(|(block, data)| match &data.term.kind {
        TermKind::InvokeOwn { op, .. } => matches!(**op, Operation::Ownership(OwnershipOp::ScopeEnd { .. }))
            .then_some(block),
        _ => None,
    }).unwrap();
    let layout = body.frame_layout.as_mut().unwrap();
    let continuation = layout.continuations.iter_mut()
        .find(|continuation| continuation.site == SuspendSite::Terminator(close)).unwrap();
    let duty = continuation.operands.pop().unwrap();
    assert!(layout.slots[duty as usize].linear);
    assert!(full_module(&module).contains(&RuleId::CoroLayoutTypeMismatch));
}

#[test]
fn a_duty_left_running_across_await_unwind_is_an_incomplete_saved_set() {
    let source = edited(PLAIN, "    continuation 2 at #10 saves [3] operands [1, 5, 6, 7]\n",
        "    continuation 2 at #10 saves [] operands [1, 5, 6, 7]\n");
    let source = edited(&source, "frame.save %43, slots [frame_slot(slot=3, value=%47, mode=consume)], continuation 2",
        "frame.save %43, continuation 2");
    let resume = concat!("(%25: frame<@main>, %26: cache-state) = frame.resume %22, continuation 2, ",
        "slots [saved_slot(slot=3, ty=cache-state, linear=true)]");
    let source = edited(&source, resume, "%25: frame<@main> = frame.resume %22, continuation 2");
    let source = edited(&source, "duties [%26]", "duties [%47]");
    assert_eq!(full(&source), [RuleId::CoroSavedSetIncomplete]);
}

#[test]
fn a_sync_function_has_no_frame_of_its_own() {
    let source = "oir 2 coro
module sync_frame
scope S action in @f
summary Mf internal contract call_contract(publication=atomic)
func @f(%frame: frame<@f>) -> void summary(Mf) {
^entry in S:
    ret
}
";
    assert!(full(source).contains(&RuleId::CoroSyncSuspends));
    assert!(!full(&source.replace("frame<@f>", "frame<@g>").replace("scope S action in @f",
        "scope S action in @f\nscope SG action in @g\nfunc @g() -> void summary(Mf) {\n^entry in SG:\n    ret\n}"))
        .contains(&RuleId::CoroSyncSuspends));
}

const CAUGHT: &str = include_str!("../../tests/fixtures/async/coro/async-caught-throw.oir");

#[test]
fn only_a_failure_that_leaves_a_d21_body_must_be_published() {
    // C-2: a throw that the block unwind catches inside the body is ordinary control flow.
    assert_eq!(structural(CAUGHT), []);
    assert_eq!(full(CAUGHT), []);
    assert_eq!(full(FAILURE), []);
    let caught = "^b2(%7: scope<S0>, %8: Stop facts(plain(certificate=scalar))) in S0";
    for terminator in ["throw %8", "panic %8"] {
        let source = edited(CAUGHT, "    throw %8\n", &format!("    {terminator}\n"));
        assert!(!structural(&source).contains(&RuleId::CoroBodyRaises), "{terminator}");
        // Without its unwind edge the same terminator leaves the body unpublished.
        let source = edited(&source, &format!("{caught} unwind ^b3(%7):"), &format!("{caught}:"));
        assert!(structural(&source).contains(&RuleId::CoroBodyRaises), "{terminator}");
    }
    // The raise after the lexical unwind leaves the body when the pass has not rewritten it.
    let source = edited(CAUGHT, "    fut.publish %1, failure(%11)\n    ret\n", "    raise %11\n");
    assert!(structural(&source).contains(&RuleId::CoroBodyRaises));
}

/// A sync owner that creates a Future of `@task` whose outcome R has a destroy that may throw, and closes
/// its scope with `close`; lowered to coro by the pass (D16: own declares the schedule).
fn cache_outcome(close: &str) -> Module {
    cache_outcome_with("", close)
}

/// [`cache_outcome`] whose owner also takes the entry parameters `params`.
fn cache_outcome_with(params: &str, close: &str) -> Module {
    let source = format!("oir 2 own
module cache_outcome
protocol Services kind services holders [scope, cache] rules [protocol_rule(transition=settle, from=cache, to=scope,
    checks=[owner, live_dependencies], normal=protocol_poststate(holder=scope, state=settled))]
object R {{
    destroy @R.destroy
}}
scope SD action in @R.destroy
scope S action in @f
summary Md internal contract call_contract(failures=[thrown], publication=atomic)
summary Mt internal contract call_contract(results=[R], failures=[cancelled], suspends=true, publication=atomic)
summary Mf internal contract call_contract(failures=[thrown], publication=atomic)
func @R.destroy(%self: R) -> void [destroy R] summary(Md);
func @task() -> R [async] summary(Mt);
func @f({params}) -> void summary(Mf) {{
    service.schedule Schedule0 owner S, protocol Services, duties [%t]
    selection s0: selection = union([])
    key k0: key = service_of(value=%t, kind=cache)
    dependency d0: dependency = key(k0)
^entry in S:
    %q0: scope<S> = scope.begin S
    %e: own = own.empty
    (%l: own, %r: own, %deps: deps) = own.split %e, s0
    %in: own = own.join [%l, %r]
    (%t: future<R> facts(managed(deps=d0)), %q1: scope<S>) = fut.new %in, %deps, %q0, environment deferred(
        function=@task), result R, owner S
{close}
}}
");
    let own = text::parse(&source).unwrap();
    crate::coro::lower_coro(own, &crate::coro::CoroOptions::default()).unwrap()
}

/// The close of `cache_outcome` whose envelope states the cleanup effects of R exactly.
const EXACT_CLOSE: &str = "    invoke.own scope.end %q1, effects scope_effects(failures=[thrown],
        suspends=false, cancel=false)
        normal ^done($closed)
        unwind ^failed($closed, $failure)
^done(%closed: closed) in S:
    ret
^failed(%closed2: closed, %failure: failure) in S:
    raise %failure";

fn close_rules(module: &Module) -> Vec<RuleId> {
    let function = module.functions.iter().find(|(_, function)| function.symbol == "f").unwrap().0;
    let options = VerifyOptions { execution: ExecutionMode::Tests, ..VerifyOptions::default() };
    let report = verify_function(module, function, VerifyContext::default(), &options);
    report.diagnostics.into_iter().map(|diagnostic| diagnostic.rule).collect()
}

#[test]
fn a_cache_state_token_needs_the_cleanup_effects_of_its_outcome() {
    // Async ABI D24.2 (ruling 4): no settle waits, and a settled Future releases its outcome into the owner's
    // plan. The token adds no effect of its own; the close states exactly the cleanup effects of the outcome,
    // which come with the duty in the inventory, in a sync owner too.
    let exact = cache_outcome(EXACT_CLOSE);
    assert_eq!(close_rules(&exact), []);
    let understated = cache_outcome(concat!(
        "    %closed: closed = scope.end %q1, effects scope_effects(failures=[], suspends=false, cancel=false)\n",
        "    ret"));
    assert_eq!(close_rules(&understated), [RuleId::B104]);
    // Any other service token at the close keeps unknown cleanup effects.
    let task = cache_outcome_with("%task: task-state", EXACT_CLOSE);
    assert!(close_rules(&task).contains(&RuleId::B104), "{:?}", close_rules(&task));
}

/// A verifier state over `module`, as the verifier runs it.
fn state<'a>(module: &'a Module, options: &'a VerifyOptions) -> State<'a> {
    State {
        module, options, context: VerifyContext::default(), diagnostics: vec![], stats: VerifyStats::default(),
        incomplete: false, exhausted_limit: None, suppressed: 0, location: EntityLocation::Module, source_loc: None,
        poisoned: std::collections::BTreeSet::new(), positions: std::collections::BTreeMap::new(),
        edge_positions: std::collections::BTreeMap::new(), cleanup_origins: None,
    }
}

#[test]
fn the_cleanup_of_a_future_is_the_cleanup_of_its_outcome() {
    // Async ABI D24.2: a Future's obligation is its cache duty, which never waits; a settled Future releases
    // its outcome, so its cleanup effects are those of the outcome type.
    let mut module = cache_outcome(EXACT_CLOSE);
    let int = module.types.intern(Type::Int).unwrap();
    let r = module.types.iter().find(|(_, ty)| matches!(ty, Type::Object { .. })).unwrap().0;
    let futures = [Type::Future(Some(r)), Type::Future(Some(int)), Type::Future(None)]
        .map(|ty| module.types.intern(ty).unwrap());
    let options = VerifyOptions::default();
    let mut state = state(&module, &options);
    let body = module.functions.iter().find(|(_, function)| function.symbol == "f").unwrap().1.body.as_ref().unwrap();
    let effects = futures.map(|ty| {
        let effects = super::super::effects::cleanup(&module, body, ty, &mut state);
        (effects.suspends, effects.failures)
    });
    assert_eq!(effects, [(false, vec![FailureKind::Thrown]), (false, vec![]), (false, vec![])]);
}

/// An async coro body declaring a one-slot layout whose entry block is `ENTRY`, then `ret`.
fn bare_body(profile: &str, attrs: &str, entry: &str) -> String {
    format!("oir 2 {profile}
module bare
scope S action in @f
summary Mf internal contract call_contract(failures=[thrown, program, panic, native, cancelled], suspends=true,
    publication=atomic)
func @f(%frame: frame<@f>) -> void {attrs}summary(Mf) {{
    frame.layout {{
        slot 0: str
    }}
    continuation 0 start saves [0]
^entry in S:
{entry}    ret
}}
")
}

#[test]
fn only_an_async_coro_or_low_body_declares_a_frame_layout() {
    for source in [AWAIT, DESTROY, LAYOUT, EXTERNAL] {
        assert!(!structural(source).contains(&RuleId::CoroLayoutMisplaced));
    }
    // A layout on an ordinary function.
    let ordinary = bare_body("coro", "", "");
    assert_eq!(structural(&ordinary), [RuleId::CoroLayoutMisplaced]);
    // A layout on an async body before the coro profile.
    let early = bare_body("own", "[async] ", "");
    assert!(structural(&early).contains(&RuleId::CoroLayoutMisplaced), "{:?}", structural(&early));
}

#[test]
fn an_async_body_starts_with_the_resume_of_continuation_0() {
    let resumed = "    (%fr0: frame<@f>, %name: str) = frame.resume %frame, continuation 0, slots [
        saved_slot(slot=0, ty=str, linear=false)]\n";
    assert_eq!(structural(&bare_body("coro", "[async] ", resumed)), []);
    assert_eq!(structural(&bare_body("coro", "[async] ", "")), [RuleId::CoroMissingResume]);
}

#[test]
fn every_declared_continuation_has_a_suspension_site() {
    let unused = edited(AWAIT, "    continuation 2 at #9 saves [1] operands [0, 3, 4, 5]\n",
        "    continuation 2 at #9 saves [1] operands [0, 3, 4, 5]\n    continuation 3 at ^ok saves [1]\n");
    assert_eq!(structural(&unused), [RuleId::CoroContinuationMismatch]);
}

const NEVER: &str = include_str!("../../tests/fixtures/async/coro/async-never-awaited.oir");

/// A coro owner whose cold Future leaves the inner scope S2 for S1: the token form of `cache.rehome` (C-4)
/// consumes the duty token into the pending reserve of S2's exit.
fn token_rehome(target: &str) -> String {
    let start = NEVER.find("func @main").unwrap();
    let main = format!("func @main() -> int [entry] abi observation summary(M6) {{
    service.schedule Schedule0 owner S2, protocol P0, duties [%8]
    service.schedule Schedule1 owner S1, protocol P0, duties [dynamic]
    boundary B0 lexical in S1
    boundary B1 lexical in S2
    key k0: key = service_of(value=%7, kind=cache)
    dependency d0: dependency = key(k0)
    dependency d1: dependency = from_ssa(%10)
    selection s0: selection = union([])
    ^b0 in S1:
    %0: scope<S1> = scope.begin S1
    %1: scope<S2> = scope.begin S2
    br ^b1(%0, %1)
    ^b1(%20: scope<S1>, %21: scope<S2>) in S2:
    %2: own = own.empty
    (%3: own, %4: own, %5: deps) = own.split %2, s0
    %6: own = own.join [%3, %4]
    (%7: future<int> facts(managed(deps=d0)), %8: cache-state, %9: scope<S2>) = fut.new %6, %5, %21, \
environment deferred(function=@explode), result int, owner S2
    %10: deps = deps.of %7
    %11: own = own.empty
    %12: pending = exit.prepare fallthrough %7, %11, deps(%10) at B1
    %13: pending = cache.rehome %12, %8, target {target}, protocol P0
    (%14: closed, %15: pending) = scope.end %9, reserve %13, effects scope_effects(failures=[], suspends=false, \
cancel=false)
    br ^b2(%20, %15)
    ^b2(%22: scope<S1>, %23: pending) in S1:
    (%16: future<int> facts(managed(deps=d1)), %17: own, %18: deps) = exit.commit %23, destination scope(S1), \
result future<int>
    %19: scope<S1> = own.admit %22, %17
    %26: bool = fut.done %16
    %24: int = const 0
    %25: closed = scope.end %19, effects scope_effects(failures=[], suspends=false, cancel=false)
    ret %24
}}
");
    let scopes = "scope S1 action in @main\nscope S2 block in @main parent S1\n";
    let module = NEVER[..start].replace("scope S1 action in @main\n", scopes);
    format!("{module}{main}")
}

/// C-4: the token form moves the duty and what its cache holds from every carrier into the reserve, targeted at
/// the exit's destination. The full verifier still refuses the form until the coro pass produces it (R2), so the
/// ownership rules are read from their own diagnostics.
#[test]
fn a_token_rehome_moves_the_duty_into_the_reserve_of_its_exit() {
    // The token form is the form coro produces for a duty created here (#601 D25); the full verifier accepts it.
    assert_eq!(full(&token_rehome("S1")), []);
    // The destination of S2's exit is S1, not the caller.
    assert!(full(&token_rehome("caller")).contains(&RuleId::B94));
    // The token was consumed by the first rehome: a second one has no cache-state of a live Future.
    let twice = edited(&token_rehome("S1"), "    (%14: closed, %15: pending) = scope.end %9, reserve %13",
        "    %30: pending = cache.rehome %13, %8, target S1, protocol P0\n    \
(%14: closed, %15: pending) = scope.end %9, reserve %30");
    let module = text::parse(&twice).unwrap();
    let options = VerifyOptions { execution: ExecutionMode::Tests, ..VerifyOptions::default() };
    let report = verify_with(&module, &options, VerifyContext::default()).unwrap_err();
    assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.rule == RuleId::B94
        && diagnostic.message == "cache.rehome consumes the cache-state token of a Future"), "{report:?}");
}

/// The token rehome of a cold Future from S3 to S2, then the own form from S2 to S1 (both forms are valid in
/// coro, C-4): the second rehome needs the duty that the first one handed to S2.
fn token_then_scope_rehome() -> String {
    let start = NEVER.find("func @main").unwrap();
    let main = "func @main() -> int [entry] abi observation summary(M6) {
    service.schedule Schedule0 owner S3, protocol P0, duties [%8]
    service.schedule Schedule1 owner S2, protocol P0, duties [dynamic]
    service.schedule Schedule2 owner S1, protocol P0, duties [dynamic]
    boundary B1 lexical in S3
    boundary B2 lexical in S2
    key k0: key = service_of(value=%7, kind=cache)
    dependency d0: dependency = key(k0)
    dependency d1: dependency = from_ssa(%10)
    dependency d2: dependency = from_ssa(%18)
    selection s0: selection = union([])
    ^b0 in S1:
    %0: scope<S1> = scope.begin S1
    %1: scope<S2> = scope.begin S2
    %2: scope<S3> = scope.begin S3
    br ^b1(%0, %1, %2)
    ^b1(%20: scope<S1>, %21: scope<S2>, %22: scope<S3>) in S3:
    %3: own = own.empty
    (%4: own, %5: own, %6: deps) = own.split %3, s0
    %26: own = own.join [%4, %5]
    (%7: future<int> facts(managed(deps=d0)), %8: cache-state, %9: scope<S3>) = fut.new %26, %6, %22, \
environment deferred(function=@explode), result int, owner S3
    %10: deps = deps.of %7
    %11: own = own.empty
    %12: pending = exit.prepare fallthrough %7, %11, deps(%10) at B1
    %13: pending = cache.rehome %12, %8, target S2, protocol P0
    (%14: closed, %15: pending) = scope.end %9, reserve %13, effects scope_effects(failures=[], suspends=false, \
cancel=false)
    br ^b2(%20, %21, %15)
    ^b2(%23: scope<S1>, %24: scope<S2>, %25: pending) in S2:
    (%16: future<int> facts(managed(deps=d1)), %17: own, %18: deps) = exit.commit %25, destination scope(S2), \
result future<int>
    %19: scope<S2> = own.admit %24, %17
    %30: own = own.empty
    %31: pending = exit.prepare fallthrough %16, %30, deps(%18) at B2
    (%32: scope<S2>, %33: pending) = cache.rehome %19, %31, future %16, target S1, protocol P0
    (%34: closed, %35: pending) = scope.end %32, reserve %33, effects scope_effects(failures=[], suspends=false, \
cancel=false)
    br ^b3(%23, %35)
    ^b3(%36: scope<S1>, %37: pending) in S1:
    (%38: future<int> facts(managed(deps=d2)), %39: own, %40: deps) = exit.commit %37, destination scope(S1), \
result future<int>
    %41: scope<S1> = own.admit %36, %39
    %42: bool = fut.done %38
    %43: int = const 0
    %44: closed = scope.end %41, effects scope_effects(failures=[], suspends=false, cancel=false)
    ret %43
}
";
    let scopes = "scope S1 action in @main\nscope S2 block in @main parent S1\nscope S3 block in @main parent S2\n";
    let module = NEVER[..start].replace("scope S1 action in @main\n", scopes);
    format!("{module}{main}")
}

/// C-4: the token form hands the duty and its cache's holdings to the reserve, so the destination scope holds
/// the duty after the commit, and a later rehome from there finds it.
#[test]
fn a_token_rehome_hands_the_duty_to_its_destination() {
    let module = text::parse(&token_then_scope_rehome()).unwrap();
    let options = VerifyOptions { execution: ExecutionMode::Tests, ..VerifyOptions::default() };
    let rules: Vec<_> = match verify_with(&module, &options, VerifyContext::default()) {
        Ok(_) => vec![],
        Err(report) => report.diagnostics.into_iter().map(|diagnostic| (diagnostic.rule, diagnostic.message)).collect(),
    };
    assert!(rules.is_empty(), "{rules:?}");
}

const SUCCESSOR: &str = include_str!("../../tests/fixtures/async/coro/async-successor.oir");

#[test]
fn an_async_successor_body_is_checked_against_its_language_signature() {
    // #733, C-2: the coro signature of an async body is `[frame<@f>] -> void`; T5 reads the successor result
    // from its language signature, and B8.2 counts the published successor as the body's return. A failure
    // publication is a failure exit, which may drop a successor (main's failing `Counter(n: 5)`).
    assert_eq!(full(SUCCESSOR), []);
    // T5: the body publishes an int, so its language signature no longer returns the successor type.
    let other = edited(SUCCESSOR, "fut.publish %1, success(value=%25,", "fut.publish %1, success(value=%11,");
    let rules = full(&other);
    assert!(rules.contains(&RuleId::T5), "{rules:?}");
    // B8.2: main's success exit leaves the awaited successor unused.
    let unused = edited(SUCCESSOR, concat!(
        "    %36: app.Counter facts(plain(certificate=closed_graph(ty=app.Counter))) = succ.take %34, ty app.Counter\n",
        "    %37: int = call @app.Counter.n#1(%36)\n"), "    %37: int = const 2\n");
    let rules = full(&unused);
    assert!(rules.contains(&RuleId::B82), "{rules:?}");
}
