//! Pure final OBJECT layout computation, shared by emission and runtime consumers.
use super as s;

/// Final layout includes runtime overrides and hidden physical fields (§2.7).
/// Declaration IDs are SPB object indices, independent of compiler arenas.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectLayout {
    pub runtime: s::ObjectRuntimeLayout,
    pub source_fields: Vec<u32>,
}

pub trait LayoutRegistry {
    fn object_layout(&self, declaration: u32) -> Result<ObjectLayout, s::Error>;
}

impl LayoutRegistry for std::collections::BTreeMap<u32, ObjectLayout> {
    fn object_layout(&self, declaration: u32) -> Result<ObjectLayout, s::Error> {
        self.get(&declaration).cloned().ok_or_else(|| s::error(format!(
            "missing final runtime layout for object {declaration}")))
    }
}

/// A physical storage word that can contain a GC pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraceSlot {
    pub index: usize,
    pub repr: Repr,
}

pub const OBJECT_KIND: u8 = 1;
pub const HAS_IDENTITY: u64 = 1 << 15;
pub const LIFE_SHIFT: u32 = 16;
pub const LIFE_MASK: u64 = 3 << LIFE_SHIFT;

/// How a word of a given static type is traced and converted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Repr {
    /// Tagged small integer or BigInt pointer.
    Int = 0,
    /// Raw bits: float, bool, byte, char.
    Raw = 1,
    /// Pointer to a heap object (or zero).
    Ref = 2,
}

