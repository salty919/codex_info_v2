//! Persistent canonical radix Merkle map. Root lookup never walks the cache.
use super::{Digest, WireError};
use std::sync::Arc;
#[derive(Clone, Debug, Default)]
pub struct Tree {
    root: Option<Arc<Node>>,
    pub visits: usize,
}
#[derive(Debug)]
enum Node {
    Leaf {
        key: Vec<u8>,
        value: Digest,
        hash: Digest,
    },
    Branch {
        bit: usize,
        left: Arc<Node>,
        right: Arc<Node>,
        hash: Digest,
    },
}
impl Node {
    fn hash(&self) -> Digest {
        match self {
            Self::Leaf { hash, .. } | Self::Branch { hash, .. } => *hash,
        }
    }
    fn key(&self) -> &[u8] {
        match self {
            Self::Leaf { key, .. } => key,
            Self::Branch { left, .. } => left.key(),
        }
    }
    fn leaf(key: Vec<u8>, value: Digest) -> Arc<Self> {
        let mut bytes = vec![0];
        bytes.extend((key.len() as u64).to_be_bytes());
        bytes.extend(&key);
        bytes.extend(value);
        Arc::new(Self::Leaf {
            key,
            value,
            hash: super::hash(bytes),
        })
    }
    fn branch(bit: usize, left: Arc<Self>, right: Arc<Self>) -> Arc<Self> {
        let mut bytes = vec![1];
        bytes.extend(left.hash());
        bytes.extend(right.hash());
        Arc::new(Self::Branch {
            bit,
            left,
            right,
            hash: super::hash(bytes),
        })
    }
}
fn bit(key: &[u8], index: usize) -> bool {
    key[index / 8] & (0x80 >> (index % 8)) != 0
}
fn difference(left: &[u8], right: &[u8]) -> Result<Option<usize>, WireError> {
    for (i, (&l, &r)) in left.iter().zip(right).enumerate() {
        let xor = l ^ r;
        if xor != 0 {
            return Ok(Some(i * 8 + xor.leading_zeros() as usize));
        }
    }
    if left.len() != right.len() {
        return Err(WireError("manifest key is a proper prefix"));
    }
    Ok(None)
}
impl Tree {
    pub fn root(&self) -> Digest {
        self.root
            .as_ref()
            .map_or_else(|| super::hash([2]), |n| n.hash())
    }
    pub fn get(&self, key: &[u8]) -> Option<Digest> {
        let mut node = self.root.as_deref()?;
        loop {
            match node {
                Node::Leaf {
                    key: old, value, ..
                } => return (old == key).then_some(*value),
                Node::Branch {
                    bit: index,
                    left,
                    right,
                    ..
                } => {
                    if *index >= key.len() * 8 {
                        return None;
                    }
                    node = if bit(key, *index) { right } else { left };
                }
            }
        }
    }
    pub fn insert(&mut self, key: Vec<u8>, value: Digest) -> Result<(), WireError> {
        self.visits = 0;
        let Some(root) = &self.root else {
            self.root = Some(Node::leaf(key, value));
            self.visits = 1;
            return Ok(());
        };
        fn add(
            node: &Arc<Node>,
            key: &[u8],
            value: Digest,
            visits: &mut usize,
        ) -> Result<Arc<Node>, WireError> {
            *visits += 1;
            let split = difference(node.key(), key)?;
            if let Some(split) = split {
                if !matches!(node.as_ref(),Node::Branch{bit,..}if *bit<=split) {
                    let leaf = Node::leaf(key.to_vec(), value);
                    return Ok(if bit(key, split) {
                        Node::branch(split, Arc::clone(node), leaf)
                    } else {
                        Node::branch(split, leaf, Arc::clone(node))
                    });
                }
            }
            Ok(match node.as_ref() {
                Node::Leaf {
                    key: old,
                    value: previous,
                    ..
                } => {
                    if *previous == value {
                        Arc::clone(node)
                    } else {
                        Node::leaf(old.clone(), value)
                    }
                }
                Node::Branch {
                    bit: index,
                    left,
                    right,
                    ..
                } => {
                    if bit(key, *index) {
                        Node::branch(*index, Arc::clone(left), add(right, key, value, visits)?)
                    } else {
                        Node::branch(*index, add(left, key, value, visits)?, Arc::clone(right))
                    }
                }
            })
        }
        self.root = Some(add(root, &key, value, &mut self.visits)?);
        Ok(())
    }
    pub fn remove(&mut self, key: &[u8]) -> Result<(), WireError> {
        fn remove(
            node: &Arc<Node>,
            key: &[u8],
            visits: &mut usize,
        ) -> Result<Option<Arc<Node>>, WireError> {
            *visits += 1;
            match node.as_ref() {
                Node::Leaf { key: old, .. } => {
                    if old == key {
                        Ok(None)
                    } else {
                        Err(WireError("missing manifest key"))
                    }
                }
                Node::Branch {
                    bit: index,
                    left,
                    right,
                    ..
                } => {
                    if *index >= key.len() * 8 {
                        return Err(WireError("missing manifest key"));
                    }
                    if bit(key, *index) {
                        Ok(Some(match remove(right, key, visits)? {
                            Some(right) => Node::branch(*index, Arc::clone(left), right),
                            None => Arc::clone(left),
                        }))
                    } else {
                        Ok(Some(match remove(left, key, visits)? {
                            Some(left) => Node::branch(*index, left, Arc::clone(right)),
                            None => Arc::clone(right),
                        }))
                    }
                }
            }
        }
        self.visits = 0;
        self.root = remove(
            self.root
                .as_ref()
                .ok_or(WireError("missing manifest key"))?,
            key,
            &mut self.visits,
        )?;
        Ok(())
    }
}
impl Tree {
    /// Atomically splice one canonical binding namespace without walking its rows.
    pub fn replace_prefix(&mut self, prefix: &[u8], replacement: &Self) -> Result<(), WireError> {
        fn splice(
            old: Option<&Arc<Node>>,
            prefix: &[u8],
            new: Option<&Arc<Node>>,
            visits: &mut usize,
        ) -> Result<Option<Arc<Node>>, WireError> {
            *visits += 1;
            let Some(old) = old else {
                return Ok(new.cloned());
            };
            let boundary = prefix.len() * 8;
            let shared = old
                .key()
                .iter()
                .zip(prefix)
                .enumerate()
                .find_map(|(i, (&a, &b))| {
                    let x = a ^ b;
                    (x != 0).then(|| i * 8 + x.leading_zeros() as usize)
                });
            let index = if let Node::Branch { bit, .. } = old.as_ref() {
                *bit
            } else {
                usize::MAX
            };
            if shared.is_none_or(|split| split >= index) && index < boundary {
                let Node::Branch {
                    bit: index,
                    left,
                    right,
                    ..
                } = old.as_ref()
                else {
                    unreachable!()
                };
                return if bit(prefix, *index) {
                    Ok(Some(match splice(Some(right), prefix, new, visits)? {
                        Some(right) => Node::branch(*index, left.clone(), right),
                        None => left.clone(),
                    }))
                } else {
                    Ok(Some(match splice(Some(left), prefix, new, visits)? {
                        Some(left) => Node::branch(*index, left, right.clone()),
                        None => right.clone(),
                    }))
                };
            }
            if old.key().starts_with(prefix) {
                return Ok(new.cloned());
            }
            let Some(new) = new else {
                return Ok(Some(old.clone()));
            };
            let split =
                difference(old.key(), new.key())?.ok_or(WireError("namespace collision"))?;
            Ok(Some(if bit(new.key(), split) {
                Node::branch(split, old.clone(), new.clone())
            } else {
                Node::branch(split, new.clone(), old.clone())
            }))
        }
        self.visits = 0;
        self.root = splice(
            self.root.as_ref(),
            prefix,
            replacement.root.as_ref(),
            &mut self.visits,
        )?;
        Ok(())
    }
}
/// Lazy changed-key walk: equal subtree hashes are never expanded.
type NodePair = (Option<Arc<Node>>, Option<Arc<Node>>);
#[derive(Clone, Debug)]
pub struct Differences {
    stack: Vec<NodePair>,
}
impl Tree {
    pub fn differences(&self, previous: &Self) -> Differences {
        Differences {
            stack: vec![(self.root.clone(), previous.root.clone())],
        }
    }
}
impl Iterator for Differences {
    type Item = (Vec<u8>, Option<Digest>);
    fn next(&mut self) -> Option<Self::Item> {
        while let Some((new, old)) = self.stack.pop() {
            match (new, old) {
                (None, None) => {}
                (Some(new), Some(old)) => {
                    if new.hash() == old.hash() {
                        continue;
                    }
                    if let (Node::Leaf { key, value, .. }, Node::Leaf { key: old_key, .. }) =
                        (new.as_ref(), old.as_ref())
                    {
                        if key == old_key {
                            return Some((key.clone(), Some(*value)));
                        }
                    }
                    let index = |node: &Node| {
                        if let Node::Branch { bit, .. } = node {
                            *bit
                        } else {
                            usize::MAX
                        }
                    };
                    let split = index(&new).min(index(&old)).min(
                        difference(new.key(), old.key())
                            .ok()
                            .flatten()
                            .unwrap_or(usize::MAX),
                    );
                    fn parts(
                        node: Arc<Node>,
                        split: usize,
                    ) -> (Option<Arc<Node>>, Option<Arc<Node>>) {
                        if let Node::Branch {
                            bit: index,
                            left,
                            right,
                            ..
                        } = node.as_ref()
                        {
                            if *index == split {
                                return (Some(left.clone()), Some(right.clone()));
                            }
                        }
                        if bit(node.key(), split) {
                            (None, Some(node))
                        } else {
                            (Some(node), None)
                        }
                    }
                    let (nl, nr) = parts(new, split);
                    let (ol, or) = parts(old, split);
                    self.stack.push((nr, or));
                    self.stack.push((nl, ol));
                }
                (Some(node), None) => match node.as_ref() {
                    Node::Leaf { key, value, .. } => return Some((key.clone(), Some(*value))),
                    Node::Branch { left, right, .. } => {
                        self.stack.push((Some(right.clone()), None));
                        self.stack.push((Some(left.clone()), None));
                    }
                },
                (None, Some(node)) => match node.as_ref() {
                    Node::Leaf { key, .. } => return Some((key.clone(), None)),
                    Node::Branch { left, right, .. } => {
                        self.stack.push((None, Some(right.clone())));
                        self.stack.push((None, Some(left.clone())));
                    }
                },
            }
        }
        None
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn large_received_cache_small_change_only_visits_its_key_path() {
        let mut tree = Tree::default();
        let mut entries = Vec::new();
        for i in 0..10000u64 {
            let key = super::super::canonical_json(None, &serde_json::json!(["alias", i])).unwrap();
            let digest = super::super::hash(i.to_be_bytes());
            tree.insert(key.clone(), digest).unwrap();
            entries.push((key, digest));
        }
        let old = tree.root();
        let key = entries[4321].0.clone();
        let new = super::super::hash(b"changed");
        tree.insert(key, new).unwrap();
        assert!(
            tree.visits < 128,
            "one binding update cannot walk all 10000 aliases"
        );
        let visits = tree.visits;
        for _ in 0..10000 {
            assert_ne!(tree.root(), old);
        }
        assert_eq!(tree.visits, visits);
        entries[4321].1 = new;
        assert_eq!(tree.root(), super::super::manifest_root(&entries).unwrap());
    }
}
