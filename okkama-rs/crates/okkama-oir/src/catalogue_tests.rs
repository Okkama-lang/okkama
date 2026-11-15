//! The runtime catalogue's cleanup family is keyed by type (#569 C3b, C6): one
//! closure cleanup per open `fn` type and one `any` cleanup, each registered only
//! with the exact ABI of its own key.
use crate::catalogue::{ANY_CLEANUP, RuntimeCatalogue, SYNC_CLOSURE_CLEANUP};
use crate::verify::{RuleId, VerifyContext, VerifyOptions, verify_with};
use crate::*;

const SOURCE: &str = include_str!("catalogue-two-callables.oir");

fn module() -> Module {
    text::parse(SOURCE).unwrap()
}

fn cleanup(module: &Module, symbol: &str) -> FuncId {
    module.functions.iter().find_map(|(id, function)| (function.symbol == symbol).then_some(id)).unwrap()
}

fn signature(module: &Module, symbol: &str) -> Signature {
    module.functions.get(cleanup(module, symbol)).unwrap().signature.clone()
}

fn check(module: &Module) -> Result<(), Vec<RuleId>> {
    let catalogue = RuntimeCatalogue::for_module(module);
    let context = VerifyContext { text_locations: None, external_contracts: &catalogue };
    verify_with(module, &VerifyOptions::default(), context).map(|_| ())
        .map_err(|report| report.diagnostics.iter().map(|diagnostic| diagnostic.rule).collect())
}

#[test]
fn every_open_fn_type_registers_its_own_cleanup() {
    let module = module();
    let catalogue = RuntimeCatalogue::for_module(&module);
    assert_eq!(catalogue.cleanup_keys(SYNC_CLOSURE_CLEANUP).count(), 2);
    for symbol in ["drop_fn", "drop_int"] {
        let declared = signature(&module, symbol);
        assert_eq!(catalogue.cleanup_signature(SYNC_CLOSURE_CLEANUP, &declared), Some(&declared), "{symbol}");
    }
    assert!(catalogue.cleanup_summary(SYNC_CLOSURE_CLEANUP).is_some());
    check(&module).unwrap_or_else(|rules| panic!("{rules:?}"));
}

#[test]
fn own_profile_registers_the_elaborated_abi_per_key() {
    let module = module();
    let catalogue = RuntimeCatalogue::for_profile(&module, Profile::Own);
    for symbol in ["drop_fn", "drop_int", "drop_any"] {
        let declared = signature(&module, symbol);
        let elaborated = catalogue::opaque_cleanup_signature(&module, declared.params[0], Profile::Own).unwrap();
        let name = if symbol == "drop_any" { ANY_CLEANUP } else { SYNC_CLOSURE_CLEANUP };
        assert_eq!(catalogue.cleanup_signature(name, &declared), Some(&elaborated), "{symbol}");
        assert_eq!(elaborated.ownership_abi, OwnershipAbi::Explicit { input_packet: 2, output_packet: 0 });
    }
}

#[test]
fn two_declarations_of_one_key_register_neither() {
    let mut module = module();
    let mut duplicate = module.functions.get(cleanup(&module, "drop_int")).unwrap().clone();
    duplicate.symbol = "drop_int_again".into();
    module.functions.alloc(duplicate).unwrap();
    let catalogue = RuntimeCatalogue::for_module(&module);
    assert_eq!(catalogue.cleanup_keys(SYNC_CLOSURE_CLEANUP).count(), 1);
    assert!(catalogue.cleanup_signature(SYNC_CLOSURE_CLEANUP, &signature(&module, "drop_int")).is_none());
    assert!(check(&module).unwrap_err().contains(&RuleId::T5));
}

#[test]
fn a_cleanup_for_an_unregistered_key_is_not_trusted() {
    let mut module = module();
    let int = module.types.intern(Type::Int).unwrap();
    let id = cleanup(&module, "drop_int");
    module.functions.get_mut(id).unwrap().signature.params = vec![int];
    let catalogue = RuntimeCatalogue::for_module(&module);
    assert!(catalogue.cleanup_signature(SYNC_CLOSURE_CLEANUP, &signature(&module, "drop_int")).is_none());
    assert!(check(&module).unwrap_err().contains(&RuleId::T5));
}

#[test]
fn the_any_cleanup_takes_only_any() {
    let module = module();
    let catalogue = RuntimeCatalogue::for_module(&module);
    let declared = signature(&module, "drop_any");
    assert_eq!(catalogue.cleanup_signature(ANY_CLEANUP, &declared), Some(&declared));
    assert!(catalogue.cleanup_summary(ANY_CLEANUP).is_some());
    // The names are not interchangeable: the closure family does not take `any`.
    assert!(catalogue.cleanup_signature(SYNC_CLOSURE_CLEANUP, &declared).is_none());
    let mut module = module;
    let id = cleanup(&module, "drop_any");
    let callable = signature(&module, "drop_fn").params[0];
    module.functions.get_mut(id).unwrap().signature.params = vec![callable];
    let catalogue = RuntimeCatalogue::for_module(&module);
    assert!(catalogue.cleanup_summary(ANY_CLEANUP).is_none());
    assert!(check(&module).unwrap_err().contains(&RuleId::T5));
}

#[test]
fn the_any_cleanup_needs_its_exact_abi() {
    let mut module = module();
    let int = module.types.intern(Type::Int).unwrap();
    let id = cleanup(&module, "drop_any");
    module.functions.get_mut(id).unwrap().signature.results = vec![int];
    assert!(RuntimeCatalogue::for_module(&module).cleanup_summary(ANY_CLEANUP).is_none());
    assert!(check(&module).unwrap_err().contains(&RuleId::T5));
}

#[test]
fn closure_cleanups_need_the_open_protocol_but_any_does_not() {
    let mut module = module();
    let protocols: Vec<_> = module.protocols.iter().map(|(id, _)| id).collect();
    for id in protocols {
        module.protocols.get_mut(id).unwrap().roots.clear();
    }
    let catalogue = RuntimeCatalogue::for_module(&module);
    assert_eq!(catalogue.cleanup_keys(SYNC_CLOSURE_CLEANUP).count(), 0);
    assert_eq!(catalogue.cleanup_keys(ANY_CLEANUP).count(), 1);
}

/// A descriptor of one open `fn` type cannot carry the cleanup registered for another (#569 C3b): the
/// cleanup family is keyed by type, and the runtime would run another key's entry on this type's values.
#[test]
fn a_descriptor_names_the_cleanup_registered_for_its_own_type() {
    let swaps = [
        ("retention=owner_graph(0), cleanup=@drop_fn", "retention=owner_graph(0), cleanup=@drop_int"),
        ("retention=owner_graph(0), cleanup=@drop_int", "retention=owner_graph(0), cleanup=@drop_fn"),
        ("retention=owner_graph(0), cleanup=@drop_any", "retention=owner_graph(0), cleanup=@drop_int"),
    ];
    for (registered, other) in swaps {
        assert_eq!(SOURCE.matches(registered).count(), 1, "{registered}");
        let module = text::parse(&SOURCE.replacen(registered, other, 1)).unwrap();
        assert!(check(&module).unwrap_err().contains(&RuleId::B111), "{other}");
    }
}
