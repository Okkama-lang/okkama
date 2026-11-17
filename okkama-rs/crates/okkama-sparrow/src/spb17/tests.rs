use super::*;

pub(super) fn empty_function() -> Function {
    Function {
        symbol: "main".into(),
        entry_abi: EntryAbi::Observation,
        registers: vec![],
        services: vec![],
        params: vec![],
        results: vec![],
        blocks: vec![Block { pc: 0, params: vec![] }],
        proofs: FunctionProofs::default(),
        code: vec![Instruction::Ret(vec![])],
    }
}

fn plain() -> Program {
    Program {
        module: "S1 \"codec\"\nЮникод".into(),
        functions: vec![empty_function()],
        entry: Some(0),
        ..Program::default()
    }
}

fn produced(destination: u32, slot: u32) -> Assignment {
    Assignment { destination: Location::Service(destination), source: Source::ProducedSlot(slot) }
}

fn multi_result() -> Program {
    let mut p = plain();
    let f = &mut p.functions[0];
    f.entry_abi = EntryAbi::Observation;
    f.services = vec![ServiceKind::Packet, ServiceKind::Packet, ServiceKind::Packet,
        ServiceKind::Deps, ServiceKind::Packet];
    f.registers = vec![Type::Failure];
    f.params = vec![Location::Service(0)];
    f.blocks = vec![
        Block { pc: 0, params: f.params.clone() },
        Block { pc: 1, params: vec![Location::Service(1), Location::Service(2), Location::Service(3)] },
        Block { pc: 2, params: vec![Location::Service(4), Location::Register(0)] },
    ];
    f.proofs.selections.push(Selection { locations: vec![] });
    f.proofs.order_witnesses.push(OrderWitness { creation_pcs: vec![0] });
    f.proofs.plans.push(Plan { merge: true, selection: 0, order_witness: 0 });
    f.proofs.schedules.push(Schedule { services: vec![] });
    f.proofs.envelopes.push(Envelope { failures: 15 });
    f.proofs.states.push(StateRecord { pc: 0, locations: vec![Location::Service(0)] });
    f.proofs.roots.push(RootRequirement { pc: 0, locations: vec![Location::Service(0)] });
    f.code = vec![
        Instruction::Champ {
            op: Opcode::OwnSplitChecked,
            arguments: vec![Argument::Location(Location::Service(0)), Argument::Selection(0)],
            flow: Flow::Outcomes(vec![
                Outcome { tag: Tag::Normal, edge: Edge {
                    target_pc: 1, assignments: vec![produced(1, 0), produced(2, 1), produced(3, 2)],
                } },
                Outcome { tag: Tag::Unwind, edge: Edge {
                    target_pc: 2, assignments: vec![produced(4, 0), Assignment {
                        destination: Location::Register(0), source: Source::ProducedSlot(1),
                    }],
                } },
            ]),
        },
        Instruction::Ret(vec![]),
        Instruction::Ret(vec![]),
    ];
    p
}

#[test]
fn s1_codec_and_assembly_round_trip_tables_multi_results() {
    for p in [plain(), multi_result()] {
        let bytes = encode(&p).unwrap();
        assert_eq!(&bytes[..6], b"SPRW\0\x11");
        assert_eq!(decode(&bytes).unwrap(), p);
        let text = disassemble(&p).unwrap();
        assert!(text.starts_with("sparrow 17\n"));
        assert_eq!(assemble(&text).unwrap(), p);
        assert_eq!(disassemble(&assemble(&text).unwrap()).unwrap(), text);
    }
}

#[test]
fn s1_versions_are_checked_before_any_table() {
    let error = decode(b"SPRW\0\x10\xff").unwrap_err();
    assert!(error.message.contains("version 16"), "{error}");
    let error = crate::decode(b"SPRW\0\x11\xff").unwrap_err();
    assert!(error.message.contains("version 17"), "{error}");
    assert!(assemble("sparrow 16 garbage").unwrap_err().message.contains("sparrow 17"));
    assert!(crate::assemble("sparrow 17 garbage").is_err());
}

