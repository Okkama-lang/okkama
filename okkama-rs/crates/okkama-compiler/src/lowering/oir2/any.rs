//! #569 C6: `any` boundaries. `Coercion::Box` becomes `any.box` at every
//! boundary the checker marks (bindings, arguments, results, fields, collection
//! elements); a heterogeneous literal boxes each element where it is coerced.
//! An `any` value crossing another `any` boundary keeps its existing box. The
//! box keeps the known graph of its payload: own reads the retention of
//! `any.box`, so a known resource keeps its owner and no dynamic fallback is
//! forced. Narrowing out of `any` stays a checker rejection.
use super::{LowerError, Result};
use crate::analysis::thir;
use crate::oir2 as ir;

impl super::body::BodyLowerer<'_, '_> {
    pub fn box_any(&mut self, expr: &thir::Expr, value: &thir::Expr) -> Result<Option<ir::ValueId>> {
        let ty = self.ty(expr.ty.as_ref().expect("typed any boundary"), expr.span)?;
        if *self.b.module().types.get(ty)? != ir::Type::Any {
            return Err(LowerError::invalid(expr.span, "box coercion to a type other than any"));
        }
        let Some(value) = self.value(value)? else {
            return Ok(None);
        };
        let value = self.take(value)?;
        let payload = self.b.body().values.get(value)?.ty;
        match self.b.module().types.get(payload)? {
            // A repeated `any` boundary keeps the existing box.
            ir::Type::Any => Ok(Some(value)),
            ir::Type::Fn(_) | ir::Type::Cap { .. } | ir::Type::Future(_) | ir::Type::Handle(_) => {
                Err(LowerError::unsupported(expr.span, "boxing an action, cap or task value into any"))
            }
            _ => self.core(ir::CoreOp::AnyBox { value, ty, packet: None }).map(Some),
        }
    }
}

/// Checked runtime descriptors for `any` (#569 C6). A box whose payload is not
/// visible, such as an `any` parameter, result or element, is open: every
/// function whose values reach `any` binds `type.of ty any` with the registered
/// `oir.any.cleanup` dictionary, so its owner graph is released by the runtime
/// and never treated as plain because no fields are visible.
pub(super) fn attach_descriptors(module: &mut ir::Module) -> Result<()> {
    let Some(any) = module.types.iter().find_map(|(id, ty)| (*ty == ir::Type::Any).then_some(id)) else {
        return Ok(());
    };
    let reaching = reaching_any(module)?;
    let functions: Vec<_> = module.functions.iter().filter_map(|(function, data)| {
        let body = data.body.as_ref()?;
        body.values.iter().any(|(_, value)| reaching.contains(&value.ty)).then_some(function)
    }).collect();
    if functions.is_empty() {
        return Ok(());
    }
    let summary = module.summaries.alloc(ir::catalogue::sync_closure_cleanup_summary())?;
    let signature = ir::catalogue::opaque_cleanup_signature(module, any, ir::Profile::High)
        .ok_or_else(|| LowerError::unsupported(crate::source::Span::default(), "any cleanup ABI"))?;
    let cleanup = module.functions.alloc(ir::Function {
        symbol: ir::catalogue::ANY_CLEANUP.into(),
        signature,
        attrs: vec![ir::FunctionAttr::Intrinsic {
            name: ir::catalogue::ANY_CLEANUP.into(), objects: vec![], helpers: vec![],
        }],
        type_params: vec![],
        summary: Some(summary),
        async_creation_summary: None,
        async_body_summary: None,
        body: None,
        loc: None,
    })?;
    let descriptor = module.types.intern(ir::Type::Descriptor)?;
    for function in functions {
        let Some(mut body) = module.functions.get_mut(function)?.body.take() else { continue };
        let entry = body.entry;
        let op = ir::Operation::Core(ir::CoreOp::TypeOf {
            ty: any,
            descriptor: ir::DescriptorContract {
                managed: ir::DescriptorManaged::Runtime,
                retention: ir::SummarySelection::OwnerGraph(0),
                cleanup,
                async_destroy: false,
                summary,
            },
        });
        ir::FunctionEditor::new(module, &mut body).insert_instruction(entry, 0, op,
            &[ir::ResultSpec::new(descriptor)], ir::InstructionFacts::default(), None)?;
        module.functions.get_mut(function)?.body = Some(body);
    }
    Ok(())
}

/// The types that structurally reach `any`: through collections, successors,
/// action signatures, object and variant fields and the implementors of a role.
/// A least fixed point over the module's interned types, so cycles are exact.
fn reaching_any(module: &ir::Module) -> Result<std::collections::HashSet<ir::TypeId>> {
    let mut edges = vec![];
    for (ty, data) in module.types.iter() {
        let mut children = vec![];
        match data {
            ir::Type::Array(child) | ir::Type::Successor(child) | ir::Type::Builder(child) => children.push(*child),
            ir::Type::Map(key, value) | ir::Type::MapBuilder(key, value) => children.extend([*key, *value]),
            ir::Type::Object { decl, .. } => fields(module, *decl, &mut children)?,
            ir::Type::Variant(decl) => {
                if let ir::DeclarationKind::Variant { cases, .. } = &module.declarations.get(*decl)?.kind {
                    for case in cases {
                        fields(module, case.layout, &mut children)?;
                    }
                }
            }
            ir::Type::Role { .. } => {
                for (decl, declaration) in module.declarations.iter() {
                    if let ir::DeclarationKind::Object { witnesses, .. } = &declaration.kind
                        && witnesses.iter().any(|witness| witness.role == ty) {
                        fields(module, decl, &mut children)?;
                    }
                }
            }
            ir::Type::Fn(function) => {
                children.extend(function.params.iter().copied());
                children.extend(function.result);
            }
            _ => {}
        }
        edges.push((ty, children));
    }
    let mut reaching: std::collections::HashSet<_> = module.types.iter()
        .filter_map(|(id, ty)| (*ty == ir::Type::Any).then_some(id)).collect();
    loop {
        let before = reaching.len();
        for (ty, children) in &edges {
            if children.iter().any(|child| reaching.contains(child)) {
                reaching.insert(*ty);
            }
        }
        if reaching.len() == before {
            return Ok(reaching);
        }
    }
}

fn fields(module: &ir::Module, decl: ir::DeclId, children: &mut Vec<ir::TypeId>) -> Result<()> {
    if let ir::DeclarationKind::Object { fields, .. } = &module.declarations.get(decl)?.kind {
        children.extend(fields.iter().map(|field| field.ty));
    }
    Ok(())
}
