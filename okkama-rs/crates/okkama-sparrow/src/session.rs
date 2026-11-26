//! #588 phase B: the program of a REPL session, append-only.
//!
//! Each input recompiles the whole session (its declarations and every cell) with the ordinary compiler, which
//! numbers items its own way. [`extend`] translates that fresh program into the published program's numbering by
//! stable identities, so a live runtime can take it (`Runtime::extend`):
//! - functions by symbol (and occurrence, for a repeated symbol);
//! - objects, roles, enums, variants and caps by name;
//! - handles by text;
//! - globals by their `init` function's symbol;
//! - types by structure.
//!
//! A published entry must translate to exactly what was published: any difference is an error, never a silent
//! renumbering. Everything new is appended in the fresh program's order. Published entries the fresh program
//! does not mention (the runtime's synthetic workers, items of earlier cells) stay where they are.
use crate::operand::Access;
use crate::{
    CapDecl, CatchKind, EnumDecl, FuncRef, Function, GlobalDecl, Instruction, Intrinsic, ObjectDecl,
    ObjectRef, Program, RoleDecl, RoleMethod, Type, TypeRef, VariantDecl, Witness,
};
use std::collections::HashMap;

/// Why a fresh program cannot extend the published one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionError(pub String);

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Fresh index → result index, per table.
#[derive(Default)]
struct Maps {
    types: Vec<u32>,
    objects: Vec<u32>,
    roles: Vec<u32>,
    enums: Vec<u32>,
    variants: Vec<u32>,
    caps: Vec<u32>,
    handles: Vec<u32>,
    globals: Vec<u32>,
    functions: Vec<u32>,
}

/// Map each fresh item to the published one with the same key, or to a new index after the published ones.
/// A key the fresh program repeats is an error, also when it is new (#975): a new key is known from its first
/// occurrence on, so two equal new names never get two indices.
fn by_key<T, K: std::hash::Hash + Eq + std::fmt::Debug>(
    published: &[T],
    fresh: &[T],
    kind: &str,
    key: impl Fn(&T) -> K,
) -> Result<Vec<u32>, SessionError> {
    let mut known: HashMap<K, u32> = HashMap::new();
    for (index, item) in published.iter().enumerate() {
        known.entry(key(item)).or_insert(index as u32);
    }
    let mut next = published.len() as u32;
    // Result index → the fresh entry that took it.
    let mut seen: HashMap<u32, usize> = HashMap::new();
    fresh.iter().enumerate().map(|(position, item)| {
        let k = key(item);
        let index = match known.get(&k) {
            Some(&index) => index,
            None => {
                let index = next;
                next += 1;
                index
            }
        };
        if let Some(first) = seen.insert(index, position) {
            return Err(SessionError(format!(
                "the session program repeats the {kind} name {k:?} (entries {first} and {position})"
            )));
        }
        known.entry(k).or_insert(index);
        Ok(index)
    }).collect()
}

/// Functions by (symbol, occurrence of that symbol).
fn function_keys(functions: &[Function]) -> Vec<(String, usize)> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    functions.iter().map(|f| {
        let count = counts.entry(&f.symbol).or_insert(0);
        *count += 1;
        (f.symbol.clone(), *count)
    }).collect()
}

