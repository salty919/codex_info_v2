//! Persistent AVL map used for bounded receipt staging. Cloning shares nodes.
use serde::ser::{Serialize, SerializeMap, Serializer};
use std::{
    borrow::Borrow,
    cmp::Ordering,
    ops::Index,
    sync::{
        atomic::{AtomicU64, Ordering as AtomicOrdering},
        Arc,
    },
};
#[derive(Debug)]
struct Node<K, V> {
    key: K,
    value: Arc<V>,
    height: u16,
    left: Link<K, V>,
    right: Link<K, V>,
}
type Link<K, V> = Option<Arc<Node<K, V>>>;
#[derive(Debug)]
pub struct Map<K, V> {
    root: Link<K, V>,
    len: usize,
    walks: Arc<AtomicU64>,
}
impl<K, V> Clone for Map<K, V> {
    fn clone(&self) -> Self {
        Self {
            root: self.root.clone(),
            len: self.len,
            walks: Arc::clone(&self.walks),
        }
    }
}
impl<K, V> Default for Map<K, V> {
    fn default() -> Self {
        Self {
            root: None,
            len: 0,
            walks: Arc::new(AtomicU64::new(0)),
        }
    }
}
impl<K: Clone, V> Clone for Node<K, V> {
    fn clone(&self) -> Self {
        Self {
            key: self.key.clone(),
            value: Arc::clone(&self.value),
            height: self.height,
            left: self.left.clone(),
            right: self.right.clone(),
        }
    }
}
fn height<K, V>(node: &Link<K, V>) -> u16 {
    node.as_ref().map_or(0, |node| node.height)
}
fn node<K, V>(key: K, value: Arc<V>, left: Link<K, V>, right: Link<K, V>) -> Arc<Node<K, V>> {
    Arc::new(Node {
        key,
        value,
        height: 1 + height(&left).max(height(&right)),
        left,
        right,
    })
}
fn balance<K: Clone, V>(
    key: K,
    value: Arc<V>,
    left: Link<K, V>,
    right: Link<K, V>,
) -> Arc<Node<K, V>> {
    if height(&left) > height(&right) + 1 {
        let l = left.as_ref().unwrap();
        if height(&l.left) >= height(&l.right) {
            return node(
                l.key.clone(),
                Arc::clone(&l.value),
                l.left.clone(),
                Some(node(key, value, l.right.clone(), right)),
            );
        }
        let m = l.right.as_ref().unwrap();
        return node(
            m.key.clone(),
            Arc::clone(&m.value),
            Some(node(
                l.key.clone(),
                Arc::clone(&l.value),
                l.left.clone(),
                m.left.clone(),
            )),
            Some(node(key, value, m.right.clone(), right)),
        );
    }
    if height(&right) > height(&left) + 1 {
        let r = right.as_ref().unwrap();
        if height(&r.right) >= height(&r.left) {
            return node(
                r.key.clone(),
                Arc::clone(&r.value),
                Some(node(key, value, left, r.left.clone())),
                r.right.clone(),
            );
        }
        let m = r.left.as_ref().unwrap();
        return node(
            m.key.clone(),
            Arc::clone(&m.value),
            Some(node(key, value, left, m.left.clone())),
            Some(node(
                r.key.clone(),
                Arc::clone(&r.value),
                m.right.clone(),
                r.right.clone(),
            )),
        );
    }
    node(key, value, left, right)
}
impl<K: Ord + Clone, V> Map<K, V> {
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn get<Q: Ord + ?Sized>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
    {
        let mut at = self.root.as_deref();
        while let Some(node) = at {
            match key.cmp(node.key.borrow()) {
                Ordering::Less => at = node.left.as_deref(),
                Ordering::Greater => at = node.right.as_deref(),
                Ordering::Equal => return Some(&node.value),
            }
        }
        None
    }
    pub fn contains_key<Q: Ord + ?Sized>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
    {
        self.get(key).is_some()
    }
    pub fn insert(&mut self, key: K, value: V) -> Option<V>
    where
        V: Clone,
    {
        fn put<K: Ord + Clone, V>(
            root: &Link<K, V>,
            key: K,
            value: Arc<V>,
        ) -> (Arc<Node<K, V>>, Option<Arc<V>>) {
            let Some(old) = root else {
                return (node(key, value, None, None), None);
            };
            match key.cmp(&old.key) {
                Ordering::Less => {
                    let (left, previous) = put(&old.left, key, value);
                    (
                        balance(
                            old.key.clone(),
                            Arc::clone(&old.value),
                            Some(left),
                            old.right.clone(),
                        ),
                        previous,
                    )
                }
                Ordering::Greater => {
                    let (right, previous) = put(&old.right, key, value);
                    (
                        balance(
                            old.key.clone(),
                            Arc::clone(&old.value),
                            old.left.clone(),
                            Some(right),
                        ),
                        previous,
                    )
                }
                Ordering::Equal => (
                    node(key, value, old.left.clone(), old.right.clone()),
                    Some(Arc::clone(&old.value)),
                ),
            }
        }
        let (root, previous) = put(&self.root, key, Arc::new(value));
        self.root = Some(root);
        if previous.is_none() {
            self.len += 1
        }
        previous.map(|old| Arc::try_unwrap(old).unwrap_or_else(|old| (*old).clone()))
    }
    pub fn remove<Q: Ord + ?Sized>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        V: Clone,
    {
        fn pop_min<K: Ord + Clone, V>(root: &Arc<Node<K, V>>) -> (Link<K, V>, K, Arc<V>) {
            if let Some(left) = &root.left {
                let (left, key, value) = pop_min(left);
                (
                    Some(balance(
                        root.key.clone(),
                        Arc::clone(&root.value),
                        left,
                        root.right.clone(),
                    )),
                    key,
                    value,
                )
            } else {
                (
                    root.right.clone(),
                    root.key.clone(),
                    Arc::clone(&root.value),
                )
            }
        }
        fn take<K: Ord + Clone + Borrow<Q>, V, Q: Ord + ?Sized>(
            root: &Link<K, V>,
            key: &Q,
        ) -> (Link<K, V>, Option<Arc<V>>) {
            let Some(old) = root else { return (None, None) };
            match key.cmp(old.key.borrow()) {
                Ordering::Less => {
                    let (left, value) = take(&old.left, key);
                    (
                        Some(balance(
                            old.key.clone(),
                            Arc::clone(&old.value),
                            left,
                            old.right.clone(),
                        )),
                        value,
                    )
                }
                Ordering::Greater => {
                    let (right, value) = take(&old.right, key);
                    (
                        Some(balance(
                            old.key.clone(),
                            Arc::clone(&old.value),
                            old.left.clone(),
                            right,
                        )),
                        value,
                    )
                }
                Ordering::Equal => {
                    let next = match (&old.left, &old.right) {
                        (None, _) => old.right.clone(),
                        (_, None) => old.left.clone(),
                        (_, Some(right)) => {
                            let (right, key, value) = pop_min(right);
                            Some(balance(key, value, old.left.clone(), right))
                        }
                    };
                    (next, Some(Arc::clone(&old.value)))
                }
            }
        }
        let (root, old) = take(&self.root, key);
        self.root = root;
        if old.is_some() {
            self.len -= 1
        }
        old.map(|old| Arc::try_unwrap(old).unwrap_or_else(|old| (*old).clone()))
    }
    pub fn get_mut<Q: Ord + ?Sized>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
        V: Clone,
    {
        fn find<'a, K: Ord + Clone + Borrow<Q>, V: Clone, Q: Ord + ?Sized>(
            root: &'a mut Link<K, V>,
            key: &Q,
        ) -> Option<&'a mut V> {
            let node = Arc::make_mut(root.as_mut()?);
            match key.cmp(node.key.borrow()) {
                Ordering::Less => find(&mut node.left, key),
                Ordering::Greater => find(&mut node.right, key),
                Ordering::Equal => Some(Arc::make_mut(&mut node.value)),
            }
        }
        find(&mut self.root, key)
    }
    pub fn entry(&mut self, key: K) -> Entry<'_, K, V> {
        Entry { map: self, key }
    }
    pub fn iter(&self) -> Iter<'_, K, V> {
        self.walks.fetch_add(1, AtomicOrdering::Relaxed);
        Iter::new(self.root.as_deref())
    }
    pub fn keys(&self) -> impl DoubleEndedIterator<Item = &K> {
        self.iter().map(|(key, _)| key)
    }
    pub fn values(&self) -> impl DoubleEndedIterator<Item = &V> {
        self.iter().map(|(_, value)| value)
    }
    pub fn full_walks(&self) -> u64 {
        self.walks.load(AtomicOrdering::Relaxed)
    }
    pub fn retain(&mut self, mut keep: impl FnMut(&K, &V) -> bool)
    where
        V: Clone,
    {
        let remove = self
            .iter()
            .filter(|(k, v)| !keep(k, v))
            .map(|(k, _)| k.clone())
            .collect::<Vec<_>>();
        for key in remove {
            self.remove(&key);
        }
    }
    pub fn clear(&mut self) {
        self.root = None;
        self.len = 0
    }
}
pub struct Entry<'a, K, V> {
    map: &'a mut Map<K, V>,
    key: K,
}
impl<'a, K: Ord + Clone, V: Clone> Entry<'a, K, V> {
    pub fn or_insert_with(self, create: impl FnOnce() -> V) -> &'a mut V {
        if !self.map.contains_key(&self.key) {
            self.map.insert(self.key.clone(), create());
        }
        self.map.get_mut(&self.key).unwrap()
    }
    pub fn or_default(self) -> &'a mut V
    where
        V: Default,
    {
        self.or_insert_with(V::default)
    }
}
pub struct Iter<'a, K, V> {
    front: Vec<&'a Node<K, V>>,
    back: Vec<&'a Node<K, V>>,
}
impl<'a, K, V> Iter<'a, K, V> {
    fn new(root: Option<&'a Node<K, V>>) -> Self {
        let mut out = Self {
            front: Vec::new(),
            back: Vec::new(),
        };
        let mut at = root;
        while let Some(node) = at {
            out.front.push(node);
            at = node.left.as_deref()
        }
        let mut at = root;
        while let Some(node) = at {
            out.back.push(node);
            at = node.right.as_deref()
        }
        out
    }
}
impl<'a, K: Ord, V> Iterator for Iter<'a, K, V> {
    type Item = (&'a K, &'a V);
    fn next(&mut self) -> Option<Self::Item> {
        let node = self.front.pop()?;
        if self.back.last().is_none_or(|last| node.key > last.key) {
            return None;
        }
        let mut at = node.right.as_deref();
        while let Some(node) = at {
            self.front.push(node);
            at = node.left.as_deref()
        }
        Some((&node.key, &node.value))
    }
}
impl<K: Ord, V> DoubleEndedIterator for Iter<'_, K, V> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let node = self.back.pop()?;
        if self.front.last().is_none_or(|first| node.key < first.key) {
            return None;
        }
        let mut at = node.left.as_deref();
        while let Some(node) = at {
            self.back.push(node);
            at = node.right.as_deref()
        }
        Some((&node.key, &node.value))
    }
}
impl<'a, K: Ord + Clone, V> IntoIterator for &'a Map<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter = Iter<'a, K, V>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
impl<K: Ord + Clone, V: Clone> Extend<(K, V)> for Map<K, V> {
    fn extend<I: IntoIterator<Item = (K, V)>>(&mut self, items: I) {
        for (key, value) in items {
            self.insert(key, value);
        }
    }
}
impl<K: Ord + Clone, V: Clone> FromIterator<(K, V)> for Map<K, V> {
    fn from_iter<T: IntoIterator<Item = (K, V)>>(items: T) -> Self {
        let mut map = Self::default();
        map.extend(items);
        map
    }
}
impl<K: Ord + Clone + Borrow<Q>, V, Q: Ord + ?Sized> Index<&Q> for Map<K, V> {
    type Output = V;
    fn index(&self, key: &Q) -> &V {
        self.get(key).expect("missing persistent key")
    }
}
impl<K: Ord + Clone + Serialize, V: Serialize> Serialize for Map<K, V> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.len))?;
        for (key, value) in self {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}
