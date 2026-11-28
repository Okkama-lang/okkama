//! Static, trusted extension registration and load-time signature binding.
#![allow(unsafe_code)]

use crate::{Error, host, program::Loaded};
use okkama_runtime_abi::{Native, Signature, Type as Abi, ext::*};
use okkama_sparrow::{Program, Type, TypeRef};
use std::{collections::HashMap, ffi::CStr, rc::Rc, sync::Arc};

#[derive(Debug)]
struct Export {
    symbol: &'static str,
    text: &'static str,
    signature: Signature,
    callback: host::native_ext::Callback,
}

/// An immutable descriptor registered by the embedding, never by bytecode.
#[derive(Debug, Clone)]
pub struct Extension {
    exports: Arc<[Export]>,
    states: &'static [okk_state_type],
}

impl Extension {
    /// Validate a static descriptor without invoking its code.
    ///
    /// # Safety
    /// All descriptor pointers must be aligned, readable and immutable for
    /// the process lifetime, including strings, arrays and callbacks. Strings
    /// must be NUL-terminated. Callbacks obey `okkama_ext.h`, never unwind, and
    /// support independent runtimes on different threads. Async jobs own their
    /// copied inputs and can move to and from a blocking pool thread; job drop
    /// may run on either thread after execute has returned.
    pub unsafe fn from_static(descriptor: &'static okk_extension) -> Result<Self, Error> {
        if descriptor.abi != OKKAMA_EXT_ABI || descriptor.size as usize != std::mem::size_of::<okk_extension>() {
            return Err(Error("extension ABI or descriptor size does not match".into()));
        }
        unsafe { text(descriptor.name)? };
        if (descriptor.export_count != 0 && descriptor.exports.is_null())
            || (descriptor.state_count != 0 && descriptor.states.is_null())
        {
            return Err(Error("extension descriptor has a null table".into()));
        }
        let exports = if descriptor.export_count == 0 { &[][..] } else {
            unsafe { std::slice::from_raw_parts(descriptor.exports, descriptor.export_count as usize) }
        };
        let mut parsed = Vec::with_capacity(exports.len());
        let mut symbols = std::collections::HashSet::new();
        for export in exports {
            let symbol = unsafe { text(export.symbol)? };
            let signature_text = unsafe { text(export.signature)? };
            unsafe { text(export.params)? };
            let signature = Signature::try_parse(signature_text).map_err(Error)?;
            let valid = symbol.split_once("::").and_then(|(module, name)| {
                let (module, major) = module.split_once('@')?;
                let (name, arity) = name.split_once('/')?;
                let component = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
                Some(module.split('/').all(component) && component(name)
                    && !major.is_empty() && major.bytes().all(|b| b.is_ascii_digit())
                    && arity.parse::<usize>().ok() == Some(signature.params.len()))
            }).unwrap_or(false);
            if !valid {
                return Err(Error(format!("invalid extension symbol {symbol}")));
            }
            if Native::resolve(symbol).is_some() || symbol.starts_with("runtime/") {
                return Err(Error(format!("extension symbol {symbol} shadows the std native table")));
            }
            if !symbols.insert(symbol) {
                return Err(Error(format!("duplicate extension symbol {symbol}")));
            }
            if signature.is_async {
                if export.sync.is_some() || export.r#async.is_null() {
                    return Err(Error(format!("extension {symbol} needs an async callback")));
                }
            } else if export.sync.is_none() || !export.r#async.is_null() {
                return Err(Error(format!("extension {symbol} needs a sync callback")));
            }
            let callback = match export.sync {
                Some(sync) => host::native_ext::Callback::Sync(sync),
                None => host::native_ext::Callback::Async(unsafe { &*export.r#async }),
            };
            parsed.push(Export { symbol, text: signature_text, signature, callback });
        }
        let states = if descriptor.state_count == 0 { &[][..] } else {
            unsafe { std::slice::from_raw_parts(descriptor.states, descriptor.state_count as usize) }
        };
        let mut names = std::collections::HashSet::new();
        for state in states {
            let name = unsafe { text(state.name)? };
            if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                return Err(Error(format!("invalid extension state name {name}")));
            }
            if !names.insert(name) { return Err(Error(format!("duplicate extension state {name}"))); }
        }
        Ok(Self { exports: parsed.into(), states })
    }
}

unsafe fn text(pointer: *const core::ffi::c_char) -> Result<&'static str, Error> {
    if pointer.is_null() {
        return Err(Error("extension descriptor has a null string".into()));
    }
    unsafe { CStr::from_ptr(pointer) }.to_str().map_err(|_| Error("extension descriptor has invalid UTF-8".into()))
}

