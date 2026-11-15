//! #683: an open `fn.call` hands its unknown retention to the dynamic part of the
//! named scope through the registered Closure protocol (OIR-2 §6.4, §7.1). Low keeps
//! the counted `own.dynamic` site and settles its entries in the common cleanup
//! automaton: they join the scope inventory in serial order and the executor
//! releases each through its runtime descriptor, with the reserve and failure
//! dispatch of the storage merge entries. Without order evidence there is no
//! direct plan.
use super::*;
use crate::catalogue::RuntimeCatalogue;

const ESCAPE: &str = include_str!("fixtures/open-call-escape.own.oir");
const DROP: &str = include_str!("fixtures/open-call-drop.own.oir");
const ANY: &str = include_str!("fixtures/open-any.own.oir");

/// The registered open callable ABI and cleanup intrinsics come from the runtime catalogue.
fn verify_open(module: &Module) -> Result<VerifiedModule<'_>, VerifyReport> {
    let trusted = RuntimeCatalogue::for_module(module);
    let context = VerifyContext { text_locations: None, external_contracts: &trusted };
    verify_with(module, &VerifyOptions::default(), context)
}

fn parsed(source: &str) -> Module {
    text::parse(source).unwrap_or_else(|error| panic!("{error}"))
}

fn lowered(source: &str) -> Module {
    let module = parsed(source);
    verify_open(&module).unwrap_or_else(|report| panic!("Own: {report:?}"));
    let output = lower_cleanup(module, &LowOptions::default()).unwrap();
    verify_open(&output).unwrap_or_else(|report| panic!("Low: {report:?}"));
    let printed = text::print(&output).unwrap();
    let reparsed = parsed(&printed);
    check_cleanup_plans(&reparsed, &LowOptions::default()).unwrap();
    verify_open(&reparsed).unwrap_or_else(|report| panic!("round-trip: {report:?}"));
    assert!(text::normalized_eq(&output, &reparsed).unwrap());
    output
}

fn body_f(module: &Module) -> &Body {
    module.functions.iter().find(|(_, function)| function.symbol == "f").unwrap().1.body.as_ref().unwrap()
}

fn body_f_mut(module: &mut Module) -> &mut Body {
    module.functions.iter_mut().find(|(_, function)| function.symbol == "f").unwrap().1.body.as_mut().unwrap()
}

/// The verifier rejects `module` and its text round trip with `rule`.
fn rejected(module: &Module, rule: RuleId) {
    let reparsed = parsed(&text::print(module).unwrap());
    for module in [module, &reparsed] {
        let report = verify_open(module).unwrap_err();
        assert!(report.diagnostics.iter().any(|diagnostic| diagnostic.rule == rule), "{report:?}");
    }
}

/// The dynamic transfer terminator of `@f`: its normal and unwind targets.
fn dynamic_site(body: &Body) -> (DynamicReason, BlockId, BlockId) {
    body.blocks.iter().find_map(|(_, block)| match &block.term.kind {
        TermKind::InvokeOwn { op, normal, unwind } => match op.as_ref() {
            Operation::Ownership(OwnershipOp::Dynamic { reason, .. }) => Some((*reason, normal.block, unwind.block)),
            _ => None,
        },
        _ => None,
    }).expect("own.dynamic survives in Low")
}

/// The close opened by a `cleanup.begin` in `block`, if any.
fn close_in(body: &Body, block: BlockId) -> Option<CleanupCloseId> {
    body.blocks.get(block).unwrap().insts.iter().find_map(|id| match body.insts.get(*id).unwrap().op {
        Operation::Cleanup(CleanupOp::Begin { close, .. }) => Some(close),
        _ => None,
    })
}

/// The first close reached from `block` along branches and normal invoke edges, and its block.
fn close_after(body: &Body, mut block: BlockId) -> Option<(BlockId, CleanupCloseId)> {
    for _ in 0..body.blocks.len() {
        if let Some(close) = close_in(body, block) {
            return Some((block, close));
        }
        block = match &body.blocks.get(block).unwrap().term.kind {
            TermKind::Br(target) => target.block,
            TermKind::InvokeOwn { normal, .. } => normal.block,
            _ => return None,
        };
    }
    None
}

