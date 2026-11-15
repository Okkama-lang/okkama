//! S2 transport. Machine payloads and outcome ordinals come from the shared SPB registry.
use super::*;
#[path = "cleanup.rs"]
mod cleanup;
#[path = "futures.rs"]
mod futures;
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
            o::ServiceType::CacheState => P::Service(K::CacheState),
            o::ServiceType::TaskState => P::Service(K::TaskState),
            o::ServiceType::Receipt => P::Service(K::Receipt),
            o::ServiceType::UniqueCache => P::Service(K::UniqueCache),
            _ => return Err(later("async service type")),
        },
        // The running body's frame token (`frame<@f>`); `translate` checks that it names its own function.
        o::Type::Frame(_) => P::Service(K::Frame),
        _ => P::Value(translate_type(module, ty)?),
    })
}

/// The operands of a `variant.get` whose receiver is a one-case Error variant (`single_error_case`).
pub(super) fn error_case_get(module: &o::Module, body: &o::Body, op: &o::Operation)
    -> Result<Option<(o::ValueId, o::FieldRef)>, GenerateError>
{
    let o::Operation::Core(o::CoreOp::VariantGet { value, case, field }) = op else { return Ok(None) };
    let Some(layout) = single_error_case(module, body.values.get(*value)?.ty)? else { return Ok(None) };
    if *case != 0 || field.declaration != layout {
        return Err(error("variant.get of a one-case Error variant names another case"));
    }
    Ok(Some((*value, *field)))
}

/// OIR scalar operations whose SPB data form is fallible (`DataOp::fallible`). A conversion fails only
/// where its pair of types can trap; a total one (int to string) stays plain data. Test assertions are
/// evaluated the same way: they fail only outside a running case.
pub(super) fn fallible_scalar(op: &o::Operation) -> bool {
    if test_check(op).is_some() {
        return true;
    }
    if let o::Operation::Core(core) = op
        && let Some((data, _)) = family_operation(core)
    {
        return data.fallible(&[], None);
    }
    match op {
        o::Operation::Core(o::CoreOp::IntMod { .. } | o::CoreOp::RangeNew { .. } | o::CoreOp::IntDiv { .. }
            | o::CoreOp::RatNew { .. } | o::CoreOp::ValEq { .. } | o::CoreOp::ValNe { .. }
            | o::CoreOp::VariantGet { .. } | o::CoreOp::RoleNarrow { .. }) => true,
        o::Operation::Collection(o::CollectionOp::MapGet { .. } | o::CollectionOp::MapKeyAt { .. }
            | o::CollectionOp::MapValueAt { .. } | o::CollectionOp::MapDifference { packet: None, .. }
            | o::CollectionOp::ArraySlice { packet: None, .. } | o::CollectionOp::ArrayRemove { packet: None, .. }
            | o::CollectionOp::ArrayReplace { packet: None, .. }
            | o::CollectionOp::ArrayDifference { packet: None, .. } | o::CollectionOp::ArrayJoin { .. }) => true,
        o::Operation::Core(o::CoreOp::Convert { from, to, .. }) => {
            matches!((numeric_type(*from), numeric_type(*to)),
                (Some(from), Some(to)) if s::conversion(from, to) == Some(true))
        }
        _ => false,
    }
}

/// The SPB 17 result of a fallible collection operation on operands of `inputs`.
fn collection_result(module: &o::Module, op: s::DataOp, inputs: &[s::Type]) -> Result<s::Type, GenerateError> {
    use s::DataOp as D;
    let receiver = *inputs.first().ok_or_else(|| error("collection operation without a receiver"))?;
    let entry = |index: u32| composites(module).entries().get(index as usize).copied();
    Ok(match (op, receiver) {
        (D::ArraySlice | D::ArrayRemove | D::ArrayReplace | D::ArrayDifference | D::MapDifference, _) => receiver,
        (D::ArrayJoin, _) => s::Type::Str,
        (D::MapGet | D::MapValueAt, s::Type::Map(id)) => match entry(id) {
            Some(s::Composite::Map { value, .. }) => value,
            _ => return Err(error("map operation on a non-map composite")),
        },
        (D::MapKeyAt, s::Type::Map(id)) => match entry(id) {
            Some(s::Composite::Map { key, .. }) => key,
            _ => return Err(error("map operation on a non-map composite")),
        },
        _ => return Err(later("collection operation outcome")),
    })
}

/// The Champ form of a builder transition with outcomes.
fn builder_opcode(op: &o::CollectionOp) -> Option<O> {
    use o::CollectionOp as C;
    Some(match op {
        C::BuilderPush { .. } | C::MapBuilderPut { .. } => O::BuilderPush,
        C::BuilderAppend { .. } | C::MapBuilderAppend { .. } => O::BuilderAppend,
        C::BuilderAppendRange { .. } => O::BuilderRange,
        C::BuilderFinish { .. } | C::MapBuilderFinish { .. } => O::BuilderFinish,
        _ => return None,
    })
}

