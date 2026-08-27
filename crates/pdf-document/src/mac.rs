//! PDF MAC integrity protection — writer + verifier (ISO/TS 32004:2024).
//!
//! Wires the cryptographic core in `pdf-crypto` into a document. [`Document::save_encrypted_with_mac`]
//! serializes an AES-256 (V5/R6) encrypted PDF carrying a **standalone** PDF MAC token in a direct
//! `/AuthCode` trailer dictionary (§5.2.3): it stores a fresh `KDFSalt` in `/Encrypt` (§5.1.1),
//! lays out the `/ByteRange` and `/MAC` placeholders, then — in a second pass over the laid-out
//! bytes — computes the real `/ByteRange`, digests the bytes it covers, composes the token keyed by
//! the file encryption key, and writes its DER **exactly** into `/MAC` (no trailing padding, Table 6
//! NOTE 2; the token length is deterministic, so the reserved span is measured from a sample).
//!
//! [`Document::verify_pdf_mac`] is the read side (Annex B): locate `/AuthCode`, recover the file key
//! from `/Encrypt` + password, re-digest the `ByteRange`, and check the token. PDF MAC keys on the
//! file encryption key, so it only applies to V5/R6 handlers (§3.3).

use pdf_cos::{Array, Name, Object, ObjectId, PdfString};
use pdf_crypto::{
    StandardSecurityHandler, compose_pdf_mac_token, random_kdf_salt, sha256,
    verify_attached_pdf_mac, verify_pdf_mac_token,
};
use pdf_writer::write_document_encrypted_with_authcode;

use crate::signing::{find, to_hex_upper, trim_to_der};
use crate::{Algorithm, DocError, Document, Permissions, Result};

/// `/P` permission bit 13 (ISO/TS 32004 §5.1.2): when zero, a PDF MAC token is required in all
/// revisions. PDF permission bits are 1-indexed, so bit 13 is `1 << 12`.
const MAC_REQUIRED_BIT: i32 = 1 << 12;

impl Document {
    /// Serialize to a fresh AES-256 encrypted PDF protected by a standalone PDF MAC token
    /// (ISO/TS 32004). Grants all permissions and encrypts metadata; use
    /// [`Document::save_encrypted_with_mac_full`] to restrict them. `algorithm` must be
    /// [`Algorithm::Aes256`] or [`Algorithm::Aes256Gcm`] (V5/R6) — anything else is
    /// [`DocError::MacRequiresV5`]. Reopen and check with [`Document::verify_pdf_mac`].
    pub fn save_encrypted_with_mac(
        &self,
        user_password: &[u8],
        owner_password: &[u8],
        algorithm: Algorithm,
    ) -> Result<Vec<u8>> {
        self.save_encrypted_with_mac_full(
            user_password,
            owner_password,
            Permissions::ALL,
            true,
            algorithm,
        )
    }

    /// As [`Document::save_encrypted_with_mac`], with explicit [`Permissions`] and `encrypt_metadata`.
    pub fn save_encrypted_with_mac_full(
        &self,
        user_password: &[u8],
        owner_password: &[u8],
        permissions: Permissions,
        encrypt_metadata: bool,
        algorithm: Algorithm,
    ) -> Result<Vec<u8>> {
        if !matches!(algorithm, Algorithm::Aes256 | Algorithm::Aes256Gcm) {
            return Err(DocError::MacRequiresV5);
        }
        // Clear /P bit 13 (§5.1.2/5.1.3): zero signals "a PDF MAC token is required in all
        // revisions", making the /AuthCode entry mandatory (§5.2.1, Table 5). The V5 /Perms entry
        // re-seals this /P, so reader-side permission validation stays consistent.
        let permissions = permissions.bits() & !MAC_REQUIRED_BIT;
        let (handler, mut encrypt_dict, id0) = StandardSecurityHandler::new_encrypter(
            user_password,
            owner_password,
            permissions,
            encrypt_metadata,
            algorithm,
        )
        .ok_or(DocError::RandomUnavailable)?;
        // KDFSalt (§5.1.1, Table 2): a direct 32-byte string in /Encrypt, constant for the file.
        let kdf_salt = random_kdf_salt().ok_or(DocError::RandomUnavailable)?;
        encrypt_dict.insert(
            Name::from("KDFSalt"),
            Object::String(PdfString::from(kdf_salt.to_vec())),
        );

        // The PDF MAC token has a fixed DER length for our fixed algorithm set; measure it from a
        // sample so the /MAC placeholder reserves exactly the right span (no trailing data).
        let file_key = handler.file_key().to_vec();
        let der_len = compose_pdf_mac_token(&file_key, &kdf_salt, &[0u8; 32], None)
            .ok_or(DocError::MacFailed)?
            .len();

        // Assemble objects exactly as the plain encrypted writer does (encrypt each, append the
        // cleartext /Encrypt object), but route through the /AuthCode-bearing trailer writer.
        let root = self.xref.root().ok_or(DocError::MissingCatalog)?;
        let info = self
            .xref
            .trailer
            .get(&Name::from("Info"))
            .and_then(Object::as_reference);
        // V5/AES-256 + ISO/TS 32004 is a PDF 2.0 feature; floor the header version there.
        let base = self.version().map_or((1, 4), |v| (v.major, v.minor));
        let version = base.max((2, 0));
        let mut objects = self.collect_objects()?;
        for (id, object) in &mut objects {
            let (number, generation) = (id.number, id.generation);
            *object = crate::encryption::encrypt_object(object, &|data| {
                handler.encrypt(number, generation, data)
            })
            .ok_or(DocError::RandomUnavailable)?;
        }
        let encrypt_number = objects.iter().map(|(id, _)| id.number).max().unwrap_or(0) + 1;
        let encrypt_id = ObjectId::new(encrypt_number, 0);
        objects.push((encrypt_id, Object::Dictionary(encrypt_dict)));

        let authcode = format!(
            " /AuthCode << /MACLocation /Standalone /ByteRange [0 0000000000 0000000000 0000000000] /MAC <{}> >>",
            "0".repeat(der_len * 2)
        );
        let mut bytes = write_document_encrypted_with_authcode(
            &objects, root, info, version, encrypt_id, &id0, &authcode,
        );

        patch_mac(&mut bytes, der_len, |digest| {
            compose_pdf_mac_token(&file_key, &kdf_salt, digest, None)
        })?;
        Ok(bytes)
    }

