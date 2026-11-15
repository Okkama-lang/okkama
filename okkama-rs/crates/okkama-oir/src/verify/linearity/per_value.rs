//! Logical fixed-length arrays with implicit defaults and shared immutable entries.
//! Stored defaults are permitted: equality and all read iterators see logical values.
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::{Index, IndexMut};
use super::shared::{Iter as StoredIter, SharedMap};
use std::rc::Rc;

/// Conservative upper bounds of current origin IDs, partitioned by kind.
/// Zero means absent; otherwise the bound is the largest ID plus one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct OriginBounds(pub [u32; 5]);
impl OriginBounds {
    pub const EMPTY: Self = Self([0; 5]);
    pub fn include(&mut self, other: Self) {
        for (bound, incoming) in self.0.iter_mut().zip(other.0) {
            *bound = (*bound).max(incoming);
        }
    }
}
pub(super) trait AllocationSize {
    fn shared_id(&self) -> Option<usize> { None }
    fn origin_bounds(&self) -> OriginBounds { OriginBounds::EMPTY }
    fn comparison_work(&self, other: &Self) -> usize {
        self.work().saturating_add(other.work())
    }
    fn heap_bytes(&self) -> usize { 0 }
    fn work(&self) -> usize { 1 }
    fn unshared_bytes(&self) -> usize { self.heap_bytes() }
    fn charge_shared(&self) -> usize { 0 }
    fn pending_shared(&self) -> usize { 0 }
    fn temporary_bytes(&self) -> usize { 0 }
}
impl AllocationSize for usize {}
impl AllocationSize for u8 {}
impl AllocationSize for bool {}
impl<T: AllocationSize> AllocationSize for Option<T> {
    fn origin_bounds(&self) -> OriginBounds {
        self.as_ref().map_or(OriginBounds::EMPTY, AllocationSize::origin_bounds)
    }
    fn heap_bytes(&self) -> usize { self.as_ref().map_or(0, AllocationSize::heap_bytes) }
    fn work(&self) -> usize { self.as_ref().map_or(1, AllocationSize::work) }
    fn unshared_bytes(&self) -> usize { self.as_ref().map_or(0, AllocationSize::unshared_bytes) }
    fn charge_shared(&self) -> usize { self.as_ref().map_or(0, AllocationSize::charge_shared) }
    fn pending_shared(&self) -> usize { self.as_ref().map_or(0, AllocationSize::pending_shared) }
    fn temporary_bytes(&self) -> usize { self.as_ref().map_or(0, AllocationSize::temporary_bytes) }
}

