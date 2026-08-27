//! Encryption plumbing for [`Document`] (ISO 32000-1 §7.6).
//!
//! Opening installs the security handler and the decryptor the reader calls per object; saving and
//! incremental updates run the inverse over everything they write. §7.6.2 scopes both directions:
//! encryption "applies to all strings and streams in the document's PDF file", with only four
//! exceptions — the trailer `/ID`, strings in the `/Encrypt` dictionary, strings already inside an
//! encrypted stream, and the signature `/Contents` hex string.

use std::sync::Arc;

use pdf_cos::{Dictionary, Name, Object, ObjectId, PdfString, Stream};
use pdf_crypto::StandardSecurityHandler;

use crate::{DocError, Document, Result};

impl Document {
    /// Record the security handler an `open` installed, so a later incremental update can
    /// re-encrypt what it writes (§7.6.2) and carry `/Encrypt` into its trailer (§7.5.6).
    fn remember_security(&mut self, handler: &Arc<StandardSecurityHandler>, encrypt_ref: &Object) {
        if let Some(id) = encrypt_ref.as_reference() {
            self.security = Some((Arc::clone(handler), id));
        }
    }

    /// The security handler and `/Encrypt` reference for an encrypted document, or `None` when the
    /// document is not encrypted (or its handler could not be opened).
    pub(crate) fn security(&self) -> Option<(&StandardSecurityHandler, ObjectId)> {
        self.security
            .as_ref()
            .map(|(handler, id)| (handler.as_ref(), *id))
    }

    /// Decode one of this document's streams through its `/Filter` chain (§7.4), under the limits
    /// the document was opened with.
    ///
    /// Every decode of a stream that came out of *this* document must go through here rather than
    /// calling `pdf_filters` directly, so that a caller who tightened [`Limits`](crate::Limits) at open time
    /// actually gets those ceilings — including the chain-length bound, without which a stream's
    /// decode cost is its body size times its (attacker-chosen) filter count.
    pub fn decode_stream(&self, stream: &Stream) -> Result<Vec<u8>> {
        pdf_filters::decode_stream_with_limits(
            stream,
            self.limits.max_decoded_stream,
            self.limits.max_filter_chain,
        )
        .map_err(|_| DocError::ContentDecode)
    }

    /// If the document is encrypted (§7.6), build the standard security handler from `/Encrypt`
    /// (trying `password` as user then owner) and install it so fetched objects are decrypted —
    /// RC4, AES-128 and AES-256 are supported. A supported handler with a wrong password is an
    /// error ([`DocError::NeedsPassword`]); an unsupported handler (e.g. public-key) leaves the
    /// document undecrypted.
    pub(crate) fn setup_encryption(&mut self, password: &[u8]) -> Result<()> {
        let Some(encrypt_ref) = self.xref.trailer.get(&Name::from("Encrypt")).cloned() else {
            return Ok(());
        };
        let exempt = encrypt_ref.as_reference().map(|id| id.number);
        let Object::Dictionary(encrypt) = self.resolve(&encrypt_ref)? else {
            return Ok(());
        };
        let id0 = self.file_id0();
        if let Some(handler) = StandardSecurityHandler::open(&encrypt, &id0, password) {
            let handler = Arc::new(handler);
            self.remember_security(&handler, &encrypt_ref);
            let decryptor = Arc::clone(&handler);
            self.xref
                .set_decryptor(exempt, Arc::new(move |n, g, d| decryptor.decrypt(n, g, d)));
        } else if pdf_crypto::supports(&encrypt) {
            return Err(DocError::NeedsPassword);
        }
        Ok(())
    }

    /// Arm the public-key decryptor (§7.6.5) if the document carries an `Adobe.PPKLite` `/Encrypt`.
    pub(crate) fn setup_encryption_public_key(
        &mut self,
        cert_der: &[u8],
        key_der: &[u8],
    ) -> Result<()> {
        let Some(encrypt_ref) = self.xref.trailer.get(&Name::from("Encrypt")).cloned() else {
            return Ok(());
        };
        let exempt = encrypt_ref.as_reference().map(|id| id.number);
        let Object::Dictionary(encrypt) = self.resolve(&encrypt_ref)? else {
            return Ok(());
        };
        if let Some(handler) = StandardSecurityHandler::open_public_key(&encrypt, cert_der, key_der)
        {
            let handler = Arc::new(handler);
            self.remember_security(&handler, &encrypt_ref);
            let decryptor = Arc::clone(&handler);
            self.xref
                .set_decryptor(exempt, Arc::new(move |n, g, d| decryptor.decrypt(n, g, d)));
        } else if encrypt.get_name(&Name::from("Filter")).map(Name::as_bytes)
            == Some(b"Adobe.PPKLite")
        {
            return Err(DocError::NeedsPassword);
        }
        Ok(())
    }
}

/// Recursively transform an object's strings and stream bytes through `transform` — used to encrypt
/// an object's content for [`Document::save_encrypted`] (§7.6.2). The structure (dictionaries,
/// arrays, names, references) is preserved; only string bytes and raw stream bytes change.
///
/// `transform` is fallible because the AES modes draw a fresh IV or nonce per object: if the OS
/// RNG is unavailable there is no safe value to substitute, so the whole save fails rather than
/// emitting objects under a predictable IV.
pub(crate) fn encrypt_object(
    object: &Object,
    transform: &dyn Fn(&[u8]) -> Option<Vec<u8>>,
) -> Option<Object> {
    Some(match object {
        Object::String(s) => Object::String(PdfString::from(transform(s.as_bytes())?)),
        Object::Array(array) => Object::Array(
            array
                .iter()
                .map(|item| encrypt_object(item, transform))
                .collect::<Option<_>>()?,
        ),
        Object::Dictionary(dict) => Object::Dictionary(encrypt_dict(dict, transform)?),
        Object::Stream(stream) => {
            let dict = encrypt_dict(stream.dict(), transform)?;
            Object::Stream(Stream::new(dict, transform(stream.raw())?))
        }
        other => other.clone(),
    })
}

/// Apply [`encrypt_object`] to every value of a dictionary.
fn encrypt_dict(
    dict: &Dictionary,
    transform: &dyn Fn(&[u8]) -> Option<Vec<u8>>,
) -> Option<Dictionary> {
    dict.iter()
        .map(|(key, value)| Some((key.clone(), encrypt_object(value, transform)?)))
        .collect()
}
