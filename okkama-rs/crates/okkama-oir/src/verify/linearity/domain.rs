use crate::*;
use super::per_value::{AllocationSize, OriginBounds, PerValue};
use std::rc::Rc;
use super::shared::SharedSet;
use std::collections::{BTreeMap, BTreeSet};

pub(super) const ABSENT: u8 = 1;
pub(super) const AVAILABLE: u8 = 2;
pub(super) const CONSUMED: u8 = 4;
pub(super) const FRESH: u8 = 1 | 32;

/// Origins are static families, including creations inside loops. A journal
/// stores a family once, rather than enumerating its dynamic incarnations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Origin {
    Input(ValueId),
    Instruction(InstId, u32),
    Invoke(BlockId, u32),
    External(ValueId),
    RetiredInstruction(InstId, u32),
    RetiredInvoke(BlockId, u32),
    Projection(ValueId, ProjectionKind),
    RetiredProjection(ValueId, ProjectionKind),
    ViewInstruction(InstId, u16),
    ViewInvoke(BlockId, u16),
    RetiredViewInstruction(InstId, u16),
    RetiredViewInvoke(BlockId, u16),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum ProjectionKind {
    Field(DeclId, u32),
    Path { fields: [(DeclId, u32); 4], len: u8 },
    Index(ValueId),
    Slice(ValueId, ValueId),
    Map(ValueId),
    MapIndex(ValueId, bool),
    Keys,
    Values,
    Remove(ValueId),
}
type Fields = BTreeMap<(Origin, DeclId, u32), (BTreeSet<Origin>, Option<Origin>)>;

