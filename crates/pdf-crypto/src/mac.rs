//! PDF MAC — integrity protection for encrypted documents (ISO/TS 32004:2024).
//!
//! A *PDF MAC token* is a CMS `AuthenticatedData` object (RFC 5652 §9.1) that authenticates a
//! digest of the document's `ByteRange` with an HMAC keyed by material derived from the **file
//! encryption key**. A valid MAC therefore proves knowledge of that key — a property digital
//! signatures (which use public-key cryptography) do not have. The mechanism is backwards
//! compatible with ISO 32000-2 and may coexist with signatures (TS 32004, Introduction).
//!
//! The token's encapsulated content is a DER `PdfMacIntegrityInfo` (§6.2): the `dataDigest`
//! over the `ByteRange`, plus an optional `signatureDigest` when the token is attached to a
//! signature (§6.6.3). The MAC key is a fresh random value, AES-256 key-wrapped (RFC 3394, no
//! padding, §6.3.3) under a key-encryption key obtained from `pdfMacWrapKdf` — HKDF-SHA256 with
//! `info = "PDFMAC"` and the document's `KDFSalt` (§6.4). The MAC itself is HMAC-SHA256 over the
//! DER encoding of the authenticated attributes (RFC 5652 §9.2), which carry the content-type,
//! message-digest and CMS algorithm-protection (RFC 6211, §6.3.6) attributes.
//!
//! This module is the cryptographic core (compose + verify of the token, the KDF, and the AES
//! key wrap). Locating/embedding the token in a PDF — the `AuthCode` dictionary, `ByteRange` and
//! `MAC` trailer entries (§5.2.3, §6.5) — is the writer's job. Input is untrusted (DESIGN.md
//! §3.4): [`verify_pdf_mac_token`] never panics; any malformed structure yields `false`.

use aes::Aes256;
use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use der::asn1::{OctetString, SetOfVec};
use der::oid::ObjectIdentifier;
use der::{Any, Decode, Encode, Sequence};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use x509_cert::attr::Attribute;
use x509_cert::spki::AlgorithmIdentifierOwned;

use cms::authenticated_data::AuthenticatedData;
use cms::content_info::{CmsVersion, ContentInfo};
use cms::enveloped_data::{PasswordRecipientInfo, RecipientInfo, RecipientInfos};
use cms::signed_data::{EncapsulatedContentInfo, SignedData, SignerInfo, SignerInfos};

/// `id-ct-pdfMacIntegrityInfo` — { iso(1) standard(0) iso32004(32004) pdfmac(1) 0 } (TS 32004 §6.2).
const ID_CT_PDF_MAC_INTEGRITY_INFO: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("1.0.32004.1.0");
/// `id-kdf-pdfMacWrapKdf` — { iso32004 pdfmac 1 } (TS 32004 §6.4): the HKDF-SHA256 KDF.
const ID_KDF_PDF_MAC_WRAP_KDF: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.0.32004.1.1");
/// `id-ct-authData` (RFC 5652 §9.1) — the content type of a CMS `AuthenticatedData`.
const ID_CT_AUTH_DATA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.1.2");
/// `id-aes256-wrap` (NIST CSOR / TS 32004 §6.1) — AES-256 key wrap without padding.
const ID_AES256_WRAP: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.1.45");
/// `hmacWithSHA256` (RFC 4231) — the MAC algorithm (TS 32004 Table 9).
const ID_HMAC_SHA256: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.2.9");
/// `id-sha256` — the digest algorithm (TS 32004 Table 8).
const ID_SHA256: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.2.1");
/// `id-contentType` signed attribute (RFC 5652 §11.1).
const ID_CONTENT_TYPE: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.3");
/// `id-messageDigest` signed attribute (RFC 5652 §11.2).
const ID_MESSAGE_DIGEST: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.4");
/// `id-aa-CMSAlgorithmProtection` (RFC 6211 §4) — binds the digest/MAC algorithms (§6.3.6.4).
const ID_AA_CMS_ALGORITHM_PROTECTION: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.52");
/// `id-attr-pdfMacData` — { iso32004 pdfmac 2 } (TS 32004 §6.5.2): the unsigned attribute that
/// carries a PDF MAC token attached to a digital signature.
const ID_ATTR_PDF_MAC_DATA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.0.32004.1.2");
/// `id-signedData` (RFC 5652 §5.1) — the content type of a CMS signature.
const ID_SIGNED_DATA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.2");

type HmacSha256 = Hmac<Sha256>;

