#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//! pdf-document — the document object model (EPIC 4, ISO 32000 §7.7).
//!
//! Sits above [`pdf_reader`]: a [`Document`] owns the file bytes and its cross-reference table and
//! turns the flat object store into a navigable DOM — resolving indirect references (ADR-0001),
//! reaching the catalog (§7.7.2) and walking the page tree (§7.7.3) to count and enumerate pages.
//!
//! This is where Milestone M1 culminates: [`Document::open`] + [`Document::page_count`] open a
//! real (classic- or stream-xref) PDF and count its pages. Hostile input is assumed throughout
//! (DESIGN.md §3.4): reference chains and the page tree are depth- and cycle-guarded, and a
//! reference to a missing object resolves to null (§7.3.10) rather than failing.

use std::collections::BTreeSet;
use std::sync::Arc;

use pdf_cos::{Dictionary, Name, Object, ObjectId};
use pdf_crypto::StandardSecurityHandler;
/// Encryption algorithm selector and access permissions for the `save_encrypted*` methods (§7.6).
pub use pdf_crypto::{Algorithm, Permissions};
/// PAdES-LT revocation outcomes surfaced by [`Document::verify_signatures_ltv`] (§12.8.4.3).
pub use pdf_crypto::{RevocationData, RevocationStatus, RevocationSummary};
/// Configurable anti-DoS limits applied to untrusted input (§3.4): nesting depth, object-stream
/// size, and total object count. Pass to [`Document::open_with_limits`].
pub use pdf_reader::Limits;
use pdf_reader::{ReaderError, Version, XRef, XRefEntry};
use pdf_writer::{
    write_document, write_document_encrypted, write_document_xref_stream, write_incremental,
};

use crate::trace::log_warn;

/// The result type returned throughout `pdf-document`.
pub type Result<T> = std::result::Result<T, DocError>;

/// A failure while building or navigating the document model (§7.7).
#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DocError {
    /// An error from the reader/parser layer (§7.2–§7.5).
    #[error(transparent)]
    Reader(#[from] ReaderError),
    /// The trailer had no usable `/Root` catalog (§7.7.2).
    #[error("document has no catalog (/Root)")]
    MissingCatalog,
    /// An object expected to be a dictionary was something else (the context names which).
    #[error("expected a dictionary for {0}")]
    NotADictionary(&'static str),
    /// The page tree (§7.7.3) was cyclic or nested past the safety limit.
    #[error("malformed or pathological page tree")]
    BadPageTree,
    /// A page's content stream (§7.8.2) could not be decoded by the filter layer.
    #[error("failed to decode a content stream")]
    ContentDecode,
    /// The document is encrypted with a supported handler, but the supplied credentials did not
    /// unlock it (§7.6): for the standard handler neither the user nor the owner password matched;
    /// for the public-key handler (§7.6.5) the certificate matched no recipient.
    #[error("wrong or missing credentials for encrypted document")]
    NeedsPassword,
    /// A recipient certificate supplied to [`Document::save_encrypted_public_key`] could not be
    /// parsed or used (§7.6.5).
    #[error("invalid recipient certificate for public-key encryption")]
    BadRecipientCert,
    /// The operating system's random number generator was unavailable, so no file key, salt, IV or
    /// nonce could be drawn (§7.6). Encryption is refused rather than performed under predictable
    /// material — a document "encrypted" under a zero key would report success while protecting
    /// nothing.
    #[error("the system random number generator is unavailable")]
    RandomUnavailable,
    /// Signing failed (§12.8): the certificate/key was unusable, the document had no page to host
    /// the signature, or the CMS exceeded the reserved `/Contents` space.
    #[error("could not sign the document")]
    SigningFailed,
    /// PDF MAC integrity protection (ISO/TS 32004) was requested with an algorithm other than
    /// AES-256 (V5/R6) — the only handler whose file encryption key the MAC can key on (§3.3).
    #[error("PDF MAC requires AES-256 (V5/R6) encryption")]
    MacRequiresV5,
    /// PDF MAC composition failed: the placeholder layout or the deterministic token length did
    /// not hold (ISO/TS 32004 §6).
    #[error("could not compute the PDF MAC token")]
    MacFailed,
    /// The content contains a construct that requires a PDF version above the declared target
    /// (§7.5.2, M17 construct gate): the diagnostic names the offending construct. Raise the
    /// target, or drop the construct.
    #[error(
        "{construct} requires PDF {}.{}, above the target version {}.{}",
        .required.0, .required.1, .target.0, .target.1
    )]
    TargetVersionExceeded {
        /// The offending construct, citing its ISO 32000 section.
        construct: String,
        /// The minimum version that construct is valid at.
        required: (u8, u8),
        /// The requested target version.
        target: (u8, u8),
    },
}

mod annotations;
mod builder;
mod dss;
mod edit;
mod encryption;
mod extensions;
mod flatten;
mod forms;
mod mac;
mod metadata;
mod names;
mod outlines;
mod signing;
mod trace;
mod wrapper;

pub use annotations::Annotation;
pub use builder::{
    AnnotationSpec, Attachment, AttrValue, Builder, CidFont, DocumentFacts, DocumentPart,
    EncryptedPayloadSpec, FormFieldSpec, ImageColorSpace, ImageFilter, ImageXObject, LinkTarget,
    ListNumbering, MATHML_STRUCT_NS, PDF2_STRUCT_NS, PageLabelRange, PageLabelStyle, PageSpec,
    PrintFieldRole, RoleMapEntry, SeparationSpec, StdFont, StructAttr, StructElem, StructKid,
    StructureElementFact, ThScope,
};
pub use dss::{DssInfo, SignatureValidation, ValidationData};
pub use edit::{merge, merge_with_report};
pub use extensions::DeveloperExtension;
pub use forms::FormField;
pub use names::{ExtractedAttachment, decode_text_string};
pub use outlines::OutlineItem;
pub use signing::{SignSettings, SignatureAppearance, SignatureStatus, TsaCredentials};
pub use wrapper::EncryptedPayload;

