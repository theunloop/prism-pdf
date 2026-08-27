//! Signing and verifying PDF documents (ISO 32000-1 §12.8).
//!
//! [`Document::sign`] appends a signature field plus a detached CMS signature as an incremental
//! update (§7.5.6): it lays out the new revision with a fixed-size `/Contents` placeholder and a
//! fixed-width `/ByteRange`, then computes the real `/ByteRange`, signs the bytes it covers, and
//! patches both in place. [`Document::sign_with`] additionally records a signing time (`/M` plus the
//! CMS `signingTime` attribute, §12.7.4.5), an optional visible appearance (§12.5.5), and an optional
//! RFC 3161 timestamp (PAdES-B). [`Document::verify_signatures`] does the inverse — recompute each
//! signature's covered bytes and check its CMS — and [`Document::verify_signatures_with`] also
//! validates the signer's certificate chain against a trust store. The cryptography is [`pdf_crypto`];
//! verification is total and panic-free (DESIGN.md §3.4).

use std::time::{SystemTime, UNIX_EPOCH};

use pdf_cos::syntax::escape_literal_string;
use pdf_cos::{Array, Dictionary, Name, Object, ObjectId, PdfString, Stream};
pub use pdf_crypto::TsaCredentials;
use pdf_crypto::{
    SignOptions, VerifyOptions, attach_pdf_mac_to_signature, make_timestamp_token, pdf_date,
    sha256, sign_digest_with, verify_detached_with, verify_timestamp_token,
};
use pdf_writer::{write_incremental_signed, write_incremental_signed_with_trailer};

use crate::{DocError, Document, Result};

/// Bytes reserved for the CMS signature in `/Contents` (so `2 × RESERVE` hex digits). An RSA-2048
/// `SignedData` with one certificate is well under this; a timestamp token roughly doubles it.
const RESERVE: usize = 16384;

/// Serial number stamped into the offline RFC 3161 token of a document timestamp (§12.8.5). Fixed so
/// `Document::timestamp` output is reproducible; a network TSA would assign its own.
const DTS_SERIAL: u64 = 1;

/// How a signature should be produced ([`Document::sign_with`], §12.8).
#[derive(Clone, Debug, Default)]
pub struct SignSettings {
    /// The signer's human-readable name, recorded as `/Name` and used in the default appearance.
    pub name: Option<String>,
    /// The stated reason for signing (`/Reason`, §12.7.4.5).
    pub reason: Option<String>,
    /// The signer's location (`/Location`).
    pub location: Option<String>,
    /// Signer contact information (`/ContactInfo`).
    pub contact_info: Option<String>,
    /// The signing time as Unix seconds (UTC); `None` uses the system clock at signing. Recorded as
    /// `/M` and as the CMS `signingTime` attribute, which agree.
    pub signing_time: Option<u64>,
    /// A visible appearance for the signature widget (§12.5.5); `None` leaves it invisible.
    pub appearance: Option<SignatureAppearance>,
    /// A local time-stamping authority for an embedded RFC 3161 timestamp (PAdES-B); `None` omits it.
    pub timestamp: Option<TsaCredentials>,
    /// Produce a **PAdES-B** signature (§12.8.3.3): emit `/SubFilter /ETSI.CAdES.detached` and add
    /// the `signing-certificate-v2` signed attribute binding the signer certificate (RFC 5035).
    pub pades: bool,
}

/// A visible signature appearance: where on the page it sits and what it shows.
#[derive(Clone, Debug)]
pub struct SignatureAppearance {
    /// Zero-based page index the signature appears on.
    pub page_index: usize,
    /// The widget rectangle `[x0 y0 x1 y1]` in default user space (§12.5.2).
    pub rect: [f32; 4],
    /// The text to draw; `None` derives a two-line label from the signer name and signing time.
    pub text: Option<String>,
}

/// Everything `apply_signature_revision` needs to lay out one signature revision, other than the
/// callback that produces the signature value.
struct SignatureRevision<'a> {
    /// The signature dictionary's opening tokens, up to but not including its text entries.
    sig_prefix: &'a [u8],
    /// `(key, value)` text entries for the signature dictionary. Kept as values rather than
    /// pre-written bytes because in an encrypted document each has to be encrypted under the
    /// signature object's number (§7.6.2), which is not allocated until layout.
    text_entries: &'a [(&'a str, String)],
    /// A visible appearance for the widget (§12.5.5), or `None` for an invisible signature.
    appearance: Option<&'a SignatureAppearance>,
    /// The signer name, used in the default appearance text.
    name: Option<&'a str>,
    /// The signing date as a PDF date string, used in the default appearance text.
    date: &'a str,
    /// Whether a PDF MAC token rides on this signature (ISO/TS 32004 §6.5.2).
    attach_mac: bool,
}

