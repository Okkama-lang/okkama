//! Trusted runtime catalogue for the open synchronous callable ABI (#569 O1;
//! OIR-2 §6.4, §7.1 and §11; P1.3 elaboration §12).
//!
//! An open `fn.call` invokes an action value whose target is unknown. Its only
//! contract is the conservative one registered here, never a declaration of the
//! input module:
//! - parameter 0 is the environment, the explicit arguments are 1…N;
//! - the environment is borrowed: an action value is called many times, and the
//!   caller keeps it alive while dependent results live;
//! - every argument that may own takes its offered fresh part, may retain its
//!   complete owner graph in the result and lists every parameter as a dependency;
//! - a managed result is opaque: a fresh graph or an alias of the environment or
//!   of an input, with external dependencies on all of them; the runtime returns
//!   the actual packet and deps;
//! - both outcomes settle every accepted input that the result does not retain;
//! - the contract is synchronous: no suspension, cancellation or async cleanup.
//!
//! The opaque runtime descriptor of the callable releases the complete owner graph
//! through [`SYNC_CLOSURE_CLEANUP`]. The catalogue registers that intrinsic only
//! for a declaration with the exact ABI below, so a name alone certifies nothing.
//! The intrinsic is a family keyed by the callable type (#569 C3b): one
//! declaration per open `fn` type, each checked against the ABI of its own key.
//! [`ANY_CLEANUP`] is the same kind of entry for `any` boxes (#569 C6).
//! Own (`NativeSummaries`) and the verifier (`ExternalContracts`) consume the same
//! entries; the verifier also recomputes the canonical contract independently.
use crate::*;
use std::collections::BTreeMap;

/// Runtime intrinsic releasing an opaque sync callable's complete owner graph.
pub const SYNC_CLOSURE_CLEANUP: &str = "oir.sync_closure.cleanup";

/// Runtime intrinsic releasing the complete owner graph of an opaque `any` box
/// (#569 C6). It backs the checked runtime descriptor of `any` values whose
/// payload is not visible: open `any` parameters, results and elements.
pub const ANY_CLEANUP: &str = "oir.any.cleanup";

/// The fresh family of an opaque result. Creation keys are qualified by the call
/// origin, so one site identifies the callee's fresh graph at each invocation.
pub const OPEN_RESULT_SITE: u32 = 0;

/// The Closure protocol of the open ABI: publication of the returned packet and
/// the checked dynamic transfer of unknown retention (§6.4, §7.1).
pub fn open_callable_protocol() -> ProtocolDecl {
    let checks = vec![
        ProtocolCheck::Owner, ProtocolCheck::LiveDependencies, ProtocolCheck::Snapshot, ProtocolCheck::Transferable,
    ];
    let open = ProtocolPoststate { holder: HolderKind::Scope, state: HolderState::Open };
    ProtocolDecl {
        kind: ProtocolKind::Closure,
        holders: vec![HolderKind::Scope, HolderKind::Packet],
        rules: vec![
            ProtocolRule {
                transition: ProtocolTransition::Publish,
                from: HolderKind::Packet,
                to: HolderKind::Scope,
                checks: checks.clone(),
                normal: open,
                unwind: Some(ProtocolPoststate { holder: HolderKind::Scope, state: HolderState::Settled }),
                cancel: None,
            },
            ProtocolRule {
                transition: ProtocolTransition::Dynamic,
                from: HolderKind::Packet,
                to: HolderKind::Scope,
                checks,
                normal: open,
                unwind: Some(ProtocolPoststate { holder: HolderKind::Packet, state: HolderState::Unaccepted }),
                cancel: None,
            },
        ],
        roots: vec![RootKind::Operands],
    }
}

/// The Services protocol of plain async (elaboration D16, async ABI D24.2 and
/// D24.4). Own names it in every `await.unwind` and cache schedule, so a module
/// whose bodies hold Futures declares it once:
/// - a cache duty settles into its owner scope, or detaches a running Future to
///   the root registry (D24.2, parity);
/// - an await receipt settles into the awaiting scope (C-3);
/// - a spawned task's duty settles by detaching a running task, with its
///   cache duty, to the root registry (`frame → root`, D24.2, slice P1);
/// - `cache.rehome` moves a cache duty from its owner scope into the pending
///   reserve of the scope's exit (C-4).
///
/// The D23 families arrive with their slices.
pub fn services_protocol() -> ProtocolDecl {
    services_protocol_rules()
}

