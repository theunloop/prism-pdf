//! Unencrypted wrapper documents (ISO 32000-2 §7.6.7): a plain PDF that carries, as its single
//! embedded file, a payload PDF encrypted with a *non-standard* (custom) security handler.
//!
//! The wrapper names the cryptographic filter in the payload filespec's `/EP` encrypted payload
//! dictionary (Table 28), relates the file via `/AFRelationship /EncryptedPayload`, and declares a
//! hidden collection (`/Collection /View /H`, §12.3.5) whose initial document is the payload — so
//! a processor that *has* the filter opens the payload directly, while any other shows the
//! wrapper's visible pages (the "install this handler" instructions). Authoring goes through
//! [`Builder::encrypted_payload`](crate::Builder::encrypted_payload); this module reads it back.

use pdf_cos::{Name, Object};

use crate::names::decode_text_string;
use crate::{Document, Result};

/// The encrypted payload read back from an unencrypted wrapper document (§7.6.7).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EncryptedPayload {
    /// The payload's file name (`/UF`/`/F` of its filespec).
    pub file_name: String,
    /// `/EP /Subtype` — the name of the cryptographic filter needed to decrypt the payload.
    pub filter_subtype: String,
    /// `/EP /Version` of that filter, when declared (a `/M.m` name).
    pub version: Option<(u8, u8)>,
    /// The still-encrypted payload bytes (the embedded-file stream through its transport filters —
    /// decoding the *cryptographic* filter is the custom handler's job, not Prism PDF's).
    pub data: Vec<u8>,
}

impl Document {
    /// The encrypted payload of an unencrypted wrapper document (§7.6.7), or `None` when no
    /// embedded file carries an `/EP` encrypted payload dictionary. Best-effort: a malformed
    /// wrapper yields `None` rather than an error.
    pub fn encrypted_payload(&self) -> Result<Option<EncryptedPayload>> {
        for (key, value) in self.names("EmbeddedFiles")? {
            let Ok(Object::Dictionary(filespec)) = self.resolve(&value) else {
                continue;
            };
            let Some(ep) = filespec.get(&Name::from("EP")) else {
                continue;
            };
            let Ok(Object::Dictionary(ep)) = self.resolve(ep) else {
                continue;
            };
            let Some(subtype) = ep.get_name(&Name::from("Subtype")) else {
                continue;
            };
            let filter_subtype = String::from_utf8_lossy(subtype.as_bytes()).into_owned();
            let version = ep
                .get_name(&Name::from("Version"))
                .and_then(|n| std::str::from_utf8(n.as_bytes()).ok())
                .and_then(|s| s.split_once('.'))
                .and_then(|(major, minor)| Some((major.parse().ok()?, minor.parse().ok()?)));
            let file_name = filespec
                .get(&Name::from("UF"))
                .or_else(|| filespec.get(&Name::from("F")))
                .and_then(|v| match self.resolve(v).ok()? {
                    Object::String(s) => Some(decode_text_string(s.as_bytes())),
                    _ => None,
                })
                .unwrap_or_else(|| String::from_utf8_lossy(&key).into_owned());
            // The payload bytes: /EF /F run through its *transport* filter chain only.
            let Some(ef) = filespec.get(&Name::from("EF")) else {
                continue;
            };
            let Ok(Object::Dictionary(ef)) = self.resolve(ef) else {
                continue;
            };
            let Some(stream) = ef.get(&Name::from("F")) else {
                continue;
            };
            let Ok(Object::Stream(stream)) = self.resolve(stream) else {
                continue;
            };
            let Ok(data) = self.decode_stream(&stream) else {
                continue;
            };
            return Ok(Some(EncryptedPayload {
                file_name,
                filter_subtype,
                version,
                data,
            }));
        }
        Ok(None)
    }
}