/// The result of verifying one signature in a document.
#[derive(Clone, Debug, PartialEq)]
pub struct SignatureStatus {
    /// Whether the signature is intact: its CMS verifies over exactly the bytes it covers (§12.8.1).
    pub valid: bool,
    /// The signer certificate's subject distinguished name, when available.
    pub signer: Option<String>,
    /// The number of bytes the signature's `/ByteRange` covers (the signed portion of the file).
    pub covered_bytes: usize,
    /// The cryptographically-bound signing time (CMS `signingTime`) as Unix seconds, when present.
    pub signing_time: Option<i64>,
    /// Whether the signer certificate chains to a trusted root: `Some(_)` when a trust store was
    /// supplied to [`Document::verify_signatures_with`], `None` otherwise.
    pub trusted: Option<bool>,
    /// The `genTime` of a verified embedded RFC 3161 timestamp, as Unix seconds, when present.
    pub timestamp_time: Option<i64>,
    /// Whether this is a **PAdES-B** signature: it carries a matching `signing-certificate-v2`
    /// signed attribute (RFC 5035), as produced by [`SignSettings::pades`].
    pub pades: bool,
    /// The certificate chain's revocation outcome (**PAdES-LT**), when revocation material was
    /// evaluated — populated by [`Document::verify_signatures_ltv`] from the document's `/DSS`
    /// (§12.8.4.3). `None` = revocation not evaluated.
    pub revocation: Option<pdf_crypto::RevocationSummary>,
}

impl Document {
    /// Sign the document (§12.8) with the RSA `key_der` (PKCS#8) and X.509 `cert_der`, returning the
    /// signed PDF as an incremental update. Adds an (invisible) signature field on the first page
    /// and an `/AcroForm` entry if there isn't one. The signature covers the entire file except its
    /// own `/Contents`, and records the signing time as both `/M` and the CMS `signingTime` attribute.
    pub fn sign(&self, cert_der: &[u8], key_der: &[u8]) -> Result<Vec<u8>> {
        self.sign_with(cert_der, key_der, &SignSettings::default())
    }

    /// As [`Document::sign`], but driven by [`SignSettings`]: a chosen signing time, signer metadata
    /// (`/Reason`/`/Location`/`/ContactInfo`/`/Name`), an optional visible appearance (§12.5.5), and
    /// an optional embedded RFC 3161 timestamp (PAdES-B).
    pub fn sign_with(
        &self,
        cert_der: &[u8],
        key_der: &[u8],
        settings: &SignSettings,
    ) -> Result<Vec<u8>> {
        self.sign_inner(cert_der, key_der, settings, None)
    }

    /// As [`Document::sign_with`], but also attaches a **PDF MAC token** to the signature
    /// (ISO/TS 32004 §6.5.2, `/MACLocation /AttachedToSig`): the document must already be AES-256
    /// (V5/R6) encrypted with a `KDFSalt` in `/Encrypt` (e.g. produced by
    /// [`Document::save_encrypted_with_mac`]), and `password` unlocks the file key the MAC keys on.
    /// The MAC binds both the signature's `ByteRange` and the signature value, so it survives only
    /// while neither is altered. Verify with [`Document::verify_pdf_mac`].
    pub fn sign_with_mac(
        &self,
        cert_der: &[u8],
        key_der: &[u8],
        settings: &SignSettings,
        password: &[u8],
    ) -> Result<Vec<u8>> {
        let material = self.mac_material(password)?;
        self.sign_inner(cert_der, key_der, settings, Some(material))
    }

