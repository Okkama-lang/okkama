use super::*;

const SOURCE: &str = "oir 2 high
module creation
object R {
    destroy @destroy
}
scope SD action in @destroy
scope SW action in @work
scope SV action in @void
func @destroy(%self: R) -> void [destroy R] {
^entry in SD:
    ret
}
func @work(%n: int, %r: R) -> int [async] {
^entry in SW:
    ret %n
}
func @void(%n: int) -> void [async] {
^entry in SV:
    ret
}
";

fn prepared() -> (Module, BTreeMap<TypeId, TypeClassification>) {
    let mut module = crate::text::parse(SOURCE).unwrap();
    intern_futures(&mut module).unwrap();
    let options = OwnOptions::default();
    let types = classify::classify(&module, &options, &mut Budget::new(&options)).unwrap();
    (module, types)
}

fn function<'m>(module: &'m Module, symbol: &str) -> &'m Function {
    module.functions.iter().find(|(_, function)| function.symbol == symbol).unwrap().1
}

#[test]
fn creation_retains_offered_cold_inputs_and_their_external_deps() {
    // Elaboration H6, §3.3: creation keeps every managed input for the Future's
    // inputs holder even though the body only borrows it; plain inputs offer nothing.
    let (module, types) = prepared();
    let options = OwnOptions::default();
    let summary = creation_summary(&module, function(&module, "work"), &types, &mut Budget::new(&options)).unwrap();
    let int = module.types.iter().find_map(|(id, ty)| (*ty == Type::Int).then_some(id)).unwrap();
    assert_eq!(summary.entry_kind, EntryKind::CallBoundary);
    assert_eq!(summary.parameters, [
        ParameterEffect {
            convention: FreshConvention::Borrow, retention: None, deps: vec![], destination: TransferDestination::None,
        },
        ParameterEffect {
            convention: FreshConvention::Take, retention: Some(SummarySelection::Parameter(1)), deps: vec![1],
            destination: TransferDestination::Cache,
        },
    ]);
    // The cache duty goes to the owner state (D12): the Future has no fresh family.
    assert_eq!(summary.results, [ResultRelation { beside: vec![], held: vec![], several: false,
        fresh: vec![], aliases: vec![SummarySelection::Parameter(1)], external_deps: vec![1], opaque: false,
        provenance: AdmissionProvenance::UnboundResult,
    }]);
    assert_eq!(summary.contract, CallContract {
        results: vec![future_type(&module, Some(int)).unwrap()], failures: vec![], suspends: false,
        async_destroy: false, publication: Publication::Atomic, protocol: None,
    });
}

#[test]
fn a_void_entry_creates_a_future_of_void() {
    let (module, types) = prepared();
    let options = OwnOptions::default();
    let summary = creation_summary(&module, function(&module, "void"), &types, &mut Budget::new(&options)).unwrap();
    assert_eq!(summary.contract.results, [future_type(&module, None).unwrap()]);
    assert_eq!(outcome(&module, function(&module, "void")).unwrap(), None);
}

#[test]
fn a_cache_settle_neither_fails_nor_waits() {
    // Async ABI D24.2: no service settle waits in 0.6.0; a cold Future's
    // Cancelled is observed, never an escaping cleanup failure.
    let summary = settle_summary();
    assert!(summary.contract.failures.is_empty());
    assert!(!summary.contract.suspends && !summary.contract.async_destroy);
    assert_eq!(summary.entry_kind, EntryKind::Internal);
}

#[test]
fn async_entries_with_resource_inputs_pass_the_boundary() {
    // Commit 5b (R1): an async entry's resource inputs are cold Future inputs, retained by the creation.
    let (module, _) = prepared();
    let options = OwnOptions::default();
    let mut budget = Budget::new(&options);
    for (id, _) in module.functions.iter() {
        check_boundary(&module, id, &mut budget).unwrap();
    }
}

#[test]
fn the_services_protocol_is_unique() {
    let services = "protocol Services kind services\n";
    let source = |count: usize| SOURCE.replace("scope SD", &format!("{}scope SD", services.repeat(count)));
    let none = crate::text::parse(&source(0)).unwrap();
    assert!(matches!(services_protocol(&none), Err(OwnError::Diagnostics { .. })));
    let one = crate::text::parse(&source(1)).unwrap();
    assert_eq!(services_protocol(&one).unwrap(), ProtocolId::from_u32(0));
    let two = crate::text::parse(&source(2).replacen("protocol Services", "protocol Other", 1)).unwrap();
    assert!(matches!(services_protocol(&two), Err(OwnError::Diagnostics { .. })));
}