pub(crate) fn bind(loaded: &Loaded, extensions: &[Extension], host: &mut host::Heap)
    -> Result<Vec<Option<Rc<host::native_ext::Binding>>>, Error>
{
    let mut exports = HashMap::new();
    for extension in extensions {
        for export in extension.exports.iter() {
            if exports.insert(export.symbol, (export, extension.states)).is_some() {
                return Err(Error(format!("duplicate extension symbol {} in two extensions", export.symbol)));
            }
        }
    }
    let program = &loaded.program;
    program.functions.iter().map(|function| {
        let Some((export, states)) = function.native.as_deref().and_then(|s| exports.get(s)) else { return Ok(None); };
        let signature = &export.signature;
        let matches = signature.is_async == function.is_async
            && signature.params.len() == function.params.len()
            && signature.params.iter().zip(&function.params).all(|(abi, reg)| {
                abi_matches(program, function.registers[reg.0 as usize], abi, export.symbol)
            })
            && match function.result {
                Some(ty) => abi_matches(program, ty, &signature.result, export.symbol),
                None => signature.result == Abi::Void,
            };
        if !matches {
            let params: Vec<_> = function.params.iter().map(|r| type_text(program, function.registers[r.0 as usize])).collect();
            let result = function.result.map(|t| type_text(program, t)).unwrap_or_else(|| "void".into());
            let asynchronous = if function.is_async { "async " } else { "" };
            return Err(Error(format!("native {} signature mismatch: program {asynchronous}({}) -> {result}; extension {}",
                export.symbol, params.join(", "), export.text)));
        }
        host::native_ext::Binding::new(host, program, function, export.symbol, signature, export.callback, states).map(|b| Some(Rc::new(b)))
    }).collect()
}

// The same structural rule as Typing::abi; the verifier has no extension registry.
fn abi_matches(program: &Program, ty: TypeRef, abi: &Abi, symbol: &str) -> bool {
    match (&program.types[ty.0 as usize], abi) {
        (Type::Int, Abi::Int) | (Type::Float, Abi::Float) | (Type::Rat, Abi::Rat)
        | (Type::Bool, Abi::Bool) | (Type::Byte, Abi::Byte) | (Type::Char, Abi::Char)
        | (Type::Str, Abi::Str) | (Type::Any, Abi::Any) => true,
        (Type::Handle(h), Abi::Handle(name)) => program.handles.get(*h as usize).is_some_and(|h| {
            if name.contains("::") { h == name } else {
                let module = symbol.split('@').next().unwrap_or("").replace('/', ".");
                h == &format!("{module}::{name}")
            }
        }),
        (Type::Array(e), Abi::Array(a)) => abi_matches(program, *e, a, symbol),
        (Type::Map(k, v), Abi::Map(a, b)) => abi_matches(program, *k, a, symbol) && abi_matches(program, *v, b, symbol),
        (Type::Variant(v), Abi::Maybe(inner)) => program.variants.get(v.0 as usize).is_some_and(|variant| {
            let name = variant.name.rsplit("::").next().unwrap_or(&variant.name);
            let case = |wanted: &str| variant.cases.iter().find(|(case, _)| case == wanted).map(|(_, object)| *object);
            name.starts_with("$variant_Maybe_") && variant.cases.len() == 2
                && case("none").and_then(|o| program.objects.get(o.0 as usize)).is_some_and(|o| o.fields.is_empty())
                && case("some").and_then(|o| program.objects.get(o.0 as usize)).is_some_and(|o| {
                    matches!(o.fields.as_slice(), [(field, ty)] if field == "value" && abi_matches(program, *ty, inner, symbol))
                })
        }),
        _ => false,
    }
}

fn type_text(program: &Program, ty: TypeRef) -> String {
    match &program.types[ty.0 as usize] {
        Type::Int => "int".into(), Type::Float => "float".into(), Type::Rat => "rat".into(),
        Type::Bool => "bool".into(), Type::Byte => "byte".into(), Type::Char => "char".into(),
        Type::Str => "string".into(), Type::Any => "any".into(),
        Type::Handle(h) => program.handles[*h as usize].clone(),
        Type::Array(e) => format!("@{}", type_text(program, *e)),
        Type::Map(k, v) => format!("%{}:{}", type_text(program, *k), type_text(program, *v)),
        other => format!("{other:?}"),
    }
}