    /// Recover the file encryption key and `KDFSalt` for PDF MAC operations (§6.4): open the
    /// standard security handler from `/Encrypt` + `password`. Errors if the document is not
    /// AES-256 (V5/R6) encrypted, carries no `KDFSalt`, or the password is wrong.
    pub(crate) fn mac_material(&self, password: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
        let Some(encrypt_ref) = self.xref.trailer.get(&Name::from("Encrypt")).cloned() else {
            return Err(DocError::MacRequiresV5);
        };
        let encrypt = self.resolve_dict(&encrypt_ref, "Encrypt")?;
        let Some(Object::String(salt)) = encrypt.get(&Name::from("KDFSalt")) else {
            return Err(DocError::MacFailed);
        };
        let kdf_salt = salt.as_bytes().to_vec();
        let id0 = self.file_id0();
        let handler = StandardSecurityHandler::open(&encrypt, &id0, password)
            .ok_or(DocError::NeedsPassword)?;
        if !handler.is_v5() {
            return Err(DocError::MacRequiresV5);
        }
        Ok((handler.file_key().to_vec(), kdf_salt))
    }

    /// Verify the document's PDF MAC token (ISO/TS 32004 Annex B), in either location: a standalone
    /// token in the trailer `/AuthCode` (§6.5.1) or one attached to the signature referenced by
    /// `/SigObjRef` (`/MACLocation /AttachedToSig`, §6.5.2). Returns `Ok(None)` when the document
    /// carries no (recognised) `/AuthCode`, `Ok(Some(true))` when the token authenticates the
    /// current bytes, and `Ok(Some(false))` otherwise (tamper, stale MAC, malformed token).
    /// `password` unlocks `/Encrypt` to recover the file key; a wrong password is
    /// [`DocError::NeedsPassword`].
    pub fn verify_pdf_mac(&self, password: &[u8]) -> Result<Option<bool>> {
        let Some(Object::Dictionary(auth)) = self.xref.trailer.get(&Name::from("AuthCode")) else {
            return Ok(None);
        };
        let location = auth.get(&Name::from("MACLocation")).cloned();

        // Recover the file encryption key + KDFSalt (shared by both locations).
        let Some(encrypt_ref) = self.xref.trailer.get(&Name::from("Encrypt")) else {
            return Ok(Some(false));
        };
        let encrypt = self.resolve_dict(encrypt_ref, "Encrypt")?;
        let Some(Object::String(salt)) = encrypt.get(&Name::from("KDFSalt")) else {
            return Ok(Some(false));
        };
        let kdf_salt = salt.as_bytes().to_vec();
        let id0 = self.file_id0();
        let handler = StandardSecurityHandler::open(&encrypt, &id0, password)
            .ok_or(DocError::NeedsPassword)?;
        if !handler.is_v5() {
            return Ok(Some(false));
        }
        let file_key = handler.file_key();

        match location {
            Some(Object::Name(n)) if n == "Standalone" => {
                let (Some(Object::String(mac)), Some(Object::Array(range))) = (
                    auth.get(&Name::from("MAC")),
                    auth.get(&Name::from("ByteRange")),
                ) else {
                    return Ok(Some(false));
                };
                let Some(covered) = byte_range_bytes(&self.bytes, range) else {
                    return Ok(Some(false));
                };
                Ok(Some(verify_pdf_mac_token(
                    mac.as_bytes(),
                    file_key,
                    &kdf_salt,
                    &sha256(&covered),
                    None,
                )))
            }
            Some(Object::Name(n)) if n == "AttachedToSig" => {
                // The MAC rides on the signature /SigObjRef points at (§6.5.2). Its dataDigest is
                // over that signature's ByteRange; the CMS is the signature's /Contents, read raw
                // from the ByteRange gap so the (cleartext) signature bytes are not run through the
                // decryptor (§7.6.2 exempts signature Contents from encryption anyway).
                let Some(sig_ref) = auth.get(&Name::from("SigObjRef")) else {
                    return Ok(Some(false));
                };
                let sig = self.resolve_dict(sig_ref, "SigObjRef")?;
                let Some(Object::Array(range)) = sig.get(&Name::from("ByteRange")) else {
                    return Ok(Some(false));
                };
                let (Some(covered), Some(cms)) = (
                    byte_range_bytes(&self.bytes, range),
                    contents_in_byte_range_gap(&self.bytes, range),
                ) else {
                    return Ok(Some(false));
                };
                Ok(Some(verify_attached_pdf_mac(
                    &cms,
                    file_key,
                    &kdf_salt,
                    &sha256(&covered),
                )))
            }
            _ => Ok(None),
        }
    }
}

