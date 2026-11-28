//! Trusted extension loading, descriptor validation and signature binding.
#![allow(unsafe_code)]

use crate::{Error, host, program::Loaded};
use okkama_runtime_abi::{Native, Signature, Type as Abi, ext::*};
use okkama_sparrow::{Program, Type, TypeRef};
use std::{collections::HashMap, ffi::CStr, rc::Rc, sync::Arc};

#[derive(Debug)]
struct Export {
    symbol: &'static str,
    text: &'static str,
    params: &'static str,
    signature: Signature,
    callback: host::native_ext::Callback,
}

/// An immutable descriptor registered by the embedding, never by bytecode.
#[derive(Debug, Clone)]
pub struct Extension {
    name: &'static str,
    exports: Arc<[Export]>,
    states: &'static [okk_state_type],
}

impl Extension {
    /// An editable module skeleton for `okkama ext declare`, without author documentation.
    pub fn declarations(&self) -> Result<String, Error> {
        let module = self.exports.first().map(|e| e.symbol.split('@').next().unwrap()).unwrap_or(self.name);
        if !module.split('/').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')) {
            return Err(Error("extension has no exports or module name from which to declare a module".into()));
        }
        if self.exports.iter().any(|e| e.symbol.split('@').next() != Some(module)) {
            return Err(Error("extension exports more than one module; declare each module by hand".into()));
        }
        // The symbol's major is the module's: a package module without a version is major 0.
        let major = |e: &Export| e.symbol.split('@').nth(1).and_then(|rest| rest.split("::").next()).unwrap_or("0");
        let first = self.exports.first().map(major).unwrap_or("0");
        if self.exports.iter().any(|e| major(e) != first) {
            return Err(Error("extension exports more than one major version of its module".into()));
        }
        let mut out = if first == "0" { format!("module {module};\n\n") } else { format!("module {module} v{first}.0.0;\n\n") };
        for state in self.states {
            let name = unsafe { text(state.name)? };
            out.push_str(&format!("native object {name};\n"));
        }
        for export in self.exports.iter() {
            let name = export.symbol.split_once("::").unwrap().1.split('/').next().unwrap();
            let names: Vec<_> = if export.params.trim().is_empty() { vec![] }
                else { export.params.split(',').map(str::trim).collect() };
            if names.len() != export.signature.params.len() || names.iter().any(|n| {
                n.is_empty() || !n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                    || n.as_bytes()[0].is_ascii_digit()
            }) || names.iter().enumerate().any(|(i, n)| names[..i].contains(n)) {
                return Err(Error(format!("extension {} has invalid parameter names", export.symbol)));
            }
            let params: Vec<_> = names.iter().zip(&export.signature.params)
                .map(|(name, ty)| format!("{name} is {}", declaration_type(ty))).collect();
            let asynchronous = if export.signature.is_async { "async " } else { "" };
            out.push_str(&format!("native {asynchronous}{name}({}) is {};\n", params.join(", "), declaration_type(&export.signature.result)));
        }
        Ok(out)
    }

    pub(crate) fn same_descriptor(&self, other: &Self) -> bool { Arc::ptr_eq(&self.exports, &other.exports) }

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
        let name = unsafe { text(descriptor.name)? };
        if (descriptor.export_count != 0 && descriptor.exports.is_null())
            || (descriptor.state_count != 0 && descriptor.states.is_null())
        {
            return Err(Error("extension descriptor has a null table".into()));
        }
        let exports = unsafe { table(descriptor.exports, descriptor.export_count)? };
        let mut parsed = Vec::with_capacity(exports.len());
        let mut symbols = std::collections::HashSet::new();
        for export in exports {
            let symbol = unsafe { text(export.symbol)? };
            let signature_text = unsafe { text(export.signature)? };
            let params = unsafe { text(export.params)? };
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
                None => {
                    let callbacks = unsafe { table(export.r#async.cast::<RawAsync>(), 1)? };
                    let callbacks = &callbacks[0];
                    if callbacks.prepare.is_none() || callbacks.execute.is_none() || callbacks.complete.is_none() || callbacks.drop.is_none() {
                        return Err(Error(format!("extension {symbol} has a null async callback")));
                    }
                    host::native_ext::Callback::Async(unsafe { &*export.r#async })
                }
            };
            parsed.push(Export { symbol, text: signature_text, params, signature, callback });
        }
        let raw_states = unsafe { table(descriptor.states.cast::<RawState>(), descriptor.state_count)? };
        if raw_states.iter().any(|state| state.drop.is_none()) {
            return Err(Error("extension has a null state drop callback".into()));
        }
        let states = unsafe { table(descriptor.states, descriptor.state_count)? };
        let mut names = std::collections::HashSet::new();
        for state in states {
            let name = unsafe { text(state.name)? };
            if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                return Err(Error(format!("invalid extension state name {name}")));
            }
            if !names.insert(name) { return Err(Error(format!("duplicate extension state {name}"))); }
        }
        Ok(Self { name, exports: parsed.into(), states })
    }
}