#[test]
fn s1_truncation_and_malformed_inputs_never_panic() {
    let p = multi_result();
    let bytes = encode(&p).unwrap();
    for end in 0..bytes.len() {
        assert!(decode(&bytes[..end]).is_err(), "prefix {end}");
    }
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(decode(&trailing).unwrap_err().message.contains("trailing"));
    let p = Program { entry: None, ..plain() };
    let bytes = encode(&p).unwrap();
    // `ret` with no values, no entry, no tests table, no composites and no globals (#709).
    assert_eq!(&bytes[bytes.len() - 6..], &[142, 0, 0, 0, 0, 0]);
    let prefix = &bytes[..bytes.len() - 6];
    for id in [0, 19, 25, 34, 35, 36, 47, 127, 128, u32::MAX] {
        let mut malformed = prefix.to_vec();
        malformed.push(255);
        let mut number = id;
        loop {
            let low = (number & 127) as u8;
            number >>= 7;
            malformed.push(if number == 0 { low } else { low | 128 });
            if number == 0 {
                break;
            }
        }
        let err = decode(&malformed).unwrap_err();
        assert!(err.message.contains("opcode"), "{id}: {err}");
    }
    for code in [0, 130, 131, 132, 133, 134, 135, 138, 139, 254] {
        let mut malformed = prefix.to_vec();
        malformed.push(code);
        assert!(decode(&malformed).unwrap_err().message.contains("opcode"));
    }
    for suffix in [&[0x80, 0][..], &[0xff; 5][..], &[0xff; 10][..]] {
        let mut malformed = b"SPRW\0\x11".to_vec();
        malformed.extend_from_slice(suffix);
        assert!(decode(&malformed).is_err());
    }
    let mut invalid_utf8 = b"SPRW\0\x11".to_vec();
    invalid_utf8.extend_from_slice(&[1, 255]);
    assert!(decode(&invalid_utf8).unwrap_err().message.contains("UTF-8"));
}

#[test]
fn s1_edges_reject_bad_arity_schema_and_cross_outcome_slots() {
    let mut p = multi_result();
    let Instruction::Champ { flow: Flow::Outcomes(outcomes), .. } = &mut p.functions[0].code[0] else {
        panic!("fixture");
    };
    outcomes[0].edge.assignments.pop();
    assert!(encode(&p).unwrap_err().message.contains("edge arity"));
    assert!(decode(&codec::encode_unchecked(&p)).unwrap_err().message.contains("edge arity"));
    let mut p = multi_result();
    let Instruction::Champ { flow: Flow::Outcomes(outcomes), .. } = &mut p.functions[0].code[0] else {
        panic!("fixture");
    };
    outcomes[1].edge.assignments[1].source = Source::ProducedSlot(2);
    assert!(encode(&p).is_err());
    let mut p = multi_result();
    let Instruction::Champ { flow: Flow::Outcomes(outcomes), .. } = &mut p.functions[0].code[0] else {
        panic!("fixture");
    };
    outcomes.swap(0, 1);
    assert!(encode(&p).unwrap_err().message.contains("outcome set or order"));
    let mut p = multi_result();
    let Instruction::Champ { flow: Flow::Outcomes(outcomes), .. } = &mut p.functions[0].code[0] else {
        panic!("fixture");
    };
    outcomes[1].tag = Tag::Normal;
    assert!(encode(&p).unwrap_err().message.contains("outcome set"));
}

