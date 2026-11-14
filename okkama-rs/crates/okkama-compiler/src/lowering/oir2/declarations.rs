use std::collections::HashMap;

use super::{LowerError, Result};
use crate::analysis::{Analysis, thir, ty};
use crate::oir2 as ir;
use crate::lowering::ctx::PENDING_FIELD;
use crate::source::Span;
use crate::syntax::ast;

pub(super) struct Declarations {
    pub builder: ir::ModuleBuilder,
    pub natives: HashMap<usize, ir::FuncId>,
    pub error_text: Option<ir::FuncId>,
    pub functions: HashMap<thir::BodyOwner, ir::FuncId>,
    pub types: HashMap<ty::Ty, ir::TypeId>,
    pub objects: HashMap<ty::ObjectId, ir::DeclId>,
    pub constructors: HashMap<(ty::ObjectId, usize, Vec<usize>), ir::FuncId>,
    pub defers: Vec<super::defer::Deferred>,
    pub builtin_adapters: HashMap<(ir::TypeId, ir::TypeId), ir::FuncId>,
    pub ctor_fields: HashMap<ir::FuncId, Vec<usize>>,
}

impl Declarations {
    pub fn new(analysis: &Analysis<'_>) -> Result<Self> {
        let reg = &analysis.registry;
        let source = &reg.modules[reg.entry_module];
        let mut builder = ir::ModuleBuilder::new(&source.name, ir::Profile::High);
        builder.raw_mut().requires.push(ir::Feature::Core);
        builder.raw_mut().version = source.program.module.version.as_ref().map(|v| {
            format!("{}.{}.{}", v.major, v.minor, v.patch)
        });
        let mut this = Self {
            builder,
            natives: HashMap::new(),
            error_text: None,
            functions: HashMap::new(),
            types: HashMap::new(),
            objects: HashMap::new(),
            constructors: HashMap::new(),
            ctor_fields: HashMap::new(),
            builtin_adapters: HashMap::new(),
            defers: vec![],
        };
        for (ty, lowered) in [
            (ty::Ty::Int, ir::Type::Int),
            (ty::Ty::Bool, ir::Type::Bool),
            (ty::Ty::Str, ir::Type::Str),
            (ty::Ty::Range, ir::Type::Range),
        ] {
            this.types.insert(ty, this.builder.intern_type(lowered)?);
        }
        if let Some(handle) = reg.handles.first() {
            return Err(LowerError::unsupported(handle.span, "native object declaration"));
        }
        let byte = this.builder.intern_type(ir::Type::Byte)?;
        this.types.insert(ty::Ty::Byte, byte);
        for (source, lowered) in [(ty::Ty::Byte, byte), (ty::Ty::Str, this.types[&ty::Ty::Str])] {
            let array = this.builder.intern_type(ir::Type::Array(lowered))?;
            this.types.insert(ty::Ty::array(source.clone()), array);
            this.types.insert(ty::Ty::Varargs(Box::new(source)), array);
        }
        for (index, object) in reg.objects.iter().enumerate() {
            let Some(decl) = object.decl else {
                continue;
            };
            if !object.params.is_empty() || !object.caps.is_empty() {
                return Err(LowerError::unsupported(decl.span, "generic object, capability or role witness"));
            }
            let id = this.builder.declare(ir::Declaration {
                name: if object.name.contains('<') {
                    format!("{}::{}", reg.modules[object.module].name, object.name)
                } else {
                    format!("{}.{}", reg.modules[object.module].name.replace('/', "."), object.name)
                },
                type_params: vec![],
                kind: ir::DeclarationKind::Object {
                    fields: vec![], witnesses: vec![], destroy: None,
                    builtin: reg.modules[object.module].name == "async/atomic" && object.name.starts_with("Atomic_"),
                },
                loc: None,
            })?;
            this.objects.insert(ty::ObjectId(index), id);
            let lowered = this.builder.intern_type(ir::Type::Object { decl: id, args: vec![] })?;
            this.types.insert(ty::Ty::object(ty::ObjectId(index)), lowered);
            let successor = this.builder.intern_type(ir::Type::Successor(lowered))?;
            this.types.insert(ty::Ty::Successor(Box::new(ty::Ty::object(ty::ObjectId(index)))), successor);
        }
        for (index, role) in reg.roles.iter().enumerate() {
            if role.decl.is_none() || !role.params.is_empty() {
                continue;
            }
            let name = if role.name.contains('<') {
                format!("{}::{}", reg.modules[role.module].name, role.name)
            } else {
                format!("{}.{}", reg.modules[role.module].name.replace('/', "."), role.name)
            };
            let declaration = this.builder.declare(ir::Declaration {
                name, type_params: vec![], loc: None,
                kind: ir::DeclarationKind::Role { methods: vec![], builtin: false },
            })?;
            let lowered = this.builder.intern_type(ir::Type::Role { decl: declaration, args: vec![] })?;
            let source = ty::Ty::role(ty::RoleId(index));
            this.types.insert(source.clone(), lowered);
            let successor = this.builder.intern_type(ir::Type::Successor(lowered))?;
            this.types.insert(ty::Ty::Successor(Box::new(source)), successor);
        }
        // Reserve nominal types before resolving potentially recursive payloads.
        let mut variants = vec![];
        for (index, enumeration) in reg.enums.iter().enumerate() {
            if enumeration.span.is_empty() {
                continue;
            }
            if !enumeration.params.is_empty() {
                return Err(LowerError::unsupported(enumeration.span, "generic enum"));
            }
            let payload = enumeration.payloads.iter().any(|payload| !payload.is_empty());
            let name = if enumeration.name.contains('<') {
                format!("{}::{}", reg.modules[enumeration.module].name, enumeration.name)
            } else {
                format!("{}.{}", reg.modules[enumeration.module].name.replace('/', "."), enumeration.name)
            };
            let mut early_cases = vec![];
            let plain_error = enumeration.error && enumeration.payloads.iter().flatten().all(|(_, ty)| {
                matches!(ty, ty::Ty::Int | ty::Ty::Str | ty::Ty::Bool)
            });
            if payload && enumeration.module < reg.root_start && plain_error {
                for (case, fields) in enumeration.cases.iter().zip(&enumeration.payloads) {
                    let fields = fields.iter().map(|(name, ty)| Ok(ir::Field {
                        name: name.clone(), ty: this.ty(ty, enumeration.span)?, default: None, once: false,
                    })).collect::<Result<_>>()?;
                    let layout = this.builder.declare(ir::Declaration {
                        name: format!("{name}.{case}"), type_params: vec![], loc: None,
                        kind: ir::DeclarationKind::Object { fields, witnesses: vec![], destroy: None, builtin: false },
                    })?;
                    early_cases.push(ir::VariantCase { name: case.clone(), layout });
                }
            }
            let early = !early_cases.is_empty();
            let id = this.builder.declare(ir::Declaration {
                name,
                type_params: vec![],
                kind: if payload {
                    ir::DeclarationKind::Variant { cases: early_cases, is_error: enumeration.error }
                } else {
                    ir::DeclarationKind::Enum { cases: enumeration.cases.clone(), is_error: enumeration.error }
                },
                loc: None,
            })?;
            let lowered = this.builder.intern_type(if payload { ir::Type::Variant(id) } else { ir::Type::Enum(id) })?;
            this.types.insert(ty::Ty::Enum(ty::EnumId(index)), lowered);
            this.types.insert(ty::Ty::Variant(ty::EnumId(index), vec![]), lowered);
            if payload && !early {
                variants.push((id, enumeration));
            }
        }
        // Nominal identities are reserved. Intern concrete collection shapes before
        // resolving fields/signatures, including recursive nominal payloads.
        let mut requested = vec![];
        for body in &analysis.bodies {
            requested.extend(body.locals.iter().map(|local| (local.ty.clone(), body.span)));
            super::walk::block(&body.block, &mut |expr| {
                if let Some(ty) = &expr.ty {
                    requested.push((ty.clone(), expr.span));
                }
            });
        }
        for (index, object) in reg.objects.iter().enumerate() {
            if object.decl.is_none() || !object.params.is_empty() {
                continue;
            }
            requested.extend(object.fields.iter().map(|field| (field.ty.clone(), field.span)));
            for (method_index, method) in object.methods.iter().enumerate() {
                if !analysis.bodies.iter().any(|body| body.owner == thir::BodyOwner::Method {
                    object: ty::ObjectId(index), method: method_index,
                }) {
                    continue;
                }
                requested.extend(method.params.iter().map(|(_, ty)| (ty.clone(), source.program.module.span)));
                requested.extend(method.result.iter().map(|ty| (ty.clone(), source.program.module.span)));
            }
        }
        for role in &reg.roles {
            if let Some(declaration) = role.decl {
                for method in &role.methods {
                    requested.extend(method.params.iter().map(|(_, ty)| (ty.clone(), declaration.span)));
                    requested.extend(method.result.iter().map(|ty| (ty.clone(), declaration.span)));
                }
            }
        }
        for enumeration in &reg.enums {
            if enumeration.span.is_empty() || !enumeration.params.is_empty() {
                continue;
            }
            requested.extend(enumeration.payloads.iter().flatten()
                .map(|(_, ty)| (ty.clone(), enumeration.span)));
        }
        for (ty, span) in requested {
            this.collection_type(&ty, span)?;
        }
        for (id, enumeration) in variants {
            let mut cases = vec![];
            let name = this.builder.module().declarations.get(id)?.name.clone();
            for (case, fields) in enumeration.cases.iter().zip(&enumeration.payloads) {
                let fields = fields.iter().map(|(name, ty)| Ok(ir::Field {
                    name: name.clone(), ty: this.ty(ty, enumeration.span)?, default: None, once: false,
                })).collect::<Result<_>>()?;
                let layout = this.builder.declare(ir::Declaration {
                    name: format!("{name}.{case}"), type_params: vec![], loc: None,
                    kind: ir::DeclarationKind::Object { fields, witnesses: vec![], destroy: None, builtin: false },
                })?;
                cases.push(ir::VariantCase { name: case.clone(), layout });
            }
            this.builder.raw_mut().declarations.get_mut(id)?.kind = ir::DeclarationKind::Variant {
                cases, is_error: enumeration.error,
            };
        }
        for (index, role) in reg.roles.iter().enumerate() {
            let Some(&ty) = this.types.get(&ty::Ty::role(ty::RoleId(index))) else {
                continue;
            };
            let ir::Type::Role { decl, .. } = *this.builder.module().types.get(ty)? else {
                unreachable!("reserved role");
            };
            let span = role.decl.expect("source role").span;
            let methods = role.methods.iter().map(|method| {
                if method.is_async || method.variadic {
                    return Err(LowerError::unsupported(span, "async or variadic role slot"));
                }
                Ok(ir::RoleMethod {
                    name: method.name.clone(),
                    signature: ir::Signature {
                        params: method.params.iter().map(|(_, ty)| this.ty(ty, span)).collect::<Result<_>>()?,
                        results: method.result.iter().map(|ty| this.ty(ty, span)).collect::<Result<_>>()?,
                        ownership_abi: ir::OwnershipAbi::Unelaborated,
                    },
                })
            }).collect::<Result<_>>()?;
            this.builder.raw_mut().declarations.get_mut(decl)?.kind = ir::DeclarationKind::Role {
                methods, builtin: false,
            };
        }
        let error = ty::Ty::role(reg.system.error);
        let mut uses_error = analysis.bodies.iter().any(|body| body.locals.iter().any(|local| local.ty == error));
        for body in &analysis.bodies {
            super::walk::block(&body.block, &mut |expr| uses_error |= expr.ty.as_ref() == Some(&error));
        }
        if uses_error {
            let declaration = this.builder.declare(ir::Declaration {
                name: "Error".into(), type_params: vec![], loc: None,
                kind: ir::DeclarationKind::Role {
                    methods: vec![ir::RoleMethod {
                        name: "text".into(),
                        signature: ir::Signature {
                            params: vec![], results: vec![this.types[&ty::Ty::Str]],
                            ownership_abi: ir::OwnershipAbi::Unelaborated,
                        },
                    }],
                    builtin: true,
                },
            })?;
            let lowered = this.builder.intern_type(ir::Type::Role { decl: declaration, args: vec![] })?;
            this.types.insert(error, lowered);
            let mut calls_text = false;
            for body in &analysis.bodies {
                super::walk::block(&body.block, &mut |expr| {
                    calls_text |= matches!(expr.kind, thir::ExprKind::Call {
                        callee: thir::Callee::Role { role, slot: 0, .. }, ..
                    } if role == reg.system.error);
                });
            }
            if calls_text {
                this.error_text = Some(this.external("okkama.Error.text", false, vec![lowered],
                    vec![this.types[&ty::Ty::Str]], vec![], source.program.module.span)?);
            }
        }
        let mut requested_ctors = HashMap::new();
        for body in &analysis.bodies {
            super::walk::block(&body.block, &mut |expr| {
                if let thir::ExprKind::Call {
                    callee: thir::Callee::Ctor { object, ctor, given, .. }, ..
                } = &expr.kind {
                    requested_ctors.entry((*object, *ctor)).or_insert_with(|| given.clone());
                }
            });
        }
        // Declare all symbols before bodies: forward references and stable field/default links.
        for body in &analysis.bodies {
            if body.is_async {
                return Err(LowerError::unsupported(body.span, "async action or cleanup"));
            }
            let mut params = vec![];
            let (symbol, attrs, result) = match body.owner {
                thir::BodyOwner::Entry => {
                    let result = reg.entry.map(|entry| &entry.result);
                    let result = match result {
                        Some(ast::ResultSpec::Void(_)) => None,
                        _ => Some(ty::Ty::Int),
                    };
                    ("main".into(), vec![ir::FunctionAttr::Entry], result)
                }
                thir::BodyOwner::Method { object, method } => {
                    let info = &reg.objects[object.0];
                    let decl = this.objects[&object];
                    let method_info = &info.methods[method];
                    params.push(this.ty(&ty::Ty::object(object), body.span)?);
                    let attr = if method_info.successor {
                        ir::FunctionAttr::Successor(decl)
                    } else {
                        ir::FunctionAttr::Method(decl)
                    };
                    let mut attrs = vec![attr];
                    if method_info.hidden {
                        attrs.push(ir::FunctionAttr::Hidden);
                    }
                    let name = &this.builder.module().declarations.get(decl)?.name;
                    (format!("{name}.{}#{method}", method_info.name), attrs, method_info.result.clone())
                }
                thir::BodyOwner::Ctor { object, ctor } => {
                    let decl = this.objects[&object];
                    let name = &this.builder.module().declarations.get(decl)?.name;
                    (format!("{name}.new#{ctor}"), vec![ir::FunctionAttr::Ctor(decl)], Some(ty::Ty::object(object)))
                }
                thir::BodyOwner::Default { object, field } => {
                    let decl = this.objects[&object];
                    let name = &this.builder.module().declarations.get(decl)?.name;
                    let attr = ir::FunctionAttr::Default { object: decl, field: field as u32 };
                    let result = reg.objects[object.0].fields[field].ty.clone();
                    (format!("{name}.default#{field}"), vec![attr], Some(result))
                }
                thir::BodyOwner::Destroy { object } => {
                    let decl = this.objects[&object];
                    params.push(this.ty(&ty::Ty::object(object), body.span)?);
                    let name = &this.builder.module().declarations.get(decl)?.name;
                    (format!("{name}.destroy"), vec![ir::FunctionAttr::Destroy(decl)], None)
                }
                _ => return Err(LowerError::unsupported(body.span, "test, closure or capability body")),
            };
            let shorthand = match body.owner {
                thir::BodyOwner::Ctor { object, ctor } => {
                    match &reg.objects[object.0].ctors[ctor].shape {
                        crate::analysis::registry::CtorShape::Shorthand { fields, .. } => Some(fields.clone()),
                        _ => None,
                    }
                }
                _ => None,
            };
            let primary_given = if let thir::BodyOwner::Ctor { object, ctor } = body.owner
                && reg.objects[object.0].name.contains('<') && shorthand.is_some() {
                requested_ctors.get(&(object, ctor)).cloned()
            } else {
                None
            };
            for param in &body.params {
                let local = &body.locals[param.0];
                params.push(this.ty(&local.ty, local.span)?);
            }
            if let Some(given) = &primary_given
                && let thir::BodyOwner::Ctor { object, .. } = body.owner {
                params = given.iter().map(|&field| {
                    let field = &reg.objects[object.0].fields[field];
                    this.ty(&field.ty, field.span)
                }).collect::<Result<_>>()?;
            }
            let symbol = if let Some(object) = super::specialize::owner_object(&body.owner)
                && reg.objects[object.0].name.contains('<') {
                let info = &reg.objects[object.0];
                let name = format!("{}::{}", reg.modules[info.module].name, info.name);
                let shape = body.params.iter().map(|param| {
                    let local = &body.locals[param.0];
                    Ok(format!("{}:{}", local.name, super::specialize::type_name(reg, &local.ty)?))
                }).collect::<Result<Vec<_>>>()?.join(",");
                match body.owner {
                    thir::BodyOwner::Method { method, .. } => {
                        let member = &info.methods[method].name;
                        format!("{name}.{member}#overload({shape})")
                    }
                    thir::BodyOwner::Ctor { .. } => format!("{name}.new#ctor({shape})"),
                    thir::BodyOwner::Default { field, .. } => format!("{name}.default#{}", info.fields[field].name),
                    thir::BodyOwner::Destroy { .. } => format!("{name}.destroy"),
                    _ => symbol,
                }
            } else {
                symbol
            };
            let symbol = if let Some(given) = &primary_given
                && shorthand.as_ref() != Some(given)
                && let thir::BodyOwner::Ctor { object, .. } = body.owner {
                let names = given.iter().map(|&field| reg.objects[object.0].fields[field].name.clone())
                    .collect::<Vec<_>>().join(".");
                format!("{symbol}.given_{names}")
            } else {
                symbol
            };
            let results = result.as_ref().map(|ty| this.ty(ty, body.span)).transpose()?.into_iter().collect();
            let atomic = match body.owner {
                thir::BodyOwner::Ctor { object, .. } | thir::BodyOwner::Method { object, .. }
                    if reg.modules[reg.objects[object.0].module].name == "async/atomic" => {
                    let info = &reg.objects[object.0];
                    let suffix = info.name.split_once('_').expect("closed Atomic").1;
                    let op = match body.owner {
                        thir::BodyOwner::Ctor { .. } => "new_value",
                        thir::BodyOwner::Method { method, .. } => info.methods[method].name.as_str(),
                        _ => unreachable!(),
                    };
                    Some(format!("okkama.atomic.Atomic.{op}${suffix}"))
                }
                _ => None,
            };
            let function = if let Some(name) = atomic {
                this.external(&name, false, params, results, attrs, body.span)?
            } else {
                this.builder.declare_function(ir::Function {
                    symbol,
                    signature: ir::Signature { params, results, ownership_abi: ir::OwnershipAbi::Unelaborated },
                    attrs,
                    type_params: vec![],
                    summary: None,
                    async_creation_summary: None,
                    async_body_summary: None,
                    body: None,
                    loc: None,
                })?
            };
            if body.owner == thir::BodyOwner::Entry {
                this.builder.raw_mut().entry = Some(function);
            }
            this.functions.insert(body.owner.clone(), function);
            if let thir::BodyOwner::Ctor { object, ctor } = body.owner {
                let given = primary_given.or(shorthand).unwrap_or_default();
                this.constructors.insert((object, ctor, given.clone()), function);
                this.ctor_fields.insert(function, given);
            }
        }
        let mut object_ids: Vec<_> = this.objects.iter().map(|(&object, &decl)| (object, decl)).collect();
        object_ids.sort_by_key(|(object, _)| *object);
        for (object, decl) in object_ids {
            let info = &reg.objects[object.0];
            let mut fields = vec![];
            for (index, field) in info.fields.iter().enumerate() {
                fields.push(ir::Field {
                    name: field.name.clone(),
                    ty: this.ty(&field.ty, field.span)?,
                    default: this.functions.get(&thir::BodyOwner::Default { object, field: index }).copied(),
                    once: field.name.starts_with("$once"),
                });
            }
            let defer_receiver = analysis.bodies.iter().any(|body| {
                matches!(body.owner, thir::BodyOwner::Ctor { object: owner, .. } if owner == object)
                    && body.block.stmts.iter().any(super::defer::uses_receiver)
            });
            if thir::escapes_self(&analysis.bodies, object) || defer_receiver {
                fields.push(ir::Field {
                    name: PENDING_FIELD.into(), ty: this.types[&ty::Ty::Bool], default: None, once: false,
                });
            }
            let destroy = this.functions.get(&thir::BodyOwner::Destroy { object }).copied();
            this.builder.raw_mut().declarations.get_mut(decl)?.kind = ir::DeclarationKind::Object {
                fields, witnesses: vec![], destroy,
                builtin: reg.modules[info.module].name == "async/atomic" && info.name.starts_with("Atomic_"),
            };
        }
        // Shorthand omissions change the language signature, so each supplied-field
        // set gets a finite direct-call specialization. Defaults still run in it.
        let mut calls = Vec::new();
        for body in &analysis.bodies {
            super::walk::block(&body.block, &mut |expr| {
                if let thir::ExprKind::Call {
                    callee: thir::Callee::Ctor { object, ctor, given, .. }, ..
                } = &expr.kind {
                    calls.push((*object, *ctor, given.clone()));
                }
            });
        }
        for (object, ctor, given) in calls {
            let key = (object, ctor, given.clone());
            if this.constructors.contains_key(&key) {
                continue;
            }
            let original = this.functions.get(&thir::BodyOwner::Ctor { object, ctor }).copied()
                .ok_or_else(|| LowerError::unsupported(reg.objects[object.0].ctors[ctor].span,
                    "constructor source body was not discovered"))?;
            let mut function = this.builder.module().functions.get(original)?.clone();
            let suffix = if reg.objects[object.0].name.contains('<') {
                given.iter().map(|&field| reg.objects[object.0].fields[field].name.clone())
                    .collect::<Vec<_>>().join(".")
            } else {
                given.iter().map(usize::to_string).collect::<Vec<_>>().join(".")
            };
            let symbol = if reg.objects[object.0].name.contains('<') {
                function.symbol.split(".given_").next().expect("constructor symbol")
            } else {
                &function.symbol
            };
            function.symbol = format!("{symbol}.given_{suffix}");
            function.signature.params = given.iter().map(|&field| {
                this.ty(&reg.objects[object.0].fields[field].ty, reg.objects[object.0].fields[field].span)
            }).collect::<Result<_>>()?;
            let id = this.builder.declare_function(function)?;
            this.constructors.insert(key, id);
            this.ctor_fields.insert(id, given);
        }
        let mut native_calls = vec![];
        for body in &analysis.bodies {
            super::walk::block(&body.block, &mut |expr| {
                if let thir::ExprKind::Call { callee: thir::Callee::Native(index), .. } = expr.kind {
                    native_calls.push(index);
                }
            });
            this.declare_defers(body)?;
        }
        native_calls.sort();
        native_calls.dedup();
        for index in native_calls {
            let native = &reg.natives[index];
            let module = &reg.modules[native.module];
            let name = format!("{}@{}::{}/{}", module.name, module.program.module.major(),
                native.name, native.params.len());
            if native.is_async || native.variadic {
                return Err(LowerError::unsupported(native.decl.span, "async or variadic native"));
            }
            let params = native.params.iter().map(|(_, ty)| this.ty(ty, native.decl.span)).collect::<Result<_>>()?;
            let results = native.result.iter().map(|ty| this.ty(ty, native.decl.span)).collect::<Result<_>>()?;
            let function = this.external(&name, true, params, results, vec![], native.decl.span)?;
            this.natives.insert(index, function);
        }
        super::roles::declare_witnesses(&mut this, analysis)?;
        super::roles::declare_builtins(&mut this, analysis)?;
        Ok(this)
    }

