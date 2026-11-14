//! Persistent ordered AVL relations. Cloning a root is constant time; changing
//! one key copies only its search path and the changed entry. Iteration and
//! equality use key order, independently of tree shape and allocation identity.
use super::per_value::AllocationSize;
use std::cell::Cell;
use std::cmp::Ordering;
use std::rc::Rc;

#[cfg(any(test, feature = "verifier-differential"))]
thread_local! {
    static RETENTION_COUNTS: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
}
#[cfg(any(test, feature = "verifier-differential"))]
pub(super) fn retention_counts() -> (usize, usize) {
    RETENTION_COUNTS.with(|counts| counts.replace((0, 0)))
}

type Link<K, V> = Option<Rc<Node<K, V>>>;

#[derive(Debug)]
struct Entry<K, V> {
    pair: (K, V),
    charged: Cell<usize>,
}
impl<K: Clone, V: Clone> Clone for Entry<K, V> {
    fn clone(&self) -> Self {
        Self { pair: self.pair.clone(), charged: Cell::new(0) }
    }
}
#[derive(Debug)]
struct Node<K, V> {
    entry: Rc<Entry<K, V>>,
    left: Link<K, V>,
    right: Link<K, V>,
    height: usize,
    charged: Cell<bool>,
}
impl<K, V> Clone for Node<K, V> {
    fn clone(&self) -> Self {
        Self {
            entry: self.entry.clone(), left: self.left.clone(), right: self.right.clone(),
            height: self.height, charged: Cell::new(false),
        }
    }
}
#[derive(Debug)]
pub(super) struct SharedMap<K, V> {
    root: Link<K, V>,
    len: usize,
}
impl<K, V> Clone for SharedMap<K, V> {
    fn clone(&self) -> Self {
        Self { root: self.root.clone(), len: self.len }
    }
}
impl<K, V> Default for SharedMap<K, V> {
    fn default() -> Self {
        Self { root: None, len: 0 }
    }
}
impl<K: Ord + Clone, V: Clone> SharedMap<K, V> {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn scratch_directory_bytes(&self) -> usize {
        // Pruning may copy a path even when the source root is already charged.
        // Include those node headers, iterator stacks and growing rewrite vectors.
        self.len().saturating_mul(64 + size_of::<Node<K, V>>() + 2 * size_of::<(K, V)>())
    }
    pub fn clear(&mut self) {
        self.root = None;
        self.len = 0;
    }
    fn height(link: &Link<K, V>) -> usize {
        link.as_ref().map_or(0, |node| node.height)
    }
    fn update(node: &mut Node<K, V>) {
        node.height = 1 + Self::height(&node.left).max(Self::height(&node.right));
    }
    fn rotate_left(link: &mut Link<K, V>) {
        let mut old = link.take().unwrap();
        let mut next = Rc::make_mut(&mut old).right.take().unwrap();
        Rc::make_mut(&mut old).right = Rc::make_mut(&mut next).left.take();
        Self::update(Rc::make_mut(&mut old));
        Rc::make_mut(&mut next).left = Some(old);
        Self::update(Rc::make_mut(&mut next));
        *link = Some(next);
    }
    fn rotate_right(link: &mut Link<K, V>) {
        let mut old = link.take().unwrap();
        let mut next = Rc::make_mut(&mut old).left.take().unwrap();
        Rc::make_mut(&mut old).left = Rc::make_mut(&mut next).right.take();
        Self::update(Rc::make_mut(&mut old));
        Rc::make_mut(&mut next).right = Some(old);
        Self::update(Rc::make_mut(&mut next));
        *link = Some(next);
    }
    fn balance(link: &mut Link<K, V>) {
        let node = Rc::make_mut(link.as_mut().unwrap());
        Self::update(node);
        if Self::height(&node.left) > Self::height(&node.right) + 1 {
            let left = node.left.as_ref().unwrap();
            if Self::height(&left.right) > Self::height(&left.left) {
                Self::rotate_left(&mut node.left);
            }
            Self::rotate_right(link);
        } else if Self::height(&node.right) > Self::height(&node.left) + 1 {
            let right = node.right.as_ref().unwrap();
            if Self::height(&right.left) > Self::height(&right.right) {
                Self::rotate_right(&mut node.right);
            }
            Self::rotate_left(link);
        }
    }
    pub fn get(&self, key: &K) -> Option<&V> {
        let mut link = &self.root;
        while let Some(node) = link {
            match key.cmp(&node.entry.pair.0) {
                Ordering::Less => link = &node.left,
                Ordering::Greater => link = &node.right,
                Ordering::Equal => return Some(&node.entry.pair.1),
            }
        }
        None
    }
    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        // An absent lookup must not copy a path.
        self.get(key)?;
        let mut link = &mut self.root;
        while let Some(node) = link {
            let node = Rc::make_mut(node);
            match key.cmp(&node.entry.pair.0) {
                Ordering::Less => link = &mut node.left,
                Ordering::Greater => link = &mut node.right,
                Ordering::Equal => return Some(&mut Rc::make_mut(&mut node.entry).pair.1),
            }
        }
        None
    }
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        fn put<K: Ord + Clone, V: Clone>(link: &mut Link<K, V>, key: K, value: V) -> Option<V> {
            let Some(node) = link else {
                *link = Some(Rc::new(Node {
                    entry: Rc::new(Entry { pair: (key, value), charged: Cell::new(0) }),
                    left: None, right: None, height: 1, charged: Cell::new(false),
                }));
                return None;
            };
            let node = Rc::make_mut(node);
            let old = match key.cmp(&node.entry.pair.0) {
                Ordering::Less => put(&mut node.left, key, value),
                Ordering::Greater => put(&mut node.right, key, value),
                Ordering::Equal => return Some(std::mem::replace(&mut Rc::make_mut(&mut node.entry).pair.1, value)),
            };
            SharedMap::balance(link);
            old
        }
        let old = put(&mut self.root, key, value);
        if old.is_none() {
            self.len += 1;
        }
        old
    }
    pub fn remove(&mut self, key: &K) -> Option<V> {
        fn take_min<K: Ord + Clone, V: Clone>(link: &mut Link<K, V>) -> Rc<Entry<K, V>> {
            let node = Rc::make_mut(link.as_mut().unwrap());
            if node.left.is_none() {
                let entry = node.entry.clone();
                *link = node.right.take();
                return entry;
            }
            let entry = take_min(&mut node.left);
            SharedMap::balance(link);
            entry
        }
        fn erase<K: Ord + Clone, V: Clone>(link: &mut Link<K, V>, key: &K) -> V {
            let node = Rc::make_mut(link.as_mut().unwrap());
            let old = match key.cmp(&node.entry.pair.0) {
                Ordering::Less => erase(&mut node.left, key),
                Ordering::Greater => erase(&mut node.right, key),
                Ordering::Equal => {
                    let value = node.entry.pair.1.clone();
                    if node.right.is_none() {
                        *link = node.left.take();
                        return value;
                    }
                    node.entry = take_min(&mut node.right);
                    value
                }
            };
            SharedMap::balance(link);
            old
        }
        self.get(key)?;
        let old = erase(&mut self.root, key);
        self.len -= 1;
        Some(old)
    }
    pub fn iter(&self) -> Iter<'_, K, V> {
        let mut iter = Iter { stack: Vec::new() };
        iter.descend(self.root.as_deref());
        iter
    }
    pub fn keys(&self) -> impl Iterator<Item = &K> + Clone {
        self.iter().map(|(key, _)| key)
    }
    pub fn values(&self) -> impl Iterator<Item = &V> + Clone {
        self.iter().map(|(_, value)| value)
    }
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&K, &mut V)> {
        fn visit<'a, K: Clone, V: Clone>(link: &'a mut Link<K, V>, out: &mut Vec<(&'a K, &'a mut V)>) {
            if let Some(node) = link {
                let node = Rc::make_mut(node);
                visit(&mut node.left, out);
                let (key, value) = &mut Rc::make_mut(&mut node.entry).pair;
                out.push((key, value));
                visit(&mut node.right, out);
            }
        }
        let mut entries = Vec::with_capacity(self.len);
        visit(&mut self.root, &mut entries);
        entries.into_iter()
    }
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut V> {
        self.iter_mut().map(|(_, value)| value)
    }
    pub fn retain(&mut self, mut keep: impl FnMut(&K, &V) -> bool) {
        // Discover removals immutably: unchanged subtrees remain shared.
        let removed: Vec<_> = self.iter().filter(|(key, value)| !keep(key, value))
            .map(|(key, _)| key.clone()).collect();
        for key in removed {
            self.remove(&key);
        }
    }
    pub fn entry(&mut self, key: K) -> MapEntry<'_, K, V> {
        MapEntry { map: self, key }
    }
}
pub(super) struct MapEntry<'a, K, V> {
    map: &'a mut SharedMap<K, V>,
    key: K,
}
impl<'a, K: Ord + Clone, V: Clone> MapEntry<'a, K, V> {
    pub fn or_insert_with(self, value: impl FnOnce() -> V) -> &'a mut V {
        if self.map.get(&self.key).is_none() {
            self.map.insert(self.key.clone(), value());
        }
        self.map.get_mut(&self.key).unwrap()
    }

}
pub(super) struct Iter<'a, K, V> {
    stack: Vec<&'a Node<K, V>>,
}
impl<K, V> Clone for Iter<'_, K, V> {
    fn clone(&self) -> Self {
        Self { stack: self.stack.clone() }
    }
}
impl<'a, K, V> Iter<'a, K, V> {
    fn descend(&mut self, mut node: Option<&'a Node<K, V>>) {
        while let Some(current) = node {
            self.stack.push(current);
            node = current.left.as_deref();
        }
    }
}
impl<'a, K, V> Iterator for Iter<'a, K, V> {
    type Item = (&'a K, &'a V);
    fn next(&mut self) -> Option<Self::Item> {
        let node = self.stack.pop()?;
        self.descend(node.right.as_deref());
        Some((&node.entry.pair.0, &node.entry.pair.1))
    }
}
impl<'a, K: Ord + Clone, V: Clone> IntoIterator for &'a SharedMap<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter = Iter<'a, K, V>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
impl<K: Ord + Clone, V: Clone> IntoIterator for SharedMap<K, V> {
    type Item = (K, V);
    type IntoIter = std::vec::IntoIter<(K, V)>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter().map(|(key, value)| (key.clone(), value.clone())).collect::<Vec<_>>().into_iter()
    }
}
impl<K: Ord + Clone, V: Clone> FromIterator<(K, V)> for SharedMap<K, V> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut map = Self::new();
        map.extend(iter);
        map
    }
}
impl<K: Ord + Clone, V: Clone> Extend<(K, V)> for SharedMap<K, V> {
    fn extend<I: IntoIterator<Item = (K, V)>>(&mut self, iter: I) {
        for (key, value) in iter {
            self.insert(key, value);
        }
    }
}
impl<K: Ord + Clone, V: Clone + Eq> PartialEq for SharedMap<K, V> {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len && (self.same_root(other) || self.iter().eq(other.iter()))
    }
}
impl<K: Ord + Clone, V: Clone + Eq> Eq for SharedMap<K, V> {}
impl<K, V> SharedMap<K, V> {
    pub fn same_root(&self, other: &Self) -> bool {
        match (&self.root, &other.root) {
            (Some(left), Some(right)) => Rc::ptr_eq(left, right),
            (None, None) => true,
            _ => false,
        }
    }
}
impl<K: Ord + Clone + AllocationSize, V: Clone + AllocationSize> SharedMap<K, V> {
    pub fn allocation_bytes(&self) -> usize {
        fn visit<K: AllocationSize, V: AllocationSize>(link: &Link<K, V>) -> usize {
            link.as_ref().map_or(0, |node| {
                64 + size_of::<Node<K, V>>() + size_of::<Entry<K, V>>()
                    + node.entry.pair.0.heap_bytes() + node.entry.pair.1.heap_bytes()
                    + visit(&node.left) + visit(&node.right)
            })
        }
        visit(&self.root)
    }
    pub fn new_bytes(&self, charge: bool) -> usize {
        fn visit<K: AllocationSize, V: AllocationSize>(link: &Link<K, V>, charge: bool) -> usize {
            link.as_ref().map_or(0, |node| {
                let mut bytes = if node.charged.get() { 0 } else { 32 + size_of::<Node<K, V>>() };
                if charge {
                    #[cfg(any(test, feature = "verifier-differential"))]
                    RETENTION_COUNTS.with(|counts| {
                        let (shared, allocated) = counts.get();
                        if node.charged.get() {
                            counts.set((shared + 1, allocated));
                        } else {
                            counts.set((shared, allocated + 1));
                        }
                    });
                    node.charged.set(true);
                }
                let entry = &node.entry;
                let allocation = 32 + size_of::<Entry<K, V>>()
                    + entry.pair.0.unshared_bytes() + entry.pair.1.unshared_bytes();
                bytes += allocation.saturating_sub(entry.charged.get());
                bytes += if charge { entry.pair.0.charge_shared() + entry.pair.1.charge_shared() }
                    else { entry.pair.0.pending_shared() + entry.pair.1.pending_shared() };
                if charge {
                    entry.charged.set(allocation.max(entry.charged.get()));
                }
                bytes + visit(&node.left, charge) + visit(&node.right, charge)
            })
        }
        visit(&self.root, charge)
    }
    pub fn charge_new(&self) -> usize {
        self.new_bytes(true)
    }

}
impl<K: Ord + Clone + AllocationSize, V: Clone + AllocationSize> AllocationSize for SharedMap<K, V> {
    fn heap_bytes(&self) -> usize {
        self.allocation_bytes()
    }
    fn work(&self) -> usize {
        self.iter().fold(1usize, |work, (key, value)| work.saturating_add(key.work()).saturating_add(value.work()))
    }
}

