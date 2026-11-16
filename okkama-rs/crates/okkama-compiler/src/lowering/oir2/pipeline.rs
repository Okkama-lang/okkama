//! Source-facing experimental pipeline. Builds use structural gates; tests can request full ownership gates.
use super::{LowerError, lower_to_oir2, native_summaries::P1NativeSummaries};
use crate::analysis;
use crate::codegen::sparrow_oir2;
use crate::lowering::{ModuleLoader, RootFile, modules};
use crate::oir2 as o;
use okkama_sparrow::spb17 as s;
use std::collections::BTreeMap;
use std::time::Instant;

/// Per-stage wall times on stderr under `OKKAMA_PROFILE_PIPELINE=1` (#701): one
/// `pipeline <stage> <ms>` line per stage. Diagnostics only; keep it out of timed samples.
struct Stages {
    last: Option<Instant>,
}

impl Stages {
    fn start() -> Self {
        Self::enabled(std::env::var("OKKAMA_PROFILE_PIPELINE").is_ok_and(|value| !value.is_empty() && value != "0"))
    }

    fn enabled(on: bool) -> Self {
        Self { last: on.then(Instant::now) }
    }

    fn mark(&mut self, stage: &str) {
        if let Some(line) = self.line(stage) {
            eprintln!("{line}");
        }
    }

    /// The report line of the stage that ends now, measured from the previous mark; none when disabled.
    fn line(&mut self, stage: &str) -> Option<String> {
        let last = self.last?;
        let now = Instant::now();
        self.last = Some(now);
        Some(format!("pipeline {stage} {:.1}", (now - last).as_secs_f64() * 1000.0))
    }
}

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
    let mut stages = Stages::start();
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
    stages.mark("imports");
    let sources = modules::source_modules(&graph, &roots);
    let analysis = analysis::analyze_mode(sources, roots.len(), test_mode)
        .map_err(|error| phase("analysis", error))?;
    stages.mark("analysis");
    // Async execution is #570: an async test stops the whole test profile before lowering, by name.
    let asynchronous = analysis.registry.tests.iter().filter(|test| test.is_async).collect::<Vec<_>>();
    if let Some(first) = asynchronous.first() {
        let names = asynchronous.iter().map(|test| format!("{:?}", test.name)).collect::<Vec<_>>().join(", ");
        let message = format!("async test {names} (#570)");
        return Err(PipelineError::Lower(super::LowerError::unsupported(first.span, message)));
    }
    let high = lower_to_oir2(&analysis).map_err(PipelineError::Lower);
    stages.mark("high");
    high
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

