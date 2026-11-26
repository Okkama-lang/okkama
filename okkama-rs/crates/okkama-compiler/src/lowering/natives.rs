//! A native declaration against the runtime's native table (#1098): a
//! declaration whose canonical symbol names a native of the table takes that
//! native's signature, or the program is refused at the declaration. The SPB
//! verifier (`okkama_sparrow` typing) checks the same for any SPB at load; this
//! check makes it a diagnostic, so whatever lowering accepts, codegen generates.
//! A symbol the table does not name is left to the runtime, as the verifier
//! leaves it.

use okkama_sparrow::abi::{Native, Type as Abi};

use super::ctx::Ctx;
use super::{LoweringError, Result};
use crate::analysis::registry::NativeInfo;
use crate::ir::{ResultType, Type};

/// Whether OIR type `ty` is the table's type `abi`, as the SPB verifier
/// compares them (a handle by its name, without the module).
fn same(ctx: &Ctx<'_, '_>, ty: &Type, abi: &Abi) -> bool {
    match (ty, abi) {
        (Type::Int, Abi::Int)
        | (Type::Float, Abi::Float)
        | (Type::Rat, Abi::Rat)
        | (Type::Bool, Abi::Bool)
        | (Type::Byte, Abi::Byte)
        | (Type::Char, Abi::Char)
        | (Type::Str, Abi::Str)
        | (Type::Any, Abi::Any) => true,
        (Type::Handle(handle), Abi::Handle(name)) => ctx.reg.handles.get(handle.0).is_some_and(|decl| decl.name == *name),
        (Type::Array(element), Abi::Array(item)) => same(ctx, element, item),
        (Type::Map(key, value), Abi::Map(k, v)) => same(ctx, key, k) && same(ctx, value, v),
        _ => false,
    }
}

/// Native `info`, lowered to parameters `params` and `result` under the
/// canonical symbol `symbol`, has the signature the table gives that symbol.
pub(super) fn check_abi(ctx: &Ctx<'_, '_>, info: &NativeInfo<'_>, symbol: &str, params: &[Type], result: &ResultType) -> Result<()> {
    let Some(native) = Native::resolve(symbol) else {
        return Ok(());
    };
    let signature = native.signature();
    let matches = info.is_async == signature.is_async
        && params.len() == signature.params.len()
        && params.iter().zip(&signature.params).all(|(ty, abi)| same(ctx, ty, abi))
        && match result {
            ResultType::Void => signature.result == Abi::Void,
            ResultType::Value(ty) => same(ctx, ty, &signature.result),
        };
    if matches {
        return Ok(());
    }
    Err(LoweringError::invalid(
        info.decl.span,
        format!(
            "Native \"{}\" does not match {symbol} of the runtime, which is {}",
            info.name,
            native.signature_text()
        ),
    ))
}

#[cfg(test)]
mod tests {
    use crate::{SourceFile, lowering::{self, LoweringError}, syntax};

    fn lowered(text: &str) -> Result<crate::ir::Module, LoweringError> {
        let source = SourceFile::new("natives.ok", text).unwrap();
        let program = syntax::parse(&source, &syntax::ParseOptions::default()).unwrap();
        lowering::lower_program(&syntax::prune::strip_test_items(program))
    }

    /// The module is refused at `at` with a semantic error that says `says`.
    fn refused(text: &str, at: &str, says: &str) {
        let error = lowered(text).unwrap_err();
        assert!(matches!(error, LoweringError::Invalid { .. }), "{error:?}");
        assert!(error.message().contains(says), "{}", error.message());
        assert!(text[error.span().start as usize..].starts_with(at), "{error:?}");
    }

    const ZONE: &str = "module runtime/time_zone v1.0.0;\n";
    const BUFFER: &str = "module runtime/buffer v1.0.0;\nnative object BufferState;\nnative object Other;\n";

    /// #1098: a native of the table keeps the table's parameters, result and
    /// asynchrony; the symbol is the module, its major version, the name and arity.
    #[test]
    fn a_native_of_the_table_has_its_signature() {
        lowered(&format!("{ZONE}native timezone_version() is string;")).expect("the table's signature");
        lowered(&format!("{BUFFER}native buffer_state_size(state is BufferState) is int;")).expect("a handle by its name");
        lowered("module runtime/application v1.0.0;\nnative object TerminalState;\n\
            native async terminal_standard(output is int) is TerminalState;").expect("an async native");
        refused("module runtime/application v1.0.0;\nnative object TerminalState;\n\
            native terminal_standard(output is int) is TerminalState;", "native terminal_standard",
            "which is async (int) -> TerminalState");
        let version = "Native \"timezone_version\" does not match runtime/time_zone@1::timezone_version/0 of the runtime, \
            which is () -> string";
        refused(&format!("{ZONE}native timezone_version() is int;"), "native timezone_version", version);
        refused(&format!("{ZONE}native timezone_version() is void;"), "native timezone_version", version);
        refused(&format!("{ZONE}native async timezone_version() is string;"), "native async timezone_version", version);
        refused(&format!("{BUFFER}native buffer_state_size(state is Other) is int;"), "native buffer_state_size",
            "which is (BufferState) -> int");
        refused(&format!("{BUFFER}native buffer_state_size(state is @byte) is int;"), "native buffer_state_size",
            "which is (BufferState) -> int");
    }

    /// A symbol the table does not name is the runtime's to refuse (another
    /// arity, module or major version), as the SPB verifier leaves it.
    #[test]
    fn a_native_outside_the_table_is_left_to_the_runtime() {
        lowered(&format!("{ZONE}native timezone_version(extra is int) is int;")).expect("another arity");
        lowered("module app/zone v1.0.0;\nnative timezone_version() is int;").expect("another module");
        lowered("module runtime/time_zone v2.0.0;\nnative timezone_version() is int;").expect("another major version");
    }
}