#[test]
fn s1_registry_has_exact_p1_ids_and_normative_schemas() {
    let reserved = [0, 19, 25, 34, 35, 36];
    // P1 and the synchronous lanes take 57 IDs with the ten async IDs of #601 V6 (64..=95), and #676 adds
    // the dispatch ids 96 and 97 and the dynamic transfer 98; #678 adds the sort dispatch 99. #601 R4 assigns
    // take, its proof and rehome, and #601 D25 the dynamic settle and service materialize/absorb; #601 P1 assigns
    // spawn (65), #601 F1 the bound spawn (87, ruling PR-E), #855 the empty duty slot (88).
    assert_eq!(Opcode::ALL.len(), 70);
    for id in 0..47 {
        assert_eq!(Opcode::from_id(id).is_err(), reserved.contains(&id));
    }
    // #622: builders and lookup take 56..=61; plain await takes 64, 66 and 70..=77, resources 67..=69 and
    // 84..=86, spawn 65 (Champ draft 3 §14.1), the bound spawn 87 and the empty duty slot 88.
    for id in 47..96 {
        let assigned = (56..=61).contains(&id) || matches!(id, 64..=77 | 84..=88);
        assert_eq!(Opcode::from_id(id).is_ok(), assigned, "{id}");
    }
    assert_eq!(Opcode::from_id(96).unwrap(), Opcode::InvokeDispatch);
    assert_eq!(Opcode::from_id(97).unwrap(), Opcode::InvokeDispatchOwn);
    assert_eq!(Opcode::from_id(98).unwrap(), Opcode::OwnDynamic);
    assert_eq!(Opcode::from_id(99).unwrap(), Opcode::OrderDispatch);
    for id in 100..=103 {
        assert!(Opcode::from_id(id).is_err(), "{id}");
    }
    // array.order.dispatch yields its array type, and fails only when its dispatch may.
    let array = Type::Array(ArrayType { leaf: ArrayLeaf::Object(1), depth: 1 });
    let sorted = ResultContext { value: Some(array), ..ResultContext::default() };
    assert_eq!(Opcode::OrderDispatch.tags(&sorted), vec![Tag::Normal]);
    assert_eq!(Opcode::OrderDispatch.results(Tag::Normal, &sorted).unwrap(), vec![ProducedType::Value(array)]);
    let failing = ResultContext { may_fail: true, ..sorted.clone() };
    assert_eq!(Opcode::OrderDispatch.tags(&failing), vec![Tag::Normal, Tag::Unwind]);
    assert_eq!(Opcode::OrderDispatch.results(Tag::Unwind, &failing).unwrap(),
        vec![ProducedType::Value(Type::Failure)]);
    assert!(Opcode::OrderDispatch.is_terminator() && Opcode::OrderDispatch.helper().is_none());
    // own.dynamic joins the packet on success and returns state and packet with a failure on a failed check.
    let plain = ResultContext::default();
    assert_eq!(Opcode::OwnDynamic.tags(&plain), vec![Tag::Normal, Tag::Unwind]);
    assert_eq!(Opcode::OwnDynamic.results(Tag::Normal, &plain).unwrap(),
        vec![ProducedType::Service(ServiceKind::ScopeState)]);
    assert_eq!(Opcode::OwnDynamic.results(Tag::Unwind, &plain).unwrap(), vec![
        ProducedType::Service(ServiceKind::ScopeState), ProducedType::Service(ServiceKind::Packet),
        ProducedType::Value(Type::Failure),
    ]);
    assert!(Opcode::OwnDynamic.is_terminator());
    for &op in Opcode::ALL {
        assert_eq!(Opcode::from_id(op.id()).unwrap(), op);
        assert!(!op.name().contains("shield"));
    }
    use ProducedType::{Service as S, Value};
    use ServiceKind::*;
    let ctx = ResultContext { value: Some(Type::Int), between: true, may_fail: true,
        labels: vec![DeferLabel { label: 9, result: Some(Type::Int) }, DeferLabel { label: 10, result: None }],
        reserve: true, active_failure: false, buffer: None, slots: vec![] };
    assert_eq!(Opcode::DepsOf.id(), 40);
    assert_eq!(Opcode::InvokeObserve.id(), 41);
    assert_eq!(Opcode::DepsOf.results(Tag::Normal, &ctx).unwrap(), vec![S(Deps)]);
    assert_eq!(Opcode::InvokeObserve.results(Tag::Normal, &ctx).unwrap(), vec![Value(Type::Int)]);
    assert_eq!(Opcode::InvokeObserve.results(Tag::Unwind, &ctx).unwrap(), vec![Value(Type::Failure)]);
    // A dispatch has the outcomes and payloads of the call its slot ABI names.
    for tag in [Tag::Normal, Tag::Unwind] {
        assert_eq!(Opcode::InvokeDispatch.results(tag, &ctx).unwrap(),
            Opcode::InvokeObserve.results(tag, &ctx).unwrap());
        assert_eq!(Opcode::InvokeDispatchOwn.results(tag, &ctx).unwrap(),
            Opcode::InvokeCallOwn.results(tag, &ctx).unwrap());
    }
    assert_eq!(Opcode::InvokeDispatchOwn.tags(&ctx), Opcode::InvokeCallOwn.tags(&ctx));
    assert_eq!(Opcode::InvokeDispatchOwn.call_abi(), Opcode::InvokeCallOwn.call_abi());
    assert_eq!(Opcode::OwnSplitChecked.results(Tag::Normal, &ctx).unwrap(), vec![S(Packet), S(Packet), S(Deps)]);
    assert_eq!(Opcode::OwnSplitChecked.results(Tag::Unwind, &ctx).unwrap(), vec![S(Packet), Value(Type::Failure)]);
    assert_eq!(Opcode::ScopeSplit.results(Tag::Unwind, &ctx).unwrap(), vec![S(ScopeState), Value(Type::Failure)]);
    assert_eq!(Opcode::ScopeTransfer.results(Tag::Unwind, &ctx).unwrap(),
        vec![S(ScopeState), S(ScopeState), Value(Type::Failure)]);
    assert_eq!(Opcode::InvokeCallOwn.results(Tag::Normal, &ctx).unwrap(), vec![Value(Type::Int), S(Packet), S(Deps)]);
    assert_eq!(Opcode::InvokeCallOwn.results(Tag::Unwind, &ctx).unwrap(), vec![Value(Type::Failure)]);
    assert_eq!(Opcode::InvokeDefer.results(Tag::Replace(9), &ctx).unwrap(), vec![Value(Type::Int), S(Packet), S(Deps)]);
    assert!(Opcode::InvokeDefer.results(Tag::Replace(8), &ctx).is_err());
    assert_eq!(Opcode::InvokeDefer.results(Tag::Replace(10), &ctx).unwrap(), vec![S(Packet), S(Deps)]);
    assert!(Opcode::InvokeDefer.results(Tag::Continue, &ctx).unwrap().is_empty());
    assert_eq!(Opcode::ExitReplace.results(Tag::Normal, &ctx).unwrap(), vec![S(Pending), S(Packet)]);
    assert_eq!(Opcode::CleanupFinish.results(Tag::Normal, &ctx).unwrap(),
        vec![S(Closed), S(FailureState), S(ExitState)]);
    assert_eq!(Opcode::ExitDispatch.tags(&ctx), vec![Tag::Pending, Tag::Failure]);
    let ctx = ResultContext { active_failure: true, ..ctx };
    assert_eq!(Opcode::ExitDispatch.tags(&ctx), vec![Tag::Failure]);
    assert_eq!(Opcode::ExitDispatch.results(Tag::Failure, &ctx).unwrap(), vec![S(Closed), Value(Type::Failure)]);
    let ctx = ResultContext::default();
    assert_eq!(Opcode::ExitCommit.results(Tag::Normal, &ctx).unwrap(), vec![S(Packet), S(Deps)]);
    assert_eq!(Opcode::ScopeTransfer.results(Tag::Unwind, &ctx).unwrap(), vec![S(ScopeState), Value(Type::Failure)]);
}

