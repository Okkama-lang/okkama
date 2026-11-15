//! Structural call-entry packets (B5.1, audit #668 F5).
//!
//! A callee with a body and no language parameters has no argument retention
//! graph, so its entry packet is empty. Low's eligibility relies on that; the
//! linearity rules that bound a call packet by its arguments run only in full
//! verification. This check holds in every mode: such a call passes a packet
//! defined by `own.empty`. So does a `fn.call` of a lambda made in the body by
//! `fn.closure` without captures, of a type without language parameters: its
//! signature is `(; own)`.
use super::{EntityLocation as L, RuleId as R, State};
use crate::*;

pub(super) fn check(module: &Module, selected: Option<FuncId>, state: &mut State<'_>) {
    if module.profile == Profile::High {
        return;
    }
    for (function, data) in module.functions.iter() {
        if selected.is_some_and(|id| id != function) {
            continue;
        }
        let Some(body) = &data.body else {
            continue;
        };
        if !super::graph::eligible(function, body, state) {
            continue;
        }
        for (block_id, block) in body.blocks.iter() {
            for &instruction in &block.insts {
                if !state.step(1) {
                    return;
                }
                let Ok(inst) = body.insts.get(instruction) else {
                    continue;
                };
                state.at(L::Instruction { function, instruction }, inst.loc);
                operation(module, body, &inst.op, state);
            }
            state.at(L::Terminator { function, block: block_id }, block.term.loc);
            match &block.term.kind {
                TermKind::InvokeOwn { op, .. } => operation(module, body, op, state),
                TermKind::InvokeDefer { function: target, call, .. } => {
                    packet(module, body, *target, call.packet, state);
                }
                _ => {}
            }
        }
    }
}

fn operation(module: &Module, body: &Body, op: &Operation, state: &mut State<'_>) {
    let Some((call, _, target)) = super::summaries::call(op) else {
        return;
    };
    if let Operation::Call(CallOp::Closure { callee, .. }) = op {
        closure(module, body, callee.value, call.packet, state);
        return;
    }
    let Some(target) = target.or_else(|| super::summaries::cap_target(module, op)) else {
        return;
    };
    packet(module, body, target, call.packet, state);
}

/// The receiver of a closure call is itself a language argument: a closure's packet may carry the
/// obligations of its captures (an open call hands over the callable's selected parts). A closure made
/// here by `fn.closure` without captures, of a type without language parameters, has no retention
/// graph at all, so its call passes a packet defined by `own.empty`.
fn closure(module: &Module, body: &Body, callee: ValueId, packet: Option<ValueId>, state: &mut State<'_>) {
    let Some(packet) = packet else {
        return;
    };
    let Ok(Type::Fn(ty)) = body.values.get(callee).and_then(|value| module.types.get(value.ty)) else {
        return;
    };
    if ty.params.iter().any(|&ty| language(module, ty)) {
        return;
    }
    let captureless = body.values.get(callee).ok().and_then(|value| match value.def {
        Definition::InstResult { inst, .. } => body.insts.get(inst).ok(),
        Definition::BlockParam { .. } => None,
    }).is_some_and(|inst| matches!(&inst.op, Operation::Core(CoreOp::FnClosure { captures, .. })
        if captures.is_empty()));
    if !captureless {
        return;
    }
    if !from_empty(body, packet) {
        state.error(R::B51,
            "a call of a lambda without captures or parameters must pass a packet defined by own.empty");
    }
}

fn language(module: &Module, ty: TypeId) -> bool {
    module.types.get(ty).is_ok_and(|ty| !matches!(ty, Type::Service(_) | Type::Descriptor))
}

fn from_empty(body: &Body, packet: ValueId) -> bool {
    body.values.get(packet).ok().and_then(|value| match value.def {
        Definition::InstResult { inst, .. } => body.insts.get(inst).ok(),
        Definition::BlockParam { .. } => None,
    }).is_some_and(|inst| matches!(inst.op, Operation::Ownership(OwnershipOp::Empty {})))
}

fn packet(module: &Module, body: &Body, target: FuncId, packet: Option<ValueId>, state: &mut State<'_>) {
    let Some(packet) = packet else {
        return;
    };
    let Ok(callee) = module.functions.get(target) else {
        return;
    };
    if callee.body.is_none() || !matches!(callee.signature.ownership_abi, OwnershipAbi::Explicit { .. }) {
        return;
    }
    if callee.signature.params.iter().any(|&ty| language(module, ty)) {
        return;
    }
    if !from_empty(body, packet) {
        state.error(R::B51, "a callee with no language parameters must receive a packet defined by own.empty");
    }
}

#[cfg(test)]
mod tests {
    use crate::verify::{verify_with, RuleId, VerifyContext, VerifyMode, VerifyOptions};