/// The phases after the producer: verify_high, own, verify_own, coro, verify_coro, low, verify_low, SPB 17 and
/// its load check (inside `compile_low_with`).
pub(super) fn compile_high_with(high: o::Module, mode: o::VerifyMode) -> Result<Vec<u8>, PipelineError> {
    let execution = if high.tests.is_some() {
        o::verify::ExecutionMode::Tests
    } else {
        o::verify::ExecutionMode::Production
    };
    // The Full verifier is the gate profile: its work limit is the gate's (#693, owner decision O3).
    let limits = if mode == o::VerifyMode::Full { o::verify::VerifyLimits::gate() } else { Default::default() };
    let verify = o::VerifyOptions { mode, execution, limits, ..o::VerifyOptions::default() };
    let mut stages = Stages::start();
    let catalog = P1NativeSummaries::for_module(&high).map_err(|error| phase("contracts", error))?;
    let context = || o::VerifyContext { external_contracts: &catalog, ..o::VerifyContext::default() };
    o::verify_high_with(&high, &verify, context())
        .map_err(|error| phase("verify_high", error))?;
    stages.mark("verify_high");
    let options = o::own::OwnOptions { verify, ..o::own::OwnOptions::default() };
    let inferred = o::own::elaborate_ownership(high, &catalog, &options)
        .map_err(|error| phase("own_elaborate", error))?;
    let own = o::own::rewrite_subset(inferred, &options)
        .map_err(|error| phase("own_rewrite", error))?;
    stages.mark("own");
    let catalog = P1NativeSummaries::for_module(&own.module).map_err(|error| phase("contracts_own", error))?;
    let context = || o::VerifyContext { external_contracts: &catalog, ..o::VerifyContext::default() };
    o::verify_own_with(&own.module, &options.verify, context()).map_err(|error| phase("verify_own", error))?;
    // OIR-2 §12: coro gives async bodies their frames and suspension brackets; a synchronous module keeps its
    // bodies. Its verifier follows the build's mode, like the others (structural in builds, full in the gate).
    stages.mark("verify_own");
    let coro = o::coro::lower_coro(own.module, &o::coro::CoroOptions::default())
        .map_err(|error| phase("coro", error))?;
    stages.mark("coro");
    o::verify_coro_with(&coro, &options.verify, context()).map_err(|error| phase("verify_coro", error))?;
    stages.mark("verify_coro");
    let low = o::low::lower_cleanup(coro, &o::low::LowOptions::default())
        .map_err(|error| phase("low", error))?;
    stages.mark("low");
    o::verify_low_with(&low, &options.verify, context()).map_err(|error| phase("verify_low", error))?;
    stages.mark("verify_low");
    let layouts = sparrow_oir2::with_composites(&low, || layouts(&low))?;
    let objects = sparrow_oir2::with_composites(&low, || sparrow_oir2::objects(&low, &layouts))
        .map_err(|error| phase("layout", error))?;
    let contracts = s::p1::P1Contracts::new(&objects).map_err(|error| phase("spb_contracts", error))?;
    let output = sparrow_oir2::compile_low_with(&low, &layouts, context(), &contracts, &options.verify)
        .map_err(|error| phase("compile_low", error))?;
    // `compile_low_with` has already passed this program through `verify_load` against these contracts;
    // a second identical check of the same program cannot fail and costs as much again (#701).
    stages.mark("codegen+verify_load");
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
            // A module constant lives in the SPB 17 globals table (#709); its declaration is a placeholder.
            o::DeclarationKind::Global { .. } => (&[][..], None, false),
        };
        let atomic = declaration.name.strip_prefix("async.atomic.Atomic_")
            .filter(|_| builtin && fields.is_empty() && destroy.is_none())
            .and_then(s::p1::atomic_value);
        // A once slot is an ordinary Maybe field: the producer stores Maybe.none at construction and
        // once.load/once.store read and fill it (SPB 17 `Field::once` keeps it out of equality and keys).
        // The Event cell of the std async primitives (#601 Q1): `[initial, set, waiters]`, as Tomato keeps it.
        let event = builtin && destroy.is_none() && declaration.name == s::drivers::EVENT
            && matches!(fields, [field] if field.name == "initial" && !field.once);
        if builtin && atomic.is_none() && !event {
            return Err(phase("layout", format!("missing final runtime override for {}", declaration.name)));
        }
        let mut storage_fields = fields.iter().map(|field| sparrow_oir2::translate_type(module, field.ty))
            .collect::<Result<Vec<_>, _>>().map_err(|error| phase("layout", error))?;
        if let Some(value) = atomic {
            storage_fields = vec![value, s::Type::Bool];
        } else if event {
            storage_fields = s::drivers::EVENT_STORAGE.to_vec();
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
                    // A role or `any` field, or a managed collection of them, reaches what one of the role's
                    // implementations or one of the any boxes reaches (#711).
                    Some(s::Type::Role(id) | s::Type::RoleSuccessor(id)) => role_reaches(&id),
                    Some(s::Type::Any) => any_reaches,
                    // A Future field holds a service entry that travels with the object's packet (D25.1, #771): the
                    // selection walk reaches its holder through the field.
                    Some(s::Type::Future(_)) => true,
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

#[cfg(test)]
#[path = "pipeline_tests.rs"]
mod tests;

#[cfg(test)]
mod stage_tests {
    use super::Stages;

    #[test]
    fn disabled_stages_report_nothing() {
        let mut stages = Stages::enabled(false);
        assert_eq!(stages.line("own"), None);
    }

    #[test]
    fn enabled_stages_report_each_stage_once_in_milliseconds() {
        let mut stages = Stages::enabled(true);
        std::thread::sleep(std::time::Duration::from_millis(5));
        let line = stages.line("own").unwrap();
        let fields = line.split(' ').collect::<Vec<_>>();
        assert_eq!(fields[..2], ["pipeline", "own"]);
        assert!(fields[2].parse::<f64>().unwrap() >= 5.0, "{line}");
        let next = stages.line("low").unwrap();
        assert!(next.starts_with("pipeline low "));
        assert!(next.split(' ').nth(2).unwrap().parse::<f64>().is_ok(), "{next}");
    }
}