/// Extend `published` with what `fresh` adds, in `published`'s numbering.
pub fn extend(published: &Program, fresh: &Program) -> Result<Program, SessionError> {
    let mut maps = Maps {
        objects: by_key(&published.objects, &fresh.objects, "object", |o| o.name.clone())?,
        roles: by_key(&published.roles, &fresh.roles, "role", |r| r.name.clone())?,
        enums: by_key(&published.enums, &fresh.enums, "enum", |e| e.name.clone())?,
        variants: by_key(&published.variants, &fresh.variants, "variant", |v| v.name.clone())?,
        caps: by_key(&published.caps, &fresh.caps, "cap", |c| c.name.clone())?,
        handles: by_key(&published.handles, &fresh.handles, "handle", Clone::clone)?,
        ..Maps::default()
    };
    let published_keys = function_keys(&published.functions);
    let fresh_keys = function_keys(&fresh.functions);
    maps.functions = by_key(&published_keys, &fresh_keys, "function", Clone::clone)?;
    let symbol = |program: &Program, global: &GlobalDecl| program.functions[global.init.0 as usize].symbol.clone();
    let published_globals: Vec<String> = published.globals.iter().map(|g| symbol(published, g)).collect();
    let fresh_globals: Vec<String> = fresh.globals.iter().map(|g| symbol(fresh, g)).collect();
    maps.globals = by_key(&published_globals, &fresh_globals, "constant", Clone::clone)?;

    let mut out = published.clone();
    // Types by structure, after their parts: a type's components are other types, objects, roles, enums,
    // variants, caps and handles, all mapped by now or resolved in earlier rounds.
    let mut index: HashMap<Type, u32> = HashMap::new();
    for (i, ty) in out.types.iter().enumerate() {
        index.entry(ty.clone()).or_insert(i as u32);
    }
    maps.types = vec![u32::MAX; fresh.types.len()];
    loop {
        let mut progress = false;
        for (i, ty) in fresh.types.iter().enumerate() {
            if maps.types[i] != u32::MAX {
                continue;
            }
            let Some(translated) = translate_type(ty, &maps) else { continue };
            let target = match index.get(&translated) {
                Some(&target) => target,
                None => {
                    out.types.push(translated.clone());
                    let target = (out.types.len() - 1) as u32;
                    index.insert(translated, target);
                    target
                }
            };
            maps.types[i] = target;
            progress = true;
        }
        if maps.types.iter().all(|&t| t != u32::MAX) {
            break;
        }
        if !progress {
            return Err(SessionError("the session program has types that refer to each other".into()));
        }
    }

    place(&mut out.objects, &maps.objects, fresh.objects.iter().map(|o| object(o, &maps)), "object", |_, o| o.name.clone())?;
    place(&mut out.roles, &maps.roles, fresh.roles.iter().map(|r| role(r, &maps)), "role", |_, r| r.name.clone())?;
    place(&mut out.enums, &maps.enums, fresh.enums.iter().cloned(), "enum", |_, e: &EnumDecl| e.name.clone())?;
    place(&mut out.variants, &maps.variants, fresh.variants.iter().map(|v| variant(v, &maps)), "variant", |_, v| v.name.clone())?;
    place(&mut out.caps, &maps.caps, fresh.caps.iter().map(|c| cap(c, &maps)), "cap", |_, c| c.name.clone())?;
    place(&mut out.handles, &maps.handles, fresh.handles.iter().cloned(), "handle", |_, h| h.clone())?;
    let globals: Vec<GlobalDecl> = fresh.globals.iter().map(|g| global(g, &maps)).collect();
    // A constant is named by its init's symbol, its key in the merge (#1057), not by a FuncRef.
    place(&mut out.globals, &maps.globals, globals.into_iter(), "constant", |position, _| fresh_globals[position].clone())?;
    let functions = fresh.functions.iter().map(|f| function(f, &maps)).collect::<Result<Vec<_>, _>>()?;
    place(&mut out.functions, &maps.functions, functions.into_iter(), "function", |_, f| f.symbol.clone())?;

    let func = |f: FuncRef| FuncRef(maps.functions[f.0 as usize]);
    out.entry = fresh.entry.map(func);
    out.tests = fresh.tests.as_ref().map(|tests| tests.iter().map(|(name, f)| (name.clone(), func(*f))).collect());
    out.module = fresh.module.clone();
    out.version = fresh.version.clone();
    Ok(out)
}

/// Put each translated fresh item at its mapped index: a published one must be equal, a new one is appended
/// (in index order, which is the fresh order). `name` names a changed one by its fresh position and itself.
fn place<T: PartialEq>(
    out: &mut Vec<T>,
    map: &[u32],
    items: impl Iterator<Item = T>,
    kind: &str,
    name: impl Fn(usize, &T) -> String,
) -> Result<(), SessionError> {
    let mut appended: Vec<(u32, T)> = Vec::new();
    for (position, (item, &target)) in items.zip(map).enumerate() {
        match out.get(target as usize) {
            Some(published) if *published == item => {}
            Some(_) => {
                return Err(SessionError(format!("the session changes the published {kind} {}", name(position, &item))));
            }
            None => appended.push((target, item)),
        }
    }
    appended.sort_by_key(|(target, _)| *target);
    for (target, item) in appended {
        debug_assert_eq!(target as usize, out.len());
        out.push(item);
    }
    Ok(())
}