// A B-tree node reserves eleven entries and up to twelve child pointers.
// Non-root nodes have at least five entries. Include an allocator header per
// node; charging every node as an internal node conservatively covers leaves.
pub(super) fn tree_bytes<T>(len: usize) -> usize {
    len.div_ceil(5).saturating_mul(32 + 11 * size_of::<T>() + 12 * size_of::<usize>())
}
impl<T: AllocationSize> AllocationSize for BTreeSet<T> {
    fn comparison_work(&self, other: &Self) -> usize {
        if self.len() != other.len() { 1 } else { self.work().saturating_add(other.work()) }
    }
    fn origin_bounds(&self) -> OriginBounds {
        let mut ranges = OriginBounds::EMPTY;
        for value in self {
            ranges.include(value.origin_bounds());
        }
        ranges
    }
    fn work(&self) -> usize {
        self.iter().fold(1usize, |work, value| work.saturating_add(value.work()))
    }
    fn heap_bytes(&self) -> usize {
        self.iter().fold(tree_bytes::<T>(self.len()), |bytes, value| {
            bytes.saturating_add(value.heap_bytes())
        })
    }
    fn unshared_bytes(&self) -> usize {
        self.iter().fold(tree_bytes::<T>(self.len()), |bytes, value| {
            bytes.saturating_add(value.unshared_bytes())
        })
    }
    fn charge_shared(&self) -> usize {
        self.iter().fold(0usize, |bytes, value| {
            bytes.saturating_add(value.charge_shared())
        })
    }
    fn pending_shared(&self) -> usize {
        self.iter().fold(0usize, |bytes, value| {
            bytes.saturating_add(value.pending_shared())
        })
    }
    fn temporary_bytes(&self) -> usize {
        self.iter().fold(0usize, |bytes, value| {
            bytes.saturating_add(value.temporary_bytes())
        })
    }
}
impl<K: AllocationSize, V: AllocationSize> AllocationSize for BTreeMap<K, V> {
    fn origin_bounds(&self) -> OriginBounds {
        let mut ranges = OriginBounds::EMPTY;
        for (key, value) in self {
            ranges.include(key.origin_bounds());
            ranges.include(value.origin_bounds());
        }
        ranges
    }
    fn work(&self) -> usize {
        self.iter().fold(1usize, |work, (key, value)| {
            work.saturating_add(key.work()).saturating_add(value.work())
        })
    }
    fn heap_bytes(&self) -> usize {
        self.iter().fold(tree_bytes::<(K, V)>(self.len()), |bytes, (key, value)| {
            bytes.saturating_add(key.heap_bytes()).saturating_add(value.heap_bytes())
        })
    }
    fn unshared_bytes(&self) -> usize {
        self.iter().fold(tree_bytes::<(K, V)>(self.len()), |bytes, (key, value)| {
            bytes.saturating_add(key.unshared_bytes()).saturating_add(value.unshared_bytes())
        })
    }
    fn charge_shared(&self) -> usize {
        self.iter().fold(0usize, |bytes, (key, value)| {
            bytes.saturating_add(key.charge_shared()).saturating_add(value.charge_shared())
        })
    }
    fn pending_shared(&self) -> usize {
        self.iter().fold(0usize, |bytes, (key, value)| {
            bytes.saturating_add(key.pending_shared()).saturating_add(value.pending_shared())
        })
    }
    fn temporary_bytes(&self) -> usize {
        self.iter().fold(0usize, |bytes, (key, value)| {
            bytes.saturating_add(key.temporary_bytes()).saturating_add(value.temporary_bytes())
        })
    }
}

