//! Source-facing experimental pipeline. Every phase has an independent semantic gate.
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
    let high = lower_linked_high(roots, loader)?;
    let catalog = P1NativeSummaries::for_module(&high).map_err(|error| phase("contracts", error))?;
    let context = || o::VerifyContext { external_contracts: &catalog, ..o::VerifyContext::default() };
    o::verify_with(&high, &o::VerifyOptions::default(), context())
        .map_err(|error| phase("verify_high", error))?;
    let options = o::own::OwnOptions::default();
    let own = o::own::elaborate_ownership(high, &catalog, &options)
        .and_then(|output| o::own::rewrite_subset(output, &options))
        .map_err(|error| phase("own", error))?;
    o::verify_with(&own.module, &options.verify, context()).map_err(|error| phase("verify_own", error))?;
    let low = o::low::lower_cleanup(own.module, &o::low::LowOptions::default())
        .map_err(|error| phase("low", error))?;
    o::verify_with(&low, &options.verify, context()).map_err(|error| phase("verify_low", error))?;
    let layouts = layouts(&low)?;
    let output = sparrow_oir2::compile_low(&low, &layouts, context(), &s::NoExternalContracts)
        .map_err(|error| phase("compile_low", error))?;
    s::verify_load(&output.program, &s::NoExternalContracts).map_err(|error| phase("verify_load", error))?;
    s::encode(&output.program).map_err(|error| phase("encode", error))
}

fn layouts(module: &o::Module) -> Result<BTreeMap<u32, sparrow_oir2::ObjectLayout>, PipelineError> {
    let mut layouts = BTreeMap::new();
    for (id, declaration) in module.declarations.iter() {
        let o::DeclarationKind::Object { fields, destroy, builtin, .. } = &declaration.kind else {
            return Err(phase("layout", format!("unsupported declaration {}", declaration.name)));
        };
        if *builtin || fields.iter().any(|field| field.once) {
            return Err(phase("layout", format!("missing final runtime override for {}", declaration.name)));
        }
        let storage_fields = fields.iter().map(|field| sparrow_oir2::translate_type(module, field.ty))
            .collect::<Result<Vec<_>, _>>().map_err(|error| phase("layout", error))?;
        let trace_layout = storage_fields.iter().enumerate().filter_map(|(index, ty)| {
            matches!(ty, s::Type::Int | s::Type::Str | s::Type::Object(_) | s::Type::Successor(_)
                | s::Type::Range | s::Type::Failure).then_some(index as u32)
        }).collect::<Vec<_>>();
        // Plain layouts can contain resource fields; compute the transitive fact below.
        layouts.insert(id.as_u32(), sparrow_oir2::ObjectLayout {
            runtime: s::ObjectRuntimeLayout {
                allocation_words: (1 + fields.len() as u32 + 4 * u32::from(destroy.is_some())).max(2),
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