#[test]
fn open_call_low_goldens() {
    for (source, golden, reason, reserve) in [
        (ESCAPE, include_str!("fixtures/open-call-escape.low.oir"), DynamicReason::Closure, true),
        (DROP, include_str!("fixtures/open-call-drop.low.oir"), DynamicReason::Closure, false),
        (ANY, include_str!("fixtures/open-any.low.oir"), DynamicReason::Any, false),
    ] {
        let output = lowered(source);
        let expected = parsed(golden);
        verify_open(&expected).unwrap_or_else(|report| panic!("golden: {report:?}"));
        assert!(text::normalized_eq(&output, &expected).unwrap(), "{}", text::print(&output).unwrap());

        // One counted dynamic site with its reason and protocol (principle 11).
        let verified = verify_open(&output).unwrap();
        assert_eq!(verified.stats().dynamic.census.module_count(reason), 1);
        assert_eq!(verified.stats().dynamic.census.module_count(DynamicReason::Merge), 0);
        let body = body_f(&output);
        let (found, normal, unwind) = dynamic_site(body);
        assert_eq!(found, reason);

        // Every close of `@f` merges the whole scope inventory by the executor: the
        // dynamic part has no finite order evidence, so no plan is direct.
        assert!(!body.cleanup.plans.is_empty());
        for (_, plan) in body.cleanup.plans.iter() {
            assert_eq!(plan.direct, None);
            assert_eq!(plan.sources[0], MergeSource::ScopeInventory);
        }
        assert!(body.insts.iter().any(|(_, inst)| matches!(inst.op, Operation::Cleanup(CleanupOp::Destroy { .. }))));

        // The scope close after a transfer settles the protocol entries (and joins the
        // escaping result's reserve only on failure).
        let (_, close) = close_after(body, normal).expect("normal close after the transfer");
        let close = body.cleanup.closes.get(close).unwrap();
        assert_eq!(close.reserve, reserve);
        assert!(!close.active_failure);

        // A failed transfer hands the unaccepted packet back; it is admitted to the
        // scope before the unwind close, so it cannot disappear on failure.
        let unwind_block = body.blocks.get(unwind).unwrap();
        let unaccepted = unwind_block.params[1];
        assert!(unwind_block.insts.iter().any(|id| matches!(body.insts.get(*id).unwrap().op,
            Operation::Ownership(OwnershipOp::Admit { packet, .. }) if packet == unaccepted)));
        let unwind_close = close_in(body, unwind).expect("unwind close after a failed transfer");
        assert!(body.cleanup.closes.get(unwind_close).unwrap().active_failure);
    }
}

// The executor loop of the normal close after the transfer skipped on one path: the
// dynamic entry would never be released there.
#[test]
fn rejects_an_open_entry_missing_from_the_plan() {
    let golden = include_str!("fixtures/open-call-drop.low.oir");
    assert!(golden.contains("    br ^b26(%51)\n"));
    assert!(golden.contains("    ^b29(%108: cleanup-state) in S2:\n"));
    let skipped = golden.replace("    br ^b26(%51)\n",
        "    [iskip] %skip: bool = const true\n    cond %skip, ^b29(%51), ^b26(%51)\n");
    rejected(&parsed(&skipped), RuleId::B102);

    let mut output = lowered(DROP);
    let body = body_f_mut(&mut output);
    let (_, normal, _) = dynamic_site(body);
    let (_, close) = close_after(body, normal).unwrap();
    let plan = body.cleanup.plans.iter_mut().find(|(_, plan)| plan.close == close).unwrap().1;
    plan.sources.retain(|source| *source != MergeSource::ScopeInventory);
    assert!(matches!(check_cleanup_plans(&output, &LowOptions::default()), Err(LowError::InvalidOutput(_))));
    rejected(&output, RuleId::B101);
}

// A second `cleanup.destroy` of the selected protocol entry in the same executor step.
#[test]
fn rejects_an_open_entry_settled_twice() {
    let mut output = lowered(DROP);
    let body = body_f_mut(&mut output);
    let (_, normal, _) = dynamic_site(body);
    let (begin, _) = close_after(body, normal).unwrap();
    let TermKind::Br(next) = body.blocks.get(begin).unwrap().term.kind.clone() else {
        panic!("the normal close enters its executor");
    };
    let TermKind::CleanupNext { selected, .. } = body.blocks.get(next.block).unwrap().term.kind.clone() else {
        panic!("the executor selects the next entry");
    };
    let destroy = selected.block;
    let first = body.blocks.get(destroy).unwrap().insts[0];
    let Operation::Cleanup(CleanupOp::Destroy { key, .. }) = body.insts.get(first).unwrap().op else {
        panic!("the selected entry is destroyed through its descriptor");
    };
    let results = body.insts.get(first).unwrap().results.clone();
    let types: Vec<_> = results.iter().map(|value| body.values.get(*value).unwrap().ty).collect();
    let second = body.insts.alloc(Instruction {
        results: vec![],
        op: Operation::Cleanup(CleanupOp::Destroy { cleanup: results[0], key }),
        facts: InstructionFacts::default(),
        loc: None,
    }).unwrap();
    let mut repeated = vec![];
    for (index, ty) in types.into_iter().enumerate() {
        let def = Definition::InstResult { inst: second, index: index as u16 };
        repeated.push(body.values.alloc(ValueData { ty, def, facts: ValueFacts::Service }).unwrap());
    }
    body.insts.get_mut(second).unwrap().results.clone_from(&repeated);
    let block = body.blocks.get_mut(destroy).unwrap();
    block.insts.push(second);
    let TermKind::FailureDispatch { failure, none, some } = &mut block.term.kind else {
        panic!("the destroy step dispatches its failure");
    };
    *failure = repeated[1];
    none.args[0] = repeated[0];
    some.args[0] = EdgeArg::Value(repeated[0]);
    rejected(&output, RuleId::B102);
}