#[test]
fn s1_mutated_binary_corpus_does_not_panic() {
    let bytes = encode(&multi_result()).unwrap();
    for index in 0..bytes.len() {
        for mask in [1, 128, 255] {
            let mut changed = bytes.clone();
            changed[index] ^= mask;
            let _ = decode(&changed);
        }
    }
}

#[test]
fn s1_plain_and_identity_layouts_round_trip_and_reject_bad_offsets() {
    let mut p = plain();
    p.objects.push(Object {
        error_cases: vec![],
        enum_cases: 0,
        variant_cases: vec![], handle: false, role: None, implements: vec![], any_box: false, enum_names: vec![],
        name: "Plain".into(),
        fields: vec![Field { name: "text".into(), storage: 0, once: false }],
        layout: ObjectRuntimeLayout {
            storage_fields: vec![Type::Str],
            allocation_words: 2,
            identity_offset: None,
            has_destroy: false,
            may_reach_resource: false,
            may_async_destroy: false,
            trace_layout: vec![0],
            selection_layout: vec![],
        },
    });
    let mut object = p.objects[0].clone();
    object.name = "Identity".into();
    object.layout.has_destroy = true;
    object.layout.may_reach_resource = true;
    object.layout.allocation_words = 6;
    object.layout.identity_offset = Some(16);
    p.objects.push(object);
    assert_eq!(decode(&encode(&p).unwrap()).unwrap(), p);
    p.objects[1].layout.identity_offset = Some(8);
    assert!(decode(&codec::encode_unchecked(&p)).unwrap_err().message.contains("identity_offset"));
}