    fn sign_inner(
        &self,
        cert_der: &[u8],
        key_der: &[u8],
        settings: &SignSettings,
        mac: Option<(Vec<u8>, Vec<u8>)>,
    ) -> Result<Vec<u8>> {
        let signing_time = settings.signing_time.unwrap_or_else(now_secs);
        let date = pdf_date(signing_time);

        // The /Sig dictionary prefix: the detached-CMS subfilter (PAdES `ETSI.CAdES.detached` when
        // requested, else `adbe.pkcs7.detached`) plus signer metadata (§12.8.1/§12.8.3.3).
        let subfilter: &[u8] = if settings.pades {
            b"/ETSI.CAdES.detached"
        } else {
            b"/adbe.pkcs7.detached"
        };
        let mut prefix = b"<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter ".to_vec();
        prefix.extend_from_slice(subfilter);
        // The text entries are handed over rather than written here: in an encrypted document each
        // value has to be encrypted under the signature object's own number (§7.6.2), which is not
        // allocated until the revision is laid out.
        let mut text: Vec<(&str, String)> = vec![("M", date.clone())];
        for (key, value) in [
            ("Name", &settings.name),
            ("Reason", &settings.reason),
            ("Location", &settings.location),
            ("ContactInfo", &settings.contact_info),
        ] {
            if let Some(value) = value {
                text.push((key, value.clone()));
            }
        }

        let cert = cert_der.to_vec();
        let key = key_der.to_vec();
        let timestamp = settings.timestamp.clone();
        let pades = settings.pades;
        let attach_mac = mac.is_some();
        let plan = SignatureRevision {
            sig_prefix: &prefix,
            text_entries: &text,
            appearance: settings.appearance.as_ref(),
            name: settings.name.as_deref(),
            date: &date,
            attach_mac,
        };
        self.apply_signature_revision(&plan, move |message| {
            let cms = sign_digest_with(
                message,
                &cert,
                &key,
                &SignOptions {
                    signing_time: Some(signing_time),
                    timestamp,
                    pades,
                },
            )?;
            // Attach the PDF MAC (§6.6.3): dataDigest over the ByteRange (= `message`), and
            // signatureDigest derived inside from the signer's signature value.
            match &mac {
                Some((file_key, kdf_salt)) => {
                    attach_pdf_mac_to_signature(&cms, file_key, kdf_salt, &sha256(message))
                }
                None => Some(cms),
            }
        })
    }

    /// Apply a **document timestamp** (DTS, §12.8.5 — PAdES document-timestamp): append an invisible
    /// signature field whose value is a `/DocTimeStamp` dictionary (`/SubFilter /ETSI.RFC3161`)
    /// covering the whole file, with an RFC 3161 timestamp token over the `/ByteRange` bytes as its
    /// `/Contents`. The token is minted from the TSA credentials `tsa_cert_der` (X.509 DER) and
    /// `tsa_key_der` (PKCS#8 DER) at `gen_time` (Unix seconds; `None` = now), so the path is testable
    /// offline; a production deployment would fetch an equivalent token from a network TSA. The result
    /// is an incremental update, verifiable via [`Document::verify_signatures`].
    pub fn timestamp(
        &self,
        tsa_cert_der: &[u8],
        tsa_key_der: &[u8],
        gen_time: Option<u64>,
    ) -> Result<Vec<u8>> {
        let gen_at = gen_time.unwrap_or_else(now_secs);
        let prefix =
            b"<< /Type /DocTimeStamp /Filter /Adobe.PPKLite /SubFilter /ETSI.RFC3161".to_vec();
        let cert = tsa_cert_der.to_vec();
        let key = tsa_key_der.to_vec();
        let plan = SignatureRevision {
            sig_prefix: &prefix,
            text_entries: &[],
            appearance: None,
            name: None,
            date: "",
            attach_mac: false,
        };
        self.apply_signature_revision(&plan, move |message| {
            make_timestamp_token(message, &cert, &key, gen_at, DTS_SERIAL)
        })
    }