/// `PdfMacIntegrityInfo` (TS 32004 §6.2) — the encapsulated content of a PDF MAC token.
///
/// ```text
/// PdfMacIntegrityInfo ::= SEQUENCE {
///     version INTEGER,                                -- shall be 0
///     dataDigest OCTET STRING,                        -- digest over the PDF ByteRange
///     signatureDigest [0] IMPLICIT OCTET STRING OPTIONAL }  -- only in a signed container
/// ```
#[derive(Sequence)]
struct PdfMacIntegrityInfo {
    version: u8,
    data_digest: OctetString,
    #[asn1(context_specific = "0", tag_mode = "IMPLICIT", optional = "true")]
    signature_digest: Option<OctetString>,
}

/// `CMSAlgorithmProtection` (RFC 6211 §4) — for a MAC, the digest and MAC algorithms are set
/// (the `[1]` signature algorithm is absent).
#[derive(Sequence)]
struct CmsAlgorithmProtection {
    digest_algorithm: AlgorithmIdentifierOwned,
    #[asn1(
        context_specific = "1",
        tag_mode = "IMPLICIT",
        constructed = "true",
        optional = "true"
    )]
    signature_algorithm: Option<AlgorithmIdentifierOwned>,
    #[asn1(
        context_specific = "2",
        tag_mode = "IMPLICIT",
        constructed = "true",
        optional = "true"
    )]
    mac_algorithm: Option<AlgorithmIdentifierOwned>,
}

fn algid(oid: ObjectIdentifier) -> AlgorithmIdentifierOwned {
    AlgorithmIdentifierOwned {
        oid,
        parameters: None,
    }
}

/// `pdfMacWrapKdf` (TS 32004 §6.4): HKDF-SHA256 with `info = "PDFMAC"` over the file encryption
/// key, producing the 256-bit key-encryption key for the AES key wrap. `salt` is the document's
/// 32-byte `KDFSalt`. `None` only on the (unreachable for a 32-octet output) HKDF length error.
#[must_use]
pub fn pdf_mac_wrap_kdf(file_key: &[u8], salt: &[u8]) -> Option<[u8; 32]> {
    let hk = Hkdf::<Sha256>::new(Some(salt), file_key);
    let mut okm = [0u8; 32];
    hk.expand(b"PDFMAC", &mut okm).ok()?;
    Some(okm)
}

// --- AES-256 key wrap (RFC 3394), the algorithm `id-aes256-wrap` identifies (TS 32004 §6.3.3) ---

/// The RFC 3394 default initial value (the "integrity check register").
const KW_IV: [u8; 8] = [0xA6; 8];

/// Copy a length-8 slice into an array. Its one caller passes the 8-byte IV prefix of a
/// length-checked ciphertext, so the source is always at least 8 bytes; the rest is ignored.
fn eight(c: &[u8]) -> [u8; 8] {
    let mut a = [0u8; 8];
    a.copy_from_slice(&c[..8]);
    a
}

/// Wrap `plaintext` (a multiple of 8 bytes, ≥ 16) under the 256-bit `kek` per RFC 3394 §2.2.1.
fn aes256_key_wrap(kek: &[u8; 32], plaintext: &[u8]) -> Option<Vec<u8>> {
    let n = plaintext.len() / 8;
    if !plaintext.len().is_multiple_of(8) || n < 2 {
        return None;
    }
    let cipher = Aes256::new(GenericArray::from_slice(kek));
    let mut a = KW_IV;
    let mut r: Vec<[u8; 8]> = plaintext.as_chunks::<8>().0.to_vec();
    for j in 0..6u64 {
        for (i, ri) in r.iter_mut().enumerate() {
            let mut block = [0u8; 16];
            block[..8].copy_from_slice(&a);
            block[8..].copy_from_slice(ri);
            cipher.encrypt_block(GenericArray::from_mut_slice(&mut block));
            // A = MSB64(B) XOR t,  t = (n * j) + (i + 1);  R[i] = LSB64(B).
            let t = n as u64 * j + i as u64 + 1;
            a.copy_from_slice(&block[..8]);
            for (b, tb) in a.iter_mut().zip(t.to_be_bytes()) {
                *b ^= tb;
            }
            ri.copy_from_slice(&block[8..]);
        }
    }
    let mut out = Vec::with_capacity(plaintext.len() + 8);
    out.extend_from_slice(&a);
    for ri in &r {
        out.extend_from_slice(ri);
    }
    Some(out)
}

