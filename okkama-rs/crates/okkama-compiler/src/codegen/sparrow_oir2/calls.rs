//! S2 transport. Machine payloads and outcome ordinals come from the shared SPB registry.
use super::*;
#[path = "cleanup.rs"]
mod cleanup;
use s::{Argument as A, Location as L, Opcode as O, ProducedType as P, ServiceKind as K};

fn error(message: impl Into<String>) -> GenerateError {
    GenerateError(message.into())
}

fn later(name: &str) -> GenerateError {
    error(format!("{name}: not yet in S2"))
}

fn kind(module: &o::Module, ty: o::TypeId) -> Result<P, GenerateError> {
    Ok(match module.types.get(ty)? {
        o::Type::Service(service) => match service {
            o::ServiceType::Own => P::Service(K::Packet),
            o::ServiceType::Deps => P::Service(K::Deps),
            o::ServiceType::Scope(_) => P::Service(K::ScopeState),
            o::ServiceType::Failure => P::Value(s::Type::Failure),
            o::ServiceType::Pending => P::Service(K::Pending),
            o::ServiceType::CleanupState => P::Service(K::Cleanup),
            o::ServiceType::FailureState => P::Service(K::FailureState),
            o::ServiceType::ExitState => P::Service(K::ExitState),
            o::ServiceType::StableKey => P::Service(K::StableKey),
            o::ServiceType::Closed => P::Service(K::Closed),
            _ => return Err(later("async service type")),
        },
        _ => P::Value(translate_type(module, ty)?),
    })
}

pub(super) fn needs_s2(module: &o::Module, function: &o::Function) -> Result<bool, GenerateError> {
    if function.summary.is_some() || function.signature.ownership_abi != o::OwnershipAbi::Observation {
        return Ok(true);
    }
    let Some(body) = &function.body else { return Ok(false) };
    if !body.deps.is_empty() || !body.keys.is_empty() || !body.selections.is_empty()
        || function.attrs.iter().any(|attr| matches!(attr,
            o::FunctionAttr::Cleanup | o::FunctionAttr::DeferEntry { .. } | o::FunctionAttr::Ctor(_)
            | o::FunctionAttr::Destroy(_)))
    {
        return Ok(true);
    }
    for (_, value) in body.values.iter() {
        if matches!(module.types.get(value.ty)?, o::Type::Service(_)) {
            return Ok(true);
        }
    }
    Ok(body.blocks.iter().any(|(_, block)| matches!(block.term.kind,
        o::TermKind::InvokeOwn { .. } | o::TermKind::InvokeDefer { .. } | o::TermKind::RetDefer { .. })))
}

fn labels(module: &o::Module, function: &o::Function) -> Result<Vec<s::DeferLabel>, GenerateError> {
    let Some(labels) = function.attrs.iter().find_map(|attr| match attr {
        o::FunctionAttr::DeferEntry { replacements } => Some(replacements),
        _ => None,
    }) else { return Ok(vec![]) };
    let mut seen = BTreeSet::new();
    labels.iter().enumerate().map(|(index, label)| {
        if !seen.insert(&label.label) {
            return Err(error("duplicate defer label"));
        }
        Ok(s::DeferLabel {
            label: u32::try_from(index).map_err(|_| error("too many defer labels"))?,
            result: label.result.map(|ty| translate_type(module, ty)).transpose()?,
        })
    }).collect()
}

struct Emitter<'a> {
    module: &'a o::Module,
    function: &'a o::Function,
    body: &'a o::Body,
    objects: &'a [s::Object],
    locations: Vec<L>,
    pcs: BTreeMap<o::BlockId, u32>,
    summaries: &'a mut Vec<s::Summary>,
    scope: o::ScopeId,
    result: s::Function,
}