    fn external(&mut self, name: &str, native: bool, params: Vec<ir::TypeId>, results: Vec<ir::TypeId>,
        mut attrs: Vec<ir::FunctionAttr>, span: Span) -> Result<ir::FuncId> {
        let signature = ir::Signature { params, results, ownership_abi: ir::OwnershipAbi::Unelaborated };
        let summary = super::native_summaries::contract(self.builder.module(), name, native, &signature)
            .map_err(|error| LowerError::unsupported(span, error.message))?;
        let summary = Some(self.builder.add_summary(summary)?);
        let objects = if native { vec![] } else {
            super::native_summaries::intrinsic_objects(self.builder.module(), name, &signature)
                .map_err(|error| LowerError::unsupported(span, error.message))?
        };
        attrs.push(if native { ir::FunctionAttr::Extern(name.into()) } else {
            ir::FunctionAttr::Intrinsic { name: name.into(), objects, helpers: vec![] }
        });
        Ok(self.builder.declare_function(ir::Function {
            symbol: name.replace('/', ".").replace('@', ".v").replace('$', "#"), signature, attrs, summary,
            type_params: vec![], body: None, loc: None,
            async_creation_summary: None, async_body_summary: None,
        })?)
    }

    fn collection_type(&mut self, ty: &ty::Ty, span: Span) -> Result<()> {
        use ty::Ty as T;
        if self.types.contains_key(ty) {
            return Ok(());
        }
        let lowered = match ty {
            T::Never => ir::Type::Never,
            T::Float => ir::Type::Float,
            T::Rat => ir::Type::Rat,
            T::Char => ir::Type::Char,
            T::Array(element) | T::Varargs(element) => {
                self.collection_type(element, span)?;
                ir::Type::Array(self.ty(element, span)?)
            }
            T::Map(key, value) => {
                self.collection_type(key, span)?;
                self.collection_type(value, span)?;
                ir::Type::Map(self.ty(key, span)?, self.ty(value, span)?)
            }
            _ => return Ok(()),
        };
        let lowered = self.builder.intern_type(lowered)?;
        self.types.insert(ty.clone(), lowered);
        Ok(())
    }

    pub(super) fn ty(&self, ty: &ty::Ty, span: Span) -> Result<ir::TypeId> {
        self.types.get(ty).copied().ok_or_else(|| LowerError::unsupported(span, format!("type {ty:?}")))
    }
}
