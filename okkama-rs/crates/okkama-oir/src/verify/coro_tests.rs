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

fn full(source: &str) -> Vec<RuleId> {
    let module = text::parse(source).unwrap_or_else(|error| panic!("{error}"));
    let options = VerifyOptions { execution: ExecutionMode::Tests, ..VerifyOptions::default() };
    match verify_with(&module, &options, VerifyContext::default()) {
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
    let cases = [
        // B5.1: the summary results disagree with the published value.
        (PLAIN, "call_contract(results=[int],\n    failures=", "call_contract(results=[str],\n    failures=",
            RuleId::B51),
        // B5.1: the summary has no parameter for the argument restored from continuation 0.
        (PLAIN, "summary Mwb call-boundary parameters [parameter_effect(convention=borrow, destination=none)] results",
            "summary Mwb call-boundary results", RuleId::B51),
        // T1: the deferred environment no longer matches the callee's start continuation.
        (PLAIN, "function=@work, arguments=[%seven]", "function=@work, arguments=[]", RuleId::T1),
        // B5.1: the published failure has a kind that the body summary does not declare.
        (FAILURE, "call_contract(results=[int],\n    failures=[thrown, program, panic, native, cancelled]",
            "call_contract(results=[int],\n    failures=[program, panic, native, cancelled]", RuleId::B51),
    ];
    for (source, from, to, rule) in cases {
        let rules = full(&edited(source, from, to));
        assert!(rules.contains(&rule), "{rule:?} for {from:?}: {rules:?}");
    }
}

#[test]
fn b95_variants_report_their_stable_codes() {
    let cases = [
        // D21: the save lists its slots in another order than the declared saved set.
        (PLAIN, "continuation 1 at ^entry saves [1, 2]", "continuation 1 at ^entry saves [2, 1]",
            RuleId::CoroLayoutTypeMismatch),
        // D21: the operand slots of the close omit the cache duty it settles.
        (PLAIN, "continuation 2 at ^ok saves [] operands [1, 4, 2]", "continuation 2 at ^ok saves [] operands [1, 4]",
            RuleId::CoroLayoutTypeMismatch),
        // D21: the declared saved set of continuation 3 omits the slot that its save fills.
        (PLAIN, "    continuation 3 at #90 saves [2] operands [1, 5, 6, 7]\n",
            "    continuation 3 at #90 saves [] operands [1, 5, 6, 7]\n", RuleId::CoroLayoutTypeMismatch),
        // A read of a saved value at an instruction after its resume.
        (REPEATED, "    br ^second(%b0, %bd0, %q3r, %ar)",
            "    %dup: int = int.add %a, %b0\n    br ^second(%b0, %bd0, %q3r, %ar)", RuleId::CoroReadAfterSave),
    ];
    for (source, from, to, rule) in cases {
        let rules = full(&edited(source, from, to));
        assert!(rules.contains(&rule), "{rule:?} for {from:?}: {rules:?}");
    }
}

#[test]
fn a_duty_left_running_across_await_unwind_is_an_incomplete_saved_set() {
    let source = edited(PLAIN, "    continuation 3 at #90 saves [2] operands [1, 5, 6, 7]\n",
        "    continuation 3 at #90 saves [] operands [1, 5, 6, 7]\n");
    let source = edited(&source, "frame.save %fr6, slots [frame_slot(slot=2, value=%c2, mode=consume)], continuation 3",
        "frame.save %fr6, continuation 3");
    let resume = concat!("    (%fr8: frame<@main>, %c3: cache-state) = frame.resume %fr7, continuation 3, ",
        "slots [saved_slot(slot=2,\n        ty=cache-state, linear=true)]");
    let source = edited(&source, resume, "    %fr8: frame<@main> = frame.resume %fr7, continuation 3");
    let source = edited(&source, "duties [%c3]", "duties [%c2]");
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
    let caught = "^throw(%e: Stop facts(plain(certificate=scalar)), %q2: scope<SW>) in SW";
    for terminator in ["throw %e", "panic %e"] {
        let source = edited(CAUGHT, "    throw %e\n", &format!("    {terminator}\n"));
        assert!(!structural(&source).contains(&RuleId::CoroBodyRaises), "{terminator}");
        // Without its unwind edge the same terminator leaves the body unpublished.
        let source = edited(&source, &format!("{caught} unwind ^thrown(%q2):"), &format!("{caught}:"));
        assert!(structural(&source).contains(&RuleId::CoroBodyRaises), "{terminator}");
    }
    // The raise after the lexical unwind leaves the body when the pass has not rewritten it.
    let source = edited(CAUGHT, "    fut.publish %fr0, failure(%combined)\n    ret\n", "    raise %combined\n");
    assert!(structural(&source).contains(&RuleId::CoroBodyRaises));
}