fn ty(t: TypeRef, maps: &Maps) -> TypeRef {
    TypeRef(maps.types[t.0 as usize])
}

fn func(f: FuncRef, maps: &Maps) -> FuncRef {
    FuncRef(maps.functions[f.0 as usize])
}

fn obj(o: ObjectRef, maps: &Maps) -> ObjectRef {
    ObjectRef(maps.objects[o.0 as usize])
}

/// A fresh type in the result's numbering; `None` while a component type is not mapped yet.
fn translate_type(t: &Type, maps: &Maps) -> Option<Type> {
    let ty = |t: TypeRef| {
        let mapped = maps.types[t.0 as usize];
        (mapped != u32::MAX).then_some(TypeRef(mapped))
    };
    Some(match t {
        Type::Object(o) => Type::Object(obj(*o, maps)),
        Type::Successor(o) => Type::Successor(obj(*o, maps)),
        Type::Role(r) => Type::Role(crate::RoleRef(maps.roles[r.0 as usize])),
        Type::Enum(e) => Type::Enum(crate::EnumRef(maps.enums[e.0 as usize])),
        Type::Variant(v) => Type::Variant(crate::VariantRef(maps.variants[v.0 as usize])),
        Type::Cap(c) => Type::Cap(crate::CapRef(maps.caps[c.0 as usize])),
        Type::Handle(h) => Type::Handle(maps.handles[*h as usize]),
        Type::Array(e) => Type::Array(ty(*e)?),
        Type::Builder(e) => Type::Builder(ty(*e)?),
        Type::Map(k, v) => Type::Map(ty(*k)?, ty(*v)?),
        Type::MapBuilder(k, v) => Type::MapBuilder(ty(*k)?, ty(*v)?),
        Type::Fn { params, result } => Type::Fn {
            params: params.iter().map(|p| ty(*p)).collect::<Option<_>>()?,
            result: match result { Some(r) => Some(ty(*r)?), None => None },
        },
        Type::Future(result) => Type::Future(match result { Some(r) => Some(ty(*r)?), None => None }),
        other => other.clone(),
    })
}

fn object(o: &ObjectDecl, maps: &Maps) -> ObjectDecl {
    ObjectDecl {
        name: o.name.clone(),
        fields: o.fields.iter().map(|(name, t)| (name.clone(), ty(*t, maps))).collect(),
        does: o.does.iter().map(|w| Witness {
            role: crate::RoleRef(maps.roles[w.role.0 as usize]),
            slots: w.slots.iter().map(|f| func(*f, maps)).collect(),
        }).collect(),
        destroy: o.destroy.map(|f| func(f, maps)),
        builtin: o.builtin,
    }
}

fn role(r: &RoleDecl, maps: &Maps) -> RoleDecl {
    RoleDecl {
        name: r.name.clone(),
        builtin: r.builtin,
        methods: r.methods.iter().map(|m| RoleMethod {
            name: m.name.clone(),
            params: m.params.iter().map(|p| ty(*p, maps)).collect(),
            result: m.result.map(|t| ty(t, maps)),
        }).collect(),
    }
}

fn variant(v: &VariantDecl, maps: &Maps) -> VariantDecl {
    VariantDecl { name: v.name.clone(), cases: v.cases.iter().map(|(name, o)| (name.clone(), obj(*o, maps))).collect() }
}

fn cap(c: &CapDecl, maps: &Maps) -> CapDecl {
    CapDecl {
        name: c.name.clone(),
        owner: obj(c.owner, maps),
        fields: c.fields.iter().map(|(name, f)| (name.clone(), func(*f, maps))).collect(),
    }
}