/// Maximum length of an indirect-reference chain before it is treated as a cycle (§7.3.10).
const MAX_RESOLVE_DEPTH: usize = 64;
/// Maximum page-tree depth. Legitimate trees are shallow; this bounds recursion (anti-DoS).
const MAX_PAGE_TREE_DEPTH: usize = 1024;
/// Page attributes inherited down the page tree (§7.7.3.4).
const INHERITABLE_KEYS: [&str; 4] = ["Resources", "MediaBox", "CropBox", "Rotate"];

/// An opened PDF document: its bytes plus the resolved cross-reference table (§7.5/§7.7).
#[derive(Clone, Debug)]
pub struct Document {
    bytes: Vec<u8>,
    xref: XRef,
    open_report: OpenReport,
    /// The anti-DoS bounds this document was opened under (§3.4). Kept so that every later decode
    /// of one of its streams honours the same ceilings the caller chose at `open` time.
    limits: Limits,
    /// The security handler this document was opened with, when it is encrypted (§7.6), together
    /// with the reference to its `/Encrypt` dictionary.
    ///
    /// Kept because an **incremental update** has to re-encrypt what it writes: §7.6.2 applies
    /// encryption to every string and stream in the file, and §7.5.6 requires the added trailer to
    /// carry `/Encrypt` forward. Signing therefore needs the file key again, and holding the
    /// handler here means it does not have to ask the caller for the password a second time.
    security: Option<(Arc<StandardSecurityHandler>, ObjectId)>,
}

/// Whether a document opened from its declared cross-reference data or bounded scan recovery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenMode {
    /// The declared cross-reference data led to a reachable catalog.
    Strict,
    /// A bounded object scan rebuilt the cross-reference data.
    Recovered,
}

/// Why the strict open path switched to recovery (DESIGN.md §3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryReason {
    /// Parsing the declared cross-reference data failed at the recorded byte offset.
    XrefParseFailure,
    /// The declared cross-reference data parsed but did not lead to a reachable catalog.
    UnreachableCatalog,
}

/// One bounded diagnostic recorded while opening a document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenDiagnostic {
    /// Recovery trigger.
    pub reason: RecoveryReason,
    /// Input byte offset when the reader supplied one.
    pub offset: Option<usize>,
}

/// Outcome of opening a document. At most two recovery diagnostics are recorded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenReport {
    mode: OpenMode,
    diagnostics: Vec<OpenDiagnostic>,
}

impl OpenReport {
    fn strict() -> Self {
        Self {
            mode: OpenMode::Strict,
            diagnostics: Vec::new(),
        }
    }

    /// Strict or recovered open mode.
    #[must_use]
    pub fn mode(&self) -> OpenMode {
        self.mode
    }

    /// Bounded recovery diagnostics, in the order encountered.
    #[must_use]
    pub fn diagnostics(&self) -> &[OpenDiagnostic] {
        &self.diagnostics
    }

    fn recovered(&mut self, diagnostic: OpenDiagnostic) {
        log_warn!(
            "open switched to bounded recovery: {:?} (byte offset {:?})",
            diagnostic.reason,
            diagnostic.offset
        );
        self.mode = OpenMode::Recovered;
        if self.diagnostics.len() < 2 {
            self.diagnostics.push(diagnostic);
        }
    }
}

/// How a manipulation serialised its result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RewriteMode {
    /// Appended a new revision while retaining every original byte (§7.5.6).
    Incremental,
    /// Re-emitted the existing object graph as a fresh single-revision file.
    FullRewrite,
    /// Built a new object graph from selected source content, with fresh object identities.
    Reconstructed,
}

/// Effect of a manipulation on signatures already present in the input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignatureEffect {
    /// Existing signed byte ranges remain byte-for-byte intact.
    Preserved,
    /// Signature objects remain, but rewriting bytes invalidates their cryptographic coverage.
    Invalidated,
    /// Signature objects are not carried into the reconstructed output.
    Removed,
}

/// Effect of a manipulation on the logical structure tree (§14.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StructureEffect {
    /// The existing structure object graph is retained.
    Preserved,
    /// The tree remains present, but the operation can leave semantic or object references stale.
    Invalidated,
    /// The structure tree is deliberately omitted because references cannot remain valid.
    Removed,
}

/// Owned bytes plus explicit preservation effects from a document manipulation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransformReport {
    bytes: Vec<u8>,
    rewrite_mode: RewriteMode,
    signature_effect: SignatureEffect,
    structure_effect: StructureEffect,
}

impl TransformReport {
    pub(crate) fn new(
        bytes: Vec<u8>,
        rewrite_mode: RewriteMode,
        signature_effect: SignatureEffect,
        structure_effect: StructureEffect,
    ) -> Self {
        Self {
            bytes,
            rewrite_mode,
            signature_effect,
            structure_effect,
        }
    }

    /// Construct a report for an additive facade operation that performs a full rewrite while
    /// preserving the logical structure object graph.
    #[must_use]
    pub fn preserving_full_rewrite(bytes: Vec<u8>) -> Self {
        Self::new(
            bytes,
            RewriteMode::FullRewrite,
            SignatureEffect::Invalidated,
            StructureEffect::Preserved,
        )
    }

