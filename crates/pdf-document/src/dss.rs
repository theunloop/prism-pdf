//! Document Security Store (DSS) and Validation-Related Information (VRI) — ISO 32000-2 §12.8.4.3
//! / §12.8.4.4, the basis of long-term validation (LTV, PAdES-LT).
//!
//! A signed PDF only stays verifiable as long as the validation material (the signer's certificate
//! chain, plus the CRLs / OCSP responses that proved those certificates unrevoked) remains
//! available. The DSS collects that material *inside the document* so verification no longer needs
//! the network or the issuer's live infrastructure. [`Document::add_validation_info`] appends a
//! `/DSS` dictionary to the catalog as an incremental update (§7.5.6) — the original bytes, and so
//! every prior signature's `/ByteRange`, are preserved untouched — holding the certificates, OCSP
//! responses and CRLs as DER streams (Table 261), optionally with one VRI entry per signature
//! (Table 262) keyed by the SHA-1 of that signature's `/Contents`. [`Document::validation_info`]
//! reads it back, and [`Document::signature_vri_keys`] computes the VRI key for each signature so a
//! caller can target its validation material at a specific signature.

use std::collections::BTreeMap;

use pdf_cos::{Array, Dictionary, Name, Object, ObjectId, PdfString, Stream};
use pdf_crypto::{pdf_date, sha1};

use crate::signing::to_hex_upper;
use crate::{DocError, Document, Result};

/// DER-encoded long-term validation material for a DSS (ISO 32000-2 §12.8.4.3, Table 261): the
/// certificates, OCSP responses and CRLs that prove a signature's chain. Each `Vec<u8>` is one
/// DER object — an X.509 certificate (RFC 5280), an OCSP response (RFC 6960), or a CRL (RFC 5280).
#[derive(Clone, Debug, Default)]
pub struct ValidationData {
    /// DER-encoded X.509 certificates (the signer's, plus any intermediate/auxiliary certs).
    pub certs: Vec<Vec<u8>>,
    /// DER-encoded OCSP responses (`OCSPResponse`, RFC 6960).
    pub ocsps: Vec<Vec<u8>>,
    /// DER-encoded CRLs (`CertificateList`, RFC 5280).
    pub crls: Vec<Vec<u8>>,
}

/// Validation material for one specific signature, becoming a VRI entry (§12.8.4.4, Table 262).
/// The signature is identified by `key` — the uppercase base-16 SHA-1 of its `/Contents` hex
/// string, as returned by [`Document::signature_vri_keys`]. The material it references is also
/// folded into the document-wide DSS arrays (the spec requires VRI-referenced data to appear in
/// the DSS too).
#[derive(Clone, Debug)]
pub struct SignatureValidation {
    /// The VRI key: uppercase SHA-1 (hex) of the target signature's `/Contents` (§12.8.4.3 footnote).
    pub key: String,
    /// The certificates / OCSP responses / CRLs used to validate that one signature.
    pub data: ValidationData,
    /// When this VRI was created, as Unix seconds — emitted as the `/TU` date (§7.9.4); `None` omits it.
    pub created: Option<u64>,
    /// A DER-encoded RFC 3161 timestamp token over the VRI creation, emitted as the `/TS` stream
    /// (§12.8.4.4, Table 262); `None` omits it. `/TS` is the stronger sibling of `/TU`: a
    /// TSA-attested creation time rather than a self-declared one.
    pub timestamp_token: Option<Vec<u8>>,
}

/// The validation material read back from a document's `/DSS` (§12.8.4.3).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DssInfo {
    /// The decoded DER bytes of every `/Certs` stream.
    pub certs: Vec<Vec<u8>>,
    /// The decoded DER bytes of every `/OCSPs` stream.
    pub ocsps: Vec<Vec<u8>>,
    /// The decoded DER bytes of every `/CRLs` stream.
    pub crls: Vec<Vec<u8>>,
    /// The keys of the `/VRI` dictionary (uppercase SHA-1 hex of each covered signature's `/Contents`).
    pub vri_keys: Vec<String>,
}