/// A finite partition cell. Opposite predicates prove disjointness, including
/// when the contents of an ABI input are not statically enumerable.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Filter {
    pub keys: BTreeSet<Origin>,
    pub fresh: bool,
    pub possible: BTreeSet<Origin>,
    pub labels: SharedSet<ValueId>,
    pub stale: bool,
    /// A check reads the whole graph of a returned Future, its cache's holdings included; it extracts
    /// nothing. No selection sets it (C-4).
    pub whole_future: bool,
    pub state: u8,
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Piece {
    pub origin: Origin,
    pub predicates: BTreeMap<Filter, bool>,
    pub provenance: u8,
    pub owner: Option<ScopeId>,
    pub journal: bool,
    pub family: bool,
    pub holder: Option<Origin>,
    pub channel: bool,
}
impl Filter {
    /// Filters that name the same stable keys, possible graph, admission filter
    /// and piece state select the same cell of a piece. Labels name SSA aliases of
    /// the selecting value; they distinguish incarnations only for retired
    /// families, so elsewhere a forwarded alias name does not change the cell.
    pub fn same_cell(&self, other: &Self) -> bool {
        let retired = |key: &Origin| matches!(key, Origin::RetiredInstruction(..) | Origin::RetiredInvoke(..)
            | Origin::RetiredProjection(..) | Origin::RetiredViewInstruction(..) | Origin::RetiredViewInvoke(..));
        self.keys == other.keys && self.fresh == other.fresh && self.possible == other.possible
            && self.stale == other.stale && self.whole_future == other.whole_future && self.state == other.state
            && (self.labels == other.labels || !self.keys.iter().any(retired))
    }
}
impl Piece {
    pub fn new(origin: Origin, provenance: AdmissionProvenance) -> Self {
        Self {
            origin,
            predicates: BTreeMap::new(),
            provenance: provenance_bit(provenance),
            owner: None,
            journal: false,
            family: false,
            holder: None,
            channel: false,
        }
    }
    /// The recorded side of this piece for `filter`'s cell (#569 O1): an exact
    /// predicate, else one recorded by a non-stale filter selecting the same cell.
    pub fn predicate(&self, filter: &Filter) -> Option<bool> {
        if let Some(value) = self.predicates.get(filter) {
            return Some(*value);
        }
        self.predicates.iter().find(|(old, _)| !old.stale && old.same_cell(filter)).map(|(_, value)| *value)
    }
    pub fn overlaps(&self, other: &Self) -> bool {
        self.origin == other.origin && self.journal == other.journal && !self.predicates.iter().any(|(key, value)| {
            !key.stale && other.predicates.get(key).is_some_and(|other| value != other)
        })
    }
}
pub(super) fn provenance_bit(provenance: AdmissionProvenance) -> u8 {
    match provenance {
        AdmissionProvenance::UnboundResult => 1,
        AdmissionProvenance::Bound => 2,
        AdmissionProvenance::StagedOrCarrierOwned => 4,
        AdmissionProvenance::CacheOwned => 8,
        AdmissionProvenance::ExtractedAtExistingBoundary => 16,
        AdmissionProvenance::TemporaryAwaitResult => 32,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CleanupFact {
    pub aborted: bool,
    pub phase: u8,
    pub pending: bool,
    pub reserve: BTreeSet<Piece>,
    pub inventory: BTreeSet<Piece>,
    pub failures: u8,
    pub failure_origin: Option<Origin>,
    pub cancel_delivered: bool,
    /// D21 body: `cleanup.deliver_cancel` ran and the next selector decides whether it delivered.
    pub cancel_open: bool,
    pub may_fail: bool,
    pub awaiting_failure: bool,
    pub awaiting_source: Option<ValueId>,
    pub awaiting_origin: Option<Origin>,
    pub selected: Option<Origin>,
    pub executed: BTreeSet<Origin>,
    pub services: BTreeSet<Origin>,
    pub had_services: bool,
    pub close: CleanupCloseId,
    pub plan: Option<CleanupPlanId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DictionarySource { Instruction(InstId), Parameter(ValueId) }

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Token {
    pub pieces: BTreeSet<Piece>,
    pub scope: Option<ScopeId>,
    pub reserve: BTreeSet<ScopeId>,
    pub destinations: BTreeSet<ScopeId>,
    pub sources: SharedSet<ValueId>,
    pub selected_scope: Option<ScopeId>,
    pub selection: Option<SelectionId>,
    pub bound_admission: bool,
    pub private: bool,
    pub service: Option<Origin>,
    pub cell: Option<Origin>,
    pub dictionary: Option<DictionarySource>,
    pub unique: Option<Origin>,
    pub frame: Option<Origin>,
    pub cache: Option<Origin>,
    pub unique_region: BTreeSet<BlockId>,
    pub saved_type: Option<TypeId>,
    pub pin_deps: BTreeSet<Origin>,
    pub settled: bool,
    pub finish_origin: Option<Origin>,
    pub pending_type: Option<TypeId>,
    pub pending_kind: Option<ExitKind>,
    pub pending_boundary: Option<BoundaryId>,
    /// The destination named by the `cache.rehome`s into this pending exit (C-4).
    pub rehome: Option<RehomeTarget>,
    pub failure_origin: Option<Origin>,
    pub cleanup: Option<CleanupFact>,
    pub snapshot: Option<(BTreeSet<Origin>, bool)>,

}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Flow {
    pub availability: Vec<u8>,
    pub tokens: PerValue<Token>,
    pub journals: PerValue<Token>,
    pub refs: PerValue<BTreeSet<Origin>>,
    pub full_deps: PerValue<BTreeSet<Origin>>,
    pub lifetimes: PerValue<u8>,
    pub identities: PerValue<Option<Origin>>,
    pub dead: BTreeSet<Origin>,
    pub taken_caches: BTreeSet<Origin>,
    pub cache_keys: BTreeSet<Origin>,
    pub provenance: BTreeMap<Origin, u8>,
    pub scopes: BTreeMap<ScopeId, u8>,
    pub bindings: BTreeSet<BindingId>,
    pub must_use: PerValue<u8>,
    pub must_use_aliases: PerValue<SharedSet<ValueId>>,
    pub entry_accepted: bool,
    pub ended: BTreeSet<ScopeId>,
    pub closed_inputs: BTreeSet<Piece>,
    pub failures: PerValue<u8>,
    pub active_failure: u8,
    pub active_failure_origin: Option<Origin>,
    pub latest_failure: u8,
    pub latest_failure_origin: Option<Origin>,
    pub fields: Fields,
    pub aliases: PerValue<SharedSet<ValueId>>,
    pub contained: PerValue<SharedSet<ValueId>>,
    pub views: BTreeMap<Origin, BTreeSet<Origin>>,
    pub witnesses: Rc<Vec<bool>>,
    pub frames: BTreeMap<Origin, BTreeMap<u32, super::Slot>>,
    pub frame_conflict: bool,
    pub cells: BTreeMap<Origin, super::Slot>,
}
impl Flow {
    pub fn token_values(&self) -> impl Iterator<Item = ValueId> + '_ {
        self.tokens.indexed().map(|(index, _)| ValueId::from_u32(index as u32))
    }
    pub fn dependency_values(&self) -> impl Iterator<Item = ValueId> + '_ {
        self.full_deps.indexed().filter(|(_, refs)| !refs.is_empty())
            .map(|(index, _)| ValueId::from_u32(index as u32))
    }
    pub fn new(values: usize, witnesses: Rc<Vec<bool>>) -> Self {
        Self {
            availability: vec![ABSENT; values], tokens: PerValue::new(values, Token::default()),
            journals: PerValue::new(values, Token::default()),
            refs: PerValue::new(values, BTreeSet::new()), full_deps: PerValue::new(values, BTreeSet::new()),
            lifetimes: PerValue::new(values, 1),
            identities: PerValue::new(values, None),
            dead: BTreeSet::new(),
            taken_caches: BTreeSet::new(),
            cache_keys: BTreeSet::new(),
            provenance: BTreeMap::new(),
            scopes: BTreeMap::new(), bindings: BTreeSet::new(),
            must_use: PerValue::new(values, 0), entry_accepted: true,
            must_use_aliases: PerValue::new(values, SharedSet::new()),
            ended: BTreeSet::new(),
            closed_inputs: BTreeSet::new(),
            failures: PerValue::new(values, 0),
            active_failure: 0,
            active_failure_origin: None,
            latest_failure: 0,
            latest_failure_origin: None,
            fields: BTreeMap::new(),
            aliases: PerValue::new(values, SharedSet::new()),
            contained: PerValue::new(values, SharedSet::new()),
            views: BTreeMap::new(),
            witnesses,
            frames: BTreeMap::new(),
            frame_conflict: false,
            cells: BTreeMap::new(),
        }
    }
    /// Only availability is joined independently. All relational components
    /// stay together, so a phi cannot pair one edge's owner with another's packet.
    pub fn same_relations(&self, other: &Self, state: &mut super::State<'_>) -> bool {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("domain::same_relations");
        if !state.step(self.cells.work().saturating_add(other.cells.work())) || self.cells != other.cells {
            return false;
        }
        if !state.step(1) || self.frame_conflict != other.frame_conflict {
            return false;
        }
        if !state.step(self.frames.work().saturating_add(other.frames.work())) || self.frames != other.frames {
            return false;
        }
        if !self.tokens.equal(&other.tokens, |work| state.step(work), AllocationSize::work) {
            return false;
        }
        if !self.journals.equal(&other.journals, |work| state.step(work), AllocationSize::work) {
            return false;
        }
        if !self.refs.equal(&other.refs, |work| state.step(work), AllocationSize::work) {
            return false;
        }
        if !self.full_deps.equal(&other.full_deps, |work| state.step(work), AllocationSize::work) {
            return false;
        }
        if !self.lifetimes.equal(&other.lifetimes, |work| state.step(work), AllocationSize::work) {
            return false;
        }
        if !self.identities.equal(&other.identities, |work| state.step(work), AllocationSize::work) {
            return false;
        }
        if !state.step(self.dead.work().saturating_add(other.dead.work())) || self.dead != other.dead {
            return false;
        }
        if !state.step(self.taken_caches.work().saturating_add(other.taken_caches.work()))
            || self.taken_caches != other.taken_caches {
            return false;
        }
        if !state.step(self.cache_keys.work().saturating_add(other.cache_keys.work()))
            || self.cache_keys != other.cache_keys {
            return false;
        }
        if !state.step(self.provenance.work().saturating_add(other.provenance.work()))
            || self.provenance != other.provenance {
            return false;
        }
        if !state.step(self.scopes.work().saturating_add(other.scopes.work())) || self.scopes != other.scopes {
            return false;
        }
        if !state.step(self.bindings.work().saturating_add(other.bindings.work())) || self.bindings != other.bindings {
            return false;
        }
        if !self.must_use.equal(&other.must_use, |work| state.step(work), AllocationSize::work) {
            return false;
        }
        if !self.must_use_aliases.equal(&other.must_use_aliases, |work| state.step(work), AllocationSize::work) {
            return false;
        }
        if !state.step(1) || self.entry_accepted != other.entry_accepted {
            return false;
        }
        if !state.step(self.ended.work().saturating_add(other.ended.work())) || self.ended != other.ended {
            return false;
        }
        if !state.step(self.closed_inputs.work().saturating_add(other.closed_inputs.work()))
            || self.closed_inputs != other.closed_inputs {
            return false;
        }
        if !self.failures.equal(&other.failures, |work| state.step(work), AllocationSize::work) {
            return false;
        }
        if !state.step(1) || self.active_failure != other.active_failure {
            return false;
        }
        if !state.step(1) || self.active_failure_origin != other.active_failure_origin {
            return false;
        }
        if !state.step(1) || self.latest_failure != other.latest_failure {
            return false;
        }
        if !state.step(1) || self.latest_failure_origin != other.latest_failure_origin {
            return false;
        }
        if !state.step(self.fields.work().saturating_add(other.fields.work())) || self.fields != other.fields {
            return false;
        }
        if !self.aliases.equal(&other.aliases, |work| state.step(work), AllocationSize::work) {
            return false;
        }
        if !self.contained.equal(&other.contained, |work| state.step(work), AllocationSize::work) {
            return false;
        }
        if !state.step(self.views.work().saturating_add(other.views.work())) || self.views != other.views {
            return false;
        }
        true
    }
    pub fn join_masks(&mut self, other: &Self) -> bool {
        // The join is still bytewise OR. Process eight independent masks per
        // word so its work charge follows the loads, stores and comparisons.
        let mut changed = false;
        let mut masks = self.availability.chunks_exact_mut(8);
        let mut incoming = other.availability.chunks_exact(8);
        for (mask, incoming) in masks.by_ref().zip(incoming.by_ref()) {
            let old = u64::from_ne_bytes((&*mask).try_into().unwrap());
            let next = old | u64::from_ne_bytes(incoming.try_into().unwrap());
            changed |= old != next;
            mask.copy_from_slice(&next.to_ne_bytes());
        }
        for (mask, incoming) in masks.into_remainder().iter_mut().zip(incoming.remainder()) {
            let next = *mask | incoming;
            changed |= *mask != next;
            *mask = next;
        }
        changed
    }
    pub fn normalize_token(&self, mut token: Token, state: &mut super::State<'_>) -> Token {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("domain::normalize_token");
        token.pieces = token.pieces.into_iter().map(|mut piece| {
            piece.predicates = piece.predicates.into_iter().map(|(mut filter, selected)| {
                let mut labels = SharedSet::new();
                for value in &filter.labels {
                    let aliases = &self.aliases[value.as_u32() as usize];
                    if !state.step(aliases.len().saturating_add(1)) {
                        break;
                    }
                    labels.extend(aliases);
                }
                filter.labels = labels;
                if !state.step(filter.keys.len().saturating_add(1)) {
                    return (filter, selected);
                }
                let current = filter.keys.iter().all(|key| !matches!(key, Origin::RetiredInstruction(..)
                    | Origin::RetiredInvoke(..) | Origin::RetiredProjection(..)
                    | Origin::RetiredViewInstruction(..) | Origin::RetiredViewInvoke(..)));
                if current {
                    for (index, refs) in self.refs.indexed() {
                        let contained = &self.contained[index];
                        if !state.step(1 + refs.len() + contained.len()) {
                            break;
                        }
                        if self.witnesses[index] && !refs.is_empty() && refs.is_subset(&filter.keys) {
                            filter.labels.extend(&self.contained[index]);
                        }
                    }
                }
                (filter, selected)
            }).collect();
            piece
        }).collect();
        token
    }
    pub fn remap_aliases(
        &mut self, classes: &BTreeMap<SharedSet<ValueId>, SharedSet<ValueId>>, state: &mut super::State<'_>,
    ) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("domain::remap_aliases");
        if !state.step(self.aliases.clone_work()) {
            return;
        }
        let old = self.aliases.clone();
        let rewrite = |piece: &Piece| {
            let mut piece = piece.clone();
            piece.predicates = piece.predicates.into_iter().map(|(mut filter, selected)| {
                let mut labels = SharedSet::new();
                for label in &filter.labels {
                    if let Some(members) = classes.get(&old[label.as_u32() as usize]) {
                        labels.extend(members);
                        filter.stale |= members.is_empty();
                    }
                }
                filter.labels = labels;
                (filter, selected)
            }).collect();
            piece
        };
        #[cfg(any(test, feature = "verifier-differential"))]
        let token_work = super::super::work_profile::Operation::new("remap::tokens");
        for tokens in [&mut self.tokens, &mut self.journals] {
            let work = tokens.rewrite_changed(|token| {
                if !state.step(1 + token.pieces.len())
                    || !token.pieces.iter().any(|piece| !piece.predicates.is_empty()) {
                    return None;
                }
                if !state.step(2 * token.work()) {
                    return None;
                }
                let mut next = token.clone();
                next.pieces = token.pieces.iter().map(rewrite).collect();
                (next != *token).then_some(next)
            });
            state.step(work);
        }
        #[cfg(any(test, feature = "verifier-differential"))]
        drop(token_work);
        if self.closed_inputs.iter().any(|piece| !piece.predicates.is_empty()) {
            if !state.step(self.closed_inputs.work()) {
                return;
            }
            self.closed_inputs = self.closed_inputs.iter().map(rewrite).collect();
        }
        #[cfg(any(test, feature = "verifier-differential"))]
        let contained_work = super::super::work_profile::Operation::new("remap::contained");
        let mut contained_cache = BTreeMap::new();
        let work = self.contained.rewrite_changed(|contained| {
            if !state.step(2 + contained_cache.len().max(1).ilog2() as usize) {
                return None;
            }
            let next = contained_cache.entry(contained.root_id()).or_insert_with(|| {
                state.step(1 + contained.len());
                if let Some(label) = contained.iter().next() {
                    let class = &old[label.as_u32() as usize];
                    state.step(contained.comparison_work(class));
                    let uniform = contained == class && contained.iter().all(|member| {
                        let aliases = &old[member.as_u32() as usize];
                        state.step(1 + aliases.comparison_work(class));
                        aliases == class
                    });
                    if uniform {
                        let desired = classes.get(class).cloned().unwrap_or_default();
                        return (desired.root_id() != contained.root_id()).then_some(desired);
                    }
                }
                let mut expanded = BTreeSet::new();
                let mut desired = SharedSet::new();
                for label in contained {
                    let class = &old[label.as_u32() as usize];
                    state.step(2 + expanded.len().max(1).ilog2() as usize);
                    if !expanded.insert(class.root_id()) {
                        continue;
                    }
                    if let Some(members) = classes.get(class) {
                        if desired.is_empty() {
                            // A one-class union is already represented by this root.
                            desired = members.clone();
                            state.step(1);
                        } else {
                            let (union, work) = desired.difference_union(&SharedSet::new(), members);
                            state.step(work);
                            desired = union;
                        }
                    }
                }
                state.step(1);
                (desired.root_id() != contained.root_id()).then_some(desired)
            });
            next.clone()
        });
        state.step(work);
        #[cfg(any(test, feature = "verifier-differential"))]
        drop(contained_work);
        #[cfg(any(test, feature = "verifier-differential"))]
        let _directory_work = super::super::work_profile::Operation::new("remap::directory");
        let mut aliases = Vec::new();
        for members in classes.values() {
            for member in members {
                if !state.step(1) {
                    return;
                }
                aliases.push((member.as_u32() as usize, members.clone()));
            }
        }
        self.aliases.assign_entries(aliases, |work| state.step(work));
    }
    pub fn add_contained(
        &mut self, roots: &SharedSet<ValueId>, child: &SharedSet<ValueId>, state: &mut super::State<'_>,
    ) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("domain::add_contained");
        if roots.is_empty() || child.is_empty() {
            state.step(1);
            return;
        }
        if !state.step(self.relation_work()) {
            return;
        }
        self.contained.rewrite_stored(|contained| {
            let mut next = contained.clone();
            if !contained.is_disjoint(roots) {
                next.extend(child);
            }
            next
        });
        let add = |piece: &Piece| {
            let mut piece = piece.clone();
            piece.predicates = piece.predicates.into_iter().map(|(mut filter, selected)| {
                if !filter.labels.is_disjoint(roots) {
                    filter.labels.extend(child);
                }
                (filter, selected)
            }).collect();
            piece
        };
        let add_token = |token: &Token| {
            let mut next = token.clone();
            next.pieces = token.pieces.iter().map(add).collect();
            next
        };
        self.tokens.rewrite_stored(add_token);
        self.journals.rewrite_stored(add_token);
        self.closed_inputs = self.closed_inputs.iter().map(add).collect();
    }
    pub fn replace_aliases(
        &mut self, old: &SharedSet<ValueId>, new: &SharedSet<ValueId>, state: &mut super::State<'_>,
    ) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("domain::replace_aliases");
        if !state.step(1) {
            return;
        }
        if old.is_empty() {
            for alias in new {
                if !state.step(2 + self.availability.len().max(1).ilog2() as usize) {
                    return;
                }
                self.aliases.set(alias.as_u32() as usize, new.clone());
            }
            return;
        }
        let rewrite = |piece: &Piece| {
            let mut piece = piece.clone();
            piece.predicates = piece.predicates.into_iter().map(|(mut filter, selected)| {
                if !filter.labels.is_disjoint(old) {
                    filter.labels = filter.labels.difference(old).copied().chain(new.iter().copied()).collect();
                    filter.stale |= new.is_empty();
                }
                (filter, selected)
            }).collect();
            piece
        };
        let affected = |piece: &Piece, state: &mut super::State<'_>| {
            piece.predicates.keys().any(|filter| {
                let (disjoint, work) = filter.labels.disjoint_work(old);
                state.step(work);
                !disjoint
            })
        };
        #[cfg(any(test, feature = "verifier-differential"))]
        let token_work = super::super::work_profile::Operation::new("aliases::tokens");
        for tokens in [&mut self.tokens, &mut self.journals] {
            let work = tokens.rewrite_changed(|token| {
                if !state.step(1) || !token.pieces.iter().any(|piece| {
                    state.step(1);
                    affected(piece, state)
                }) {
                    return None;
                }
                if !state.step(2 * token.work()) {
                    return None;
                }
                let mut next = token.clone();
                next.pieces = token.pieces.iter().map(rewrite).collect();
                (next != *token).then_some(next)
            });
            state.step(work);
        }
        #[cfg(any(test, feature = "verifier-differential"))]
        drop(token_work);
        if self.closed_inputs.iter().any(|piece| {
            state.step(1);
            affected(piece, state)
        }) && state.step(self.closed_inputs.work()) {
            self.closed_inputs = self.closed_inputs.iter().map(rewrite).collect();
        }
        #[cfg(any(test, feature = "verifier-differential"))]
        let contained_work = super::super::work_profile::Operation::new("aliases::contained");
        let (removed_labels, removed_work) = old.difference_union(new, &SharedSet::new());
        let (added_labels, added_work) = new.difference_union(old, &SharedSet::new());
        if !state.step(removed_work + added_work) {
            return;
        }
        let mut contained_cache = BTreeMap::new();
        let work = self.contained.rewrite_changed(|contained| {
            if !state.step(2 + contained_cache.len().max(1).ilog2() as usize) {
                return None;
            }
            contained_cache.entry(contained.root_id()).or_insert_with(|| {
                let (includes_old, work) = old.subset_work(contained);
                if !state.step(work) {
                    return None;
                }
                if includes_old {
                    let (next, work) = contained.difference_union(&removed_labels, &added_labels);
                    if !state.step(work) {
                        return None;
                    }
                    return (next.root_id() != contained.root_id()).then_some(next);
                }
                let (disjoint, work) = contained.disjoint_work(old);
                if !state.step(work) || disjoint {
                    return None;
                }
                let (next, work) = contained.difference_union(old, new);
                if !state.step(work) {
                    return None;
                }
                (next.root_id() != contained.root_id()).then_some(next)
            }).clone()
        });
        state.step(work);
        #[cfg(any(test, feature = "verifier-differential"))]
        drop(contained_work);
        #[cfg(any(test, feature = "verifier-differential"))]
        let _directory_work = super::super::work_profile::Operation::new("aliases::directory");
        let mut aliases = Vec::new();
        for alias in old.union(new) {
            if !state.step(1) {
                return;
            }
            aliases.push((alias.as_u32() as usize, new.clone()));
        }
        self.aliases.update_entries(aliases, |work| state.step(work));
    }
    pub fn projection_keys(&self) -> BTreeSet<Origin> {
        let mut keys = BTreeSet::new();
        for refs in self.refs.stored().chain(self.full_deps.stored()).chain(self.views.values()) {
            keys.extend(refs.iter().filter(|key| matches!(key, Origin::Projection(..))).copied());
        }
        keys.extend(self.views.keys().filter(|key| matches!(key, Origin::Projection(..))).copied());
        for token in self.tokens.stored().chain(self.journals.stored()) {
            for piece in &token.pieces {
                for filter in piece.predicates.keys() {
                    keys.extend(filter.keys.iter().chain(&filter.possible)
                        .filter(|key| matches!(key, Origin::Projection(..))).copied());
                }
            }
        }
        keys
    }
    fn origin_bounds(&self, state: &mut super::State<'_>) -> OriginBounds {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("domain::origin_bounds");
        let mut ranges = OriginBounds::EMPTY;
        ranges.include(self.tokens.origin_bounds(&mut |work| state.step(work)));
        ranges.include(self.journals.origin_bounds(&mut |work| state.step(work)));
        ranges.include(self.refs.origin_bounds(&mut |work| state.step(work)));
        ranges.include(self.full_deps.origin_bounds(&mut |work| state.step(work)));
        ranges.include(self.identities.origin_bounds(&mut |work| state.step(work)));
        state.step(self.dead.work());
        ranges.include(self.dead.origin_bounds());
        state.step(self.taken_caches.work());
        ranges.include(self.taken_caches.origin_bounds());
        state.step(self.cache_keys.work());
        ranges.include(self.cache_keys.origin_bounds());
        state.step(self.provenance.work());
        ranges.include(self.provenance.origin_bounds());
        state.step(self.closed_inputs.work());
        ranges.include(self.closed_inputs.origin_bounds());
        state.step(self.fields.work());
        ranges.include(self.fields.origin_bounds());
        state.step(self.views.work());
        ranges.include(self.views.origin_bounds());
        state.step(self.frames.work());
        ranges.include(self.frames.origin_bounds());
        state.step(self.cells.work());
        ranges.include(self.cells.origin_bounds());
        state.step(self.active_failure_origin.work());
        ranges.include(self.active_failure_origin.origin_bounds());
        state.step(self.latest_failure_origin.work());
        ranges.include(self.latest_failure_origin.origin_bounds());
        ranges
    }
    pub fn retire_origin(&mut self, origin: Origin, state: &mut super::State<'_>) {
        #[cfg(any(test, feature = "verifier-differential"))]
        let _work = super::super::work_profile::Operation::new("domain::retire_origin");
        let retired = match origin {
            Origin::Instruction(inst, slot) => Origin::RetiredInstruction(inst, slot),
            Origin::Invoke(block, slot) => Origin::RetiredInvoke(block, slot),
            Origin::Projection(value, kind) => Origin::RetiredProjection(value, kind),
            Origin::ViewInstruction(inst, slot) => Origin::RetiredViewInstruction(inst, slot),
            Origin::ViewInvoke(block, slot) => Origin::RetiredViewInvoke(block, slot),
            _ => return,
        };
        let ranges = self.origin_bounds(state);
        if state.incomplete {
            return;
        }
        if ranges.0[4] != 0 {
            if !state.step(self.origin_work()) {
                return;
            }
            for key in self.projection_keys() {
                if let Origin::Projection(value, _) = key {
                    self.views.entry(key).or_insert_with(|| self.refs[value.as_u32() as usize].clone());
                }
            }
        }
        if let Some((kind, id)) = origin.indexed_id()
            && (ranges.0[kind] == 0 || ranges.0[kind] < id.saturating_add(1)) {
            return;
        }
        if !state.step(self.origin_work()) {
            return;
        }
        let identities = self.identities.clone();
        let rename = |key: Origin| {
            if key == origin {
                return retired;
            }
            if let Origin::Projection(value, kind) = key
                && identities.get(value.as_u32() as usize) == Some(&Some(origin)) {
                return Origin::RetiredProjection(value, kind);
            }
            key
        };
        for refs in self.refs.stored_mut().chain(self.full_deps.stored_mut()) {
            *refs = refs.iter().copied().map(rename).collect();
        }
        for identity in self.identities.stored_mut() {
            *identity = identity.map(rename);
        }
        for token in self.tokens.stored_mut().chain(self.journals.stored_mut())
            .chain(self.frames.values_mut().flat_map(|layout| layout.values_mut().map(|slot| &mut slot.token)))
            .chain(self.cells.values_mut().map(|slot| &mut slot.token)) {
            token.pieces = token.pieces.iter().cloned().map(|mut piece| {
                let changed = piece.origin != rename(piece.origin);
                piece.origin = rename(piece.origin);
                piece.holder = piece.holder.map(rename);
                piece.family |= changed;
                piece.predicates = piece.predicates.into_iter().map(|(mut filter, selected)| {
                    filter.keys = filter.keys.into_iter().map(rename).collect();
                    filter.possible = filter.possible.into_iter().map(rename).collect();
                    (filter, selected)
                }).collect();
                piece
            }).collect();
        }
        for token in self.tokens.stored_mut().chain(self.journals.stored_mut())
            .chain(self.frames.values_mut().flat_map(|layout| layout.values_mut().map(|slot| &mut slot.token)))
            .chain(self.cells.values_mut().map(|slot| &mut slot.token)) {
            token.unique = token.unique.map(rename);
            token.failure_origin = token.failure_origin.map(rename);
            token.finish_origin = token.finish_origin.map(rename);
            token.service = token.service.map(rename);
            token.cache = token.cache.map(rename);
            token.frame = token.frame.map(rename);
            token.cell = token.cell.map(rename);
            token.pin_deps = token.pin_deps.iter().copied().map(rename).collect();
            if let Some((keys, stale)) = &mut token.snapshot {
                *stale |= keys.contains(&origin);
                *keys = keys.iter().copied().map(rename).collect();
            }
            if let Some(fact) = &mut token.cleanup {
                let rewrite = |piece: &Piece| {
                    let mut piece = piece.clone();
                    piece.origin = rename(piece.origin);
                    piece.holder = piece.holder.map(rename);
                    piece
                };
                fact.reserve = fact.reserve.iter().map(rewrite).collect();
                fact.inventory = fact.inventory.iter().map(rewrite).collect();
                fact.selected = fact.selected.map(rename);
                fact.failure_origin = fact.failure_origin.map(rename);
                fact.awaiting_origin = fact.awaiting_origin.map(rename);
                fact.executed = fact.executed.iter().copied().map(rename).collect();
                fact.services = fact.services.iter().copied().map(rename).collect();
            }
        }
        self.active_failure_origin = self.active_failure_origin.map(rename);
        self.latest_failure_origin = self.latest_failure_origin.map(rename);
        for slot in self.frames.values_mut().flat_map(|layout| layout.values_mut()) {
            slot.refs = slot.refs.iter().copied().map(rename).collect();
            slot.requirements = slot.requirements.iter().copied().map(rename).collect();
            slot.identity = slot.identity.map(rename);
        }
        if let Some(layout) = self.frames.remove(&origin) {
            if self.frames.get(&retired).is_some_and(|old| !old.is_empty()) && !layout.is_empty() {
                self.frame_conflict = true;
            }
            self.frames.entry(retired).or_default().extend(layout);
        }
        for slot in self.cells.values_mut() {
            slot.refs = slot.refs.iter().copied().map(rename).collect();
            slot.requirements = slot.requirements.iter().copied().map(rename).collect();
            slot.identity = slot.identity.map(rename);
        }
        if let Some(cell) = self.cells.remove(&origin) {
            self.cells.entry(retired).and_modify(|old| {
                old.refs.extend(&cell.refs);
                old.requirements.extend(&cell.requirements);
                old.lifetime |= cell.lifetime;
                old.failures |= cell.failures;
                if old.identity != cell.identity { old.identity = None; }
                if old.token.finish_origin != cell.token.finish_origin { old.token.finish_origin = None; }
                if old.token.failure_origin != cell.token.failure_origin { old.token.failure_origin = None; }
            }).or_insert(cell);
        }
        self.cache_keys = self.cache_keys.iter().copied().map(rename).collect();
        self.taken_caches = self.taken_caches.iter().copied().map(rename).collect();
        self.closed_inputs = self.closed_inputs.iter().cloned().map(|mut piece| {
            piece.origin = rename(piece.origin);
            piece.predicates = piece.predicates.into_iter().map(|(mut filter, selected)| {
                filter.keys = filter.keys.into_iter().map(rename).collect();
                filter.possible = filter.possible.into_iter().map(rename).collect();
                (filter, selected)
            }).collect();
            piece
        }).collect();
        let mut views: BTreeMap<Origin, BTreeSet<Origin>> = BTreeMap::new();
        for (key, refs) in &self.views {
            views.entry(rename(*key)).or_default().extend(refs.iter().copied().map(rename));
        }
        self.views = views;
        let mut fields: Fields = BTreeMap::new();
        for ((key, declaration, index), (refs, identity)) in &self.fields {
            let entry = fields.entry((rename(*key), *declaration, *index))
                .or_insert_with(|| (BTreeSet::new(), identity.map(rename)));
            entry.0.extend(refs.iter().copied().map(rename));
            if entry.1 != identity.map(rename) {
                entry.1 = None;
            }
        }
        self.fields = fields;
        self.dead = self.dead.iter().copied().map(rename).collect();
        if self.dead.remove(&origin) {
            self.dead.insert(retired);
        }
        if let Some(provenance) = self.provenance.remove(&origin) {
            self.provenance.entry(retired).and_modify(|old| *old |= provenance).or_insert(provenance);
        }
    }
    pub fn retire(&mut self, value: ValueId) {
        let index = value.as_u32() as usize;
        self.availability[index] = CONSUMED;
        self.tokens[index] = Token { service: self.tokens[index].service, ..Token::default() };
        self.journals[index] = Token::default();
    }
}