/// SSA labels have a stable ordinal. Group nearby labels into bounded pages;
/// a changed label copies at most 64 labels and the AVL path to their page.
pub(super) trait PageKey: Ord + Clone {
    fn ordinal(&self) -> usize;
}
impl PageKey for crate::ValueId {
    fn ordinal(&self) -> usize { self.as_u32() as usize }
}
impl PageKey for usize {
    fn ordinal(&self) -> usize { *self }
}
const PAGE_SHIFT: usize = 6;
#[derive(Clone, Debug)]
pub(super) struct Page<T>(Vec<T>);
impl<T: AllocationSize> AllocationSize for Page<T> {
    fn heap_bytes(&self) -> usize {
        16 + self.0.capacity() * size_of::<T>()
    }
}
#[derive(Clone, Debug)]
pub(super) struct SharedSet<T> {
    pages: SharedMap<usize, Page<T>>,
    len: usize,
}
impl<T> Default for SharedSet<T> {
    fn default() -> Self {
        Self { pages: SharedMap::default(), len: 0 }
    }
}
impl<T: PageKey> SharedSet<T> {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn clear(&mut self) {
        self.pages.clear();
        self.len = 0;
    }
    pub fn insert(&mut self, value: T) -> bool {
        if self.contains(&value) {
            return false;
        }
        let page = self.pages.entry(value.ordinal() >> PAGE_SHIFT).or_insert_with(|| Page(Vec::new()));
        let index = page.0.binary_search(&value).unwrap_err();
        page.0.insert(index, value);
        self.len += 1;
        true
    }
    pub fn remove(&mut self, value: &T) -> bool {
        if !self.contains(value) {
            return false;
        }
        let index = value.ordinal() >> PAGE_SHIFT;
        let page = self.pages.get_mut(&index).unwrap();
        page.0.remove(page.0.binary_search(value).unwrap());
        if page.0.is_empty() {
            self.pages.remove(&index);
        }
        self.len -= 1;
        true
    }
    pub fn contains(&self, value: &T) -> bool {
        self.pages.get(&(value.ordinal() >> PAGE_SHIFT)).is_some_and(|page| page.0.binary_search(value).is_ok())
    }
    pub fn iter(&self) -> impl Iterator<Item = &T> + Clone {
        self.pages.values().flat_map(|page| page.0.iter())
    }
    pub fn is_subset(&self, other: &Self) -> bool {
        self.iter().all(|value| other.contains(value))
    }
    pub fn is_disjoint(&self, other: &Self) -> bool {
        self.iter().all(|value| !other.contains(value))
    }
    pub fn difference<'a>(&'a self, other: &'a Self) -> impl Iterator<Item = &'a T> {
        self.iter().filter(|value| !other.contains(value))
    }
    /// Reuse surviving pages instead of rebuilding a tree from an iterator.
    pub fn rewritten(&self, values: impl IntoIterator<Item = T>) -> Self {
        let desired: std::collections::BTreeSet<_> = values.into_iter().collect();
        let mut next = self.clone();
        let removed: Vec<_> = self.iter().filter(|value| !desired.contains(value)).cloned().collect();
        for value in removed {
            next.remove(&value);
        }
        next.extend(desired);
        next
    }
    pub fn without(&self, other: &Self) -> Self {
        let mut next = self.clone();
        for value in other {
            next.remove(value);
        }
        next
    }
    pub fn union<'a>(&'a self, other: &'a Self) -> impl Iterator<Item = &'a T> {
        let mut left = self.iter().peekable();
        let mut right = other.iter().peekable();
        std::iter::from_fn(move || match (left.peek(), right.peek()) {
            (Some(a), Some(b)) => match a.cmp(b) {
                Ordering::Less => left.next(),
                Ordering::Greater => right.next(),
                Ordering::Equal => {
                    right.next();
                    left.next()
                }
            },
            (Some(_), None) => left.next(),
            (None, Some(_)) => right.next(),
            (None, None) => None,
        })
    }
}
impl<T: PageKey> PartialEq for SharedSet<T> {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len && (self.pages.same_root(&other.pages) || self.iter().eq(other.iter()))
    }
}
impl<T: PageKey> Eq for SharedSet<T> {}
impl<T: PageKey> PartialOrd for SharedSet<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl<T: PageKey> Ord for SharedSet<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.iter().cmp(other.iter())
    }
}
impl<T: PageKey> PartialEq<std::collections::BTreeSet<T>> for SharedSet<T> {
    fn eq(&self, other: &std::collections::BTreeSet<T>) -> bool {
        self.iter().eq(other.iter())
    }
}
impl<T: PageKey> FromIterator<T> for SharedSet<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let mut set = Self::new();
        set.extend(iter);
        set
    }
}
impl<T: PageKey, const N: usize> From<[T; N]> for SharedSet<T> {
    fn from(values: [T; N]) -> Self {
        values.into_iter().collect()
    }
}
impl<T: PageKey> Extend<T> for SharedSet<T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        for value in iter {
            self.insert(value);
        }
    }
}
impl<'a, T: PageKey + 'a> Extend<&'a T> for SharedSet<T> {
    fn extend<I: IntoIterator<Item = &'a T>>(&mut self, iter: I) {
        self.extend(iter.into_iter().cloned());
    }
}
impl<'a, T: PageKey> IntoIterator for &'a SharedSet<T> {
    type Item = &'a T;
    type IntoIter = std::iter::FlatMap<Iter<'a, usize, Page<T>>, std::slice::Iter<'a, T>,
        fn((&'a usize, &'a Page<T>)) -> std::slice::Iter<'a, T>>;
    fn into_iter(self) -> Self::IntoIter {
        self.pages.iter().flat_map(|(_, page)| page.0.iter())
    }
}
impl<T: PageKey> IntoIterator for SharedSet<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter().cloned().collect::<Vec<_>>().into_iter()
    }
}
impl<T: PageKey + AllocationSize> AllocationSize for SharedSet<T> {
    fn heap_bytes(&self) -> usize {
        self.pages.allocation_bytes()
    }
    fn work(&self) -> usize {
        self.len.saturating_add(1)
    }
    fn unshared_bytes(&self) -> usize { 0 }
    fn charge_shared(&self) -> usize { self.pages.charge_new() }
    fn pending_shared(&self) -> usize { self.pages.new_bytes(false) }
    fn temporary_bytes(&self) -> usize {
        super::per_value::tree_bytes::<T>(self.len) + self.len * size_of::<T>() * 2 + 16
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn check_tree<K: Ord, V>(link: &Link<K, V>, low: Option<&K>, high: Option<&K>) -> usize {
        let Some(node) = link else {
            return 0;
        };
        let key = &node.entry.pair.0;
        assert!(low.is_none_or(|low| low < key));
        assert!(high.is_none_or(|high| key < high));
        let left = check_tree(&node.left, low, Some(key));
        let right = check_tree(&node.right, Some(key), high);
        assert!(left.abs_diff(right) <= 1);
        assert_eq!(node.height, 1 + left.max(right));
        node.height
    }

    #[test]
    fn label_pages_share_untouched_ranges_and_compare_by_contents() {
        let labels: SharedSet<_> = (0..256usize).collect();
        labels.charge_shared();
        let mut changed = labels.clone();
        changed.remove(&1);
        changed.insert(300);
        assert!(std::ptr::eq(labels.pages.get(&2).unwrap(), changed.pages.get(&2).unwrap()));
        assert!(!std::ptr::eq(labels.pages.get(&0).unwrap(), changed.pages.get(&0).unwrap()));
        assert!(labels.contains(&1));
        assert!(!labels.contains(&300));
        let reversed: SharedSet<_> = (0..256usize).rev().collect();
        assert_eq!(labels, reversed);
        assert!(!labels.pages.same_root(&reversed.pages));
    }

    #[test]
    fn updates_removals_and_snapshots_match_ordered_map() {
        let mut tree = SharedMap::new();
        let mut dense = BTreeMap::new();
        let mut snapshots = Vec::new();
        let mut seed = 17u32;
        for step in 0..4000 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let key = (seed as usize >> 8) % 127;
            if seed & 3 == 0 {
                assert_eq!(tree.remove(&key), dense.remove(&key));
            } else {
                assert_eq!(tree.insert(key, step), dense.insert(key, step));
            }
            if step % 137 == 0 {
                snapshots.push((tree.clone(), dense.clone()));
            }
            assert!(tree.iter().eq(dense.iter()));
            assert_eq!(tree.len(), dense.len());
            check_tree(&tree.root, None, None);
        }
        for (snapshot, expected) in snapshots {
            assert!(snapshot.iter().eq(expected.iter()));
            check_tree(&snapshot.root, None, None);
        }
        for (_, value) in tree.iter_mut() {
            *value += 1;
        }
        for value in dense.values_mut() {
            *value += 1;
        }
        tree.retain(|key, _| key % 2 == 0);
        dense.retain(|key, _| key % 2 == 0);
        assert!(tree.iter().eq(dense.iter()));
        check_tree(&tree.root, None, None);
    }

