//! The COS object model (ISO 32000 §7.3): the [`Object`] enum and its accessors.

use crate::{Array, Dictionary, Name, ObjectId, PdfString, Stream};

/// A PDF object — one of the nine COS value types (§7.3.2–§7.3.9) plus the indirect reference
/// (§7.3.10).
///
/// `Object` is **dumb data** (ADR-0001): an [`Object::Reference`] carries an [`ObjectId`] but
/// cannot resolve itself — that is the `Document`'s job. Cloning any `Object` is O(1) because the
/// heavy arms are reference-counted (ADR-0002).
///
/// Equality is structural and canonical (ADR-0003): it never resolves references (so a
/// `Reference` is never equal to its target), never coerces across numeric types
/// (`Integer(1) != Real(1.0)` — use [`as_f64`](Object::as_f64) for numeric coercion), and treats
/// dictionaries as order-independent.
#[derive(Clone, PartialEq, Default, Debug)]
pub enum Object {
    /// The null object (§7.3.9).
    #[default]
    Null,
    /// A boolean (§7.3.2).
    Boolean(bool),
    /// An integer number (§7.3.3).
    Integer(i64),
    /// A real number (§7.3.3), stored as `f64`; the original lexeme is not retained (ADR-0003).
    Real(f64),
    /// A byte string (§7.3.4).
    String(PdfString),
    /// A name (§7.3.5).
    Name(Name),
    /// An array (§7.3.6).
    Array(Array),
    /// A dictionary (§7.3.7).
    Dictionary(Dictionary),
    /// A stream (§7.3.8).
    Stream(Stream),
    /// An indirect reference, `n g R` (§7.3.10).
    Reference(ObjectId),
}

impl Object {
    /// Whether this is the null object.
    #[must_use]
    pub fn is_null(&self) -> bool {
        matches!(self, Object::Null)
    }

    /// The boolean value, if this is a `Boolean`.
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Object::Boolean(b) => Some(*b),
            _ => None,
        }
    }

    /// The integer value, if this is an `Integer`. Does not match `Real` (ADR-0003).
    #[must_use]
    pub fn as_integer(&self) -> Option<i64> {
        match self {
            Object::Integer(i) => Some(*i),
            _ => None,
        }
    }

    /// The real value, if this is a `Real`. Does not match `Integer` (use [`as_f64`](Self::as_f64)
    /// for numeric coercion).
    #[must_use]
    pub fn as_real(&self) -> Option<f64> {
        match self {
            Object::Real(r) => Some(*r),
            _ => None,
        }
    }

    /// The numeric value as `f64`, coercing an `Integer` to floating point. This is the **only**
    /// accessor that crosses the integer/real divide (ADR-0003).
    #[must_use]
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Object::Integer(i) => Some(*i as f64),
            Object::Real(r) => Some(*r),
            _ => None,
        }
    }

    /// A reference to the string, if this is a `String`.
    #[must_use]
    pub fn as_string(&self) -> Option<&PdfString> {
        match self {
            Object::String(s) => Some(s),
            _ => None,
        }
    }

    /// A reference to the name, if this is a `Name`.
    #[must_use]
    pub fn as_name(&self) -> Option<&Name> {
        match self {
            Object::Name(n) => Some(n),
            _ => None,
        }
    }

    /// A reference to the array, if this is an `Array`.
    #[must_use]
    pub fn as_array(&self) -> Option<&Array> {
        match self {
            Object::Array(a) => Some(a),
            _ => None,
        }
    }

    /// A reference to the dictionary, if this is a `Dictionary`.
    #[must_use]
    pub fn as_dict(&self) -> Option<&Dictionary> {
        match self {
            Object::Dictionary(d) => Some(d),
            _ => None,
        }
    }

    /// A reference to the stream, if this is a `Stream`.
    #[must_use]
    pub fn as_stream(&self) -> Option<&Stream> {
        match self {
            Object::Stream(s) => Some(s),
            _ => None,
        }
    }

    /// The object id, if this is an indirect `Reference`.
    #[must_use]
    pub fn as_reference(&self) -> Option<ObjectId> {
        match self {
            Object::Reference(id) => Some(*id),
            _ => None,
        }
    }
}

impl From<bool> for Object {
    fn from(b: bool) -> Self {
        Object::Boolean(b)
    }
}

impl From<i64> for Object {
    fn from(i: i64) -> Self {
        Object::Integer(i)
    }
}

impl From<i32> for Object {
    fn from(i: i32) -> Self {
        Object::Integer(i.into())
    }
}

impl From<f64> for Object {
    fn from(r: f64) -> Self {
        Object::Real(r)
    }
}

impl From<ObjectId> for Object {
    fn from(id: ObjectId) -> Self {
        Object::Reference(id)
    }
}

impl From<PdfString> for Object {
    fn from(s: PdfString) -> Self {
        Object::String(s)
    }
}

impl From<Name> for Object {
    fn from(n: Name) -> Self {
        Object::Name(n)
    }
}

impl From<Array> for Object {
    fn from(a: Array) -> Self {
        Object::Array(a)
    }
}

impl From<Dictionary> for Object {
    fn from(d: Dictionary) -> Self {
        Object::Dictionary(d)
    }
}

impl From<Stream> for Object {
    fn from(s: Stream) -> Self {
        Object::Stream(s)
    }
}

impl From<Vec<Object>> for Object {
    fn from(items: Vec<Object>) -> Self {
        Object::Array(Array::from_vec(items))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_null() {
        assert_eq!(Object::default(), Object::Null);
        assert!(Object::Null.is_null());
    }

    #[test]
    fn integer_and_real_do_not_coerce_in_equality() {
        // §7.3.3 + ADR-0003: distinct types compare unequal even for the "same" number.
        assert_ne!(Object::Integer(1), Object::Real(1.0));
        // ...but as_f64 deliberately coerces for callers who want it.
        assert_eq!(Object::Integer(1).as_f64(), Some(1.0));
        assert_eq!(Object::Real(1.0).as_f64(), Some(1.0));
        assert_eq!(Object::Integer(1).as_real(), None);
    }

    #[test]
    fn a_reference_is_never_equal_to_its_target() {
        // ADR-0001: equality does not resolve, so these are simply different objects.
        let reference = Object::Reference(ObjectId::new(7, 0));
        let target = Object::Integer(42);
        assert_ne!(reference, target);
        assert_eq!(reference.as_reference(), Some(ObjectId::new(7, 0)));
    }

    #[test]
    fn typed_accessors_reject_wrong_variants() {
        assert_eq!(Object::Boolean(true).as_bool(), Some(true));
        assert_eq!(Object::Boolean(true).as_integer(), None);
        assert!(Object::Null.as_dict().is_none());
    }
}