/// #810 (G1): whether a builtin declaration is a D23 holder the runtime provides: the specialization of a holder
/// family (`async.scope.FutureGroup_<suffix>`), whose runtime word holds the service duties of its members. It is a
/// service holder of its own (its packet carries `ServiceOf{holder, Group}`), not a capability like an Atomic.
pub fn service_holder(name: &str) -> bool {
    name.starts_with("async.scope.FutureGroup_")
}

/// #810: the member Future types a group holder registers: the parameter its `start` intrinsic takes. The group's
/// settle may release their outcomes, so a holder reaches what they reach.
pub fn holder_members(module: &crate::Module, decl: crate::DeclId) -> Vec<crate::TypeId> {
    module.functions.iter().filter_map(|(_, function)| {
        let receiver = function.attrs.iter()
            .any(|attr| matches!(attr, crate::FunctionAttr::Method(owner) if *owner == decl));
        let start = function.attrs.iter().any(|attr| matches!(attr, crate::FunctionAttr::Intrinsic { name, .. }
            if name.starts_with("okkama.scope.FutureGroup.start$")));
        (receiver && start).then(|| function.signature.params.get(1).copied()).flatten()
    }).collect()
}

fn services_protocol_rules() -> ProtocolDecl {
    let checks = vec![ProtocolCheck::Owner, ProtocolCheck::LiveDependencies];
    let settle = |from: HolderKind, holder: HolderKind, state: HolderState| ProtocolRule {
        transition: ProtocolTransition::Settle,
        from,
        to: holder,
        checks: checks.clone(),
        normal: ProtocolPoststate { holder, state },
        unwind: None,
        cancel: None,
    };
    ProtocolDecl {
        kind: ProtocolKind::Services,
        holders: vec![HolderKind::Scope, HolderKind::Cache, HolderKind::Frame, HolderKind::Receipt,
            HolderKind::RootRegistry, HolderKind::PendingExit],
        rules: vec![
            settle(HolderKind::Cache, HolderKind::Scope, HolderState::Settled),
            settle(HolderKind::Receipt, HolderKind::Scope, HolderState::Settled),
            settle(HolderKind::Cache, HolderKind::RootRegistry, HolderState::Running),
            // D24.2, §6.4: a running spawned task detaches with its cache duty to the root registry.
            settle(HolderKind::Frame, HolderKind::RootRegistry, HolderState::Running),
            ProtocolRule {
                transition: ProtocolTransition::Rehome,
                from: HolderKind::Scope,
                to: HolderKind::PendingExit,
                checks: vec![ProtocolCheck::Owner, ProtocolCheck::LiveDependencies, ProtocolCheck::SufficientBoundary],
                normal: ProtocolPoststate { holder: HolderKind::PendingExit, state: HolderState::Open },
                unwind: None,
                cancel: None,
            },
        ],
        roots: vec![],
    }
}

/// The first declaration of the module that is exactly the open Closure protocol.
pub fn open_callable_protocol_id(module: &Module) -> Option<ProtocolId> {
    let expected = open_callable_protocol();
    module.protocols.iter().find(|(_, protocol)| **protocol == expected).map(|(id, _)| id)
}

/// Values of these types own nothing; they need neither transfer nor aliases.
pub fn plain_scalar(module: &Module, ty: TypeId) -> bool {
    matches!(module.types.get(ty), Ok(Type::Int | Type::Bool | Type::Byte | Type::Float | Type::Rat | Type::Char
        | Type::Str | Type::Range | Type::Never | Type::Enum(_)))
}

