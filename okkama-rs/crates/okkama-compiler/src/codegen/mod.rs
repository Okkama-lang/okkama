//! Выбор backend и генерация выходного артефакта из IR.
//!
//! Каждый backend располагается в своём подмодуле. Общая точка входа
//! [`generate`] проверяет модуль, направляет его выбранному генератору и
//! возвращает [`Artifact`] (основной файл и вспомогательные) либо ошибку.
//! Запись файлов и параметры командной строки остаются в CLI.

pub mod sparrow;

use std::fmt;

use crate::ir;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Backend {
    Sparrow,
}

impl Backend {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "sparrow" => Some(Self::Sparrow),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Sparrow => "sparrow",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodegenError {
    InvalidInput { backend: Backend, message: String },
}

impl fmt::Display for CodegenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput { backend, message } => {
                write!(f, "backend `{}`: {message}", backend.name())
            }
        }
    }
}

impl std::error::Error for CodegenError {}

/// A codegen error with its place, `file:line:column: …`, when the module's
/// debug sources know the failing function's file (#1006).
fn located(module: &ir::Module, error: &sparrow::GenerateError) -> String {
    let place = error
        .function
        .filter(|_| !error.span.is_empty())
        .and_then(|function| module.source_of(ir::FuncId(function)))
        .map(|source| format!("{}:{}: ", source.name(), source.location(error.span.start)));
    format!("{}{error}", place.unwrap_or_default())
}

/// Логическое имя выходного артефакта, без каталога и расширения.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub artifact_name: String,
}

/// Один выходной файл backend: имя относительно каталога основного артефакта.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactFile {
    pub name: String,
    pub bytes: Vec<u8>,
}

/// Результат генерации: основной файл первым, затем вспомогательные.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Artifact {
    pub files: Vec<ArtifactFile>,
}

impl Artifact {
    /// Байты основного файла.
    pub fn main(&self) -> &[u8] {
        &self.files[0].bytes
    }
}

pub fn generate(
    module: &ir::Module,
    backend: Backend,
    options: &Options,
) -> Result<Artifact, CodegenError> {
    let files = match backend {
        Backend::Sparrow => {
            let program =
                sparrow::generate(module).map_err(|error| CodegenError::InvalidInput {
                    backend,
                    message: located(module, &error),
                })?;
            let bytes =
                okkama_sparrow::encode(&program).map_err(|error| CodegenError::InvalidInput {
                    backend,
                    message: error.to_string(),
                })?;
            vec![(format!("{}.spb", options.artifact_name), bytes)]
        }
    };
    Ok(Artifact {
        files: files
            .into_iter()
            .map(|(name, bytes)| ArtifactFile { name, bytes })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{SourceFile, Span};
    use std::sync::Arc;

    /// #1006: a function's codegen error names its file, line and column; a
    /// module-level one, or one without a known source, has no place.
    #[test]
    fn a_codegen_error_is_placed_through_the_debug_sources() {
        let text = "module shop/cart v1.0.0;\n\nmain() is int { 0; }\n";
        let source = SourceFile::new("shop/cart.ok", text).unwrap();
        let ast = crate::syntax::parse(&source, &crate::syntax::ParseOptions::default()).unwrap();
        let mut module = crate::lowering::lower_program(&ast).unwrap();
        module.debug.sources = vec![Some(Arc::new(source))];
        module.debug.function_modules = vec![0];
        let error = |function| sparrow::GenerateError { span: Span::new(26, 30), message: "bad".to_string(), function };
        assert_eq!(located(&module, &error(Some(0))), "shop/cart.ok:3:1: Sparrow codegen: bad");
        assert_eq!(located(&module, &error(None)), "Sparrow codegen: bad");
        assert_eq!(located(&module, &error(Some(7))), "Sparrow codegen: bad");
    }
}