impl Document {
    /// Append a Document Security Store (§12.8.4.3) as an incremental update, embedding long-term
    /// validation material so the document's signatures stay verifiable offline (LTV / PAdES-LT).
    ///
    /// `data` is the document-wide material that goes in the DSS `/Certs` / `/OCSPs` / `/CRLs`
    /// arrays. `vris` adds one VRI entry (§12.8.4.4) per signature: each [`SignatureValidation`]
    /// references the subset of material used for that signature, and that subset is also merged
    /// into the document-wide arrays (identical DER blobs are stored once and shared). Pass an
    /// empty `vris` for a DSS with no `/VRI` dictionary.
    ///
    /// The result reopens with `/DSS` on the catalog; the original bytes (and every signature's
    /// `/ByteRange`) are a verbatim prefix, so existing signatures still verify.
    pub fn add_validation_info(
        &self,
        data: &ValidationData,
        vris: &[SignatureValidation],
    ) -> Result<Vec<u8>> {
        let root = self.xref.root().ok_or(DocError::MissingCatalog)?;
        let mut next = self.max_object_number() + 1;
        let mut changed: Vec<(ObjectId, Object)> = Vec::new();

        // One stream object per *distinct* DER blob, shared between the document-wide arrays and any
        // VRI that references it (§12.8.4.3: VRI-referenced material is included in the DSS too).
        let mut certs = StreamPool::default();
        let mut ocsps = StreamPool::default();
        let mut crls = StreamPool::default();

        // Document-wide arrays first, so their order is stable and caller-controlled.
        intern_all(&mut certs, &data.certs, &mut next, &mut changed);
        intern_all(&mut ocsps, &data.ocsps, &mut next, &mut changed);
        intern_all(&mut crls, &data.crls, &mut next, &mut changed);

        // VRI entries: intern each one's material (sharing streams already pooled), then build a
        // VRI dictionary referencing exactly that subset.
        let mut vri_dict = Dictionary::new();
        for vri in vris {
            let cert_refs = intern_all(&mut certs, &vri.data.certs, &mut next, &mut changed);
            let ocsp_refs = intern_all(&mut ocsps, &vri.data.ocsps, &mut next, &mut changed);
            let crl_refs = intern_all(&mut crls, &vri.data.crls, &mut next, &mut changed);

            let mut vri_entry = Dictionary::new();
            vri_entry.insert(Name::from("Type"), Object::Name(Name::from("VRI")));
            insert_ref_array(&mut vri_entry, "Cert", cert_refs);
            insert_ref_array(&mut vri_entry, "CRL", crl_refs);
            insert_ref_array(&mut vri_entry, "OCSP", ocsp_refs);
            if let Some(created) = vri.created {
                vri_entry.insert(
                    Name::from("TU"),
                    Object::String(PdfString::from(pdf_date(created).into_bytes())),
                );
            }
            if let Some(token) = &vri.timestamp_token {
                // /TS (Table 262): the RFC 3161 timestamp token as a bare stream, DER verbatim.
                let ts_id = next_id(&mut next);
                changed.push((
                    ts_id,
                    Object::Stream(Stream::new(Dictionary::new(), token.clone())),
                ));
                vri_entry.insert(Name::from("TS"), Object::Reference(ts_id));
            }
            let entry_id = next_id(&mut next);
            changed.push((entry_id, Object::Dictionary(vri_entry)));
            vri_dict.insert(Name::from(vri.key.as_str()), Object::Reference(entry_id));
        }

        // The DSS dictionary itself (Table 261).
        let mut dss = Dictionary::new();
        dss.insert(Name::from("Type"), Object::Name(Name::from("DSS")));
        insert_ref_array(&mut dss, "Certs", certs.refs);
        insert_ref_array(&mut dss, "OCSPs", ocsps.refs);
        insert_ref_array(&mut dss, "CRLs", crls.refs);
        if !vris.is_empty() {
            let vri_id = next_id(&mut next);
            changed.push((vri_id, Object::Dictionary(vri_dict)));
            dss.insert(Name::from("VRI"), Object::Reference(vri_id));
        }
        let dss_id = next_id(&mut next);
        changed.push((dss_id, Object::Dictionary(dss)));

        // Point the catalog's /DSS at it (§7.7.2), via an incremental update.
        let Object::Dictionary(mut catalog) = self.get(root)? else {
            return Err(DocError::MissingCatalog);
        };
        catalog.insert(Name::from("DSS"), Object::Reference(dss_id));
        changed.push((root, Object::Dictionary(catalog)));

        self.save_incremental(&changed)
    }

    /// The VRI key for every signature in the document (§12.8.4.3, Table 261 footnote): the uppercase
    /// base-16 SHA-1 of the signature's `/Contents` hex string — the bytes of the complete hexadecimal
    /// string, including its zero padding. Returned in the same order as [`Document::verify_signatures`],
    /// so a caller can pair each key with the matching [`SignatureValidation`].
    pub fn signature_vri_keys(&self) -> Result<Vec<String>> {
        let mut out = Vec::new();
        let numbers: Vec<u32> = self.xref.entries.keys().copied().collect();
        for number in numbers {
            let Ok(Some(Object::Dictionary(dict))) = self.xref.fetch(&self.bytes, number) else {
                continue;
            };
            if dict.get(&Name::from("ByteRange")).is_none() {
                continue;
            }
            let Some(contents) = dict.get(&Name::from("Contents")) else {
                continue;
            };
            let Ok(Object::String(contents)) = self.resolve(contents) else {
                continue;
            };
            out.push(vri_key(contents.as_bytes()));
        }
        Ok(out)
    }

