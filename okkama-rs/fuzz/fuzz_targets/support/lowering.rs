//! Shared rejection policy for source fuzz targets.
use okkama_compiler::{ir, lowering::LoweringError};

pub fn accepted(result: Result<ir::Module, LoweringError>) -> Option<ir::Module> {
    match result {
        Ok(module) => Some(module),
        Err(error @ LoweringError::Internal { .. }) => panic!("{error}"),
        Err(LoweringError::Invalid { .. } | LoweringError::Unsupported { .. }) => None,
    }
}
