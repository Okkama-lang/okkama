//! ABI-1 descriptors and non-unwinding shims for ordinary Rust functions.
use proc_macro::TokenStream;
use proc_macro2::TokenStream as Tokens;
use quote::{quote, format_ident};
use syn::{parse_macro_input, parse::{Parse, ParseStream}, punctuated::Punctuated, Token, Type, GenericArgument, PathArguments, ReturnType};

fn error(node: impl quote::ToTokens, message: &str) -> syn::Error { syn::Error::new_spanned(node, message) }
fn args(ty: &Type) -> syn::Result<(String, Vec<&Type>)> {
    let Type::Path(p) = ty else { return Err(error(ty, "expected an ABI value type")); };
    let last = p.path.segments.last().unwrap();
    let types = match &last.arguments {
        PathArguments::None => vec![],
        PathArguments::AngleBracketed(a) => a.args.iter().map(|a| match a {
            GenericArgument::Type(t) => Ok(t), _ => Err(error(a, "only type arguments are supported")),
        }).collect::<syn::Result<Vec<_>>>()?,
        _ => return Err(error(ty, "unsupported type arguments")),
    };
    Ok((last.ident.to_string(), types))
}
fn signature(ty: &Type) -> syn::Result<String> {
    match ty {
        Type::Reference(r) => signature(&r.elem),
        Type::Slice(s) if matches!(&*s.elem, Type::Path(p) if p.path.is_ident("u8")) => Ok("@byte".into()),
        Type::Tuple(t) if t.elems.is_empty() => Ok("void".into()),
        _ => {
            let (name, types) = args(ty)?;
            let arity = match name.as_str() { "Vec" | "Option" | "Shared" => 1, "BTreeMap" | "Result" => 2, _ => 0 };
            if types.len() != arity { return Err(error(ty, "unsupported generic ABI type")); }
            Ok(match name.as_str() {
                "i64" | "BigInt" => "int".into(), "f64" => "float".into(), "bool" => "bool".into(),
                "u8" => "byte".into(), "char" => "char".into(), "str" | "String" => "string".into(),
                "Vec" => format!("@{}", signature(types[0])?),
                "BTreeMap" => format!("%{}:{}", signature(types[0])?, signature(types[1])?),
                "Option" => {
                    let inner = signature(types[0])?;
                    if inner.starts_with('?') || inner == "void" { return Err(error(ty, "ABI 1 permits only one Option layer over a value")); }
                    format!("?{inner}")
                }
                "Shared" => signature(types[0])?,
                "Result" => {
                    if args(types[1])?.0 != "Failure" { return Err(error(ty, "native errors must be Failure")); }
                    signature(types[0])?
                }
                "i8" | "i16" | "i32" | "u16" | "u32" | "u64" | "usize" | "isize" | "f32" => return Err(error(ty, "use an ABI scalar: i64, f64, bool, u8 or char")),
                _ => name,
            })
        }
    }
}