    /// Append a signature-field revision (§12.7.4.5): host an (optionally visible) widget on a page,
    /// register it with the `/AcroForm`, and lay out the signature value object from `sig_prefix`
    /// with `/ByteRange`/`/Contents` placeholders, then fill `/Contents` from `produce` (the detached
    /// CMS for a `/Sig`, or the RFC 3161 token for a `/DocTimeStamp`). Shared by `sign_with` /
    /// `timestamp`.
    fn apply_signature_revision(
        &self,
        plan: &SignatureRevision<'_>,
        produce: impl FnOnce(&[u8]) -> Option<Vec<u8>>,
    ) -> Result<Vec<u8>> {
        let &SignatureRevision {
            sig_prefix,
            text_entries,
            appearance,
            name,
            date,
            attach_mac,
        } = plan;
        let root = self.xref.root().ok_or(DocError::MissingCatalog)?;
        let info = self
            .xref
            .trailer
            .get(&Name::from("Info"))
            .and_then(Object::as_reference);

        let mut next = self.max_object_number() + 1;
        let sig_id = ObjectId::new(next, 0);
        next += 1;
        let field_id = ObjectId::new(next, 0);
        next += 1;

        let mut changed: Vec<(ObjectId, Object)> = Vec::new();

        // Build the (optional) visible appearance: a Form XObject plus a Helvetica font (§12.5.5).
        let (rect, appearance_ref) = match appearance {
            Some(ap) => {
                let xobject_id = ObjectId::new(next, 0);
                next += 1;
                let font_id = ObjectId::new(next, 0);
                next += 1;
                let lines = appearance_lines(ap, name, date);
                let (width, height) = (ap.rect[2] - ap.rect[0], ap.rect[3] - ap.rect[1]);
                changed.push((
                    xobject_id,
                    Object::Stream(appearance_xobject(width, height, font_id, &lines)),
                ));
                changed.push((font_id, Object::Dictionary(helvetica_font())));
                (ap.rect, Some(xobject_id))
            }
            None => {
                // Even an invisible signature widget carries an (empty, zero-size) appearance:
                // PDF/A-1 §6.9 requires every form field to have one, and it is harmless elsewhere.
                let xobject_id = ObjectId::new(next, 0);
                next += 1;
                changed.push((xobject_id, Object::Stream(empty_appearance_xobject())));
                ([0.0; 4], Some(xobject_id))
            }
        };

        let target_page = appearance.map_or(0, |ap| ap.page_index);
        let page_id = self
            .page_entries()?
            .get(target_page)
            .and_then(|(id, _)| *id)
            .ok_or(DocError::SigningFailed)?;

        // The signature field, merged with its widget annotation (§12.7.4.5).
        changed.push((
            field_id,
            Object::Dictionary(signature_field(sig_id, page_id, rect, appearance_ref)),
        ));

        // Host the widget on the target page's /Annots.
        let Object::Dictionary(mut page) = self.get(page_id)? else {
            return Err(DocError::SigningFailed);
        };
        let mut annots = self.annots_of(&page)?;
        annots.push(Object::Reference(field_id));
        page.insert(Name::from("Annots"), Object::Array(Array::from_vec(annots)));
        changed.push((page_id, Object::Dictionary(page)));

        // Register the field with the AcroForm (existing — referenced or inline — or a new one).
        let Object::Dictionary(mut catalog) = self.get(root)? else {
            return Err(DocError::MissingCatalog);
        };
        match catalog.get(&Name::from("AcroForm")).cloned() {
            Some(Object::Reference(acro_id)) => {
                if let Object::Dictionary(mut acroform) = self.get(acro_id)? {
                    self.add_signature_field(&mut acroform, field_id);
                    changed.push((acro_id, Object::Dictionary(acroform)));
                }
            }
            Some(Object::Dictionary(mut acroform)) => {
                self.add_signature_field(&mut acroform, field_id);
                catalog.insert(Name::from("AcroForm"), Object::Dictionary(acroform));
                changed.push((root, Object::Dictionary(catalog)));
            }
            _ => {
                let acro_id = ObjectId::new(next, 0);
                next += 1;
                changed.push((acro_id, Object::Dictionary(new_acroform(field_id))));
                catalog.insert(Name::from("AcroForm"), Object::Reference(acro_id));
                changed.push((root, Object::Dictionary(catalog)));
            }
        }

        // §7.6.2 applies encryption to *every* string and stream in an encrypted file, with only
        // four exceptions — the trailer `/ID`, strings in the `/Encrypt` dictionary, strings already
        // inside an encrypted stream, and the signature `/Contents` hex string (the last added by
        // PDF 2.0). An incremental update is part of the same file, so everything this revision
        // writes has to be encrypted with the document's own key before it is laid out.
        if let Some((handler, _)) = self.security() {
            for (id, object) in &mut changed {
                let (number, generation) = (id.number, id.generation);
                *object = crate::encryption::encrypt_object(object, &|data| {
                    handler.encrypt(number, generation, data)
                })
                .ok_or(DocError::RandomUnavailable)?;
            }
        }

        // The signature value object: the caller's dictionary prefix, its text entries, then the
        // placeholders to patch after layout (the real /ByteRange offsets and the /Contents DER).
        let mut sig_body = sig_prefix.to_vec();
        for (key, value) in text_entries {
            let bytes = match self.security() {
                // Encrypted under the signature object's own number/generation, like any other
                // string in the file. `/Contents` below is the one entry that stays in the clear.
                Some((handler, _)) => handler
                    .encrypt(sig_id.number, sig_id.generation, value.as_bytes())
                    .ok_or(DocError::RandomUnavailable)?,
                None => value.as_bytes().to_vec(),
            };
            write_text_entry(&mut sig_body, key, &bytes);
        }
        sig_body.extend_from_slice(b" /ByteRange [0 0000000000 0000000000 0000000000] /Contents <");
        sig_body.extend(std::iter::repeat_n(b'0', RESERVE * 2));
        sig_body.extend_from_slice(b"> >>");

        let size = self.trailer_size().max(u64::from(next));
        let mut bytes = if attach_mac {
            // The PDF MAC token rides as an unsigned attribute on this signature; point the
            // trailer's /AuthCode at the signature dictionary (ISO/TS 32004 §5.2.3, §6.5.2).
            // /Encrypt is carried into the added trailer by the writer, for every incremental
            // revision (§7.5.6), so it must not be repeated here — but a MAC still requires the
            // document to be encrypted in the first place.
            if self.security().is_none() {
                return Err(DocError::MacRequiresV5);
            }
            let authcode = format!(
                " /AuthCode << /MACLocation /AttachedToSig /SigObjRef {} 0 R >>",
                sig_id.number
            );
            write_incremental_signed_with_trailer(
                &self.bytes,
                &changed,
                (sig_id, &sig_body),
                root,
                info,
                size,
                &authcode,
            )
        } else {
            write_incremental_signed(&self.bytes, &changed, (sig_id, &sig_body), root, info, size)
        };
        patch_contents(&mut bytes, produce)?;
        Ok(bytes)
    }

