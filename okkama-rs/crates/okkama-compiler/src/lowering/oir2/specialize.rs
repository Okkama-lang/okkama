//! Demand-driven concrete THIR specialization for the high producer.
//! Ordinary std bodies remain THIR and use the same high producer as application bodies.
use std::collections::{HashMap, HashSet, VecDeque};

use crate::analysis::{Analysis, Registry, SourceModule, registry, thir, ty};
use crate::source::Span;
use super::{LowerError, Result};

#[derive(Clone, Copy)]
pub(super) struct Limits {
    pub depth: usize,
    pub count: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self { depth: 256, count: 1 << 18 }
    }
}

pub(super) fn prepare<'a>(input: &Analysis<'a>) -> Result<Analysis<'a>> {
    prepare_with_limits(input, Limits::default())
}

pub(super) fn prepare_with_limits<'a>(input: &Analysis<'a>, limits: Limits) -> Result<Analysis<'a>> {
    let sources = input.registry.modules.iter().map(|module| SourceModule {
        path: module.path.clone(),
        program: module.program,
        imports: module.imports.clone(),
    }).collect();
    let mut registry = Registry::build_files(sources, input.registry.modules.len() - input.registry.root_start);
    for object in &mut registry.objects {
        if object.module < registry.root_start || !object.params.is_empty() {
            object.decl = None;
        }
    }
    for role in &mut registry.roles {
        if role.module < registry.root_start || !role.params.is_empty() {
            role.decl = None;
        }
    }
    for enumeration in &mut registry.enums {
        if enumeration.module < registry.root_start || !enumeration.params.is_empty() {
            enumeration.span = Span::default();
        }
    }
    let mut slice = Specializer {
        input,
        registry,
        instances: HashMap::new(),
        enums: HashMap::new(),
        roles: HashMap::new(),
        limits,
        chain: vec![],
        requests: 0,
        reserved: HashSet::new(),
        queued: HashSet::new(),
        pending: VecDeque::new(),
        bodies: vec![],
    };
    for body in &input.bodies {
        let generic = owner_object(&body.owner).is_some_and(|object| {
            !input.registry.objects[object.0].params.is_empty()
        });
        if body.module >= input.registry.root_start && !generic {
            slice.queue(body.owner.clone(), body.owner.clone(), vec![], None, body.span)?;
        }
    }
    for (index, source) in input.registry.objects.iter().enumerate() {
        if source.module < input.registry.root_start || source.decl.is_none() || !source.params.is_empty() {
            continue;
        }
        for (field, info) in source.fields.iter().enumerate() {
            slice.registry.objects[index].fields[field].ty = slice.ty(&info.ty, info.span)?;
        }
        for (method, info) in source.methods.iter().enumerate() {
            for (param, (_, ty)) in info.params.iter().enumerate() {
                let concrete = slice.ty(ty, source.decl.unwrap().span)?;
                slice.registry.objects[index].methods[method].params[param].1 = concrete;
            }
            slice.registry.objects[index].methods[method].result = info.result.as_ref()
                .map(|ty| slice.ty(ty, source.decl.expect("source object").span)).transpose()?;
        }
    }
    for (index, role) in input.registry.roles.iter().enumerate() {
        if role.module >= input.registry.root_start && role.params.is_empty()
            && let Some(declaration) = role.decl {
            slice.role(ty::RoleId(index), &[], declaration.span)?;
        }
    }
    for (index, object) in input.registry.objects.iter().enumerate() {
        if object.module >= input.registry.root_start && object.params.is_empty()
            && let Some(declaration) = object.decl {
            slice.witnesses(ty::ObjectId(index), ty::ObjectId(index), &[], declaration.span)?;
        }
    }
    for (index, info) in input.registry.enums.iter().enumerate() {
        if info.module >= input.registry.root_start && info.params.is_empty() && !info.span.is_empty() {
            slice.enumeration(ty::EnumId(index), &[], info.span)?;
        }
    }
    while let Some(BodyInstance { source: owner, target, substitution: env, chain, span }) = slice.pending.pop_front() {
        slice.chain = chain;
        let mut body = input.bodies.iter().find(|body| body.owner == owner).cloned().ok_or_else(|| {
            LowerError::unsupported(span, format!("missing source body for {}",
                slice.chain.last().expect("request chain")))
        })?;
        if let thir::BodyOwner::Method { object, method } = target {
            let original = owner_object(&owner).expect("method owner");
            let result = input.registry.objects[original.0].methods[method].result.as_ref();
            slice.registry.objects[object.0].methods[method].result = result
                .map(|ty| slice.ty(&ty.subst(&env), body.span)).transpose()?;
        }
        body.owner = target;
        for local in &mut body.locals {
            local.ty = slice.ty(&local.ty.subst(&env), local.span)?;
        }
        slice.block(&mut body.block, &env)?;
        slice.bodies.push(body);
    }
    Ok(Analysis { registry: slice.registry, bodies: slice.bodies, warnings: input.warnings.clone() })
}