/// Unwrap `ciphertext` (a multiple of 8 bytes, ≥ 24) under the 256-bit `kek` per RFC 3394 §2.2.2.
/// Returns `None` if the integrity check (the recovered IV must equal [`KW_IV`]) fails — this is
/// what authenticates the wrapped key.
fn aes256_key_unwrap(kek: &[u8; 32], ciphertext: &[u8]) -> Option<Vec<u8>> {
    if !ciphertext.len().is_multiple_of(8) || ciphertext.len() < 24 {
        return None;
    }
    let n = ciphertext.len() / 8 - 1;
    let cipher = Aes256::new(GenericArray::from_slice(kek));
    let mut a = eight(&ciphertext[..8]);
    let mut r: Vec<[u8; 8]> = ciphertext[8..].as_chunks::<8>().0.to_vec();
    for j in (0..6u64).rev() {
        for i in (0..n).rev() {
            let t = n as u64 * j + i as u64 + 1;
            let mut block = [0u8; 16];
            // A ^= t, then B = AES^-1(A | R[i]).
            for (b, tb) in a.iter_mut().zip(t.to_be_bytes()) {
                *b ^= tb;
            }
            block[..8].copy_from_slice(&a);
            block[8..].copy_from_slice(&r[i]);
            cipher.decrypt_block(GenericArray::from_mut_slice(&mut block));
            a.copy_from_slice(&block[..8]);
            r[i].copy_from_slice(&block[8..]);
        }
    }
    if a != KW_IV {
        return None;
    }
    let mut out = Vec::with_capacity(n * 8);
    for ri in &r {
        out.extend_from_slice(ri);
    }
    Some(out)
}

/// The three authenticated attributes of a PDF MAC token (§6.3.6), as a DER-sorted `SET OF`.
/// Built once and shared: the same bytes are HMAC'd (RFC 5652 §9.2 mandates the `SET OF` tag, not
/// the `[2]` implicit tag, in the MAC input) and stored as the token's `authAttrs`.
fn build_auth_attrs(econtent: &[u8]) -> Option<SetOfVec<Attribute>> {
    let message_digest = Sha256::digest(econtent);
    let cap = CmsAlgorithmProtection {
        digest_algorithm: algid(ID_SHA256),
        signature_algorithm: None,
        mac_algorithm: Some(algid(ID_HMAC_SHA256)),
    };
    let attrs = vec![
        attribute(
            ID_CONTENT_TYPE,
            Any::encode_from(&ID_CT_PDF_MAC_INTEGRITY_INFO).ok()?,
        )?,
        attribute(
            ID_MESSAGE_DIGEST,
            Any::encode_from(&OctetString::new(message_digest.as_slice()).ok()?).ok()?,
        )?,
        attribute(ID_AA_CMS_ALGORITHM_PROTECTION, Any::encode_from(&cap).ok()?)?,
    ];
    SetOfVec::try_from(attrs).ok()
}

fn attribute(oid: ObjectIdentifier, value: Any) -> Option<Attribute> {
    let mut values = SetOfVec::new();
    values.insert(value).ok()?;
    Some(Attribute { oid, values })
}

/// Compose a standalone PDF MAC token (TS 32004 §6): a DER-encoded CMS `ContentInfo` wrapping an
/// `AuthenticatedData` over a `PdfMacIntegrityInfo`.
///
/// `file_key` is the document's file encryption key and `kdf_salt` its 32-byte `KDFSalt`; together
/// they derive the key that wraps a fresh random HMAC key. `data_digest` is the SHA-256 digest of
/// the bytes the `ByteRange` covers (§6.6). `signature_digest`, when present, is the SHA-256 digest
/// of the underlying signature value (§6.6.3) — supply it only for a token attached to a signature.
/// Returns `None` on any encoding failure.
#[must_use]
pub fn compose_pdf_mac_token(
    file_key: &[u8],
    kdf_salt: &[u8],
    data_digest: &[u8],
    signature_digest: Option<&[u8]>,
) -> Option<Vec<u8>> {
    let mut mac_key = [0u8; 32];
    getrandom::getrandom(&mut mac_key).ok()?;
    compose_with_key(file_key, kdf_salt, data_digest, signature_digest, &mac_key)
}