fn declaration_type(ty: &Abi) -> String {
    match ty {
        Abi::Int => "int".into(), Abi::Float => "float".into(), Abi::Rat => "rat".into(),
        Abi::Bool => "bool".into(), Abi::Byte => "byte".into(), Abi::Char => "char".into(),
        Abi::Str => "string".into(), Abi::Void => "void".into(), Abi::Any => "any".into(),
        Abi::Handle(name) => (*name).into(),
        Abi::Array(t) => format!("@{}", declaration_type(t)),
        Abi::Map(k, v) => format!("%{}:{}", declaration_type(k), declaration_type(v)),
        Abi::Maybe(t) => format!("Maybe<{}>", declaration_type(t)),
    }
}

// Nullable mirrors let validation inspect C data before forming Rust function pointers.
#[repr(C)]
struct RawAsync {
    prepare: Option<unsafe extern "C" fn(*mut okk_call, *const okk_slot, *mut *mut core::ffi::c_void) -> u32>,
    execute: Option<unsafe extern "C" fn(*mut core::ffi::c_void, *const okk_cancel)>,
    complete: Option<unsafe extern "C" fn(*mut okk_call, *mut core::ffi::c_void, *mut okk_slot) -> u32>,
    drop: Option<unsafe extern "C" fn(*mut core::ffi::c_void)>,
}
#[repr(C)]
struct RawState {
    name: *const core::ffi::c_char,
    drop: Option<unsafe extern "C" fn(*mut core::ffi::c_void)>,
}

unsafe fn table<T>(pointer: *const T, len: u32) -> Result<&'static [T], Error> {
    if len == 0 { return Ok(&[]); }
    if pointer.is_null() || !pointer.is_aligned() || len as usize > isize::MAX as usize / std::mem::size_of::<T>() {
        return Err(Error("extension descriptor has an invalid table".into()));
    }
    Ok(unsafe { std::slice::from_raw_parts(pointer, len as usize) })
}

unsafe fn text(pointer: *const core::ffi::c_char) -> Result<&'static str, Error> {
    if pointer.is_null() {
        return Err(Error("extension descriptor has a null string".into()));
    }
    unsafe { CStr::from_ptr(pointer) }.to_str().map_err(|_| Error("extension descriptor has invalid UTF-8".into()))
}

/// Load trusted process code. Libraries and descriptor pointers live until process exit.
/// Bytecode never supplies this path; loading can run the library's OS constructors.
pub fn load(path: impl AsRef<std::path::Path>) -> Result<Extension, Error> {
    #[cfg(all(unix, feature = "host"))]
    { loader::load(path.as_ref()) }
    #[cfg(not(all(unix, feature = "host")))]
    { Err(Error(format!("{}: native extensions are not supported on this platform", path.as_ref().display()))) }
}

#[cfg(all(unix, feature = "host"))]
mod loader {
    use super::*;
    use std::{ffi::CString, os::unix::ffi::OsStrExt, path::{Path, PathBuf}, sync::{Mutex, OnceLock}};

    pub(super) fn load(path: &Path) -> Result<Extension, Error> {
        let error = |reason| Error(format!("native extension {}: {reason}", path.display()));
        let canonical = path.canonicalize().map_err(|e| error(format!("cannot resolve path: {e}")))?;
        // Serialise validation too, so every runtime gets the same descriptor and state identities.
        static CACHE: OnceLock<Mutex<HashMap<PathBuf, Result<Extension, String>>>> = OnceLock::new();
        let mut cache = CACHE.get_or_init(Mutex::default).lock().unwrap_or_else(|e| e.into_inner());
        cache.entry(canonical.clone()).or_insert_with(|| unsafe { open(&canonical) })
            .clone().map_err(error)
    }