struct Specializer<'a, 'b> {
    input: &'b Analysis<'a>,
    registry: Registry<'a>,
    instances: HashMap<(ty::ObjectId, Vec<ty::Ty>), ty::ObjectId>,
    enums: HashMap<(ty::EnumId, Vec<ty::Ty>), ty::EnumId>,
    roles: HashMap<(ty::RoleId, Vec<ty::Ty>), ty::RoleId>,
    limits: Limits,
    chain: Vec<String>,
    requests: usize,
    reserved: HashSet<SpecializationKey>,
    queued: HashSet<thir::BodyOwner>,
    pending: VecDeque<BodyInstance>,
    bodies: Vec<thir::Body>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct SpecializationKey {
    declaration: thir::BodyOwner,
    arguments: Vec<ty::Ty>,
    supplied_fields: Option<Vec<usize>>,
}

struct BodyInstance {
    source: thir::BodyOwner,
    target: thir::BodyOwner,
    substitution: Vec<(ty::ParamId, ty::Ty)>,
    chain: Vec<String>,
    span: Span,
}

pub(super) fn owner_object(owner: &thir::BodyOwner) -> Option<ty::ObjectId> {
    match owner {
        thir::BodyOwner::Method { object, .. } | thir::BodyOwner::Ctor { object, .. }
        | thir::BodyOwner::Default { object, .. } | thir::BodyOwner::Destroy { object } => Some(*object),
        _ => None,
    }
}

impl Specializer<'_, '_> {
    fn request(&mut self, name: String, span: Span) -> Result<()> {
        self.requests += 1;
        if self.chain.len() >= self.limits.depth || self.requests > self.limits.count {
            let mut chain = self.chain.clone();
            chain.push(name);
            return Err(LowerError::unsupported(span, format!(
                "specialization limit (depth {}, count {}): {}",
                self.limits.depth, self.limits.count, chain.join(" -> ")
            )));
        }
        Ok(())
    }

    fn queue(&mut self, owner: thir::BodyOwner, target: thir::BodyOwner,
        env: Vec<(ty::ParamId, ty::Ty)>, supplied_fields: Option<Vec<usize>>, span: Span) -> Result<()> {
        let key = SpecializationKey {
            declaration: owner.clone(),
            arguments: env.iter().map(|(_, ty)| ty.clone()).collect(),
            supplied_fields,
        };
        if !self.reserved.insert(key) {
            return Ok(());
        }
        let name = self.body_name(&target);
        self.request(name.clone(), span)?;
        // Reserve before visiting the body: recursive requests see this identity.
        if !self.queued.insert(target.clone()) {
            return Ok(());
        }
        let mut chain = self.chain.clone();
        chain.push(name);
        self.pending.push_back(BodyInstance { source: owner, target, substitution: env, chain, span });
        Ok(())
    }

    fn body_name(&self, owner: &thir::BodyOwner) -> String {
        let Some(object) = owner_object(owner) else {
            return "main".into();
        };
        let info = &self.registry.objects[object.0];
        let member = match owner {
            thir::BodyOwner::Method { method, .. } => info.methods[*method].name.clone(),
            thir::BodyOwner::Ctor { .. } => "new".into(),
            thir::BodyOwner::Default { field, .. } => format!("default.{}", info.fields[*field].name),
            thir::BodyOwner::Destroy { .. } => "destroy".into(),
            _ => unreachable!(),
        };
        format!("{}::{}.{member}", self.registry.modules[info.module].name, info.name)
    }