#[derive(Debug)]
pub(super) struct PerValue<T> {
    len: usize,
    default: Option<Rc<DefaultEntry<T>>>,
    entries: SharedMap<usize, T>,
    // Only mutable borrows can leave an explicit default behind. Cloning
    // checks those slots, rather than rechecking every immutable shared entry.
    dirty: Vec<usize>,
    dirty_charged: Cell<usize>,
}
impl<T: Clone + Eq> Clone for PerValue<T> {
    fn clone(&self) -> Self {
        let mut entries = self.entries.clone();
        if let Some(default) = &self.default {
            for index in &self.dirty {
                if entries.get(index).is_some_and(|value| *value == default.value) {
                    entries.remove(index);
                }
            }
        }
        Self {
            len: self.len, default: self.default.clone(), entries, dirty: Vec::new(), dirty_charged: Cell::new(0),
        }
    }
}
#[derive(Debug)]
struct DefaultEntry<T> {
    value: T,
    charged: Cell<usize>,
}
impl<T: Clone + Eq> PartialEq for PerValue<T> {
    fn eq(&self, other: &Self) -> bool {
        self.equal(other, |_| true, |_| 0)
    }
}
impl<T: Clone + Eq> Eq for PerValue<T> {}
impl<T: Clone + Eq> PerValue<T> {
    pub fn equal(
        &self, other: &Self, mut charge: impl FnMut(usize) -> bool, cost: impl Fn(&T) -> usize,
    ) -> bool {
        if !charge(1) || self.len != other.len {
            return false;
        }
        let defaults_equal = self.default.as_ref().map(|entry| &entry.value)
            == other.default.as_ref().map(|entry| &entry.value);
        if defaults_equal && self.entries.same_root(&other.entries) {
            return true;
        }
        let mut left = self.entries.iter().peekable();
        let mut right = other.entries.iter().peekable();
        let mut covered = 0;
        while left.peek().is_some() || right.peek().is_some() {
            let index = left.peek().map(|(index, _)| **index).unwrap_or(usize::MAX)
                .min(right.peek().map(|(index, _)| **index).unwrap_or(usize::MAX));
            let a = if left.peek().is_some_and(|(stored, _)| **stored == index) {
                left.next().unwrap().1
            } else {
                &self.default.as_ref().unwrap().value
            };
            let b = if right.peek().is_some_and(|(stored, _)| **stored == index) {
                right.next().unwrap().1
            } else {
                &other.default.as_ref().unwrap().value
            };
            if !charge(1) || (!std::ptr::eq(a, b)
                && (!charge(cost(a).saturating_add(cost(b))) || a != b)) {
                return false;
            }
            covered += 1;
        }
        covered == self.len || (charge(1) && defaults_equal)
    }
    pub fn new(len: usize, default: T) -> Self {
        Self {
            len,
            default: (len != 0).then(|| Rc::new(DefaultEntry { value: default, charged: Cell::new(0) })),
            entries: SharedMap::new(), dirty: Vec::new(), dirty_charged: Cell::new(0),
        }
    }
    /// Stored slots include explicit defaults; callers must preserve their neutral meaning.
    pub fn indexed(&self) -> impl Iterator<Item = (usize, &T)> + Clone {
        self.entries.iter().map(|(index, value)| (*index, value))
    }
    pub fn stored_len(&self) -> usize {
        self.entries.len()
    }
    pub fn stored(&self) -> impl Iterator<Item = &T> + Clone {
        self.entries.values()
    }
    pub fn get(&self, index: usize) -> Option<&T> {
        (index < self.len).then(|| &self[index])
    }
    pub fn iter(&self) -> Iter<'_, T> {
        Iter {
            default: self.default.as_ref().map(|entry| &entry.value),
            entries: self.entries.iter().peekable(),
            indices: 0..self.len,
        }
    }
    pub fn indexed_mut(&mut self) -> impl Iterator<Item = (usize, &mut T)> {
        self.dirty.extend(self.entries.keys().copied());
        self.compact_dirty();
        self.entries.iter_mut().map(|(index, value)| (*index, value))
    }
    pub fn stored_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.dirty.extend(self.entries.keys().copied());
        self.compact_dirty();
        self.entries.values_mut()
    }
    /// Every mutable access records its slot, and a slot is usually accessed many times between clones. Once
    /// the record outgrows the stored slots, keep each stored slot once: only a stored slot can hold an explicit
    /// default, so a clone and its charge scale with the slots, not with the accesses (#693).
    fn compact_dirty(&mut self) {
        if self.dirty.len() <= 2 * self.entries.len() + 16 {
            return;
        }
        self.dirty.sort_unstable();
        self.dirty.dedup();
        let entries = &self.entries;
        self.dirty.retain(|index| entries.get(index).is_some());
    }
    /// Bulk rewrites may touch only a few logical slots. Do not detach the
    /// unchanged entries merely because a caller visits them during a scan.
    pub fn rewrite_stored(&mut self, mut rewrite: impl FnMut(&T) -> T) {
        let changed: Vec<_> = self.indexed().filter_map(|(index, value)| {
            let next = rewrite(value);
            (next != *value).then_some((index, next))
        }).collect();
        for (index, value) in changed {
            self.set(index, value);
        }
    }
    /// The callback proves unaffected entries unchanged before cloning them.
    pub fn rewrite_changed(&mut self, mut rewrite: impl FnMut(&T) -> Option<T>) -> usize
    where T: AllocationSize {
        let mut work = 1;
        let changed: Vec<_> = self.indexed().filter_map(|(index, value)| {
            let next = rewrite(value)?;
            Some((index, next))
        }).collect();
        for (index, value) in &changed {
            let default = &self.default.as_ref().unwrap().value;
            work += value.comparison_work(default);
            if value == default {
                self.dirty.push(*index);
            }
        }
        self.compact_dirty();
        work + self.entries.replace_values(changed)
    }
    #[cfg(test)]
    pub fn fill(&mut self, value: T) {
        self.default = (self.len != 0).then(|| Rc::new(DefaultEntry { value, charged: Cell::new(0) }));
        self.entries.clear();
        self.dirty.clear();
    }
    pub fn set(&mut self, index: usize, value: T) {
        if self.default.as_ref().is_some_and(|entry| entry.value == value) {
            assert!(index < self.len);
            self.entries.remove(&index);
        } else if self[index] != value {
            self.entries.insert(index, value);
        }
    }
    pub fn retain_indices(&mut self, keep: &[bool]) {
        self.entries.retain(|index, _| keep[*index]);
    }
}
impl<T: Clone + Eq + AllocationSize> PerValue<T> {
    /// Stable last-write-wins assignment, merged with the old sorted directory.
    /// Unchanged entries need neither a search nor a new persistent path.
    pub fn assign_entries(&mut self, desired: Vec<(usize, T)>, charge: impl FnMut(usize) -> bool) {
        self.merge_entries(desired, true, charge);
    }
    pub fn update_entries(&mut self, desired: Vec<(usize, T)>, charge: impl FnMut(usize) -> bool) {
        self.merge_entries(desired, false, charge);
    }
    fn merge_entries(
        &mut self, mut desired: Vec<(usize, T)>, clear_missing: bool, mut charge: impl FnMut(usize) -> bool,
    ) {
        desired.sort_by(|left, right| {
            charge(1);
            left.0.cmp(&right.0)
        });
        let mut incoming = desired.into_iter().peekable();
        let mut current = self.indexed().peekable();
        let mut changed = Vec::new();
        let mut inserted = Vec::new();
        let mut removed = Vec::new();
        let mut equal_cache = BTreeMap::new();
        while current.peek().is_some() || incoming.peek().is_some() {
            if !charge(1) {
                return;
            }
            let left = current.peek().map_or(usize::MAX, |(index, _)| *index);
            let right = incoming.peek().map_or(usize::MAX, |(index, _)| *index);
            if left < right {
                current.next();
                if clear_missing {
                    removed.push(left);
                }
                continue;
            }
            let (index, mut value) = incoming.next().unwrap();
            assert!(index < self.len);
            while incoming.peek().is_some_and(|(next, _)| *next == index) {
                charge(1);
                value = incoming.next().unwrap().1;
            }
            if left == index {
                let (_, old) = current.next().unwrap();
                let equal = if let (Some(a), Some(b)) = (old.shared_id(), value.shared_id()) {
                    if !charge(2 + equal_cache.len().max(1).ilog2() as usize) {
                        return;
                    }
                    let (_, _, equal) = equal_cache.entry((a, b)).or_insert_with(|| {
                        charge(old.comparison_work(&value));
                        // Hold both roots while their addresses are cache keys.
                        (old.clone(), value.clone(), old == &value)
                    });
                    *equal
                } else {
                    if !charge(old.comparison_work(&value)) {
                        return;
                    }
                    old == &value
                };
                if equal {
                    continue;
                }
            }
            let default = &self.default.as_ref().unwrap().value;
            if !charge(value.comparison_work(default)) {
                return;
            }
            if value == *default {
                removed.push(index);
            } else if left == index {
                changed.push((index, value));
            } else {
                inserted.push((index, value));
            }
        }
        drop(current);
        if !charge(self.entries.replace_values(changed)) {
            return;
        }
        let path = 2 + self.entries.len().max(1).ilog2() as usize;
        for index in removed {
            if !charge(path) {
                return;
            }
            self.entries.remove(&index);
        }
        for (index, value) in inserted {
            if !charge(path) {
                return;
            }
            self.entries.insert(index, value);
        }
    }
    pub fn clone_work(&self) -> usize {
        let path = 2 + self.len.max(1).ilog2() as usize;
        self.dirty.iter().fold(1usize, |work, index| {
            work + 2 * path + self.entries.get(index).map_or(1, |value| {
                value.comparison_work(&self.default.as_ref().unwrap().value)
            })
        })
    }
    pub fn origin_bounds(&self, charge: &mut impl FnMut(usize) -> bool) -> OriginBounds {
        // Defaults are neutral in Flow. Stored explicit defaults are neutral too.
        self.entries.origin_bounds(charge)
    }