#[test]
fn s1_multi_destination_normal_poststate_round_trip() {
    let mut p = plain();
    let f = &mut p.functions[0];
    f.services = vec![ServiceKind::Cleanup, ServiceKind::Closed, ServiceKind::FailureState, ServiceKind::ExitState];
    f.params = vec![Location::Service(0)];
    f.blocks[0].params = f.params.clone();
    f.code.insert(0, Instruction::Champ {
        op: Opcode::CleanupFinish,
        arguments: vec![Argument::Location(Location::Service(0))],
        flow: Flow::Next(vec![Location::Service(1), Location::Service(2), Location::Service(3)]),
    });
    assert_eq!(decode(&encode(&p).unwrap()).unwrap(), p);
    let text = disassemble(&p).unwrap();
    assert!(text.contains("champ cleanup.finish"));
    assert_eq!(assemble(&text).unwrap(), p);
    let Instruction::Champ { flow: Flow::Next(destinations), .. } = &mut p.functions[0].code[0] else {
        panic!("fixture");
    };
    destinations.swap(1, 2);
    assert!(decode(&codec::encode_unchecked(&p)).unwrap_err().message.contains("multi-result type"));
}

#[test]
fn s2_call_transport_uses_runtime_post_positions_without_machine_deps() {
    let context = ResultContext {
        value: Some(Type::Int), may_fail: true,
        labels: vec![DeferLabel { label: 17, result: Some(Type::Str) }, DeferLabel { label: 9, result: None }],
        ..ResultContext::default()
    };
    assert_eq!(Opcode::InvokeCallOwn.call_abi().unwrap().helper, "champ_call_owned");
    assert_eq!(Opcode::InvokeDefer.call_abi().unwrap().helper, "champ_invoke_defer");
    assert_eq!(Opcode::InvokeCallOwn.call_post_offsets(Tag::Normal, &context).unwrap(),
        [Some(0), Some(8), None]);
    assert_eq!(Opcode::InvokeCallOwn.call_post_offsets(Tag::Unwind, &context).unwrap(), [Some(16)]);
    assert_eq!(Opcode::InvokeDefer.tags(&context),
        [Tag::Continue, Tag::Replace(17), Tag::Replace(9), Tag::Unwind]);
    assert_eq!(Opcode::InvokeDefer.call_post_offsets(Tag::Continue, &context).unwrap(), []);
    assert_eq!(Opcode::InvokeDefer.call_post_offsets(Tag::Replace(9), &context).unwrap(), [Some(8), None]);
    assert!(Opcode::InvokeDefer.call_post_offsets(Tag::Replace(2), &context).is_err());
}