    fn check_arguments(&self, params: &[ty::ParamId], args: &[ty::Ty], span: Span) -> Result<()> {
        if args.iter().any(ty::Ty::is_error) {
            return Err(LowerError::invalid(span, "specialization argument contains a frontend error type"));
        }
        if params.len() != args.len() || args.iter().any(ty::Ty::mentions_param) {
            return Err(LowerError::unsupported(span, "reachable generic is not closed after substitution"));
        }
        let env: Vec<_> = params.iter().copied().zip(args.iter().cloned()).collect();
        for (param, arg) in params.iter().zip(args) {
            let info = &self.input.registry.params[param.0];
            if info.key && let Some(problem) = self.input.registry.key_problem(arg) {
                return Err(LowerError::invalid(span, format!("specialization key constraint: {problem}")));
            }
            if let Some(bound) = &info.bound {
                let required = bound.subst(&env);
                if !self.input.registry.assignable(arg, &required) {
                    return Err(LowerError::invalid(span, format!(
                        "specialization bound for {}: {} does not satisfy {}",
                        info.name, self.encode(arg)?, self.encode(&required)?
                    )));
                }
            }
        }
        Ok(())
    }

    fn encode(&self, value: &ty::Ty) -> Result<String> {
        type_name(&self.input.registry, value)
    }

    fn instance(&mut self, object: ty::ObjectId, args: &[ty::Ty], span: Span) -> Result<ty::ObjectId> {
        let key = (object, args.to_vec());
        if let Some(&instance) = self.instances.get(&key) {
            return Ok(instance);
        }
        let source = &self.input.registry.objects[object.0];
        self.check_arguments(&source.params, args, span)?;
        if source.module >= self.registry.root_start && source.params.is_empty() {
            return Ok(object);
        }
        if !source.caps.is_empty() {
            return Err(LowerError::unsupported(span, "capability specialization (#569)"));
        }
        let module = self.input.registry.modules[source.module].name.as_str();
        let generic = matches!((module, source.name.as_str()), ("async/atomic", "Atomic") | ("cell", "Cell"));
        let ordinary = matches!((module, source.name.as_str()),
            ("io/stdout", "Stdout") | ("encoding/utf", "Utf8Encoded"));
        let imported = source.module < self.registry.root_start;
        if imported && (source.decl.is_none()
            || generic && !matches!(args, [ty::Ty::Int] | [ty::Ty::Str])) {
            return Err(LowerError::unsupported(span, format!("std object {module}.{}<{args:?}>", source.name)));
        }
        if module == "async/atomic" {
            for (index, enumeration) in self.input.registry.enums.iter().enumerate() {
                if enumeration.module == source.module && enumeration.name == "AtomicClosed" {
                    self.registry.enums[index].span = enumeration.span;
                }
            }
        }
        let specialized = !source.params.is_empty();
        let instance = if specialized { ty::ObjectId(self.registry.objects.len()) } else { object };
        let type_names = args.iter().map(|ty| self.encode(ty)).collect::<Result<Vec<_>>>()?;
        let name = if imported && (generic || ordinary) {
            let suffix = if args == [ty::Ty::Int] { "_int" } else if args == [ty::Ty::Str] { "_string" } else { "" };
            format!("{}{suffix}", source.name)
        } else if specialized {
            format!("{}<{}>", source.name, type_names.join(","))
        } else {
            source.name.clone()
        };
        self.request(format!("{module}::{name}"), span)?;
        self.instances.insert(key, instance);
        self.chain.push(format!("{module}::{name}"));
        let mut info = registry::ObjectInfo {
            name,
            module: source.module,
            params: vec![],
            fields: source.fields.clone(),
            ctors: source.ctors.clone(),
            methods: source.methods.clone(),
            // Registered Atomic/Cell and finite P1 std boundaries retain their
            // existing direct-call ABI; their unused std witnesses are not requested.
            does: if imported && (generic || ordinary) { vec![] } else { source.does.clone() },
            destroy: source.destroy,
            caps: vec![],
            decl: source.decl,
        };
        if specialized {
            // Reserve the ID before specializing fields (Cell refers to Atomic).
            self.registry.objects.push(registry::ObjectInfo {
                name: info.name.clone(), module: info.module, params: vec![], fields: vec![],
                ctors: vec![], methods: vec![], does: vec![], destroy: None, caps: vec![], decl: info.decl,
            });
        }
        let env: Vec<_> = source.params.iter().copied().zip(args.iter().cloned()).collect();
        for field in &mut info.fields {
            field.ty = self.ty(&field.ty.subst(&env), field.span)?;
        }
        // Method signatures are substituted when their body is requested. This
        // avoids discovering an infinite family through an unused method result.
        self.registry.objects[instance.0] = info;
        if !self.registry.objects[instance.0].does.is_empty() {
            self.witnesses(object, instance, &env, span)?;
        }
        for (field, info) in source.fields.iter().enumerate() {
            if info.default.is_some() {
                self.queue(thir::BodyOwner::Default { object, field },
                    thir::BodyOwner::Default { object: instance, field }, env.clone(), None, span)?;
            }
        }
        if source.destroy.is_some() {
            self.queue(thir::BodyOwner::Destroy { object },
                thir::BodyOwner::Destroy { object: instance }, env, None, span)?;
        }
        self.chain.pop();
        Ok(instance)
    }

