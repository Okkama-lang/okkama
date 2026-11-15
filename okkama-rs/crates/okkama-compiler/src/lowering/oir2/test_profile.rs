//! C8a uses the existing core observation contracts and ordinary desugared fake objects.
//! The runner selects Module.tests, supplies a fresh case context and waits for cleanup.
//! Some([]) is an empty-suite error at entry; None selects the production entry instead.
//! C8b: `test.skip` and the typed `close`/`text`/`bytes` checks are registered test intrinsics
//! (`okkama.test.*`, `native_summaries::contract`). Skip has no normal continuation: its program
//! failure is the skip marker, which no `catch` takes and which unwinds through cleanup to the runner.
use super::body::BodyLowerer;
use super::{LowerError, Result};
use crate::analysis::{registry::TestInfo, thir};
use crate::oir2 as ir;
use crate::source::Span;

/// Assertions lowered as registered test intrinsics rather than core operations.
pub(super) const INTRINSICS: &[&str] = &["skip", "close", "text", "bytes"];

pub(super) fn declare(builder: &mut ir::ModuleBuilder, function: ir::FuncId, test: &TestInfo<'_>) -> Result<()> {
    let module = builder.raw_mut();
    if module.tests.is_none() {
        return Err(LowerError::invalid(test.span, "test in production"));
    }
    if test.is_async {
        // A parameterless, void test body has the High envelopes of every async entry. They do
        // not implement Future ownership or async execution.
        super::asynchronous::declare(builder, function)?;
    }
    Ok(())
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
        // Reject before evaluating arguments: no unregistered intrinsic.
        let intrinsic = INTRINSICS.contains(&name);
        if !intrinsic && !matches!(name, "ok" | "is" | "isnt" | "fail") {
            return Err(LowerError::unsupported(span, format!("test.{name} (unregistered test assertion)")));
        }
        let mut values = vec![];
        for arg in args {
            let Some(value) = self.value(arg)? else {
                return Ok(None);
            };
            values.push(value);
        }
        if intrinsic {
            let function = self.context.test_intrinsics.get(name).copied()
                .ok_or_else(|| LowerError::invalid(span, format!("undeclared test intrinsic {name}")))?;
            self.call(function, values)?;
            if name == "skip" && self.current.is_some() {
                // The skip marker always unwinds: nothing follows its call.
                self.terminate(ir::TermKind::Unreachable)?;
            }
            return Ok(None);
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