/// The registered conservative contract for an open sync call of `callable`,
/// in own's normal form. `None` for a non-`fn` or an async `fn` type.
pub fn open_callable_summary(module: &Module, callable: TypeId, protocol: ProtocolId) -> Option<Summary> {
    let Ok(Type::Fn(function)) = module.types.get(callable) else {
        return None;
    };
    if function.is_async {
        return None;
    }
    let count = function.params.len().checked_add(1)?;
    let mut owning = vec![0u16];
    for (index, &ty) in function.params.iter().enumerate() {
        if !plain_scalar(module, ty) {
            owning.push(u16::try_from(index + 1).ok()?);
        }
    }
    let all: Vec<u16> = (0..u16::try_from(count).ok()?).collect();
    let parameters = (0..count).map(|index| {
        let index = index as u16;
        if index != 0 && owning.contains(&index) {
            ParameterEffect {
                convention: FreshConvention::Take,
                retention: Some(SummarySelection::OwnerGraph(index)),
                deps: all.clone(),
                destination: TransferDestination::Result,
            }
        } else {
            ParameterEffect {
                convention: FreshConvention::Borrow,
                retention: None,
                deps: vec![],
                destination: TransferDestination::None,
            }
        }
    }).collect();
    let results = function.result.iter().map(|&ty| {
        if plain_scalar(module, ty) {
            ResultRelation { beside: vec![], held: vec![],
                fresh: vec![], aliases: vec![], external_deps: vec![], opaque: false,
                provenance: AdmissionProvenance::UnboundResult,
            }
        } else {
            ResultRelation { beside: vec![], held: vec![],
                fresh: vec![CreationSiteId::from_u32(OPEN_RESULT_SITE)],
                aliases: owning.iter().map(|index| SummarySelection::OwnerGraph(*index)).collect(),
                external_deps: owning.clone(),
                opaque: true,
                provenance: AdmissionProvenance::UnboundResult,
            }
        }
    }).collect();
    Some(Summary {
        entry_kind: EntryKind::CallBoundary,
        parameters,
        results,
        contract: CallContract {
            results: function.result.iter().copied().collect(),
            failures: vec![FailureKind::Thrown, FailureKind::Program, FailureKind::Panic, FailureKind::Native],
            suspends: false,
            async_destroy: false,
            publication: Publication::Atomic,
            protocol: Some(protocol),
        },
    })
}

/// The cleanup intrinsic takes the complete fresh graph and cannot fail or suspend.
pub fn sync_closure_cleanup_summary() -> Summary {
    Summary {
        entry_kind: EntryKind::CallBoundary,
        parameters: vec![ParameterEffect {
            convention: FreshConvention::Take, retention: None, deps: vec![], destination: TransferDestination::None,
        }],
        results: vec![],
        contract: CallContract {
            results: vec![], failures: vec![], suspends: false, async_destroy: false,
            publication: Publication::Atomic, protocol: None,
        },
    }
}

/// The cleanup family of the runtime catalogue: `(name, key type)` pairs. The
/// closure cleanup takes a registered sync `fn` type, the `any` cleanup `any`.
fn cleanup_admits(callables: &BTreeMap<TypeId, Summary>, module: &Module, name: &str, ty: TypeId) -> bool {
    match name {
        SYNC_CLOSURE_CLEANUP => callables.contains_key(&ty),
        ANY_CLEANUP => module.types.get(ty) == Ok(&Type::Any),
        _ => false,
    }
}

/// The exact ABI of the cleanup intrinsic for `callable` in `profile`.
pub fn sync_closure_cleanup_signature(module: &Module, callable: TypeId, profile: Profile) -> Option<Signature> {
    opaque_cleanup_signature(module, callable, profile)
}

/// The exact ABI of an opaque cleanup intrinsic for its key type (a sync `fn`
/// type or `any`) in `profile`: High is unelaborated; Own and later take
/// `(key; deps, own) -> own`. Before the synchronous rewrite the service types
/// may not be interned yet; they receive the identifiers the rewrite assigns
/// (deps first, then own).
pub fn opaque_cleanup_signature(module: &Module, key: TypeId, profile: Profile) -> Option<Signature> {
    if profile == Profile::High {
        return Some(Signature { params: vec![key], results: vec![], ownership_abi: OwnershipAbi::Unelaborated });
    }
    let mut types = module.types.clone();
    let deps = types.intern(Type::Service(ServiceType::Deps)).ok()?;
    let own = types.intern(Type::Service(ServiceType::Own)).ok()?;
    Some(Signature {
        params: vec![key, deps, own],
        results: vec![own],
        ownership_abi: OwnershipAbi::Explicit { input_packet: 2, output_packet: 0 },
    })
}

/// Entries registered for one module: every sync `fn` type, when the module
/// declares the open Closure protocol, and the cleanup intrinsics. The cleanup
/// family is keyed by `(name, type)`, the type being the first parameter: one
/// closure cleanup per open `fn` type, so a module can invoke several open `fn`
/// types, and one `any` cleanup. A declaration is registered only with the exact
/// ABI for its key; two declarations for one key register none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeCatalogue {
    callables: BTreeMap<TypeId, Summary>,
    cleanup: BTreeMap<(&'static str, TypeId), Signature>,
    cleanup_summary: Summary,
}

impl Default for RuntimeCatalogue {
    fn default() -> Self {
        Self { callables: BTreeMap::new(), cleanup: BTreeMap::new(), cleanup_summary: sync_closure_cleanup_summary() }
    }
}