/// Mutations of the protocol named by the dynamic site. Each leaves the general B7.1
/// family check satisfied, so only the Low synchronous-protocol rule refuses it.
fn protocol_mutations(source: &str) -> Vec<Box<dyn Fn(&mut ProtocolDecl)>> {
    let closure = source.contains("kind closure");
    let mut mutations: Vec<Box<dyn Fn(&mut ProtocolDecl)>> = vec![
        // The transfer leaves the packet accepted on unwind.
        Box::new(|protocol| {
            let rule = protocol.rules.iter_mut().find(|rule| rule.transition == ProtocolTransition::Dynamic).unwrap();
            rule.unwind.as_mut().unwrap().state = HolderState::Accepted;
        }),
        // The transfer may be cancelled.
        Box::new(|protocol| {
            let rule = protocol.rules.iter_mut().find(|rule| rule.transition == ProtocolTransition::Dynamic).unwrap();
            rule.cancel = rule.unwind;
        }),
        // A second transfer.
        Box::new(|protocol| {
            let rule = protocol.rules.iter().find(|rule| rule.transition == ProtocolTransition::Dynamic).unwrap();
            protocol.rules.push(rule.clone());
        }),
    ];
    if closure {
        // The publication of the returned packet does not settle the scope on unwind.
        mutations.push(Box::new(|protocol| {
            let rule = protocol.rules.iter_mut().find(|rule| rule.transition == ProtocolTransition::Publish).unwrap();
            rule.unwind = Some(ProtocolPoststate { holder: HolderKind::Packet, state: HolderState::Unaccepted });
        }));
        // No publication rule.
        mutations.push(Box::new(|protocol| {
            protocol.rules.retain(|rule| rule.transition != ProtocolTransition::Publish);
        }));
    } else {
        // An `any` box is transferred, never published.
        mutations.push(Box::new(|protocol| {
            let mut rule = protocol.rules[0].clone();
            rule.transition = ProtocolTransition::Publish;
            protocol.rules.push(rule);
        }));
    }
    mutations
}

#[test]
fn low_accepts_only_the_synchronous_closure_and_any_protocols() {
    for source in [ESCAPE, DROP, ANY] {
        for mutate in protocol_mutations(source) {
            let mut input = parsed(source);
            let protocol = input.protocols.get_mut(ProtocolId::from_u32(0)).unwrap();
            mutate(protocol);
            assert!(matches!(lower_cleanup(input, &LowOptions::default()), Err(LowError::Unsupported { .. })));
            let mut output = lowered(source);
            mutate(output.protocols.get_mut(ProtocolId::from_u32(0)).unwrap());
            rejected(&output, RuleId::B71);
        }
        // A dynamic site naming a protocol of another family.
        let mut output = lowered(source);
        let protocol = output.protocols.get_mut(ProtocolId::from_u32(0)).unwrap();
        protocol.kind = if protocol.kind == ProtocolKind::Any { ProtocolKind::Closure } else { ProtocolKind::Any };
        rejected(&output, RuleId::B71);
    }
}

#[test]
fn eligibility_contract_of_dynamic_protocols() {
    let open = crate::catalogue::open_callable_protocol();
    assert!(eligibility::synchronous_dynamic(DynamicReason::Closure, &open));
    for reason in [DynamicReason::Merge, DynamicReason::Any, DynamicReason::Atomic, DynamicReason::Cell,
        DynamicReason::Channel, DynamicReason::Group] {
        assert!(!eligibility::synchronous_dynamic(reason, &open), "{reason:?}");
    }
    let transfer = open.rules.iter().find(|rule| rule.transition == ProtocolTransition::Dynamic).unwrap().clone();
    let single = |kind| ProtocolDecl {
        kind, holders: open.holders.clone(), rules: vec![transfer.clone()], roots: open.roots.clone(),
    };
    assert!(eligibility::synchronous_dynamic(DynamicReason::Any, &single(ProtocolKind::Any)));
    assert!(eligibility::synchronous_dynamic(DynamicReason::Merge, &single(ProtocolKind::Storage)));
    assert!(!eligibility::synchronous_dynamic(DynamicReason::Closure, &single(ProtocolKind::Closure)));
    for kind in [ProtocolKind::Atomic, ProtocolKind::Cell, ProtocolKind::Channel, ProtocolKind::Group,
        ProtocolKind::Callable, ProtocolKind::Services] {
        let reason = match kind {
            ProtocolKind::Atomic => DynamicReason::Atomic,
            ProtocolKind::Cell => DynamicReason::Cell,
            ProtocolKind::Channel => DynamicReason::Channel,
            _ => DynamicReason::Group,
        };
        assert!(!eligibility::synchronous_dynamic(reason, &single(kind)));
    }
    let mut unrooted = single(ProtocolKind::Any);
    unrooted.roots.clear();
    assert!(!eligibility::synchronous_dynamic(DynamicReason::Any, &unrooted));
    let mut held = single(ProtocolKind::Any);
    held.holders.push(HolderKind::Frame);
    assert!(!eligibility::synchronous_dynamic(DynamicReason::Any, &held));
}