    fn role(&mut self, role: ty::RoleId, args: &[ty::Ty], span: Span) -> Result<ty::RoleId> {
        if let Some(&target) = self.roles.get(&(role, args.to_vec())) {
            return Ok(target);
        }
        let source = &self.input.registry.roles[role.0];
        self.check_arguments(&source.params, args, span)?;
        let target = if args.is_empty() { role } else { ty::RoleId(self.registry.roles.len()) };
        let names = args.iter().map(|ty| self.encode(ty)).collect::<Result<Vec<_>>>()?;
        let name = if args.is_empty() { source.name.clone() } else {
            format!("{}<{}>", source.name, names.join(","))
        };
        let request = format!("{}::{name}", self.registry.modules[source.module].name);
        self.request(request.clone(), span)?;
        self.roles.insert((role, args.to_vec()), target);
        self.chain.push(request);
        let info = registry::RoleInfo {
            name, module: source.module, params: vec![], methods: source.methods.clone(), decl: source.decl,
        };
        if target != role {
            self.registry.roles.push(info);
        } else {
            self.registry.roles[target.0] = info;
        }
        let env: Vec<_> = source.params.iter().copied().zip(args.iter().cloned()).collect();
        let mut methods = source.methods.clone();
        for method in &mut methods {
            for (_, ty) in &mut method.params {
                *ty = self.ty(&ty.subst(&env), span)?;
            }
            method.result = method.result.as_ref().map(|ty| self.ty(&ty.subst(&env), span)).transpose()?;
        }
        self.registry.roles[target.0].methods = methods;
        self.chain.pop();
        Ok(target)
    }

    fn witnesses(&mut self, source: ty::ObjectId, target: ty::ObjectId,
        env: &[(ty::ParamId, ty::Ty)], span: Span) -> Result<()> {
        let info = &self.input.registry.objects[source.0];
        let mut witnesses = vec![];
        for (role, args) in &info.does {
            let args: Vec<_> = args.iter().map(|ty| ty.subst(env)).collect();
            let specialized = self.role(*role, &args, span)?;
            witnesses.push((specialized, vec![]));
            for slot in &self.input.registry.roles[role.0].methods {
                let role_info = &self.input.registry.roles[role.0];
                let role_env: Vec<_> = role_info.params.iter().copied().zip(args.iter().cloned()).collect();
                let method = info.methods.iter().position(|method| {
                    method.name == slot.name && method.params.len() == slot.params.len()
                        && method.params.iter().zip(&slot.params)
                            .all(|((_, a), (_, b))| a.subst(env) == b.subst(&role_env))
                }).ok_or_else(|| LowerError::invalid(span, format!("missing implementation of {}", slot.name)))?;
                self.queue(thir::BodyOwner::Method { object: source, method },
                    thir::BodyOwner::Method { object: target, method }, env.to_vec(), None, span)?;
                let params = info.methods[method].params.iter().map(|(name, ty)| {
                    Ok((name.clone(), self.ty(&ty.subst(env), span)?))
                }).collect::<Result<_>>()?;
                self.registry.objects[target.0].methods[method].params = params;
            }
        }
        self.registry.objects[target.0].does = witnesses;
        Ok(())
    }

