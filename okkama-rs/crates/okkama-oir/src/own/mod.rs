//! Ownership elaboration, independent of the verifier's implementation.
//!
//! Classification freezes source and memoized entry equations before solving.
//! The synchronous rewrite preserves lexical cleanup, snapshot dependencies and
//! checked callable/storage ABIs through the shared outcome emitter. Its public
//! checked entry points certify output with the independent semantic verifier.
pub mod domain;
pub mod analyze;
pub mod bindings;
pub mod edges;
pub mod emit;
mod driver;
pub use driver::{rewrite_subset, rewrite_synchronous, rewrite_synchronous_with};
mod classify;
mod normal;
mod summaries;
mod scopes;
mod inventory;
mod origins;
mod entries;
mod census;
mod routing;
mod arrays;
mod construction;
mod futures;

use crate::*;
use std::collections::BTreeMap;
pub use classify::{Classification, TypeClassification};
pub use scopes::{ElisionBlocker, ScopeFacts};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalBoundary<'a> {
    Native(&'a str),
    Intrinsic(&'a str),
    Callable(TypeId),
    Service(TypeId),
    ServiceProtocol(ProtocolId),
}

/// Mono's actual arguments and bound runtime dictionaries, in ABI order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Substitution {
    pub types: Vec<TypeId>,
    pub descriptors: Vec<DescriptorContract>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalSummary {
    pub summary: Summary,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeSummaryError {
    pub message: String,
}

/// Trusted native/intrinsic and open-callable catalog. An absent contract is
/// an error, including for unknown intrinsics; there is no guessed retention.
pub trait NativeSummaries {
    fn lookup(
        &self,
        boundary: ExternalBoundary<'_>,
        substitution: &Substitution,
    ) -> Result<ExternalSummary, NativeSummaryError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnOptions {
    pub verify: VerifyOptions,
    pub work_limit: usize,
    /// Conservative charged allocation bound, not process RSS.
    pub memory_limit: usize,
    /// Mono binds residual type dictionaries explicitly. Body TypeOf records
    /// additionally bind their concrete type; dictionaries never imply plain.
    pub descriptors: BTreeMap<TypeId, DescriptorContract>,
}

impl Default for OwnOptions {
    fn default() -> Self {
        Self {
            verify: VerifyOptions::default(),
            work_limit: 50_000_000,
            memory_limit: 256 << 20,
            descriptors: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnLimit {
    Work,
    Memory,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnStats {
    pub schema_version: u32,
    pub active_function: Option<FuncId>,
    pub exhausted_limit: Option<OwnLimit>,
    pub functions: usize,
    pub emitted_checked_splits: usize,
    pub splits_considered: usize,
    pub splits_elided: usize,
    pub splits_left_unelided: usize,
    pub checked_partitions: usize,
    pub known_owner_journals: usize,
    pub emitted_dynamic_sites: u64,
    pub universe_nodes: usize,
    pub adapter_equations: usize,
    pub materialized_adapters: usize,
    pub implicit_destroy_edges: usize,
    pub sccs: usize,
    pub cyclic_sccs: usize,
    pub equation_evaluations: usize,
    pub empty_scope_candidates: usize,
    /// Counts actual emitted lexical elisions in the partial driver.
    pub elided_lexical_scopes: usize,
    pub incompatible_publication_joins: usize,
    pub incompatible_protocol_joins: usize,
    pub charged_work: usize,
    pub peak_auxiliary_bytes: usize,
}

impl Default for OwnStats {
    fn default() -> Self {
        Self {
            schema_version: 1,
            active_function: None,
            exhausted_limit: None,
            functions: 0,
            emitted_checked_splits: 0,
            splits_considered: 0,
            splits_elided: 0,
            splits_left_unelided: 0,
            checked_partitions: 0,
            known_owner_journals: 0,
            emitted_dynamic_sites: 0,
            universe_nodes: 0,
            adapter_equations: 0,
            materialized_adapters: 0,
            implicit_destroy_edges: 0,
            sccs: 0,
            cyclic_sccs: 0,
            equation_evaluations: 0,
            empty_scope_candidates: 0,
            elided_lexical_scopes: 0,
            incompatible_publication_joins: 0,
            incompatible_protocol_joins: 0,
            charged_work: 0,
            peak_auxiliary_bytes: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnDiagnosticKind {
    MissingDescriptor,
    InvalidDescriptor,
    IncompatiblePublicationJoin,
    IncompatibleProtocolJoin,
    IncompatibleDestinationJoin,
    IncompatibleEntryJoin,
    IncompatibleProvenanceJoin,
    IncompatibleSignature,
    UnsupportedAsync,
    MissingBoundary,
    InvalidEquation,
    InvalidFlow,
    UnsupportedConstruct,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnDiagnostic {
    pub kind: OwnDiagnosticKind,
    pub function: Option<FuncId>,
    pub site: Option<HighOperationSite>,
    pub message: String,
}

#[derive(Debug)]
pub enum OwnError {
    InvalidInput(Box<VerifyReport>),
    InvalidOutput(Box<VerifyReport>),
    Diagnostics { diagnostics: Vec<OwnDiagnostic>, stats: Box<OwnStats> },
    Incomplete { stats: Box<OwnStats> },
    IncompleteVerification { report: Box<VerifyReport>, stats: Box<OwnStats> },
}

impl std::fmt::Display for OwnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for OwnError {}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OwnAnalysis {
    pub types: BTreeMap<TypeId, TypeClassification>,
    pub managed_references: std::collections::BTreeSet<TypeId>,
    /// Final projections for bound cleanup graphs. Unbound, unowned service
    /// types have no entry; absence never means an empty cleanup effect.
    pub cleanup_effects: BTreeMap<TypeId, ScopeEffectEnvelope>,
    pub scopes: BTreeMap<ScopeId, ScopeFacts>,
}

#[derive(Debug)]
pub struct OwnOutput {
    pub module: Module,
    /// Emitted sites only; collected after unreachable landings are removed.
    pub census: DynamicCensus,
    pub stats: OwnStats,
    pub analysis: OwnAnalysis,
}

#[derive(Debug)]
pub(super) struct Budget<'a> {
    options: &'a OwnOptions,
    pub stats: OwnStats,
    live_bytes: usize,
}

impl<'a> Budget<'a> {
    fn new(options: &'a OwnOptions) -> Self {
        Self { options, stats: OwnStats::default(), live_bytes: 0 }
    }
    pub fn work(&mut self, count: usize) -> Result<(), OwnError> {
        let next = self.stats.charged_work.checked_add(count);
        if next.is_none_or(|next| next > self.options.work_limit) {
            let mut stats = self.stats.clone();
            stats.exhausted_limit = Some(OwnLimit::Work);
            return Err(OwnError::Incomplete { stats: Box::new(stats) });
        }
        self.stats.charged_work = next.unwrap();
        Ok(())
    }
    pub fn memory(&mut self, bytes: usize) -> Result<(), OwnError> {
        let next = self.live_bytes.checked_add(bytes);
        if next.is_none_or(|next| next > self.options.memory_limit) {
            let mut stats = self.stats.clone();
            stats.exhausted_limit = Some(OwnLimit::Memory);
            return Err(OwnError::Incomplete { stats: Box::new(stats) });
        }
        self.live_bytes = next.unwrap();
        self.stats.peak_auxiliary_bytes = self.stats.peak_auxiliary_bytes.max(self.live_bytes);
        Ok(())
    }
    pub fn release(&mut self, bytes: usize) {
        self.live_bytes = self.live_bytes.checked_sub(bytes).expect("balanced ownership memory accounting");
    }
    pub fn resize(&mut self, old: usize, new: usize) -> Result<(), OwnError> {
        if new > old {
            self.memory(new - old)?;
        } else {
            self.release(old - new);
        }
        Ok(())
    }
    pub fn fail(&self, kind: OwnDiagnosticKind, message: impl Into<String>) -> OwnError {
        OwnError::Diagnostics {
            diagnostics: vec![OwnDiagnostic { kind, function: None, site: None, message: message.into() }],
            stats: Box::new(self.stats.clone()),
        }
    }
}

pub(super) fn arena(error: ArenaError) -> OwnError {
    OwnError::Diagnostics {
        diagnostics: vec![OwnDiagnostic {
            kind: OwnDiagnosticKind::InvalidEquation,
            function: None,
            site: None,
            message: error.to_string(),
        }],
        stats: Box::default(),
    }
}

pub(super) fn language(module: &Module, types: &[TypeId]) -> Vec<TypeId> {
    types.iter().copied().filter(|ty| module.types.get(*ty).is_ok_and(|ty| {
        !matches!(ty, Type::Service(_) | Type::Descriptor)
    })).collect()
}

fn external_substitutions(
    module: &Module,
    target: FuncId,
    budget: &mut Budget<'_>,
) -> Result<Vec<Substitution>, OwnError> {
    let mut substitutions = vec![];
    for (_, function) in module.functions.iter() {
        let Some(body) = &function.body else {
            continue;
        };
        // Read arenas directly here: structural verification follows catalog
        // binding, so malformed instruction membership cannot cause a panic.
        let mut ops: Vec<_> = body.insts.iter().map(|(_, instruction)| &instruction.op).collect();
        for (_, block) in body.blocks.iter() {
            if let TermKind::InvokeOwn { op, .. } = &block.term.kind {
                ops.push(op);
            }
        }
        budget.memory(ops.len() * 16)?;
        for op in ops {
            budget.work(1)?;
            let call = match op {
                Operation::Call(CallOp::Direct { function, call } | CallOp::Native { function, call }
                    | CallOp::Intrinsic { function, call }) if *function == target => call,
                _ => continue,
            };
            let mut descriptors = vec![];
            for value in &call.descriptors {
                if let Some(binding) = body.descriptor_bindings.iter().find(|binding| binding.dictionary == *value) {
                    descriptors.push(binding.contract.clone());
                } else if let Ok(ValueData { def: Definition::InstResult { inst, .. }, .. }) = body.values.get(*value)
                    && let Ok(instruction) = body.insts.get(*inst)
                    && let Operation::Core(CoreOp::TypeOf { descriptor, .. }) = &instruction.op {
                    descriptors.push(descriptor.clone());
                }
            }
            let substitution = Substitution { types: call.type_args.clone(), descriptors };
            if !substitutions.contains(&substitution) {
                budget.memory(128 + substitution.types.len() * 4 + substitution.descriptors.len() * 256)?;
                substitutions.push(substitution);
            }
        }
    }
    if substitutions.is_empty() {
        substitutions.push(Substitution::default());
    }
    Ok(substitutions)
}

/// Infer final summaries, cleanup projections and per-scope D18 envelopes.
///
/// Classification and inference materialize memoized High callable entries,
/// then freeze their equations. The returned module retains its input profile;
/// cleanup, paired dependency SSA and ownership transport are emitted by
/// `rewrite_synchronous`. This intermediate result is not a certificate.
/// Exhaustion returns `Incomplete` without a partial module.
pub fn elaborate_ownership(
    mut module: Module,
    natives: &dyn NativeSummaries,
    options: &OwnOptions,
) -> Result<OwnOutput, OwnError> {
    let mut budget = Budget::new(options);
    if !matches!(module.profile, Profile::High | Profile::Own) {
        return Err(budget.fail(OwnDiagnosticKind::InvalidEquation,
            "classification/summary elaboration requires High or an existing Own fixture"));
    }
    futures::intern_futures(&mut module)?;
    let mut external = BTreeMap::new();
    for (id, function) in module.functions.iter() {
        budget.work(1)?;
        futures::check_boundary(&module, id, &mut budget)?;
        if function.body.is_some() {
            continue;
        }
        let boundary = function.attrs.iter().find_map(|attr| match attr {
            FunctionAttr::Extern(name) => Some(ExternalBoundary::Native(name)),
            FunctionAttr::Intrinsic { name, .. } => Some(ExternalBoundary::Intrinsic(name)),
            _ => None,
        }).ok_or_else(|| budget.fail(OwnDiagnosticKind::MissingBoundary, function.symbol.clone()))?;
        let substitutions = external_substitutions(&module, id, &mut budget)?;
        let mut joined = None;
        for substitution in substitutions {
            let mut summary = natives.lookup(boundary, &substitution).map_err(|error| {
                budget.fail(OwnDiagnosticKind::MissingBoundary, format!("{boundary:?}: {}", error.message))
            })?.summary;
            normal::normalize(&mut summary, &mut budget)?;
            if let Some(previous) = &mut joined {
                normal::join(previous, &summary, &mut budget)?;
            } else {
                joined = Some(summary);
            }
        }
        let summary = joined.unwrap();
        budget.memory(512 + normal::size(&summary))?;
        external.insert(id, summary);
    }
    if module.profile == Profile::High {
        entries::witness_entries(&mut module, &mut budget)?;
        entries::materialize(&mut module, &mut budget)?;
    }
    let types = classify::classify(&module, options, &mut budget)?;
    let mut universe = summaries::Universe::build(&module, &types, external, natives, &mut budget)?;
    universe.solve(&module, &types, &mut budget)?;
    let mut analysis = scopes::analyze(&module, &types, &universe, &mut budget)?;
    universe.annotate(&mut module, &analysis.scopes, &mut budget)?;
    analysis.managed_references = types.iter().filter_map(|(id, facts)| facts.references.then_some(*id)).collect();
    analysis.types = types;
    Ok(OwnOutput {
        module,
        census: DynamicCensus::default(),
        stats: budget.stats,
        analysis,
    })
}

/// The `fn` values of `body` that own treats as open callees (#569 C3b): an
/// entry parameter, a produced edge result or any definition other than a
/// closure, a bound action or a witnessed role slot, also through retags and
/// block parameters; a value without any finite origin is open as well. A
/// producer attaches the open callable ABI exactly to the invocations of these
/// values; every other invocation is known to own.
pub fn open_callees(
    module: &Module,
    body: &Body,
    options: &OwnOptions,
) -> Result<std::collections::BTreeSet<ValueId>, OwnError> {
    let mut budget = Budget::new(options);
    let origins = origins::callable_origins(module, body, &mut budget)?;
    Ok(origins.into_iter().filter_map(|(value, origin)| {
        (origin.open || origin.entries.is_empty()).then_some(value)
    }).collect())
}

#[cfg(test)]
mod tests;

/// The module entry and the test functions are roots the runtime calls with no
/// arguments and no packet: their ownership ABI stays an observation, and every
/// obligation of their root scope is cleaned up before they return (spec §14).
fn fixed_root(module: &Module, function: FuncId) -> bool {
    module.entry == Some(function)
        || module.tests.as_ref().is_some_and(|tests| tests.iter().any(|test| test.function == function))
}

fn flow_error(message: impl Into<String>) -> OwnError {
    phase_error(OwnDiagnosticKind::InvalidFlow, message)
}

fn unsupported(operation: &Operation) -> OwnError {
    let (kind, phase) = match operation {
        Operation::Async(op) => (OwnDiagnosticKind::UnsupportedAsync, async_phase(op)),
        Operation::Carrier(_) => (OwnDiagnosticKind::UnsupportedConstruct,
            "owned failure carriers are excluded from 0.6.0"),
        _ => (OwnDiagnosticKind::UnsupportedConstruct, "outside the synchronous P1 producer"),
    };
    phase_error(kind, format!("{phase}: {}", operation.opcode()))
}

/// The slice of the P2.5 plan that lifts an async operation beyond plain async.
fn async_phase(op: &AsyncOp) -> &'static str {
    match op {
        AsyncOp::FutureSpawn { .. } => "own forwards a state only to its own fut.spawn (D24.1)",
        AsyncOp::WorkerBind { .. } | AsyncOp::WorkerSpawn { .. } | AsyncOp::ProcessSpawn { .. } => {
            "process workers arrive with X1"
        }
        AsyncOp::Pin { .. } | AsyncOp::PinTransfer { .. } | AsyncOp::Unpin { .. } => "pins arrive with D23 (Q2)",
        AsyncOp::AwaitTake { .. } | AsyncOp::UniqueCacheProof { .. } | AsyncOp::CacheRehome { .. } => {
            "unique take and rehome are own decisions of R1, never High input (C1)"
        }
        _ => "not a High operation; own and coro produce it",
    }
}

fn phase_error(kind: OwnDiagnosticKind, message: impl Into<String>) -> OwnError {
    OwnError::Diagnostics {
        diagnostics: vec![OwnDiagnostic { kind, function: None, site: None, message: message.into() }],
        stats: Box::default(),
    }
}