pub(super) fn failure_bit(kind: FailureKind) -> u8 {
    match kind {
        FailureKind::Thrown => 1,
        FailureKind::Program => 2,
        FailureKind::Panic => 4,
        FailureKind::Native => 8,
        FailureKind::Cancelled => 16,
    }
}

pub(super) fn creation(origin: Origin, site: CreationSiteId) -> Origin {
    match origin {
        Origin::Instruction(inst, _) => Origin::Instruction(inst, site.as_u32()),
        Origin::Invoke(block, _) => Origin::Invoke(block, site.as_u32()),
        _ => origin,
    }
}

pub(super) fn view_origin(origin: Origin, slot: usize) -> Origin {
    match origin {
        Origin::Instruction(inst, _) => Origin::ViewInstruction(inst, slot as u16),
        Origin::Invoke(block, _) => Origin::ViewInvoke(block, slot as u16),
        _ => origin,
    }
}

impl Origin {
    fn indexed_id(self) -> Option<(usize, u32)> {
        match self {
            Self::Instruction(id, _) => Some((0, id.as_u32())),
            Self::Invoke(id, _) => Some((1, id.as_u32())),
            Self::ViewInstruction(id, _) => Some((2, id.as_u32())),
            Self::ViewInvoke(id, _) => Some((3, id.as_u32())),
            Self::Projection(id, _) => Some((4, id.as_u32())),
            _ => None,
        }
    }
}
impl AllocationSize for Origin {
    fn origin_bounds(&self) -> OriginBounds {
        let mut ranges = OriginBounds::EMPTY;
        if let Some((kind, id)) = self.indexed_id() {
            ranges.0[kind] = id.saturating_add(1);
        }
        ranges
    }
}
impl AllocationSize for ValueId {}
impl AllocationSize for ScopeId {}
impl AllocationSize for BlockId {}
impl AllocationSize for BindingId {}
impl AllocationSize for DeclId {}
impl AllocationSize for u32 {}
impl<A: AllocationSize, B: AllocationSize> AllocationSize for (A, B) {
    fn origin_bounds(&self) -> OriginBounds {
        let mut ranges = OriginBounds::EMPTY;
        ranges.include(self.0.origin_bounds());
        ranges.include(self.1.origin_bounds());
        ranges
    }
    fn heap_bytes(&self) -> usize { self.0.heap_bytes().saturating_add(self.1.heap_bytes()) }
    fn unshared_bytes(&self) -> usize { self.0.unshared_bytes().saturating_add(self.1.unshared_bytes()) }
    fn charge_shared(&self) -> usize { self.0.charge_shared().saturating_add(self.1.charge_shared()) }
    fn pending_shared(&self) -> usize { self.0.pending_shared().saturating_add(self.1.pending_shared()) }
    fn temporary_bytes(&self) -> usize { self.0.temporary_bytes().saturating_add(self.1.temporary_bytes()) }
    fn work(&self) -> usize { self.0.work().saturating_add(self.1.work()) }
}
impl<A: AllocationSize, B: AllocationSize, C: AllocationSize> AllocationSize for (A, B, C) {
    fn origin_bounds(&self) -> OriginBounds {
        let mut ranges = OriginBounds::EMPTY;
        ranges.include(self.0.origin_bounds());
        ranges.include(self.1.origin_bounds());
        ranges.include(self.2.origin_bounds());
        ranges
    }
    fn work(&self) -> usize { self.0.work().saturating_add(self.1.work()).saturating_add(self.2.work()) }
    fn heap_bytes(&self) -> usize {
        self.0.heap_bytes().saturating_add(self.1.heap_bytes()).saturating_add(self.2.heap_bytes())
    }
    fn unshared_bytes(&self) -> usize {
        self.0.unshared_bytes().saturating_add(self.1.unshared_bytes()).saturating_add(self.2.unshared_bytes())
    }
    fn charge_shared(&self) -> usize {
        self.0.charge_shared().saturating_add(self.1.charge_shared()).saturating_add(self.2.charge_shared())
    }
    fn pending_shared(&self) -> usize {
        self.0.pending_shared().saturating_add(self.1.pending_shared()).saturating_add(self.2.pending_shared())
    }
    fn temporary_bytes(&self) -> usize {
        self.0.temporary_bytes().saturating_add(self.1.temporary_bytes()).saturating_add(self.2.temporary_bytes())
    }
}
impl AllocationSize for Filter {
    fn origin_bounds(&self) -> OriginBounds {
        let mut ranges = OriginBounds::EMPTY;
        ranges.include(self.keys.origin_bounds());
        ranges.include(self.possible.origin_bounds());
        ranges
    }
    fn work(&self) -> usize {
        self.keys.work() + self.possible.work() + self.labels.work()
    }
    fn heap_bytes(&self) -> usize {
        self.keys.heap_bytes().saturating_add(self.possible.heap_bytes()).saturating_add(self.labels.heap_bytes())
    }
    fn unshared_bytes(&self) -> usize {
        self.keys.unshared_bytes().saturating_add(self.possible.unshared_bytes())
            .saturating_add(self.labels.unshared_bytes())
    }
    fn charge_shared(&self) -> usize {
        self.keys.charge_shared().saturating_add(self.possible.charge_shared())
            .saturating_add(self.labels.charge_shared())
    }
    fn pending_shared(&self) -> usize {
        self.keys.pending_shared().saturating_add(self.possible.pending_shared())
            .saturating_add(self.labels.pending_shared())
    }
    fn temporary_bytes(&self) -> usize {
        self.keys.temporary_bytes().saturating_add(self.possible.temporary_bytes())
            .saturating_add(self.labels.temporary_bytes())
    }
}
impl AllocationSize for Piece {
    fn origin_bounds(&self) -> OriginBounds {
        let mut ranges = OriginBounds::EMPTY;
        ranges.include(self.origin.origin_bounds());
        ranges.include(self.holder.origin_bounds());
        ranges.include(self.predicates.origin_bounds());
        ranges
    }
    fn work(&self) -> usize {
        1 + self.predicates.work()
    }
    fn heap_bytes(&self) -> usize { self.predicates.heap_bytes() }
    fn unshared_bytes(&self) -> usize { self.predicates.unshared_bytes() }
    fn charge_shared(&self) -> usize { self.predicates.charge_shared() }
    fn pending_shared(&self) -> usize { self.predicates.pending_shared() }
    fn temporary_bytes(&self) -> usize { self.predicates.temporary_bytes() }
}
impl AllocationSize for CleanupFact {
    fn origin_bounds(&self) -> OriginBounds {
        let mut ranges = OriginBounds::EMPTY;
        ranges.include(self.reserve.origin_bounds());
        ranges.include(self.inventory.origin_bounds());
        ranges.include(self.failure_origin.origin_bounds());
        ranges.include(self.awaiting_origin.origin_bounds());
        ranges.include(self.selected.origin_bounds());
        ranges.include(self.executed.origin_bounds());
        ranges.include(self.services.origin_bounds());
        ranges
    }
    fn work(&self) -> usize {
        1 + self.reserve.work() + self.inventory.work() + self.executed.work() + self.services.work()
    }
    fn heap_bytes(&self) -> usize {
        self.reserve.heap_bytes().saturating_add(self.inventory.heap_bytes())
            .saturating_add(self.executed.heap_bytes()).saturating_add(self.services.heap_bytes())
    }
    fn unshared_bytes(&self) -> usize {
        self.reserve.unshared_bytes().saturating_add(self.inventory.unshared_bytes())
            .saturating_add(self.executed.unshared_bytes()).saturating_add(self.services.unshared_bytes())
    }
    fn charge_shared(&self) -> usize {
        self.reserve.charge_shared().saturating_add(self.inventory.charge_shared())
            .saturating_add(self.executed.charge_shared()).saturating_add(self.services.charge_shared())
    }
    fn pending_shared(&self) -> usize {
        self.reserve.pending_shared().saturating_add(self.inventory.pending_shared())
            .saturating_add(self.executed.pending_shared()).saturating_add(self.services.pending_shared())
    }
    fn temporary_bytes(&self) -> usize {
        self.reserve.temporary_bytes().saturating_add(self.inventory.temporary_bytes())
            .saturating_add(self.executed.temporary_bytes()).saturating_add(self.services.temporary_bytes())
    }
}
impl AllocationSize for Token {
    fn comparison_work(&self, other: &Self) -> usize {
        if self.pieces.len() != other.pieces.len() {
            1
        } else if self.pieces.is_empty() && self.scope != other.scope {
            2
        } else {
            self.work().saturating_add(other.work())
        }
    }