impl Repr {
    pub fn from_bits(bits: u64) -> Self {
        match bits {
            0 => Self::Int,
            1 => Self::Raw,
            _ => Self::Ref,
        }
    }
    /// The heap object a word of this representation points to, if any.
    #[inline]
    pub fn pointer(self, word: u64) -> Option<u64> {
        match self {
            Self::Int if word & 1 == 0 && word != 0 => Some(word),
            Self::Ref if word != 0 => Some(word),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LayoutFacts {
    pub has_destroy: bool,
    pub may_reach_resource: bool,
    pub may_async_destroy: bool,
}

#[derive(Debug, Clone)]
pub struct ObjectRuntimeLayout {
    storage_fields: Box<[Repr]>,
    allocation_words: usize,
    identity_offset: Option<usize>,
    facts: LayoutFacts,
    trace_layout: Box<[TraceSlot]>,
    selection_layout: Box<[usize]>,
}

impl ObjectRuntimeLayout {
    pub fn new(storage: Vec<Repr>, facts: LayoutFacts, selection: Vec<usize>) -> Result<Self, s::Error> {
        if facts.has_destroy && !facts.may_reach_resource {
            return Err(s::error("destroy layout must reach self"));
        }
        if facts.may_async_destroy {
            return Err(s::error("async destroy is outside Champ P1"));
        }
        let mut selected = std::collections::BTreeSet::new();
        for &index in &selection {
            if index >= storage.len() || !selected.insert(index) {
                return Err(s::error("invalid selection layout"));
            }
        }
        let base = storage.len().checked_add(1).ok_or_else(|| s::error("object layout overflow"))?;
        let identity_offset = if facts.has_destroy {
            Some(base.checked_mul(8).ok_or_else(|| s::error("identity offset overflow"))?)
        } else {
            None
        };
        let words = base.checked_add(4 * usize::from(facts.has_destroy))
            .ok_or_else(|| s::error("object allocation overflow"))?;
        // Every emitted load/store uses a signed 32-bit byte offset.
        if words > i32::MAX as usize / 8 {
            return Err(s::error("object layout exceeds JIT offsets"));
        }
        let trace_layout = storage.iter().enumerate().filter_map(|(index, &repr)| {
            (repr != Repr::Raw).then_some(TraceSlot { index, repr })
        }).collect();
        Ok(Self {
            storage_fields: storage.into_boxed_slice(),
            allocation_words: words.max(2),
            identity_offset,
            facts,
            trace_layout,
            selection_layout: selection.into_boxed_slice(),
        })
    }

    pub fn storage_fields(&self) -> &[Repr] {
        &self.storage_fields
    }

    pub fn storage_field_count(&self) -> usize {
        self.storage_fields.len()
    }

    pub fn allocation_words(&self) -> usize {
        self.allocation_words
    }

    pub fn identity_offset(&self) -> Option<usize> {
        self.identity_offset
    }

    pub fn facts(&self) -> LayoutFacts {
        self.facts
    }

    pub fn trace_layout(&self) -> &[TraceSlot] {
        &self.trace_layout
    }

    pub fn selection_layout(&self) -> &[usize] {
        &self.selection_layout
    }

    pub fn field_offset(&self, index: usize) -> usize {
        assert!(index < self.storage_field_count());
        8 * (1 + index)
    }

    pub fn header(&self, descriptor: u32) -> u64 {
        u64::from(OBJECT_KIND) | ((Repr::Raw as u64) << 9) | (u64::from(descriptor) << 32)
            | if self.facts.has_destroy { HAS_IDENTITY } else { 0 }
    }

    /// Freeze a verifier-checked SPB 17 descriptor, checking the structural ABI again.
    pub fn from_spb17(layout: &s::ObjectRuntimeLayout) -> Result<Self, s::Error> {
        let result = Self::new(
            layout.storage_fields.iter().copied().map(repr).collect(),
            LayoutFacts {
                has_destroy: layout.has_destroy,
                may_reach_resource: layout.may_reach_resource,
                may_async_destroy: layout.may_async_destroy,
            },
            layout.selection_layout.iter().map(|&index| index as usize).collect(),
        )?;
        let mut trace = layout.trace_layout.clone();
        trace.sort_unstable();
        if result.allocation_words as u32 != layout.allocation_words
            || result.identity_offset.map(|offset| offset as u32) != layout.identity_offset
            || reference_slots(&layout.storage_fields) != trace
        {
            return Err(s::error("inconsistent SPB 17 runtime layout"));
        }
        Ok(result)
    }

    fn spb17(&self, types: &[s::Type]) -> s::ObjectRuntimeLayout {
        s::ObjectRuntimeLayout {
            storage_fields: types.to_vec(),
            allocation_words: self.allocation_words as u32,
            identity_offset: self.identity_offset.map(|offset| offset as u32),
            has_destroy: self.facts.has_destroy,
            may_reach_resource: self.facts.may_reach_resource,
            may_async_destroy: self.facts.may_async_destroy,
            trace_layout: reference_slots(types),
            selection_layout: self.selection_layout.iter().map(|&index| index as u32).collect(),
        }
    }
}

fn repr(ty: s::Type) -> Repr {
    match ty {
        s::Type::Int => Repr::Int,
        s::Type::Bool | s::Type::Never | s::Type::Descriptor | s::Type::Byte | s::Type::Float | s::Type::Char => {
            Repr::Raw
        }
        _ => Repr::Ref,
    }
}

// SPB 17 lists reference fields; the GC additionally traces tagged Int words.
fn reference_slots(types: &[s::Type]) -> Vec<u32> {
    types.iter().enumerate().filter_map(|(index, ty)| ty.traced().then_some(index as u32)).collect()
}

#[derive(Debug)]
struct RegisteredLayout {
    runtime: ObjectRuntimeLayout,
    types: Vec<s::Type>,
    source_fields: Vec<u32>,
}

/// Runtime-owned implementation of #571's mandatory final-layout seam.
/// Register final storage AFTER intrinsic/construction overrides; source slots are explicit.
#[derive(Debug, Default)]
pub struct RuntimeLayoutRegistry {
    objects: std::collections::BTreeMap<u32, RegisteredLayout>,
}

impl RuntimeLayoutRegistry {
    pub fn register(
        &mut self,
        declaration: u32,
        storage: Vec<s::Type>,
        source_fields: Vec<u32>,
        facts: LayoutFacts,
        selection: Vec<usize>,
    ) -> Result<(), s::Error> {
        if self.objects.contains_key(&declaration) {
            return Err(s::error("duplicate runtime layout"));
        }
        let mut mapped = std::collections::BTreeSet::new();
        for &index in &source_fields {
            if index as usize >= storage.len() || !mapped.insert(index) {
                return Err(s::error("invalid source field mapping"));
            }
        }
        let runtime = ObjectRuntimeLayout::new(storage.iter().copied().map(repr).collect(), facts, selection)?;
        self.objects.insert(declaration, RegisteredLayout { runtime, types: storage, source_fields });
        Ok(())
    }

    pub fn runtime_layout(&self, declaration: u32) -> Option<&ObjectRuntimeLayout> {
        self.objects.get(&declaration).map(|entry| &entry.runtime)
    }
}

impl LayoutRegistry for RuntimeLayoutRegistry {
    fn object_layout(&self, declaration: u32) -> Result<ObjectLayout, s::Error> {
        let entry = self.objects.get(&declaration).ok_or_else(|| s::error(format!(
            "missing final runtime layout for object {declaration}")))?;
        Ok(ObjectLayout {
            runtime: entry.runtime.spb17(&entry.types),
            source_fields: entry.source_fields.clone(),
        })
    }
}

/// Frame storage shape from #572 repr(C) records. Offsets are bytes; record addresses are not roots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServiceLayout {
    pub words: u32,
    pub references: &'static [u32],
}

pub const fn service_layout(kind: s::ServiceKind) -> ServiceLayout {
    use s::ServiceKind::*;
    match kind {
        Deps | Closed => ServiceLayout { words: 0, references: &[] },
        StableKey => ServiceLayout { words: 2, references: &[0] },
        DestroyBorrow => ServiceLayout { words: 5, references: &[0, 32] },
        _ => ServiceLayout { words: 1, references: &[0] },
    }
}