fn global(g: &GlobalDecl, maps: &Maps) -> GlobalDecl {
    GlobalDecl { ty: ty(g.ty, maps), init: func(g.init, maps), data: g.data.clone() }
}

fn function(f: &Function, maps: &Maps) -> Result<Function, SessionError> {
    let code = f.code.iter().map(|i| instruction(i, maps)).collect::<Result<Vec<_>, _>>()
        .map_err(|message| SessionError(format!("{}: {message}", f.symbol)))?;
    Ok(Function {
        symbol: f.symbol.clone(),
        is_async: f.is_async,
        native: f.native.clone(),
        intrinsic: f.intrinsic.as_ref().map(|i| Intrinsic {
            name: i.name.clone(),
            objects: i.objects.iter().map(|o| obj(*o, maps)).collect(),
            helpers: i.helpers.iter().map(|h| func(*h, maps)).collect(),
        }),
        registers: f.registers.iter().map(|t| ty(*t, maps)).collect(),
        params: f.params.clone(),
        result: f.result.map(|t| ty(t, maps)),
        code,
        handlers: f.handlers.clone(),
    })
}

/// An instruction in the result's numbering. The table references `visit` reports are checked against the
/// translation, so an instruction kind added later with a reference this match misses is an error, never a
/// silently wrong index.
fn instruction(i: &Instruction, maps: &Maps) -> Result<Instruction, String> {
    use Instruction as I;
    let mut out = i.clone();
    match &mut out {
        I::GlobalGet { global, .. } => *global = maps.globals[*global as usize],
        I::EnumCase { ty, .. } => ty.0 = maps.enums[ty.0 as usize],
        I::ObjNew { object, .. } => *object = obj(*object, maps),
        I::RoleWrap { role, .. } | I::RoleNarrow { role, .. } | I::CallRole { role, .. } => {
            role.0 = maps.roles[role.0 as usize];
        }
        I::VariantNew { variant, .. } | I::VariantOf { variant, .. } => variant.0 = maps.variants[variant.0 as usize],
        I::Call { function, .. } | I::Closure { function, .. } => *function = func(*function, maps),
        I::CallCap { ty, .. } => ty.0 = maps.caps[ty.0 as usize],
        I::Catch { kind, ty, .. } => {
            *ty = match kind {
                CatchKind::Object => maps.objects[*ty as usize],
                CatchKind::Role => maps.roles[*ty as usize],
                CatchKind::Variant => maps.variants[*ty as usize],
            }
        }
        _ => {}
    }
    let expected = references(i).into_iter().map(|r| match r {
        Reference::Function(f) => Reference::Function(maps.functions[f as usize]),
        Reference::Object(o) => Reference::Object(maps.objects[o as usize]),
        Reference::Role(r) => Reference::Role(maps.roles[r as usize]),
        Reference::Enum(e) => Reference::Enum(maps.enums[e as usize]),
        Reference::Cap(c) => Reference::Cap(maps.caps[c as usize]),
        Reference::Variant(v) => Reference::Variant(maps.variants[v as usize]),
    }).collect::<Vec<_>>();
    if references(&out) != expected {
        return Err(format!("instruction {} has a reference the session merge does not translate", i.mnemonic()));
    }
    Ok(out)
}

#[derive(Debug, PartialEq, Eq)]
enum Reference {
    Function(u32),
    Object(u32),
    Role(u32),
    Enum(u32),
    Cap(u32),
    Variant(u32),
}

fn references(i: &Instruction) -> Vec<Reference> {
    let mut found = Vec::new();
    i.visit(&mut |access| match access {
        Access::Function(f) => found.push(Reference::Function(f.0)),
        Access::Object(o) => found.push(Reference::Object(o.0)),
        Access::Role(r) => found.push(Reference::Role(r.0)),
        Access::Enum(e) => found.push(Reference::Enum(e.0)),
        Access::Cap(c) => found.push(Reference::Cap(c.0)),
        Access::Variant(v) => found.push(Reference::Variant(v.0)),
        _ => {}
    });
    found
}
