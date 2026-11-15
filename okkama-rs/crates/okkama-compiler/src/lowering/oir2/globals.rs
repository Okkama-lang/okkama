//! Module constants (#153) that are arrays or maps: one OIR-2 global each, with a
//! no-argument init function that builds the value once; every use reads it with `global`.
//! Scalar and enum constants arrive in THIR already folded into literals.
use std::collections::{BTreeSet, HashMap};

use super::{LowerError, Result, declarations::Declarations};
use crate::analysis::{registry::ConstState, thir, ty::Ty};
use crate::oir2 as ir;
use crate::source::Span;

impl Declarations {
    /// Declares the globals that reachable bodies read. `constants` are the frontend's
    /// evaluated values; the specializer's registry does not evaluate them again.
    pub fn declare_globals(&mut self, constants: &[ConstState], names: &[String],
        bodies: &[&thir::Body]) -> Result<HashMap<usize, ir::DeclId>> {
        let mut used = BTreeSet::new();
        for body in bodies {
            super::walk::block(&body.block, &mut |expr| {
                if let thir::ExprKind::Global(index) = expr.kind {
                    used.insert(index);
                }
            });
        }
        let mut globals = HashMap::new();
        for index in used {
            let span = Span::default();
            let Some(ConstState::Done(Some(result))) = constants.get(index) else {
                return Err(LowerError::invalid(span, "a module constant without a value"));
            };
            let ty = self.ty(&result.ty, span)?;
            let init = self.builder.declare_function(ir::Function {
                symbol: format!("{}.init", names[index]),
                signature: ir::Signature {
                    params: vec![], results: vec![ty], ownership_abi: ir::OwnershipAbi::Unelaborated,
                },
                attrs: vec![], type_params: vec![], summary: None,
                async_creation_summary: None, async_body_summary: None, body: None, loc: None,
            })?;
            let root = self.builder.declare_scope(ir::ScopeDecl {
                function: init, parent: None, kind: ir::ScopeKind::Action, loc: None,
            })?;
            let types = self.types.clone();
            let mut builder = self.builder.function_builder(init, root)?;
            let entry = builder.entry();
            let failure = builder.intern_type(ir::Type::Service(ir::ServiceType::Failure))?;
            let unwind = builder.create_block(root, &[ir::ParamSpec::new(failure)])?;
            builder.switch_to(unwind)?;
            builder.terminate(ir::TermKind::Raise(builder.block_params(unwind)?[0]))?;
            builder.switch_to(entry)?;
            builder.set_unwind(Some(ir::BlockUnwind { target: ir::Target { block: unwind, args: vec![] } }))?;
            let value = build(&mut builder, &types, &result.value, &result.ty, span)?;
            super::roles::finish_adapter(&mut builder, root, entry, Some(value))?;
            let body = builder.finish()?;
            self.builder.define_function(init, body)?;
            let declaration = self.builder.declare(ir::Declaration {
                name: names[index].clone(), type_params: vec![], loc: None,
                kind: ir::DeclarationKind::Global { ty, init },
            })?;
            globals.insert(index, declaration);
        }
        Ok(globals)
    }
}

/// The constant's value from literals, `array.new` and `map.new` (map entries in key order).
fn build(builder: &mut ir::FunctionBuilder<'_>, types: &HashMap<Ty, ir::TypeId>, value: &thir::ConstValue,
    ty: &Ty, span: Span) -> Result<ir::ValueId> {
    let lowered = |ty: &Ty| types.get(ty).copied().ok_or_else(|| {
        LowerError::unsupported(span, format!("type {ty:?}"))
    });
    let operation = match (value, ty) {
        (thir::ConstValue::Scalar(constant), _) => ir::Operation::Const(super::scalar::literal(constant)),
        (thir::ConstValue::Rat(numerator, denominator), _) => ir::Operation::Const(ir::Literal::Rat {
            numerator: numerator.clone(), denominator: denominator.clone(),
        }),
        (thir::ConstValue::Enum(_, case), Ty::Enum(_)) => {
            let ir::Type::Enum(declaration) = *builder.module().types.get(lowered(ty)?)? else {
                return Err(LowerError::invalid(span, "enum constant without an enum type"));
            };
            ir::Operation::Core(ir::CoreOp::EnumCase { declaration, case: *case as u32 })
        }
        (thir::ConstValue::Array(items), Ty::Array(element)) => {
            let values = items.iter().map(|item| build(builder, types, item, element, span))
                .collect::<Result<Vec<_>>>()?;
            ir::Operation::Collection(ir::CollectionOp::ArrayNew { values, ty: lowered(ty)?, packet: None })
        }
        (thir::ConstValue::Map(items), Ty::Map(key, item)) => {
            let mut entries = vec![];
            for (left, right) in items {
                entries.push((build(builder, types, left, key, span)?, build(builder, types, right, item, span)?));
            }
            ir::Operation::Collection(ir::CollectionOp::MapNew { entries, ty: lowered(ty)?, packet: None })
        }
        _ => return Err(LowerError::invalid(span, "constant value does not match its type")),
    };
    Ok(builder.emit_schema(operation)?[0])
}
