//! Persistent ordered AVL relations. Cloning a root is constant time; changing
//! one key copies only its search path and the changed entry. Iteration and
//! equality use key order, independently of tree shape and allocation identity.
use super::per_value::{AllocationSize, OriginBounds};
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
    origins: Cell<Option<OriginBounds>>,
    charged: Cell<usize>,
}
impl<K: Clone, V: Clone> Clone for Entry<K, V> {
    fn clone(&self) -> Self {
        Self { pair: self.pair.clone(), origins: Cell::new(self.origins.get()), charged: Cell::new(0) }
    }
}
#[derive(Debug)]
struct Node<K, V> {
    entry: Rc<Entry<K, V>>,
    left: Link<K, V>,
    right: Link<K, V>,
    height: usize,
    origins: Cell<Option<OriginBounds>>,
    charged: Cell<bool>,
}
impl<K, V> Clone for Node<K, V> {
    fn clone(&self) -> Self {
        Self {
            entry: self.entry.clone(), left: self.left.clone(), right: self.right.clone(),
            height: self.height, origins: Cell::new(self.origins.get()), charged: Cell::new(false),
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
impl<K: Ord + Clone + AllocationSize, V: Clone + AllocationSize> SharedMap<K, V> {
    pub fn origin_bounds(&self, charge: &mut impl FnMut(usize) -> bool) -> OriginBounds {
        fn ranges<K: AllocationSize, V: AllocationSize>(
            link: &Link<K, V>, charge: &mut impl FnMut(usize) -> bool,
        ) -> OriginBounds {
            let Some(node) = link else {
                charge(1);
                return OriginBounds::EMPTY;
            };
            if !charge(1) {
                return OriginBounds([u32::MAX; 5]);
            }
            if let Some(cached) = node.origins.get() {
                return cached;
            }
            if !charge(3) {
                return OriginBounds([u32::MAX; 5]);
            }
            let mut result = ranges(&node.left, charge);
            result.include(ranges(&node.right, charge));
            let entry = &node.entry;
            let own = if let Some(cached) = entry.origins.get() {
                cached
            } else {
                if !charge(16usize.saturating_add(entry.pair.0.work()).saturating_add(entry.pair.1.work())) {
                    return OriginBounds([u32::MAX; 5]);
                }
                let mut own = entry.pair.0.origin_bounds();
                own.include(entry.pair.1.origin_bounds());
                entry.origins.set(Some(own));
                own
            };
            result.include(own);
            node.origins.set(Some(result));
            result
        }
        ranges(&self.root, charge)
    }
}
impl<K: Ord + Clone, V: Clone> SharedMap<K, V> {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn root_id(&self) -> usize {
        self.root.as_ref().map_or(0, |root| Rc::as_ptr(root) as usize)
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
        node.origins.set(None);
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
            node.origins.set(None);
            match key.cmp(&node.entry.pair.0) {
                Ordering::Less => link = &mut node.left,
                Ordering::Greater => link = &mut node.right,
                Ordering::Equal => {
                    let entry = Rc::make_mut(&mut node.entry);
                    entry.origins.set(None);
                    return Some(&mut entry.pair.1);
                }
            }
        }
        None
    }
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        fn put<K: Ord + Clone, V: Clone>(link: &mut Link<K, V>, key: K, value: V) -> Option<V> {
            let Some(node) = link else {
                *link = Some(Rc::new(Node {
                    entry: Rc::new(Entry { pair: (key, value), origins: Cell::new(None), charged: Cell::new(0) }),
                    left: None, right: None, height: 1, origins: Cell::new(None), charged: Cell::new(false),
                }));
                return None;
            };
            let node = Rc::make_mut(node);
            node.origins.set(None);
            let old = match key.cmp(&node.entry.pair.0) {
                Ordering::Less => put(&mut node.left, key, value),
                Ordering::Greater => put(&mut node.right, key, value),
                Ordering::Equal => {
                    let entry = Rc::make_mut(&mut node.entry);
                    entry.origins.set(None);
                    return Some(std::mem::replace(&mut entry.pair.1, value));
                }
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
                node.origins.set(None);
                node.entry.origins.set(None);
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
    /// Replace existing sorted keys together, copying each affected path once.
    pub fn replace_values(&mut self, values: Vec<(K, V)>) -> usize {
        fn apply<K: Ord + Clone, V: Clone>(
            link: &mut Link<K, V>, pending: &mut std::iter::Peekable<std::vec::IntoIter<(K, V)>>,
            upper: Option<&K>,
        ) -> usize {
            let Some((key, _)) = pending.peek() else {
                return 1;
            };
            if upper.is_some_and(|upper| key >= upper) {
                return 1;
            }
            let node = Rc::make_mut(link.as_mut().expect("replacement keys must exist"));
            node.origins.set(None);
            let key = node.entry.pair.0.clone();
            let mut work = 1 + apply(&mut node.left, pending, Some(&key));
            if pending.peek().is_some_and(|(next, _)| *next == key) {
                let (key, value) = pending.next().unwrap();
                node.entry = Rc::new(Entry {
                    pair: (key, value), origins: Cell::new(None), charged: Cell::new(0),
                });
                work += 1;
            }
            work += apply(&mut node.right, pending, upper);
            work
        }
        if values.is_empty() {
            return 1;
        }
        let mut pending = values.into_iter().peekable();
        let work = apply(&mut self.root, &mut pending, None);
        assert!(pending.next().is_none());
        work
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
    fn origin_bounds(&self) -> OriginBounds {
        self.origin_bounds(&mut |_| true)
    }
    fn heap_bytes(&self) -> usize {
        self.allocation_bytes()
    }
    fn work(&self) -> usize {
        self.iter().fold(1usize, |work, (key, value)| work.saturating_add(key.work()).saturating_add(value.work()))
    }
}

/// SSA labels have unique ordinals ordered with the keys. Group them into bounded pages;
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
pub(super) struct Page<T>(Vec<T>, u64);
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
    pub fn root_id(&self) -> usize {
        self.pages.root_id()
    }
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
        let page = self.pages.entry(value.ordinal() >> PAGE_SHIFT).or_insert_with(|| Page(Vec::new(), 0));
        let index = page.0.binary_search(&value).unwrap_err();
        page.1 |= 1u64 << (value.ordinal() & 63);
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
        page.1 &= !(1u64 << (value.ordinal() & 63));
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
    pub fn subset_work(&self, other: &Self) -> (bool, usize) {
        fn subset<T: PageKey>(left: &Link<usize, Page<T>>, right: &Link<usize, Page<T>>) -> (bool, usize) {
            let Some(a) = left else {
                return (true, 1);
            };
            let Some(b) = right else {
                return (false, 1);
            };
            if Rc::ptr_eq(a, b) {
                return (true, 1);
            }
            if a.entry.pair.0 == b.entry.pair.0 {
                if a.entry.pair.1.1 & !b.entry.pair.1.1 != 0 {
                    return (false, 2);
                }
                let (included, mut work) = subset(&a.left, &b.left);
                work += 2;
                if !included {
                    return (false, work);
                }
                let (included, tail) = subset(&a.right, &b.right);
                return (included, work + tail);
            }
            // Different tree shapes still compare the same ordered page map.
            let mut pages = Iter { stack: Vec::new() };
            pages.descend(left.as_deref());
            let mut work = 1;
            for (key, page) in pages {
                let mut target = right.as_deref();
                loop {
                    work += 1;
                    let Some(node) = target else {
                        return (false, work);
                    };
                    match key.cmp(&node.entry.pair.0) {
                        Ordering::Less => target = node.left.as_deref(),
                        Ordering::Greater => target = node.right.as_deref(),
                        Ordering::Equal => {
                            if page.1 & !node.entry.pair.1.1 != 0 {
                                return (false, work + 1);
                            }
                            break;
                        }
                    }
                }
            }
            (true, work)
        }
        if self.len > other.len {
            return (false, 1);
        }
        subset(&self.pages.root, &other.pages.root)
    }
    pub fn is_subset(&self, other: &Self) -> bool {
        self.subset_work(other).0
    }
    pub fn is_disjoint(&self, other: &Self) -> bool {
        self.disjoint_work(other).0
    }
    pub fn disjoint_work(&self, other: &Self) -> (bool, usize) {
        if self.is_empty() || other.is_empty() {
            return (true, 1);
        }
        if self.pages.same_root(&other.pages) {
            return (false, 1);
        }
        let mut left = self.pages.iter().peekable();
        let mut right = other.pages.iter().peekable();
        let mut work = 1;
        while let (Some((a, page_a)), Some((b, page_b))) = (left.peek(), right.peek()) {
            work += 1;
            match a.cmp(b) {
                Ordering::Less => {
                    left.next();
                }
                Ordering::Greater => {
                    right.next();
                }
                Ordering::Equal => {
                    work += 1;
                    if page_a.1 & page_b.1 != 0 {
                        return (false, work);
                    }
                    left.next();
                    right.next();
                }
            }
        }
        (true, work)
    }
    pub fn difference<'a>(&'a self, other: &'a Self) -> impl Iterator<Item = &'a T> {
        self.iter().filter(|value| !other.contains(value))
    }
    /// Exact (self minus removed) union added, rewriting only changed pages.
    pub fn difference_union(&self, removed: &Self, added: &Self) -> (Self, usize) {
        if self.pages.same_root(&removed.pages) {
            return (added.clone(), 1);
        }
        if self.is_empty() {
            return (added.clone(), 1);
        }
        if removed.is_empty() && self.pages.same_root(&added.pages) {
            return (self.clone(), 1);
        }
        let mut next = self.clone();
        let mut left = removed.pages.iter().peekable();
        let mut right = added.pages.iter().peekable();
        let mut changed = Vec::new();
        let mut inserted = Vec::new();
        let mut deleted = Vec::new();
        let path = 2 + self.pages.len().max(1).ilog2() as usize;
        let mut work = 1;
        while left.peek().is_some() || right.peek().is_some() {
            let key = left.peek().map_or(usize::MAX, |(key, _)| **key)
                .min(right.peek().map_or(usize::MAX, |(key, _)| **key));
            let remove = if left.peek().is_some_and(|(page, _)| **page == key) {
                left.next().unwrap().1.1
            } else { 0 };
            let add = if right.peek().is_some_and(|(page, _)| **page == key) {
                Some(right.next().unwrap().1)
            } else { None };
            let original = self.pages.get(&key);
            let before = original.map_or(0, |page| page.1);
            let after = (before & !remove) | add.map_or(0, |page| page.1);
            work += path + 3;
            if before == after {
                continue;
            }
            next.len = next.len - before.count_ones() as usize + after.count_ones() as usize;
            if after == 0 {
                deleted.push(key);
                continue;
            }
            let mut a = original.map_or(&[][..], |page| page.0.as_slice()).iter().peekable();
            let mut b = add.map_or(&[][..], |page| page.0.as_slice()).iter().peekable();
            let mut values = Vec::with_capacity(after.count_ones() as usize);
            while a.peek().is_some() || b.peek().is_some() {
                work += 1;
                let value = match (a.peek(), b.peek()) {
                    (Some(x), Some(y)) => match x.cmp(y) {
                        Ordering::Less => a.next().unwrap(),
                        Ordering::Greater => b.next().unwrap(),
                        Ordering::Equal => {
                            b.next();
                            a.next().unwrap()
                        }
                    },
                    (Some(_), None) => a.next().unwrap(),
                    (None, Some(_)) => b.next().unwrap(),
                    (None, None) => unreachable!(),
                };
                if after & (1u64 << (value.ordinal() & 63)) != 0 {
                    values.push(value.clone());
                }
            }
            let page = Page(values, after);
            if original.is_some() {
                changed.push((key, page));
            } else {
                inserted.push((key, page));
            }
        }
        work += next.pages.replace_values(changed);
        for key in deleted {
            work += path;
            next.pages.remove(&key);
        }
        for (key, page) in inserted {
            work += path;
            next.pages.insert(key, page);
        }
        (next, work)
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
        self.len == other.len && (self.pages.same_root(&other.pages)
            || self.pages.iter().map(|(key, page)| (key, page.1))
                .eq(other.pages.iter().map(|(key, page)| (key, page.1))))
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
        if self.pages.same_root(&other.pages) {
            Ordering::Equal
        } else {
            self.iter().cmp(other.iter())
        }
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
    fn shared_id(&self) -> Option<usize> { Some(self.root_id()) }
    fn comparison_work(&self, other: &Self) -> usize {
        if self.len != other.len || self.pages.same_root(&other.pages) {
            1
        } else {
            self.pages.len().saturating_add(other.pages.len()).saturating_add(1)
        }
    }
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
    fn disjoint_merge_matches_ordered_sets_and_stops_at_shared_roots() {
        use std::collections::BTreeSet;
        let labels = [0, 63, 64, 127, 128, 4096];
        for left_mask in 0..64 {
            for right_mask in 0..64 {
                let left: SharedSet<usize> = labels.iter().enumerate()
                    .filter_map(|(i, label)| (left_mask & (1 << i) != 0).then_some(*label)).collect();
                let right: SharedSet<usize> = labels.iter().enumerate()
                    .filter_map(|(i, label)| (right_mask & (1 << i) != 0).then_some(*label)).collect();
                let a: BTreeSet<_> = left.iter().copied().collect();
                let b: BTreeSet<_> = right.iter().copied().collect();
                assert_eq!(left.is_disjoint(&right), a.is_disjoint(&b));
                assert_eq!(left.is_subset(&right), a.is_subset(&b));
                let mut expanded = left.clone();
                expanded.insert(65);
                assert!(left.is_subset(&expanded));
                assert_eq!(left.disjoint_work(&left.clone()), (left.is_empty(), 1));
                let mut removed = left.clone();
                let mut expected = a.clone();
                removed.remove(&63);
                expected.remove(&63);
                assert_eq!(removed.is_disjoint(&right), expected.is_disjoint(&b));
                assert_eq!(removed, expected.iter().copied().collect::<SharedSet<_>>());
                let added: SharedSet<_> = labels.iter().enumerate()
                    .filter_map(|(i, label)| ((left_mask ^ right_mask ^ 42) & (1 << i) != 0)
                        .then_some(*label)).collect();
                let c: BTreeSet<_> = added.iter().copied().collect();
                let reference: BTreeSet<_> = a.difference(&b).copied().chain(c).collect();
                let (rewritten, _) = left.difference_union(&right, &added);
                assert_eq!(rewritten, reference);
                assert_eq!(rewritten, reference.iter().copied().collect::<SharedSet<_>>());
            }
        }
    }

    #[test]
    fn origin_index_survives_updates_rotations_and_shared_versions() {
        use super::super::domain::Origin;
        use crate::InstId;
        use std::collections::BTreeSet;

        let mut map = SharedMap::<usize, BTreeSet<Origin>>::new();
        for index in 0..96 {
            let origin = Origin::Instruction(InstId::from_u32(index as u32), 0);
            map.insert(index, BTreeSet::from([origin]));
        }
        for round in 0..128 {
            let mut expected = OriginBounds::EMPTY;
            for (_, origins) in map.iter() {
                expected.include(origins.origin_bounds());
            }
            assert_eq!(map.origin_bounds(&mut |_| true), expected);
            let mut cached_work = 0;
            assert_eq!(map.origin_bounds(&mut |work| {
                cached_work += work;
                true
            }), expected);
            assert_eq!(cached_work, 1);
            let original = map.clone();
            let key = round % 96;
            match round % 5 {
                0 => {
                    map.insert(key, BTreeSet::from([
                        Origin::ViewInstruction(InstId::from_u32(1000 + round as u32), 1),
                    ]));
                }
                1 => {
                    if let Some(origins) = map.get_mut(&key) {
                        origins.clear();
                    }
                }
                2 => {
                    map.remove(&key);
                }
                3 => {
                    for (key, origins) in map.iter_mut() {
                        if key % 3 == 0 {
                            origins.clear();
                        }
                    }
                }
                _ => {
                    let changed = map.keys().copied().map(|key| {
                        (key, BTreeSet::from([Origin::ViewInvoke(crate::BlockId::from_u32(2000), 2)]))
                    }).collect();
                    map.replace_values(changed);
                }
            }
            // A mutation of a shared version must never invalidate the old proof.
            assert_eq!(original.origin_bounds(&mut |_| true), expected);
        }
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