/// A test assertion as its SPB 17 test operation and operands.
fn test_check(op: &o::Operation) -> Option<(s::DataOp, Vec<o::ValueId>)> {
    let o::Operation::Core(core) = op else {
        return None;
    };
    Some(match *core {
        o::CoreOp::TestOk { condition } => (s::DataOp::TestOk, vec![condition]),
        o::CoreOp::TestIs { left, right } => (s::DataOp::TestIs, vec![left, right]),
        o::CoreOp::TestIsnt { left, right } => (s::DataOp::TestIsnt, vec![left, right]),
        o::CoreOp::TestFail { message } => (s::DataOp::TestFail, vec![message]),
        _ => return None,
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
    synthetic: &'a roles::Synthetic,
    /// Finite action-value origins of this body, computed at its first `fn.call`.
    origins: Option<BTreeMap<o::ValueId, Option<BTreeSet<usize>>>>,
    /// Plain two-instruction expansions (`fn.closure`/`fn.bind`/`fn.bind.role` without a packet, `cap.make`
    /// with one): the pc of the first instruction and the register it defines for the second.
    expansions: Vec<(u32, u32)>,
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
        // An async defer entry suspends its caller while its shielded child runs (C-6).
        let async_defer = defer && declaration.attrs.contains(&o::FunctionAttr::Async);
        if !call.type_args.is_empty() || !call.descriptors.is_empty() || (summary.contract.suspends && !async_defer)
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
        // An async defer entry takes its frame; its captures arrive through its start continuation (C-2).
        let inputs = match declaration.body.as_ref().and_then(|body| body.frame_layout.as_ref()) {
            Some(layout) if async_defer => {
                let start = layout.continuations.first().ok_or_else(|| error("async defer entry without a start"))?;
                start.saved.iter().map(|&slot| {
                    let slot = layout.slots.get(slot as usize).ok_or_else(|| error("start slot outside layout"))?;
                    kind(self.module, slot.ty)
                }).collect::<Result<Vec<_>, _>>()?
            }
            _ => declaration.signature.params.iter().map(|&ty| kind(self.module, ty)).collect::<Result<Vec<_>, _>>()?,
        };
        let params = inputs;
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
        // An observation keeps no obligation, but its result may still depend on a parameter (a holder of a
        // role or Atomic reference): the entry contract states the same retained parameters.
        let retained_params = retained_parameters(summary)?.into_iter().collect();
        self.summaries.push(s::Summary { entry: function.as_u32(), may_fail, retained_params });
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

    /// `call.role` and `fn.call`: a dispatch on the receiver's object descriptor. A role call lists every
    /// implementation of the role (one, for a concrete receiver); a call of an action value lists the
    /// closure layouts of its finite origins, or every closure layout of its `fn` type when it is open.
    /// The contract is the site summary's; the loader checks that every listed implementation refines it.
    fn dispatch(&mut self, op: &o::CallOp) -> Result<(O, Vec<A>, s::ResultContext), GenerateError> {
        let (receiver, role, slot, call) = match op {
            o::CallOp::Role { receiver, role, slot, call } => (*receiver, role.as_u32(), *slot, call),
            o::CallOp::Closure { callee, call } => {
                let ty = self.body.values.get(callee.value)?.ty;
                (*callee, roles::callable_role(self.module, ty)?, 0, call)
            }
            _ => return Err(error("expected a dynamic call")),
        };
        let summary = self.module.summaries.get(call.summary.ok_or_else(|| error("dynamic call requires summary"))?)?;
        // An open call (its callee's closures are unknown) has exactly the registered conservative contract of
        // its `fn` type, with the open Closure protocol: its result packet then goes to `own.dynamic`.
        let open = match (op, summary.contract.protocol) {
            (o::CallOp::Closure { callee, .. }, Some(protocol)) => {
                let ty = self.body.values.get(callee.value)?.ty;
                if o::catalogue::open_callable_summary(self.module, ty, protocol).as_ref() != Some(summary)
                    || super::protocol(self.module, protocol)? != s::Protocol::Closure
                {
                    return Err(error("open call without the registered open callable contract"));
                }
                true
            }
            (_, Some(_)) => return Err(later("protocol dynamic call")),
            (_, None) => false,
        };
        if !call.type_args.is_empty() || !call.descriptors.is_empty() || summary.contract.suspends
            || summary.contract.async_destroy || summary.contract.publication != o::Publication::Atomic
        {
            return Err(later("async/generic dynamic call"));
        }
        let shielded = call.cleanup == o::CallCleanupMode::Shielded;
        if shielded && !self.function.attrs.contains(&o::FunctionAttr::Cleanup) {
            return Err(error("cleanup shielded outside cleanup function"));
        }
        if !shielded && summary.contract.failures.contains(&o::FailureKind::Cancelled) {
            return Err(later("delivered cancellation"));
        }
        let declaration = self.objects.get(role as usize).and_then(|object| object.role.as_ref())
            .ok_or_else(|| error("dynamic call of a non-role"))?;
        let signature = declaration.slots.get(slot as usize).ok_or_else(|| error("dynamic call of a missing slot"))?;
        let observation = summary.entry_kind == o::EntryKind::ObservationAdmissionStaysCaller;
        if observation != (signature.entry_abi == s::EntryAbi::Observation) {
            return Err(error("dynamic call ABI differs from its slot"));
        }
        if call.args.len() != signature.params.len() || call.args.len() + 1 != summary.parameters.len() {
            return Err(error("dynamic call argument arity mismatch"));
        }
        let results = summary.contract.results.iter().map(|&ty| translate_type(self.module, ty))
            .collect::<Result<Vec<_>, _>>()?;
        if results != signature.results {
            return Err(error("dynamic call summary/result signature mismatch"));
        }
        let failures = cleanup::mask(&summary.contract.failures.iter().copied()
            .filter(|kind| *kind != o::FailureKind::Cancelled).collect::<Vec<_>>())?;
        let retained = retained_parameters(summary)?;
        let implementations = self.objects.iter().enumerate()
            .filter(|(_, object)| object.implements.iter().any(|table| table.role == role))
            .map(|(id, _)| id as u32).collect::<Vec<_>>();
        let targets = match op {
            o::CallOp::Role { .. } => match self.module.types.get(self.body.values.get(receiver.value)?.ty)? {
                o::Type::Object { decl, .. } => vec![decl.as_u32()],
                _ => implementations,
            },
            // An open call can reach every closure layout of its `fn` type.
            _ if open => implementations,
            _ => {
                if self.origins.is_none() {
                    self.origins = Some(roles::callable_origins(self.module, self.body, self.synthetic)?);
                }
                match self.origins.as_ref().and_then(|origins| origins.get(&receiver.value)).cloned().flatten() {
                    Some(layouts) if !layouts.is_empty() => layouts.iter()
                        .map(|&index| self.synthetic.closure_object(self.module, index)).collect(),
                    _ => implementations,
                }
            }
        };
        if targets.is_empty() {
            return Err(error("dynamic call without implementations"));
        }
        let dispatch = s::Dispatch {
            role, slot, failures, retained_params: retained.into_iter().collect(), targets,
        };
        let mut arguments = vec![A::Dispatch(dispatch)];
        if !observation {
            arguments.push(A::Flag(shielded));
        }
        arguments.push(A::Location(self.loc(receiver.value)?));
        arguments.extend(call.args.iter().map(|arg| self.loc(arg.value).map(A::Location))
            .collect::<Result<Vec<_>, _>>()?);
        match (observation, call.packet) {
            (true, None) if call.deps.is_empty() => {}
            (false, Some(packet)) => {
                if kind(self.module, self.body.values.get(packet)?.ty)? != P::Service(K::Packet) {
                    return Err(error("owning dynamic call input is not a packet"));
                }
                arguments.push(A::Location(self.loc(packet)?));
                arguments.extend(call.deps.iter().map(|&dep| self.loc(dep).map(A::Location))
                    .collect::<Result<Vec<_>, _>>()?);
            }
            _ => return Err(error("dynamic call packet differs from its ABI")),
        }
        let opcode = if observation { O::InvokeDispatch } else { O::InvokeDispatchOwn };
        Ok((opcode, arguments, s::ResultContext {
            value: signature.results.first().copied(), may_fail: failures != 0, ..s::ResultContext::default()
        }))
    }

    /// `call.cap`: a direct call of the computed field's function with the cap (its owner's reference) first.
    fn cap_call(&mut self, cap: o::CallArgument, field: o::FieldRef, call: &o::CallSite)
        -> Result<(O, Vec<A>, s::ResultContext), GenerateError>
    {
        let o::DeclarationKind::Cap { fields, .. } = &self.module.declarations.get(field.declaration)?.kind else {
            return Err(error("call.cap of a non-cap field"));
        };
        let function = fields.get(field.index as usize).ok_or_else(|| error("call.cap of a missing field"))?.function;
        let mut site = call.clone();
        site.args.insert(0, cap);
        let summary = self.module.summaries.get(site.summary.ok_or_else(|| error("missing summary"))?)?;
        if summary.entry_kind == o::EntryKind::ObservationAdmissionStaysCaller {
            // An observation takes exactly its parameters; the cap's proof-only deps stay with its value.
            site.deps.clear();
            let (arguments, context) = self.observation(function, &site)?;
            Ok((O::InvokeObserve, arguments, context))
        } else {
            let (arguments, context) = self.call(function, &site, false)?;
            Ok((O::InvokeCallOwn, arguments, context))
        }
    }

    /// `fn.closure`/`fn.bind`/`fn.bind.role`: allocate the closure layout with its captures, then view it as
    /// the `fn` role. A closure that captures a resource by reference gets an empty packet from own (the
    /// capture's obligation stays with its owner, P2.4 §4.1): it commits like any construction with a packet,
    /// so the resource-reaching closure object is committed (CREATION_UNCOMMITTED) and owns nothing.
    fn closure(&mut self, site: roles::ClosureSite, results: &[o::ValueId]) -> Result<(), GenerateError> {
        let index = self.synthetic.closure_index(&site.layout).ok_or_else(|| error("action value without layout"))?;
        let layout = self.synthetic.closure_object(self.module, index);
        self.view_construction(layout, &site.values, site.packet, results)
    }

    /// `any.box`: allocate the any box of the value's type with the value, then view it as `any`. A boxed value
    /// that reaches a resource comes with its packet, committed like any construction with a packet.
    fn any_box(&mut self, ty: o::TypeId, value: o::ValueId, packet: Option<o::ValueId>, results: &[o::ValueId])
        -> Result<(), GenerateError>
    {
        let layout = self.synthetic.box_object(self.module, ty).ok_or_else(|| error("any.box without its box"))?;
        self.view_construction(layout, &[value], packet, results)
    }

    /// The construction of an object that a value views (a closure layout as its `fn` role, an any box as
    /// `any`): allocation and move; with a packet also the commit into the scope and the join of the packets.
    fn view_construction(&mut self, layout: u32, values: &[o::ValueId], packet: Option<o::ValueId>,
        results: &[o::ValueId]) -> Result<(), GenerateError>
    {
        let inputs = values.iter().map(|&value| self.reg(value)).collect::<Result<Vec<_>, _>>()?;
        let object = fresh(&mut self.result, s::Type::Object(layout))?;
        let value = self.reg(*results.first().ok_or_else(|| error("view construction without result"))?)?;
        let allocation = s::Instruction::Data { op: s::DataOp::ObjNew, inputs, output: Some(object),
            immediate: s::Immediate::Object(layout) };
        let Some(packet) = packet else {
            if self.objects[layout as usize].layout.may_reach_resource {
                return Err(unsupported("a value referencing a resource without its packet"));
            }
            self.expansions.push((self.result.code.len() as u32, object));
            self.result.code.push(allocation);
            self.result.code.push(data(s::DataOp::Move, vec![object], value, s::Immediate::None));
            return Ok(());
        };
        let [_, joined] = results else { return Err(error("a construction with a packet has two results")) };
        let committed = L::Service(self.result.services.len() as u32);
        self.result.services.push(K::Packet);
        self.result.code.push(allocation);
        self.result.code.push(s::Instruction::Champ { op: O::OwnCreateCommit,
            arguments: vec![A::Location(L::Register(object)), A::Object(layout), A::Boundary(self.scope.as_u32())],
            flow: s::Flow::Next(vec![committed]) });
        self.result.code.push(s::Instruction::Champ { op: O::OwnJoin,
            arguments: vec![A::Location(self.loc(packet)?), A::Location(committed)],
            flow: s::Flow::Next(vec![self.loc(*joined)?]) });
        self.result.code.push(data(s::DataOp::Move, vec![object], value, s::Immediate::None));
        Ok(())
    }

    /// `cap.make` with a packet: the cap is its owner's reference and carries the owner's packet.
    fn cap_with_packet(&mut self, owner: o::ValueId, packet: o::ValueId, results: &[o::ValueId])
        -> Result<(), GenerateError>
    {
        let [value, moved] = results else { return Err(error("cap.make with a packet has two results")) };
        let value = self.reg(*value)?;
        self.expansions.push((self.result.code.len() as u32, value));
        self.result.code.push(data(s::DataOp::Move, vec![self.reg(owner)?], value, s::Immediate::None));
        self.result.code.push(s::Instruction::Champ { op: O::OwnMove, arguments: vec![A::Location(self.loc(packet)?)],
            flow: s::Flow::Next(vec![self.loc(*moved)?]) });
        Ok(())
    }

    /// A fallible scalar has explicit outcomes in S2: the loader never admits its data form here.
    /// Without a block unwind its failure leaves the function through a raise block right after it,
    /// which is what Low defines for such a block; the loader rejects that exit if services are live.
    fn evaluate(&mut self, data: s::Instruction, unwind: Option<&o::BlockUnwind>) -> Result<(), GenerateError> {
        let s::Instruction::Data { op, inputs, output: Some(output), immediate } = data else {
            return Err(error("fallible scalar without a result"));
        };
        let result = self.result.registers[output as usize];
        let pc = self.result.code.len() as u32;
        let produced = |destination| s::Assignment { destination, source: s::Source::ProducedSlot(0) };
        let (unwind, exit) = match unwind {
            Some(unwind) => {
                let mut args = unwind.target.args.iter().copied().map(o::EdgeArg::Value).collect::<Vec<_>>();
                args.push(o::EdgeArg::Produced(o::EdgeResult { outcome: o::Outcome::Unwind, slot: 0 }));
                let edge = self.edge(unwind.target.block, &args, o::Outcome::Unwind, &[P::Value(s::Type::Failure)])?;
                let sources = edge.assignments.iter().filter_map(|assignment| match assignment.source {
                    s::Source::Location(location) => Some(location),
                    s::Source::ProducedSlot(_) => None,
                }).collect();
                self.result.proofs.roots.push(s::RootRequirement { pc, locations: sources });
                (edge, None)
            }
            None => {
                let failure = fresh(&mut self.result, s::Type::Failure)?;
                (s::Edge { target_pc: pc + 1, assignments: vec![produced(L::Register(failure))] }, Some(failure))
            }
        };
        let next = pc + 1 + u32::from(exit.is_some());
        let normal = s::Edge { target_pc: next, assignments: vec![produced(L::Register(output))] };
        self.result.code.push(s::Instruction::Champ {
            op: O::Evaluate,
            arguments: std::iter::once(A::Evaluation(s::Evaluation { op, result, immediate }))
                .chain(inputs.into_iter().map(|input| A::Location(L::Register(input)))).collect(),
            flow: s::Flow::Outcomes(vec![
                s::Outcome { tag: s::Tag::Normal, edge: normal },
                s::Outcome { tag: s::Tag::Unwind, edge: unwind },
            ]),
        });
        if let Some(failure) = exit {
            self.result.blocks.push(s::Block { pc: pc + 1, params: vec![L::Register(failure)] });
            self.result.proofs.roots.push(s::RootRequirement { pc: pc + 1, locations: vec![L::Register(failure)] });
            self.result.code.push(s::Instruction::Raise(failure));
        }
        self.result.blocks.push(s::Block { pc: next, params: vec![L::Register(output)] });
        Ok(())
    }

    /// The field write that completes an incomplete construction, then the completed identity's admission
    /// into `state`. An object without destroy completes no obligation, so its packet is empty.
    fn complete_construction(&mut self, object: o::ValueId, field: o::FieldRef, value: o::ValueId,
        state: o::ValueId, results: &[o::ValueId]) -> Result<(), GenerateError>
    {
        let [result] = results else { return Err(error("construction completion needs its state result")) };
        let s::Type::Object(descriptor) = translate_type(self.module, self.body.values.get(object)?.ty)? else {
            return Err(error("construction completion of a non-object"));
        };
        if self.objects[descriptor as usize].layout.has_destroy {
            return Err(later("construction completion of an identity with destroy"));
        }
        let storage = self.objects.get(descriptor as usize)
            .and_then(|declared| declared.fields.get(field.index as usize))
            .filter(|_| field.declaration.as_u32() == descriptor)
            .map(|declared| declared.storage).ok_or_else(|| error("invalid completion field"))?;
        let packet = L::Service(self.result.services.len() as u32);
        self.result.services.push(K::Packet);
        self.result.code.push(s::Instruction::Data { op: s::DataOp::ObjInit,
            inputs: vec![self.reg(object)?, self.reg(value)?], output: None,
            immediate: s::Immediate::Field(storage) });
        self.result.code.push(s::Instruction::Champ { op: O::OwnEmpty, arguments: vec![],
            flow: s::Flow::Next(vec![packet]) });
        self.result.code.push(s::Instruction::Champ { op: O::OwnAdmit,
            arguments: vec![A::Location(self.loc(state)?), A::Location(packet)],
            flow: s::Flow::Next(vec![self.loc(*result)?]) });
        Ok(())
    }

    /// A builder transition: buffer, operands and the journal packet in; outcomes per the registry.
    fn builder(&mut self, opcode: O, op: &o::CollectionOp, normal: &o::InvokeEdge, unwind: &o::InvokeEdge)
        -> Result<s::Instruction, GenerateError>
    {
        use o::CollectionOp as C;
        let (flag, values, packet) = match op {
            C::BuilderPush { buffer, value, packet } => (None, vec![*buffer, *value], packet),
            C::MapBuilderPut { buffer, key, value, packet } => (None, vec![*buffer, *key, *value], packet),
            C::BuilderAppend { buffer, array, packet } => (None, vec![*buffer, *array], packet),
            C::MapBuilderAppend { buffer, map, packet } => (None, vec![*buffer, *map], packet),
            C::BuilderAppendRange { buffer, start, end, inclusive, packet } => {
                (Some(*inclusive), vec![*buffer, *start, *end], packet)
            }
            C::BuilderFinish { buffer, packet } | C::MapBuilderFinish { buffer, packet } => {
                (None, vec![*buffer], packet)
            }
            _ => return Err(error("expected a builder transition")),
        };
        let packet = packet.ok_or_else(|| error("builder transition without its element journal"))?;
        let mut arguments = flag.map(A::Flag).into_iter().collect::<Vec<_>>();
        arguments.extend(values.iter().chain([&packet]).map(|&value| self.loc(value).map(A::Location))
            .collect::<Result<Vec<_>, _>>()?);
        let buffer = translate_type(self.module, self.body.values.get(values[0])?.ty)?;
        let finished = match buffer {
            s::Type::Builder(id) => match composites(self.module).entries().get(id as usize) {
                Some(s::Composite::Builder { array }) => Some(*array),
                _ => None,
            },
            s::Type::MapBuilder(id) => Some(s::Type::Map(id)),
            _ => return Err(error("builder transition on a non-builder")),
        };
        let context = s::ResultContext {
            buffer: Some(buffer),
            value: if opcode == O::BuilderFinish { finished } else { None },
            may_fail: true,
            ..s::ResultContext::default()
        };
        let outcomes = [(s::Tag::Normal, o::Outcome::Normal, normal), (s::Tag::Unwind, o::Outcome::Unwind, unwind)]
            .into_iter().map(|(tag, outcome, edge)| {
                let schema = opcode.results(tag, &context)?;
                Ok(s::Outcome { tag, edge: self.edge(edge.block, &edge.args, outcome, &schema)? })
            }).collect::<Result<_, GenerateError>>()?;
        Ok(s::Instruction::Champ { op: opcode, arguments, flow: s::Flow::Outcomes(outcomes) })
    }

    /// A scalar family operation as an invocation: a fallible one is an `evaluate` whose outcomes are the
    /// invocation's; a total one (which OIR-2 lets trap) computes into a fresh register and takes the
    /// normal edge, its result in place of the produced slot.
    fn family_invocation(&mut self, op: s::DataOp, inputs: &[o::ValueId], normal: &o::InvokeEdge,
        unwind: &o::InvokeEdge) -> Result<s::Instruction, GenerateError>
    {
        let (_, result) = op.scalar_signature().ok_or_else(|| error("scalar family operation without signature"))?;
        if op.fallible(&[], None) {
            let context = s::ResultContext { value: Some(result), may_fail: true, ..s::ResultContext::default() };
            let mut arguments = vec![A::Evaluation(s::Evaluation { op, result, immediate: s::Immediate::None })];
            for &input in inputs {
                arguments.push(A::Location(self.loc(input)?));
            }
            let outcomes = [(s::Tag::Normal, o::Outcome::Normal, normal),
                (s::Tag::Unwind, o::Outcome::Unwind, unwind)].into_iter().map(|(tag, outcome, edge)| {
                let schema = O::Evaluate.results(tag, &context)?;
                Ok(s::Outcome { tag, edge: self.edge(edge.block, &edge.args, outcome, &schema)? })
            }).collect::<Result<_, GenerateError>>()?;
            return Ok(s::Instruction::Champ { op: O::Evaluate, arguments, flow: s::Flow::Outcomes(outcomes) });
        }
        let registers = inputs.iter().map(|&input| self.reg(input)).collect::<Result<Vec<_>, _>>()?;
        let value = fresh(&mut self.result, result)?;
        self.result.code.push(data(op, registers, value, s::Immediate::None));
        let mut edge = self.edge(normal.block, &normal.args, o::Outcome::Normal, &[P::Value(result)])?;
        for assignment in &mut edge.assignments {
            if assignment.source == s::Source::ProducedSlot(0) {
                assignment.source = s::Source::Location(L::Register(value));
            }
        }
        Ok(s::Instruction::Br(edge))
    }

    /// `variant.get` of a one-case Error variant: the value is its case object, so the payload is a field
    /// read that cannot fail. It computes into a fresh register and takes the normal edge.
    fn error_case_payload(&mut self, value: o::ValueId, field: o::FieldRef, normal: &o::InvokeEdge)
        -> Result<s::Instruction, GenerateError>
    {
        let storage = variant_storage(self.module, self.objects, field)?;
        let ty = self.objects[field.declaration.as_u32() as usize].layout.storage_fields[storage as usize];
        let target = fresh(&mut self.result, ty)?;
        let receiver = self.reg(value)?;
        self.result.code.push(data(s::DataOp::ObjGet, vec![receiver], target, s::Immediate::Field(storage)));
        let mut edge = self.edge(normal.block, &normal.args, o::Outcome::Normal, &[P::Value(ty)])?;
        for assignment in &mut edge.assignments {
            if assignment.source == s::Source::ProducedSlot(0) {
                assignment.source = s::Source::Location(L::Register(target));
            }
        }
        Ok(s::Instruction::Br(edge))
    }

    fn invocation(&mut self, term: &o::TermKind) -> Result<s::Instruction, GenerateError> {
        let (opcode, arguments, context, edges) = match term {
            o::TermKind::InvokeOwn { op, normal, unwind } => {
                if let Some((value, field)) = error_case_get(self.module, self.body, op)? {
                    return self.error_case_payload(value, field, normal);
                }
                if let o::Operation::Core(core) = op.as_ref()
                    && let Some((data, inputs)) = family_operation(core)
                {
                    return self.family_invocation(data, &inputs, normal, unwind);
                }
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
                        o::CoreOp::IntDiv { left, right } => Some((s::DataOp::RatNew,
                            s::Type::Rat, s::Immediate::None, vec![*left, *right])),
                        o::CoreOp::RatNew { numerator, denominator } => Some((s::DataOp::RatNew,
                            s::Type::Rat, s::Immediate::None, vec![*numerator, *denominator])),
                        o::CoreOp::ValEq { left, right } => Some((s::DataOp::ValEq,
                            s::Type::Bool, s::Immediate::None, vec![*left, *right])),
                        o::CoreOp::ValNe { left, right } => Some((s::DataOp::ValNe,
                            s::Type::Bool, s::Immediate::None, vec![*left, *right])),
                        o::CoreOp::RoleNarrow { value, ty } => Some((s::DataOp::RoleNarrow,
                            translate_type(self.module, *ty)?, s::Immediate::None, vec![*value])),
                        o::CoreOp::VariantGet { value, case, field } => {
                            let storage = variant_storage(self.module, self.objects, *field)?;
                            let ty = self.objects[field.declaration.as_u32() as usize].layout.storage_fields
                                [storage as usize];
                            Some((s::DataOp::VariantGet, ty, s::Immediate::Updates(vec![*case, storage]), vec![*value]))
                        }
                        o::CoreOp::Convert { value, from, to } if fallible_scalar(op) => {
                            let target = numeric_type(*to).ok_or_else(|| later("conversion type"))?;
                            if numeric_type(*from).is_none() {
                                return Err(later("conversion type"));
                            }
                            Some((s::DataOp::Convert, target, s::Immediate::None, vec![*value]))
                        }
                        // The check's bool outcome (whether it held) is not used by the source.
                        _ => test_check(op).map(|(check, inputs)| (check, s::Type::Bool, s::Immediate::None, inputs)),
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
                if let o::Operation::Collection(collection) = op.as_ref()
                    && let Some(opcode) = builder_opcode(collection)
                {
                    return self.builder(opcode, collection, normal, unwind);
                }
                if let o::Operation::Collection(collection) = op.as_ref()
                    && !matches!(collection, o::CollectionOp::ArrayGet { .. })
                {
                    let (data, values) = collection_data(self.module, self.body, collection, None)?
                        .ok_or_else(|| later(op.opcode()))?;
                    let inputs = values.iter()
                        .map(|&value| translate_type(self.module, self.body.values.get(value)?.ty))
                        .collect::<Result<Vec<_>, GenerateError>>()?;
                    let result = collection_result(self.module, data, &inputs)?;
                    let context = s::ResultContext {
                        value: Some(result), may_fail: true, ..s::ResultContext::default()
                    };
                    let evaluation = s::Evaluation { op: data, result, immediate: s::Immediate::None };
                    let mut arguments = vec![A::Evaluation(evaluation)];
                    arguments.extend(values.iter().map(|&value| self.loc(value).map(A::Location))
                        .collect::<Result<Vec<_>, _>>()?);
                    let outcomes = [(s::Tag::Normal, o::Outcome::Normal, normal),
                        (s::Tag::Unwind, o::Outcome::Unwind, unwind)].into_iter().map(|(tag, outcome, edge)| {
                        let schema = O::Evaluate.results(tag, &context)?;
                        Ok(s::Outcome { tag, edge: self.edge(edge.block, &edge.args, outcome, &schema)? })
                    }).collect::<Result<_, GenerateError>>()?;
                    return Ok(s::Instruction::Champ { op: O::Evaluate, arguments, flow: s::Flow::Outcomes(outcomes) });
                }
                if let o::Operation::Collection(o::CollectionOp::ArrayGet { array, index }) = op.as_ref() {
                    // A plain array or a byte array (`@byte`, SPB 17 `Type::Bytes`): the loader reads the element
                    // type from the receiver's declared type.
                    let receiver = translate_type(self.module, self.body.values.get(*array)?.ty)?;
                    let element = match receiver {
                        s::Type::Array(_) | s::Type::Bytes => s::element_type(receiver),
                        _ => return Err(error("array.get requires a plain array or a byte array")),
                    };
                    let context = s::ResultContext { value: element, may_fail: true, ..s::ResultContext::default() };
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
                if let o::Operation::Call(call @ (o::CallOp::Role { .. } | o::CallOp::Closure { .. })) = op.as_ref() {
                    let (opcode, arguments, context) = self.dispatch(call)?;
                    (opcode, arguments, context, vec![normal, unwind])
                } else if let o::Operation::Call(o::CallOp::Cap { cap, field, call }) = op.as_ref() {
                    let (opcode, arguments, context) = self.cap_call(*cap, *field, call)?;
                    (opcode, arguments, context, vec![normal, unwind])
                } else {
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
        if !matches!(opcode, O::InvokeObserve | O::InvokeDispatch) {
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
            if !matches!(opcode, O::InvokeObserve | O::InvokeDispatch) {
                opcode.call_post_offsets(tag, &context)?;
            }
            Ok(s::Outcome { tag, edge: self.edge(edge.block, &edge.args, outcome, &schema)? })
        }).collect::<Result<_, GenerateError>>()?;
        Ok(s::Instruction::Champ { op: opcode, arguments, flow: s::Flow::Outcomes(outcomes) })
    }
}

/// The SPB 17 instructions an OIR instruction expands to, when more than one: an action value or an `any` box
/// (allocation and move; with a packet also commit and join) and a cap with a packet (move and packet move).
pub(super) fn expansion(op: &o::Operation) -> u32 {
    match op {
        o::Operation::Core(o::CoreOp::FnClosure { packet, .. } | o::CoreOp::FnBind { packet, .. }
            | o::CoreOp::FnBindRole { packet, .. } | o::CoreOp::AnyBox { packet, .. }) => {
            if packet.is_some() { 4 } else { 2 }
        }
        o::Operation::Core(o::CoreOp::CapMake { packet: Some(_), .. }) => 2,
        _ => 1,
    }
}

/// The summary parameters (receiver first) whose graphs a call may retain: in result dependencies, result
/// aliases or parameter retention. SPB keeps the conservative parameter inventory.
fn retained_parameters(summary: &o::Summary) -> Result<BTreeSet<u32>, GenerateError> {
    let mut selections = summary.parameters.iter().filter_map(|parameter| parameter.retention.as_ref())
        .chain(summary.results.iter().flat_map(|result| result.aliases.iter())).collect::<Vec<_>>();
    let mut retained = summary.results.iter().flat_map(|result| result.external_deps.iter().copied())
        .map(u32::from).collect::<BTreeSet<_>>();
    while let Some(selection) = selections.pop() {
        match selection {
            o::SummarySelection::Parameter(parameter) | o::SummarySelection::OwnerGraph(parameter)
            | o::SummarySelection::Field { parameter, .. } | o::SummarySelection::Path { parameter, .. } => {
                retained.insert(u32::from(*parameter));
            }
            o::SummarySelection::Union(parts) => selections.extend(parts),
            o::SummarySelection::Descriptor(_) => return Err(later("descriptor summary retention")),
        }
    }
    Ok(retained)
}

pub(super) fn translate(module: &o::Module, id: o::FuncId, function: &o::Function, objects: &[s::Object],
    tables: &mut s::ChampTables, synthetic: &roles::Synthetic) -> Result<s::Function, GenerateError>
{
    let asynchronous = futures::async_body(module, id, function)?;
    if function.body.is_none() {
        return cleanup::external(module, function);
    }
    if !function.type_params.is_empty() {
        return Err(later(&format!("{}: generic entry", function.symbol)));
    }
    if let Some(attr) = function.attrs.iter().find(|attr| !declaration_attr(attr) && !matches!(attr,
        o::FunctionAttr::Entry | o::FunctionAttr::Hidden | o::FunctionAttr::Cleanup | o::FunctionAttr::Async
        | o::FunctionAttr::DeferEntry { .. } | o::FunctionAttr::Ctor(_) | o::FunctionAttr::Destroy(_)))
    {
        return Err(later(&format!("{}: entry attribute {attr:?}", function.symbol)));
    }
    let defer = function.attrs.iter().any(|attr| matches!(attr, o::FunctionAttr::DeferEntry { .. }));
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
    let entry_abi = if let Some(shape) = &asynchronous {
        // Its layout is pushed once the pcs of its sites are known, below.
        let layout = u32::try_from(tables.frames.len()).map_err(|_| error("too many frame layouts"))?;
        if shape.defer { s::EntryAbi::AsyncDefer { layout } } else { s::EntryAbi::AsyncBody { layout } }
    } else if defer {
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
    // An async body returns nothing: its results are the Future's outcome, which fut.publish delivers.
    if let Some(body) = &asynchronous {
        result.results.extend(body.outcome);
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
            if block.unwind.is_some() && !fallible_scalar(&inst.op) && !total_family_operation(&inst.op)
                && o::ContractRegistry::new(module)
                .lookup(&inst.op).map_err(|failure| error(failure.to_string()))?.unwind.is_some()
            {
                return Err(later(&format!("{}: implicit instruction unwind {}", function.symbol, inst.op.opcode())));
            }
            inst_pcs.insert(inst_id, pc);
            let size = match &inst.op {
                o::Operation::Core(o::CoreOp::RangeLen { .. }) => 3,
                // The last field write, the completed identity's packet, and its admission into the state.
                o::Operation::Core(o::CoreOp::ObjInit { completion: Some(_), .. }) => 3,
                // D8 registration is static CFG metadata, not a second runtime defer runner.
                o::Operation::Lexical(o::LexicalOp::DeferRegister { .. }) => 0,
                // allocate the closure layout, then move it into the `fn` role; move a cap and its packet.
                op if expansion(op) > 1 => expansion(op),
                // evaluate, then the function failure exit unless the block unwind receives the failure.
                op if fallible_scalar(op) && block.unwind.is_none() => 2,
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
            // A plain variant value is its case object moved into the variant type.
            o::TermKind::InvokeOwn { op, .. } if matches!(op.as_ref(),
                o::Operation::Core(o::CoreOp::VariantNew { packet: None, ty, .. })
                    if matches!(translate_type(module, *ty), Ok(s::Type::Variant(_)))) => 3,
            // A total scalar family operation computes into a register and branches with it, as does the
            // payload read of a one-case Error variant.
            o::TermKind::InvokeOwn { op, .. } if total_family_operation(op)
                || error_case_get(module, body, op)?.is_some() => 2,
            o::TermKind::InvokeOwn { op, .. } if matches!(op.as_ref(),
                o::Operation::Core(o::CoreOp::ObjNew { packet: None, .. }
                    | o::CoreOp::ObjNext { packet: None, .. } | o::CoreOp::VariantNew { packet: None, .. })) => 2,
            // A case that refers to resources: allocate/commit/join, then the move into the variant type.
            o::TermKind::InvokeOwn { op, .. } if matches!(op.as_ref(),
                o::Operation::Core(o::CoreOp::VariantNew { packet: Some(_), ty, .. })
                    if matches!(translate_type(module, *ty), Ok(s::Type::Variant(_)))) => 5,
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
    if asynchronous.is_some() {
        let layout = body.frame_layout.as_ref().ok_or_else(|| error("async body without its frame layout"))?;
        tables.frames.push(futures::frame_layout(module, layout, &inst_pcs, &term_pcs)?);
    }
    let summaries = &mut tables.summaries;
    let mut emitter = Emitter {
        module, function, body, objects, locations, pcs, summaries, scope: entry.lexical_scope, result,
        synthetic, origins: None, expansions: vec![],
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
                let obligation = emitter.loc(emitter.through_frame(owner, value)?)?;
                emitter.result.proofs.dependencies.push(s::Dependency {
                    value: emitter.loc(owner)?, obligations: vec![obligation],
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
                o::Operation::Collection(o::CollectionOp::BuilderAbandon { buffer, packet }
                    | o::CollectionOp::MapBuilderAbandon { buffer, packet }) => {
                    let packet = packet.ok_or_else(|| error("builder.abandon without its element journal"))?;
                    let arguments = vec![A::Location(emitter.loc(*buffer)?), A::Location(emitter.loc(packet)?)];
                    let outputs = inst.results.iter().map(|&value| emitter.loc(value)).collect::<Result<_, _>>()?;
                    s::Instruction::Champ { op: O::BuilderAbandon, arguments, flow: s::Flow::Next(outputs) }
                }
                o::Operation::Core(o::CoreOp::ObjInit { object, field, value, completion: Some(completion) }) => {
                    emitter.complete_construction(*object, *field, *value, completion.state, &inst.results)?;
                    continue;
                }
                o::Operation::Cleanup(op) => emitter.cleanup_instruction(op, &inst.results)?,
                o::Operation::Exit(op) => emitter.exit_instruction(op, &inst.results)?,
                o::Operation::Async(op) => emitter.async_instruction(op, &inst.results)?,
                o::Operation::Core(o::CoreOp::TypeOf { ty, descriptor }) => {
                    let id = match module.types.get(*ty)? {
                        o::Type::Object { decl, .. } => decl.as_u32(),
                        // An action value's runtime descriptor is its `fn` role; its opaque cleanup releases
                        // the value's complete owner graph.
                        o::Type::Fn(_) => {
                            roles::opaque_descriptor(module, *ty, descriptor, o::catalogue::SYNC_CLOSURE_CLEANUP)?;
                            roles::callable_role(module, *ty)?
                        }
                        // The descriptor of `any`; its opaque cleanup releases a box's complete owner graph.
                        o::Type::Any => {
                            roles::opaque_descriptor(module, *ty, descriptor, o::catalogue::ANY_CLEANUP)?;
                            emitter.synthetic.any_object(module)
                        }
                        _ => return Err(error("descriptor of a type without one")),
                    };
                    data(s::DataOp::Const, vec![], emitter.reg(inst.results[0])?,
                        s::Immediate::Constant(s::Constant::Descriptor(id)))
                }
                o::Operation::Call(call @ (o::CallOp::Role { .. } | o::CallOp::Closure { .. })) => {
                    let (opcode, arguments, context) = emitter.dispatch(call)?;
                    if context.may_fail {
                        return Err(error("fallible call requires explicit outcomes"));
                    }
                    s::Instruction::Champ { op: opcode, arguments,
                        flow: s::Flow::Next(inst.results.iter().map(|&v| emitter.loc(v))
                            .collect::<Result<_, _>>()?) }
                }
                o::Operation::Call(o::CallOp::Cap { cap, field, call }) => {
                    let (opcode, arguments, context) = emitter.cap_call(*cap, *field, call)?;
                    if context.may_fail {
                        return Err(error("fallible call requires explicit outcomes"));
                    }
                    s::Instruction::Champ { op: opcode, arguments,
                        flow: s::Flow::Next(inst.results.iter().map(|&v| emitter.loc(v))
                            .collect::<Result<_, _>>()?) }
                }
                o::Operation::Core(o::CoreOp::FnClosure { .. } | o::CoreOp::FnBind { .. }
                    | o::CoreOp::FnBindRole { .. }) => {
                    let site = roles::closure_site(body, &inst.op)?.expect("action value site");
                    emitter.closure(site, &inst.results)?;
                    continue;
                }
                o::Operation::Core(o::CoreOp::AnyBox { .. }) => {
                    let (ty, value, packet) = roles::any_box_site(body, &inst.op)?.expect("any.box site");
                    emitter.any_box(ty, value, packet, &inst.results)?;
                    continue;
                }
                o::Operation::Core(o::CoreOp::CapMake { owner, packet: Some(packet), .. }) => {
                    emitter.cap_with_packet(*owner, *packet, &inst.results)?;
                    continue;
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
                op if test_check(op).is_some() => {
                    let (op, operands) = test_check(op).expect("test assertion");
                    if !inst.results.is_empty() {
                        return Err(error("test assertion cannot produce results"));
                    }
                    let inputs = operands.iter().map(|&value| emitter.reg(value)).collect::<Result<_, _>>()?;
                    // The check's outcome is a fresh bool: whether it held. The source discards it.
                    let held = fresh(&mut emitter.result, s::Type::Bool)?;
                    emitter.evaluate(data(op, inputs, held, s::Immediate::None), block.unwind.as_ref())?;
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
                    let fallible = emitter.result.data_fallible(&translated);
                    if fallible != fallible_scalar(&inst.op) {
                        return Err(error(format!("{}: fallible scalar classification", inst.op.opcode())));
                    }
                    if fallible {
                        emitter.evaluate(translated, block.unwind.as_ref())?;
                        continue;
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
                    // An Error variant is caught by its declaration: any of its cases.
                    s::Type::Object(id) | s::Type::Variant(id) => A::Object(id),
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
            o::TermKind::InvokeOwn { op, normal, unwind } if matches!(op.as_ref(), o::Operation::Async(_)) => {
                emitter.async_invocation(op, normal, unwind)?
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
            o::TermKind::Lookup { collection, key, found, missing, .. } => {
                let value = match translate_type(module, body.values.get(*collection)?.ty)? {
                    array @ (s::Type::Array(_) | s::Type::Bytes) => s::element_type(array),
                    s::Type::Map(id) => match composites(module).entries().get(id as usize) {
                        Some(s::Composite::Map { value, .. }) => Some(*value),
                        _ => None,
                    },
                    _ => None,
                }.ok_or_else(|| error("lookup on a value that is neither an array nor a map"))?;
                s::Instruction::Champ { op: O::Lookup,
                    arguments: vec![A::Location(emitter.loc(*collection)?), A::Location(emitter.loc(*key)?)],
                    flow: s::Flow::Outcomes(vec![
                        s::Outcome { tag: s::Tag::Some,
                            edge: emitter.edge(found.block, &found.args, o::Outcome::Normal, &[P::Value(value)])? },
                        s::Outcome { tag: s::Tag::None, edge: emitter.branch(missing)? },
                    ]) }
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
                if emitter.result.entry_abi == s::EntryAbi::Observation || emitter.result.entry_abi.asynchronous() {
                    // An async body returns nothing: fut.publish just before has delivered its outcome.
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
        };
        emitter.result.code.push(term);
    }
    emitter.finish_evidence(&inst_pcs, &term_pcs)?;
    Ok(emitter.result)
}