    /// Resulting PDF bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Consume the report and return its PDF bytes.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// Serialization mode used by the operation.
    #[must_use]
    pub fn rewrite_mode(&self) -> RewriteMode {
        self.rewrite_mode
    }

    /// Effect on signatures already present in the source.
    #[must_use]
    pub fn signature_effect(&self) -> SignatureEffect {
        self.signature_effect
    }

    /// Effect on the source logical structure tree.
    #[must_use]
    pub fn structure_effect(&self) -> StructureEffect {
        self.structure_effect
    }
}

/// One leaf document part (§14.12, PDF 2.0) read back via [`Document::document_parts`]: the inclusive
/// page range it spans and its `/DPM` Document Part Metadata.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct DocumentPartInfo {
    /// 0-based index of the part's first page (from `/Start`).
    pub start_page: usize,
    /// 0-based index of the part's last page (from `/End`, or `start_page` when absent).
    pub end_page: usize,
    /// `/DPM` Document Part Metadata: `(key, value)` text entries, in dictionary order.
    pub metadata: Vec<(String, String)>,
}

impl Document {
    /// Open a document from its raw bytes, parsing the header, cross-reference table and trailer
    /// (§7.5). Object bodies are parsed lazily on demand (DESIGN.md §3: lazy & streaming).
    ///
    /// Recovery is first-class (DESIGN.md §3): if the cross-reference table cannot be parsed — or
    /// parses but does not yield a reachable catalog (stale offsets in an edited file) — the table
    /// is rebuilt by scanning the whole file ([`XRef::rebuild`]).
    pub fn open(bytes: impl Into<Vec<u8>>) -> Result<Self> {
        Self::open_inner(bytes.into(), b"", Limits::default())
    }

    /// Open a document with explicit anti-DoS [`Limits`] (§3.4) — tune the caps on nesting, object
    /// streams and total object count when handling especially hostile or especially large input.
    pub fn open_with_limits(bytes: impl Into<Vec<u8>>, limits: Limits) -> Result<Self> {
        Self::open_inner(bytes.into(), b"", limits)
    }

    /// Open a document, supplying a `password` for encrypted files (§7.6). The password is tried as
    /// both the user and the owner password; an encrypted document whose handler is supported but
    /// whose password matches neither yields [`DocError::NeedsPassword`]. For unencrypted files the
    /// password is ignored.
    pub fn open_with_password(bytes: impl Into<Vec<u8>>, password: &[u8]) -> Result<Self> {
        Self::open_inner(bytes.into(), password, Limits::default())
    }

    /// Open with both an encryption password (§7.6) and explicit hostile-input limits
    /// (DESIGN.md §3.4). This is the fully configurable form of [`open`](Self::open).
    pub fn open_with_password_and_limits(
        bytes: impl Into<Vec<u8>>,
        password: &[u8],
        limits: Limits,
    ) -> Result<Self> {
        Self::open_inner(bytes.into(), password, limits)
    }

    /// The shared open path: parse the cross-reference table (falling back to a bounded recovery
    /// scan), install decryption, and rebuild once more if the table parsed but reaches no catalog.
    fn open_inner(bytes: Vec<u8>, password: &[u8], limits: Limits) -> Result<Self> {
        let mut open_report = OpenReport::strict();
        let xref = match XRef::parse_with_limits(&bytes, limits) {
            Ok(xref) => xref,
            Err(error) => {
                open_report.recovered(OpenDiagnostic {
                    reason: RecoveryReason::XrefParseFailure,
                    offset: Some(error.offset()),
                });
                XRef::rebuild_with_limits(&bytes, limits)?
            }
        };
        let mut doc = Self {
            bytes,
            xref,
            open_report,
            limits,
            security: None,
        };
        doc.setup_encryption(password)?;

        // A table that parsed but cannot reach its catalog (e.g. offsets invalidated by an edit)
        // is as good as broken — rebuild and prefer the result if it is actually better.
        if doc.catalog().is_err()
            && let Ok(rebuilt) = XRef::rebuild_with_limits(&doc.bytes, limits)
        {
            doc.xref = rebuilt;
            doc.open_report.recovered(OpenDiagnostic {
                reason: RecoveryReason::UnreachableCatalog,
                offset: None,
            });
            doc.setup_encryption(password)?; // re-arm decryption on the rebuilt table
        }
        Ok(doc)
    }

    /// Report whether opening used strict cross-reference data or bounded recovery.
    #[must_use]
    pub fn open_report(&self) -> &OpenReport {
        &self.open_report
    }

    /// The anti-DoS bounds this document was opened under (§3.4).
    #[must_use]
    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// Open a public-key-encrypted document (§7.6.5, `/Filter /Adobe.PPKLite`) using the recipient's
    /// certificate (`cert_der`, DER X.509) and private key (`key_der`, PKCS#8 DER). A document whose
    /// certificate matches no recipient yields [`DocError::NeedsPassword`]; unencrypted and
    /// password-encrypted files ignore the key material (use [`Document::open_with_password`] for
    /// those).
    pub fn open_with_private_key(
        bytes: impl Into<Vec<u8>>,
        cert_der: &[u8],
        key_der: &[u8],
    ) -> Result<Self> {
        let bytes = bytes.into();
        let mut open_report = OpenReport::strict();
        let xref = match XRef::parse(&bytes) {
            Ok(xref) => xref,
            Err(error) => {
                open_report.recovered(OpenDiagnostic {
                    reason: RecoveryReason::XrefParseFailure,
                    offset: Some(error.offset()),
                });
                XRef::rebuild(&bytes)?
            }
        };
        let mut doc = Self {
            bytes,
            xref,
            open_report,
            limits: Limits::default(),
            security: None,
        };
        doc.setup_encryption_public_key(cert_der, key_der)?;
        if doc.catalog().is_err()
            && let Ok(rebuilt) = XRef::rebuild(&doc.bytes)
        {
            doc.xref = rebuilt;
            doc.open_report.recovered(OpenDiagnostic {
                reason: RecoveryReason::UnreachableCatalog,
                offset: None,
            });
            doc.setup_encryption_public_key(cert_der, key_der)?;
        }
        Ok(doc)
    }

