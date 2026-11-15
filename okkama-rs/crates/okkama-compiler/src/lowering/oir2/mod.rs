//! Developer-only THIR -> OIR-2 high producer for #566 and #567 S1–S3.
//!
//! This module is independent of OIR-1. It never enters the OIR-1
//! pipeline. Every reachable imported ordinary action has a lowered source body;
//! application declarations are checked in full. Only registered boundaries are external.
//! Operations outside the synchronous slice produce source diagnostics. D11 result exits
//! are centralized in `BodyLowerer` and call envelopes are emitted at their instruction sites.

mod body;
mod collections;
mod builders;
#[cfg(test)]
mod collection_tests;
#[cfg(test)]
mod builder_tests;
#[cfg(test)]
mod iteration_tests;
#[cfg(test)]
mod functional_tests;
mod specialize;
mod callables;
mod anonymous;
mod bound;
mod caps;
mod known_calls;
#[cfg(test)]
mod open_calls;
mod test_profile;
#[cfg(test)]
mod test_profile_tests;
#[cfg(test)]
mod callable_tests;
mod native_summaries;
#[cfg(test)]
mod native_catalogue_tests;
mod pipeline;
pub use pipeline::{PipelineError, compile_linked, compile_linked_with, lower_linked_high};
mod construction;
mod declarations;
mod defer;
mod high;
mod failures;
mod objects;
mod once;
mod sorting;
mod builtin_roles;
#[cfg(test)]
mod std_collection_tests;
mod roles;
mod walk;
mod scalar;
mod operators;
mod globals;
mod streams;
mod structure;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod specialization_tests;
#[cfg(test)]
mod role_tests;
#[cfg(test)]
mod case_tests;

use crate::analysis::{Analysis, thir};
use crate::oir2 as ir;
use crate::source::Span;

#[derive(Debug)]
pub enum LowerError {
    Unsupported { span: Span, message: String },
    Build(ir::BuildError),
    InvalidThir { span: Span, message: String },
}

impl LowerError {
    fn unsupported(span: Span, construct: impl std::fmt::Display) -> Self {
        Self::Unsupported {
            span,
            message: format!("unsupported in OIR-2 lowering: {construct}"),
        }
    }

    fn in_function(self, symbol: &str) -> Self {
        match self {
            Self::Unsupported { span, message } => Self::Unsupported {
                span, message: format!("{message} in function {symbol}"),
            },
            error => error,
        }
    }

    fn invalid(span: Span, message: impl Into<String>) -> Self {
        Self::InvalidThir { span, message: message.into() }
    }
}

impl From<ir::BuildError> for LowerError {
    fn from(error: ir::BuildError) -> Self {
        Self::Build(error)
    }
}

impl From<ir::ArenaError> for LowerError {
    fn from(error: ir::ArenaError) -> Self {
        Self::Build(ir::BuildError::Arena(error))
    }
}

impl std::fmt::Display for LowerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported { span, message } | Self::InvalidThir { span, message } => {
                write!(f, "{span}: {message}")
            }
            Self::Build(error) => write!(f, "OIR-2 construction: {error}"),
        }
    }
}

impl std::error::Error for LowerError {}

type Result<T> = std::result::Result<T, LowerError>;

/// Independent developer entry: analysis supplies resolved THIR and declarations.
/// No CLI routing, ownership elaboration or semantic verifier is invoked here.
pub fn lower_to_oir2(analysis: &Analysis<'_>) -> Result<ir::Module> {
    lower(analysis, false)
}

/// #569 O1 evidence only: open invocations of `fn` parameters receive the
/// registered open sync callable ABI instead of the C3a diagnostic. C3b, after
/// the audit, integrates open invocations into the producer itself.
#[cfg(test)]
pub(crate) fn lower_to_oir2_with_open_calls(analysis: &Analysis<'_>) -> Result<ir::Module> {
    lower(analysis, true)
}