/// Patch the laid-out standalone MAC: fill the real `/ByteRange`, digest the bytes it covers, then
/// write the produced token's DER hex into `/MAC` — which must be exactly `der_len` bytes (the gap
/// the `/ByteRange` excludes is sized for it, with no padding allowed; Table 6 NOTE 2).
fn patch_mac(
    bytes: &mut [u8],
    der_len: usize,
    produce: impl FnOnce(&[u8]) -> Option<Vec<u8>>,
) -> Result<()> {
    let marker = b"/MAC <";
    let lt = find(bytes, marker).ok_or(DocError::MacFailed)? + marker.len() - 1; // index of '<'
    let hex_start = lt + 1;
    let hex_end = hex_start + der_len * 2; // index of '>'
    if bytes.get(hex_end) != Some(&b'>') {
        return Err(DocError::MacFailed);
    }
    let after = hex_end + 1; // first byte past '>'
    let total = bytes.len();

    // ByteRange [0 lt after (total-after)] — the gap [lt, after) is exactly "<…>" (the MAC value).
    let placeholder = b"/ByteRange [0 0000000000 0000000000 0000000000]";
    let br = find(bytes, placeholder).ok_or(DocError::MacFailed)?;
    let new_range = format!("/ByteRange [0 {lt:010} {after:010} {:010}]", total - after);
    if new_range.len() != placeholder.len() {
        return Err(DocError::MacFailed); // offsets too large for the reserved width
    }
    bytes[br..br + placeholder.len()].copy_from_slice(new_range.as_bytes());

    // The dataDigest is SHA-256 over the covered bytes (everything except the "<…>" value).
    let mut covered = Vec::with_capacity(lt + total - after);
    covered.extend_from_slice(&bytes[..lt]);
    covered.extend_from_slice(&bytes[after..]);
    let token = produce(&sha256(&covered)).ok_or(DocError::MacFailed)?;
    if token.len() != der_len {
        return Err(DocError::MacFailed); // the deterministic-length assumption must hold
    }
    let hex = to_hex_upper(&token);
    bytes[hex_start..hex_start + hex.len()].copy_from_slice(hex.as_bytes());
    Ok(())
}

/// The bytes a four-element `/ByteRange` `[0 L1 S L2]` covers: `data[0..L1] ++ data[S..S+L2]`.
/// `None` if the array is malformed or any segment runs past the end (hostile input, §3.4).
fn byte_range_bytes(data: &[u8], range: &Array) -> Option<Vec<u8>> {
    let [start1, len1, start2, len2] = byte_range_ints(range)?;
    let first = data.get(start1..start1.checked_add(len1)?)?;
    let second = data.get(start2..start2.checked_add(len2)?)?;
    let mut out = Vec::with_capacity(len1 + len2);
    out.extend_from_slice(first);
    out.extend_from_slice(second);
    Some(out)
}

/// The four non-negative integers of a `/ByteRange`, or `None` if malformed.
fn byte_range_ints(range: &Array) -> Option<[usize; 4]> {
    if range.len() != 4 {
        return None;
    }
    let mut v = [0usize; 4];
    for (slot, obj) in v.iter_mut().zip(range.iter()) {
        let Object::Integer(n) = obj else {
            return None;
        };
        *slot = usize::try_from(*n).ok()?;
    }
    Some(v)
}