    /// Verify every signature in the document (§12.8.1) without a trust store: for each signature
    /// dictionary, recompute the bytes its `/ByteRange` covers and check the detached CMS in
    /// `/Contents`. [`SignatureStatus::trusted`] is left `None`.
    pub fn verify_signatures(&self) -> Result<Vec<SignatureStatus>> {
        self.verify_signatures_with(&[])
    }

    /// As [`Document::verify_signatures`], but additionally chaining each signer's certificate to one
    /// of the supplied trusted `roots` (DER X.509), populating [`SignatureStatus::trusted`] (PAdES-B).
    pub fn verify_signatures_with(&self, roots: &[Vec<u8>]) -> Result<Vec<SignatureStatus>> {
        let options = VerifyOptions {
            roots: roots.to_vec(),
            ..Default::default()
        };
        self.verify_signatures_opts(&options)
    }

    /// **PAdES-LT** verification (§12.8.4.3): as [`Document::verify_signatures_with`], but also
    /// checking each chain link's revocation against the OCSP responses and CRLs embedded in the
    /// document's `/DSS` — the long-term-validation promise: the file itself carries the evidence,
    /// so no network is needed. [`SignatureStatus::revocation`] reports the outcome per signature
    /// (`Good` / `Revoked` / `Incomplete`); with no `/DSS` present every chain is `Incomplete`.
    pub fn verify_signatures_ltv(&self, roots: &[Vec<u8>]) -> Result<Vec<SignatureStatus>> {
        let dss = self.validation_info()?.unwrap_or_default();
        let options = VerifyOptions {
            roots: roots.to_vec(),
            revocation: Some(pdf_crypto::RevocationData {
                ocsps: dss.ocsps,
                crls: dss.crls,
            }),
        };
        self.verify_signatures_opts(&options)
    }

    /// The shared signature-enumeration loop behind the `verify_signatures*` frontends.
    fn verify_signatures_opts(&self, options: &VerifyOptions) -> Result<Vec<SignatureStatus>> {
        let mut out = Vec::new();
        let numbers: Vec<u32> = self.xref.entries.keys().copied().collect();
        for number in numbers {
            let Ok(Some(Object::Dictionary(dict))) = self.xref.fetch(&self.bytes, number) else {
                continue;
            };
            // A signature dictionary is identified by carrying both /ByteRange and /Contents.
            if dict.get(&Name::from("ByteRange")).is_none()
                || dict.get(&Name::from("Contents")).is_none()
            {
                continue;
            }
            if let Some(status) = self.verify_signature_dict(&dict, options) {
                out.push(status);
            }
        }
        Ok(out)
    }