    unsafe fn open(path: &Path) -> Result<Extension, String> {
        let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| "path contains NUL".to_owned())?;
        unsafe {
            let library = libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
            if library.is_null() { return Err(dlerror()); }
            // Never dlclose, including refused descriptors: constructors may have retained pointers.
            libc::dlerror();
            let descriptor = libc::dlsym(library, c"okkama_extension".as_ptr());
            let failure = libc::dlerror();
            if !failure.is_null() { return Err(CStr::from_ptr(failure).to_string_lossy().into_owned()); }
            if descriptor.is_null() { return Err("missing okkama_extension descriptor".into()); }
            let descriptor = descriptor.cast::<okk_extension>();
            if !descriptor.is_aligned() { return Err("unaligned extension descriptor".into()); }
            // Read the common prefix before forming a reference to the complete ABI-1 descriptor.
            let prefix = descriptor.cast::<u32>();
            if prefix.read() != OKKAMA_EXT_ABI || prefix.add(1).read() as usize != std::mem::size_of::<okk_extension>() {
                return Err("extension ABI or descriptor size does not match".into());
            }
            Extension::from_static(&*descriptor).map_err(|e| e.0)
        }
    }

    unsafe fn dlerror() -> String {
        let pointer = unsafe { libc::dlerror() };
        if pointer.is_null() { "dynamic loader failed without a diagnostic".into() }
        else { unsafe { CStr::from_ptr(pointer) }.to_string_lossy().into_owned() }
    }
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

#[cfg(test)]
mod declaration_tests {
    use super::*;
    unsafe extern "C" fn callback(_: *mut okk_call, _: *const okk_slot, _: *mut okk_slot) -> u32 { OKK_PANIC }
    fn descriptor(exports: Vec<okk_export>) -> Extension {
        let count = exports.len();
        let exports = Box::leak(exports.into_boxed_slice());
        let descriptor = Box::leak(Box::new(okk_extension {
            abi: OKKAMA_EXT_ABI, size: std::mem::size_of::<okk_extension>() as u32, name: c"empty/module".as_ptr(),
            exports: exports.as_ptr(), export_count: count as u32, states: std::ptr::null(), state_count: 0,
        }));
        unsafe { Extension::from_static(descriptor).unwrap() }
    }
    fn export(symbol: &'static CStr, signature: &'static CStr, params: &'static CStr) -> okk_export {
        okk_export { symbol: symbol.as_ptr(), signature: signature.as_ptr(), params: params.as_ptr(), sync: Some(callback), r#async: std::ptr::null() }
    }
    #[test]
    fn ext_declare_translates_abi_types_without_invoking_callbacks() {
        let extension = descriptor(vec![export(c"sample@0::convert/2", c"(@?int, %string:@byte) -> rat", c"values, entries")]);
        assert_eq!(extension.declarations().unwrap(), "module sample;\n\nnative convert(values is @Maybe<int>, entries is %string:@byte) is rat;\n");
        assert_eq!(descriptor(vec![]).declarations().unwrap(), "module empty/module;\n\n");
        // A versioned module keeps its major, or its symbols would be `@0` and never bind.
        let versioned = descriptor(vec![export(c"sample@2::f/0", c"() -> int", c"")]);
        assert_eq!(versioned.declarations().unwrap(), "module sample v2.0.0;\n\nnative f() is int;\n");
        let mixed = descriptor(vec![export(c"sample@1::f/0", c"() -> int", c""), export(c"sample@2::g/0", c"() -> int", c"")]);
        assert!(mixed.declarations().unwrap_err().0.contains("more than one major"));
    }
    #[test]
    fn ext_declare_refuses_ambiguous_modules_and_invalid_parameter_names() {
        for params in [c"", c"a,b", c"a; injected", c"1name"] {
            let extension = descriptor(vec![export(c"sample@0::f/1", c"(int) -> int", params)]);
            assert!(extension.declarations().unwrap_err().0.contains("parameter names"));
        }
        let extension = descriptor(vec![export(c"sample@0::f/2", c"(int, int) -> int", c"a,a")]);
        assert!(extension.declarations().unwrap_err().0.contains("parameter names"));
        let extension = descriptor(vec![export(c"a@0::f/0", c"() -> int", c""), export(c"b@0::f/0", c"() -> int", c"")]);
        assert!(extension.declarations().unwrap_err().0.contains("more than one module"));
    }
}
