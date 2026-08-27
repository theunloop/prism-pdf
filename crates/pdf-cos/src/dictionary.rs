//! PDF dictionary objects (ISO 32000 §7.3.7).

use core::fmt;
use std::sync::Arc;

use indexmap::IndexMap;

use crate::{Array, Name, Object, ObjectId, Stream};

/// A PDF dictionary: a map of [`Name`] keys to [`Object`] values.
///
/// Backed by `Arc<IndexMap<Name, Object>>`: cloning is O(1) (ADR-0002), **insertion order is
/// preserved** for readable output and round-trip debugging, yet equality is **order-independent**
/// (ADR-0003). Mutation copies on write through [`Arc::make_mut`].
///
/// Accessors do **not** resolve references (ADR-0001): if a value is an [`Object::Reference`] it
/// is returned (or rejected by the typed getters) as-is — resolving it is the `Document`'s job.
#[derive(Clone, PartialEq, Default)]
pub struct Dictionary(Arc<IndexMap<Name, Object>>);

impl Dictionary {
    /// Creates an empty dictionary.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the dictionary has no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether `key` is present.
    #[must_use]
    pub fn contains_key(&self, key: &Name) -> bool {
        self.0.contains_key(key)
    }

    /// The value for `key`, if present. Not resolved: may be an [`Object::Reference`].
    #[must_use]
    pub fn get(&self, key: &Name) -> Option<&Object> {
        self.0.get(key)
    }

    /// Inserts `value` under `key`, returning the previous value if any (copy-on-write).
    pub fn insert(&mut self, key: Name, value: Object) -> Option<Object> {
        Arc::make_mut(&mut self.0).insert(key, value)
    }

    /// Removes `key`, preserving the order of the remaining entries (copy-on-write).
    pub fn remove(&mut self, key: &Name) -> Option<Object> {
        Arc::make_mut(&mut self.0).shift_remove(key)
    }

    /// Iterates over `(key, value)` pairs in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&Name, &Object)> {
        self.0.iter()
    }

    /// Iterates over the keys in insertion order.
    pub fn keys(&self) -> impl Iterator<Item = &Name> {
        self.0.keys()
    }

    // --- Typed accessors (DESIGN.md §EPIC 1). None of these resolve references. ---

    /// The value for `key` if it is a direct integer.
    #[must_use]
    pub fn get_integer(&self, key: &Name) -> Option<i64> {
        self.get(key).and_then(Object::as_integer)
    }

    /// The value for `key` if it is a direct name.
    #[must_use]
    pub fn get_name(&self, key: &Name) -> Option<&Name> {
        self.get(key).and_then(Object::as_name)
    }

    /// The value for `key` if it is a direct array.
    #[must_use]
    pub fn get_array(&self, key: &Name) -> Option<&Array> {
        self.get(key).and_then(Object::as_array)
    }

    /// The value for `key` if it is a direct dictionary.
    #[must_use]
    pub fn get_dict(&self, key: &Name) -> Option<&Dictionary> {
        self.get(key).and_then(Object::as_dict)
    }

    /// The value for `key` if it is a direct stream.
    #[must_use]
    pub fn get_stream(&self, key: &Name) -> Option<&Stream> {
        self.get(key).and_then(Object::as_stream)
    }

    /// The value for `key` if it is an indirect reference.
    #[must_use]
    pub fn get_reference(&self, key: &Name) -> Option<ObjectId> {
        self.get(key).and_then(Object::as_reference)
    }
}

impl FromIterator<(Name, Object)> for Dictionary {
    fn from_iter<I: IntoIterator<Item = (Name, Object)>>(iter: I) -> Self {
        Self(Arc::new(iter.into_iter().collect()))
    }
}

impl fmt::Debug for Dictionary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.0.iter()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(s: &'static str) -> Name {
        Name::from_static(s)
    }

    #[test]
    fn equality_is_order_independent_but_order_is_preserved() {
        // §7.3.7: key order is not semantically significant, so `==` ignores it (ADR-0003)...
        let mut a = Dictionary::new();
        a.insert(n("X"), Object::Integer(1));
        a.insert(n("Y"), Object::Integer(2));

        let mut b = Dictionary::new();
        b.insert(n("Y"), Object::Integer(2));
        b.insert(n("X"), Object::Integer(1));

        assert_eq!(a, b);

        // ...but insertion order is still observable via iteration.
        let keys: Vec<&Name> = a.keys().collect();
        assert_eq!(keys, vec![&n("X"), &n("Y")]);
    }

    #[test]
    fn accessors_do_not_resolve_references() {
        // §7.3.10 + ADR-0001: a Reference value is returned as-is, never dereferenced.
        let mut d = Dictionary::new();
        d.insert(n("Kids"), Object::Reference(ObjectId::new(3, 0)));

        assert_eq!(
            d.get(&n("Kids")),
            Some(&Object::Reference(ObjectId::new(3, 0)))
        );
        assert_eq!(d.get_reference(&n("Kids")), Some(ObjectId::new(3, 0)));
        // It is a reference, not a dictionary, so the typed getter declines it.
        assert!(d.get_dict(&n("Kids")).is_none());
    }

    #[test]
    fn insert_is_copy_on_write() {
        let mut d = Dictionary::new();
        d.insert(n("A"), Object::Integer(1));
        let shared = d.clone();
        d.insert(n("B"), Object::Integer(2));
        assert_eq!(shared.len(), 1);
        assert_eq!(d.len(), 2);
    }
}