    /// Verify one signature dictionary, or `None` if its `/ByteRange`/`/Contents` are unusable.
    fn verify_signature_dict(
        &self,
        sig: &Dictionary,
        options: &VerifyOptions,
    ) -> Option<SignatureStatus> {
        let range = self.float_ints(sig.get(&Name::from("ByteRange"))?)?;
        let [a, b, c, d] = range[..4].try_into().ok()?;
        let (a, b, c, d) = (a as usize, b as usize, c as usize, d as usize);
        let end1 = a.checked_add(b)?;
        let end2 = c.checked_add(d)?;
        if end1 > self.bytes.len() || end2 > self.bytes.len() || c < end1 {
            return None;
        }
        let mut message = Vec::with_capacity(b + d);
        message.extend_from_slice(&self.bytes[a..end1]);
        message.extend_from_slice(&self.bytes[c..end2]);

        let Object::String(contents) = self.resolve(sig.get(&Name::from("Contents"))?).ok()? else {
            return None;
        };
        // The /Contents bytes are the DER blob padded with trailing zeros; parse exactly the DER.
        let der = trim_to_der(contents.as_bytes());
        // A document timestamp (§12.8.5) carries an RFC 3161 token, not a detached CMS over the
        // message — its imprint is SHA-256 of the /ByteRange bytes. Route by /SubFilter (or /Type).
        let is_dts = sig.get_name(&Name::from("SubFilter")).map(Name::as_bytes)
            == Some(&b"ETSI.RFC3161"[..])
            || sig.get_name(&Name::from("Type")).map(Name::as_bytes) == Some(&b"DocTimeStamp"[..]);
        let verified = if is_dts {
            verify_timestamp_token(der, &message, options)
        } else {
            verify_detached_with(der, &message, options)
        };
        Some(SignatureStatus {
            valid: verified.valid,
            signer: verified.signer,
            covered_bytes: b + d,
            signing_time: verified.signing_time,
            trusted: verified.trusted,
            timestamp_time: verified.timestamp_time,
            pades: verified.pades,
            revocation: verified.revocation,
        })
    }

    /// Add the signature field to an AcroForm dictionary (`/Fields` + `/SigFlags`, §12.7.4.5).
    fn add_signature_field(&self, acroform: &mut Dictionary, field_id: ObjectId) {
        let mut fields = match acroform.get(&Name::from("Fields")) {
            Some(value) => match self.resolve(value) {
                Ok(Object::Array(array)) => array.iter().cloned().collect(),
                _ => Vec::new(),
            },
            None => Vec::new(),
        };
        fields.push(Object::Reference(field_id));
        acroform.insert(Name::from("Fields"), Object::Array(Array::from_vec(fields)));
        acroform.insert(Name::from("SigFlags"), Object::Integer(3));
    }

    /// The page's existing `/Annots` as an owned vector (resolving an indirect array), or empty.
    fn annots_of(&self, page: &Dictionary) -> Result<Vec<Object>> {
        Ok(match page.get(&Name::from("Annots")) {
            Some(value) => match self.resolve(value)? {
                Object::Array(array) => array.iter().cloned().collect(),
                _ => Vec::new(),
            },
            None => Vec::new(),
        })
    }

    /// The highest in-use object number (§7.5), for allocating new ones.
    pub(crate) fn max_object_number(&self) -> u32 {
        self.xref.entries.keys().copied().max().unwrap_or(0)
    }

    /// The trailer `/Size` (§7.5.5), or 0 if absent.
    fn trailer_size(&self) -> u64 {
        self.xref
            .trailer
            .get(&Name::from("Size"))
            .and_then(Object::as_integer)
            .and_then(|n| u64::try_from(n).ok())
            .unwrap_or(0)
    }

    /// Resolve `value` to a vector of integers (a `/ByteRange`-style numeric array).
    fn float_ints(&self, value: &Object) -> Option<Vec<i64>> {
        match self.resolve(value).ok()? {
            Object::Array(array) => Some(
                array
                    .iter()
                    .filter_map(|x| self.resolve(x).ok()?.as_integer())
                    .collect(),
            ),
            _ => None,
        }
    }
}

/// The current time as Unix seconds, or 0 if the clock is unavailable.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The signature field dictionary (§12.7.4.5), merged with a widget annotation. `rect` is the widget
/// rectangle and `appearance` an optional `/AP /N` Form XObject (a visible signature, §12.5.5).
fn signature_field(
    sig_id: ObjectId,
    page_id: ObjectId,
    rect: [f32; 4],
    appearance: Option<ObjectId>,
) -> Dictionary {
    let mut field = Dictionary::new();
    field.insert(Name::from("FT"), Object::Name(Name::from("Sig")));
    field.insert(Name::from("Type"), Object::Name(Name::from("Annot")));
    field.insert(Name::from("Subtype"), Object::Name(Name::from("Widget")));
    field.insert(
        Name::from("T"),
        Object::String(PdfString::from(b"Signature1".to_vec())),
    );
    field.insert(Name::from("V"), Object::Reference(sig_id));
    field.insert(Name::from("F"), Object::Integer(132)); // Print + Locked
    field.insert(
        Name::from("Rect"),
        Object::Array(Array::from_vec(
            rect.iter().map(|&v| Object::Real(f64::from(v))).collect(),
        )),
    );
    field.insert(Name::from("P"), Object::Reference(page_id));
    if let Some(ap_id) = appearance {
        let mut ap = Dictionary::new();
        ap.insert(Name::from("N"), Object::Reference(ap_id));
        field.insert(Name::from("AP"), Object::Dictionary(ap));
    }
    field
}

