//! The program the exec target runs: the source linked as `okkama build`
//! links it, with no module to import (#1089). Unlinked OIR
//! (`lowering::lower_program`) keeps its imports unresolved by contract, and
//! codegen refuses such a module; linking refuses the import first, with the
//! loader's diagnostic. Whatever this accepts, codegen must generate.
use okkama_compiler::{ir, lowering::{self, LinkError, LoadedModule, ModuleLoader}, syntax::ast};

/// A loader with no modules: every import is "not found".
struct NoModules;

impl ModuleLoader for NoModules {
    fn load(&mut self, _path: &str, _importer: &str) -> Result<Option<LoadedModule>, String> {
        Ok(None)
    }
}

/// The linked module of `program` (test items already stripped), `None` for a
/// language or load error; an internal compiler error panics.
pub fn linked(program: &ast::Program) -> Option<ir::Module> {
    match lowering::lower_linked(program, "fuzz.ok", &mut NoModules) {
        Ok(module) => Some(module),
        Err(LinkError::Lowering { error, .. }) => crate::lowering::accepted(Err(error)),
        Err(LinkError::Load(_)) => None,
    }
}
