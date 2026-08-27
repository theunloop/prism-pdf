//! PDF array objects (ISO 32000 §7.3.6).

use core::fmt;
use core::ops::Deref;
use std::sync::Arc;

use crate::Object;

/// A PDF array: an ordered, heterogeneous sequence of [`Object`]s.
///
/// Backed by `Arc<Vec<Object>>`, so cloning is O(1) (ADR-0002). Read access is via [`Deref`] to
/// `[Object]`; mutation copies on write through [`Arc::make_mut`]. As with all COS containers,
/// elements that are [`Object::Reference`]s are stored as-is and never resolved here.
#[derive(Clone, PartialEq, Default)]
pub struct Array(Arc<Vec<Object>>);

impl Array {
    /// Creates an empty array.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an array from a vector of objects.
    #[must_use]
    pub fn from_vec(items: Vec<Object>) -> Self {
        Self(Arc::new(items))
    }

    /// Appends an object, cloning the backing storage only if it is shared (copy-on-write).
    pub fn push(&mut self, object: Object) {
        Arc::make_mut(&mut self.0).push(object);
    }
}

impl Deref for Array {
    type Target = [Object];

    fn deref(&self) -> &[Object] {
        &self.0
    }
}

impl FromIterator<Object> for Array {
    fn from_iter<I: IntoIterator<Item = Object>>(iter: I) -> Self {
        Self(Arc::new(iter.into_iter().collect()))
    }
}

impl From<Vec<Object>> for Array {
    fn from(items: Vec<Object>) -> Self {
        Self::from_vec(items)
    }
}

impl fmt::Debug for Array {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.0.iter()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_is_copy_on_write() {
        let mut a = Array::from_vec(vec![Object::Integer(1)]);
        let shared = a.clone();
        a.push(Object::Integer(2));
        // The earlier clone is untouched; only `a` grew.
        assert_eq!(shared.len(), 1);
        assert_eq!(a.len(), 2);
    }
}