impl Emitter<'_> {
    fn loc(&self, value: o::ValueId) -> Result<L, GenerateError> {
        self.locations.get(value.as_u32() as usize).copied().ok_or_else(|| error("invalid SSA operand"))
    }

    fn reg(&self, value: o::ValueId) -> Result<u32, GenerateError> {
        let L::Register(reg) = self.loc(value)? else { return Err(error("expected language value")) };
        Ok(reg)
    }

    fn edge(&self, block: o::BlockId, args: &[o::EdgeArg], outcome: o::Outcome,
        schema: &[P]) -> Result<s::Edge, GenerateError>
    {
        let params = &self.body.blocks.get(block)?.params;
        if params.len() != args.len() {
            return Err(error("bad input edge arity"));
        }
        let assignments = params.iter().zip(args).map(|(&dst, arg)| {
            let destination = self.loc(dst)?;
            let source = match *arg {
                o::EdgeArg::Value(src) => {
                    if self.body.values.get(src)?.ty != self.body.values.get(dst)?.ty {
                        return Err(error("edge value type mismatch"));
                    }
                    s::Source::Location(self.loc(src)?)
                }
                o::EdgeArg::Produced(slot) => {
                    if slot.outcome != outcome {
                        return Err(error("produced result belongs to another outcome"));
                    }
                    if schema.get(usize::from(slot.slot)) != Some(&kind(self.module, self.body.values.get(dst)?.ty)?) {
                        return Err(error("produced result slot/type mismatch"));
                    }
                    s::Source::ProducedSlot(u32::from(slot.slot))
                }
            };
            Ok(s::Assignment { destination, source })
        }).collect::<Result<_, GenerateError>>()?;
        Ok(s::Edge { target_pc: self.pcs[&block], assignments })
    }

    fn branch(&self, target: &o::Target) -> Result<s::Edge, GenerateError> {
        let args = target.args.iter().copied().map(o::EdgeArg::Value).collect::<Vec<_>>();
        self.edge(target.block, &args, o::Outcome::Normal, &[])
    }

    fn call(&mut self, entry: o::FuncId, call: &o::CallSite, defer: bool)
        -> Result<(Vec<A>, s::ResultContext), GenerateError>
    {
        let declaration = self.module.functions.get(entry)?;
        let summary_id = call.summary.ok_or_else(|| error("owning call requires summary"))?;
        if declaration.summary != Some(summary_id) {
            return Err(error("entry/summary mismatch"));
        }
        let summary = self.module.summaries.get(summary_id)?;
        if summary.entry_kind == o::EntryKind::ObservationAdmissionStaysCaller {
            return Err(error("owning call cannot use an observation summary"));
        }
        if !call.type_args.is_empty() || !call.descriptors.is_empty() || summary.contract.suspends
            || summary.contract.async_destroy || summary.contract.publication != o::Publication::Atomic
            || summary.contract.protocol.is_some()
        {
            return Err(later("async/generic/protocol call"));
        }
        let shielded = defer || call.cleanup == o::CallCleanupMode::Shielded;
        if !defer && shielded && !self.function.attrs.contains(&o::FunctionAttr::Cleanup) {
            return Err(error("cleanup shielded outside cleanup function"));
        }
        if !shielded && summary.contract.failures.contains(&o::FailureKind::Cancelled) {
            return Err(later("delivered cancellation"));
        }
        let params = declaration.signature.params.iter().map(|&ty| kind(self.module, ty))
            .collect::<Result<Vec<_>, _>>()?;
        let language = params.iter().filter_map(|ty| match ty { P::Value(ty) => Some(*ty), _ => None })
            .collect::<Vec<_>>();
        if call.args.len() != language.len() || call.args.len() != summary.parameters.len() {
            return Err(error("owning call language argument arity mismatch"));
        }
        for ((arg, expected), effect) in call.args.iter().zip(language).zip(&summary.parameters) {
            if kind(self.module, self.body.values.get(arg.value)?.ty)? != P::Value(expected) {
                return Err(error("owning call argument type mismatch"));
            }
            if !matches!((arg.convention, effect.convention),
                (o::OperandConvention::Borrow, o::FreshConvention::Borrow)
                | (o::OperandConvention::Consume, o::FreshConvention::Take))
            {
                return Err(error("owning call convention mismatch"));
            }
        }
        let packet = call.packet.ok_or_else(|| error("owning call requires explicit input packet"))?;
        if kind(self.module, self.body.values.get(packet)?.ty)? != P::Service(K::Packet) {
            return Err(error("owning call input is not a packet"));
        }
        for &dep in &call.deps {
            if kind(self.module, self.body.values.get(dep)?.ty)? != P::Service(K::Deps) {
                return Err(error("call deps operand is not deps"));
            }
        }
        let results = summary.contract.results.iter().map(|&ty| translate_type(self.module, ty))
            .collect::<Result<Vec<_>, _>>()?;
        let declared = declaration.signature.results.iter().map(|&ty| kind(self.module, ty))
            .collect::<Result<Vec<_>, _>>()?;
        let declared_values = declared.iter().filter_map(|ty| match ty { P::Value(ty) => Some(*ty), _ => None })
            .collect::<Vec<_>>();
        if results.len() > 1 || (!defer && results != declared_values) {
            return Err(error("call summary/result signature mismatch"));
        }
        let may_fail = summary.contract.failures.iter().any(|&kind| kind != o::FailureKind::Cancelled);
        // SPB keeps a conservative parameter inventory. Typed path precision stays in the
        // verified Low facts; dropping a path must never drop its owning parameter's roots.
        let mut selections = summary.parameters.iter().filter_map(|parameter| parameter.retention.as_ref())
            .chain(summary.results.iter().flat_map(|result| result.aliases.iter())).collect::<Vec<_>>();
        let mut retained = summary.results.iter().flat_map(|result| result.external_deps.iter().copied())
            .collect::<BTreeSet<_>>();
        while let Some(selection) = selections.pop() {
            match selection {
                o::SummarySelection::Parameter(parameter) | o::SummarySelection::OwnerGraph(parameter)
                | o::SummarySelection::Field { parameter, .. } | o::SummarySelection::Path { parameter, .. } => {
                    retained.insert(*parameter);
                }
                o::SummarySelection::Union(parts) => selections.extend(parts),
                o::SummarySelection::Descriptor(_) => return Err(later("descriptor summary retention")),
            }
        }
        let parameter_indices = params.iter().enumerate().filter_map(|(index, ty)| {
            matches!(ty, P::Value(_)).then_some(index as u32)
        }).collect::<Vec<_>>();
        let retained_params = retained.into_iter().map(|parameter| {
            parameter_indices.get(usize::from(parameter)).copied()
                .ok_or_else(|| error("summary retained parameter outside signature"))
        }).collect::<Result<_, _>>()?;
        let summary_index = self.summaries.len() as u32;
        self.summaries.push(s::Summary { entry: entry.as_u32(), may_fail, retained_params });
        let mut arguments = vec![A::Entry(entry.as_u32()), A::Summary(summary_index)];
        if defer {
            let declared_labels = labels(self.module, declaration)?;
            let mut scope = Some(self.scope);
            let mut seen_scopes = BTreeSet::new();
            let registrations = loop {
                let Some(current) = scope else { break &[][..] };
                if !seen_scopes.insert(current) {
                    return Err(error("cyclic lexical scopes"));
                }
                let candidates = self.body.insts.iter().filter_map(|(_, inst)| match &inst.op {
                    o::Operation::Lexical(o::LexicalOp::DeferRegister { scope, action })
                        if *scope == current && action.function == entry => Some(action.replacements.as_slice()),
                    _ => None,
                }).collect::<Vec<_>>();
                if let Some(&first) = candidates.first() {
                    if candidates.iter().any(|&candidate| candidate != first) {
                        return Err(error("ambiguous defer registration map at invocation"));
                    }
                    break first;
                }
                scope = self.module.scopes.get(current)?.parent;
            };
            let mut registered = Vec::new();
            let original = declaration.attrs.iter().find_map(|attr| match attr {
                o::FunctionAttr::DeferEntry { replacements } => Some(replacements),
                _ => None,
            }).ok_or_else(|| error("invoke.defer requires defer entry"))?;
            for (index, label) in original.iter().enumerate() {
                let registration = registrations.iter().find(|registration| registration.label == label.label)
                    .ok_or_else(|| error("missing defer label registration"))?;
                let boundary = self.body.boundaries.get(registration.boundary)?;
                registered.push(s::RegistrationLabel {
                    label: index as u32,
                    kind: match registration.kind {
                        o::ExitKind::Return => s::ExitKind::Return,
                        o::ExitKind::Last => s::ExitKind::Last,
                        o::ExitKind::Next => s::ExitKind::Next,
                        o::ExitKind::Fallthrough => s::ExitKind::Fallthrough,
                    },
                    boundary: boundary.scope.as_u32(),
                });
            }
            let map = self.result.proofs.registrations.len() as u32;
            self.result.proofs.registrations.push(s::RegistrationMap {
                entry: entry.as_u32(), labels: registered,
            });
            arguments.push(A::Registration(map));
            if declared_labels.len() != registrations.len() {
                return Err(error("invalid defer registration map"));
            }
        } else {
            arguments.push(A::Flag(shielded));
        }
        arguments.extend(call.args.iter().map(|arg| self.loc(arg.value).map(A::Location))
            .collect::<Result<Vec<_>, _>>()?);
        arguments.push(A::Location(self.loc(packet)?));
        arguments.extend(call.deps.iter().map(|&dep| self.loc(dep).map(A::Location))
            .collect::<Result<Vec<_>, _>>()?);
        Ok((arguments, s::ResultContext {
            value: results.first().copied(), may_fail, labels: labels(self.module, declaration)?,
            ..s::ResultContext::default()
        }))
    }

    fn observation(&mut self, function: o::FuncId, call: &o::CallSite)
        -> Result<(Vec<A>, s::ResultContext), GenerateError>
    {
        let declaration = self.module.functions.get(function)?;
        let summary = self.module.summaries.get(call.summary.ok_or_else(|| error("missing observation summary"))?)?;
        if summary.entry_kind != o::EntryKind::ObservationAdmissionStaysCaller || call.packet.is_some()
            || !call.type_args.is_empty() || !call.descriptors.is_empty() || summary.contract.suspends
        {
            return Err(error("invalid observation call"));
        }
        let may_fail = !summary.contract.failures.is_empty();
        let id = self.summaries.len() as u32;
        self.summaries.push(s::Summary { entry: function.as_u32(), may_fail, retained_params: vec![] });
        let mut arguments = vec![A::Entry(function.as_u32()), A::Summary(id)];
        arguments.extend(call.args.iter().map(|arg| self.loc(arg.value).map(A::Location))
            .collect::<Result<Vec<_>, _>>()?);
        arguments.extend(call.deps.iter().map(|&value| self.loc(value).map(A::Location))
            .collect::<Result<Vec<_>, _>>()?);
        Ok((arguments, s::ResultContext {
            value: declaration.signature.results.first().map(|&ty| translate_type(self.module, ty)).transpose()?,
            may_fail, ..s::ResultContext::default()
        }))
    }

    fn invocation(&mut self, term: &o::TermKind) -> Result<s::Instruction, GenerateError> {
        let (opcode, arguments, context, edges) = match term {
            o::TermKind::InvokeOwn { op, normal, unwind } => {
                if let o::Operation::Core(core) = op.as_ref() {
                    let scalar = match core {
                        o::CoreOp::RangeNew { start, end, inclusive } => Some((s::DataOp::RangeNew,
                            s::Type::Range, s::Immediate::Inclusive(*inclusive), vec![*start, *end])),
                        o::CoreOp::RangeEnd { value } => Some((s::DataOp::RangeEnd,
                            s::Type::Int, s::Immediate::None, vec![*value])),
                        o::CoreOp::RangeLen { value } => Some((s::DataOp::RangeLen,
                            s::Type::Int, s::Immediate::None, vec![*value])),
                        o::CoreOp::IntMod { left, right } => Some((s::DataOp::IntMod,
                            s::Type::Int, s::Immediate::None, vec![*left, *right])),
                        _ => None,
                    };
                    if let Some((op, result, immediate, inputs)) = scalar {
                        let context = s::ResultContext {
                            value: Some(result), may_fail: true, ..s::ResultContext::default()
                        };
                        let mut arguments = vec![A::Evaluation(s::Evaluation { op, result, immediate })];
                        arguments.extend(inputs.iter().map(|&value| self.loc(value).map(A::Location))
                            .collect::<Result<Vec<_>, _>>()?);
                        let outcomes = [(s::Tag::Normal, o::Outcome::Normal, normal),
                            (s::Tag::Unwind, o::Outcome::Unwind, unwind)].into_iter().map(|(tag, outcome, edge)| {
                            let schema = O::Evaluate.results(tag, &context)?;
                            Ok(s::Outcome { tag, edge: self.edge(edge.block, &edge.args, outcome, &schema)? })
                        }).collect::<Result<_, GenerateError>>()?;
                        return Ok(s::Instruction::Champ { op: O::Evaluate, arguments,
                            flow: s::Flow::Outcomes(outcomes) });
                    }
                }
                if let o::Operation::Collection(o::CollectionOp::ArrayGet { array, index }) = op.as_ref() {
                    let s::Type::Array(shape) = translate_type(self.module, self.body.values.get(*array)?.ty)? else {
                        return Err(error("array.get requires a certified plain array"));
                    };
                    let context = s::ResultContext {
                        value: shape.element(), may_fail: true, ..s::ResultContext::default()
                    };
                    let outcomes = [(s::Tag::Normal, o::Outcome::Normal, normal),
                        (s::Tag::Unwind, o::Outcome::Unwind, unwind)].into_iter().map(|(tag, outcome, edge)| {
                        let schema = O::ArrayGet.results(tag, &context)?;
                        Ok(s::Outcome { tag, edge: self.edge(edge.block, &edge.args, outcome, &schema)? })
                    }).collect::<Result<_, GenerateError>>()?;
                    return Ok(s::Instruction::Champ {
                        op: O::ArrayGet,
                        arguments: vec![A::Location(self.loc(*array)?), A::Location(self.loc(*index)?)],
                        flow: s::Flow::Outcomes(outcomes),
                    });
                }
                let o::Operation::Call(o::CallOp::Direct { function, call } | o::CallOp::Native { function, call }
                    | o::CallOp::Intrinsic { function, call }) = op.as_ref() else {
                    return Err(later(op.opcode()));
                };
                let summary = self.module.summaries.get(call.summary.ok_or_else(|| error("missing summary"))?)?;
                if summary.entry_kind == o::EntryKind::ObservationAdmissionStaysCaller {
                    let (arguments, context) = self.observation(*function, call)?;
                    (O::InvokeObserve, arguments, context, vec![normal, unwind])
                } else {
                    let (arguments, context) = self.call(*function, call, false)?;
                    (O::InvokeCallOwn, arguments, context, vec![normal, unwind])
                }
            }
            o::TermKind::InvokeDefer { function, call, continue_edge, replacements, unwind } => {
                let (arguments, context) = self.call(*function, call, true)?;
                let declaration = self.module.functions.get(*function)?;
                let original = declaration.attrs.iter().find_map(|attr| match attr {
                    o::FunctionAttr::DeferEntry { replacements } => Some(replacements),
                    _ => None,
                }).ok_or_else(|| error("missing defer declaration"))?;
                if replacements.len() != original.len() {
                    return Err(error("incomplete defer replacement edges"));
                }
                let mut edges = vec![continue_edge];
                for label in original {
                    let matches = replacements.iter().filter(|edge| edge.label == label.label).collect::<Vec<_>>();
                    if matches.len() != 1 {
                        return Err(error("unknown or duplicate defer replacement edge"));
                    }
                    edges.push(&matches[0].edge);
                }
                edges.extend(unwind.iter());
                (O::InvokeDefer, arguments, context, edges)
            }
            _ => return Err(error("expected invocation")),
        };
        if opcode != O::InvokeObserve {
            opcode.call_abi().ok_or_else(|| error("missing shared helper ABI"))?;
        }
        let tags = opcode.tags(&context);
        if edges.len() != tags.len() {
            return Err(error("invocation outcome inventory mismatch"));
        }
        let outcomes = tags.into_iter().zip(edges).map(|(tag, edge)| {
            let outcome = match tag {
                s::Tag::Normal => o::Outcome::Normal,
                s::Tag::Unwind => o::Outcome::Unwind,
                s::Tag::Continue => o::Outcome::DeferContinue,
                s::Tag::Replace(label) => o::Outcome::DeferReplace(label as u16),
                _ => return Err(error("unexpected call outcome")),
            };
            let schema = opcode.results(tag, &context)?;
            if opcode != O::InvokeObserve {
                opcode.call_post_offsets(tag, &context)?;
            }
            Ok(s::Outcome { tag, edge: self.edge(edge.block, &edge.args, outcome, &schema)? })
        }).collect::<Result<_, GenerateError>>()?;
        Ok(s::Instruction::Champ { op: opcode, arguments, flow: s::Flow::Outcomes(outcomes) })
    }
}

