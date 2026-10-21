//! Small shared data structures.

use std::fmt;

use hashbrown::HashTable;

use crate::value::PhpStr;
use crate::value::string::hash_bytes;

/// An insertion-ordered map keyed by byte strings (exact, case-sensitive
/// matching). Used for class members; callers lowercase keys where PHP is
/// case-insensitive (method names).
pub struct NameMap<V> {
    entries: Vec<(PhpStr, V)>,
    index: HashTable<usize>,
}

impl<V> Default for NameMap<V> {
    fn default() -> Self {
        NameMap {
            entries: Vec::new(),
            index: HashTable::new(),
        }
    }
}

impl<V: Clone> Clone for NameMap<V> {
    fn clone(&self) -> Self {
        let mut m = NameMap::default();
        for (k, v) in &self.entries {
            m.insert(k.clone(), v.clone());
        }
        m
    }
}

impl<V> NameMap<V> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn find(&self, key: &[u8]) -> Option<usize> {
        let entries = &self.entries;
        self.index
            .find(hash_bytes(key), |&i| entries[i].0.as_bytes() == key)
            .copied()
    }

    pub fn get(&self, key: &[u8]) -> Option<&V> {
        self.find(key).map(|i| &self.entries[i].1)
    }

    pub fn get_mut(&mut self, key: &[u8]) -> Option<&mut V> {
        self.find(key).map(|i| &mut self.entries[i].1)
    }

    pub fn contains_key(&self, key: &[u8]) -> bool {
        self.find(key).is_some()
    }

    /// Inserts or replaces (keeping the original position). Returns the old value.
    pub fn insert(&mut self, key: PhpStr, value: V) -> Option<V> {
        if let Some(i) = self.find(&key) {
            return Some(std::mem::replace(&mut self.entries[i].1, value));
        }
        let h = hash_bytes(&key);
        let idx = self.entries.len();
        self.entries.push((key, value));
        let entries = &self.entries;
        self.index
            .insert_unique(h, idx, |&i| hash_bytes(&entries[i].0));
        None
    }

    /// Index of `key` in insertion order.
    pub fn index_of(&self, key: &[u8]) -> Option<usize> {
        self.find(key)
    }

    pub fn get_index(&self, i: usize) -> Option<(&PhpStr, &V)> {
        self.entries.get(i).map(|(k, v)| (k, v))
    }

    pub fn iter(&self) -> impl Iterator<Item = (&PhpStr, &V)> {
        self.entries.iter().map(|(k, v)| (k, v))
    }

    pub fn keys(&self) -> impl Iterator<Item = &PhpStr> {
        self.entries.iter().map(|(k, _)| k)
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.entries.iter().map(|(_, v)| v)
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut V> {
        self.entries.iter_mut().map(|(_, v)| v)
    }
}

impl<V: fmt::Debug> fmt::Debug for NameMap<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

/// Lowercases ASCII letters (`zend_str_tolower`), as used for function,
/// class and method lookup.
pub fn lower(name: &[u8]) -> PhpStr {
    PhpStr::from_vec(name.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_map_keeps_order_and_replaces_in_place() {
        let mut m = NameMap::new();
        m.insert(PhpStr::from("b"), 1);
        m.insert(PhpStr::from("a"), 2);
        m.insert(PhpStr::from("b"), 3);
        let keys: Vec<_> = m.keys().map(|k| k.to_string()).collect();
        assert_eq!(keys, ["b", "a"]);
        assert_eq!(m.get(b"b"), Some(&3));
        assert_eq!(m.get(b"B"), None);
        assert_eq!(m.index_of(b"a"), Some(1));
    }
}