    fn origin_bounds(&self) -> OriginBounds {
        let mut ranges = OriginBounds::EMPTY;
        ranges.include(self.pieces.origin_bounds());
        ranges.include(self.service.origin_bounds());
        ranges.include(self.cell.origin_bounds());
        ranges.include(self.unique.origin_bounds());
        ranges.include(self.frame.origin_bounds());
        ranges.include(self.cache.origin_bounds());
        ranges.include(self.pin_deps.origin_bounds());
        ranges.include(self.finish_origin.origin_bounds());
        ranges.include(self.failure_origin.origin_bounds());
        ranges.include(self.cleanup.origin_bounds());
        ranges.include(self.snapshot.origin_bounds());
        ranges
    }
    fn work(&self) -> usize {
        1 + self.pieces.work() + self.reserve.work() + self.destinations.work() + self.sources.work()
            + self.unique_region.work() + self.pin_deps.work() + self.cleanup.work() + self.snapshot.work()
    }
    fn heap_bytes(&self) -> usize {
        self.pieces.heap_bytes().saturating_add(self.reserve.heap_bytes())
            .saturating_add(self.destinations.heap_bytes()).saturating_add(self.sources.heap_bytes())
            .saturating_add(self.unique_region.heap_bytes()).saturating_add(self.pin_deps.heap_bytes())
            .saturating_add(self.cleanup.heap_bytes()).saturating_add(self.snapshot.heap_bytes())
    }
    fn unshared_bytes(&self) -> usize {
        self.pieces.unshared_bytes().saturating_add(self.reserve.unshared_bytes())
            .saturating_add(self.destinations.unshared_bytes()).saturating_add(self.sources.unshared_bytes())
            .saturating_add(self.unique_region.unshared_bytes()).saturating_add(self.pin_deps.unshared_bytes())
            .saturating_add(self.cleanup.unshared_bytes()).saturating_add(self.snapshot.unshared_bytes())
    }
    fn charge_shared(&self) -> usize {
        self.pieces.charge_shared().saturating_add(self.reserve.charge_shared())
            .saturating_add(self.destinations.charge_shared()).saturating_add(self.sources.charge_shared())
            .saturating_add(self.unique_region.charge_shared()).saturating_add(self.pin_deps.charge_shared())
            .saturating_add(self.cleanup.charge_shared()).saturating_add(self.snapshot.charge_shared())
    }
    fn pending_shared(&self) -> usize {
        self.pieces.pending_shared().saturating_add(self.reserve.pending_shared())
            .saturating_add(self.destinations.pending_shared()).saturating_add(self.sources.pending_shared())
            .saturating_add(self.unique_region.pending_shared()).saturating_add(self.pin_deps.pending_shared())
            .saturating_add(self.cleanup.pending_shared()).saturating_add(self.snapshot.pending_shared())
    }
    fn temporary_bytes(&self) -> usize {
        self.pieces.temporary_bytes().saturating_add(self.reserve.temporary_bytes())
            .saturating_add(self.destinations.temporary_bytes()).saturating_add(self.sources.temporary_bytes())
            .saturating_add(self.unique_region.temporary_bytes()).saturating_add(self.pin_deps.temporary_bytes())
            .saturating_add(self.cleanup.temporary_bytes()).saturating_add(self.snapshot.temporary_bytes())
    }
}
impl Flow {
    /// Copy/scan bound in stored entries and nested relations. Mask copies and
    /// joins process words, rather than dense per-value relational records.
    pub fn clone_work(&self) -> usize {
        self.mask_work() + self.tokens.clone_work() + self.journals.clone_work() + self.refs.clone_work()
            + self.full_deps.clone_work() + self.lifetimes.clone_work() + self.identities.clone_work()
            + self.must_use.clone_work() + self.failures.clone_work() + self.must_use_aliases.clone_work()
            + self.aliases.clone_work()
            + self.contained.clone_work() + self.dead.work() + self.taken_caches.work() + self.cache_keys.work()
            + self.provenance.work() + self.scopes.work() + self.bindings.work() + self.ended.work()
            + self.closed_inputs.work() + self.fields.work() + self.views.work() + self.frames.work()
            + self.cells.work()
    }
    pub fn work(&self) -> usize {
        self.mask_work() + self.relation_work()
    }
    pub fn mask_work(&self) -> usize {
        self.availability.len() / 8 + self.availability.len() % 8
    }
    pub fn origin_work(&self) -> usize {
        // Retirement never scans SSA alias/contained label directories.
        1 + self.tokens.work() + self.journals.work() + self.refs.work() + self.full_deps.work()
            + self.identities.work() + self.dead.work() + self.taken_caches.work() + self.cache_keys.work()
            + self.provenance.work() + self.closed_inputs.work() + self.fields.work() + self.views.work()
            + self.frames.work() + self.cells.work()
    }
    pub fn relation_work(&self) -> usize {
        1 + self.tokens.work() + self.journals.work() + self.refs.work() + self.full_deps.work()
            + self.lifetimes.work() + self.identities.work() + self.must_use.work() + self.failures.work()
            + self.must_use_aliases.work() + self.aliases.work() + self.contained.work() + self.dead.work()
            + self.taken_caches.work()
            + self.cache_keys.work() + self.provenance.work() + self.scopes.work() + self.bindings.work()
            + self.ended.work() + self.closed_inputs.work() + self.fields.work() + self.views.work()
            + self.frames.work() + self.cells.work()
    }
    #[cfg(any(test, feature = "verifier-differential"))]
    fn global_bytes(&self) -> usize {
        self.availability.capacity()
            + self.dead.heap_bytes() + self.taken_caches.heap_bytes() + self.cache_keys.heap_bytes()
            + self.provenance.heap_bytes() + self.scopes.heap_bytes() + self.bindings.heap_bytes()
            + self.ended.heap_bytes() + self.closed_inputs.heap_bytes() + self.fields.heap_bytes()
            + self.views.heap_bytes() + self.frames.heap_bytes() + self.cells.heap_bytes()
    }
    fn global_retained_bytes(&self) -> usize {
        self.availability.capacity()
            + self.dead.unshared_bytes() + self.taken_caches.unshared_bytes() + self.cache_keys.unshared_bytes()
            + self.provenance.unshared_bytes() + self.scopes.unshared_bytes() + self.bindings.unshared_bytes()
            + self.ended.unshared_bytes() + self.closed_inputs.unshared_bytes() + self.fields.unshared_bytes()
            + self.views.unshared_bytes() + self.frames.unshared_bytes() + self.cells.unshared_bytes()
    }
    fn charge_global_shared(&self) -> usize {
        self.dead.charge_shared() + self.taken_caches.charge_shared() + self.cache_keys.charge_shared()
            + self.provenance.charge_shared() + self.scopes.charge_shared() + self.bindings.charge_shared()
            + self.ended.charge_shared() + self.closed_inputs.charge_shared() + self.fields.charge_shared()
            + self.views.charge_shared() + self.frames.charge_shared() + self.cells.charge_shared()
    }
    fn pending_global_shared(&self) -> usize {
        self.dead.pending_shared() + self.taken_caches.pending_shared() + self.cache_keys.pending_shared()
            + self.provenance.pending_shared() + self.scopes.pending_shared() + self.bindings.pending_shared()
            + self.ended.pending_shared() + self.closed_inputs.pending_shared() + self.fields.pending_shared()
            + self.views.pending_shared() + self.frames.pending_shared() + self.cells.pending_shared()
    }
    fn global_temporary_bytes(&self) -> usize {
        self.dead.temporary_bytes() + self.taken_caches.temporary_bytes() + self.cache_keys.temporary_bytes()
            + self.provenance.temporary_bytes() + self.scopes.temporary_bytes() + self.bindings.temporary_bytes()
            + self.ended.temporary_bytes() + self.closed_inputs.temporary_bytes() + self.fields.temporary_bytes()
            + self.views.temporary_bytes() + self.frames.temporary_bytes() + self.cells.temporary_bytes()
    }
    pub fn scratch_bytes(&self) -> usize {
        // Eight transient flows plus iterator/retain directories are bounded by
        // charge_flow_work. Already retained nodes are immutable shared inputs.
        self.global_retained_bytes() + self.pending_global_shared() + self.global_temporary_bytes()
            + self.tokens.scratch_bytes() + self.journals.scratch_bytes()
            + self.refs.scratch_bytes() + self.full_deps.scratch_bytes() + self.lifetimes.scratch_bytes()
            + self.identities.scratch_bytes() + self.must_use.scratch_bytes() + self.failures.scratch_bytes()
            + self.must_use_aliases.scratch_bytes() + self.aliases.scratch_bytes() + self.contained.scratch_bytes()
    }
    #[cfg(any(test, feature = "verifier-differential"))]
    pub fn allocation_bytes(&self) -> usize {
        self.global_bytes() + self.tokens.allocation_bytes() + self.journals.allocation_bytes()
            + self.refs.allocation_bytes() + self.full_deps.allocation_bytes() + self.lifetimes.allocation_bytes()
            + self.identities.allocation_bytes() + self.must_use.allocation_bytes() + self.failures.allocation_bytes()
            + self.must_use_aliases.allocation_bytes() + self.aliases.allocation_bytes()
            + self.contained.allocation_bytes()
    }
    pub fn charge_retained(&self) -> usize {
        // Incoming vectors reserve exactly one slot for each retained state.
        // Inline Flow storage is charged here, not as eight heap scratch clones.
        let parts = [size_of::<Self>() + 16 + self.global_retained_bytes() + self.charge_global_shared(),
            self.tokens.charge_new(), self.journals.charge_new(), self.refs.charge_new(),
            self.full_deps.charge_new(), self.lifetimes.charge_new(), self.identities.charge_new(),
            self.must_use.charge_new(), self.failures.charge_new(), self.must_use_aliases.charge_new(),
            self.aliases.charge_new(),
            self.contained.charge_new()];
        #[cfg(any(test, feature = "verifier-differential"))]
        super::super::differential::record_allocations(&parts);
        parts.iter().sum()
    }
}
impl AllocationSize for super::Slot {
    fn origin_bounds(&self) -> OriginBounds {
        let mut ranges = OriginBounds::EMPTY;
        ranges.include(self.token.origin_bounds());
        ranges.include(self.journal.origin_bounds());
        ranges.include(self.refs.origin_bounds());
        ranges.include(self.identity.origin_bounds());
        ranges.include(self.requirements.origin_bounds());
        ranges
    }
    fn work(&self) -> usize {
        1 + self.token.work() + self.journal.work() + self.refs.work() + self.must_use_aliases.work()
            + self.aliases.work() + self.contained.work() + self.bases.work() + self.requirements.work()
    }
    fn heap_bytes(&self) -> usize {
        self.token.heap_bytes() + self.journal.heap_bytes() + self.refs.heap_bytes()
            + self.must_use_aliases.heap_bytes() + self.aliases.heap_bytes() + self.contained.heap_bytes()
            + self.bases.heap_bytes()
            + self.requirements.heap_bytes()
    }
    fn unshared_bytes(&self) -> usize {
        self.token.unshared_bytes() + self.journal.unshared_bytes() + self.refs.unshared_bytes()
            + self.must_use_aliases.unshared_bytes() + self.aliases.unshared_bytes() + self.contained.unshared_bytes()
            + self.bases.unshared_bytes()
            + self.requirements.unshared_bytes()
    }
    fn charge_shared(&self) -> usize {
        self.token.charge_shared() + self.journal.charge_shared() + self.refs.charge_shared()
            + self.must_use_aliases.charge_shared() + self.aliases.charge_shared() + self.contained.charge_shared()
            + self.bases.charge_shared()
            + self.requirements.charge_shared()
    }
    fn pending_shared(&self) -> usize {
        self.token.pending_shared() + self.journal.pending_shared() + self.refs.pending_shared()
            + self.must_use_aliases.pending_shared() + self.aliases.pending_shared() + self.contained.pending_shared()
            + self.bases.pending_shared()
            + self.requirements.pending_shared()
    }
    fn temporary_bytes(&self) -> usize {
        self.token.temporary_bytes() + self.journal.temporary_bytes() + self.refs.temporary_bytes()
            + self.must_use_aliases.temporary_bytes() + self.aliases.temporary_bytes()
            + self.contained.temporary_bytes() + self.bases.temporary_bytes()
            + self.requirements.temporary_bytes()
    }
}

