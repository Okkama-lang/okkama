//! Logical fixed-length arrays with implicit defaults and shared immutable entries.
//! Stored defaults are permitted: equality and all read iterators see logical values.
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::{Index, IndexMut};
use std::rc::Rc;

pub(super) trait AllocationSize {
    fn heap_bytes(&self) -> usize { 0 }
    fn work(&self) -> usize { 1 }
}
impl AllocationSize for u8 {}
impl AllocationSize for bool {}
impl<T: AllocationSize> AllocationSize for Option<T> {
    fn heap_bytes(&self) -> usize { self.as_ref().map_or(0, AllocationSize::heap_bytes) }
    fn work(&self) -> usize { self.as_ref().map_or(1, AllocationSize::work) }
}

// A B-tree node reserves eleven entries and up to twelve child pointers.
// Non-root nodes have at least five entries. Include an allocator header per
// node; charging every node as an internal node conservatively covers leaves.
fn tree_bytes<T>(len: usize) -> usize {
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
}

#[derive(Debug)]
pub(super) struct Entry<T> {
    value: T,
    charged: Cell<usize>,
}
impl<T: Clone> Clone for Entry<T> {
    fn clone(&self) -> Self {
        Self { value: self.value.clone(), charged: Cell::new(0) }
    }
}

#[derive(Debug)]
pub(super) struct PerValue<T> {
    len: usize,
    default: Option<Rc<Entry<T>>>,
    entries: Vec<(usize, Rc<Entry<T>>)>,
    directory_charged: Cell<usize>,
}
impl<T: Clone + Eq> Clone for PerValue<T> {
    fn clone(&self) -> Self {
        Self {
            len: self.len,
            default: self.default.clone(),
            entries: self.entries.iter()
                .filter(|(_, entry)| entry.value != self.default.as_ref().unwrap().value).cloned().collect(),
            directory_charged: Cell::new(0),
        }
    }
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
        let mut left = self.entries.iter().peekable();
        let mut right = other.entries.iter().peekable();
        let mut covered = 0;
        while left.peek().is_some() || right.peek().is_some() {
            let index = left.peek().map(|(index, _)| *index).unwrap_or(usize::MAX)
                .min(right.peek().map(|(index, _)| *index).unwrap_or(usize::MAX));
            let a = if left.peek().is_some_and(|(stored, _)| *stored == index) {
                &left.next().unwrap().1
            } else {
                self.default.as_ref().unwrap()
            };
            let b = if right.peek().is_some_and(|(stored, _)| *stored == index) {
                &right.next().unwrap().1
            } else {
                other.default.as_ref().unwrap()
            };
            if !charge(1) {
                return false;
            }
            if !Rc::ptr_eq(a, b) && (!charge(cost(&a.value).saturating_add(cost(&b.value)))
                || a.value != b.value) {
                return false;
            }
            covered += 1;
        }
        covered == self.len || (charge(1) && self.default.as_ref().map(|entry| &entry.value)
            == other.default.as_ref().map(|entry| &entry.value))
    }
    pub fn new(len: usize, default: T) -> Self {
        Self {
            len,
            default: (len != 0).then(|| Rc::new(Entry { value: default, charged: Cell::new(0) })),
            entries: Vec::new(),
            directory_charged: Cell::new(0),
        }
    }
    /// Stored slots include explicit defaults; callers must preserve their neutral meaning.
    pub fn indexed(&self) -> impl Iterator<Item = (usize, &T)> + Clone {
        self.entries.iter().map(|(index, entry)| (*index, &entry.value))
    }
    pub fn stored(&self) -> impl Iterator<Item = &T> + Clone {
        self.indexed().map(|(_, value)| value)
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
    /// These callers only rewrite existing relations and preserve the default.
    /// Indexed iteration retains SSA indices even when sparse slots are absent.
    pub fn indexed_mut(&mut self) -> impl Iterator<Item = (usize, &mut T)> {
        self.entries.iter_mut().map(|(index, value)| (*index, &mut Rc::make_mut(value).value))
    }
    pub fn stored_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.indexed_mut().map(|(_, value)| value)
    }
    pub fn fill(&mut self, value: T) {
        self.default = (self.len != 0).then(|| Rc::new(Entry { value, charged: Cell::new(0) }));
        self.entries.clear();
    }
    pub fn set(&mut self, index: usize, value: T) {
        if self[index] != value {
            self[index] = value;
        }
    }
}
impl<T: Clone + Eq + AllocationSize> PerValue<T> {
    pub fn work(&self) -> usize {
        self.entries.iter().fold(1usize, |work, (_, entry)| work.saturating_add(entry.value.work()))
    }
    pub fn allocation_bytes(&self) -> usize {
        let initial = self.directory_bytes()
            + self.default.as_ref().map_or(0, |entry| Self::entry_bytes(entry));
        self.entries.iter().fold(initial, |bytes, (_, entry)| {
            bytes.saturating_add(Self::entry_bytes(entry))
        })
    }
    fn directory_bytes(&self) -> usize {
        self.entries.capacity().saturating_mul(size_of::<(usize, Rc<Entry<T>>)>())
    }
    fn entry_bytes(entry: &Entry<T>) -> usize {
        // Rc strong/weak counters, allocator header, the value and its tree allocations.
        (32 + size_of::<Entry<T>>()).saturating_add(entry.value.heap_bytes())
    }
    pub fn charge_new(&self) -> usize {
        let directory = self.directory_bytes();
        let mut bytes = directory.saturating_sub(self.directory_charged.get());
        self.directory_charged.set(directory.max(self.directory_charged.get()));
        for entry in self.default.iter().chain(self.entries.iter().map(|(_, entry)| entry)) {
            let allocation = Self::entry_bytes(entry);
            bytes = bytes.saturating_add(allocation.saturating_sub(entry.charged.get()));
            entry.charged.set(allocation.max(entry.charged.get()));
        }
        bytes
    }
}
impl<T: Clone + Eq> Index<usize> for PerValue<T> {
    type Output = T;
    fn index(&self, index: usize) -> &T {
        assert!(index < self.len);
        match self.entries.binary_search_by_key(&index, |(index, _)| *index) {
            Ok(position) => &self.entries[position].1.value,
            Err(_) => &self.default.as_ref().unwrap().value,
        }
    }
}
impl<T: Clone + Eq> IndexMut<usize> for PerValue<T> {
    fn index_mut(&mut self, index: usize) -> &mut T {
        assert!(index < self.len);
        let position = match self.entries.binary_search_by_key(&index, |(index, _)| *index) {
            Ok(position) => position,
            Err(position) => {
                let entry = Entry {
                    value: self.default.as_ref().unwrap().value.clone(), charged: Cell::new(0),
                };
                self.entries.insert(position, (index, Rc::new(entry)));
                position
            }
        };
        &mut Rc::make_mut(&mut self.entries[position].1).value
    }
}
#[derive(Clone)]
pub(super) struct Iter<'a, T> {
    default: Option<&'a T>,
    entries: std::iter::Peekable<std::slice::Iter<'a, (usize, Rc<Entry<T>>)>>,
    indices: std::ops::Range<usize>,
}
impl<'a, T: Clone + Eq> Iterator for Iter<'a, T> {
    type Item = &'a T;
    fn next(&mut self) -> Option<Self::Item> {
        let index = self.indices.next()?;
        if self.entries.peek().is_some_and(|(stored, _)| *stored == index) {
            self.entries.next().map(|(_, entry)| &entry.value)
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