    #[test]
    fn equality_is_independent_of_shape_and_allocation() {
        let forward: SharedMap<_, _> = (0..1000).map(|key| (key, key + 1)).collect();
        let backward: SharedMap<_, _> = (0..1000).rev().map(|key| (key, key + 1)).collect();
        assert!(!forward.same_root(&backward));
        assert_eq!(forward, backward);
    }

    #[test]
    fn one_change_allocates_only_a_path_and_nested_sets_are_charged_once() {
        let labels: SharedSet<_> = (0..1024usize).collect();
        let mut original: SharedMap<_, _> = (0..100usize).map(|key| (key, labels.clone())).collect();
        let initial = original.charge_new();
        assert_eq!(original.charge_new(), 0);
        let mut changed = original.clone();
        assert_eq!(changed.charge_new(), 0);
        changed.get_mut(&50).unwrap().insert(2048);
        let extra = changed.charge_new();
        assert!(extra > 0);
        assert!(extra < initial / 10, "{extra} / {initial}");
        assert_eq!(changed.charge_new(), 0);
        assert!(!original.get(&50).unwrap().contains(&2048));
        assert!(changed.get(&50).unwrap().contains(&2048));
        original.clear();
        assert!(changed.get(&49).unwrap().contains(&1023));
    }
}