    /// The first element of the file `/ID` (used in key derivation, §7.6.4.3.2), or empty.
    fn file_id0(&self) -> Vec<u8> {
        match self.xref.trailer.get(&Name::from("ID")) {
            Some(Object::Array(ids)) => match ids.first() {
                Some(Object::String(s)) => s.as_bytes().to_vec(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    /// The header version (§7.5.2), if the file declared one.
    #[must_use]
    pub fn version(&self) -> Option<Version> {
        self.xref.version
    }

    /// The minimum PDF version this document's *content* requires (§7.5.2) — the checker side of
    /// M18, sharing [`pdf_writer::min_version`] with the producer (Builder auto-stamp, M17). It
    /// combines the object-set analysis with the encryption-method floor (`/Encrypt /V`: V5 → 2.0,
    /// V4 → 1.6), which the cipher hides from the object set. A declared [`Document::version`] below
    /// this is a version-boundary violation (a file claiming a version it doesn't actually fit).
    ///
    /// It is a sound *lower bound*: structural floors that the writer re-derives on save (the
    /// cross-reference *stream* form, ≥1.5) are not added, and unrecognised constructs never raise
    /// it — so it never over-reports a violation.
    pub fn min_pdf_version(&self) -> Result<(u8, u8)> {
        let objects = self.live_objects()?;
        let mut v = pdf_writer::min_version(&objects);
        if let Some(enc) = self.xref.trailer.get(&Name::from("Encrypt"))
            && let Ok(dict) = self.resolve_dict(enc, "Encrypt")
        {
            let floor = match dict.get_integer(&Name::from("V")) {
                Some(5) => (2, 0),
                Some(4) => (1, 6),
                _ => (1, 4),
            };
            if floor > v {
                v = floor;
            }
        }
        Ok(v)
    }

    /// Fetch the object with the given identity, or [`Object::Null`] if it is free or undefined
    /// (§7.3.10: a reference to a missing object is null, not an error).
    pub fn get(&self, id: ObjectId) -> Result<Object> {
        Ok(self
            .xref
            .fetch(&self.bytes, id.number)?
            .unwrap_or(Object::Null))
    }

    /// Resolve `object` to a direct value, following a chain of indirect references (§7.3.10).
    /// Non-references are returned as-is. A chain longer than `MAX_RESOLVE_DEPTH` is treated as a
    /// cycle.
    pub fn resolve(&self, object: &Object) -> Result<Object> {
        let mut current = object.clone();
        for _ in 0..MAX_RESOLVE_DEPTH {
            match current {
                Object::Reference(id) => current = self.get(id)?,
                other => return Ok(other),
            }
        }
        Err(DocError::BadPageTree)
    }

    /// Resolve `object` and require it to be a dictionary; `context` names it for error messages.
    fn resolve_dict(&self, object: &Object, context: &'static str) -> Result<Dictionary> {
        match self.resolve(object)? {
            Object::Dictionary(dict) => Ok(dict),
            _ => Err(DocError::NotADictionary(context)),
        }
    }

    /// The document catalog (`/Root`), §7.7.2.
    pub fn catalog(&self) -> Result<Dictionary> {
        let root = self.xref.root().ok_or(DocError::MissingCatalog)?;
        self.resolve_dict(&Object::Reference(root), "Catalog")
    }

    /// The document information dictionary (`/Info`, §14.3.3), if present.
    pub fn info(&self) -> Result<Option<Dictionary>> {
        match self.xref.trailer.get(&Name::from("Info")) {
            None | Some(Object::Null) => Ok(None),
            Some(object) => Ok(Some(self.resolve_dict(object, "Info")?)),
        }
    }

    /// Every leaf page dictionary, in document order (§7.7.3), with inherited attributes
    /// (§7.7.3.4) folded in so each is self-contained.
    pub fn pages(&self) -> Result<Vec<Dictionary>> {
        Ok(self
            .page_entries()?
            .into_iter()
            .map(|(_, page)| page)
            .collect())
    }

    /// Like [`pages`](Self::pages) but pairs each page with its object id (when it has one — i.e.
    /// it is referenced indirectly, the normal case). Used by the editing operations.
    pub fn page_entries(&self) -> Result<Vec<(Option<ObjectId>, Dictionary)>> {
        let catalog = self.catalog()?;
        let pages_obj = catalog
            .get(&Name::from("Pages"))
            .ok_or(DocError::BadPageTree)?;
        let root_id = match pages_obj {
            Object::Reference(id) => Some(*id),
            _ => None,
        };
        let root = self.resolve_dict(pages_obj, "Pages")?;

        let mut visited = BTreeSet::new();
        if let Some(id) = root_id {
            visited.insert(id);
        }
        let mut out = Vec::new();
        self.walk_pages(
            root_id,
            &root,
            &Dictionary::new(),
            &mut visited,
            &mut out,
            0,
        )?;
        Ok(out)
    }

    /// The number of pages in the document (§7.7.3) — the M1 headline operation.
    pub fn page_count(&self) -> Result<usize> {
        Ok(self.page_entries()?.len())
    }

    /// Read the structure namespaces declared by the document (`/StructTreeRoot /Namespaces`,
    /// §14.7.4, PDF 2.0): the `/NS` URI of each `/Namespace` dictionary, in array order. Empty when
    /// the document is untagged or declares no namespaces. The read complement of
    /// [`Builder::structure_namespace`](crate::Builder::structure_namespace).
    pub fn structure_namespaces(&self) -> Result<Vec<String>> {
        let catalog = self.catalog()?;
        let Some(root_obj) = catalog.get(&Name::from("StructTreeRoot")) else {
            return Ok(Vec::new());
        };
        let root = self.resolve_dict(root_obj, "StructTreeRoot")?;
        let Some(list) = root
            .get(&Name::from("Namespaces"))
            .and_then(Object::as_array)
        else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for ns_ref in list.iter() {
            let ns = self.resolve_dict(ns_ref, "Namespace")?;
            if let Some(Object::String(s)) = ns.get(&Name::from("NS")) {
                out.push(names::decode_text_string(s.as_bytes()));
            }
        }
        Ok(out)
    }

    /// Read the document-part hierarchy (`/DPartRoot`, §14.12, PDF 2.0) as a flat list of leaf parts,
    /// each with its page range (resolved from `/Start`/`/End` to 0-based page indices) and `/DPM`
    /// metadata. Returns an empty vector when the document has no `/DPartRoot`. The complement of the
    /// authoring side ([`Builder::document_parts`](crate::Builder::document_parts)).
    pub fn document_parts(&self) -> Result<Vec<DocumentPartInfo>> {
        let catalog = self.catalog()?;
        let Some(root_obj) = catalog.get(&Name::from("DPartRoot")) else {
            return Ok(Vec::new());
        };
        // Map each page's object id to its 0-based index, to resolve /Start and /End references.
        let page_index: std::collections::HashMap<ObjectId, usize> = self
            .page_entries()?
            .into_iter()
            .enumerate()
            .filter_map(|(i, (id, _))| id.map(|id| (id, i)))
            .collect();
        let page_of = |obj: Option<&Object>| match obj {
            Some(Object::Reference(id)) => page_index.get(id).copied(),
            _ => None,
        };

        let root = self.resolve_dict(root_obj, "DPartRoot")?;
        let node = match root.get(&Name::from("DPartRootNode")) {
            Some(obj) => self.resolve_dict(obj, "DPart")?,
            None => return Ok(Vec::new()),
        };
        let mut parts = Vec::new();
        let Some(leaves) = node.get(&Name::from("DParts")).and_then(Object::as_array) else {
            return Ok(parts);
        };
        for leaf_ref in leaves.iter() {
            let leaf = self.resolve_dict(leaf_ref, "DPart")?;
            let start = page_of(leaf.get(&Name::from("Start"))).unwrap_or(0);
            let end = page_of(leaf.get(&Name::from("End"))).unwrap_or(start);
            let metadata = match leaf.get(&Name::from("DPM")) {
                Some(obj) => self
                    .resolve_dict(obj, "DPM")?
                    .iter()
                    .map(|(k, v)| {
                        let value = match v {
                            Object::String(s) => names::decode_text_string(s.as_bytes()),
                            _ => String::new(),
                        };
                        (String::from_utf8_lossy(k.as_bytes()).into_owned(), value)
                    })
                    .collect(),
                None => Vec::new(),
            };
            parts.push(DocumentPartInfo {
                start_page: start,
                end_page: end,
                metadata,
            });
        }
        Ok(parts)
    }

    /// The document's permanent file identifier (`/ID` element 1, §14.4), when it has one.
    ///
    /// §14.4 makes the first element "permanent … it shall not change" for the life of the file, so
    /// every writer that re-serializes this document hands it back to `pdf-writer` instead of
    /// letting a fresh one be synthesized. Absent (or malformed) `/ID` yields `None`, which leaves
    /// the writer to synthesize one where the target version requires it (§7.5.5).
    fn preserved_file_id(&self) -> Option<Vec<u8>> {
        let entry = self
            .resolve(self.xref.trailer.get(&Name::from("ID"))?)
            .ok()?;
        let permanent = self.resolve(entry.as_array()?.first()?).ok()?;
        let bytes = permanent.as_string()?.as_bytes();
        (!bytes.is_empty()).then(|| bytes.to_vec())
    }

    /// Serialize the document to a fresh single-revision PDF (full rewrite, §7.5).
    ///
    /// Every live object is re-emitted with a classic cross-reference table; compressed objects
    /// (§7.5.7) are exploded into normal indirect objects, and the now-redundant object/xref
    /// *stream* containers are dropped. This normalises any input — classic, stream-xref, or one
    /// that only opened via recovery — into a clean, widely-readable file.
    ///
    /// The trailer file identifier (§14.4) is carried over from the input when it has one, and
    /// synthesized when the declared version requires an `/ID` and the input had none (§7.5.5:
    /// required from PDF 2.0 on).
    pub fn save(&self) -> Result<Vec<u8>> {
        self.save_with_overrides(&std::collections::HashMap::new())
    }

    /// Full-rewrite save with explicit signature and structure preservation effects.
    pub fn save_with_report(&self) -> Result<TransformReport> {
        Ok(TransformReport::new(
            self.save()?,
            RewriteMode::FullRewrite,
            SignatureEffect::Invalidated,
            StructureEffect::Preserved,
        ))
    }

    /// Like [`save`](Self::save), but each live object whose number is a key in `overrides` is
    /// re-emitted with the supplied value instead of its current one. Used to rewrite specific
    /// objects (e.g. re-embedding subsetted font programs) in a full rewrite.
    pub fn save_with_overrides(
        &self,
        overrides: &std::collections::HashMap<u32, Object>,
    ) -> Result<Vec<u8>> {
        let root = self.xref.root().ok_or(DocError::MissingCatalog)?;
        let info = self
            .xref
            .trailer
            .get(&Name::from("Info"))
            .and_then(Object::as_reference);
        let version = self.version().map_or((1, 7), |v| (v.major, v.minor));
        let mut objects = self.collect_objects()?;
        for (id, object) in &mut objects {
            if let Some(replacement) = overrides.get(&id.number) {
                *object = replacement.clone();
            }
        }
        Ok(write_document(
            &objects,
            root,
            info,
            version,
            self.preserved_file_id().as_deref(),
        ))
    }

    /// Full-rewrite save with object overrides and explicit preservation effects (§7.5).
    pub fn save_with_overrides_report(
        &self,
        overrides: &std::collections::HashMap<u32, Object>,
    ) -> Result<TransformReport> {
        Ok(TransformReport::new(
            self.save_with_overrides(overrides)?,
            RewriteMode::FullRewrite,
            SignatureEffect::Invalidated,
            StructureEffect::Preserved,
        ))
    }

    /// Serialize the document as a fresh single-revision PDF **declaring exactly the target
    /// version** `(major, minor)` (§7.5.2, M17 Phase 2) — with the *guarantee* that the content
    /// fits that version: if any construct requires a higher version than the target, the save is
    /// refused with [`DocError::TargetVersionExceeded`] naming the offending construct.
    ///
    /// The output uses a classic cross-reference table (valid at every version; the compact
    /// stream form of [`Document::save_compact`] would itself require 1.5). Declaring a target
    /// *above* the content's minimum is always allowed — over-declaring is harmless, only
    /// under-declaring is a violation. Note the check runs on the objects actually written: an
    /// encrypted document saves decrypted here, so its encryption-method floor does not apply.
    pub fn save_as(&self, major: u8, minor: u8) -> Result<Vec<u8>> {
        let root = self.xref.root().ok_or(DocError::MissingCatalog)?;
        let info = self
            .xref
            .trailer
            .get(&Name::from("Info"))
            .and_then(Object::as_reference);
        let objects = self.collect_objects()?;
        if let Some(v) = pdf_writer::version_violation(&objects, (major, minor)) {
            return Err(DocError::TargetVersionExceeded {
                construct: v.construct.to_owned(),
                required: v.version,
                target: (major, minor),
            });
        }
        Ok(write_document(
            &objects,
            root,
            info,
            (major, minor),
            self.preserved_file_id().as_deref(),
        ))
    }

    /// Version-targeted full rewrite with explicit preservation effects.
    pub fn save_as_with_report(&self, major: u8, minor: u8) -> Result<TransformReport> {
        Ok(TransformReport::new(
            self.save_as(major, minor)?,
            RewriteMode::FullRewrite,
            SignatureEffect::Invalidated,
            StructureEffect::Preserved,
        ))
    }

    /// Serialize the document to a fresh single-revision PDF that uses a **cross-reference stream**
    /// (§7.5.8) instead of a classic table — the compact form for PDF 1.5+ (this bumps the header
    /// to at least 1.5). Like [`save`](Self::save), compressed objects are exploded and the old
    /// structural streams dropped.
    pub fn save_compact(&self) -> Result<Vec<u8>> {
        let root = self.xref.root().ok_or(DocError::MissingCatalog)?;
        let info = self
            .xref
            .trailer
            .get(&Name::from("Info"))
            .and_then(Object::as_reference);
        // Cross-reference streams require PDF 1.5; raise the declared version if it is older.
        let version = match self.version() {
            Some(v) if (v.major, v.minor) >= (1, 5) => (v.major, v.minor),
            _ => (1, 5),
        };
        let objects = self.collect_objects()?;
        Ok(write_document_xref_stream(
            &objects,
            root,
            info,
            version,
            self.preserved_file_id().as_deref(),
        ))
    }

    /// Cross-reference-stream full rewrite with explicit preservation effects.
    pub fn save_compact_with_report(&self) -> Result<TransformReport> {
        Ok(TransformReport::new(
            self.save_compact()?,
            RewriteMode::FullRewrite,
            SignatureEffect::Invalidated,
            StructureEffect::Preserved,
        ))
    }

    /// Serialize the document to a fresh single-revision PDF that packs its non-stream objects
    /// into **object streams** (§7.5.7) cross-referenced by a **cross-reference stream** (§7.5.8)
    /// — the most compact form, for PDF 1.5+ (the header is bumped to at least 1.5). Not
    /// applicable to encrypted output (compressed objects cannot carry their own crypt state);
    /// use [`Document::save_encrypted`] for that.
    pub fn save_packed(&self) -> Result<Vec<u8>> {
        let root = self.xref.root().ok_or(DocError::MissingCatalog)?;
        let info = self
            .xref
            .trailer
            .get(&Name::from("Info"))
            .and_then(Object::as_reference);
        let version = match self.version() {
            Some(v) if (v.major, v.minor) >= (1, 5) => (v.major, v.minor),
            _ => (1, 5),
        };
        let objects = self.collect_objects()?;
        Ok(pdf_writer::write_document_object_streams(
            &objects,
            root,
            info,
            version,
            self.preserved_file_id().as_deref(),
        ))
    }

    /// Object-stream full rewrite with explicit preservation effects.
    pub fn save_packed_with_report(&self) -> Result<TransformReport> {
        Ok(TransformReport::new(
            self.save_packed()?,
            RewriteMode::FullRewrite,
            SignatureEffect::Invalidated,
            StructureEffect::Preserved,
        ))
    }

    /// Serialize the document to a fresh **encrypted** PDF with the standard security handler
    /// (§7.6). Every object's strings and streams are encrypted; the generated `/Encrypt` dictionary
    /// and a random `/ID` are added to the trailer. Reopening with the same user password decrypts it.
    ///
    /// `algorithm` selects RC4-128, AES-128 or AES-256; the owner password defaults to the user
    /// password when empty. This convenience grants all permissions and encrypts metadata — use
    /// [`Document::save_encrypted_with`] to restrict them.
    pub fn save_encrypted(
        &self,
        user_password: &[u8],
        owner_password: &[u8],
        algorithm: Algorithm,
    ) -> Result<Vec<u8>> {
        self.save_encrypted_with(
            user_password,
            owner_password,
            Permissions::ALL,
            true,
            algorithm,
        )
    }

    /// As [`Document::save_encrypted`], but with explicit access [`Permissions`] (`/P`, §7.6.3.2) and
    /// `encrypt_metadata` (`/EncryptMetadata`, §7.6.4.3 — ignored for RC4, which always encrypts it).
    /// We store and (for AES-256) seal these on write; we do not enforce `/P` on read.
    pub fn save_encrypted_with(
        &self,
        user_password: &[u8],
        owner_password: &[u8],
        permissions: Permissions,
        encrypt_metadata: bool,
        algorithm: Algorithm,
    ) -> Result<Vec<u8>> {
        let (handler, encrypt_dict, id0) = StandardSecurityHandler::new_encrypter(
            user_password,
            owner_password,
            permissions.bits(),
            encrypt_metadata,
            algorithm,
        )
        .ok_or(DocError::RandomUnavailable)?;
        self.write_encrypted(handler, encrypt_dict, id0)
    }

    /// Save the document encrypted with the **public-key** security handler (§7.6.5): each
    /// `recipient_certs` entry (DER X.509) can later decrypt the file with its private key (see
    /// [`Document::open_with_private_key`]). `algorithm` selects AES-128 (V4) or AES-256 (V5);
    /// [`Algorithm::Rc4`] or an unparsable certificate yields [`DocError::BadRecipientCert`]. Grants
    /// all permissions and encrypts metadata — use [`Document::save_encrypted_public_key_with`] to
    /// restrict them.
    pub fn save_encrypted_public_key(
        &self,
        recipient_certs: &[&[u8]],
        algorithm: Algorithm,
    ) -> Result<Vec<u8>> {
        self.save_encrypted_public_key_with(recipient_certs, Permissions::ALL, true, algorithm)
    }

    /// As [`Document::save_encrypted_public_key`], with explicit [`Permissions`] and
    /// `encrypt_metadata` (§7.6.3.2 / §7.6.4.3).
    pub fn save_encrypted_public_key_with(
        &self,
        recipient_certs: &[&[u8]],
        permissions: Permissions,
        encrypt_metadata: bool,
        algorithm: Algorithm,
    ) -> Result<Vec<u8>> {
        let (handler, encrypt_dict, id0) = StandardSecurityHandler::new_encrypter_public_key(
            recipient_certs,
            permissions.bits(),
            encrypt_metadata,
            algorithm,
        )
        .ok_or(DocError::BadRecipientCert)?;
        self.write_encrypted(handler, encrypt_dict, id0)
    }

    /// Shared back end for the `save_encrypted*` methods: encrypt every object with `handler`, append
    /// the (cleartext) `/Encrypt` dictionary as a new object, and serialize with a random `/ID`.
    fn write_encrypted(
        &self,
        handler: StandardSecurityHandler,
        encrypt_dict: Dictionary,
        id0: Vec<u8>,
    ) -> Result<Vec<u8>> {
        let root = self.xref.root().ok_or(DocError::MissingCatalog)?;
        let info = self
            .xref
            .trailer
            .get(&Name::from("Info"))
            .and_then(Object::as_reference);
        // Header version (§7.5.2): keep any existing higher header, but floor by the encryption
        // method (the cipher isn't visible in the object set, so `min_version` can't see it).
        // AES-256 (V5/AESV3) is a PDF 2.0 / ExtensionLevel-3 feature; AES-128 (V4/AESV2) is 1.6.
        let cipher_floor = match encrypt_dict.get_integer(&Name::from("V")) {
            Some(5) => (2, 0),
            Some(4) => (1, 6),
            _ => (1, 4),
        };
        let base = self.version().map_or((1, 4), |v| (v.major, v.minor));
        let version = base.max(cipher_floor);
        let mut objects = self.collect_objects()?;

        // Encrypt each object's strings/streams (per-object key for RC4/AES-128, file key for V5).
        for (id, object) in &mut objects {
            let (number, generation) = (id.number, id.generation);
            *object = crate::encryption::encrypt_object(object, &|data| {
                handler.encrypt(number, generation, data)
            })
            .ok_or(DocError::RandomUnavailable)?;
        }

        // The /Encrypt dictionary is itself stored in the clear, as a new highest-numbered object.
        let encrypt_number = objects.iter().map(|(id, _)| id.number).max().unwrap_or(0) + 1;
        let encrypt_id = ObjectId::new(encrypt_number, 0);
        objects.push((encrypt_id, Object::Dictionary(encrypt_dict)));

        Ok(write_document_encrypted(
            &objects, root, info, version, encrypt_id, &id0,
        ))
    }

    /// Append an incremental update (§7.5.6): keep the original bytes and append only `changes`
    /// (changed or new indirect objects) as a new revision with its own cross-reference section
    /// and a `/Prev`-chained trailer. The result reopens with `changes` overriding the originals.
    pub fn save_incremental(&self, changes: &[(ObjectId, Object)]) -> Result<Vec<u8>> {
        let root = self.xref.root().ok_or(DocError::MissingCatalog)?;
        let info = self
            .xref
            .trailer
            .get(&Name::from("Info"))
            .and_then(Object::as_reference);

        // /Size must exceed every object number: the original's, plus any newly added.
        let highest_existing = self.xref.entries.keys().max().copied().unwrap_or(0);
        let base = self
            .xref
            .size()
            .and_then(|s| u64::try_from(s).ok())
            .unwrap_or(0)
            .max(u64::from(highest_existing) + 1);
        let highest_changed = changes
            .iter()
            .map(|(id, _)| u64::from(id.number) + 1)
            .max()
            .unwrap_or(0);
        let size = base.max(highest_changed);

        Ok(write_incremental(&self.bytes, changes, root, info, size))
    }

    /// Append an incremental revision with explicit preservation effects (§7.5.6).
    /// Existing signed byte ranges and the logical structure graph remain byte-for-byte present.
    pub fn save_incremental_with_report(
        &self,
        changes: &[(ObjectId, Object)],
    ) -> Result<TransformReport> {
        Ok(TransformReport::new(
            self.save_incremental(changes)?,
            RewriteMode::Incremental,
            SignatureEffect::Preserved,
            StructureEffect::Preserved,
        ))
    }

    /// Gather every live object as `(id, value)`, skipping free entries and the structural
    /// object/xref *stream* containers (which a full rewrite replaces with a classic table).
    /// Every live (in-use) object as `(id, value)` pairs (§7.5): compressed objects (§7.5.7) are
    /// exploded into normal indirect objects and the structural object/xref *stream* containers are
    /// omitted — the exact set [`save`](Self::save) re-emits. Useful for introspection and for
    /// round-trip diffing (verifying a save preserved every object, M11).
    pub fn live_objects(&self) -> Result<Vec<(ObjectId, Object)>> {
        self.collect_objects()
    }

    fn collect_objects(&self) -> Result<Vec<(ObjectId, Object)>> {
        let mut objects = Vec::new();
        for (&number, entry) in &self.xref.entries {
            let generation = match entry {
                XRefEntry::InUse { generation, .. } => *generation,
                XRefEntry::Compressed { .. } => 0,
                XRefEntry::Free { .. } => continue,
            };
            let Some(object) = self.xref.fetch(&self.bytes, number)? else {
                continue;
            };
            if let Object::Stream(stream) = &object {
                let kind = stream
                    .dict()
                    .get_name(&Name::from("Type"))
                    .map(Name::as_bytes);
                if kind == Some(b"ObjStm") || kind == Some(b"XRef") {
                    continue;
                }
            }
            objects.push((ObjectId::new(number, generation), object));
        }
        Ok(objects)
    }

    /// The decoded bytes of a page's content stream(s), §7.8.2. A page's `/Contents` may be a
    /// single stream or an array of streams (which are concatenated with a separating newline);
    /// each stream is decoded through its `/Filter` chain (§7.4). A page with no contents yields
    /// an empty vector.
    pub fn page_content_bytes(&self, page: &Dictionary) -> Result<Vec<u8>> {
        let Some(contents) = page.get(&Name::from("Contents")) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        match self.resolve(contents)? {
            Object::Stream(stream) => {
                out = self.decode_stream(&stream)?;
            }
            Object::Array(parts) => {
                for part in parts.iter() {
                    if let Object::Stream(stream) = self.resolve(part)? {
                        let bytes = self.decode_stream(&stream)?;
                        if !out.is_empty() {
                            out.push(b'\n'); // separate adjacent content streams (§7.8.2)
                        }
                        out.extend_from_slice(&bytes);
                    }
                }
            }
            _ => {}
        }
        Ok(out)
    }

    /// Recursively gather leaf pages under `node`, threading the inheritable attributes (§7.7.3.4)
    /// down from ancestors. A node is an intermediate `/Pages` node iff it has a `/Kids` array
    /// (robust against a missing/wrong `/Type`); otherwise it is a leaf page.
    fn walk_pages(
        &self,
        node_id: Option<ObjectId>,
        node: &Dictionary,
        inherited: &Dictionary,
        visited: &mut BTreeSet<ObjectId>,
        out: &mut Vec<(Option<ObjectId>, Dictionary)>,
        depth: usize,
    ) -> Result<()> {
        if depth > MAX_PAGE_TREE_DEPTH {
            return Err(DocError::BadPageTree);
        }
        // Accumulate this node's inheritable attributes for its descendants.
        let mut inherited = inherited.clone();
        for key in INHERITABLE_KEYS {
            let key = Name::from(key);
            if let Some(value) = node.get(&key) {
                inherited.insert(key, value.clone());
            }
        }

        let Some(kids_obj) = node.get(&Name::from("Kids")) else {
            // Leaf page (§7.7.3.3): inherited attributes, then the page's own keys (which win).
            let mut page = inherited;
            for (key, value) in node.iter() {
                page.insert(key.clone(), value.clone());
            }
            out.push((node_id, page));
            return Ok(());
        };
        let Object::Array(kids) = self.resolve(kids_obj)? else {
            return Err(DocError::BadPageTree);
        };
        for kid in kids.iter() {
            // Cycle guard: never descend into the same node twice (§7.3.10 / anti-DoS).
            let kid_id = match kid {
                Object::Reference(id) => {
                    if !visited.insert(*id) {
                        continue;
                    }
                    Some(*id)
                }
                _ => None,
            };
            let child = self.resolve_dict(kid, "page-tree node")?;
            self.walk_pages(kid_id, &child, &inherited, visited, out, depth + 1)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