/// The deterministic core of [`compose_pdf_mac_token`] with a caller-supplied `mac_key` — lets
/// tests assert against fixed inputs without depending on the system RNG.
fn compose_with_key(
    file_key: &[u8],
    kdf_salt: &[u8],
    data_digest: &[u8],
    signature_digest: Option<&[u8]>,
    mac_key: &[u8; 32],
) -> Option<Vec<u8>> {
    // 1. Encapsulated content: the DER PdfMacIntegrityInfo.
    let info = PdfMacIntegrityInfo {
        version: 0,
        data_digest: OctetString::new(data_digest).ok()?,
        signature_digest: match signature_digest {
            Some(s) => Some(OctetString::new(s).ok()?),
            None => None,
        },
    };
    let econtent = info.to_der().ok()?;

    // 2. Wrap the MAC key: KEK = pdfMacWrapKdf(file_key, salt); encryptedKey = AES-256-wrap(KEK, key).
    let kek = pdf_mac_wrap_kdf(file_key, kdf_salt)?;
    let wrapped = aes256_key_wrap(&kek, mac_key)?;

    // 3. MAC over the DER SET OF authAttrs (RFC 5652 §9.2).
    let auth_attrs = build_auth_attrs(&econtent)?;
    let tbm = auth_attrs.to_der().ok()?;
    let mut hmac = <HmacSha256 as Mac>::new_from_slice(mac_key).ok()?;
    hmac.update(&tbm);
    let mac = hmac.finalize().into_bytes();

    // 4. Assemble the AuthenticatedData and wrap it in a ContentInfo.
    let pwri = PasswordRecipientInfo {
        version: CmsVersion::V0,
        key_derivation_alg: Some(algid(ID_KDF_PDF_MAC_WRAP_KDF)),
        key_enc_alg: algid(ID_AES256_WRAP),
        enc_key: OctetString::new(wrapped).ok()?,
    };
    let mut recips = SetOfVec::new();
    recips.insert(RecipientInfo::Pwri(pwri)).ok()?;

    let auth = AuthenticatedData {
        version: CmsVersion::V0,
        originator_info: None,
        recip_infos: RecipientInfos(recips),
        mac_alg: algid(ID_HMAC_SHA256),
        digest_alg: Some(algid(ID_SHA256)),
        encap_content_info: EncapsulatedContentInfo {
            econtent_type: ID_CT_PDF_MAC_INTEGRITY_INFO,
            econtent: Some(Any::encode_from(&OctetString::new(econtent).ok()?).ok()?),
        },
        auth_attrs: Some(auth_attrs),
        mac: OctetString::new(mac.as_slice()).ok()?,
        unauth_attrs: None,
    };

    let ci = ContentInfo {
        content_type: ID_CT_AUTH_DATA,
        content: Any::encode_from(&auth).ok()?,
    };
    ci.to_der().ok()
}

/// Verify a standalone PDF MAC token (TS 32004 Annex B): re-derive the MAC key from `file_key`/
/// `kdf_salt`, re-compute the HMAC over the token's authenticated attributes, and confirm the
/// encapsulated `dataDigest` (and `signatureDigest`, if expected) match. Returns `true` only when
/// every check passes; any structural defect, algorithm mismatch, or tamper yields `false`.
#[must_use]
pub fn verify_pdf_mac_token(
    token_der: &[u8],
    file_key: &[u8],
    kdf_salt: &[u8],
    expected_data_digest: &[u8],
    expected_signature_digest: Option<&[u8]>,
) -> bool {
    verify_inner(
        token_der,
        file_key,
        kdf_salt,
        expected_data_digest,
        expected_signature_digest,
    )
    .unwrap_or(false)
}

fn verify_inner(
    token_der: &[u8],
    file_key: &[u8],
    kdf_salt: &[u8],
    expected_data_digest: &[u8],
    expected_signature_digest: Option<&[u8]>,
) -> Option<bool> {
    let ci = ContentInfo::from_der(token_der).ok()?;
    if ci.content_type != ID_CT_AUTH_DATA {
        return Some(false);
    }
    let auth = ci.content.decode_as::<AuthenticatedData>().ok()?;

    // Algorithm and structural constraints (§6.3): HMAC-SHA256, SHA-256, exactly one password
    // recipient with our KDF + AES-256 wrap, authenticated attributes present, no unauth attrs.
    if auth.mac_alg.oid != ID_HMAC_SHA256
        || auth.digest_alg.as_ref().map(|a| a.oid) != Some(ID_SHA256)
        || auth.unauth_attrs.is_some()
        || auth.encap_content_info.econtent_type != ID_CT_PDF_MAC_INTEGRITY_INFO
    {
        return Some(false);
    }
    let recips = &auth.recip_infos.0;
    if recips.len() != 1 {
        return Some(false);
    }
    let RecipientInfo::Pwri(pwri) = recips.as_slice().first()? else {
        return Some(false);
    };
    if pwri.key_derivation_alg.as_ref().map(|a| a.oid) != Some(ID_KDF_PDF_MAC_WRAP_KDF)
        || pwri.key_enc_alg.oid != ID_AES256_WRAP
    {
        return Some(false);
    }

    // Recover the MAC key from the file encryption key (Annex B.3). A failed key-wrap integrity
    // check (wrong file key or tampered encryptedKey) makes this return None → not valid.
    let kek = pdf_mac_wrap_kdf(file_key, kdf_salt)?;
    let mac_key = aes256_key_unwrap(&kek, pwri.enc_key.as_bytes())?;

    // The encapsulated content octets (eContent), and the message-digest binding to them.
    let econtent = auth
        .encap_content_info
        .econtent
        .as_ref()?
        .decode_as::<OctetString>()
        .ok()?;
    let econtent = econtent.as_bytes();

    let auth_attrs = auth.auth_attrs.as_ref()?;
    if !auth_attrs_well_formed(auth_attrs, econtent) {
        return Some(false);
    }

    // Re-compute the MAC over the DER SET OF authAttrs and compare (constant time).
    let tbm = auth_attrs.to_der().ok()?;
    let mut hmac = <HmacSha256 as Mac>::new_from_slice(&mac_key).ok()?;
    hmac.update(&tbm);
    if hmac.verify_slice(auth.mac.as_bytes()).is_err() {
        return Some(false);
    }

    // The MAC is now trusted; check the payload digests match what the caller expects.
    let info = PdfMacIntegrityInfo::from_der(econtent).ok()?;
    if info.version != 0 || info.data_digest.as_bytes() != expected_data_digest {
        return Some(false);
    }
    let got_sig = info.signature_digest.as_ref().map(OctetString::as_bytes);
    if got_sig != expected_signature_digest {
        return Some(false);
    }
    Some(true)
}

