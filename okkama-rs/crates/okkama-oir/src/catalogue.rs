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
//! Own (`NativeSummaries`) and the verifier (`ExternalContracts`) consume the same
//! entries; the verifier also recomputes the canonical contract independently.
use crate::*;
use std::collections::BTreeMap;

/// Runtime intrinsic releasing an opaque sync callable's complete owner graph.
pub const SYNC_CLOSURE_CLEANUP: &str = "oir.sync_closure.cleanup";

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
            ResultRelation {
                fresh: vec![], aliases: vec![], external_deps: vec![], opaque: false,
                provenance: AdmissionProvenance::UnboundResult,
            }
        } else {
            ResultRelation {
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

/// The exact ABI of the cleanup intrinsic for `callable` in `profile`:
/// High is unelaborated; Own and later take `(fn; deps, own) -> own`.
/// Before the synchronous rewrite the service types may not be interned yet;
/// they receive the identifiers the rewrite assigns (deps first, then own).
pub fn sync_closure_cleanup_signature(module: &Module, callable: TypeId, profile: Profile) -> Option<Signature> {
    if profile == Profile::High {
        return Some(Signature { params: vec![callable], results: vec![], ownership_abi: OwnershipAbi::Unelaborated });
    }
    let mut types = module.types.clone();
    let deps = types.intern(Type::Service(ServiceType::Deps)).ok()?;
    let own = types.intern(Type::Service(ServiceType::Own)).ok()?;
    Some(Signature {
        params: vec![callable, deps, own],
        results: vec![own],
        ownership_abi: OwnershipAbi::Explicit { input_packet: 2, output_packet: 0 },
    })
}

/// Entries registered for one module: every sync `fn` type, when the module
/// declares the open Closure protocol, and the cleanup intrinsic when its single
/// declaration has the exact ABI for one of those types.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuntimeCatalogue {
    callables: BTreeMap<TypeId, Summary>,
    cleanup: Option<(Signature, Summary)>,
}

impl RuntimeCatalogue {
    pub fn for_module(module: &Module) -> Self {
        Self::for_profile(module, module.profile)
    }

    /// Entries for the ABI of `profile`. Elaboration uses its target profile:
    /// the declaration may still carry the unelaborated High signature.
    pub fn for_profile(module: &Module, profile: Profile) -> Self {
        let mut catalogue = Self::default();
        let Some(protocol) = open_callable_protocol_id(module) else {
            return catalogue;
        };
        for (id, _) in module.types.iter() {
            if let Some(summary) = open_callable_summary(module, id, protocol) {
                catalogue.callables.insert(id, summary);
            }
        }
        let mut declarations = module.functions.iter().filter(|(_, function)| {
            function.attrs.iter().any(|attr| {
                matches!(attr, FunctionAttr::Intrinsic { name, .. } if name == SYNC_CLOSURE_CLEANUP)
            })
        });
        let (Some((_, function)), None) = (declarations.next(), declarations.next()) else {
            return catalogue;
        };
        let exact_attrs = function.body.is_none() && function.type_params.is_empty()
            && function.attrs.iter().all(|attr| match attr {
                FunctionAttr::Intrinsic { name, objects, helpers } => {
                    name == SYNC_CLOSURE_CLEANUP && objects.is_empty() && helpers.is_empty()
                }
                _ => false,
            });
        let callable = function.signature.params.first().copied()
            .filter(|ty| catalogue.callables.contains_key(ty));
        let declared = |profile| callable.and_then(|ty| sync_closure_cleanup_signature(module, ty, profile));
        let matches = [Profile::High, profile].into_iter()
            .any(|profile| declared(profile).as_ref() == Some(&function.signature));
        if exact_attrs && matches && let Some(signature) = declared(profile) {
            catalogue.cleanup = Some((signature, sync_closure_cleanup_summary()));
        }
        catalogue
    }

    pub fn open_callable(&self, callable: TypeId) -> Option<&Summary> {
        self.callables.get(&callable)
    }

    pub fn cleanup_signature(&self, name: &str) -> Option<&Signature> {
        self.cleanup.as_ref().filter(|_| name == SYNC_CLOSURE_CLEANUP).map(|(signature, _)| signature)
    }

    pub fn cleanup_summary(&self, name: &str) -> Option<&Summary> {
        self.cleanup.as_ref().filter(|_| name == SYNC_CLOSURE_CLEANUP).map(|(_, summary)| summary)
    }
}

impl verify::ExternalContracts for RuntimeCatalogue {
    fn native(&self, _: &str) -> Option<&Signature> {
        None
    }
    fn intrinsic(&self, name: &str) -> Option<&Signature> {
        self.cleanup_signature(name)
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
