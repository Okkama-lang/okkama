//! Demand-driven concrete THIR specialization for the high producer.
//! Ordinary std bodies remain THIR and use the same high producer as application bodies.
use std::collections::{HashMap, HashSet, VecDeque};

use crate::analysis::{Analysis, Registry, SourceModule, registry, thir, ty};
use crate::source::Span;
use super::{LowerError, Result};
use super::callables::{self, BodyKey, Capture, Snapshot};

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

pub(super) fn prepare<'a>(input: &Analysis<'a>) -> Result<callables::Prepared<'a>> {
    prepare_with_limits(input, Limits::default())
}

pub(super) fn prepare_with_limits<'a>(input: &Analysis<'a>, limits: Limits) -> Result<callables::Prepared<'a>> {
    let sources = input.registry.modules.iter().map(|module| SourceModule {
        path: module.path.clone(),
        program: module.program,
        imports: module.imports.clone(),
    }).collect();
    let mut registry = Registry::build_mode(
        sources, input.registry.modules.len() - input.registry.root_start, input.registry.test_mode,
    );
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
        inventory: callables::Inventory {
            #[cfg(test)]
            test_mode: input.registry.test_mode,
            ..Default::default()
        },
        lexical: None,
        nested: HashSet::new(),
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
        for (cap, info) in source.caps.iter().enumerate() {
            for (field, (_, ty, _)) in info.fields.iter().enumerate() {
                slice.registry.objects[index].caps[cap].fields[field].1 = slice.ty(ty, info.span)?;
            }
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
    while let Some(job) = slice.pending.pop_front() {
        let BodyInstance { source: owner, target, substitution: env, chain, span, nested } = job;
        slice.chain = chain;
        let (nested_body, key, receiver, captures) = match nested {
            Some(context) => (Some(context.body), context.key, context.receiver, context.captures),
            None => {
                let receiver = owner_object(&target).map(ty::Ty::object);
                (None, BodyKey { root: target.clone(), path: vec![] }, receiver, vec![])
            }
        };
        let mut body = nested_body.or_else(|| input.bodies.iter().find(|body| body.owner == owner).cloned())
            .ok_or_else(|| {
                LowerError::unsupported(span, format!("missing source body for {}",
                    slice.chain.last().expect("request chain")))
            })?;
        if let thir::BodyOwner::Method { object, method } = target {
            let original = owner_object(&owner).expect("method owner");
            let source = &input.registry.objects[original.0].methods[method];
            let params = source.params.iter().map(|(name, ty)| {
                Ok((name.clone(), slice.ty(&ty.subst(&env), body.span)?))
            }).collect::<Result<Vec<_>>>()?;
            slice.registry.objects[object.0].methods[method].params = params;
            let result = source.result.as_ref();
            slice.registry.objects[object.0].methods[method].result = result
                .map(|ty| slice.ty(&ty.subst(&env), body.span)).transpose()?;
        }
        body.owner = target;
        if let thir::BodyOwner::Method { object, method } = body.owner {
            for stmt in &body.block.stmts {
                if let thir::StmtKind::Let { local, once: true, .. } = stmt.kind {
                    let value = body.locals[local.0].ty.subst(&env);
                    let ty = slice.ty(&ty::Ty::Variant(ty::EnumId(0), vec![value]), stmt.span)?;
                    slice.registry.objects[object.0].fields.push(registry::FieldInfo {
                        name: format!("$once{method}_{}", local.0),
                        ty,
                        default: None,
                        span: stmt.span,
                    });
                }
            }
        }
        for local in &mut body.locals {
            local.ty = slice.ty(&local.ty.subst(&env), local.span)?;
        }
        slice.lexical = Some(Lexical {
            key: key.clone(), receiver: receiver.clone(), locals: body.locals.clone(), params: body.params.clone(),
            module: body.module, next_closure: 0, next_open_call: 0,
        });
        slice.block(&mut body.block, &env)
            .map_err(|error| error.in_function(&slice.body_name(&body.owner)))?;
        // Bindings introduced by desugaring (stream element loops) follow the source locals.
        let lexical = slice.lexical.as_ref().expect("lexical context of the walked body");
        body.locals.extend(lexical.locals[body.locals.len()..].iter().cloned());
        slice.inventory.bodies.push(callables::DiscoveredBody {
            key,
            #[cfg(test)]
            receiver,
            captures,
            body: body.clone(),
        });
        if body.owner != thir::BodyOwner::Closure {
            slice.bodies.push(body);
        }
    }
    slice.inventory.attach(&mut slice.bodies);
    let analysis = Analysis { registry: slice.registry, bodies: slice.bodies, warnings: input.warnings.clone() };
    Ok(callables::Prepared { analysis, callables: slice.inventory })
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
    inventory: callables::Inventory,
    lexical: Option<Lexical>,
    nested: HashSet<BodyKey>,
}