impl RuntimeCatalogue {
    pub fn for_module(module: &Module) -> Self {
        Self::for_profile(module, module.profile)
    }

    /// Entries for the ABI of `profile`. Elaboration uses its target profile:
    /// the declaration may still carry the unelaborated High signature.
    pub fn for_profile(module: &Module, profile: Profile) -> Self {
        let mut catalogue = Self::default();
        if let Some(protocol) = open_callable_protocol_id(module) {
            for (id, _) in module.types.iter() {
                if let Some(summary) = open_callable_summary(module, id, protocol) {
                    catalogue.callables.insert(id, summary);
                }
            }
        }
        let mut keys = BTreeMap::<(&'static str, TypeId), usize>::new();
        let mut exact = BTreeMap::new();
        for (_, function) in module.functions.iter() {
            let name = function.attrs.iter().find_map(|attr| match attr {
                FunctionAttr::Intrinsic { name, .. } if name == SYNC_CLOSURE_CLEANUP => Some(SYNC_CLOSURE_CLEANUP),
                FunctionAttr::Intrinsic { name, .. } if name == ANY_CLEANUP => Some(ANY_CLEANUP),
                _ => None,
            });
            let (Some(name), Some(key)) = (name, function.signature.params.first().copied()) else {
                continue;
            };
            *keys.entry((name, key)).or_default() += 1;
            if let Some(signature) = catalogue.exact_cleanup(module, function, name, key, profile) {
                exact.insert((name, key), signature);
            }
        }
        exact.retain(|key, _| keys.get(key) == Some(&1));
        catalogue.cleanup = exact;
        catalogue
    }

    /// The registered signature of `function` when it is exactly the cleanup
    /// intrinsic `name` for the admitted key type `key`.
    fn exact_cleanup(&self, module: &Module, function: &Function, name: &str, key: TypeId, profile: Profile)
        -> Option<Signature> {
        let exact_attrs = function.body.is_none() && function.type_params.is_empty()
            && function.attrs.iter().all(|attr| match attr {
                FunctionAttr::Intrinsic { name: actual, objects, helpers } => {
                    actual == name && objects.is_empty() && helpers.is_empty()
                }
                _ => false,
            });
        if !exact_attrs || !cleanup_admits(&self.callables, module, name, key) {
            return None;
        }
        let declared = |profile| opaque_cleanup_signature(module, key, profile);
        let matches = [Profile::High, profile].into_iter()
            .any(|profile| declared(profile).as_ref() == Some(&function.signature));
        if matches { declared(profile) } else { None }
    }

    pub fn open_callable(&self, callable: TypeId) -> Option<&Summary> {
        self.callables.get(&callable)
    }

    /// The registered cleanup ABI for a declaration of `name` with the
    /// signature `declared`: the entry keyed by its first parameter.
    pub fn cleanup_signature(&self, name: &str, declared: &Signature) -> Option<&Signature> {
        let name = [SYNC_CLOSURE_CLEANUP, ANY_CLEANUP].into_iter().find(|known| *known == name)?;
        declared.params.first().and_then(|key| self.cleanup.get(&(name, *key)))
    }

    /// The cleanup contract is the same for every key; it exists only while
    /// at least one declaration of the named family is registered.
    pub fn cleanup_summary(&self, name: &str) -> Option<&Summary> {
        self.cleanup.keys().any(|(registered, _)| *registered == name).then_some(&self.cleanup_summary)
    }

    /// The key types whose cleanup declaration `name` is registered.
    pub fn cleanup_keys<'a>(&'a self, name: &'a str) -> impl Iterator<Item = TypeId> + 'a {
        self.cleanup.keys().filter(move |(registered, _)| *registered == name).map(|(_, key)| *key)
    }
}

impl verify::ExternalContracts for RuntimeCatalogue {
    fn native(&self, _: &str) -> Option<&Signature> {
        None
    }
    fn intrinsic(&self, _: &str) -> Option<&Signature> {
        None
    }
    fn intrinsic_declaration(&self, name: &str, declared: &Signature) -> Option<&Signature> {
        self.cleanup_signature(name, declared)
    }
    fn intrinsic_summary(&self, name: &str) -> Option<&Summary> {
        self.cleanup_summary(name)
    }
    fn descriptor_cleanup(&self, name: &str) -> Option<&Summary> {
        self.cleanup_summary(name)
    }
    fn open_callable(&self, callable: TypeId) -> Option<Summary> {
        RuntimeCatalogue::open_callable(self, callable).cloned()
    }
}