fn lower(analysis: &Analysis<'_>, open_calls: bool) -> Result<ir::Module> {
    let prepared = specialize::prepare(analysis)?;
    let constants = &analysis.registry.const_state;
    let constant_names: Vec<_> = analysis.registry.consts.iter().map(|constant| {
        let module = &analysis.registry.modules[constant.module].name;
        format!("{}.const.{}", module.replace('/', "."), constant.name)
    }).collect();
    let analysis = &prepared.analysis;
    let mut declarations = declarations::Declarations::new(analysis)?;
    declarations.declare_lambdas(&prepared.callables)?;
    let walked: Vec<_> = analysis.bodies.iter()
        .chain(prepared.callables.bodies.iter().map(|discovered| &discovered.body)).collect();
    let globals = declarations.declare_globals(constants, &constant_names, &walked)?;
    let bound_entries = bound::entries(declarations.builder.module(), &prepared.callables,
        &declarations.functions, &declarations.types)?;
    let calls = std::cell::RefCell::new(known_calls::CallSpans::new());
    // Check ordinary/default/cleanup bodies before constructor specializations.
    let mut bodies: Vec<_> = analysis.bodies.iter().collect();
    bodies.sort_by_key(|body| matches!(body.owner, thir::BodyOwner::Ctor { .. }));
    for body in bodies {
        let function = declarations.functions.get(&body.owner).copied().ok_or_else(|| {
            LowerError::unsupported(body.span, format!("body {:?}", body.owner))
        })?;
        let mut variants = vec![function];
        if let thir::BodyOwner::Ctor { object, ctor } = body.owner {
            variants = declarations.constructors.iter().filter_map(|((o, c, _), &id)| {
                (*o == object && *c == ctor).then_some(id)
            }).collect();
            variants.sort();
        }
        for function in variants {
            let symbol = declarations.builder.module().functions.get(function)?.symbol.clone();
            if declarations.builder.module().functions.get(function)?.attrs.iter().any(|attr| {
                matches!(attr, ir::FunctionAttr::Intrinsic { .. } | ir::FunctionAttr::Extern(_))
            }) {
                continue;
            }
            let root = declarations.builder.declare_scope(ir::ScopeDecl {
                function,
                parent: None,
                kind: ir::ScopeKind::Action,
                loc: None,
            })?;
            let builder = declarations.builder.function_builder(function, root)?;
            let listed = match body.owner {
                thir::BodyOwner::Ctor { object, ctor } => {
                    match &analysis.registry.objects[object.0].ctors[ctor].shape {
                        crate::analysis::registry::CtorShape::Shorthand { fields, .. } => Some(fields.clone()),
                        _ => None,
                    }
                }
                _ => None,
            };
            let context = body::BodyContext {
                functions: &declarations.functions,
                natives: &declarations.natives,
                error_text: declarations.error_text,
                builtin_adapters: &declarations.builtin_adapters,
                objects: &declarations.objects,
                constructors: &declarations.constructors,
                given: declarations.ctor_fields.get(&function).cloned().unwrap_or_default(),
                listed,
                defers: &declarations.defers,
                deferred: None,
                key: callables::BodyKey { root: body.owner.clone(), path: vec![] },
                lambdas: &declarations.lambdas, globals: &globals, calls: &calls, bound_entries: &bound_entries,
            };
            let lowered = body::BodyLowerer::lower(builder, body, root, &declarations.types, context)
                .map_err(|error| error.in_function(&symbol))?;
            declarations.builder.define_function(function, lowered)?;
        }
    }
    for lambda in &declarations.lambdas {
        let function = lambda.function;
        let root = declarations.builder.declare_scope(ir::ScopeDecl {
            function, parent: None, kind: ir::ScopeKind::Action, loc: None,
        })?;
        let symbol = declarations.builder.module().functions.get(function)?.symbol.clone();
        let builder = declarations.builder.function_builder(function, root)?;
        let context = body::BodyContext {
            functions: &declarations.functions,
            natives: &declarations.natives,
            error_text: declarations.error_text,
            builtin_adapters: &declarations.builtin_adapters,
            objects: &declarations.objects,
            constructors: &declarations.constructors,
            given: vec![], listed: None, defers: &declarations.defers, deferred: None,
            key: lambda.source.key.clone(), lambdas: &declarations.lambdas, globals: &globals,
            calls: &calls, bound_entries: &bound_entries,
        };
        let lowered = body::BodyLowerer::lower(builder, &lambda.source.body, root, &declarations.types, context)
            .map_err(|error| error.in_function(&symbol))?;
        declarations.builder.define_function(function, lowered)?;
    }
    for deferred in &declarations.defers {
        let function = deferred.function;
        let root = declarations.builder.declare_scope(ir::ScopeDecl {
            function, parent: None, kind: ir::ScopeKind::Action, loc: None,
        })?;
        let symbol = declarations.builder.module().functions.get(function)?.symbol.clone();
        let builder = declarations.builder.function_builder(function, root)?;
        let context = body::BodyContext {
            functions: &declarations.functions,
            natives: &declarations.natives,
            error_text: declarations.error_text,
            builtin_adapters: &declarations.builtin_adapters,
            objects: &declarations.objects,
            constructors: &declarations.constructors,
            given: vec![], listed: None, defers: &declarations.defers,
            deferred: Some(deferred.receiver),
            key: deferred.key.clone(), lambdas: &declarations.lambdas, globals: &globals,
            calls: &calls, bound_entries: &bound_entries,
        };
        let lowered = body::BodyLowerer::lower(builder, &deferred.source, root, &declarations.types, context)
            .map_err(|error| error.in_function(&symbol))?;
        declarations.builder.define_function(function, lowered)?;
    }
    let mut module = declarations.builder.finish()?;
    structure::finish(&mut module)?;
    test_profile::check(&module)?;
    if open_calls {
        #[cfg(test)]
        open_calls::attach(&mut module)?;
    } else {
        known_calls::check(&module, &calls.into_inner())?;
    }
    structure::check(&module)?;
    once::check(&module)?;
    Ok(module)
}