    pub fn work(&self) -> usize {
        self.entries.values().fold(1usize, |work, value| work.saturating_add(value.work()))
    }
    #[cfg(any(test, feature = "verifier-differential"))]
    pub fn allocation_bytes(&self) -> usize {
        self.dirty.capacity() * size_of::<usize>() + self.entries.allocation_bytes()
            + self.default.as_ref().map_or(0, |entry| {
            32 + size_of::<DefaultEntry<T>>() + entry.value.heap_bytes()
        })
    }
    pub fn charge_new(&self) -> usize {
        let dirty_bytes = self.dirty.capacity() * size_of::<usize>();
        let mut bytes = dirty_bytes.saturating_sub(self.dirty_charged.get()) + self.entries.charge_new();
        self.dirty_charged.set(dirty_bytes.max(self.dirty_charged.get()));
        if let Some(entry) = &self.default {
            let allocation = 32 + size_of::<DefaultEntry<T>>() + entry.value.unshared_bytes();
            bytes += allocation.saturating_sub(entry.charged.get()) + entry.value.charge_shared();
            entry.charged.set(allocation.max(entry.charged.get()));
        }
        bytes
    }
    pub fn scratch_bytes(&self) -> usize {
        let directory = self.entries.scratch_directory_bytes();
        let private = self.entries.values().map(AllocationSize::unshared_bytes).sum::<usize>();
        // A bulk rewrite can collect one desired set before applying its delta.
        // The buffer includes ordered-tree nodes, removed keys and iterator storage.
        let rewrite = self.entries.values().map(AllocationSize::temporary_bytes).max().unwrap_or(0);
        let default = self.default.as_ref().map_or(0, |entry| {
            let allocation = 32 + size_of::<DefaultEntry<T>>() + entry.value.unshared_bytes();
            allocation.saturating_sub(entry.charged.get()) + entry.value.pending_shared()
        });
        self.dirty.capacity() * size_of::<usize>()
            + self.entries.new_bytes(false) + directory + default + private + rewrite
    }

}
impl<T: Clone + Eq> Index<usize> for PerValue<T> {
    type Output = T;
    fn index(&self, index: usize) -> &T {
        assert!(index < self.len);
        self.entries.get(&index).unwrap_or_else(|| &self.default.as_ref().unwrap().value)
    }
}
impl<T: Clone + Eq> IndexMut<usize> for PerValue<T> {
    fn index_mut(&mut self, index: usize) -> &mut T {
        assert!(index < self.len);
        // Compact before recording: the slot may not be stored yet.
        self.compact_dirty();
        self.dirty.push(index);
        self.entries.entry(index).or_insert_with(|| self.default.as_ref().unwrap().value.clone())
    }
}
#[derive(Clone)]
pub(super) struct Iter<'a, T> {
    default: Option<&'a T>,
    entries: std::iter::Peekable<StoredIter<'a, usize, T>>,
    indices: std::ops::Range<usize>,
}
impl<'a, T: Clone + Eq> Iterator for Iter<'a, T> {
    type Item = &'a T;
    fn next(&mut self) -> Option<Self::Item> {
        let index = self.indices.next()?;
        if self.entries.peek().is_some_and(|(stored, _)| **stored == index) {
            self.entries.next().map(|(_, value)| value)
        } else {
            self.default
        }
    }
}
impl<'a, T: Clone + Eq> IntoIterator for &'a PerValue<T> {
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bulk_assignment_matches_last_write_wins_and_preserves_the_old_version() {
        let mut values = PerValue::new(8, 0u8);
        values.set(1, 9);
        values.set(2, 8);
        values.set(7, 7);
        let old = values.clone();
        values.assign_entries(vec![(5, 4), (1, 2), (5, 6), (3, 0)], |_| true);
        assert_eq!(values.iter().copied().collect::<Vec<_>>(), vec![0, 2, 0, 0, 0, 6, 0, 0]);
        assert_eq!(old.iter().copied().collect::<Vec<_>>(), vec![0, 9, 8, 0, 0, 0, 0, 7]);
        values.update_entries(vec![(4, 5), (1, 3), (4, 6)], |_| true);
        assert_eq!(values.iter().copied().collect::<Vec<_>>(), vec![0, 3, 0, 0, 6, 6, 0, 0]);
        values.assign_entries(Vec::new(), |_| true);
        assert_eq!(values.indexed().count(), 0);
    }