/// Attach a PDF MAC token to a digital signature (TS 32004 §6.5.2). `cms_der` is a CMS
/// `SignedData` (a signature's `/Contents`); the token is added to its single signer's
/// **unsigned** attributes as `id-attr-pdfMacData`, so the signature itself is untouched and need
/// not be recomputed. `data_digest` is SHA-256 over the signature's `ByteRange` (§6.6.3); the
/// `signatureDigest` is derived here by hashing the signer's signature value. Returns the
/// re-encoded `SignedData`, or `None` on any structural/encoding failure.
#[must_use]
pub fn attach_pdf_mac_to_signature(
    cms_der: &[u8],
    file_key: &[u8],
    kdf_salt: &[u8],
    data_digest: &[u8],
) -> Option<Vec<u8>> {
    let mut ci = ContentInfo::from_der(cms_der).ok()?;
    if ci.content_type != ID_SIGNED_DATA {
        return None;
    }
    let mut sd = ci.content.decode_as::<SignedData>().ok()?;
    let mut signers: Vec<SignerInfo> = sd.signer_infos.0.iter().cloned().collect();
    let signer = signers.first_mut()?;

    // signatureDigest = digest of the SignerInfo signature value (§6.6.3).
    let sig_digest = Sha256::digest(signer.signature.as_bytes());
    let token =
        compose_pdf_mac_token(file_key, kdf_salt, data_digest, Some(sig_digest.as_slice()))?;

    // The token rides as an unsigned attribute with exactly one value (the CMS ContentInfo).
    let attr = attribute(ID_ATTR_PDF_MAC_DATA, Any::from_der(&token).ok()?)?;
    let mut unsigned = signer.unsigned_attrs.take().unwrap_or_default();
    unsigned.insert(attr).ok()?;
    signer.unsigned_attrs = Some(unsigned);

    sd.signer_infos = SignerInfos(SetOfVec::try_from(signers).ok()?);
    ci.content = Any::encode_from(&sd).ok()?;
    ci.to_der().ok()
}

/// Verify a PDF MAC token attached to a signature (TS 32004 Annex B.2.3). Locates the
/// `id-attr-pdfMacData` unsigned attribute on the `SignedData`'s signer, recomputes the
/// `signatureDigest` from the signature value, and checks the token against `data_digest` (the
/// SHA-256 of the signature's `ByteRange`). `false` on any defect or mismatch.
#[must_use]
pub fn verify_attached_pdf_mac(
    cms_der: &[u8],
    file_key: &[u8],
    kdf_salt: &[u8],
    data_digest: &[u8],
) -> bool {
    verify_attached_inner(cms_der, file_key, kdf_salt, data_digest).unwrap_or(false)
}

fn verify_attached_inner(
    cms_der: &[u8],
    file_key: &[u8],
    kdf_salt: &[u8],
    data_digest: &[u8],
) -> Option<bool> {
    let ci = ContentInfo::from_der(cms_der).ok()?;
    if ci.content_type != ID_SIGNED_DATA {
        return Some(false);
    }
    let sd = ci.content.decode_as::<SignedData>().ok()?;
    let signer = sd.signer_infos.0.iter().next()?;
    let sig_digest = Sha256::digest(signer.signature.as_bytes());

    let token = signer
        .unsigned_attrs
        .as_ref()?
        .iter()
        .find(|a| a.oid == ID_ATTR_PDF_MAC_DATA)
        .and_then(|a| a.values.iter().next())?
        .to_der()
        .ok()?;
    Some(verify_pdf_mac_token(
        &token,
        file_key,
        kdf_salt,
        data_digest,
        Some(sig_digest.as_slice()),
    ))
}