    fn enumeration(&mut self, id: ty::EnumId, args: &[ty::Ty], span: Span) -> Result<ty::EnumId> {
        let source = &self.input.registry.enums[id.0];
        self.check_arguments(&source.params, args, span)?;
        if let Some(&target) = self.enums.get(&(id, args.to_vec())) {
            return Ok(target);
        }
        let mut info = source.clone();
        if info.span.is_empty() {
            info.span = span;
        }
        let target = if args.is_empty() { id } else { ty::EnumId(self.registry.enums.len()) };
        let names = args.iter().map(|ty| self.encode(ty)).collect::<Result<Vec<_>>>()?;
        if !args.is_empty() {
            info.name = format!("{}<{}>", source.name, names.join(","));
        }
        self.request(format!("{}::{}", self.input.registry.modules[source.module].name, info.name), span)?;
        self.enums.insert((id, args.to_vec()), target);
        self.chain.push(format!("{}::{}", self.input.registry.modules[source.module].name, info.name));
        info.params.clear();
        if target != id {
            self.registry.enums.push(info.clone());
        }
        let env: Vec<_> = source.params.iter().copied().zip(args.iter().cloned()).collect();
        for (_, ty) in info.payloads.iter_mut().flatten() {
            *ty = self.ty(&ty.subst(&env), span)?;
        }
        self.registry.enums[target.0] = info;
        self.chain.pop();
        Ok(target)
    }

    fn ty(&mut self, value: &ty::Ty, span: Span) -> Result<ty::Ty> {
        use ty::Ty as T;
        if value.mentions_param() {
            return Err(LowerError::unsupported(span, "reachable generic is not closed after substitution"));
        }
        Ok(match value {
            T::Object(object, args) => T::object(self.instance(*object, args, span)?),
            T::Role(role, args) => T::role(self.role(*role, args, span)?),
            T::Array(inner) => T::array(self.ty(inner, span)?),
            T::Map(key, value) => {
                let untyped_empty = **key == T::Never && **value == T::Never;
                if !untyped_empty && let Some(problem) = self.input.registry.key_problem(key) {
                    return Err(LowerError::invalid(span, problem));
                }
                T::map(self.ty(key, span)?, self.ty(value, span)?)
            }
            T::Varargs(inner) => T::Varargs(Box::new(self.ty(inner, span)?)),
            T::Successor(inner) => T::Successor(Box::new(self.ty(inner, span)?)),
            T::Enum(id) => T::Enum(self.enumeration(*id, &[], span)?),
            T::Variant(id, args) => T::Variant(self.enumeration(*id, args, span)?, vec![]),
            T::Param(_) => return Err(LowerError::unsupported(span,
                "reachable generic is not closed after substitution")),
            _ => value.clone(),
        })
    }

    fn block(&mut self, block: &mut thir::Block, env: &[(ty::ParamId, ty::Ty)]) -> Result<()> {
        for stmt in &mut block.stmts {
            self.stmt(stmt, env)?;
        }
        Ok(())
    }

    fn stmt(&mut self, stmt: &mut thir::Stmt, env: &[(ty::ParamId, ty::Ty)]) -> Result<()> {
        use thir::StmtKind as S;
        match &mut stmt.kind {
            S::Let { value, .. } | S::Expr(value) | S::Throw(value) | S::BindField { value, .. }
            | S::StageField { value, .. } | S::Rebind { value, .. } | S::Defer(value) => self.expr(value, env)?,
            S::Return(value) | S::Last(value) => {
                if let Some(value) = value {
                    self.expr(value, env)?;
                }
            }
            S::Guarded { condition, stmt } => {
                self.expr(condition, env)?;
                self.stmt(stmt, env)?;
            }
            S::Confirm { condition, fallback } => {
                self.expr(condition, env)?;
                if let Some(thir::Fallback::Throw(value) | thir::Fallback::Panic(value)) = fallback {
                    self.expr(value, env)?;
                }
            }
            S::Rethrow | S::Next => {}
        }
        Ok(())
    }