#[test]
fn s2_liveness_preserves_proof_and_state_only_roots() {
    let function = Function {
        symbol: "metadata".into(), entry_abi: EntryAbi::Observation,
        registers: vec![Type::Str, Type::Str, Type::Str], services: vec![ServiceKind::Packet],
        params: vec![Location::Register(0), Location::Register(1), Location::Service(0)],
        results: vec![Type::Str],
        blocks: vec![Block { pc: 0, params: vec![Location::Register(0), Location::Register(1),
            Location::Service(0)] }],
        proofs: FunctionProofs {
            dependencies: vec![Dependency { value: Location::Register(0), obligations: vec![Location::Register(1)] }],
            states: vec![StateRecord { pc: 1, locations: vec![Location::Service(0)] }],
            roots: vec![RootRequirement { pc: 1, locations: vec![Location::Register(2)] }],
            ..FunctionProofs::default()
        },
        code: vec![
            Instruction::Data { op: DataOp::Const, inputs: vec![], output: Some(2),
                immediate: Immediate::Constant(Constant::Str("temporary".into())) },
            Instruction::Ret(vec![0]),
        ],
    };
    let live = live_locations(&function).unwrap();
    assert!(live[0].contains(&Location::Register(1)));
    assert!(live[0].contains(&Location::Service(0)));
    let allocation = allocate(&function).unwrap();
    check_allocation(&function, &allocation).unwrap();
    assert_ne!(allocation.locations[&Location::Register(0)], allocation.locations[&Location::Register(1)]);
    let mut clobbered = allocation.clone();
    clobbered.locations.insert(Location::Register(1), clobbered.locations[&Location::Register(0)]);
    assert!(check_allocation(&function, &clobbered).is_err());
    let mut clobbered = allocation.clone();
    clobbered.locations.insert(Location::Register(2), clobbered.locations[&Location::Register(0)]);
    assert!(check_allocation(&function, &clobbered).unwrap_err().message.contains("clobbered"));
}

/// The dominance of the load check (#779) is the greatest fixed point of `dom(b) = {b} ∪ ⋂ dom(predecessors)`
/// over the blocks the entry reaches, as the bitsets before it computed; a block beyond the entry's reach
/// dominates only itself. Random CFGs with loops, joins, duplicate edges and unreachable parts.
#[test]
fn dominance_agrees_with_the_bitset_fixed_point() {
    fn bitsets(predecessors: &[Vec<usize>]) -> Vec<Vec<bool>> {
        let count = predecessors.len();
        let mut reachable = vec![false; count];
        reachable[0] = true;
        let mut changed = true;
        while changed {
            changed = false;
            for block in 1..count {
                if !reachable[block] && predecessors[block].iter().any(|&source| reachable[source]) {
                    reachable[block] = true;
                    changed = true;
                }
            }
        }
        let mut dom = vec![vec![true; count]; count];
        for block in 0..count {
            if block == 0 || !reachable[block] {
                dom[block] = (0..count).map(|other| other == block).collect();
            }
        }
        changed = true;
        while changed {
            changed = false;
            for block in (1..count).filter(|&block| reachable[block]) {
                let mut next = vec![true; count];
                for &source in predecessors[block].iter().filter(|&&source| reachable[source]) {
                    for (bit, &other) in next.iter_mut().zip(&dom[source]) {
                        *bit &= other;
                    }
                }
                next[block] = true;
                if next != dom[block] {
                    dom[block] = next;
                    changed = true;
                }
            }
        }
        dom
    }
    let mut state = 0x0779_d0d0_0779_d0d0u64;
    let mut next = |bound: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % bound as u64) as usize
    };
    let mut checked = 0;
    for _ in 0..3000 {
        let count = 1 + next(24);
        let mut predecessors = vec![Vec::new(); count];
        for source in 0..count {
            for _ in 0..next(4) {
                // No edge enters the entry block (ENTRY_BLOCK_TARGET).
                if count > 1 {
                    predecessors[1 + next(count - 1)].push(source);
                }
            }
        }
        let expected = bitsets(&predecessors);
        let dominance = super::verify::Dominance::of(&predecessors);
        for (block, row) in expected.iter().enumerate() {
            for (dominator, &dominates) in row.iter().enumerate() {
                assert_eq!(dominance.dominates(dominator, block), dominates,
                    "{dominator} over {block} in {predecessors:?}");
                checked += 1;
            }
        }
    }
    assert!(checked > 100_000, "{checked}");
}