#[cfg(test)]
mod memory_tests {
    use super::*;

    #[test]
    fn word_mask_join_matches_each_byte_including_unaligned_tails() {
        for len in 0..40 {
            for seed in 0..16 {
                let mut left = Flow::new(len, Rc::new(vec![true; len]));
                let mut right = left.clone();
                for index in 0..len {
                    left.availability[index] = ((index + seed) % 8) as u8;
                    right.availability[index] = ((index * 3 + seed) % 8) as u8;
                }
                let expected: Vec<_> = left.availability.iter().zip(&right.availability)
                    .map(|(left, right)| left | right).collect();
                let changed = expected != left.availability;
                assert_eq!(left.join_masks(&right), changed);
                assert_eq!(left.availability, expected);
                assert!(!left.join_masks(&right));
            }
        }
    }

    #[test]
    fn large_ssa_universe_keeps_implicit_defaults_and_shared_relations_compact() {
        let values = 10_000;
        let mut flow = Flow::new(values, Rc::new(vec![true; values]));
        flow.availability[values - 1] = AVAILABLE;
        flow.tokens[values - 1].private = true;
        let first = flow.charge_retained();
        let mut copied = flow.clone();
        copied.tokens[values - 1].settled = true;
        let copied_bytes = copied.charge_retained();
        // The precise solver's unchanged dense per-state charge, before scratch.
        let dense = values * 1536;
        assert!(first < dense / 100, "{first} vs {dense}");
        assert!(copied_bytes < dense / 100, "{copied_bytes} vs {dense}");
        assert!(!flow.tokens[values - 1].settled);
        assert!(copied.tokens[values - 1].settled);
    }
}