    fn expr(&mut self, expr: &mut thir::Expr, env: &[(ty::ParamId, ty::Ty)]) -> Result<()> {
        use thir::ExprKind as E;
        let unsupported = match &expr.kind {
            E::Functional(_) => Some("functional collection operation (C7)"),
            E::Closure { .. } | E::BoundAction { .. } | E::BoundRoleAction { .. }
            | E::CallValue { .. } | E::CapSelect { .. } | E::CapRead { .. } => Some("closure or capability (#569)"),
            E::Await(_) | E::Spawn(_) => Some("async operation (#570)"),
            E::StreamWrite { .. } => Some("stream operation (#569)"),
            _ => None,
        };
        if let Some(construct) = unsupported {
            return Err(LowerError::unsupported(expr.span, construct));
        }
        expr.ty = expr.ty.as_ref().map(|ty| self.ty(&ty.subst(env), expr.span)).transpose()?;
        match &mut expr.kind {
            E::Call { callee, args } => {
                match callee {
                    thir::Callee::Method { receiver, object, method } => {
                        let original = *object;
                        let args = match receiver.ty.as_ref().map(|ty| ty.taken().subst(env)) {
                            Some(ty::Ty::Object(_, args)) => args,
                            _ => vec![],
                        };
                        let instance = self.instance(original, &args, expr.span)?;
                        self.expr(receiver, env)?;
                        *object = instance;
                        let source = &self.input.registry.objects[original.0];
                        let subst = source.params.iter().copied().zip(args).collect();
                        self.queue(thir::BodyOwner::Method { object: original, method: *method },
                            thir::BodyOwner::Method { object: instance, method: *method }, subst, None, expr.span)?;
                    }
                    thir::Callee::Ctor { object, type_args, ctor, given } => {
                        let original = *object;
                        let args: Vec<_> = type_args.iter().map(|ty| ty.subst(env)).collect();
                        let instance = self.instance(original, &args, expr.span)?;
                        *object = instance;
                        type_args.clear();
                        let source = &self.input.registry.objects[original.0];
                        let subst = source.params.iter().copied().zip(args).collect();
                        self.queue(thir::BodyOwner::Ctor { object: original, ctor: *ctor },
                            thir::BodyOwner::Ctor { object: instance, ctor: *ctor },
                            subst, Some(given.clone()), expr.span)?;
                    }
                    thir::Callee::Role { receiver, role, .. } => {
                        let args = match receiver.ty.as_ref().map(|ty| ty.taken().subst(env)) {
                            Some(ty::Ty::Role(_, args)) => args,
                            Some(receiver) => self.input.registry.roles_of(&receiver).into_iter().find_map(|ty| {
                                match ty {
                                    ty::Ty::Role(id, args) if id == *role => Some(args),
                                    _ => None,
                                }
                            }).unwrap_or_default(),
                            None => vec![],
                        };
                        *role = self.role(*role, &args, expr.span)?;
                        self.expr(receiver, env)?;
                        if let Some(ty::Ty::Object(object, _)) = receiver.ty.as_ref().map(ty::Ty::taken) {
                            let role_info = &self.registry.roles[role.0];
                            let thir::Callee::Role { slot, .. } = callee else {
                                unreachable!("role callee");
                            };
                            let action = role_info.methods.get(*slot).ok_or_else(|| {
                                LowerError::invalid(expr.span, "unknown role slot")
                            })?;
                            let method = self.registry.objects[object.0].methods.iter().position(|method| {
                                method.name == action.name && method.params.iter().map(|(_, ty)| ty)
                                    .eq(action.params.iter().map(|(_, ty)| ty))
                            }).ok_or_else(|| {
                                LowerError::invalid(expr.span, "missing specialized role implementation")
                            })?;
                            let thir::Callee::Role { receiver, .. } = callee else {
                                unreachable!("role receiver");
                            };
                            *callee = thir::Callee::Method { receiver: receiver.clone(), object, method };
                        }
                    }
                    thir::Callee::Builtin { receiver, .. } => self.expr(receiver, env)?,
                    _ => {}
                }
                for arg in args {
                    self.expr(arg, env)?;
                }
            }
            E::Unary { operand: value, .. } | E::Coerce { value, .. } | E::Cast { value, .. }
            | E::Panic(value) | E::Unpack(value) | E::VariantTag(value) | E::VariantGet { value, .. } => {
                self.expr(value, env)?;
            }
            E::Binary { left, right, .. } | E::Logical { left, right, .. }
            | E::Range { start: left, end: right, .. } => {
                self.expr(left, env)?;
                self.expr(right, env)?;
            }
            E::Array(parts) => {
                for part in parts {
                    match part {
                        thir::ArrayPart::Value(value) => self.expr(value, env)?,
                        thir::ArrayPart::Range { start, end, .. } => {
                            self.expr(start, env)?;
                            self.expr(end, env)?;
                        }
                    }
                }
            }
            E::Map(entries) => {
                for (key, value) in entries {
                    self.expr(key, env)?;
                    self.expr(value, env)?;
                }
            }
            E::ArrayCast { parts, .. } => {
                for part in parts {
                    let (thir::CastPart::One(value) | thir::CastPart::Spread(value)) = part;
                    self.expr(value, env)?;
                }
            }
            E::Index { object, index, fallback } => {
                self.expr(object, env)?;
                self.expr(index, env)?;
                if let Some(value) = fallback {
                    self.expr(value, env)?;
                }
            }
            E::RatNew { numerator, denominator } => {
                self.expr(numerator, env)?;
                self.expr(denominator, env)?;
            }
            E::If { condition, then_block, else_block } => {
                self.expr(condition, env)?;
                self.block(then_block, env)?;
                if let Some(block) = else_block {
                    self.block(block, env)?;
                }
            }
            E::Block(block) | E::Loop(block) => self.block(block, env)?,
            E::For { source, body, .. } => {
                match source {
                    thir::Source::Range { start, end, .. } => {
                        self.expr(start, env)?;
                        self.expr(end, env)?;
                    }
                    thir::Source::Array(value) => self.expr(value, env)?,
                    _ => {}
                }
                self.block(body, env)?;
            }
            E::Try(found) => {
                self.block(&mut found.body, env)?;
                for catch in &mut found.catches {
                    self.block(&mut catch.body, env)?;
                }
            }
            E::Match(found) => {
                self.expr(&mut found.subject, env)?;
                for (patterns, block) in &mut found.cases {
                    for pattern in patterns {
                        self.expr(pattern, env)?;
                    }
                    self.block(block, env)?;
                }
                if let Some(block) = &mut found.else_block {
                    self.block(block, env)?;
                }
            }
            E::VariantNew { values, .. } | E::Pack { items: values, spread: None } => {
                for value in values {
                    self.expr(value, env)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// Structural spelling shared by concrete signatures and specialization arguments.
pub(super) fn type_name(registry: &Registry<'_>, value: &ty::Ty) -> Result<String> {
    use ty::Ty as T;
    let nominal = |kind: &str, module: usize, name: &str, args: &[T]| -> Result<String> {
        let args = args.iter().map(|ty| type_name(registry, ty)).collect::<Result<Vec<_>>>()?;
        let arguments = if args.is_empty() { String::new() } else { format!("<{}>", args.join(",")) };
        Ok(format!("{kind}({}::{name}{arguments})", registry.modules[module].name))
    };
    Ok(match value {
        T::Int => "int".into(),
        T::Str => "string".into(),
        T::Bool => "bool".into(),
        T::Byte => "byte".into(),
        T::Char => "char".into(),
        T::Float => "float".into(),
        T::Rat => "rat".into(),
        T::Range => "range".into(),
        T::Any => "any".into(),
        T::Never => "never".into(),
        T::Array(inner) => format!("array<{}>", type_name(registry, inner)?),
        T::Map(key, value) => format!("map<{},{}>", type_name(registry, key)?, type_name(registry, value)?),
        T::Successor(inner) => format!("successor<{}>", type_name(registry, inner)?),
        T::Varargs(inner) => format!("varargs<{}>", type_name(registry, inner)?),
        T::Object(id, args) => {
            let info = &registry.objects[id.0];
            nominal("object", info.module, &info.name, args)?
        }
        T::Variant(id, args) => {
            let info = &registry.enums[id.0];
            nominal("variant", info.module, &info.name, args)?
        }
        T::Enum(id) => {
            let info = &registry.enums[id.0];
            nominal("enum", info.module, &info.name, &[])?
        }
        T::Role(id, args) => {
            let info = &registry.roles[id.0];
            nominal("role", info.module, &info.name, args)?
        }
        T::Action(action) => {
            let params = action.params.iter().map(|ty| type_name(registry, ty)).collect::<Result<Vec<_>>>()?;
            let result = action.result.as_ref().map(|ty| type_name(registry, ty)).transpose()?
                .unwrap_or_else(|| "void".into());
            let effect = if action.is_async { "async" } else { "sync" };
            format!("action<{effect};({})->{result}>", params.join(","))
        }
        T::Future(result) => {
            let result = result.as_ref().map(|ty| type_name(registry, ty)).transpose()?
                .unwrap_or_else(|| "void".into());
            format!("future<{result}>")
        }
        T::Handle(id) => {
            let info = &registry.handles[id.0];
            nominal("handle", info.module, &info.name, &[])?
        }
        T::Cap(id, args, index) => {
            let info = &registry.objects[id.0];
            format!("cap<{};{}>", nominal("object", info.module, &info.name, args)?, info.caps[*index].name)
        }
        T::Test => "test".into(),
        T::Param(_) | T::Error => {
            return Err(LowerError::unsupported(Span::default(), "non-concrete specialization argument"));
        }
    })
}

