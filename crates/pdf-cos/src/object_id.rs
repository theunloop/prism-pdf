//! Indirect-object identity (ISO 32000 §7.3.10).

use core::fmt;

/// The identity of an indirect object: its object number plus generation number.
///
/// Two indirect objects with the same number but different generations are distinct (§7.3.10).
/// This is a pure value — resolving it into an [`Object`](crate::Object) is the `Document`'s
/// job, never COS's (ADR-0001).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ObjectId {
    /// Object number. Positive in valid PDFs; object 0 heads the cross-reference free list.
    pub number: u32,
    /// Generation number (`0..=65535`).
    pub generation: u16,
}

impl ObjectId {
    /// Creates an [`ObjectId`] from an object number and generation.
    #[must_use]
    pub const fn new(number: u32, generation: u16) -> Self {
        Self { number, generation }
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Reference syntax, as it appears in a PDF body: `n g R`.
        write!(f, "{} {} R", self.number, self.generation)
    }
}