/// The signature `/Contents` value (a DER CMS), read raw from the gap a `/ByteRange` `[0 L1 S L2]`
/// excludes: `data[L1..S]` is the `<…>` hex string object, whose digits decode to the (zero-padded)
/// CMS, then trimmed to the exact DER object. `None` if the gap is not a well-formed hex string.
fn contents_in_byte_range_gap(data: &[u8], range: &Array) -> Option<Vec<u8>> {
    let [_, len1, start2, _] = byte_range_ints(range)?;
    let gap = data.get(len1..start2)?;
    let inner = gap.strip_prefix(b"<")?.strip_suffix(b">")?;
    let padded = hex_decode(inner)?;
    Some(trim_to_der(&padded).to_vec())
}

/// Decode an even-length ASCII hex string to bytes; `None` on any non-hex digit or odd length.
fn hex_decode(hex: &[u8]) -> Option<Vec<u8>> {
    if hex.len() % 2 != 0 {
        return None;
    }
    fn nibble(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    }
    hex.chunks_exact(2)
        .map(|p| Some((nibble(p[0])? << 4) | nibble(p[1])?))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(v: &[i64]) -> Array {
        Array::from_vec(v.iter().map(|&n| Object::Integer(n)).collect())
    }

    #[test]
    fn hex_decode_round_trips_and_rejects_bad_input() {
        assert_eq!(hex_decode(b"00FF1a").unwrap(), vec![0x00, 0xFF, 0x1A]);
        assert_eq!(hex_decode(b"").unwrap(), Vec::<u8>::new());
        assert!(hex_decode(b"ABC").is_none()); // odd length
        assert!(hex_decode(b"GG").is_none()); // non-hex digit
    }

    #[test]
    fn byte_range_parsing() {
        assert_eq!(byte_range_ints(&range(&[0, 4, 8, 2])), Some([0, 4, 8, 2]));
        assert_eq!(byte_range_ints(&range(&[0, 4, 8])), None); // wrong arity
        assert_eq!(byte_range_ints(&range(&[0, -1, 8, 2])), None); // negative
        let arr = Array::from_vec(vec![Object::Null, Object::Null, Object::Null, Object::Null]);
        assert_eq!(byte_range_ints(&arr), None); // non-integer
    }

    #[test]
    fn byte_range_bytes_covers_two_segments_and_bounds_check() {
        let data = b"ABCDEFGHIJ";
        // [0,3,7,3] → "ABC" ++ "HIJ".
        assert_eq!(
            byte_range_bytes(data, &range(&[0, 3, 7, 3])).unwrap(),
            b"ABCHIJ"
        );
        // Second segment runs past the end → None.
        assert!(byte_range_bytes(data, &range(&[0, 3, 7, 99])).is_none());
    }

    #[test]
    fn contents_gap_extraction() {
        // bytes: [0..5)="head ", gap [5..11)="<41FF>", then tail. Range [0 5 11 …].
        let data = b"head <41FF> tail";
        let got = contents_in_byte_range_gap(data, &range(&[0, 5, 11, 5])).unwrap();
        // trim_to_der leaves the bytes as-is when not a parseable TLV; just check the hex decoded.
        assert_eq!(&got[..2.min(got.len())], &[0x41, 0xFF][..2.min(got.len())]);
        // A gap not wrapped in <…> is rejected.
        assert!(contents_in_byte_range_gap(b"head  41FF  tail", &range(&[0, 5, 11, 5])).is_none());
    }

    /// `patch_mac` fills the `/ByteRange` and writes the exact-length token into `/MAC`.
    #[test]
    fn patch_mac_happy_and_errors() {
        let layout = b"x /ByteRange [0 0000000000 0000000000 0000000000] /MAC <0000> >>\n";
        let mut good = layout.to_vec();
        // der_len = 2 → 4 reserved hex zeros (matches "<0000>").
        patch_mac(&mut good, 2, |_digest| Some(vec![0xAB, 0xCD])).unwrap();
        assert!(good.windows(6).any(|w| w == b"<ABCD>"));
        assert!(
            !good
                .windows(46)
                .any(|w| w == b"/ByteRange [0 0000000000 0000000000 0000000000]")
        );

        // Reserve width disagrees with der_len → the '>' isn't where expected.
        let mut bad_gt = layout.to_vec();
        assert!(patch_mac(&mut bad_gt, 1, |_| Some(vec![0; 1])).is_err());

        // Produced token is not the promised length.
        let mut wrong_len = layout.to_vec();
        assert!(patch_mac(&mut wrong_len, 2, |_| Some(vec![0; 5])).is_err());

        // No /ByteRange placeholder at all.
        let mut no_range = b"x /MAC <0000> >>\n".to_vec();
        assert!(patch_mac(&mut no_range, 2, |_| Some(vec![0xAB, 0xCD])).is_err());
    }
}