    /// Read back the document's `/DSS` (§12.8.4.3), or `None` if the catalog has none. Each stream's
    /// DER bytes are decoded through its filter chain; entries that cannot be resolved or decoded are
    /// skipped (best-effort, DESIGN.md §3.4).
    pub fn validation_info(&self) -> Result<Option<DssInfo>> {
        let root = self.xref.root().ok_or(DocError::MissingCatalog)?;
        let Object::Dictionary(catalog) = self.get(root)? else {
            return Err(DocError::MissingCatalog);
        };
        let Some(dss_ref) = catalog.get(&Name::from("DSS")) else {
            return Ok(None);
        };
        let Ok(Object::Dictionary(dss)) = self.resolve(dss_ref) else {
            return Ok(None);
        };
        let mut info = DssInfo {
            certs: self.read_der_streams(&dss, "Certs"),
            ocsps: self.read_der_streams(&dss, "OCSPs"),
            crls: self.read_der_streams(&dss, "CRLs"),
            vri_keys: Vec::new(),
        };
        if let Some(vri) = dss.get(&Name::from("VRI"))
            && let Ok(Object::Dictionary(vri)) = self.resolve(vri)
        {
            info.vri_keys = vri
                .iter()
                .filter_map(|(k, _)| k.as_str().map(str::to_string))
                .collect();
            info.vri_keys.sort();
        }
        Ok(Some(info))
    }

    /// Decode every stream referenced by `dict[key]` (a DSS array of stream refs) to its raw DER.
    fn read_der_streams(&self, dict: &Dictionary, key: &str) -> Vec<Vec<u8>> {
        let Some(value) = dict.get(&Name::from(key)) else {
            return Vec::new();
        };
        let Ok(Object::Array(array)) = self.resolve(value) else {
            return Vec::new();
        };
        array
            .iter()
            .filter_map(|item| match self.resolve(item).ok()? {
                Object::Stream(stream) => self.decode_stream(&stream).ok(),
                _ => None,
            })
            .collect()
    }
}

/// A pool of DER streams deduplicated by content: identical blobs become one shared stream object,
/// referenced from both the document-wide DSS array and any VRI that uses it (§12.8.4.3).
#[derive(Default)]
struct StreamPool {
    /// DER blob → its allocated object id.
    by_blob: BTreeMap<Vec<u8>, ObjectId>,
    /// References in document-wide array order (insertion order, deduplicated).
    refs: Vec<Object>,
}

impl StreamPool {
    /// Return the reference for `blob`, allocating + emitting its stream object on first sight.
    fn intern(
        &mut self,
        blob: &[u8],
        next: &mut u32,
        changed: &mut Vec<(ObjectId, Object)>,
    ) -> Object {
        if let Some(id) = self.by_blob.get(blob) {
            return Object::Reference(*id);
        }
        let id = next_id(next);
        self.by_blob.insert(blob.to_vec(), id);
        // A bare stream holding the DER verbatim — no filter, so a reader gets the bytes back as-is.
        let stream = Stream::new(Dictionary::new(), blob.to_vec());
        changed.push((id, Object::Stream(stream)));
        self.refs.push(Object::Reference(id));
        Object::Reference(id)
    }
}

/// Intern every blob in `blobs` into `pool`, returning their references (in order).
fn intern_all(
    pool: &mut StreamPool,
    blobs: &[Vec<u8>],
    next: &mut u32,
    changed: &mut Vec<(ObjectId, Object)>,
) -> Vec<Object> {
    blobs
        .iter()
        .map(|blob| pool.intern(blob, next, changed))
        .collect()
}

/// Allocate the next free object id (generation 0), bumping the counter.
fn next_id(next: &mut u32) -> ObjectId {
    let id = ObjectId::new(*next, 0);
    *next += 1;
    id
}

/// Insert `/key [refs…]` into `dict`, omitting the key entirely when there are no refs (the spec
/// says an empty Cert/CRL/OCSP array shall be omitted, §12.8.4.4).
fn insert_ref_array(dict: &mut Dictionary, key: &str, refs: Vec<Object>) {
    if !refs.is_empty() {
        dict.insert(Name::from(key), Object::Array(Array::from_vec(refs)));
    }
}

/// The VRI key for a signature's `/Contents`: uppercase base-16 of the SHA-1 of the complete
/// hexadecimal string (the bytes between `<` and `>`, including zero padding). The reader holds the
/// decoded `/Contents` bytes; re-encoding them as uppercase hex reproduces the on-disk hex string
/// Prism PDF emits (§12.8.4.3, Table 261 footnote).
fn vri_key(contents: &[u8]) -> String {
    let hex = to_hex_upper(contents);
    to_hex_upper(&sha1(hex.as_bytes()))
}
