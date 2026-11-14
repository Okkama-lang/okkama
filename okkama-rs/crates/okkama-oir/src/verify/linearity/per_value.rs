//! Logical fixed-length arrays with implicit defaults and shared immutable entries.
//! Stored defaults are permitted: equality and all read iterators see logical values.
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::{Index, IndexMut};
use super::shared::{Iter as StoredIter, SharedMap};
use std::rc::Rc;

pub(super) trait AllocationSize {
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
}
impl<T: Clone + Eq> Clone for PerValue<T> {
    fn clone(&self) -> Self {
        let mut entries = self.entries.clone();
        if let Some(default) = &self.default {
            entries.retain(|_, value| *value != default.value);
        }
        Self { len: self.len, default: self.default.clone(), entries }
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
            entries: SharedMap::new(),
        }
    }
    /// Stored slots include explicit defaults; callers must preserve their neutral meaning.
    pub fn indexed(&self) -> impl Iterator<Item = (usize, &T)> + Clone {
        self.entries.iter().map(|(index, value)| (*index, value))
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
        self.entries.iter_mut().map(|(index, value)| (*index, value))
    }
    pub fn stored_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.entries.values_mut()
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
    pub fn assign(&mut self, other: Self) {
        assert_eq!(self.len, other.len);
        assert!(self.default.as_ref().map(|entry| &entry.value)
            == other.default.as_ref().map(|entry| &entry.value));
        let removed: Vec<_> = self.entries.keys().filter(|index| other.entries.get(index).is_none())
            .copied().collect();
        for index in removed {
            self.entries.remove(&index);
        }
        for (index, value) in other.entries {
            self.set(index, value);
        }
    }
    #[cfg(test)]
    pub fn fill(&mut self, value: T) {
        self.default = (self.len != 0).then(|| Rc::new(DefaultEntry { value, charged: Cell::new(0) }));
        self.entries.clear();
    }
    pub fn set(&mut self, index: usize, value: T) {
        if self.default.as_ref().is_some_and(|entry| entry.value == value) {
            assert!(index < self.len);
            self.entries.remove(&index);
        } else if self[index] != value {
            self[index] = value;
        }
    }
    pub fn retain_indices(&mut self, keep: &[bool]) {
        self.entries.retain(|index, _| keep[*index]);
    }
}
impl<T: Clone + Eq + AllocationSize> PerValue<T> {
    pub fn work(&self) -> usize {
        self.entries.values().fold(1usize, |work, value| work.saturating_add(value.work()))
    }
    #[cfg(any(test, feature = "verifier-differential"))]
    pub fn allocation_bytes(&self) -> usize {
        self.entries.allocation_bytes() + self.default.as_ref().map_or(0, |entry| {
            32 + size_of::<DefaultEntry<T>>() + entry.value.heap_bytes()
        })
    }
    pub fn charge_new(&self) -> usize {
        let mut bytes = self.entries.charge_new();
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
        self.entries.new_bytes(false) + directory + default + private + rewrite
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