#[proc_macro_attribute]
pub fn native(attr: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as syn::ItemFn);
    let attr: Tokens = attr.into();
    expand_native(attr, function).unwrap_or_else(syn::Error::into_compile_error).into()
}
fn expand_native(attr: Tokens, f: syn::ItemFn) -> syn::Result<Tokens> {
    let blocking = if attr.is_empty() { false } else if attr.to_string() == "blocking" { true }
        else if attr.to_string() == "release" { false }
        else { return Err(error(attr, "expected #[native], #[native(blocking)] or #[native(release)]")); };
    let release = attr.to_string() == "release";
    if f.sig.asyncness.is_some() || f.sig.unsafety.is_some() || f.sig.abi.is_some() || !f.sig.generics.params.is_empty() || f.sig.variadic.is_some() {
        return Err(error(&f.sig, "native requires a safe, non-generic ordinary Rust function"));
    }
    let name = &f.sig.ident;
    let shim = format_ident!("__okk_{}", name);
    let export = format_ident!("__okk_export_{}", name);
    let result: Type = match &f.sig.output { ReturnType::Default => syn::parse_quote!(()), ReturnType::Type(_, t) => *t.clone() };
    let mut stored_result = quote!(#result);
    let mut store_result = quote!(result);
    if let Type::Reference(reference) = &result {
        if reference.mutability.is_some() { return Err(error(&result, "native results cannot be mutable references")); }
        match &*reference.elem {
            Type::Path(p) if p.path.is_ident("str") => stored_result = quote!(String),
            Type::Slice(s) if matches!(&*s.elem, Type::Path(p) if p.path.is_ident("u8")) => stored_result = quote!(Vec<u8>),
            _ => return Err(error(&result, "state results must own their data")),
        }
        store_result = quote!(result.to_owned());
    }
    let mut reads = Vec::new(); let mut calls = Vec::new(); let mut names = Vec::new(); let mut types = Vec::new(); let mut signatures = Vec::new();
    for (index, arg) in f.sig.inputs.iter().enumerate() {
        let syn::FnArg::Typed(arg) = arg else { return Err(error(arg, "native functions cannot have a receiver")); };
        let syn::Pat::Ident(pat) = &*arg.pat else { return Err(error(arg, "native parameters need names")); };
        if pat.subpat.is_some() || pat.by_ref.is_some() { return Err(error(arg, "native parameters need plain names")); }
        names.push(pat.ident.to_string().trim_start_matches("r#").to_owned());
        let var = format_ident!("arg_{index}");
        let slot = quote!(unsafe { *a.add(#index) });
        let ty = &arg.ty;
        signatures.push(signature(ty)?);
        match &**ty {
            Type::Reference(r) => {
                let element = &r.elem;
                if r.mutability.is_some() && (signature(element)? == "string" || matches!(&**element, Type::Slice(_))) {
                    return Err(error(ty, "text and byte views are immutable"));
                }
                if matches!(&**element, Type::Path(p) if p.path.is_ident("str")) {
                    if blocking {
                        reads.push(quote!(let #var = c.text(#slot)?.to_owned();));
                        types.push(quote!(String)); calls.push(quote!(&#var));
                    } else { reads.push(quote!(let #var = c.text(#slot)?;)); calls.push(quote!(#var)); }
                } else if matches!(&**element, Type::Slice(_)) {
                    if blocking {
                        reads.push(quote!(let #var = c.bytes(#slot)?.to_vec();));
                        types.push(quote!(Vec<u8>)); calls.push(quote!(&#var));
                    } else { reads.push(quote!(let #var = c.bytes(#slot)?;)); calls.push(quote!(#var)); }
                } else {
                    if blocking { return Err(error(ty, "blocking state inputs must be Shared<T>, not references")); }
                    if r.mutability.is_some() {
                        reads.push(quote!(let mut #var = c.borrow_mut::<#element>(#slot)?;)); calls.push(quote!(&mut *#var));
                    } else { reads.push(quote!(let #var = c.borrow::<#element>(#slot)?;)); calls.push(quote!(&*#var)); }
                }
            }
            _ => { reads.push(quote!(const _: () = <#ty as Read>::BY_VALUE; let #var: #ty = c.read(#slot)?;)); calls.push(quote!(#var)); types.push(quote!(#ty)); }
        }
    }
    let release_type = if release {
        let Some(syn::FnArg::Typed(first)) = f.sig.inputs.first() else {
            return Err(error(&f.sig, "#[native(release)] requires a state as its first parameter"));
        };
        match &*first.ty {
            Type::Reference(r) => {
                let ty = &r.elem;
                reads.insert(0, quote!(const _: () = assert!(!<#ty as State>::SHARED,
                    "shared release requires Shared<T>" );));
                quote!(#ty)
            }
            ty => {
                let (name, types) = args(ty)?;
                if name != "Shared" || types.len() != 1 {
                    return Err(error(ty, "#[native(release)] requires Shared<T>, &T or &mut T"));
                }
                let ty = types[0]; quote!(#ty)
            }
        }
    } else { quote!() };
    let sync_body = if release {
        quote! {
            let outcome = execute(|| {
                #(#reads)*
                let result = super::#name(#(#calls),*);
                c.write(result)
            });
            let released = c.release::<#release_type>(unsafe { *a });
            let value = outcome??;
            released?;
            unsafe { *out = value; }
        }
    } else {
        quote! {
            #(#reads)*
            let result = super::#name(#(#calls),*);
            unsafe { *out = c.write(result)?; }
        }
    };
    let arity = names.len();
    let vars: Vec<_> = (0..arity).map(|i| format_ident!("arg_{i}")).collect();
    let signature = format!("{}({}) -> {}", if blocking { "async " } else { "" }, signatures.join(", "), signature(&result)?);
    let sig = syn::LitByteStr::new(format!("{signature}\0").as_bytes(), name.span());
    let params = syn::LitByteStr::new(format!("{}\0", names.join(", ")).as_bytes(), name.span());
    let suffix = format!("@0::{}/{arity}\0", name.to_string().trim_start_matches("r#"));
    let (body, callback) = if !blocking {
        (quote! {
            pub unsafe extern "C" fn sync(raw: *mut okk_call, a: *const okk_slot, out: *mut okk_slot) -> u32 {
                guard(|| {
                    let c = unsafe { Call::new(raw) };
                    #sync_body
                    Ok(())
                })
            }
        }, quote!(sync: Some(#shim::sync), r#async: ::std::ptr::null()))
    } else {
        (quote! {
            type Work = Job<(#(#types,)*), #stored_result>;
            unsafe extern "C" fn prepare(raw: *mut okk_call, a: *const okk_slot, out: *mut *mut ::std::ffi::c_void) -> u32 {
                unsafe { *out = ::std::ptr::null_mut(); }
                guard(|| {
                    let c = unsafe { Call::new(raw) };
                    #(#reads)*
                    unsafe { *out = job::<_, #stored_result>((#(#vars,)*)); }
                    Ok(())
                })
            }
            unsafe extern "C" fn run(ptr: *mut ::std::ffi::c_void, _: *const okk_cancel) {
                let work = unsafe { &mut *ptr.cast::<Work>() };
                work.result = Some(execute(|| {
                    let (#(#vars,)*) = work.args.take().expect("execute once");
                    let result = super::#name(#(#calls),*);
                    #store_result
                }));
            }
            unsafe extern "C" fn complete(raw: *mut okk_call, ptr: *mut ::std::ffi::c_void, out: *mut okk_slot) -> u32 {
                guard(|| {
                    let c = unsafe { Call::new(raw) };
                    let work = unsafe { &mut *ptr.cast::<Work>() };
                    let result = work.result.take().ok_or(OKK_PANIC)??;
                    unsafe { *out = c.write(result)?; }
                    Ok(())
                })
            }
            unsafe extern "C" fn cleanup(ptr: *mut ::std::ffi::c_void) {
                guard(|| { unsafe { drop(Box::from_raw(ptr.cast::<Work>())); } Ok(()) });
            }
            pub static ASYNC: okk_async = okk_async { prepare, execute: run, complete, drop: cleanup };
        }, quote!(sync: None, r#async: &#shim::ASYNC))
    };
    Ok(quote! {
        #f
        #[doc(hidden)]
        #[allow(non_snake_case, unused_imports, unused_variables)]
        mod #shim {
            use super::*;
            use ::okkama_ext::*;
            use ::okkama_ext::__private::*;
            #body
        }
        #[allow(unused_macros)]
        macro_rules! #export {
            ($module:literal) => {
                ::okkama_ext::okk_export {
                    symbol: concat!($module, #suffix).as_ptr().cast(),
                    signature: #sig.as_ptr().cast(), params: #params.as_ptr().cast(), #callback
                }
            };
        }
    })
}

#[proc_macro_attribute]
pub fn state(attr: TokenStream, item: TokenStream) -> TokenStream {
    let s = parse_macro_input!(item as syn::ItemStruct);
    let attr: Tokens = attr.into();
    expand_state(attr, s).unwrap_or_else(syn::Error::into_compile_error).into()
}
fn expand_state(attr: Tokens, s: syn::ItemStruct) -> syn::Result<Tokens> {
    if !s.generics.params.is_empty() { return Err(error(&s.generics, "state types cannot be generic")); }
    let shared = if attr.is_empty() { false } else if attr.to_string() == "shared" { true }
        else { return Err(error(attr, "expected #[state] or #[state(shared)]")); };
    let name = &s.ident;
    let by_value = if shared {
        quote!(const BY_VALUE: () = panic!("by-value shared state is unsupported; use #[native(release)] with Shared<T>" );)
    } else { quote!() };
    let check = if shared { quote! { unsafe impl ::okkama_ext::__private::SharedState for #name {} } } else { quote!() };
    Ok(quote! {
        #s
        #check
        impl #name { #[doc(hidden)] const __OKK_SHARED: bool = #shared; }
        impl ::okkama_ext::__private::Read for #name {
            #by_value
            fn read(c: &::okkama_ext::__private::Call, s: ::okkama_ext::okk_slot) -> Result<Self, u32> { c.take(s) }
        }
        impl ::okkama_ext::__private::Write for #name {
            fn write(self, c: &::okkama_ext::__private::Call) -> Result<::okkama_ext::okk_slot, u32> { c.state(self) }
        }
    })
}
struct Extension { module: syn::LitStr, functions: Punctuated<syn::Ident, Token![,]>, states: Punctuated<syn::Ident, Token![,]> }
impl Parse for Extension {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let module = input.parse()?; input.parse::<Token![,]>()?;
        let f; syn::bracketed!(f in input);
        let functions = f.parse_terminated(syn::Ident::parse, Token![,])?;
        input.parse::<Token![,]>()?;
        let word: syn::Ident = input.parse()?;
        if word != "states" { return Err(error(word, "expected states: [T, ...]")); }
        input.parse::<Token![:]>()?;
        let s; syn::bracketed!(s in input);
        let states = s.parse_terminated(syn::Ident::parse, Token![,])?;
        if input.peek(Token![,]) { input.parse::<Token![,]>()?; }
        Ok(Self { module, functions, states })
    }
}
#[proc_macro]
pub fn extension(input: TokenStream) -> TokenStream {
    let Extension { module, functions, states } = parse_macro_input!(input as Extension);
    if !module.value().split('/').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')) {
        return error(module, "invalid extension module path").into_compile_error().into();
    }
    let exports: Vec<_> = functions.iter().map(|f| format_ident!("__okk_export_{f}")).collect();
    let count = exports.len(); let state_count = states.len();
    let mut descriptors = Vec::new(); let mut implementations = Vec::new();
    for (i, state) in states.iter().enumerate() {
        let name = syn::LitByteStr::new(format!("{state}\0").as_bytes(), state.span());
        descriptors.push(quote!(::okkama_ext::okk_state_type { name: #name.as_ptr().cast(), drop: ::okkama_ext::__private::drop_state::<#state> }));
        implementations.push(quote! {
            unsafe impl ::okkama_ext::__private::State for #state {
                const SHARED: bool = Self::__OKK_SHARED;
                fn descriptor() -> &'static ::okkama_ext::okk_state_type { &__OKK_STATES[#i] }
            }
        });
    }
    quote! {
        #(#implementations)*
        static __OKK_STATES: [::okkama_ext::okk_state_type; #state_count] = [#(#descriptors),*];
        static __OKK_EXPORTS: [::okkama_ext::okk_export; #count] = [#(#exports!(#module)),*];
        #[unsafe(no_mangle)]
        pub static okkama_extension: ::okkama_ext::okk_extension = ::okkama_ext::okk_extension {
            abi: ::okkama_ext::OKKAMA_EXT_ABI,
            size: ::std::mem::size_of::<::okkama_ext::okk_extension>() as u32,
            name: concat!(#module, "\0").as_ptr().cast(), exports: __OKK_EXPORTS.as_ptr(), export_count: #count as u32,
            states: __OKK_STATES.as_ptr(), state_count: #state_count as u32,
        };
    }.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn abi_signatures_follow_the_value_table() {
        for (rust, abi) in [("i64", "int"), ("BigInt", "int"), ("f64", "float"), ("bool", "bool"), ("u8", "byte"), ("char", "char"),
            ("&str", "string"), ("String", "string"), ("&[u8]", "@byte"), ("Vec<u8>", "@byte"), ("Vec<Vec<i64>>", "@@int"),
            ("BTreeMap<String, Option<i64>>", "%string:?int"), ("Option<String>", "?string"), ("()", "void"),
            ("&mut Counter", "Counter"), ("Shared<Image>", "Image"), ("Result<Image, Failure>", "Image")] {
            assert_eq!(signature(&syn::parse_str::<Type>(rust).unwrap()).unwrap(), abi);
        }
    }
    #[test]
    fn abi_rejects_unrepresentable_signatures_and_borrowed_jobs() {
        for rust in ["Option<Option<i64>>", "Option<()>", "u64", "Result<i64, String>", "Vec<i64, bool>"] {
            assert!(signature(&syn::parse_str::<Type>(rust).unwrap()).is_err(), "{rust}");
        }
        for function in ["fn f(x: &mut Counter) {}", "fn f(x: &Counter) {}"] {
            assert!(expand_native(quote!(blocking), syn::parse_str(function).unwrap()).is_err());
        }
        assert!(expand_native(quote!(), syn::parse_str("unsafe fn f() {}").unwrap()).is_err());
    }
}

#[cfg(test)]
mod release_tests {
    use super::*;
    #[test]
    fn abi_shared_by_value_expands_to_compile_time_rejection() {
        let state = expand_state(quote!(shared), syn::parse_quote!(struct Image;)).unwrap().to_string();
        assert!(state.contains("const BY_VALUE : () = panic !"));
        assert!(state.contains("use #[native(release)] with Shared<T>"));
        for attr in [quote!(), quote!(blocking)] {
            let function = expand_native(attr, syn::parse_quote!(fn close(value: Image) {})).unwrap().to_string();
            assert!(function.contains("const _ : () = < Image as Read > :: BY_VALUE"));
        }
        let plain = expand_state(quote!(), syn::parse_quote!(struct Counter;)).unwrap().to_string();
        assert!(!plain.contains("panic !"));
    }
    #[test]
    fn abi_release_requires_a_state_parameter() {
        for function in ["fn close() {}", "fn close(value: Image) {}", "fn close(value: i64) {}"] {
            assert!(expand_native(quote!(release), syn::parse_str(function).unwrap()).is_err());
        }
        for function in ["fn close(value: Shared<Image>) {}", "fn close(value: &Counter) {}", "fn close(value: &mut Counter) {}"] {
            assert!(expand_native(quote!(release), syn::parse_str(function).unwrap()).is_ok());
        }
    }
}
