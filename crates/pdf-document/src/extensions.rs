//! Developer extensions (`/Extensions`, ISO 32000 §7.12): the catalog dictionary that declares
//! which registered developer-defined extensions a document uses.
//!
//! The extensions dictionary maps registered prefix names (Annex E) to developer extensions
//! dictionaries — or, since PDF 2.0, to *arrays* of them (Table 48). Everything in it is required
//! to be a direct object nested in the catalog. Authoring goes through
//! [`Builder::developer_extension`](crate::Builder::developer_extension); this module holds the
//! shared data type and the read side.

use pdf_cos::{Name, Object};

use crate::names::decode_text_string;
use crate::{Document, Result};

/// One developer extension declaration (§7.12, Table 49) under a registered `prefix` (Table 48).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeveloperExtension {
    /// The registered developer prefix name (the key in the extensions dictionary, Annex E).
    pub prefix: String,
    /// `/BaseVersion` — the PDF version this extension applies to (e.g. `(1, 7)` for `/1.7`).
    pub base_version: (u8, u8),
    /// `/ExtensionLevel` — the developer-assigned level, monotonically increasing per base version.
    pub extension_level: i64,
    /// `/URL` (optional, PDF 2.0) — documentation for the extension.
    pub url: Option<String>,
    /// `/ExtensionRevision` (optional, PDF 2.0) — additional revision information.
    pub revision: Option<String>,
}

impl Document {
    /// The developer extensions declared in the catalog's `/Extensions` dictionary (§7.12), in
    /// dictionary order — accepting both the single-dictionary (1.7) and array (2.0) forms per
    /// prefix. Entries that are not extensions dictionaries (including the `/Type` marker) are
    /// skipped; malformed entries are dropped, not errors (best-effort, DESIGN.md §3.4).
    pub fn developer_extensions(&self) -> Result<Vec<DeveloperExtension>> {
        let catalog = self.catalog()?;
        let Some(extensions) = catalog.get(&Name::from("Extensions")) else {
            return Ok(Vec::new());
        };
        // §7.12.1 requires direct objects, but reading stays lenient (hostile/malformed input).
        let Ok(Object::Dictionary(extensions)) = self.resolve(extensions) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for (prefix, value) in extensions.iter() {
            if prefix.as_bytes() == b"Type" {
                continue;
            }
            let Some(prefix) = prefix.as_str() else {
                continue;
            };
            match self.resolve(value)? {
                Object::Dictionary(d) => out.extend(parse_extension(prefix, &d)),
                Object::Array(a) => {
                    for item in a.iter() {
                        if let Ok(Object::Dictionary(d)) = self.resolve(item) {
                            out.extend(parse_extension(prefix, &d));
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(out)
    }
}

/// Parse one developer extensions dictionary (Table 49); `None` if the required keys are missing.
fn parse_extension(prefix: &str, d: &pdf_cos::Dictionary) -> Option<DeveloperExtension> {
    let base = d.get_name(&Name::from("BaseVersion"))?;
    let base = std::str::from_utf8(base.as_bytes()).ok()?;
    let (major, minor) = base.split_once('.')?;
    let base_version = (major.parse().ok()?, minor.parse().ok()?);
    let extension_level = d.get_integer(&Name::from("ExtensionLevel"))?;
    let text = |key: &str| match d.get(&Name::from(key)) {
        Some(Object::String(s)) => Some(decode_text_string(s.as_bytes())),
        _ => None,
    };
    Some(DeveloperExtension {
        prefix: prefix.to_string(),
        base_version,
        extension_level,
        url: text("URL"),
        revision: text("ExtensionRevision"),
    })
}
