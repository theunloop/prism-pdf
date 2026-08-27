#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//! pdf-cos — Core Object System (EPIC 1, ISO 32000 §7.3).
//!
//! The PDF object model as **dumb, cheaply-cloneable data**. The shape of these types is governed
//! by the four COS design decisions below — this module doc is their canonical statement, and the
//! `ADR-000n` references scattered through `pdf-cos`, `pdf-reader`, `pdf-writer`, `pdf-document`
//! and `pdf-filters` all point here:
//!
//! - **ADR-0001** — COS objects never resolve references. [`Object::Reference`] carries an
//!   [`ObjectId`] but cannot turn it into a value; resolution is the `Document`'s job (in
//!   `pdf-reader`). There is no registry and no I/O here.
//! - **ADR-0002** — Cloning any [`Object`] is O(1). The heavy arms are reference-counted:
//!   [`PdfString`]/[`Stream`] bytes are [`bytes::Bytes`]; [`Array`]/[`Dictionary`] wrap an
//!   [`std::sync::Arc`]. Structural mutation is copy-on-write via [`std::sync::Arc::make_mut`].
//! - **ADR-0003** — Leaves are canonical, not byte-faithful: [`PdfString`] drops the
//!   literal/hex distinction, [`Name`] stores decoded bytes, `Real` is `f64`. Equality is
//!   structural, never resolves references, never coerces `Integer`↔`Real`, and is
//!   order-independent for dictionaries.
//! - **ADR-0004** — [`Stream`]s are inert raw (still-encoded) bytes; decoding lives in
//!   `pdf-filters`, and `raw().len()` — not the dict's `/Length` — is the length authority.

mod array;
mod date;
mod dictionary;
mod name;
mod object;
mod object_id;
mod stream;
mod string;
pub mod syntax;

pub use array::Array;
pub use date::PdfDate;
pub use dictionary::Dictionary;
pub use name::Name;
pub use object::Object;
pub use object_id::ObjectId;
pub use stream::Stream;
pub use string::PdfString;

#[cfg(test)]
mod tests;