#[cfg(test)]
mod cell_tests {
    use super::*;

    fn labels(values: &[u32]) -> SharedSet<ValueId> {
        values.iter().map(|value| ValueId::from_u32(*value)).collect()
    }

    fn filter(keys: &[Origin], values: &[u32]) -> Filter {
        Filter {
            keys: keys.iter().copied().collect(), fresh: false, possible: BTreeSet::new(),
            labels: labels(values), stale: false, whole_future: false, state: 0,
        }
    }

    const VIEW: Origin = Origin::ViewInstruction(InstId::from_u32(1), 0);
    const RETIRED: Origin = Origin::RetiredViewInstruction(InstId::from_u32(1), 0);

    #[test]
    fn same_cell_ignores_labels_only_for_current_keys() {
        assert!(filter(&[VIEW], &[1]).same_cell(&filter(&[VIEW], &[2])));
        assert!(filter(&[VIEW], &[]).same_cell(&filter(&[VIEW], &[1, 2])));
        // A retired family denotes several incarnations: labels tell them apart.
        assert!(!filter(&[RETIRED], &[1]).same_cell(&filter(&[RETIRED], &[2])));
        assert!(!filter(&[VIEW, RETIRED], &[1]).same_cell(&filter(&[VIEW, RETIRED], &[1, 2])));
        assert!(filter(&[RETIRED], &[1]).same_cell(&filter(&[RETIRED], &[1])));
        // Every other component still distinguishes the cells.
        let base = filter(&[VIEW], &[1]);
        assert!(!base.same_cell(&filter(&[RETIRED], &[1])));
        let mut other = base.clone();
        other.possible.insert(Origin::External(ValueId::from_u32(3)));
        assert!(!base.same_cell(&other));
        for change in 0..4 {
            let mut other = base.clone();
            match change {
                0 => other.fresh = true,
                1 => other.stale = true,
                2 => other.whole_future = true,
                _ => other.state = 1,
            }
            assert!(!base.same_cell(&other), "{change}");
            assert!(!other.same_cell(&base), "{change}");
        }
    }