    #[test]
    fn cloning_prunes_only_mutably_borrowed_slots() {
        let mut values = PerValue::new(100_000, 0u8);
        for index in 0..10_000 {
            values.set(index, 1);
        }
        let original = values.clone();
        assert_eq!(values.clone_work(), 1);
        values[321] = 0;
        values[40_000] = 0;
        let copied = values.clone();
        assert_eq!(copied.indexed().count(), 9_999);
        assert_eq!(copied[321], 0);
        assert_eq!(original[321], 1);
        assert!(values.clone_work() < 100);
        assert_eq!(copied.clone_work(), 1);
        // Bulk mutable access must mark every borrowed slot too.
        for (_, value) in values.indexed_mut() {
            *value = 0;
        }
        assert_eq!(values.clone().indexed().count(), 0);
    }

    /// A slot accessed many times between clones is recorded once: the record stays within the stored slots,
    /// the clone charge does not grow with the accesses, and a clone still drops explicit defaults (#693).
    #[test]
    fn repeated_mutable_access_records_each_slot_once() {
        let mut values = PerValue::new(1_000, 0u8);
        for index in 0..10 {
            values.set(index, 1);
        }
        let once = {
            let mut copy = values.clone();
            for index in 0..10 {
                copy[index] = 2;
            }
            copy.clone_work()
        };
        for _ in 0..1_000 {
            for index in 0..10 {
                values[index] = 2;
            }
            for value in values.stored_mut() {
                *value = 2;
            }
        }
        assert!(values.dirty.len() <= 2 * values.stored_len() + 16, "{}", values.dirty.len());
        assert!(values.clone_work() <= 3 * once, "{} {once}", values.clone_work());
        values[3] = 0;
        values[500] = 0;
        let copied = values.clone();
        assert_eq!(copied.indexed().count(), 9);
        assert_eq!(copied[3], 0);
        assert_eq!(copied[4], 2);
    }