/// Confirm the authenticated attributes carry the mandatory content-type and message-digest
/// attributes with the values TS 32004 §6.3.6 requires (the message digest must hash `econtent`).
fn auth_attrs_well_formed(attrs: &SetOfVec<Attribute>, econtent: &[u8]) -> bool {
    let value = |oid: ObjectIdentifier| -> Option<&Any> {
        attrs
            .as_slice()
            .iter()
            .find(|a| a.oid == oid)
            .and_then(|a| a.values.as_slice().first())
    };
    let Some(ct) = value(ID_CONTENT_TYPE) else {
        return false;
    };
    if ct.decode_as::<ObjectIdentifier>().ok() != Some(ID_CT_PDF_MAC_INTEGRITY_INFO) {
        return false;
    }
    let Some(md) = value(ID_MESSAGE_DIGEST) else {
        return false;
    };
    let Ok(md) = md.decode_as::<OctetString>() else {
        return false;
    };
    md.as_bytes() == Sha256::digest(econtent).as_slice()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 3394 §4.6 — wrap 256 bits of key data with a 256-bit KEK.
    #[test]
    fn aes256_kw_rfc3394_vector() {
        let kek: [u8; 32] = hex("000102030405060708090A0B0C0D0E0F101112131415161718191A1B1C1D1E1F")
            .try_into()
            .unwrap();
        let key = hex("00112233445566778899AABBCCDDEEFF000102030405060708090A0B0C0D0E0F");
        let expected =
            hex("28C9F404C4B810F4CBCCB35CFB87F8263F5786E2D80ED326CBC7F0E71A99F43BFB988B9B7A02DD21");
        let wrapped = aes256_key_wrap(&kek, &key).unwrap();
        assert_eq!(wrapped, expected);
        assert_eq!(aes256_key_unwrap(&kek, &wrapped).unwrap(), key);
    }

    #[test]
    fn key_unwrap_rejects_wrong_kek_and_short_input() {
        let kek = [7u8; 32];
        let wrapped = aes256_key_wrap(&kek, &[0u8; 32]).unwrap();
        let mut bad = [0u8; 32];
        bad[0] = 1;
        assert!(aes256_key_unwrap(&bad, &wrapped).is_none());
        assert!(aes256_key_unwrap(&kek, &wrapped[..16]).is_none());
        assert!(aes256_key_wrap(&kek, &[0u8; 4]).is_none()); // not a multiple of 8 / too short
    }

    #[test]
    fn kdf_is_deterministic_and_salt_sensitive() {
        let key = b"file encryption key bytes";
        let a = pdf_mac_wrap_kdf(key, &[1u8; 32]);
        let b = pdf_mac_wrap_kdf(key, &[1u8; 32]);
        let c = pdf_mac_wrap_kdf(key, &[2u8; 32]);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn standalone_round_trip() {
        let file_key = [0x11u8; 32];
        let salt = [0x22u8; 32];
        let data_digest = Sha256::digest(b"the PDF ByteRange bytes");
        let token = compose_pdf_mac_token(&file_key, &salt, data_digest.as_slice(), None).unwrap();
        assert!(verify_pdf_mac_token(
            &token,
            &file_key,
            &salt,
            data_digest.as_slice(),
            None
        ));
    }

    #[test]
    fn signed_variant_round_trip() {
        let file_key = [0x33u8; 32];
        let salt = [0x44u8; 32];
        let data_digest = Sha256::digest(b"document bytes");
        let sig_digest = Sha256::digest(b"the signature value");
        let token = compose_pdf_mac_token(
            &file_key,
            &salt,
            data_digest.as_slice(),
            Some(sig_digest.as_slice()),
        )
        .unwrap();
        // Correct signature digest verifies.
        assert!(verify_pdf_mac_token(
            &token,
            &file_key,
            &salt,
            data_digest.as_slice(),
            Some(sig_digest.as_slice())
        ));
        // A standalone verifier (expecting no signature digest) rejects a signed token.
        assert!(!verify_pdf_mac_token(
            &token,
            &file_key,
            &salt,
            data_digest.as_slice(),
            None
        ));
    }

    #[test]
    fn rejects_wrong_key_digest_and_tamper() {
        let file_key = [0x55u8; 32];
        let salt = [0x66u8; 32];
        let data_digest = Sha256::digest(b"covered bytes");
        let token = compose_pdf_mac_token(&file_key, &salt, data_digest.as_slice(), None).unwrap();

        // Wrong file encryption key: key unwrap integrity check fails.
        assert!(!verify_pdf_mac_token(
            &token,
            &[0x00u8; 32],
            &salt,
            data_digest.as_slice(),
            None
        ));
        // Wrong KDF salt.
        assert!(!verify_pdf_mac_token(
            &token,
            &file_key,
            &[0u8; 32],
            data_digest.as_slice(),
            None
        ));
        // Expected digest mismatch (document changed out from under a stale MAC).
        let other = Sha256::digest(b"different bytes");
        assert!(!verify_pdf_mac_token(
            &token,
            &file_key,
            &salt,
            other.as_slice(),
            None
        ));
        // Flip a byte in the encoded token: garbage / MAC mismatch, never a panic.
        let mut mutated = token.clone();
        let last = mutated.len() - 1;
        mutated[last] ^= 0xFF;
        assert!(!verify_pdf_mac_token(
            &mutated,
            &file_key,
            &salt,
            data_digest.as_slice(),
            None
        ));
    }

    #[test]
    fn rejects_non_authdata_and_garbage() {
        assert!(!verify_pdf_mac_token(
            b"", &[0u8; 32], &[0u8; 32], &[0u8; 32], None
        ));
        assert!(!verify_pdf_mac_token(
            &[0x30, 0x03, 0x02, 0x01, 0x00],
            &[0u8; 32],
            &[0u8; 32],
            &[0u8; 32],
            None
        ));
        // A SignedData-style ContentInfo (wrong content type) is rejected, not misread.
        let ci = ContentInfo {
            content_type: ID_SHA256, // any non-authData OID
            content: Any::encode_from(&OctetString::new(b"x".as_slice()).unwrap()).unwrap(),
        };
        let der = ci.to_der().unwrap();
        assert!(!verify_pdf_mac_token(
            &der, &[0u8; 32], &[0u8; 32], &[0u8; 32], None
        ));
    }

    /// The composed token is a structurally valid CMS `ContentInfo` whose payload re-decodes —
    /// guards against silently emitting something a conforming validator could not parse.
    #[test]
    fn token_is_parseable_authenticated_data() {
        let token = compose_pdf_mac_token(
            &[1u8; 32],
            &[2u8; 32],
            Sha256::digest(b"x").as_slice(),
            None,
        )
        .unwrap();
        let ci = ContentInfo::from_der(&token).unwrap();
        assert_eq!(ci.content_type, ID_CT_AUTH_DATA);
        let auth = ci.content.decode_as::<AuthenticatedData>().unwrap();
        assert_eq!(auth.mac_alg.oid, ID_HMAC_SHA256);
        assert_eq!(auth.recip_infos.0.len(), 1);
        assert!(auth.unauth_attrs.is_none());
    }

    /// Decode a valid token, apply `mutate` to its `AuthenticatedData`, re-encode, and confirm
    /// the verifier rejects it — exercising the §6.3 structural-conformance gates.
    fn assert_rejected(mutate: impl FnOnce(&mut AuthenticatedData)) {
        let file_key = [0x77u8; 32];
        let salt = [0x88u8; 32];
        let dd = Sha256::digest(b"bytes");
        let token = compose_pdf_mac_token(&file_key, &salt, dd.as_slice(), None).unwrap();
        let ci = ContentInfo::from_der(&token).unwrap();
        let mut auth = ci.content.decode_as::<AuthenticatedData>().unwrap();
        mutate(&mut auth);
        let ci = ContentInfo {
            content_type: ID_CT_AUTH_DATA,
            content: Any::encode_from(&auth).unwrap(),
        };
        let der = ci.to_der().unwrap();
        assert!(!verify_pdf_mac_token(
            &der,
            &file_key,
            &salt,
            dd.as_slice(),
            None
        ));
    }

    #[test]
    fn rejects_structurally_nonconforming_tokens() {
        // Wrong MAC algorithm.
        assert_rejected(|a| a.mac_alg = algid(ID_SHA256));
        // Missing/again-wrong digest algorithm.
        assert_rejected(|a| a.digest_alg = None);
        // Unauthenticated attributes present (forbidden, §6.3.7).
        assert_rejected(|a| a.auth_attrs.clone_into(&mut a.unauth_attrs));
        // Two recipients instead of exactly one.
        assert_rejected(|a| {
            let first = a.recip_infos.0.as_slice()[0].clone();
            if let RecipientInfo::Pwri(mut p) = first {
                p.enc_key = OctetString::new([0u8; 40].as_slice()).unwrap();
                a.recip_infos.0.insert(RecipientInfo::Pwri(p)).unwrap();
            }
        });
        // Wrong key-encryption algorithm in the password recipient.
        assert_rejected(|a| {
            if let RecipientInfo::Pwri(mut p) = a.recip_infos.0.as_slice()[0].clone() {
                p.key_enc_alg = algid(ID_SHA256);
                let mut s = SetOfVec::new();
                s.insert(RecipientInfo::Pwri(p)).unwrap();
                a.recip_infos = RecipientInfos(s);
            }
        });
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// A minimal CMS `SignedData` (`id-data` detached, one signer keyed by a subject-key id) with
    /// the given signature value — enough to exercise MAC attachment without a real signing key.
    fn dummy_signed_data(sig_value: &[u8]) -> Vec<u8> {
        use cms::signed_data::{SignerIdentifier, SignerInfo};
        use der::asn1::SetOfVec;

        let skid = x509_cert::ext::pkix::SubjectKeyIdentifier(
            OctetString::new(b"key-id".as_slice()).unwrap(),
        );
        let signer = SignerInfo {
            version: CmsVersion::V3,
            sid: SignerIdentifier::SubjectKeyIdentifier(skid),
            digest_alg: algid(ID_SHA256),
            signed_attrs: None,
            signature_algorithm: algid(ID_SHA256),
            signature: OctetString::new(sig_value).unwrap(),
            unsigned_attrs: None,
        };
        let mut digest_algs = SetOfVec::new();
        digest_algs.insert(algid(ID_SHA256)).unwrap();
        let mut signers = SetOfVec::new();
        signers.insert(signer).unwrap();
        let sd = SignedData {
            version: CmsVersion::V3,
            digest_algorithms: digest_algs,
            encap_content_info: EncapsulatedContentInfo {
                econtent_type: ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.1"),
                econtent: None,
            },
            certificates: None,
            crls: None,
            signer_infos: SignerInfos(signers),
        };
        let ci = ContentInfo {
            content_type: ID_SIGNED_DATA,
            content: Any::encode_from(&sd).unwrap(),
        };
        ci.to_der().unwrap()
    }

    #[test]
    fn attached_to_signature_round_trip() {
        let file_key = [0xABu8; 32];
        let salt = [0xCDu8; 32];
        let data_digest = Sha256::digest(b"the signed ByteRange");
        let cms = dummy_signed_data(b"a plausible signature value");

        let attached =
            attach_pdf_mac_to_signature(&cms, &file_key, &salt, data_digest.as_slice()).unwrap();
        assert!(verify_attached_pdf_mac(
            &attached,
            &file_key,
            &salt,
            data_digest.as_slice()
        ));
        // The original signature value is unchanged (MAC rides as an *unsigned* attribute).
        let orig = ContentInfo::from_der(&cms)
            .unwrap()
            .content
            .decode_as::<SignedData>()
            .unwrap();
        let augmented = ContentInfo::from_der(&attached)
            .unwrap()
            .content
            .decode_as::<SignedData>()
            .unwrap();
        assert_eq!(
            orig.signer_infos
                .0
                .iter()
                .next()
                .unwrap()
                .signature
                .as_bytes(),
            augmented
                .signer_infos
                .0
                .iter()
                .next()
                .unwrap()
                .signature
                .as_bytes()
        );
    }

    #[test]
    fn attached_mac_rejects_tamper_and_absence() {
        let file_key = [1u8; 32];
        let salt = [2u8; 32];
        let dd = Sha256::digest(b"covered");
        let cms = dummy_signed_data(b"sig-value");
        let attached = attach_pdf_mac_to_signature(&cms, &file_key, &salt, dd.as_slice()).unwrap();

        // Wrong file key, wrong data digest → rejected.
        assert!(!verify_attached_pdf_mac(
            &attached,
            &[0u8; 32],
            &salt,
            dd.as_slice()
        ));
        let other = Sha256::digest(b"different");
        assert!(!verify_attached_pdf_mac(
            &attached,
            &file_key,
            &salt,
            other.as_slice()
        ));
        // A signature without the attribute, and non-SignedData input, are rejected.
        assert!(!verify_attached_pdf_mac(
            &cms,
            &file_key,
            &salt,
            dd.as_slice()
        ));
        assert!(!verify_attached_pdf_mac(
            b"\x30\x00",
            &file_key,
            &salt,
            dd.as_slice()
        ));
        // Attaching to a non-SignedData blob fails.
        assert!(
            attach_pdf_mac_to_signature(b"\x30\x00", &file_key, &salt, dd.as_slice()).is_none()
        );
    }
}
