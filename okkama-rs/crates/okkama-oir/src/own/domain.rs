//! Finite site equations and the uncorrelated product lattice used by Own.
use crate::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Presence {
    #[default]
    Bottom,
    Absent,
    Present,
    Maybe,
}
impl Presence {
    pub fn join(self, other: Self) -> Self {
        match (self, other) {
            (Self::Bottom, value) | (value, Self::Bottom) => value,
            (left, right) if left == right => left,
            _ => Self::Maybe,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Multiplicity {
    #[default]
    Bottom,
    One,
    Many,
}
impl Multiplicity {
    pub fn join(self, other: Self) -> Self {
        self.max(other)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quantity {
    pub presence: Presence,
    pub multiplicity: Multiplicity,
}
impl Default for Quantity {
    fn default() -> Self {
        Self::ABSENT
    }
}
impl Quantity {
    pub const ABSENT: Self = Self { presence: Presence::Absent, multiplicity: Multiplicity::Bottom };
    pub const ONE: Self = Self { presence: Presence::Present, multiplicity: Multiplicity::One };
    pub fn new(presence: Presence, multiplicity: Multiplicity) -> Self {
        let multiplicity = if matches!(presence, Presence::Absent | Presence::Bottom) {
            Multiplicity::Bottom
        } else {
            multiplicity
        };
        Self { presence, multiplicity }
    }
    pub fn join(self, other: Self) -> Self {
        Self::new(self.presence.join(other.presence), self.multiplicity.join(other.multiplicity))
    }
    /// Disjoint runtime occurrences accumulate in a journal, unlike a CFG join.
    pub fn append(self, other: Self) -> Self {
        if self.multiplicity == Multiplicity::Bottom {
            return other;
        }
        if other.multiplicity == Multiplicity::Bottom {
            return self;
        }
        let presence = if self.presence == Presence::Present || other.presence == Presence::Present {
            Presence::Present
        } else {
            Presence::Maybe
        };
        Self::new(presence, Multiplicity::Many)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ShapeId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Site {
    Instruction(InstId),
    Terminator(BlockId),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Shape {
    Empty,
    Input { parameter: u32 },
    Fresh { origin: Site, current: ValueId },
    Union(BTreeSet<ShapeId>),
    Rest { site: Site },
    Selected { site: Site },
    Phi { block: BlockId, parameter: u32 },
    Journal { owner: ScopeId, slot: ValueId },
    Protocol { holder: ValueId, slot: u16 },
}

/// Nodes are interned during indexing only. Solving changes arcs, never nodes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Equations {
    pub nodes: Vec<Shape>,
    pub inputs: Vec<BTreeSet<ShapeId>>,
    interned: BTreeMap<Shape, ShapeId>,
    frozen: bool,
}
impl Equations {
    pub fn intern(&mut self, shape: Shape) -> Result<ShapeId, &'static str> {
        if let Some(id) = self.interned.get(&shape) {
            return Ok(*id);
        }
        if self.frozen {
            return Err("equation universe is sealed");
        }
        let id = ShapeId(self.nodes.len());
        self.nodes.push(shape.clone());
        self.inputs.push(BTreeSet::new());
        self.interned.insert(shape, id);
        Ok(id)
    }
    pub(super) fn allocated_bytes(&self) -> usize {
        self.nodes.capacity() * size_of::<Shape>()
            + self.inputs.capacity() * size_of::<BTreeSet<ShapeId>>()
            + self.inputs.iter().map(|inputs| inputs.len() * (size_of::<ShapeId>() + 64)).sum::<usize>()
            + self.interned.len() * (size_of::<(Shape, ShapeId)>() + 64)
    }
    pub fn freeze(&mut self) {
        self.frozen = true;
    }
    pub fn connect(&mut self, target: ShapeId, source: ShapeId) -> bool {
        self.inputs[target.0].insert(source)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Incarnation {
    Current,
    History,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Atom {
    pub origin: ShapeId,
    pub incarnation: Incarnation,
}
impl Atom {
    pub fn current(origin: ShapeId) -> Self {
        Self { origin, incarnation: Incarnation::Current }
    }
    pub fn history(self) -> Self {
        Self { incarnation: Incarnation::History, ..self }
    }
}

/// Independent masks: union expresses uncertainty; only a singleton authorizes
/// an operation requiring a definite state. Builder and must-use never alias
/// the ownership journal or one another.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Masks {
    /// Absent = 1, Available = 2, Consumed = 4.
    pub availability: u8,
    /// Live = 1, Destroying = 2, Dead = 4.
    pub lifetime: u8,
    /// Open, Deferring, Settling, Closing, Closed, in successive bits.
    pub scope_phase: u8,
    /// Absent = 1, Pending = 2, Discharged = 4.
    pub must_use: u8,
    /// Current = 1, Invalidated = 2.
    pub snapshot: u8,
    /// Open = 1, Finished = 2, Abandoned = 4.
    pub builder: u8,
    pub protocol: u16,
}
impl Masks {
    fn join(self, other: Self) -> Self {
        Self {
            availability: self.availability | other.availability,
            lifetime: self.lifetime | other.lifetime,
            scope_phase: self.scope_phase | other.scope_phase,
            must_use: self.must_use | other.must_use,
            snapshot: self.snapshot | other.snapshot,
            builder: self.builder | other.builder,
            protocol: self.protocol | other.protocol,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValueState {
    pub deps: BTreeSet<Atom>,
    pub masks: Masks,
}
impl ValueState {
    /// The state of a value before its definition: absent, without dependencies.
    pub const ABSENT: Self = Self {
        deps: BTreeSet::new(),
        masks: Masks {
            availability: 1, lifetime: 0, scope_phase: 0, must_use: 0, snapshot: 0, builder: 0, protocol: 0,
        },
    };
}
static ABSENT: ValueState = ValueState::ABSENT;
impl Default for ValueState {
    fn default() -> Self {
        Self::ABSENT
    }
}

/// Per-value states of the values that may carry ownership facts (#707). The analysis keys exactly the
/// function's values whose type may own, at every program point: a value of any other type never has
/// dependencies, so it reads as `ValueState::ABSENT`. A program point costs space and transfer work in
/// proportion to its facts, not to the size of the function, whose plain values may number thousands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ValueStates(BTreeMap<ValueId, ValueState>);
impl ValueStates {
    pub fn insert(&mut self, value: ValueId, state: ValueState) {
        self.0.insert(value, state);
    }
    pub fn get(&self, value: ValueId) -> Option<&ValueState> {
        self.0.get(&value)
    }
    pub fn remove(&mut self, value: ValueId) -> Option<ValueState> {
        self.0.remove(&value)
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn iter(&self) -> impl Iterator<Item = (&ValueId, &ValueState)> {
        self.0.iter()
    }
    pub fn states(&self) -> impl Iterator<Item = &ValueState> {
        self.0.values()
    }
    pub fn states_mut(&mut self) -> impl Iterator<Item = &mut ValueState> {
        self.0.values_mut()
    }
    /// Pointwise join; a value without a state on one side joins as `ValueState::ABSENT`.
    fn join(&mut self, other: &Self) {
        for (value, state) in &mut self.0 {
            let incoming = other.0.get(value).unwrap_or(&ABSENT);
            state.deps.extend(&incoming.deps);
            state.masks = state.masks.join(incoming.masks);
        }
        for (value, incoming) in &other.0 {
            if !self.0.contains_key(value) {
                let mut state = incoming.clone();
                state.masks = state.masks.join(ValueState::ABSENT.masks);
                self.0.insert(*value, state);
            }
        }
    }
}
impl std::ops::Index<ValueId> for ValueStates {
    type Output = ValueState;
    fn index(&self, value: ValueId) -> &ValueState {
        self.0.get(&value).unwrap_or(&ABSENT)
    }
}
impl std::ops::IndexMut<ValueId> for ValueStates {
    fn index_mut(&mut self, value: ValueId) -> &mut ValueState {
        self.0.entry(value).or_default()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FlowState {
    pub values: ValueStates,
    pub journals: BTreeMap<(ScopeId, Atom), Quantity>,
    /// 1 = admission, 2 = bound. Union is uncertainty, never an admission right.
    pub provenance: BTreeMap<(ScopeId, Atom), u8>,
    pub latest: BTreeMap<BindingId, BTreeSet<ValueId>>,
    pub binding_deps: BTreeMap<BindingId, BTreeSet<Atom>>,
    pub defers: BTreeMap<ScopeId, Vec<InstId>>,
}
impl FlowState {
    pub fn join(&mut self, other: &Self) -> bool {
        let old = self.clone();
        self.values.join(&other.values);
        let keys: BTreeSet<_> = self.journals.keys().chain(other.journals.keys()).copied().collect();
        for key in keys {
            let left = self.journals.get(&key).copied().unwrap_or_default();
            let right = other.journals.get(&key).copied().unwrap_or_default();
            self.journals.insert(key, left.join(right));
        }
        for (key, mask) in &other.provenance {
            *self.provenance.entry(*key).or_default() |= mask;
        }
        for (binding, values) in &other.latest {
            self.latest.entry(*binding).or_default().extend(values);
        }
        for (binding, deps) in &other.binding_deps {
            self.binding_deps.entry(*binding).or_default().extend(deps);
        }
        for (scope, prefix) in &other.defers {
            let target = self.defers.entry(*scope).or_default();
            for registration in prefix {
                if !target.contains(registration) {
                    target.push(*registration);
                }
            }
        }
        *self != old
    }
    pub fn retire(&mut self, origin: ShapeId) {
        for value in self.values.states_mut() {
            let current = Atom::current(origin);
            if value.deps.remove(&current) {
                value.deps.insert(current.history());
            }
        }
        for deps in self.binding_deps.values_mut() {
            let current = Atom::current(origin);
            if deps.remove(&current) {
                deps.insert(current.history());
            }
        }
        let owners: Vec<_> = self.journals.keys().filter_map(|(scope, atom)| {
            (*atom == Atom::current(origin)).then_some(*scope)
        }).collect();
        for owner in owners {
            let current = self.journals.remove(&(owner, Atom::current(origin))).unwrap();
            let history = self.journals.entry((owner, Atom::current(origin).history())).or_default();
            *history = history.append(current);
            if let Some(mask) = self.provenance.remove(&(owner, Atom::current(origin))) {
                *self.provenance.entry((owner, Atom::current(origin).history())).or_default() |= mask;
            }
        }
    }
    /// Conservative allocation bound for the materialized sparse state, including
    /// B-tree node slack/links and vector spare capacity. This is independent of
    /// the number of possible equations and of unrelated module scopes.
    pub fn allocated_bytes(&self) -> usize {
        let trees = self.values.len() * (size_of::<(ValueId, ValueState)>() + 64)
            + self.values.states().map(|value| value.deps.len() * (size_of::<Atom>() + 64)).sum::<usize>()
            + self.journals.len() * (size_of::<((ScopeId, Atom), Quantity)>() + 64)
            + self.provenance.len() * (size_of::<((ScopeId, Atom), u8)>() + 64)
            + self.latest.len() * (size_of::<(BindingId, BTreeSet<ValueId>)>() + 64)
            + self.latest.values().map(|values| values.len() * (size_of::<ValueId>() + 64)).sum::<usize>()
            + self.binding_deps.len() * (size_of::<(BindingId, BTreeSet<Atom>)>() + 64)
            + self.binding_deps.values().map(|deps| deps.len() * (size_of::<Atom>() + 64)).sum::<usize>()
            + self.defers.len() * (size_of::<(ScopeId, Vec<InstId>)>() + 64);
        size_of::<Self>() + trees
            + self.defers.values().map(|prefix| prefix.capacity() * size_of::<InstId>()).sum::<usize>()
    }
    pub(super) fn sparse_bytes(&self) -> usize {
        self.allocated_bytes() - size_of::<Self>()
    }
    pub fn stored_facts(&self) -> usize {
        self.values.states().map(|value| value.deps.len() + 8).sum::<usize>()
            + self.journals.len() * 2 + self.provenance.len()
            + self.latest.values().map(BTreeSet::len).sum::<usize>()
            + self.binding_deps.values().map(BTreeSet::len).sum::<usize>()
            + self.defers.values().map(Vec::len).sum::<usize>()
    }
}