/// A fresh AcroForm holding a single signature field, with signatures-exist + append-only flags.
fn new_acroform(field_id: ObjectId) -> Dictionary {
    let mut acroform = Dictionary::new();
    acroform.insert(
        Name::from("Fields"),
        Object::Array(Array::from_vec(vec![Object::Reference(field_id)])),
    );
    acroform.insert(Name::from("SigFlags"), Object::Integer(3));
    acroform
}

/// The text lines for a visible appearance: the caller's override, or a default derived from the
/// signer name and signing date.
fn appearance_lines(ap: &SignatureAppearance, name: Option<&str>, date: &str) -> Vec<String> {
    match &ap.text {
        Some(text) => text.lines().map(str::to_string).collect(),
        None => vec![
            match name {
                Some(name) => format!("Digitally signed by {name}"),
                None => "Digitally signed".to_string(),
            },
            format!("Date: {date}"),
        ],
    }
}

/// An empty zero-size appearance Form XObject for an **invisible** signature widget: it draws
/// nothing, but satisfies the PDF/A requirement (ISO 19005-1 §6.9) that every form field carry an
/// appearance dictionary.
fn empty_appearance_xobject() -> Stream {
    let mut dict = Dictionary::new();
    dict.insert(Name::from("Type"), Object::Name(Name::from("XObject")));
    dict.insert(Name::from("Subtype"), Object::Name(Name::from("Form")));
    dict.insert(Name::from("FormType"), Object::Integer(1));
    dict.insert(
        Name::from("BBox"),
        Object::Array(Array::from_vec(vec![Object::Real(0.0); 4])),
    );
    Stream::new(dict, Vec::new())
}

/// Build the appearance Form XObject (§12.5.5 / §8.10): a `/BBox`-bounded box drawing `lines` in
/// Helvetica. The widget maps this BBox onto its `/Rect`.
fn appearance_xobject(width: f32, height: f32, font_id: ObjectId, lines: &[String]) -> Stream {
    let mut dict = Dictionary::new();
    dict.insert(Name::from("Type"), Object::Name(Name::from("XObject")));
    dict.insert(Name::from("Subtype"), Object::Name(Name::from("Form")));
    dict.insert(Name::from("FormType"), Object::Integer(1));
    dict.insert(
        Name::from("BBox"),
        Object::Array(Array::from_vec(vec![
            Object::Real(0.0),
            Object::Real(0.0),
            Object::Real(f64::from(width)),
            Object::Real(f64::from(height)),
        ])),
    );
    let mut fonts = Dictionary::new();
    fonts.insert(Name::from("Helv"), Object::Reference(font_id));
    let mut resources = Dictionary::new();
    resources.insert(Name::from("Font"), Object::Dictionary(fonts));
    dict.insert(Name::from("Resources"), Object::Dictionary(resources));

    // Draw each line top-to-bottom with a fixed 10-unit leading (§9.4 text operators).
    let mut content = Vec::new();
    content.extend_from_slice(b"BT\n/Helv 8 Tf\n10 TL\n");
    content.extend_from_slice(format!("2 {:.2} Td\n", (height - 10.0).max(0.0)).as_bytes());
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            content.extend_from_slice(b"T*\n");
        }
        content.push(b'(');
        escape_literal_string(line.as_bytes(), &mut content);
        content.extend_from_slice(b") Tj\n");
    }
    content.extend_from_slice(b"ET");
    Stream::new(dict, content)
}

/// The standard-14 Helvetica font object (§9.6.2.2), referenced by the appearance stream.
fn helvetica_font() -> Dictionary {
    let mut font = Dictionary::new();
    font.insert(Name::from("Type"), Object::Name(Name::from("Font")));
    font.insert(Name::from("Subtype"), Object::Name(Name::from("Type1")));
    font.insert(
        Name::from("BaseFont"),
        Object::Name(Name::from("Helvetica")),
    );
    font
}

/// Append ` /Key (value)` to a hand-built dictionary body, escaping the literal string (§7.3.4.2).
/// Write ` /Key (value)` into a dictionary body. `value` is raw bytes rather than text because in
/// an encrypted document it is ciphertext by the time it reaches here; `escape_literal_string`
/// octal-escapes anything outside the printable range, so binary is safe in a literal string
/// (§7.3.4.2).
fn write_text_entry(out: &mut Vec<u8>, key: &str, value: &[u8]) {
    out.push(b' ');
    out.push(b'/');
    out.extend_from_slice(key.as_bytes());
    out.extend_from_slice(b" (");
    escape_literal_string(value, out);
    out.push(b')');
}

