//! Insertion-ordered string-keyed map that serializes as a JSON object
//!
//! Both Choice options and the question set go over the wire as JSON objects.
//! A `HashMap` would randomize their order between runs and a `BTreeMap` would
//! sort them alphabetically; either way the caller's ordering is lost. Choice
//! options are read by the model, so their order is part of the request, and a
//! stable question order keeps request bodies byte-identical across runs, which
//! matters when diffing or caching them.

use serde::ser::{Serialize, SerializeMap, Serializer};

/// A small map that keeps keys in the order they were inserted.
#[derive(Debug, Clone, PartialEq)]
pub struct OrderedMap<V> {
    entries: Vec<(String, V)>,
}

impl<V> Default for OrderedMap<V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<V> OrderedMap<V> {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Insert a key, replacing its value in place if the key already exists.
    pub fn insert(&mut self, key: impl Into<String>, value: V) -> &mut Self {
        let key = key.into();
        match self.entries.iter_mut().find(|(k, _)| *k == key) {
            Some((_, slot)) => *slot = value,
            None => self.entries.push((key, value)),
        }
        self
    }

    pub fn get(&self, key: &str) -> Option<&V> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(k, _)| k.as_str())
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &V)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl<V, K: Into<String>> FromIterator<(K, V)> for OrderedMap<V> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut map = Self::new();
        for (k, v) in iter {
            map.insert(k, v);
        }
        map
    }
}

impl<V: Serialize> Serialize for OrderedMap<V> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.entries.len()))?;
        for (k, v) in &self.entries {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_insertion_order() {
        let map: OrderedMap<u8> = [("zebra", 1u8), ("apple", 2), ("mango", 3)]
            .into_iter()
            .collect();
        assert_eq!(map.keys().collect::<Vec<_>>(), ["zebra", "apple", "mango"]);
        assert_eq!(
            serde_json::to_string(&map).unwrap(),
            r#"{"zebra":1,"apple":2,"mango":3}"#
        );
    }

    #[test]
    fn insert_replaces_in_place() {
        let mut map = OrderedMap::new();
        map.insert("a", 1).insert("b", 2).insert("a", 9);
        assert_eq!(map.keys().collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(map.get("a"), Some(&9));
        assert_eq!(map.len(), 2);
    }
}