    #[test]
    fn predicate_reads_exact_then_same_cell_and_never_stale_records() {
        let mut piece = Piece::new(Origin::Instruction(InstId::from_u32(1), 0), AdmissionProvenance::UnboundResult);
        assert_eq!(piece.predicate(&filter(&[VIEW], &[1])), None);
        piece.predicates.insert(filter(&[VIEW], &[1]), true);
        assert_eq!(piece.predicate(&filter(&[VIEW], &[1])), Some(true));
        // A forwarded alias name selects the same current cell.
        assert_eq!(piece.predicate(&filter(&[VIEW], &[2])), Some(true));
        assert_eq!(piece.predicate(&filter(&[VIEW, RETIRED], &[1])), None);

        let mut retired = Piece::new(Origin::RetiredInstruction(InstId::from_u32(1), 0),
            AdmissionProvenance::UnboundResult);
        retired.family = true;
        retired.predicates.insert(filter(&[RETIRED], &[1]), true);
        assert_eq!(retired.predicate(&filter(&[RETIRED], &[1])), Some(true));
        // Another incarnation's labels never reuse the recorded side.
        assert_eq!(retired.predicate(&filter(&[RETIRED], &[2])), None);
        assert_eq!(retired.predicate(&filter(&[RETIRED], &[])), None);

        let mut stale = Piece::new(Origin::Instruction(InstId::from_u32(1), 0), AdmissionProvenance::UnboundResult);
        let mut recorded = filter(&[VIEW], &[1]);
        recorded.stale = true;
        stale.predicates.insert(recorded.clone(), false);
        // Only the exact (stale) filter reads its record; a fresh filter does not.
        assert_eq!(stale.predicate(&recorded), Some(false));
        assert_eq!(stale.predicate(&filter(&[VIEW], &[1])), None);
        assert_eq!(stale.predicate(&filter(&[VIEW], &[2])), None);
    }
}
