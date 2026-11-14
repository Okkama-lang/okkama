//! C8a uses the existing core observation contracts and ordinary desugared fake objects.
//! The runner selects Module.tests, supplies a fresh case context and waits for cleanup.
//! Some([]) is an empty-suite error at entry; None selects the production entry instead.
use super::body::BodyLowerer;
use super::{LowerError, Result};
use crate::analysis::{registry::TestInfo, thir};
use crate::oir2 as ir;
use crate::source::Span;

pub(super) fn declare(builder: &mut ir::ModuleBuilder, function: ir::FuncId, test: &TestInfo<'_>) -> Result<()> {
    let module = builder.raw_mut();
    if module.tests.is_none() {
        return Err(LowerError::invalid(test.span, "test in production"));
    }
    if test.is_async {
        if !module.requires.contains(&ir::Feature::Async) {
            module.requires.push(ir::Feature::Async);
        }
        // Parameterless, void test bodies have no input/result retention relation.
        // These High envelopes do not implement Future ownership or async execution.
        let future = builder.intern_type(ir::Type::Future(None))?;
        let body = builder.add_summary(envelope(vec![], false))?;
        let creation = builder.add_summary(envelope(vec![future], true))?;
        let declaration = builder.raw_mut().functions.get_mut(function)?;
        declaration.async_creation_summary = Some(creation);
        declaration.async_body_summary = Some(body);
    }
    Ok(())
}

fn envelope(results: Vec<ir::TypeId>, creation: bool) -> ir::Summary {
    ir::Summary {
        entry_kind: ir::EntryKind::CallBoundary,
        parameters: vec![],
        results: results.iter().map(|_| ir::ResultRelation {
            fresh: vec![],
            aliases: vec![],
            external_deps: vec![],
            provenance: ir::AdmissionProvenance::UnboundResult,
        }).collect(),
        contract: ir::CallContract {
            results,
            failures: if creation {
                vec![ir::FailureKind::Program, ir::FailureKind::Panic, ir::FailureKind::Native]
            } else {
                vec![ir::FailureKind::Thrown, ir::FailureKind::Program, ir::FailureKind::Panic,
                    ir::FailureKind::Native, ir::FailureKind::Cancelled]
            },
            suspends: !creation,
            async_destroy: false,
            publication: ir::Publication::Atomic,
            protocol: None,
        },
    }
}

pub(super) fn check(module: &ir::Module) -> Result<()> {
    let Some(tests) = &module.tests else {
        return Ok(());
    };
    for test in tests {
        let function = module.functions.get(test.function)?;
        if !function.signature.params.is_empty() || !function.signature.results.is_empty()
            || !function.attrs.iter().all(|attr| *attr == ir::FunctionAttr::Async) {
            return Err(LowerError::invalid(Span::default(), "test entry requires () -> void without owner authority"));
        }
    }
    Ok(())
}

impl BodyLowerer<'_, '_> {
    pub(super) fn test_call(&mut self, name: &str, args: &[thir::Expr], span: Span)
        -> Result<Option<ir::ValueId>> {
        if !self.b.module().requires.contains(&ir::Feature::Test) {
            return Err(LowerError::invalid(span, "test assertion in production"));
        }
        // Reject before evaluating arguments: no unregistered intrinsic or invented skip transport.
        if name == "skip" {
            return Err(LowerError::unsupported(span, "test.skip (pending audited S1 contract)"));
        }
        if !matches!(name, "ok" | "is" | "isnt" | "fail") {
            return Err(LowerError::unsupported(span, format!("test.{name} (typed test intrinsic outside C8a)")));
        }
        let mut values = vec![];
        for arg in args {
            let Some(value) = self.value(arg)? else {
                return Ok(None);
            };
            values.push(value);
        }
        let operation = match (name, values.as_slice()) {
            ("ok", [condition]) => ir::CoreOp::TestOk { condition: *condition },
            ("is", [left, right]) => ir::CoreOp::TestIs { left: *left, right: *right },
            ("isnt", [left, right]) => ir::CoreOp::TestIsnt { left: *left, right: *right },
            ("fail", [message]) => ir::CoreOp::TestFail { message: *message },
            _ => return Err(LowerError::invalid(span, "test assertion arity differs from resolved frontend call")),
        };
        // Check operations observe their arguments, record failure and continue normally.
        // Preserve the registered Program failure envelope and the current H5 catch environment.
        self.sync_failure()?;
        self.b.emit_schema(ir::Operation::Core(operation))?;
        Ok(None)
    }
}
