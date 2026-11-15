//! Structural call-entry packets (B5.1, audit #668 F5).
//!
//! A callee with a body and no language parameters has no argument retention
//! graph, so its entry packet is empty. Low's eligibility relies on that; the
//! linearity rules that bound a call packet by its arguments run only in full
//! verification. This check holds in every mode: such a call passes a packet
//! defined by `own.empty`. So does a `fn.call` of a closure whose function type
//! has no language parameters (a lambda without captures or parameters has the
//! signature `(; own)`), whatever function it binds.
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

/// A closure's callee is unknown, but a closure type without language parameters has no argument
/// retention graph either: whatever function it binds, its entry packet is empty.
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
    if !from_empty(body, packet) {
        state.error(R::B51, "a parameterless closure call must pass a packet defined by own.empty");
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

    /// `@main` calls its parameterless closure `%f` with the packet `PACKET` (a lambda without captures
    /// or parameters has the signature `(; own)`).
    fn closure_module(profile: &str, packet: &str) -> crate::Module {
        let text = r#"oir 2 PROFILE
module f5
scope SM action in @main
summary Mf call-boundary parameters [parameter_effect(convention=borrow, destination=none)] results
        [result_relation(provenance=unbound_result)] contract call_contract(results=[int], publication=atomic)
summary Mm call-boundary parameters [parameter_effect(convention=borrow, destination=none)] contract
        call_contract(publication=atomic)
func @main(%f: fn() -> int, %given: own) -> own abi explicit(input_packet=1, output_packet=0) summary(Mm) {
^entry in SM:
    %deps: deps = deps.of %f
    %packet: own = PACKET
    (%value: int, %out: own, %after: deps) = fn.call %f(; %packet) deps [%deps] summary(Mf)
    ret %out
}
"#;
        let text = text.replace("PROFILE", profile).replace("PACKET", packet);
        crate::text::parse(&text).unwrap_or_else(|error| panic!("{error:?}"))
    }

    #[test]
    fn parameterless_closure_call_accepts_an_own_empty_packet() {
        for profile in ["own", "low"] {
            structural(&closure_module(profile, "own.empty")).unwrap_or_else(|error| panic!("{profile}: {error:?}"));
        }
    }

    #[test]
    fn parameterless_closure_call_rejects_any_other_packet_in_structural_builds() {
        for profile in ["own", "low"] {
            let rejected = structural(&closure_module(profile, "own.join [%given]")).unwrap_err();
            assert_eq!(rejected.len(), 1, "{profile}: {rejected:?}");
            assert_eq!(rejected[0].0, RuleId::B51, "{profile}");
            assert!(rejected[0].1.contains("parameterless closure call must pass a packet defined by own.empty"),
                "{profile}: {rejected:?}");
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
