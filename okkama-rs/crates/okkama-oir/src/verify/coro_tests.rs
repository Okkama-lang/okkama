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
