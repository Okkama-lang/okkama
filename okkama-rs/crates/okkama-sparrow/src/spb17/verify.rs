//! Structural, type, SSA/CFG and registry result-schema verification.
//! Executable artifacts additionally pass verify_load: service lifetimes, cleanup phases, order and roots.
use std::collections::{BTreeMap, BTreeSet};

use super::*;

type Check = Result<(), Error>;

fn require(condition: bool, message: &str) -> Check {
    if condition { Ok(()) } else { Err(error(message)) }
}

fn at<T>(items: &[T], index: u32) -> Result<&T, Error> {
    items.get(index as usize).ok_or_else(|| error(format!("invalid table/location index {index}")))
}

fn ty_valid(ty: Type, p: &Program) -> Check {
    if let Type::Object(id) | Type::Successor(id) = ty {
        at(&p.objects, id)?;
    }
    if let Type::Variant(id) = ty {
        enforce(p.objects.get(id as usize).is_some_and(|object| !object.variant_cases.is_empty()),
            "VARIANT_TYPE", "variant type names a declaration without variant cases")?;
    }
    if let Type::Array(shape) = ty {
        enforce(shape.depth > 0, "ARRAY_SHAPE", "array depth must be positive")?;
        if let ArrayLeaf::Object(id) = shape.leaf {
            let mut pending = vec![Type::Object(id)];
            let mut seen = BTreeSet::new();
            while let Some(ty) = pending.pop() {
                match ty {
                    Type::Object(id) if seen.insert(id) => {
                        let object = p.objects.get(id as usize)
                            .ok_or_else(|| violation("ARRAY_ELEMENT", "unknown array element object"))?;
                        enforce(!object.layout.has_destroy && !object.layout.may_reach_resource
                            && !object.layout.may_async_destroy, "ARRAY_PLAIN", "array element reaches resources")?;
                        pending.extend(&object.layout.storage_fields);
                    }
                    Type::Array(inner) => {
                        enforce(inner.depth > 0, "ARRAY_SHAPE", "array depth must be positive")?;
                        if let ArrayLeaf::Object(id) = inner.leaf {
                            pending.push(Type::Object(id));
                        }
                    }
                    Type::Int | Type::Bool | Type::Str | Type::Range | Type::Bytes | Type::Object(_)
                        | Type::Byte | Type::Float | Type::Char | Type::Rat => {}
                    _ => return Err(violation("ARRAY_PLAIN", "array element graph is not closed plain data")),
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn loc_type(f: &Function, location: Location) -> Result<ProducedType, Error> {
    match location {
        Location::Register(id) => Ok(ProducedType::Value(*at(&f.registers, id)?)),
        Location::Service(id) => Ok(ProducedType::Service(*at(&f.services, id)?)),
    }
}

fn constant_type(value: &Constant) -> Result<Type, Error> {
    match value {
        Constant::Int(text) => {
            let digits = text.strip_prefix('-').unwrap_or(text);
            require(!digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()), "invalid integer")?;
            require(digits == "0" || !digits.starts_with('0'), "non-canonical integer")?;
            require(text != "-0", "non-canonical integer")?;
            Ok(Type::Int)
        }
        Constant::Bool(_) => Ok(Type::Bool),
        Constant::Descriptor(_) => Ok(Type::Descriptor),
        Constant::Str(_) => Ok(Type::Str),
        Constant::Byte(value) => {
            enforce(*value <= 255, "CONST_BYTE", "byte constant outside 0..255")?;
            Ok(Type::Byte)
        }
        Constant::Char(value) => {
            enforce(char::from_u32(*value).is_some(), "CONST_CHAR", "char constant is not a Unicode scalar value")?;
            Ok(Type::Char)
        }
        Constant::Float(bits) => {
            let canonical = bits.parse::<u64>().is_ok_and(|value| value.to_string() == *bits);
            enforce(canonical, "CONST_FLOAT", "float constant is not a canonical binary64 bit pattern")?;
            Ok(Type::Float)
        }
    }
}

pub fn verify(p: &Program) -> Check {
    if let Some(entry) = p.entry {
        at(&p.functions, entry)?;
    }
    for object in &p.objects {
        let layout = &object.layout;
        let count = layout.storage_fields.len() as u64;
        let words = (1 + count + 4 * u64::from(layout.has_destroy)).max(2);
        require(u64::from(layout.allocation_words) == words, "invalid object allocation_words")?;
        let identity = if layout.has_destroy { Some(8 * (1 + count)) } else { None };
        require(layout.identity_offset.map(u64::from) == identity, "invalid object identity_offset")?;
        require(!layout.may_async_destroy, "async destroy is outside P1")?;
        require(!layout.has_destroy || layout.may_reach_resource, "destroy layout must reach self")?;
        let unique = |indices: &[u32]| -> Check {
            let mut seen = BTreeSet::new();
            for &index in indices {
                at(&layout.storage_fields, index)?;
                require(seen.insert(index), "duplicate object storage index")?;
            }
            Ok(())
        };
        if !object.variant_cases.is_empty() {
            let mut seen = BTreeSet::new();
            let plain = object.fields.is_empty() && layout.storage_fields.is_empty() && !layout.may_reach_resource
                && object.variant_cases.iter().all(|&case| seen.insert(case) && p.objects.get(case as usize)
                    .is_some_and(|case| case.variant_cases.is_empty() && !case.layout.has_destroy
                        && !case.layout.may_reach_resource));
            enforce(plain, "VARIANT_CASES", "variant cases must be distinct plain case objects")?;
        }
        unique(&object.fields.iter().map(|field| field.storage).collect::<Vec<_>>())?;
        unique(&layout.trace_layout)?;
        unique(&layout.selection_layout)?;
        for (index, &ty) in layout.storage_fields.iter().enumerate() {
            ty_valid(ty, p)?;
            if let Type::Object(id) | Type::Successor(id) = ty {
                require(layout.may_reach_resource || !p.objects[id as usize].layout.may_reach_resource,
                    "understated object retention metadata")?;
            }
            let traced = matches!(ty, Type::Error | Type::Bytes | Type::Array(_) | Type::Str | Type::Object(_)
                | Type::Successor(_) | Type::Failure | Type::Rat | Type::Variant(_));
            require(layout.trace_layout.contains(&(index as u32)) == traced, "incorrect object trace layout")?;
        }
    }
    for (index, scope) in p.tables.scopes.iter().enumerate() {
        at(&p.functions, scope.function)?;
        require(at(&p.tables.boundaries, scope.boundary)?.scope == index as u32, "scope boundary mismatch")?;
        if let Some(parent) = scope.parent {
            require((parent as usize) < index, "scope parent must precede child")?;
            require(at(&p.tables.scopes, parent)?.function == scope.function, "scope crosses function")?;
        }
    }
    for boundary in &p.tables.boundaries {
        at(&p.tables.scopes, boundary.scope)?;
    }
    for summary in &p.tables.summaries {
        let entry = at(&p.functions, summary.entry)?;
        for &param in &summary.retained_params {
            at(&entry.params, param)?;
        }
    }
    for f in &p.functions {
        verify_function(p, f).map_err(|e| Error { message: format!("{}: {}", f.symbol, e.message), ..e })?;
    }
    Ok(())
}

fn check_proofs(p: &Program, f: &Function) -> Check {
    for &ty in f.registers.iter().chain(&f.results) {
        ty_valid(ty, p)?;
    }
    let locations = |locations: &[Location]| -> Check {
        for &location in locations {
            loc_type(f, location)?;
        }
        Ok(())
    };
    if let EntryAbi::Defer { labels } = &f.entry_abi {
        let mut seen = BTreeSet::new();
        for label in labels {
            require(seen.insert(label.label), "duplicate defer label")?;
            if let Some(ty) = label.result {
                ty_valid(ty, p)?;
            }
        }
    }
    if let Some(entry) = &f.proofs.entry {
        require(entry.failures & !15 == 0, "invalid entry failure mask")?;
        if let Some(object) = entry.destroy { at(&p.objects, object)?; }
    }
    let mut begins = BTreeSet::new();
    for close in &f.proofs.closes {
        at(&f.code, close.begin_pc)?;
        at(&p.tables.boundaries, close.boundary)?;
        at(&f.proofs.envelopes, close.envelope)?;
        require(begins.insert(close.begin_pc), "duplicate close descriptor")?;
    }
    for evidence in &f.proofs.cleanup_plans {
        at(&f.proofs.plans, evidence.plan)?;
        at(&f.proofs.closes, evidence.close)?;
        for slot in &evidence.slots {
            loc_type(f, slot.identity)?;
            loc_type(f, slot.descriptor)?;
            at(&p.functions, slot.destroy)?;
            at(&f.code, slot.active_pc)?;
            match slot.origin {
                CreationOrigin::Singleton => {}
                CreationOrigin::Create(pc) => { at(&f.code, pc)?; }
                CreationOrigin::FreshResult { pc, callee } => {
                    at(&f.code, pc)?;
                    at(&p.functions, callee)?;
                }
            }
        }
        for suffix in &evidence.suffixes {
            locations(&suffix.remaining)?;
            locations(&suffix.roots)?;
        }
        for proof in evidence.preserves.iter().chain(&evidence.joins) {
            loc_type(f, proof.value)?;
            locations(&proof.obligations)?;
        }
    }
    for dependency in &f.proofs.dependencies {
        loc_type(f, dependency.value)?;
        locations(&dependency.obligations)?;
    }
    for registration in &f.proofs.registrations {
        let EntryAbi::Defer { labels } = &at(&p.functions, registration.entry)?.entry_abi else {
            return Err(error("registration requires defer entry"));
        };
        let mut seen = BTreeSet::new();
        for label in &registration.labels {
            require(seen.insert(label.label), "duplicate registration label")?;
            require(labels.iter().any(|declared| declared.label == label.label), "unknown registration label")?;
            at(&p.tables.boundaries, label.boundary)?;
        }
        require(seen.len() == labels.len(), "incomplete registration map")?;
    }
    for selection in &f.proofs.selections {
        locations(&selection.locations)?;
    }
    for plan in &f.proofs.plans {
        at(&f.proofs.selections, plan.selection)?;
        at(&f.proofs.order_witnesses, plan.order_witness)?;
    }
    for witness in &f.proofs.order_witnesses {
        for &pc in &witness.creation_pcs {
            at(&f.code, pc)?;
        }
    }
    for schedule in &f.proofs.schedules {
        require(schedule.services.is_empty(), "P1 D16 service schedule must be empty")?;
    }
    for envelope in &f.proofs.envelopes {
        require(envelope.failures & !15 == 0, "invalid P1 D18 failure mask")?;
    }
    for state in &f.proofs.states {
        at(&f.code, state.pc)?;
        locations(&state.locations)?;
    }
    for root in f.proofs.roots.iter().chain(&f.proofs.post_roots) {
        at(&f.code, root.pc)?;
        locations(&root.locations)?;
    }
    Ok(())
}

pub fn result_context(p: &Program, f: &Function, op: Opcode, args: &[Argument]) -> Result<ResultContext, Error> {
    let mut ctx = ResultContext::default();
    let mut flags = Vec::new();
    for arg in args {
        match *arg {
            Argument::Evaluation(ref evaluation) => { ty_valid(evaluation.result, p)?; }
            Argument::Result(result) => {
                if let Some(ty) = result { ty_valid(ty, p)?; }
            }
            Argument::Location(location) => { loc_type(f, location)?; }
            Argument::Scope(id) => { at(&p.tables.scopes, id)?; }
            Argument::Boundary(id) => { at(&p.tables.boundaries, id)?; }
            Argument::Summary(id) => {
                let summary = at(&p.tables.summaries, id)?;
                ctx.may_fail = summary.may_fail;
                ctx.value = at(&p.functions, summary.entry)?.results.first().copied();
            }
            Argument::Selection(id) => {
                for &location in &at(&f.proofs.selections, id)?.locations {
                    enforce(matches!(loc_type(f, location)?, ProducedType::Value(_)),
                        "OPERAND_VALUE", "selection must contain language values")?;
                }
            }
            Argument::Plan(id) => { at(&f.proofs.plans, id)?; }
            Argument::Schedule(id) => { at(&f.proofs.schedules, id)?; }
            Argument::Envelope(id) => { at(&f.proofs.envelopes, id)?; }
            Argument::Entry(id) => {
                let entry = at(&p.functions, id)?;
                ctx.value = entry.results.first().copied();
                if let EntryAbi::Defer { labels } = &entry.entry_abi {
                    ctx.labels = labels.clone();
                }
            }
            Argument::Object(id) => { at(&p.objects, id)?; }
            Argument::Destination(ExitDestination::Caller) => {}
            Argument::Destination(ExitDestination::Loop(id)) => { at(&p.tables.boundaries, id)?; }
            Argument::Destination(ExitDestination::Scope(id)) => { at(&p.tables.scopes, id)?; }
            Argument::Flag(flag) => flags.push(flag),
            Argument::Registration(id) => { at(&f.proofs.registrations, id)?; }
            Argument::Binding(_) | Argument::Field(_) | Argument::Label(_) | Argument::Kind(_) => {}
        }
    }
    if matches!(op, Opcode::ScopeTransfer | Opcode::OwnStage) {
        ctx.between = flags.first().copied().ok_or_else(|| error("missing Same/Between discriminator"))?;
    }
    if op == Opcode::ExitDispatch {
        require(flags.len() == 3, "exit.dispatch requires reserve/active-failure/may-fail close contract")?;
        ctx.reserve = flags[0];
        ctx.active_failure = flags[1];
        ctx.may_fail = flags[2];
    }
    if matches!(op, Opcode::InvokeCallOwn | Opcode::InvokeDefer) {
        require(args.iter().any(|arg| matches!(arg, Argument::Summary(_))), "missing result contract summary")?;
    }
    if op == Opcode::ExitCommit {
        // The load gate checks this schema against the typed pending, including through closes/edges.
        ctx.value = f.results.first().copied();
        if let Some(result) = args.iter().find_map(|arg| match arg {
            Argument::Result(result) => Some(*result), _ => None,
        }) {
            ctx.value = result;
        }
    }
    check_arguments(op, args)?;
    let locations = args.iter().filter_map(|arg| match arg {
        Argument::Location(location) => Some(*location),
        _ => None,
    }).collect::<Vec<_>>();
    let input = |index: usize, expected: ServiceKind| -> Check {
        let location = locations.get(index).ok_or_else(|| error("missing service operand"))?;
        require(loc_type(f, *location)? == ProducedType::Service(expected), "wrong service operand type")
    };
    use Opcode::*;
    use ServiceKind::*;
    let value = |index: usize| -> Check {
        enforce(locations.get(index).is_some_and(|&loc| matches!(loc_type(f, loc), Ok(ProducedType::Value(_)))),
            "OPERAND_VALUE", "value operand cannot contain a service")
    };
    match op {
        Catch => {
            enforce(loc_type(f, locations[0])? == ProducedType::Value(Type::Failure),
                "CATCH_FAILURE", "catch operand must be a failure")?;
            ctx.value = Some(match args.get(1) {
                Some(Argument::Object(id)) if !p.objects[*id as usize].error_cases.is_empty() => Type::Object(*id),
                Some(Argument::Flag(true)) => Type::Error,
                _ => return Err(violation("CATCH_TYPE", "catch target must be a declared plain Error")),
            });
        }
        Evaluate => {
            let Some(Argument::Evaluation(evaluation)) = args.first() else {
                return Err(violation("EVALUATE_OPERATION", "missing evaluation operation"));
            };
            let types = locations.iter().map(|&location| match loc_type(f, location)? {
                ProducedType::Value(ty) => Ok(ty),
                _ => Err(violation("EVALUATE_OPERAND", "evaluation cannot consume services")),
            }).collect::<Result<Vec<_>, _>>()?;
            enforce(evaluation.op.fallible(&types, Some(evaluation.result))
                || matches!(evaluation.op, DataOp::RangeEnd | DataOp::RangeLen),
                "EVALUATE_OPERATION", "operation is outside scalar outcomes")?;
            check_data_types(p, evaluation.op, &types, Some(evaluation.result), &evaluation.immediate)?;
            ctx.value = Some(evaluation.result);
            ctx.may_fail = true;
        }
        Fail => {
            let kind = args.iter().find_map(|arg| match arg { Argument::Label(id) => Some(*id), _ => None });
            enforce(matches!(kind, Some(0..=2)), "FAIL_KIND", "unknown failure kind")?;
            let ty = loc_type(f, locations[0])?;
            let valid = if kind == Some(0) {
                matches!(ty, ProducedType::Value(Type::Object(id)) if !p.objects[id as usize].error_cases.is_empty())
                    || ty == ProducedType::Value(Type::Error)
            } else {
                ty == ProducedType::Value(Type::Str)
            };
            enforce(valid, "FAIL_OPERAND", "failure payload differs from its kind")?;
        }
        ArrayGet => {
            enforce(locations.len() == 2, "ARRAY_GET_OPERANDS", "array.get needs array and index")?;
            let ProducedType::Value(Type::Array(shape)) = loc_type(f, locations[0])? else {
                return Err(violation("ARRAY_GET_OPERANDS", "array.get receiver must be a plain array"));
            };
            enforce(loc_type(f, locations[1])? == ProducedType::Value(Type::Int),
                "ARRAY_GET_OPERANDS", "array.get index must be int")?;
            ctx.value = shape.element();
            ctx.may_fail = true;
        }
        DepsOf => value(0)?,
        OwnMove | OwnJoin => {
            for index in 0..locations.len() {
                input(index, Packet)?;
            }
        }
        OwnSplit | OwnSplitChecked => {
            input(0, Packet)?;
            for index in 1..locations.len() { value(index)?; }
        }
        OwnAdmit | OwnAccept | OwnBind | OwnRebind => {
            input(0, ScopeState)?;
            input(locations.len() - 1, Packet)?;
            if matches!(op, OwnBind | OwnRebind) {
                value(1)?;
            }
        }
        ScopeSplit | DeferRegister => {
            input(0, ScopeState)?;
            for index in 1..locations.len() {
                value(index)?;
            }
        }
        ScopeTransfer | OwnStage => {
            let states = if ctx.between { 2 } else { 1 };
            let extra = if op == OwnStage { 2 } else { 0 };
            require(locations.len() == states + extra, "Same/Between operand arity mismatch")?;
            input(0, ScopeState)?;
            if ctx.between {
                input(1, ScopeState)?;
            }
            if op == OwnStage {
                input(locations.len() - 1, Packet)?;
                value(states)?;
                let object = args.iter().find_map(|arg| match arg {
                    Argument::Object(id) => Some(*id), _ => None,
                }).ok_or_else(|| violation("STAGE_FIELD_TYPE", "missing staging descriptor"))?;
                let field = args.iter().find_map(|arg| match arg {
                    Argument::Field(id) => Some(*id), _ => None,
                }).ok_or_else(|| violation("STAGE_FIELD_TYPE", "missing staging field"))?;
                let expected = at(&at(&p.objects, object)?.layout.storage_fields, field)?;
                enforce(loc_type(f, locations[states])? == ProducedType::Value(*expected),
                    "STAGE_FIELD_TYPE", "staged value differs from physical field type")?;
            }
        }
        ExitExtend => {
            input(0, Pending)?;
            input(1, Packet)?;
        }
        ExitPrepare => {
            input(locations.len() - 1, Packet)?;
            if locations.len() == 2 { value(0)?; }
        }
        ExitReplace => {
            input(0, Pending)?;
            input(locations.len() - 1, Packet)?;
            if locations.len() == 3 { value(1)?; }
        }
        ExitCommit => input(0, Pending)?,
        OwnCreateCommit => {
            let object = args.iter().find_map(|arg| match arg { Argument::Object(id) => Some(*id), _ => None })
                .ok_or_else(|| error("create_commit requires descriptor"))?;
            require(matches!(loc_type(f, locations[0])?,
                ProducedType::Value(Type::Object(id) | Type::Successor(id)) if id == object),
                "create_commit identity/descriptor mismatch")?;
        }
        CleanupBegin => {
            require(matches!(loc_type(f, locations[0])?, ProducedType::Service(Packet | ScopeState)),
                "cleanup.begin source must be packet or scope state")?;
            let rest = locations[1..].iter().map(|&loc| loc_type(f, loc)).collect::<Result<Vec<_>, _>>()?;
            require(matches!(rest.as_slice(), [] | [ProducedType::Service(Pending)]
                | [ProducedType::Value(Type::Failure)]
                | [ProducedType::Service(Pending), ProducedType::Value(Type::Failure)]),
                "cleanup.begin pending/failure types or ordering mismatch")?;
        }
        ExitAbort => {
            input(0, Pending)?;
            input(1, Cleanup)?;
        }
        CleanupSettle | CleanupPlan | CleanupNext | CleanupFinish => input(0, Cleanup)?,
        CleanupFailure => {
            input(0, Cleanup)?;
            require(loc_type(f, locations[1])? == ProducedType::Value(Type::Failure), "expected failure view")?;
        }
        CleanupDestroy => {
            input(0, Cleanup)?;
            input(1, StableKey)?;
        }
        DestroyDirect => {
            input(0, Cleanup)?;
            let object = args.iter().find_map(|arg| match arg { Argument::Object(id) => Some(*id), _ => None })
                .ok_or_else(|| error("direct destroy requires descriptor"))?;
            require(loc_type(f, locations[1])? == ProducedType::Value(Type::Object(object)),
                "direct destroy identity/descriptor mismatch")?;
        }
        ExitDispatch => {
            input(0, Closed)?;
            input(1, FailureState)?;
            input(2, ExitState)?;
        }
        FailureDispatch => input(0, FailureState)?,
        InvokeObserve => {
            let entry = args.iter().find_map(|arg| match arg { Argument::Entry(id) => Some(*id), _ => None })
                .ok_or_else(|| error("missing observation entry"))?;
            let summary = args.iter().find_map(|arg| match arg { Argument::Summary(id) => Some(*id), _ => None })
                .ok_or_else(|| error("missing observation summary"))?;
            require(at(&p.tables.summaries, summary)?.entry == entry, "entry/summary mismatch")?;
            let callee = at(&p.functions, entry)?;
            require(callee.entry_abi == EntryAbi::Observation, "observation entry ABI mismatch")?;
            let expected = callee.params.iter().map(|&loc| loc_type(callee, loc)).collect::<Result<Vec<_>, _>>()?;
            let actual = locations.iter().map(|&loc| loc_type(f, loc)).collect::<Result<Vec<_>, _>>()?;
            require(actual == expected, "observation argument schema mismatch")?;
        }
        InvokeCallOwn | InvokeDefer => {
            let entry = args.iter().find_map(|arg| match arg { Argument::Entry(id) => Some(*id), _ => None })
                .ok_or_else(|| error("missing entry"))?;
            let summary = args.iter().find_map(|arg| match arg { Argument::Summary(id) => Some(*id), _ => None })
                .ok_or_else(|| error("missing summary"))?;
            require(at(&p.tables.summaries, summary)?.entry == entry, "entry/summary mismatch")?;
            if op == InvokeDefer {
                let map = args.iter().find_map(|arg| match arg {
                    Argument::Registration(id) => Some(*id),
                    _ => None,
                }).ok_or_else(|| error("missing defer registration map"))?;
                require(at(&f.proofs.registrations, map)?.entry == entry, "defer registration/entry mismatch")?;
            }
            let abi = &at(&p.functions, entry)?.entry_abi;
            require(matches!((op, abi), (InvokeCallOwn, EntryAbi::Owned) | (InvokeDefer, EntryAbi::Defer { .. })),
                "invoke entry ABI mismatch")?;
            let callee = at(&p.functions, entry)?;
            let language = callee.params.iter().map(|&location| loc_type(callee, location))
                .collect::<Result<Vec<_>, _>>()?.into_iter()
                .filter(|ty| matches!(ty, ProducedType::Value(_))).collect::<Vec<_>>();
            require(locations.len() > language.len(), "owning call requires input packet")?;
            for (&location, expected) in locations.iter().zip(&language) {
                require(loc_type(f, location)? == *expected, "owning call language argument type mismatch")?;
            }
            input(language.len(), Packet)?;
            for &location in &locations[language.len() + 1..] {
                require(loc_type(f, location)? == ProducedType::Service(Deps), "expected proof-only call deps")?;
            }
            require(callee.params.iter().filter(|&&location| {
                loc_type(callee, location) == Ok(ProducedType::Service(Packet))
            }).count() == 1, "owning entry requires exactly one input packet")?;
            if op == InvokeCallOwn {
                require(flags.len() == 1, "owned call requires explicit cleanup mode")?;
            }
        }
        RetOwn => {
            require(f.entry_abi == EntryAbi::Owned, "ret.own requires owned entry")?;
            let mut expected = f.results.iter().copied().map(ProducedType::Value).collect::<Vec<_>>();
            expected.push(ProducedType::Service(Packet));
            let actual = locations.iter().map(|&location| loc_type(f, location)).collect::<Result<Vec<_>, _>>()?;
            // ret.own's runtime payload is value?, packet; explicit deps is optional proof metadata.
            if actual.last() == Some(&ProducedType::Service(Deps)) {
                expected.push(ProducedType::Service(Deps));
            }
            require(actual == expected, "ret.own result schema mismatch")?;
        }
        RetDefer => {
            let EntryAbi::Defer { labels } = &f.entry_abi else {
                return Err(error("ret.defer requires defer entry"));
            };
            let returned = args.iter().filter_map(|arg| match arg {
                Argument::Label(label) => Some(*label), _ => None,
            }).collect::<Vec<_>>();
            require(returned.len() <= 1, "ret.defer needs one tag")?;
            let expected = if let Some(&label) = returned.first() {
                Opcode::InvokeDefer.results(Tag::Replace(label), &ResultContext {
                    labels: labels.clone(), ..ResultContext::default()
                })?
            } else {
                vec![]
            };
            let actual = locations.iter().map(|&location| loc_type(f, location)).collect::<Result<Vec<_>, _>>()?;
            require(actual == expected, "ret.defer result schema mismatch")?;
        }
        _ => {}
    }
    Ok(ctx)
}

fn check_arguments(op: Opcode, args: &[Argument]) -> Check {
    use Argument::*;
    use Opcode::*;
    // S1 freezes references and result positions. Rich consume/lifetime maps
    // and binding/defer contracts are verified by the later ownership gate.
    let kind = |arg: &Argument| -> char {
        match arg {
            Evaluation(_) => 'z', Result(_) => 'r', Location(_) => 'l', Scope(_) => 's', Boundary(_) => 'b',
            Summary(_) => 'u',
            Selection(_) => 'e', Plan(_) => 'p', Schedule(_) => 'd', Envelope(_) => 'v',
            Entry(_) => 'f', Object(_) => 'o', Binding(_) => 'i', Field(_) => 't',
            Label(_) => 'a', Flag(_) => '?', Kind(_) => 'k', Registration(_) => 'm', Destination(_) => 'x',
        }
    };
    let shape: String = args.iter().map(kind).collect();
    let valid = match op {
        Catch => shape == "lo" || shape == "l?",
        Evaluate => shape.starts_with('z') && shape[1..].chars().all(|ch| ch == 'l'),
        Fail => shape == "al",
        ArrayGet => shape == "ll",
        ScopeBegin => shape == "sb",
        OwnEmpty | DepsEmpty => shape.is_empty(),
        OwnMove | DepsOf => shape == "l",
        OwnJoin => !shape.is_empty() && shape.chars().all(|ch| ch == 'l'),
        OwnAdmit | OwnAccept => shape == "ll",
        OwnBind | OwnRebind => shape == "lisll",
        OwnStage => shape == "?lotll" || shape == "?llotll",
        OwnSplit => shape == "le",
        OwnSplitChecked => shape.starts_with("le") && shape[2..].chars().all(|ch| ch == 'l'),
        ScopeSplit => shape.starts_with("le") && shape[2..].chars().all(|ch| ch == 'l' || ch == '?')
            && args.iter().filter(|arg| matches!(arg, Flag(_))).count() <= 1,
        ScopeTransfer => shape == "?le" || shape == "?lle",
        DeferRegister => shape.starts_with("lm") && shape[2..].chars().all(|ch| ch == 'l' || ch == 'i'),
        ExitPrepare => shape == "kble" || shape == "kblle",
        ExitExtend | ExitAbort | CleanupFailure | CleanupDestroy => shape == "ll",
        ExitReplace => shape == "lkble" || shape == "lkblle",
        ExitCommit => matches!(shape.as_str(), "lx" | "lb" | "lxr"),
        CleanupBegin => matches!(shape.as_str(), "lbv" | "llbv" | "lllbv"),
        CleanupSettle => shape == "ld",
        CleanupPlan => shape == "lp",
        CleanupNext | CleanupFinish | FailureDispatch => shape == "l",
        DestroyDirect => shape == "llof",
        InvokeObserve => shape.starts_with("fu") && shape[2..].chars().all(|ch| ch == 'l'),
        InvokeCallOwn => shape.starts_with("fu") && shape[2..].chars().all(|ch| ch == 'l' || ch == '?'),
        InvokeDefer => shape.starts_with("fum") && shape[3..].chars().all(|ch| ch == 'l' || ch == 'i'),
        RetOwn | RetDefer => shape.chars().all(|ch| matches!(ch, 'l' | 'a')),
        ExitDispatch => shape == "lll???",
        OwnCreateCommit => shape == "lob",
    };
    require(valid, &format!("invalid operands for {}", op.name()))
}

fn check_data(p: &Program, f: &Function, op: DataOp, inputs: &[u32], output: Option<u32>, imm: &Immediate) -> Check {
    let types = inputs.iter().map(|&id| at(&f.registers, id).copied()).collect::<Result<Vec<_>, _>>()?;
    let out = output.map(|id| at(&f.registers, id).copied()).transpose()?;
    check_data_types(p, op, &types, out, imm)
}

fn check_data_types(p: &Program, op: DataOp, types: &[Type], out: Option<Type>, imm: &Immediate) -> Check {
    use DataOp::*;
    let expect = |args: &[Type], result: Option<Type>| -> Check {
        require(types == args && out == result, "scalar operand/result type or arity mismatch")
    };
    let immediate_none = !matches!(op, Const | RangeNew | ObjNew | ObjInit | ObjGet | ObjNext | EnumCase | VariantGet);
    if immediate_none {
        require(*imm == Immediate::None, "unexpected scalar immediate")?;
    }
    match op {
        EnumCase => {
            let Immediate::Updates(indices) = imm else {
                return Err(violation("ENUM_CASE", "enum.case requires descriptor and case"));
            };
            let [id, case] = indices.as_slice() else {
                return Err(violation("ENUM_CASE", "enum.case requires descriptor and case"));
            };
            let object = at(&p.objects, *id)?;
            enforce(object.fields.is_empty() && object.layout.storage_fields == [Type::Int]
                && !object.layout.may_reach_resource
                && ((*case as usize) < object.error_cases.len() || *case < object.enum_cases),
                "ENUM_CASE", "invalid plain enum case")?;
            expect(&[], Some(Type::Object(*id)))
        }
        ArrayNew => {
            let Some(Type::Array(shape)) = out else {
                return Err(violation("ARRAY_NEW_TYPE", "array.new must produce a plain array"));
            };
            enforce(shape.element().is_some_and(|element| types.iter().all(|ty| *ty == element)),
                "ARRAY_NEW_TYPE", "array.new element types differ from the result element")
        }
        ArrayLen => enforce(matches!(types, [Type::Array(_) | Type::Bytes]) && out == Some(Type::Int),
            "ARRAY_LEN_TYPE", "array.len needs a plain array and an int result"),
        Const => {
            let Immediate::Constant(value) = imm else { return Err(error("missing constant")); };
            if let Constant::Descriptor(id) = value {
                at(&p.objects, *id)?;
            }
            expect(&[], Some(constant_type(value)?))
        }
        FailSecondary | FailCaused => expect(&[Type::Failure, Type::Failure], Some(Type::Failure)),
        IntAdd | IntSub | IntMul | IntMod => expect(&[Type::Int, Type::Int], Some(Type::Int)),
        IntNeg => expect(&[Type::Int], Some(Type::Int)),
        IntEq | IntNe | IntLt | IntLe | IntGt | IntGe => expect(&[Type::Int, Type::Int], Some(Type::Bool)),
        BoolNot => expect(&[Type::Bool], Some(Type::Bool)),
        BoolEq | BoolNe => expect(&[Type::Bool, Type::Bool], Some(Type::Bool)),
        StrConcat => expect(&[Type::Str, Type::Str], Some(Type::Str)),
        StrEq | StrNe | StrLt | StrLe | StrGt | StrGe => expect(&[Type::Str, Type::Str], Some(Type::Bool)),
        StrLen => expect(&[Type::Str], Some(Type::Int)),
        ByteEq | ByteNe | ByteLt | ByteLe | ByteGt | ByteGe => expect(&[Type::Byte, Type::Byte], Some(Type::Bool)),
        CharEq | CharNe | CharLt | CharLe | CharGt | CharGe => expect(&[Type::Char, Type::Char], Some(Type::Bool)),
        FloatEq | FloatNe | FloatLt | FloatLe | FloatGt | FloatGe => {
            expect(&[Type::Float, Type::Float], Some(Type::Bool))
        }
        RatEq | RatNe | RatLt | RatLe | RatGt | RatGe => expect(&[Type::Rat, Type::Rat], Some(Type::Bool)),
        RatNew => expect(&[Type::Int, Type::Int], Some(Type::Rat)),
        EnumEq | EnumNe => {
            let enumeration = match types {
                [Type::Object(left), Type::Object(right)] if left == right => {
                    let object = at(&p.objects, *left)?;
                    object.fields.is_empty() && object.layout.storage_fields == [Type::Int]
                        && (!object.error_cases.is_empty() || object.enum_cases > 0)
                }
                _ => false,
            };
            enforce(enumeration && out == Some(Type::Bool), "ENUM_EQ_TYPE",
                "enum comparison needs two values of one plain enum and a bool result")
        }
        VariantTag => enforce(matches!(types, [Type::Variant(_)]) && out == Some(Type::Int), "VARIANT_TAG",
            "variant.tag needs a variant value and an int result"),
        VariantGet => {
            let field = match (types, imm) {
                ([Type::Variant(variant)], Immediate::Updates(indices)) => match indices.as_slice() {
                    [case, storage] => p.objects[*variant as usize].variant_cases.get(*case as usize)
                        .and_then(|&case| p.objects.get(case as usize))
                        .filter(|case| case.fields.iter().any(|field| field.storage == *storage))
                        .and_then(|case| case.layout.storage_fields.get(*storage as usize).copied()),
                    _ => None,
                },
                _ => None,
            };
            enforce(field.is_some() && out == field, "VARIANT_GET",
                "variant.get needs a case of the variant and one of its payload fields")
        }
        ValEq | ValNe => {
            let object = matches!(types, [Type::Object(left), Type::Object(right)] if left == right);
            enforce(object && out == Some(Type::Bool), "VAL_EQ_TYPE",
                "value comparison needs two objects of one declaration and a bool result")
        }
        Convert => enforce(matches!((types, out), ([from], Some(to)) if conversion(*from, to).is_some()),
            "CONVERT_TYPE", "conversion between these types is not defined"),
        Move => {
            require(types.len() == 1, "move requires one source")?;
            let valid = out == Some(types[0]) || matches!((types[0], out),
                (Type::Successor(a), Some(Type::Object(b))) if a == b)
                || matches!((types[0], out), (Type::Object(id), Some(Type::Error))
                    if !p.objects[id as usize].error_cases.is_empty())
                || matches!((types[0], out), (Type::Object(case), Some(Type::Variant(variant)))
                    if p.objects[variant as usize].variant_cases.contains(&case));
            require(valid, "invalid move/succ.take type")
        }
        Undef => require(types.is_empty() && out.is_some(), "invalid undef"),
        RangeNew => {
            require(matches!(imm, Immediate::Inclusive(_)), "range.new requires inclusive flag")?;
            expect(&[Type::Int, Type::Int], Some(Type::Range))
        }
        RangeStart | RangeEnd | RangeLen => expect(&[Type::Range], Some(Type::Int)),
        ObjNew => {
            let Immediate::Object(id) = *imm else { return Err(error("missing object descriptor")); };
            let object = at(&p.objects, id)?;
            let fields = object.fields.iter().map(|field| object.layout.storage_fields[field.storage as usize])
                .collect::<Vec<_>>();
            expect(&fields, Some(Type::Object(id)))
        }
        ObjGet | ObjInit | ObjNext => {
            let Some(Type::Object(id) | Type::Successor(id)) = types.first().copied() else {
                return Err(error("expected object receiver"));
            };
            let layout = &at(&p.objects, id)?.layout;
            let field_type = |index: u32| -> Result<Type, Error> {
                require(at(&p.objects, id)?.fields.iter().any(|field| field.storage == index),
                    "field is hidden storage or identity tail")?;
                Ok(*at(&layout.storage_fields, index)?)
            };
            if op == ObjNext {
                let Immediate::Updates(indices) = imm else { return Err(error("missing successor updates")); };
                let mut fields = vec![types[0]];
                let mut seen = BTreeSet::new();
                for &index in indices {
                    require(seen.insert(index), "duplicate successor update")?;
                    fields.push(field_type(index)?);
                }
                expect(&fields, Some(Type::Successor(id)))
            } else {
                let Immediate::Field(index) = *imm else { return Err(error("missing field index")); };
                let field = field_type(index)?;
                match op {
                    ObjGet => expect(&[types[0]], Some(field)),
                    ObjInit => expect(&[types[0], field], None),
                    _ => unreachable!("object operation"),
                }
            }
        }
    }
}

fn terminates(inst: &Instruction) -> bool {
    match inst {
        Instruction::Data { .. } => false,
        Instruction::Champ { op, flow, .. } => matches!(op, Opcode::RetOwn | Opcode::RetDefer)
            || matches!(flow, Flow::Outcomes(_)),
        _ => true,
    }
}

fn edges(inst: &Instruction) -> Vec<(&Edge, Vec<ProducedType>)> {
    match inst {
        Instruction::Br(edge) => vec![(edge, vec![])],
        Instruction::Cond { yes, no, .. } => vec![(yes, vec![]), (no, vec![])],
        _ => vec![],
    }
}

pub(crate) fn checked_edges<'a>(p: &Program, f: &Function, inst: &'a Instruction)
    -> Result<Vec<(&'a Edge, Vec<ProducedType>)>, Error>
{
    if let Instruction::Champ { op, arguments, flow } = inst {
        let ctx = result_context(p, f, *op, arguments)?;
        let tags = op.tags(&ctx);
        if *op == Opcode::ArrayGet {
            enforce(matches!(flow, Flow::Outcomes(outcomes)
                if outcomes.iter().map(|outcome| outcome.tag).collect::<Vec<_>>() == tags),
                "ARRAY_GET_OUTCOMES", "array.get requires ordered normal and unwind outcomes")?;
        }
        match flow {
            Flow::Next(destinations) => {
                require(!op.is_terminator() || matches!(op, Opcode::RetOwn | Opcode::RetDefer)
                    || matches!(op, Opcode::InvokeCallOwn | Opcode::InvokeObserve) && !ctx.may_fail,
                    "terminator requires outcomes")?;
                if *op == Opcode::DepsOf {
                    let [Argument::Location(anchor)] = arguments.as_slice() else { unreachable!() };
                    require(destinations.len() == 1 && f.proofs.dependencies.iter().any(|dependency| {
                        dependency.value == destinations[0] && dependency.obligations.contains(anchor)
                    }), "deps.of must retain its full-deps anchor")?;
                }
                let results = op.results(Tag::Normal, &ctx)?;
                require(destinations.len() == results.len(), "bad multi-result arity")?;
                for (&loc, &ty) in destinations.iter().zip(&results) {
                    require(loc_type(f, loc)? == ty, "bad multi-result type")?;
                }
                Ok(vec![])
            }
            Flow::Outcomes(outcomes) => {
                require(op.is_terminator() && !matches!(op, Opcode::RetOwn | Opcode::RetDefer),
                    "non-outcome operation has outcomes")?;
                let actual = outcomes.iter().map(|outcome| outcome.tag).collect::<Vec<_>>();
                require(actual == tags, "invalid outcome set or order")?;
                outcomes.iter().map(|outcome| Ok((&outcome.edge, op.results(outcome.tag, &ctx)?))).collect()
            }
        }
    } else {
        Ok(edges(inst))
    }
}

fn uses(inst: &Instruction) -> Vec<Location> {
    let mut uses = match inst {
        Instruction::Data { inputs, .. } | Instruction::Ret(inputs) =>
            inputs.iter().copied().map(Location::Register).collect(),
        Instruction::Cond { condition, .. } => vec![Location::Register(*condition)],
        Instruction::Raise(value) => vec![Location::Register(*value)],
        Instruction::Champ { arguments, .. } => arguments.iter().filter_map(|arg| match arg {
            Argument::Location(location) => Some(*location),
            _ => None,
        }).collect(),
        _ => vec![],
    };
    let ordinary = edges(inst);
    let champ = match inst {
        Instruction::Champ { flow: Flow::Outcomes(outcomes), .. } =>
            outcomes.iter().map(|outcome| &outcome.edge).collect::<Vec<_>>(),
        _ => vec![],
    };
    for edge in ordinary.iter().map(|(edge, _)| *edge).chain(champ) {
        uses.extend(edge.assignments.iter().filter_map(|assignment| match assignment.source {
            Source::Location(location) => Some(location),
            Source::ProducedSlot(_) => None,
        }));
    }
    uses
}

fn verify_function(p: &Program, f: &Function) -> Check {
    check_proofs(p, f)?;
    if f.proofs.entry.as_ref().is_some_and(|entry| entry.external.is_some()) {
        require(f.code.is_empty() && f.blocks.is_empty(), "external entry cannot carry code")?;
        for &param in &f.params {
            loc_type(f, param)?;
        }
        return Ok(());
    }
    require(f.results.len() <= 1, "P1 entry has at most one language result")?;
    require(!f.blocks.is_empty() && !f.code.is_empty(), "function needs a body")?;
    require(f.blocks[0].pc == 0 && f.blocks[0].params == f.params, "invalid entry block")?;
    let mut pcs = BTreeMap::new();
    let mut definitions = BTreeMap::new();
    for (index, block) in f.blocks.iter().enumerate() {
        at(&f.code, block.pc)?;
        require(pcs.insert(block.pc, index).is_none(), "duplicate block pc")?;
        if index > 0 {
            require(block.pc > f.blocks[index - 1].pc, "blocks must be ordered by pc")?;
        }
        for &param in &block.params {
            loc_type(f, param)?;
            require(definitions.insert(param, (index, None)).is_none(), "duplicate block parameter")?;
        }
    }
    let metadata = allocation::metadata_uses(f);
    let mut predecessors = vec![Vec::new(); f.blocks.len()];
    for (block_index, block) in f.blocks.iter().enumerate() {
        let end = f.blocks.get(block_index + 1).map_or(f.code.len(), |block| block.pc as usize);
        for pc in block.pc as usize..end {
            let inst = &f.code[pc];
            require(terminates(inst) == (pc + 1 == end), "missing or early block terminator")?;
            match inst {
                Instruction::Data { op, inputs, output, immediate } => {
                    check_data(p, f, *op, inputs, *output, immediate)?;
                    if let Some(dst) = output {
                        require(definitions.insert(Location::Register(*dst), (block_index, Some(pc))).is_none(),
                            "register has multiple SSA definitions")?;
                    }
                }
                Instruction::Cond { condition, .. } => {
                    require(*at(&f.registers, *condition)? == Type::Bool, "condition must be bool")?;
                }
                Instruction::Raise(value) => {
                    require(at(&f.registers, *value)? == &Type::Failure, "raise needs failure")?;
                }
                Instruction::Ret(values) => {
                    enforce(f.entry_abi == EntryAbi::Observation, "ENTRY_RETURN_ABI",
                        "owning entries must return through ret.own/ret.defer")?;
                    let types = values.iter().map(|&id| at(&f.registers, id).copied())
                        .collect::<Result<Vec<_>, _>>()?;
                    require(types == f.results, "return type/arity mismatch")?;
                }
                Instruction::Champ { flow: Flow::Next(destinations), .. } => {
                    for &dst in destinations {
                        require(definitions.insert(dst, (block_index, Some(pc))).is_none(),
                            "location has multiple SSA definitions")?;
                    }
                }
                _ => {}
            }
            for (edge, produced) in checked_edges(p, f, inst)? {
                let target = *pcs.get(&edge.target_pc).ok_or_else(|| error("edge target is not a block"))?;
                let params = &f.blocks[target].params;
                require(edge.assignments.len() == params.len(), "bad edge arity")?;
                for (assignment, &param) in edge.assignments.iter().zip(params) {
                    require(assignment.destination == param, "edge destinations must match ordered block parameters")?;
                    let source = match assignment.source {
                        Source::Location(location) => loc_type(f, location)?,
                        Source::ProducedSlot(slot) => *at(&produced, slot)?,
                    };
                    require(loc_type(f, param)? == source, "edge assignment type mismatch")?;
                }
                predecessors[target].push(block_index);
            }
        }
    }
    // Bounded dominator bitsets: hostile CFGs cannot cause unbounded verifier
    // memory expansion. This check is independent of the decoder's budget.
    let count = f.blocks.len();
    let words = count.div_ceil(64);
    require(count.saturating_mul(words).saturating_mul(8) <= 64 * 1024 * 1024,
        "CFG exceeds dominator memory budget")?;
    let mut reachable = vec![false; count];
    reachable[0] = true;
    loop {
        let mut changed = false;
        for index in 1..count {
            if !reachable[index] && predecessors[index].iter().any(|&pred| reachable[pred]) {
                reachable[index] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut dom = vec![vec![u64::MAX; words]; count];
    for index in 0..count {
        if index == 0 || !reachable[index] {
            dom[index].fill(0);
            dom[index][index / 64] |= 1 << (index % 64);
        }
    }
    loop {
        let mut changed = false;
        for index in 1..count {
            if !reachable[index] {
                continue;
            }
            let mut next = vec![u64::MAX; words];
            for &pred in predecessors[index].iter().filter(|&&pred| reachable[pred]) {
                for (word, &other) in next.iter_mut().zip(&dom[pred]) {
                    *word &= other;
                }
            }
            next[index / 64] |= 1 << (index % 64);
            if next != dom[index] {
                dom[index] = next;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    for (block_index, block) in f.blocks.iter().enumerate() {
        let end = f.blocks.get(block_index + 1).map_or(f.code.len(), |block| block.pc as usize);
        for (pc, metadata) in metadata.iter().enumerate().take(end).skip(block.pc as usize) {
            let mut used = allocation::direct_uses(f, pc);
            used.extend(uses(&f.code[pc]));
            used.extend(metadata);
            allocation::close_dependencies(f, &mut used);
            for used in used {
                let &(def_block, def_pc) = definitions.get(&used).ok_or_else(|| error("undefined location"))?;
                require(dom[block_index][def_block / 64] & (1 << (def_block % 64)) != 0,
                    "definition does not dominate use")?;
                require(def_block != block_index || def_pc.is_none_or(|defined| defined < pc),
                    "location used before definition")?;
                if let Location::Register(reg) = used
                    && let Some(defined) = def_pc
                    && matches!(f.code[defined], Instruction::Data { op: DataOp::Undef, .. })
                {
                    let allowed = match &f.code[pc] {
                        Instruction::Data { op: DataOp::ObjNew, .. } => true,
                        Instruction::Data { op: DataOp::ObjInit | DataOp::ObjNext, inputs, .. } => {
                            inputs.first() != Some(&reg)
                        }
                        _ => false,
                    };
                    require(allowed, &format!("undef r{reg} outside construction fields at {pc}: {:?}", f.code[pc]))?;
                }
            }
        }
    }
    check_successors(p, f, &pcs)?;
    Ok(())
}

/// Successor references are copyable, but the independent must-use permission
/// must be taken or returned exactly once on every reachable path. Edge renames
/// consume all sources before creating destinations, including cyclic moves.
fn check_successors(p: &Program, f: &Function, pcs: &BTreeMap<u32, usize>) -> Check {
    let successor = |location| matches!(loc_type(f, location), Ok(ProducedType::Value(Type::Successor(_))));
    let mut incoming: Vec<Option<BTreeSet<Location>>> = vec![None; f.blocks.len()];
    incoming[0] = Some(f.params.iter().copied().filter(|&param| successor(param)).collect());
    let mut stored = incoming[0].as_ref().map_or(0, BTreeSet::len);
    let mut work = vec![0];
    while let Some(index) = work.pop() {
        let mut active = incoming[index].as_ref().expect("queued block").clone();
        let block = &f.blocks[index];
        let end = f.blocks.get(index + 1).map_or(f.code.len(), |block| block.pc as usize);
        for pc in block.pc as usize..end {
            let inst = &f.code[pc];
            if let Instruction::Data { op, inputs, output, .. } = inst {
                if *op == DataOp::Move && inputs.first().is_some_and(|&id| successor(Location::Register(id))) {
                    require(active.remove(&Location::Register(inputs[0])), "successor permission already taken")?;
                }
                if let Some(dst) = output {
                    let location = Location::Register(*dst);
                    if successor(location) {
                        require(matches!(op, DataOp::ObjNext | DataOp::Move), "invalid successor producer")?;
                        require(active.insert(location), "successor permission overwritten")?;
                    }
                }
            }
            if let Instruction::Ret(values) = inst {
                for &value in values {
                    let location = Location::Register(value);
                    if successor(location) {
                        require(active.remove(&location), "successor returned twice or after take")?;
                    }
                }
                require(active.is_empty(), "return loses successor must-use permission")?;
            }
            if matches!(inst, Instruction::Unreachable) {
                require(active.is_empty(), "unreachable loses successor must-use permission")?;
            }
            for (edge, _) in checked_edges(p, f, inst)? {
                let mut next = active.clone();
                let mut consumed = BTreeSet::new();
                let mut destinations = Vec::new();
                for assignment in &edge.assignments {
                    if !successor(assignment.destination) {
                        continue;
                    }
                    if let Source::Location(source) = assignment.source {
                        require(active.contains(&source), "edge forwards a taken successor")?;
                        require(consumed.insert(source), "edge duplicates successor permission")?;
                    }
                    destinations.push(assignment.destination);
                }
                for source in consumed {
                    next.remove(&source);
                }
                for destination in destinations {
                    require(next.insert(destination), "edge overwrites live successor permission")?;
                }
                let target = pcs[&edge.target_pc];
                if let Some(expected) = &incoming[target] {
                    require(*expected == next, "inconsistent successor permissions at CFG join")?;
                } else {
                    stored = stored.saturating_add(next.len());
                    enforce(stored <= 8 * 1024 * 1024, "LOAD_WORK_BUDGET",
                        "successor permission storage exceeds verification work budget")?;
                    incoming[target] = Some(next);
                    work.push(target);
                }
            }
        }
    }
    Ok(())
}