    /// `@callee` has a body, an explicit packet and no language parameter; `@main` hands it
    /// the packet `PACKET`. The rule holds in Own and in Low.
    fn module(profile: &str, packet: &str) -> crate::Module {
        let text = r#"oir 2 PROFILE
module f5
scope SC action in @callee
scope SM action in @main
summary Mc call-boundary contract call_contract(publication=atomic)
summary Mm call-boundary contract call_contract(publication=atomic)
func @callee(%input: own) -> own abi explicit(input_packet=0, output_packet=0) summary(Mc) {
^entry in SC:
    ret %input
}
func @main(%given: own) -> own abi explicit(input_packet=0, output_packet=0) summary(Mm) {
^entry in SM:
    %packet: own = PACKET
    (%out: own, %deps: deps) = call @callee(; %packet) summary(Mc)
    ret %out
}
"#;
        let text = text.replace("PROFILE", profile).replace("PACKET", packet);
        crate::text::parse(&text).unwrap_or_else(|error| panic!("{error:?}"))
    }

    fn structural(module: &crate::Module) -> Result<(), Vec<(RuleId, String)>> {
        let options = VerifyOptions { mode: VerifyMode::Structural, ..VerifyOptions::default() };
        verify_with(module, &options, VerifyContext::default()).map(|_| ()).map_err(|report| {
            report.diagnostics.into_iter().map(|diagnostic| (diagnostic.rule, diagnostic.message)).collect()
        })
    }

    /// `@main` makes the lambda `@lambda` without captures (signature `(; own)`) by `CLOSURE` and calls
    /// it with the packet `PACKET`; without `CLOSURE` it calls its parameter `%g` instead.
    fn closure_module(profile: &str, closure: &str, packet: &str) -> crate::Module {
        let text = r#"oir 2 PROFILE
module f5
scope SL action in @lambda
scope SM action in @main
summary Ml call-boundary results [result_relation(provenance=unbound_result)] contract
        call_contract(results=[int], publication=atomic)
summary Mf call-boundary parameters [parameter_effect(convention=borrow, destination=none)] results
        [result_relation(provenance=unbound_result)] contract call_contract(results=[int], publication=atomic)
summary Mm call-boundary parameters [parameter_effect(convention=borrow, destination=none)] contract
        call_contract(publication=atomic)
func @lambda(%input: own) -> (int, own) [lambda captures 0] abi explicit(input_packet=0, output_packet=1)
        summary(Ml) {
^entry in SL:
    %seven: int = const 7
    ret %seven, %input
}
func @main(%g: fn() -> int, %given: own) -> own abi explicit(input_packet=1, output_packet=0) summary(Mm) {
^entry in SM:
    CLOSURE
    %deps: deps = deps.of CALLEE
    %packet: own = PACKET
    (%value: int, %out: own, %after: deps) = fn.call CALLEE(; %packet) deps [%deps] summary(Mf)
    ret %out
}
"#;
        let callee = if closure.is_empty() { "%g" } else { "%f" };
        let text = text.replace("PROFILE", profile).replace("CLOSURE", closure).replace("PACKET", packet)
            .replace("CALLEE", callee);
        crate::text::parse(&text).unwrap_or_else(|error| panic!("{error:?}"))
    }

    const LAMBDA: &str = "%f: fn() -> int = fn.closure @lambda, [], ty fn() -> int";

    #[test]
    fn captureless_lambda_call_accepts_an_own_empty_packet() {
        for profile in ["own", "low"] {
            structural(&closure_module(profile, LAMBDA, "own.empty"))
                .unwrap_or_else(|error| panic!("{profile}: {error:?}"));
        }
    }

    #[test]
    fn captureless_lambda_call_rejects_any_other_packet_in_structural_builds() {
        for profile in ["own", "low"] {
            let rejected = structural(&closure_module(profile, LAMBDA, "own.join [%given]")).unwrap_err();
            assert_eq!(rejected.len(), 1, "{profile}: {rejected:?}");
            assert_eq!(rejected[0].0, RuleId::B51, "{profile}");
            assert!(rejected[0].1.contains("lambda without captures or parameters must pass a packet defined by"),
                "{profile}: {rejected:?}");
        }
    }

    /// A closure received from elsewhere may carry captures: an open call hands its selected
    /// obligations over in the packet (#569 C3b), so its packet is bounded only in full verification.
    #[test]
    fn a_closure_of_unknown_origin_may_pass_its_obligations() {
        for profile in ["own", "low"] {
            let received = closure_module(profile, "", "own.join [%given]");
            structural(&received).unwrap_or_else(|error| panic!("{profile}: {error:?}"));
        }
    }

    #[test]
    fn no_input_callee_accepts_an_own_empty_packet() {
        for profile in ["own", "low"] {
            structural(&module(profile, "own.empty")).unwrap_or_else(|error| panic!("{profile}: {error:?}"));
        }
    }

    #[test]
    fn no_input_callee_rejects_any_other_packet_in_structural_builds() {
        for profile in ["own", "low"] {
            let rejected = structural(&module(profile, "own.join [%given]")).unwrap_err();
            assert_eq!(rejected.len(), 1, "{profile}: {rejected:?}");
            assert_eq!(rejected[0].0, RuleId::B51, "{profile}");
            assert!(rejected[0].1.contains("no language parameters must receive a packet defined by own.empty"),
                "{profile}: {rejected:?}");
        }
    }
}
