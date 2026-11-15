use okkama_oir::catalogue::{RuntimeCatalogue, SYNC_CLOSURE_CLEANUP};
use okkama_oir::verify::ExternalContracts;

// The fixture files are the frozen native test ABI catalogue. Production
// verification never registers external contracts from input declarations.
pub struct FixtureContracts<'a>(pub &'a okkama_oir::Module);
impl ExternalContracts for FixtureContracts<'_> {
    fn native(&self, name: &str) -> Option<&okkama_oir::Signature> {
        self.0.functions.iter().find_map(|(_, f)| f.attrs.iter().any(|a| {
            matches!(a, okkama_oir::FunctionAttr::Extern(n) if n == name && n.starts_with("fixture."))
        }).then_some(&f.signature))
    }
    fn intrinsic(&self, name: &str) -> Option<&okkama_oir::Signature> {
        self.intrinsic_function(name).map(|function| &function.signature)
    }
    fn intrinsic_declaration(&self, name: &str, declared: &okkama_oir::Signature)
        -> Option<&okkama_oir::Signature> {
        if name == SYNC_CLOSURE_CLEANUP {
            return self.runtime_cleanup(Some(declared)).map(|function| &function.signature);
        }
        self.intrinsic(name)
    }
    fn intrinsic_summary(&self, name: &str) -> Option<&okkama_oir::Summary> {
        self.intrinsic_function(name).and_then(|function| function.summary)
            .and_then(|id| self.0.summaries.get(id).ok())
    }
    fn descriptor_cleanup(&self, name: &str) -> Option<&okkama_oir::Summary> {
        match name {
            "fixture.cleanup.fn" | "fixture.cleanup.any" | "fixture.cleanup.borrow" | SYNC_CLOSURE_CLEANUP => {
                self.intrinsic_summary(name)
            }
            _ => None,
        }
    }
    fn open_callable(&self, callable: okkama_oir::TypeId) -> Option<okkama_oir::Summary> {
        RuntimeCatalogue::for_module(self.0).open_callable(callable).cloned()
    }
    fn native_summary(&self, name: &str) -> Option<&okkama_oir::Summary> {
        self.0.functions.iter().find_map(|(_, f)| f.attrs.iter().any(|a| {
            matches!(a, okkama_oir::FunctionAttr::Extern(n) if n == name && n.starts_with("fixture."))
        }).then_some(f.summary).flatten()).and_then(|id| self.0.summaries.get(id).ok())
    }
}


impl FixtureContracts<'_> {
    fn intrinsic_function(&self, name: &str) -> Option<&okkama_oir::Function> {
        if name == SYNC_CLOSURE_CLEANUP {
            return self.runtime_cleanup(None);
        }
        if !matches!(name, "fixture.cleanup.fn" | "fixture.cleanup.any" | "fixture.cleanup.borrow"
            | "fixture.ordinary" | "fixture.source") {
            return None;
        }
        self.0.functions.iter().find_map(|(_, function)| function.attrs.iter().any(|attr| {
            matches!(attr, okkama_oir::FunctionAttr::Intrinsic { name: actual, .. } if actual == name)
        }).then_some(function))
    }
}

impl FixtureContracts<'_> {
    // The runtime cleanup intrinsic is trusted only as the catalogue registers it:
    // its declaration must carry exactly the catalogue's ABI and summary. The
    // family is keyed by type: `declared` selects the declaration of one key.
    fn runtime_cleanup(&self, declared: Option<&okkama_oir::Signature>) -> Option<&okkama_oir::Function> {
        let catalogue = RuntimeCatalogue::for_module(self.0);
        self.0.functions.iter().map(|(_, function)| function).find(|function| {
            function.attrs.iter().any(|attr| {
                matches!(attr, okkama_oir::FunctionAttr::Intrinsic { name, .. } if name == SYNC_CLOSURE_CLEANUP)
            }) && declared.is_none_or(|declared| *declared == function.signature)
                && catalogue.cleanup_signature(SYNC_CLOSURE_CLEANUP, &function.signature) == Some(&function.signature)
                && function.summary.and_then(|id| self.0.summaries.get(id).ok())
                    == catalogue.cleanup_summary(SYNC_CLOSURE_CLEANUP)
        })
    }
}
