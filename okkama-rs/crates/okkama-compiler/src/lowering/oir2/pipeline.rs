//! Source-facing experimental pipeline. Builds use structural gates; tests can request full ownership gates.
use super::{LowerError, lower_to_oir2, native_summaries::P1NativeSummaries};
use crate::analysis;
use crate::codegen::sparrow_oir2;
use crate::lowering::{ModuleLoader, RootFile, modules};
use crate::oir2 as o;
use okkama_sparrow::spb17 as s;
use std::collections::BTreeMap;

#[derive(Debug)]
pub enum PipelineError {
    Lower(LowerError),
    Phase { phase: &'static str, message: String },
}

impl PipelineError {
    pub fn is_unsupported(&self) -> bool {
        matches!(self, Self::Lower(LowerError::Unsupported { .. }))
    }
}

impl std::fmt::Display for PipelineError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Lower(error) => error.fmt(out),
            Self::Phase { phase, message } => write!(out, "OIR-2 {phase}: {message}"),
        }
    }
}

impl std::error::Error for PipelineError {}

fn phase(phase: &'static str, error: impl std::fmt::Debug) -> PipelineError {
    PipelineError::Phase { phase, message: format!("{error:?}") }
}

/// Resolve imports and analyze directly, without invoking any OIR-1 lowering.
pub fn lower_linked_high(roots: &[RootFile<'_>], loader: &mut dyn ModuleLoader)
    -> Result<o::Module, PipelineError>
{
    if roots.is_empty() {
        return Err(phase("analysis", "no source files"));
    }
    let roots: Vec<_> = roots.iter().map(|root| {
        (root.program, root.source.name(), Some(root.source))
    }).collect();
    let graph = modules::load_graph(&roots, loader).map_err(|error| phase("imports", error))?;
    let sources = modules::source_modules(&graph, &roots);
    let analysis = analysis::analyze(sources).map_err(|error| phase("analysis", error))?;
    lower_to_oir2(&analysis).map_err(PipelineError::Lower)
}

pub fn compile_linked(roots: &[RootFile<'_>], loader: &mut dyn ModuleLoader)
    -> Result<Vec<u8>, PipelineError>
{
    compile_linked_with(roots, loader, o::VerifyMode::Structural)
}

/// Select verification explicitly; environment and command-line policy belong to the CLI.
pub fn compile_linked_with(roots: &[RootFile<'_>], loader: &mut dyn ModuleLoader, mode: o::VerifyMode)
    -> Result<Vec<u8>, PipelineError>
{
    let high = lower_linked_high(roots, loader)?;
    compile_high_with(high, mode)
}

/// The phases after the producer: verify_high, own, verify_own, low, verify_low, SPB 17 and its load check.
pub(super) fn compile_high_with(high: o::Module, mode: o::VerifyMode) -> Result<Vec<u8>, PipelineError> {
    let verify = o::VerifyOptions { mode, ..o::VerifyOptions::default() };
    let catalog = P1NativeSummaries::for_module(&high).map_err(|error| phase("contracts", error))?;
    let context = || o::VerifyContext { external_contracts: &catalog, ..o::VerifyContext::default() };
    o::verify_high_with(&high, &verify, context())
        .map_err(|error| phase("verify_high", error))?;
    let options = o::own::OwnOptions { verify, ..o::own::OwnOptions::default() };
    let inferred = o::own::elaborate_ownership(high, &catalog, &options)
        .map_err(|error| phase("own_elaborate", error))?;
    let own = o::own::rewrite_subset(inferred, &options)
        .map_err(|error| phase("own_rewrite", error))?;
    let catalog = P1NativeSummaries::for_module(&own.module).map_err(|error| phase("contracts_own", error))?;
    let context = || o::VerifyContext { external_contracts: &catalog, ..o::VerifyContext::default() };
    o::verify_own_with(&own.module, &options.verify, context()).map_err(|error| phase("verify_own", error))?;
    let low = o::low::lower_cleanup(own.module, &o::low::LowOptions::default())
        .map_err(|error| phase("low", error))?;
    o::verify_low_with(&low, &options.verify, context()).map_err(|error| phase("verify_low", error))?;
    let layouts = layouts(&low)?;
    let objects = sparrow_oir2::objects(&low, &layouts).map_err(|error| phase("layout", error))?;
    let contracts = s::p1::P1Contracts::new(&objects).map_err(|error| phase("spb_contracts", error))?;
    let output = sparrow_oir2::compile_low_with(&low, &layouts, context(), &contracts, &options.verify)
        .map_err(|error| phase("compile_low", error))?;
    s::verify_load(&output.program, &contracts).map_err(|error| phase("verify_load", error))?;
    s::encode(&output.program).map_err(|error| phase("encode", error))
}

fn layouts(module: &o::Module) -> Result<BTreeMap<u32, sparrow_oir2::ObjectLayout>, PipelineError> {
    let mut layouts = BTreeMap::new();
    for (id, declaration) in module.declarations.iter() {
        let (fields, destroy, builtin) = match &declaration.kind {
            o::DeclarationKind::Object { fields, destroy, builtin, .. } => (fields.as_slice(), *destroy, *builtin),
            o::DeclarationKind::Enum { .. } | o::DeclarationKind::Variant { .. } => (&[][..], None, false),
            o::DeclarationKind::Role { builtin: true, .. } if declaration.name == "Error" => (&[][..], None, false),
            _ => return Err(phase("layout", format!("unsupported declaration {}", declaration.name))),
        };
        let atomic = match declaration.name.as_str() {
            "async.atomic.Atomic_int" if builtin && fields.is_empty() && destroy.is_none() => Some(s::Type::Int),
            "async.atomic.Atomic_string" if builtin && fields.is_empty() && destroy.is_none() => Some(s::Type::Str),
            _ => None,
        };
        if (builtin && atomic.is_none()) || fields.iter().any(|field| field.once) {
            return Err(phase("layout", format!("missing final runtime override for {}", declaration.name)));
        }
        let mut storage_fields = fields.iter().map(|field| sparrow_oir2::translate_type(module, field.ty))
            .collect::<Result<Vec<_>, _>>().map_err(|error| phase("layout", error))?;
        if let Some(value) = atomic {
            storage_fields = vec![value, s::Type::Bool];
        } else if matches!(declaration.kind, o::DeclarationKind::Enum { .. }) {
            storage_fields = vec![s::Type::Int];
        }
        let trace_layout = storage_fields.iter().enumerate().filter_map(|(index, ty)| {
            matches!(ty, s::Type::Bytes | s::Type::Str | s::Type::Object(_) | s::Type::Successor(_)
                | s::Type::Failure).then_some(index as u32)
        }).collect::<Vec<_>>();
        // Plain layouts can contain resource fields; compute the transitive fact below.
        layouts.insert(id.as_u32(), sparrow_oir2::ObjectLayout {
            runtime: s::ObjectRuntimeLayout {
                allocation_words: (1 + storage_fields.len() as u32 + 4 * u32::from(destroy.is_some())).max(2),
                identity_offset: destroy.map(|_| 8 * (1 + fields.len() as u32)),
                has_destroy: destroy.is_some(),
                may_reach_resource: destroy.is_some(),
                may_async_destroy: false,
                storage_fields,
                trace_layout,
                selection_layout: vec![],
            },
            source_fields: (0..fields.len() as u32).collect(),
        });
    }
    loop {
        let reaches = layouts.iter().filter_map(|(&id, layout)| layout.runtime.may_reach_resource.then_some(id))
            .collect::<std::collections::BTreeSet<_>>();
        let mut changed = false;
        for layout in layouts.values_mut() {
            layout.runtime.selection_layout = layout.runtime.storage_fields.iter().enumerate().filter_map(|(i, ty)| {
                matches!(ty, s::Type::Object(id) | s::Type::Successor(id) if reaches.contains(id)).then_some(i as u32)
            }).collect();
            if !layout.runtime.may_reach_resource && !layout.runtime.selection_layout.is_empty() {
                layout.runtime.may_reach_resource = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    Ok(layouts)
}