struct Lexical {
    key: BodyKey,
    receiver: Option<ty::Ty>,
    locals: Vec<thir::Local>,
    params: Vec<thir::LocalId>,
    module: usize,
    next_closure: usize,
    next_open_call: usize,
}

struct NestedBody {
    key: BodyKey,
    body: thir::Body,
    receiver: Option<ty::Ty>,
    captures: Vec<Capture>,
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
    nested: Option<NestedBody>,
}

pub(super) fn owner_object(owner: &thir::BodyOwner) -> Option<ty::ObjectId> {
    match owner {
        thir::BodyOwner::Method { object, .. } | thir::BodyOwner::Ctor { object, .. }
        | thir::BodyOwner::Default { object, .. } | thir::BodyOwner::Destroy { object }
        | thir::BodyOwner::CapField { object, .. } => Some(*object),
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
        self.pending.push_back(BodyInstance { source: owner, target, substitution: env, chain, span, nested: None });
        Ok(())
    }

    fn body_name(&self, owner: &thir::BodyOwner) -> String {
        let Some(object) = owner_object(owner) else {
            return match owner {
                thir::BodyOwner::Entry => "main".into(),
                thir::BodyOwner::Test(index) => format!("test.{}", self.registry.tests[*index].name),
                thir::BodyOwner::Closure => "closure".into(),
                _ => unreachable!("nominal body"),
            };
        };
        let info = &self.registry.objects[object.0];
        let member = match owner {
            thir::BodyOwner::Method { method, .. } => info.methods[*method].name.clone(),
            thir::BodyOwner::Ctor { .. } => "new".into(),
            thir::BodyOwner::Default { field, .. } => format!("default.{}", info.fields[*field].name),
            thir::BodyOwner::Destroy { .. } => "destroy".into(),
            thir::BodyOwner::CapField { cap, field, .. } => {
                format!("cap.{}.{}", info.caps[*cap].name, info.caps[*cap].fields[*field].0)
            }
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

    /// A std object with the registered direct-call ABI of P1 (#566): its witnesses are requested on demand.
    fn finite_boundary(&self, object: ty::ObjectId) -> bool {
        let source = &self.input.registry.objects[object.0];
        let module = self.input.registry.modules[source.module].name.as_str();
        source.module < self.registry.root_start
            && matches!((module, source.name.as_str()), ("io/stdout", "Stdout") | ("encoding/utf", "Utf8Encoded"))
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
        let module = self.input.registry.modules[source.module].name.as_str();
        let generic = matches!((module, source.name.as_str()), ("async/atomic", "Atomic") | ("cell", "Cell"));
        let ordinary = self.finite_boundary(object);
        let imported = source.module < self.registry.root_start;
        // Registered Atomic/Cell specializations (P2.4 §4.8): plain scalars and arrays of them, and role values
        // (#764), which the SPB 17 loader admits only for plain roles. Any other managed value needs the storage
        // protocol of the registered service and stays unsupported.
        let suffix = match args {
            [value] if generic => self.atomic_suffix_of(value, span)?,
            _ => None,
        };
        let plain_value = suffix.is_some();
        if imported && (source.decl.is_none() || generic && !plain_value) {
            return Err(LowerError::unsupported(span, format!("std object {module}.{}<{args:?}>", source.name)));
        }
        // The errors the runtime throws for a std object's intrinsics are declared with it: AtomicClosed, and
        // BrokenBarrier of the std async primitives (#601 Q1).
        let thrown = match module {
            "async/atomic" => Some("AtomicClosed"),
            "async/barrier" => Some("BrokenBarrier"),
            _ => None,
        };
        if let Some(thrown) = thrown {
            for (index, enumeration) in self.input.registry.enums.iter().enumerate() {
                if enumeration.module == source.module && enumeration.name == thrown {
                    self.registry.enums[index].span = enumeration.span;
                }
            }
        }
        let specialized = !source.params.is_empty();
        let instance = if specialized { ty::ObjectId(self.registry.objects.len()) } else { object };
        let type_names = args.iter().map(|ty| self.encode(ty)).collect::<Result<Vec<_>>>()?;
        let name = if imported && generic && let Some(suffix) = &suffix {
            format!("{}_{suffix}", source.name)
        } else if imported && ordinary {
            source.name.clone()
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
            caps: source.caps.clone(),
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
        for cap in &mut info.caps {
            for (_, ty, _) in &mut cap.fields {
                *ty = self.ty(&ty.subst(&env), cap.span)?;
            }
        }
        // Method signatures are substituted when their body is requested. This
        // avoids discovering an infinite family through an unused method result.
        self.registry.objects[instance.0] = info;
        for (cap, info) in source.caps.iter().enumerate() {
            for field in 0..info.fields.len() {
                self.queue(thir::BodyOwner::CapField { object, cap, field },
                    thir::BodyOwner::CapField { object: instance, cap, field }, env.clone(), None, info.span)?;
            }
        }
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

    /// The registered Atomic suffix of `value` (P2.4 §4.8): a plain scalar or array suffix, or for a role value
    /// `role_` and the role's declaration name in the module (#764).
    fn atomic_suffix_of(&mut self, value: &ty::Ty, span: Span) -> Result<Option<String>> {
        let ty::Ty::Role(role, args) = value else {
            if let Some(suffix) = atomic_suffix(value) {
                return Ok(Some(suffix));
            }
            // Other closed plain data (#778): maps, objects and variants, arrays of handles and of roles. The
            // value type is read from the specialization's operations; the loader admits it only as closed
            // plain data (ATOMIC_PLAIN), or a Future and arrays of Futures as borrowed views (PR-A, ATOMIC_BORROW).
            // An action value, `any` or a cap is never stored.
            let storable = atomic_storable(value, &|object| self.input.registry.objects[object.0].destroy.is_none());
            return Ok(storable.then(|| self.encode(value)).transpose()?.map(|name| format!("t_{name}")));
        };
        let id = self.role(*role, args, span)?;
        let info = &self.registry.roles[id.0];
        if info.decl.is_none() {
            return Ok(None);
        }
        let module = &self.registry.modules[info.module].name;
        Ok(Some(format!("role_{}", role_declaration_name(module, &info.name))))
    }

    fn role(&mut self, role: ty::RoleId, args: &[ty::Ty], span: Span) -> Result<ty::RoleId> {
        if let Some(&target) = self.roles.get(&(role, args.to_vec())) {
            return Ok(target);
        }
        let source = &self.input.registry.roles[role.0];
        if self.input.registry.modules[source.module].name == "collections/iteration" && source.name == "Cursor" {
            for (index, info) in self.input.registry.enums.iter().enumerate() {
                if info.module == source.module && info.name == "IterationEnded" {
                    self.enumeration(ty::EnumId(index), &[], span)?;
                }
            }
        }
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
            T::Cap(object, args, cap) => T::Cap(self.instance(*object, args, span)?, vec![], *cap),
            T::Action(action) => T::Action(Box::new(ty::ActionTy {
                is_async: action.is_async,
                params: action.params.iter().map(|ty| self.ty(ty, span)).collect::<Result<_>>()?,
                result: action.result.as_ref().map(|ty| self.ty(ty, span)).transpose()?,
            })),
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
            T::Future(result) => T::Future(result.as_ref().map(|ty| self.ty(ty, span).map(Box::new)).transpose()?),
            T::Enum(id) => T::Enum(self.enumeration(*id, &[], span)?),
            T::Variant(id, args) => T::Variant(self.enumeration(*id, args, span)?, vec![]),
            T::Param(_) => return Err(LowerError::unsupported(span,
                "reachable generic is not closed after substitution")),
            _ => value.clone(),
        })
    }

    fn bound(&mut self, receiver: &thir::Expr, target: callables::BoundTarget, span: Span) -> Result<()> {
        let receiver = receiver.ty.clone().ok_or_else(|| LowerError::invalid(span, "untyped bound receiver"))?;
        self.inventory.bound.push(callables::BoundAction {
            body: self.lexical.as_ref().expect("lexical body").key.clone(), span, receiver, target,
        });
        Ok(())
    }

    fn closure(&mut self, body: &thir::Body, captures: &[(thir::LocalId, thir::LocalId)],
        env: &[(ty::ParamId, ty::Ty)], span: Span) -> Result<()> {
        let lexical = self.lexical.as_mut().expect("lexical body");
        let mut key = lexical.key.clone();
        key.path.push(lexical.next_closure);
        lexical.next_closure += 1;
        if !self.nested.insert(key.clone()) {
            return Ok(());
        }
        let receiver = lexical.receiver.clone();
        let mut layout = vec![];
        if thir::uses_receiver(body) {
            let ty = receiver.clone().ok_or_else(|| LowerError::invalid(span, "closure lacks lexical receiver"))?;
            layout.push(Capture { source: Snapshot::Receiver, target: None, ty });
        }
        for (outer, inner) in captures {
            let source = if lexical.params.contains(outer) {
                Snapshot::Parameter(*outer)
            } else {
                Snapshot::Local(*outer)
            };
            let ty = lexical.locals[outer.0].ty.clone();
            layout.push(Capture { source, target: Some(*inner), ty });
        }
        let name = format!("{}::closure{:?}", self.body_name(&key.root), key.path);
        self.request(name.clone(), span)?;
        let mut chain = self.chain.clone();
        chain.push(name);
        let nested = NestedBody { key, body: body.clone(), receiver, captures: layout };
        self.pending.push_back(BodyInstance {
            source: thir::BodyOwner::Closure, target: thir::BodyOwner::Closure,
            substitution: env.to_vec(), chain, span, nested: Some(nested),
        });
        Ok(())
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

    /// A compiler binding of the body being walked, with its concrete type.
    fn fresh_local(&mut self, ty: ty::Ty, span: Span, env: &[(ty::ParamId, ty::Ty)]) -> Result<thir::LocalId> {
        let ty = self.ty(&ty.subst(env), span)?;
        let lexical = self.lexical.as_mut().expect("lexical context of the walked body");
        lexical.locals.push(thir::Local { name: "$item".into(), ty, rebindable: false, variadic: false, span });
        Ok(thir::LocalId(lexical.locals.len() - 1))
    }

    fn expr(&mut self, expr: &mut thir::Expr, env: &[(ty::ParamId, ty::Ty)]) -> Result<()> {
        use thir::ExprKind as E;
        if let Some(construct) = super::asynchronous::outside_plain(expr) {
            return Err(LowerError::unsupported(expr.span, construct));
        }
        if let E::StreamWrite { .. } = &expr.kind {
            let roles = super::streams::StreamRoles::find(&self.input.registry, expr.span)?;
            super::streams::desugar(expr, &roles, &mut |ty, span| self.fresh_local(ty, span, env))?;
        }
        expr.ty = expr.ty.as_ref().map(|ty| self.ty(&ty.subst(env), expr.span)).transpose()?;
        match &mut expr.kind {
            E::Closure { body, captures } => self.closure(body, captures, env, expr.span)?,
            E::BoundAction { receiver, object, method } => {
                let original = *object;
                let args = match receiver.ty.as_ref().map(|ty| ty.taken().subst(env)) {
                    Some(ty::Ty::Object(_, args)) => args,
                    _ => vec![],
                };
                *object = self.instance(original, &args, expr.span)?;
                self.expr(receiver, env)?;
                let source = &self.input.registry.objects[original.0];
                let subst = source.params.iter().copied().zip(args).collect();
                self.queue(thir::BodyOwner::Method { object: original, method: *method },
                    thir::BodyOwner::Method { object: *object, method: *method }, subst, None, expr.span)?;
                self.bound(receiver, callables::BoundTarget::Method { object: *object, method: *method }, expr.span)?;
            }
            E::BoundRoleAction { receiver, role, slot } => {
                let args = match receiver.ty.as_ref().map(|ty| ty.taken().subst(env)) {
                    Some(ty::Ty::Role(_, args)) => args,
                    Some(receiver) => self.input.registry.roles_of(&receiver).into_iter().find_map(|ty| match ty {
                        ty::Ty::Role(id, args) if id == *role => Some(args),
                        _ => None,
                    }).unwrap_or_default(),
                    None => vec![],
                };
                *role = self.role(*role, &args, expr.span)?;
                self.expr(receiver, env)?;
                self.bound(receiver, callables::BoundTarget::Role { role: *role, slot: *slot }, expr.span)?;
            }
            E::CallValue { callee, args } => {
                self.expr(callee, env)?;
                let ty = callee.ty.clone().ok_or_else(|| LowerError::invalid(expr.span, "untyped open call"))?;
                if !matches!(ty, ty::Ty::Action(_)) || ty.mentions_param() {
                    return Err(LowerError::invalid(expr.span, "non-concrete open callable type"));
                }
                let lexical = self.lexical.as_mut().expect("lexical body");
                let site = callables::OpenCallSite {
                    body: lexical.key.clone(), module: lexical.module, span: expr.span,
                    ordinal: lexical.next_open_call,
                };
                lexical.next_open_call += 1;
                self.inventory.open_call(ty, site);
                for arg in args {
                    self.expr(arg, env)?;
                }
            }
            E::CapSelect { owner, object, .. } => {
                self.expr(owner, env)?;
                let Some(ty::Ty::Object(concrete, _)) = owner.ty.as_ref().map(ty::Ty::taken) else {
                    return Err(LowerError::invalid(expr.span, "non-object cap owner"));
                };
                *object = concrete;
            }
            E::CapRead { cap, object, .. } => {
                self.expr(cap, env)?;
                let Some(ty::Ty::Cap(concrete, _, _)) = cap.ty.as_ref() else {
                    return Err(LowerError::invalid(expr.span, "non-cap computed field"));
                };
                *object = *concrete;
            }
            E::Call { callee, args } => {
                match callee {
                    // The system NativeFailure's getters are field reads and Cancelled's text() a constant: no body
                    // to specialize.
                    thir::Callee::Method { receiver, object, .. }
                        if *object == self.input.registry.system.native_failure
                            || *object == self.input.registry.system.cancelled => {
                        self.expr(receiver, env)?;
                    }
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
                            // Objects with a registered direct ABI (Stdout) have no requested
                            // witnesses, so the devirtualized method is requested here.
                            let (original, args) = self.instances.iter()
                                .find_map(|(key, &instance)| (instance == object).then(|| key.clone()))
                                .unwrap_or((object, vec![]));
                            let source = &self.input.registry.objects[original.0];
                            let subst = source.params.iter().copied().zip(args).collect();
                            self.queue(thir::BodyOwner::Method { object: original, method },
                                thir::BodyOwner::Method { object, method }, subst, None, expr.span)?;
                        }
                    }
                    thir::Callee::Builtin { receiver, .. } => self.expr(receiver, env)?,
                    _ => {}
                }
                for arg in args {
                    self.expr(arg, env)?;
                }
            }
            // A finite P1 boundary (Stdout, Utf8Encoded) gets its witnesses only when a value of it becomes a role
            // value: std wraps them too (`fs/temp.ok` writes text as a ByteSource), not only roots.
            E::Coerce { coercion: thir::Coercion::RoleWrap, value } => {
                self.expr(value, env)?;
                if let Some(ty::Ty::Object(object, args)) = value.ty.as_ref().map(|ty| ty.taken().subst(env))
                    && args.is_empty()
                    && self.registry.objects[object.0].does.is_empty()
                    && self.finite_boundary(object)
                {
                    self.registry.objects[object.0].does = self.input.registry.objects[object.0].does.clone();
                    self.witnesses(object, object, &[], expr.span)?;
                }
            }
            E::Unary { operand: value, .. } | E::Coerce { value, .. } | E::Cast { value, .. }
            | E::Panic(value) | E::Unpack(value) | E::VariantTag(value) | E::VariantGet { value, .. }
            | E::Await(value) | E::Spawn(value) => {
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
            E::Functional(functional) => {
                match &mut functional.source {
                    thir::Source::Range { start, end, .. } => {
                        self.expr(start, env)?;
                        self.expr(end, env)?;
                    }
                    thir::Source::Array(value) | thir::Source::Map(value)
                    | thir::Source::Iterating(value) | thir::Source::Batching(value) => self.expr(value, env)?,
                    thir::Source::Varargs(_) => {}
                }
                if let Some(initial) = &mut functional.initial {
                    self.expr(initial, env)?;
                }
                if let Some(found) = &mut functional.found {
                    self.expr(found, env)?;
                }
                self.block(&mut functional.body, env)?;
            }
            E::For { source, body, .. } => {
                match source {
                    thir::Source::Range { start, end, .. } => {
                        self.expr(start, env)?;
                        self.expr(end, env)?;
                    }
                    thir::Source::Array(value) | thir::Source::Map(value)
                    | thir::Source::Iterating(value) | thir::Source::Batching(value) => self.expr(value, env)?,
                    thir::Source::Varargs(_) => {}
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
            E::Pack { items, spread } => {
                for value in items {
                    self.expr(value, env)?;
                }
                if let Some(value) = spread {
                    self.expr(value, env)?;
                }
            }
            E::VariantNew { values, .. } => {
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
/// The name suffix of a registered plain Atomic/Cell value (P2.4 §4.8): `int`, `string`, `bool`, `byte`,
/// `char`, `float`, `rat`, or `array_` and the suffix of a plain element. Other values have none.
/// Whether a value of `value` may be the content of a registered Atomic beyond the scalar suffixes (#778): data
/// made of scalars, maps, arrays, objects, variants, enums, roles and native handles. Futures (D25), action
/// values, `any`, caps, successors and parameters are not, nor an object with a destroy (`plain_object`); a
/// resource deeper in the graph is refused when the layouts are known, and by the loader (ATOMIC_PLAIN).
fn atomic_storable(value: &ty::Ty, plain_object: &dyn Fn(ty::ObjectId) -> bool) -> bool {
    use ty::Ty as T;
    let storable = |value: &ty::Ty| atomic_storable(value, plain_object);
    match value {
        T::Int | T::Str | T::Bool | T::Byte | T::Char | T::Float | T::Rat | T::Range | T::Handle(_) => true,
        T::Array(item) => storable(item),
        T::Map(key, item) => storable(key) && storable(item),
        T::Object(object, args) => plain_object(*object) && args.iter().all(storable),
        T::Role(_, args) | T::Variant(_, args) => args.iter().all(storable),
        T::Enum(_) => true,
        // PR-A (D25 tail): an Atomic borrows a Future, whatever its outcome type. It never holds the duty; the
        // loader admits it as a borrowed view (`ATOMIC_BORROW`).
        T::Future(_) => true,
        _ => false,
    }
}

/// A role's declaration name in the OIR-2 module, as `Declarations::new` gives it.
pub(super) fn role_declaration_name(module: &str, name: &str) -> String {
    if name.contains('<') {
        format!("{module}::{name}")
    } else {
        format!("{}.{name}", module.replace('/', "."))
    }
}

pub(super) fn atomic_suffix(value: &ty::Ty) -> Option<String> {
    use ty::Ty as T;
    Some(match value {
        T::Int => "int".into(),
        T::Str => "string".into(),
        T::Bool => "bool".into(),
        T::Byte => "byte".into(),
        T::Char => "char".into(),
        T::Float => "float".into(),
        T::Rat => "rat".into(),
        T::Array(item) => format!("array_{}", atomic_suffix(item)?),
        _ => return None,
    })
}

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