    #[test]
    fn logical_defaults_and_indices_survive_sharing_and_explicit_empty_entries() {
        let mut values = PerValue::new(100, BTreeSet::<u8>::new());
        let empty = values.clone();
        values[70].clear();
        assert_eq!(values, empty);
        values[90].insert(7);
        values[3].insert(2);
        let mut copied = values.clone();
        copied[90].insert(8);
        assert_eq!(values[90], BTreeSet::from([7]));
        assert_eq!(copied[90], BTreeSet::from([7, 8]));
        assert_eq!(values.iter().enumerate().filter(|(_, value)| !value.is_empty())
            .map(|(index, _)| index).collect::<Vec<_>>(), vec![3, 90]);
        assert_eq!(values.indexed_mut().map(|(index, _)| index).collect::<Vec<_>>(), vec![3, 70, 90]);
    }

    #[test]
    fn sparse_equality_matches_logical_equality_with_distinct_defaults() {
        for len in 0..8 {
            for mask in 0..(1 << len) {
                let mut left = PerValue::new(len, 0u8);
                let mut right = PerValue::new(len, 1u8);
                for index in 0..len {
                    let value = u8::from(mask & (1 << index) != 0);
                    left[index] = value;
                    right[index] = value;
                }
                assert_eq!(left, right);
                for index in 0..len {
                    let mut changed = right.clone();
                    changed[index] ^= 1;
                    assert_ne!(left, changed);
                }
            }
        }
    }

    #[test]
    fn sparse_comparison_work_tracks_stored_entries_and_shared_values() {
        let mut left = PerValue::new(1_000_000, BTreeSet::new());
        left[999_999].insert(7u8);
        let right = left.clone();
        let mut shared_work = 0;
        assert!(left.equal(&right, |work| {
            shared_work += work;
            true
        }, AllocationSize::work));
        assert!(shared_work < 10);
        let mut copied = right.clone();
        copied[999_999].insert(8);
        let mut copied_work = 0;
        assert!(!left.equal(&copied, |work| {
            copied_work += work;
            true
        }, AllocationSize::work));
        assert!(copied_work > shared_work);
        assert!(copied_work < 20);
    }

    #[test]
    fn zero_values_do_not_allocate_an_unobservable_default() {
        let mut values = PerValue::new(0, BTreeSet::<u8>::new());
        assert_eq!(values.allocation_bytes(), 0);
        assert_eq!(values.charge_new(), 0);
        values.fill(BTreeSet::from([1]));
        assert_eq!(values, values.clone());
        assert_eq!(values.iter().next(), None);
        assert_eq!(values.get(0), None);
        assert_eq!(values.charge_new(), 0);
    }

    #[test]
    fn shared_allocations_are_charged_once_and_cow_is_charged_again() {
        let mut values = PerValue::new(100, BTreeSet::<u8>::new());
        values[4].insert(1);
        let original = values.charge_new();
        assert_eq!(values.charge_new(), 0);
        let mut copied = values.clone();
        let directory = copied.charge_new();
        assert!(directory < original);
        copied[4].insert(2);
        assert!(copied.charge_new() > directory);
        assert_eq!(copied.charge_new(), 0);
        assert_eq!(values[4], BTreeSet::from([1]));
    }
}