/// Patch the laid-out signature: compute the real `/ByteRange`, then call `produce` with the bytes it
/// covers to obtain the DER blob (a detached CMS for a `/Sig`, or an RFC 3161 token for a
/// `/DocTimeStamp`) and write its hex into `/Contents`.
fn patch_contents(bytes: &mut [u8], produce: impl FnOnce(&[u8]) -> Option<Vec<u8>>) -> Result<()> {
    // Locate the /Contents <…> placeholder of the revision just appended. Earlier revisions may
    // contain their own (already patched) signatures — e.g. a document timestamp added over a
    // signed + DSS'd file (PAdES-LTA) — so take the **last** occurrence, which necessarily
    // belongs to the new revision at the end of the file.
    let marker = b"/Contents <";
    let lt = rfind(bytes, marker).ok_or(DocError::SigningFailed)? + marker.len() - 1; // index of '<'
    let hex_start = lt + 1;
    let hex_end = hex_start + RESERVE * 2; // index of '>'
    if bytes.get(hex_end) != Some(&b'>') {
        return Err(DocError::SigningFailed);
    }
    let after = hex_end + 1; // first byte after '>'
    let total = bytes.len();

    // ByteRange [0 lt after (total-after)] — the gap [lt, after) is exactly "<…>" (excluded).
    // Earlier signatures' /ByteRange entries are already patched (non-zero), but take the last
    // all-zero placeholder anyway for symmetry with the /Contents search.
    let placeholder = b"/ByteRange [0 0000000000 0000000000 0000000000]";
    let br = rfind(bytes, placeholder).ok_or(DocError::SigningFailed)?;
    let new_range = format!(
        "/ByteRange [0 {:010} {:010} {:010}]",
        lt,
        after,
        total - after
    );
    if new_range.len() != placeholder.len() {
        return Err(DocError::SigningFailed); // offsets too large for the reserved width
    }
    bytes[br..br + placeholder.len()].copy_from_slice(new_range.as_bytes());

    // The covered bytes are everything except the "<…>" content.
    let mut message = Vec::with_capacity(lt + (total - after));
    message.extend_from_slice(&bytes[..lt]);
    message.extend_from_slice(&bytes[after..]);
    let der = produce(&message).ok_or(DocError::SigningFailed)?;

    let hex = to_hex_upper(&der);
    if hex.len() > RESERVE * 2 {
        return Err(DocError::SigningFailed); // blob larger than the reserved space
    }
    bytes[hex_start..hex_start + hex.len()].copy_from_slice(hex.as_bytes());
    Ok(())
}

/// The first occurrence of `needle` in `haystack`.
pub(crate) fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// The last occurrence of `needle` in `haystack`.
fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).rposition(|w| w == needle)
}

/// Uppercase hex encoding.
pub(crate) fn to_hex_upper(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push_str(&format!("{b:02X}"));
    }
    out
}

/// Trim trailing padding from a `/Contents` value to exactly the leading DER object (§12.8.1): read
/// the ASN.1 length of the outer TLV and slice to it. Returns the input unchanged if unparseable.
pub(crate) fn trim_to_der(bytes: &[u8]) -> &[u8] {
    der_object_len(bytes)
        .filter(|&n| n <= bytes.len())
        .map_or(bytes, |n| &bytes[..n])
}

/// Total length (header + content) of the ASN.1 TLV at the start of `bytes`, or `None`.
fn der_object_len(bytes: &[u8]) -> Option<usize> {
    let length = *bytes.get(1)?;
    if length < 0x80 {
        return Some(2 + length as usize); // short form
    }
    let n = (length & 0x7F) as usize;
    if n == 0 || n > 4 {
        return None; // indefinite or implausibly long
    }
    let mut content = 0usize;
    for &byte in bytes.get(2..2 + n)? {
        content = (content << 8) | byte as usize;
    }
    Some(2 + n + content)
}

#[cfg(test)]
mod tests {
    use super::write_text_entry;

    #[test]
    fn signing_text_entries_use_safe_literal_string_escapes() {
        // §7.3.4.2: metadata in the hand-built signature dictionary must not inject raw line
        // breaks or change literal-string nesting.
        let mut out = Vec::new();
        write_text_entry(&mut out, "Reason", b"first\n(second)\\\t\0");
        assert_eq!(out, b" /Reason (first\\n\\(second\\)\\\\\\t\\000)");
    }
}