pub(super) fn translate(module: &o::Module, id: o::FuncId, function: &o::Function, objects: &[s::Object],
    summaries: &mut Vec<s::Summary>) -> Result<s::Function, GenerateError>
{
    if function.body.is_none() {
        return cleanup::external(module, function);
    }
    if !function.type_params.is_empty() || function.async_creation_summary.is_some()
        || function.async_body_summary.is_some()
    {
        return Err(later(&format!("{}: generic/async entry", function.symbol)));
    }
    if let Some(attr) = function.attrs.iter().find(|attr| !declaration_attr(attr) && !matches!(attr,
        o::FunctionAttr::Entry | o::FunctionAttr::Hidden | o::FunctionAttr::Cleanup
        | o::FunctionAttr::DeferEntry { .. } | o::FunctionAttr::Ctor(_) | o::FunctionAttr::Destroy(_)))
    {
        return Err(later(&format!("{}: entry attribute {attr:?}", function.symbol)));
    }
    let body = function.body.as_ref().ok_or_else(|| later("entry without body"))?;
    if !body.high_results.is_empty() || !body.high_effects.is_empty() || !body.descriptor_bindings.is_empty()
        || !body.service_schedules.is_empty()
    {
        return Err(later("High/descriptor/service schedule metadata"));
    }
    let entry = body.blocks.get(body.entry)?;
    let params = entry.params.iter().map(|&value| Ok(body.values.get(value)?.ty))
        .collect::<Result<Vec<_>, GenerateError>>()?;
    if params != function.signature.params {
        return Err(error("entry parameter signature mismatch"));
    }
    let defer = function.attrs.iter().any(|attr| matches!(attr, o::FunctionAttr::DeferEntry { .. }));
    let entry_abi = if defer {
        s::EntryAbi::Defer { labels: labels(module, function)? }
    } else {
        match function.signature.ownership_abi {
            o::OwnershipAbi::Observation => s::EntryAbi::Observation,
            o::OwnershipAbi::Explicit { input_packet, output_packet } => {
                let input = params.get(usize::from(input_packet)).copied()
                    .ok_or_else(|| error("invalid input packet position"))?;
                let output = function.signature.results.get(usize::from(output_packet)).copied()
                    .ok_or_else(|| error("invalid output packet position"))?;
                if kind(module, input)? != P::Service(K::Packet) || kind(module, output)? != P::Service(K::Packet) {
                    return Err(error("entry packet position/type mismatch"));
                }
                s::EntryAbi::Owned
            }
            o::OwnershipAbi::Unelaborated if function.summary.is_some() => {
                if function.signature.results.iter().any(|&ty| kind(module, ty) == Ok(P::Service(K::Packet))) {
                    s::EntryAbi::Owned
                } else {
                    s::EntryAbi::Observation
                }
            }
            _ => return Err(error("unelaborated entry ABI")),
        }
    };
    let mut result = s::Function {
        symbol: function.symbol.clone(), entry_abi, registers: vec![], services: vec![], params: vec![],
        results: vec![], blocks: vec![], proofs: s::FunctionProofs::default(), code: vec![],
    };
    let mut locations = vec![];
    for (_, value) in body.values.iter() {
        locations.push(match kind(module, value.ty)? {
            P::Value(ty) => {
                let location = L::Register(result.registers.len() as u32);
                result.registers.push(ty);
                location
            }
            P::Service(kind) => {
                let location = L::Service(result.services.len() as u32);
                result.services.push(kind);
                location
            }
        });
    }
    for &ty in &function.signature.results {
        if let P::Value(ty) = kind(module, ty)? {
            result.results.push(ty);
        }
    }
    let mut order = vec![body.entry];
    order.extend(body.blocks.iter().map(|(id, _)| id).filter(|&id| id != body.entry));
    let mut pcs = BTreeMap::new();
    let mut inst_pcs = BTreeMap::new();
    let mut term_pcs = BTreeMap::new();
    let mut pc = 0u32;
    let mut definitions = BTreeSet::new();
    let mut instructions = BTreeSet::new();
    for &block_id in &order {
        let block = body.blocks.get(block_id)?;
        if module.scopes.get(block.lexical_scope)?.function != id {
            return Err(error("block scope belongs to another function"));
        }
        pcs.insert(block_id, pc);
        for (index, &value) in block.params.iter().enumerate() {
            let expected = o::Definition::BlockParam { block: block_id, index: index as u32 };
            if body.values.get(value)?.def != expected || !definitions.insert(value) {
                return Err(error("inconsistent or duplicate SSA definition"));
            }
        }
        for &inst_id in &block.insts {
            if !instructions.insert(inst_id) {
                return Err(error("duplicate instruction"));
            }
            let inst = body.insts.get(inst_id)?;
            if block.unwind.is_some() && o::ContractRegistry::new(module).lookup(&inst.op)
                .map_err(|failure| error(failure.to_string()))?.unwind.is_some()
            {
                return Err(later(&format!("{}: implicit instruction unwind {}", function.symbol, inst.op.opcode())));
            }
            inst_pcs.insert(inst_id, pc);
            let size = match &inst.op {
                o::Operation::Core(o::CoreOp::RangeLen { .. }) => 3,
                // D8 registration is static CFG metadata, not a second runtime defer runner.
                o::Operation::Lexical(o::LexicalOp::DeferRegister { .. }) => 0,
                _ => 1,
            };
            pc = pc.checked_add(size).ok_or_else(|| error("too much code"))?;
            for (index, &value) in inst.results.iter().enumerate() {
                let expected = o::Definition::InstResult { inst: inst_id, index: index as u16 };
                if body.values.get(value)?.def != expected || !definitions.insert(value) {
                    return Err(error("inconsistent or duplicate SSA definition"));
                }
            }
        }
        term_pcs.insert(block_id, pc);
        let size = match &block.term.kind {
            o::TermKind::InvokeOwn { op, .. } if matches!(op.as_ref(),
                o::Operation::Core(o::CoreOp::ObjNew { packet: None, .. }
                    | o::CoreOp::ObjNext { packet: None, .. } | o::CoreOp::VariantNew { packet: None, .. })) => 2,
            o::TermKind::InvokeOwn { op, .. } if matches!(op.as_ref(),
                o::Operation::Core(o::CoreOp::ObjNew { .. } | o::CoreOp::ObjNext { .. }
                    | o::CoreOp::VariantNew { .. })) => 4,
            o::TermKind::Trap { .. } => 2,
            o::TermKind::Switch { cases, .. } => cases.len() * 3 + 1,
            _ => 1,
        };
        pc = pc.checked_add(u32::try_from(size).map_err(|_| error("too much code"))?)
            .ok_or_else(|| error("too much code"))?;
    }
    if definitions.len() != body.values.len() || instructions.len() != body.insts.len() {
        return Err(error("orphan SSA value or instruction"));
    }
    let mut emitter = Emitter {
        module, function, body, objects, locations, pcs, summaries, scope: entry.lexical_scope, result,
    };
    emitter.result.proofs.entry = Some(cleanup::entry_contract(module, function)?);
    emitter.result.params = entry.params.iter().map(|&value| emitter.loc(value)).collect::<Result<_, _>>()?;
    // Expand proof DAG leaves through the OIR visitor. Value dependencies remain conditional on the
    // value's liveness; site metadata is attached exactly where it is used, including erased deps.
    let mut metadata = Vec::new();
    o::cfg::walk_body(body, o::ContractRegistry::new(module), |event| {
        if let o::cfg::CfgEvent::Use { site, operand } = event {
            metadata.push((site, operand.value));
        }
    }).map_err(|failure| error(failure.to_string()))?;
    for (site, value) in metadata {
        let location = emitter.loc(value)?;
        match site {
            o::cfg::UseSite::ValueFacts(owner) => {
                emitter.result.proofs.dependencies.push(s::Dependency {
                    value: emitter.loc(owner)?, obligations: vec![location],
                });
            }
            o::cfg::UseSite::Instruction(inst) | o::cfg::UseSite::InstructionFacts(inst) => {
                emitter.result.proofs.roots.push(s::RootRequirement {
                    pc: inst_pcs[&inst], locations: vec![location],
                });
            }
            o::cfg::UseSite::Terminator(block) => {
                emitter.result.proofs.roots.push(s::RootRequirement {
                    pc: term_pcs[&block], locations: vec![location],
                });
            }
            o::cfg::UseSite::Declaration(_) => {}
            o::cfg::UseSite::BlockUnwind(block) => {
                emitter.result.proofs.roots.push(s::RootRequirement {
                    pc: term_pcs[&block], locations: vec![location],
                });
            }
        }
    }
    emitter.cleanup_metadata(&inst_pcs, &term_pcs)?;
    for &block_id in &order {
        let block = body.blocks.get(block_id)?;
        emitter.scope = block.lexical_scope;
        emitter.result.blocks.push(s::Block {
            pc: emitter.pcs[&block_id],
            params: block.params.iter().map(|&value| emitter.loc(value)).collect::<Result<_, _>>()?,
        });
        for &inst_id in &block.insts {
            let inst = body.insts.get(inst_id)?;
            let translated = match &inst.op {
                o::Operation::Ownership(op) => {
                    let (op, arguments) = match op {
                        o::OwnershipOp::Dependencies { value } => {
                            let unbound = matches!(body.values.get(*value)?.def,
                                o::Definition::InstResult { inst, .. }
                                    if matches!(body.insts.get(inst)?.op, o::Operation::Core(o::CoreOp::Undef { .. })));
                            if unbound {
                                let flow = s::Flow::Next(inst.results.iter().map(|&value| emitter.loc(value))
                                    .collect::<Result<_, _>>()?);
                                emitter.result.code.push(s::Instruction::Champ {
                                    op: O::DepsEmpty, arguments: vec![], flow,
                                });
                                continue;
                            }
                            let anchor = emitter.loc(*value)?;
                            let [result] = inst.results.as_slice() else {
                                return Err(error("deps.of requires one result"));
                            };
                            emitter.result.proofs.dependencies.push(s::Dependency {
                                value: emitter.loc(*result)?, obligations: vec![anchor],
                            });
                            (O::DepsOf, vec![A::Location(anchor)])
                        }
                        o::OwnershipOp::Empty {} => (O::OwnEmpty, vec![]),
                        o::OwnershipOp::Move { packet } => (O::OwnMove, vec![A::Location(emitter.loc(*packet)?)]),
                        o::OwnershipOp::Join { packets } => (O::OwnJoin, packets.iter()
                            .map(|&value| emitter.loc(value).map(A::Location)).collect::<Result<_, _>>()?),
                        o::OwnershipOp::ScopeBegin { scope } => {
                            (O::ScopeBegin, vec![A::Scope(scope.as_u32()), A::Boundary(scope.as_u32())])
                        }
                        o::OwnershipOp::Accept { state, packet } | o::OwnershipOp::Admit { state, packet } => {
                            let opcode = if matches!(op, o::OwnershipOp::Accept { .. }) {
                                O::OwnAccept
                            } else {
                                O::OwnAdmit
                            };
                            (opcode, vec![A::Location(emitter.loc(*state)?), A::Location(emitter.loc(*packet)?)])
                        }
                        _ => emitter.ownership(op)?,
                    };
                    s::Instruction::Champ { op, arguments, flow: s::Flow::Next(inst.results.iter()
                        .map(|&value| emitter.loc(value)).collect::<Result<_, _>>()?) }
                }
                o::Operation::Cleanup(op) => emitter.cleanup_instruction(op, &inst.results)?,
                o::Operation::Exit(op) => emitter.exit_instruction(op, &inst.results)?,
                o::Operation::Core(o::CoreOp::TypeOf { ty, .. }) => {
                    let o::Type::Object { decl, .. } = module.types.get(*ty)? else {
                        return Err(error("descriptor is not an object"));
                    };
                    data(s::DataOp::Const, vec![], emitter.reg(inst.results[0])?,
                        s::Immediate::Constant(s::Constant::Descriptor(decl.as_u32())))
                }
                o::Operation::Call(o::CallOp::Direct { function, call } | o::CallOp::Native { function, call }
                    | o::CallOp::Intrinsic { function, call }) => {
                    let summary = module.summaries.get(call.summary.ok_or_else(|| error("missing summary"))?)?;
                    let opcode = if summary.entry_kind == o::EntryKind::ObservationAdmissionStaysCaller {
                        O::InvokeObserve
                    } else {
                        O::InvokeCallOwn
                    };
                    let (arguments, context) = if opcode == O::InvokeObserve {
                        emitter.observation(*function, call)?
                    } else {
                        emitter.call(*function, call, false)?
                    };
                    if context.may_fail {
                        return Err(error("fallible call requires explicit outcomes"));
                    }
                    s::Instruction::Champ { op: opcode, arguments,
                        flow: s::Flow::Next(inst.results.iter().map(|&v| emitter.loc(v))
                            .collect::<Result<_, _>>()?) }
                }
                o::Operation::Lexical(o::LexicalOp::DeferRegister { .. }) => {
                    if !inst.results.is_empty() {
                        return Err(error("defer registration cannot produce execution results"));
                    }
                    continue;
                }
                o::Operation::Core(o::CoreOp::RangeLen { value }) => {
                    if inst.results.len() != 1 {
                        return Err(error("range.len requires one result"));
                    }
                    let value = emitter.reg(*value)?;
                    let dst = emitter.reg(inst.results[0])?;
                    let start = fresh(&mut emitter.result, s::Type::Int)?;
                    let end = fresh(&mut emitter.result, s::Type::Int)?;
                    let pc = emitter.result.code.len() as u32;
                    let mut roots = emitter.result.proofs.roots.iter().filter(|root| root.pc == pc)
                        .flat_map(|root| root.locations.iter().copied()).collect::<BTreeSet<_>>();
                    roots.insert(L::Register(start));
                    emitter.result.proofs.roots.push(s::RootRequirement {
                        pc: pc + 1, locations: roots.iter().copied().collect(),
                    });
                    roots.insert(L::Register(end));
                    emitter.result.proofs.roots.push(s::RootRequirement {
                        pc: pc + 2, locations: roots.into_iter().collect(),
                    });
                    emitter.result.code.extend([
                        data(s::DataOp::RangeStart, vec![value], start, s::Immediate::None),
                        data(s::DataOp::RangeEnd, vec![value], end, s::Immediate::None),
                        data(s::DataOp::IntSub, vec![end, start], dst, s::Immediate::None),
                    ]);
                    continue;
                }
                _ => {
                    let mut translated = translate_instruction(module, body, inst, objects)?;
                    if let s::Instruction::Data { inputs, output, .. } = &mut translated {
                        for value in inputs {
                            *value = emitter.reg(o::ValueId::from_u32(*value))?;
                        }
                        if let Some(value) = output {
                            *value = emitter.reg(o::ValueId::from_u32(*value))?;
                        }
                    }
                    translated
                }
            };
            emitter.result.code.push(translated);
        }
        let term = match &block.term.kind {
            o::TermKind::Catch { failure, ty, handler, next } => {
                let ty = translate_type(module, *ty)?;
                let target = match ty {
                    s::Type::Object(id) => A::Object(id),
                    s::Type::Error => A::Flag(true),
                    _ => return Err(error("catch requires a plain Error")),
                };
                s::Instruction::Champ { op: O::Catch,
                    arguments: vec![A::Location(emitter.loc(*failure)?), target],
                    flow: s::Flow::Outcomes(vec![
                        s::Outcome { tag: s::Tag::Some,
                            edge: emitter.edge(handler.block, &handler.args, o::Outcome::Normal, &[P::Value(ty)])? },
                        s::Outcome { tag: s::Tag::None, edge: emitter.branch(next)? },
                    ]) }
            }
            o::TermKind::InvokeOwn { op, normal, .. }
                if matches!(op.as_ref(), o::Operation::Core(o::CoreOp::ObjNew { .. } | o::CoreOp::ObjNext { .. }
                    | o::CoreOp::VariantNew { .. })) => {
                emitter.construction(op, normal)?
            }
            o::TermKind::InvokeOwn { op, normal, unwind }
                if matches!(op.as_ref(), o::Operation::Ownership(_)) => {
                emitter.ownership_invoke(op, normal, unwind)?
            }
            o::TermKind::CleanupNext { .. } | o::TermKind::ExitDispatch { .. }
                | o::TermKind::FailureDispatch { .. } => emitter.cleanup_term(&block.term.kind)?,
            o::TermKind::Throw(_) | o::TermKind::Panic(_) | o::TermKind::Trap { .. } => {
                let (kind, payload) = match &block.term.kind {
                    o::TermKind::Throw(value) => (0, emitter.reg(*value)?),
                    o::TermKind::Panic(value) => (2, emitter.reg(*value)?),
                    o::TermKind::Trap { message, .. } => {
                        let payload = fresh(&mut emitter.result, s::Type::Str)?;
                        emitter.result.code.push(data(s::DataOp::Const, vec![], payload,
                            s::Immediate::Constant(s::Constant::Str(message.clone()))));
                        (1, payload)
                    }
                    _ => unreachable!(),
                };
                let target = &block.unwind.as_ref().ok_or_else(|| error("failure requires unwind continuation"))?
                    .target;
                let mut args = target.args.iter().copied().map(o::EdgeArg::Value).collect::<Vec<_>>();
                args.push(o::EdgeArg::Produced(o::EdgeResult { outcome: o::Outcome::Unwind, slot: 0 }));
                let edge = emitter.edge(target.block, &args, o::Outcome::Unwind, &[P::Value(s::Type::Failure)])?;
                s::Instruction::Champ { op: O::Fail,
                    arguments: vec![A::Label(kind), A::Location(L::Register(payload))],
                    flow: s::Flow::Outcomes(vec![s::Outcome { tag: s::Tag::Unwind, edge }]) }
            }
            o::TermKind::Raise(failure) => s::Instruction::Raise(emitter.reg(*failure)?),
            o::TermKind::InvokeOwn { .. } | o::TermKind::InvokeDefer { .. } => emitter.invocation(&block.term.kind)?,
            o::TermKind::Br(target) => s::Instruction::Br(emitter.branch(target)?),
            o::TermKind::Cond { condition, yes, no } => s::Instruction::Cond {
                condition: emitter.reg(*condition)?, yes: emitter.branch(yes)?, no: emitter.branch(no)?,
            },
            o::TermKind::Switch { value, cases, default } => {
                let ty = translate_type(module, body.values.get(*value)?.ty)?;
                let op = match ty {
                    s::Type::Int => s::DataOp::IntEq,
                    s::Type::Bool => s::DataOp::BoolEq,
                    s::Type::Str => s::DataOp::StrEq,
                    _ => return Err(later("switch type")),
                };
                for case in cases {
                    let literal = fresh(&mut emitter.result, ty)?;
                    let condition = fresh(&mut emitter.result, s::Type::Bool)?;
                    let immediate = s::Immediate::Constant(translate_constant(&case.value)?);
                    emitter.result.code.push(data(s::DataOp::Const, vec![], literal, immediate));
                    emitter.result.code.push(data(
                        op, vec![emitter.reg(*value)?, literal], condition, s::Immediate::None,
                    ));
                    let next_pc = emitter.result.code.len() as u32 + 1;
                    emitter.result.code.push(s::Instruction::Cond {
                        condition, yes: emitter.branch(&case.target)?,
                        no: s::Edge { target_pc: next_pc, assignments: vec![] },
                    });
                    emitter.result.blocks.push(s::Block { pc: next_pc, params: vec![] });
                }
                s::Instruction::Br(emitter.branch(default)?)
            }
            o::TermKind::Ret { values } => {
                let actual = values.iter().map(|&value| Ok(body.values.get(value)?.ty))
                    .collect::<Result<Vec<_>, GenerateError>>()?;
                if actual != function.signature.results {
                    return Err(error("return signature/type mismatch"));
                }
                if emitter.result.entry_abi == s::EntryAbi::Observation {
                    s::Instruction::Ret(values.iter().map(|&value| emitter.reg(value))
                        .collect::<Result<_, _>>()?)
                } else {
                    s::Instruction::Champ { op: O::RetOwn, arguments: values.iter()
                        .map(|&value| emitter.loc(value).map(A::Location)).collect::<Result<_, _>>()?,
                        flow: s::Flow::Next(vec![]) }
                }
            }
            o::TermKind::RetDefer { tag, values } => {
                if !defer {
                    return Err(error("ret.defer outside defer entry"));
                }
                let mut arguments = vec![];
                let schema = match tag {
                    o::DeferReturnTag::Continue => vec![],
                    o::DeferReturnTag::Replace(label) => {
                        let original = function.attrs.iter().find_map(|attr| match attr {
                            o::FunctionAttr::DeferEntry { replacements } => Some(replacements),
                            _ => None,
                        }).ok_or_else(|| error("missing defer labels"))?;
                        let index = original.iter().position(|item| item.label == *label)
                            .ok_or_else(|| error("unknown defer return label"))? as u32;
                        arguments.push(A::Label(index));
                        O::InvokeDefer.results(s::Tag::Replace(index), &s::ResultContext {
                            labels: labels(module, function)?, ..s::ResultContext::default()
                        })?
                    }
                };
                let actual = values.iter().map(|&value| kind(module, body.values.get(value)?.ty))
                    .collect::<Result<Vec<_>, _>>()?;
                if actual != schema {
                    return Err(error("ret.defer result schema mismatch"));
                }
                arguments.extend(values.iter().map(|&value| emitter.loc(value).map(A::Location))
                    .collect::<Result<Vec<_>, _>>()?);
                s::Instruction::Champ { op: O::RetDefer, arguments, flow: s::Flow::Next(vec![]) }
            }
            o::TermKind::Unreachable => s::Instruction::Unreachable,
            _ => return Err(later("failure/cleanup terminator")),
        };
        emitter.result.code.push(term);
    }
    emitter.finish_evidence(&inst_pcs, &term_pcs)?;
    Ok(emitter.result)
}
