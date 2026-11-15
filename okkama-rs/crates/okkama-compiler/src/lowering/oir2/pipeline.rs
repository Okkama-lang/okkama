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
    lower_linked_mode(roots, loader, false)
}

/// The test profile (§14): roots carry their desugared tests and fakes, and the loader
/// parses imports in test mode. The module selects its tests table instead of `main`.
pub fn lower_linked_tests_high(roots: &[RootFile<'_>], loader: &mut dyn ModuleLoader)
    -> Result<o::Module, PipelineError>
{
    lower_linked_mode(roots, loader, true)
}

fn lower_linked_mode(roots: &[RootFile<'_>], loader: &mut dyn ModuleLoader, test_mode: bool)
    -> Result<o::Module, PipelineError>
{
    if roots.is_empty() {
        return Err(phase("analysis", "no source files"));
    }
    let roots: Vec<_> = roots.iter().map(|root| {
        (root.program, root.source.name(), Some(root.source))
    }).collect();
    // As for OIR-1: `std.…` paths of the root become hidden imports, and every root file
    // belongs to the one root module (§5.1).
    let programs = crate::lowering::desugared(&roots).map_err(|error| phase("imports", error))?;
    let roots: Vec<_> = programs.iter().zip(&roots).map(|(program, (_, name, source))| {
        (program, *name, *source)
    }).collect();
    let graph = modules::load_graph(&roots, loader).map_err(|error| phase("imports", error))?;
    let sources = modules::source_modules(&graph, &roots);
    let analysis = analysis::analyze_mode(sources, roots.len(), test_mode)
        .map_err(|error| phase("analysis", error))?;
    // Async execution is #570: an async test stops the whole test profile before lowering, by name.
    let asynchronous = analysis.registry.tests.iter().filter(|test| test.is_async).collect::<Vec<_>>();
    if let Some(first) = asynchronous.first() {
        let names = asynchronous.iter().map(|test| format!("{:?}", test.name)).collect::<Vec<_>>().join(", ");
        let message = format!("async test {names} (#570)");
        return Err(PipelineError::Lower(super::LowerError::unsupported(first.span, message)));
    }
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

/// [`compile_linked_with`] for the test profile: SPB 17 with its tests table.
pub fn compile_linked_tests_with(roots: &[RootFile<'_>], loader: &mut dyn ModuleLoader, mode: o::VerifyMode)
    -> Result<Vec<u8>, PipelineError>
{
    let high = lower_linked_tests_high(roots, loader)?;
    compile_high_with(high, mode)
}

/// The phases after the producer: verify_high, own, verify_own, low, verify_low, SPB 17 and its load check.
pub(super) fn compile_high_with(high: o::Module, mode: o::VerifyMode) -> Result<Vec<u8>, PipelineError> {
    let execution = if high.tests.is_some() {
        o::verify::ExecutionMode::Tests
    } else {
        o::verify::ExecutionMode::Production
    };
    let verify = o::VerifyOptions { mode, execution, ..o::VerifyOptions::default() };
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
    let layouts = sparrow_oir2::with_composites(&low, || layouts(&low))?;
    let objects = sparrow_oir2::with_composites(&low, || sparrow_oir2::objects(&low, &layouts))
        .map_err(|error| phase("layout", error))?;
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
            // A role is a placeholder descriptor; its slots and implementations live in the SPB declarations.
            // A cap value is its owner's reference, so its declaration is a placeholder too.
            o::DeclarationKind::Role { .. } | o::DeclarationKind::Cap { .. } => (&[][..], None, false),
            o::DeclarationKind::Handle => (&[][..], None, false),
            _ => return Err(phase("layout", format!("unsupported declaration {}", declaration.name))),
        };
        let atomic = declaration.name.strip_prefix("async.atomic.Atomic_")
            .filter(|_| builtin && fields.is_empty() && destroy.is_none())
            .and_then(s::p1::atomic_value);
        // A once slot is an ordinary Maybe field: the producer stores Maybe.none at construction and
        // once.load/once.store read and fill it (SPB 17 `Field::once` keeps it out of equality and keys).
        if builtin && atomic.is_none() {
            return Err(phase("layout", format!("missing final runtime override for {}", declaration.name)));
        }
        let mut storage_fields = fields.iter().map(|field| sparrow_oir2::translate_type(module, field.ty))
            .collect::<Result<Vec<_>, _>>().map_err(|error| phase("layout", error))?;
        if let Some(value) = atomic {
            storage_fields = vec![value, s::Type::Bool];
        } else if matches!(declaration.kind, o::DeclarationKind::Enum { .. }) {
            storage_fields = vec![s::Type::Int];
        }
        let trace_layout = storage_fields.iter().enumerate()
            .filter_map(|(index, ty)| ty.traced().then_some(index as u32)).collect::<Vec<_>>();
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
    // A role value reaches a resource when one of its implementations does.
    let mut implementors = BTreeMap::<u32, Vec<u32>>::new();
    for (id, declaration) in module.declarations.iter() {
        let o::DeclarationKind::Object { witnesses, .. } = &declaration.kind else { continue };
        for witness in witnesses {
            let role = module.types.get(witness.role).map_err(|error| phase("layout", error))?;
            if let o::Type::Role { decl, .. } = role {
                implementors.entry(decl.as_u32()).or_default().push(id.as_u32());
            }
        }
    }
    // An `any` value reaches what one of its boxed values reaches (`any.box` sites).
    let mut boxed = Vec::new();
    for (_, function) in module.functions.iter() {
        let Some(body) = &function.body else { continue };
        for (_, instruction) in body.insts.iter() {
            if let o::Operation::Core(o::CoreOp::AnyBox { value, .. }) = &instruction.op
                && let Ok(ty) = sparrow_oir2::translate_type(module, body.values.get(*value)
                    .map_err(|error| phase("layout", error))?.ty)
            {
                boxed.push(ty);
            }
        }
    }
    loop {
        let reaches = layouts.iter().filter_map(|(&id, layout)| layout.runtime.may_reach_resource.then_some(id))
            .collect::<std::collections::BTreeSet<_>>();
        let role_reaches = |role: &u32| implementors.get(role).is_some_and(|objects| {
            objects.iter().any(|object| reaches.contains(object))
        });
        let any_reaches = boxed.iter().any(|ty| match *ty {
            s::Type::Object(id) | s::Type::Successor(id) | s::Type::Variant(id) => reaches.contains(&id),
            s::Type::Role(id) | s::Type::RoleSuccessor(id) => role_reaches(&id),
            _ => false,
        });
        let mut changed = false;
        for layout in layouts.values_mut() {
            // A field holding a resource object, or a managed array of them, is selected: the runtime
            // selection walk reaches the identities through it (ARRAY_RETENTION).
            layout.runtime.selection_layout = layout.runtime.storage_fields.iter().enumerate().filter_map(|(i, ty)| {
                let leaf = match *ty {
                    s::Type::Array(shape) => s::ArrayType { depth: 1, ..shape }.element(),
                    s::Type::Map(id) => match sparrow_oir2::composites(module).entries().get(id as usize) {
                        Some(s::Composite::Map { value, .. }) => Some(*value),
                        _ => None,
                    },
                    ty => Some(ty),
                };
                let reaching = match leaf {
                    Some(s::Type::Object(id) | s::Type::Successor(id) | s::Type::Variant(id)) => reaches.contains(&id),
                    // A role field reaches what one of its implementations reaches; role arrays are closed plain.
                    Some(s::Type::Role(id) | s::Type::RoleSuccessor(id)) if !matches!(ty, s::Type::Array(_)) => {
                        role_reaches(&id)
                    }
                    Some(s::Type::Any) if !matches!(ty, s::Type::Array(_)) => any_reaches,
                    _ => false,
                };
                reaching.then_some(i as u32)
            }).collect();
            if !layout.runtime.may_reach_resource && !layout.runtime.selection_layout.is_empty() {
                layout.runtime.may_reach_resource = true;
                changed = true;
            }
        }
        // A variant reaches resources when one of its cases does (VARIANT_CASES).
        for (id, declaration) in module.declarations.iter() {
            let o::DeclarationKind::Variant { cases, .. } = &declaration.kind else { continue };
            let reaches_case = cases.iter().any(|case| reaches.contains(&case.layout.as_u32()));
            if let Some(layout) = layouts.get_mut(&id.as_u32())
                && reaches_case && !layout.runtime.may_reach_resource
            {
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
