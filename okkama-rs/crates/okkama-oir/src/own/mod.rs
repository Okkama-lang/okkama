//! Ownership elaboration, independent of the verifier's implementation.
//!
//! Classification and summary inference preserve the input profile. Commit 2
//! adds a finite forward domain, sealed SSA, lexical edges and an outcome emitter.
//! `rewrite_synchronous` normalizes the synchronous High slice and independently
//! verifies its output. `rewrite_subset` exposes the raw rewrite for diagnostic
//! fixtures, including witnesses blocked by the independent verifier. D8/D17/D18
//! semantic certification still requires the #564 commit-4 gate.
pub mod domain;
pub mod analyze;
pub mod bindings;
pub mod edges;
pub mod emit;
mod driver;
pub use driver::{rewrite_subset, rewrite_synchronous};
mod classify;
mod normal;
mod summaries;
mod scopes;
mod inventory;
mod construction;

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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OwnStats {
    pub active_function: Option<FuncId>,
    pub exhausted_limit: Option<OwnLimit>,
    pub functions: usize,
    pub emitted_checked_splits: usize,
    pub universe_nodes: usize,
    pub adapter_equations: usize,
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
    Commit4,
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
    /// Final projections for bound cleanup graphs. Unbound, unowned service
    /// types have no entry; absence never means an empty cleanup effect.
    pub cleanup_effects: BTreeMap<TypeId, ScopeEffectEnvelope>,
    pub scopes: BTreeMap<ScopeId, ScopeFacts>,
}

#[derive(Debug)]
pub struct OwnOutput {
    pub module: Module,
    /// Emitted sites only; commit 1 emits no dynamic operations.
    pub census: DynamicCensus,
    pub stats: OwnStats,
    pub analysis: OwnAnalysis,
}

#[derive(Debug)]
pub(super) struct Budget<'a> {
    options: &'a OwnOptions,
    pub stats: OwnStats,
}

impl<'a> Budget<'a> {
    fn new(options: &'a OwnOptions) -> Self {
        Self { options, stats: OwnStats::default() }
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
        let next = self.stats.peak_auxiliary_bytes.checked_add(bytes);
        if next.is_none_or(|next| next > self.options.memory_limit) {
            let mut stats = self.stats.clone();
            stats.exhausted_limit = Some(OwnLimit::Memory);
            return Err(OwnError::Incomplete { stats: Box::new(stats) });
        }
        self.stats.peak_auxiliary_bytes = next.unwrap();
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
/// This commit runs classification and inference only: the returned module
/// keeps its input profile and ownership ABI. No cleanup, pending reservation,
/// adapter body or paired dependency SSA is fabricated. `analysis.scopes`
/// supplies eligibility facts for the later rewrite, not an elision claim.
/// The caller verifies input and annotated output independently. Narrowing an
/// existing Own fixture may require a later CFG rewrite; inference does not edit
/// outcome edges or certify the returned module.
/// Exhaustion returns `Incomplete`, with no certified or partial module.
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
    let mut external = BTreeMap::new();
    for (id, function) in module.functions.iter() {
        budget.work(1)?;
        if function.attrs.contains(&FunctionAttr::Async) {
            return Err(budget.fail(OwnDiagnosticKind::UnsupportedAsync, format!("async entry {}", function.symbol)));
        }
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
    let types = classify::classify(&module, options, &mut budget)?;
    let mut universe = summaries::Universe::build(&module, &types, external, natives, &mut budget)?;
    universe.solve(&module, &types, &mut budget)?;
    let mut analysis = scopes::analyze(&module, &types, &universe, &mut budget)?;
    universe.annotate(&mut module, &analysis.scopes, &mut budget)?;
    analysis.types = types;
    Ok(OwnOutput {
        module,
        census: DynamicCensus::default(),
        stats: budget.stats,
        analysis,
    })
}

#[cfg(test)]
mod tests;

fn flow_error(message: impl Into<String>) -> OwnError {
    phase_error(OwnDiagnosticKind::InvalidFlow, message)
}

fn unsupported(operation: &Operation) -> OwnError {
    let (kind, phase) = match operation {
        Operation::Async(_) => (OwnDiagnosticKind::UnsupportedAsync, "commit 5"),
        Operation::Core(CoreOp::FnClosure { .. } | CoreOp::FnBind { .. } | CoreOp::FnBindRole { .. }
            | CoreOp::CapMake { .. } | CoreOp::AnyBox { .. })
        | Operation::Call(CallOp::Closure { .. } | CallOp::Cap { .. } | CallOp::Role { .. })
        | Operation::Carrier(_) => (OwnDiagnosticKind::Commit4, "commit 4"),
        _ => (OwnDiagnosticKind::UnsupportedConstruct, "outside the synchronous P1 producer"),
    };
    phase_error(kind, format!("{phase}: {}", operation.opcode()))
}

fn phase_error(kind: OwnDiagnosticKind, message: impl Into<String>) -> OwnError {
    OwnError::Diagnostics {
        diagnostics: vec![OwnDiagnostic { kind, function: None, site: None, message: message.into() }],
        stats: Box::default(),
    }
}
