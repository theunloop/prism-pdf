//! Name trees (ISO 32000-1 §7.7.4) and embedded-file attachment reading (§7.11.4 / §14.13).
//!
//! A *name tree* maps string keys to objects through a balanced tree of nodes — each an
//! intermediate node (`/Kids`) or a leaf (`/Names`, an alternating key/value array). The catalog's
//! `/Names` dictionary roots one tree per category (`/EmbeddedFiles`, `/Dests`, …). [`Document::names`]
//! walks any such tree; [`Document::attachments`] uses the `EmbeddedFiles` tree to read back the
//! files embedded via [`crate::Builder::attach_file`] — the read half of the e-invoice
//! (FatturaPA/ZUGFeRD) round-trip.
//!
//! Traversal is best-effort and bounded against hostile input (DESIGN.md §3.4): a malformed node
//! contributes nothing, `/Kids` references are cycle-guarded, and both depth and total entries are
//! capped.

use std::collections::BTreeSet;

use pdf_cos::{Dictionary, Name, Object};

use crate::{Document, Result};

/// Maximum name-tree depth. Legitimate trees are shallow (≈ log n); this bounds recursion.
const MAX_NAME_TREE_DEPTH: usize = 64;
/// Maximum number of entries collected from one tree (anti-DoS).
const MAX_NAME_TREE_ENTRIES: usize = 1 << 16;

/// One embedded file attachment read back from a document (§7.11.4): its name, the decoded file
/// bytes, and the descriptive metadata from its `/Filespec` and `/EmbeddedFile` stream.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ExtractedAttachment {
    /// The file name (`/UF` preferred, else `/F`, else the name-tree key), as a decoded text string.
    pub name: String,
    /// The decoded file bytes (the `/EF /F` embedded-file stream, run through its filter chain).
    pub data: Vec<u8>,
    /// The embedded file's MIME type (`/EmbeddedFile /Subtype`), if recorded.
    pub mime: Option<String>,
    /// How the file relates to the document (`/AFRelationship`, §14.13), if recorded.
    pub relationship: Option<String>,
    /// A human-readable description (`/Desc`), if recorded.
    pub description: Option<String>,
}

impl Document {
    /// Walk the catalog name tree (§7.7.4) for `category` (e.g. `"EmbeddedFiles"`, `"Dests"`),
    /// returning its `(key, value)` leaves. `key` is the raw PDF string; `value` is left unresolved
    /// (often an indirect reference). Empty when the document has no such tree.
    pub fn names(&self, category: &str) -> Result<Vec<(Vec<u8>, Object)>> {
        let catalog = self.catalog()?;
        let Some(names_obj) = catalog.get(&Name::from("Names")) else {
            return Ok(Vec::new());
        };
        let Ok(names_dict) = self.resolve_dict(names_obj, "Names") else {
            return Ok(Vec::new());
        };
        let Some(root_obj) = names_dict.get(&Name::from(category)) else {
            return Ok(Vec::new());
        };
        let Ok(root) = self.resolve_dict(root_obj, "name tree") else {
            return Ok(Vec::new());
        };

        let mut out = Vec::new();
        let mut visited = BTreeSet::new();
        self.walk_name_tree(&root, &mut visited, &mut out, 0);
        Ok(out)
    }

    /// Collect the leaves of one name-tree node and recurse into its `/Kids` (§7.7.4). Best-effort:
    /// malformed parts are skipped; bounded by depth, the entry cap, and a visited-id cycle guard.
    fn walk_name_tree(
        &self,
        node: &Dictionary,
        visited: &mut BTreeSet<pdf_cos::ObjectId>,
        out: &mut Vec<(Vec<u8>, Object)>,
        depth: usize,
    ) {
        if depth > MAX_NAME_TREE_DEPTH || out.len() >= MAX_NAME_TREE_ENTRIES {
            return;
        }
        // Leaf entries: a flat [key1 val1 key2 val2 …] array.
        if let Some(names) = node.get(&Name::from("Names"))
            && let Ok(Object::Array(arr)) = self.resolve(names)
        {
            let flat: Vec<_> = arr.iter().collect();
            for pair in flat.as_chunks::<2>().0 {
                if out.len() >= MAX_NAME_TREE_ENTRIES {
                    return;
                }
                if let Ok(Object::String(key)) = self.resolve(pair[0]) {
                    out.push((key.as_bytes().to_vec(), pair[1].clone()));
                }
            }
        }
        // Intermediate node: recurse into child nodes, guarding against reference cycles.
        if let Some(kids) = node.get(&Name::from("Kids"))
            && let Ok(Object::Array(arr)) = self.resolve(kids)
        {
            for kid in arr.iter() {
                if let Object::Reference(id) = kid
                    && !visited.insert(*id)
                {
                    continue; // already visited — a cycle
                }
                if let Ok(child) = self.resolve_dict(kid, "name tree node") {
                    self.walk_name_tree(&child, visited, out, depth + 1);
                }
            }
        }
    }

    /// Read every embedded file attachment (§7.11.4) from the catalog `/Names /EmbeddedFiles` tree.
    /// Each entry's `/EmbeddedFile` stream is decoded through its filter chain. Attachments that
    /// cannot be resolved or decoded are skipped (best-effort, DESIGN.md §3.4).
    pub fn attachments(&self) -> Result<Vec<ExtractedAttachment>> {
        let mut out = Vec::new();
        for (key, value) in self.names("EmbeddedFiles")? {
            let Ok(Object::Dictionary(filespec)) = self.resolve(&value) else {
                continue;
            };
            if let Some(att) = self.read_filespec(&key, &filespec)? {
                out.push(att);
            }
        }
        Ok(out)
    }

    /// Build an [`ExtractedAttachment`] from a `/Filespec` dictionary (§7.11.3), or `None` if it has
    /// no decodable embedded-file stream.
    fn read_filespec(
        &self,
        key: &[u8],
        filespec: &Dictionary,
    ) -> Result<Option<ExtractedAttachment>> {
        // /EF: the embedded-file dictionary, keyed by the same /F, /UF slots as the file spec.
        let Some(ef_obj) = filespec.get(&Name::from("EF")) else {
            return Ok(None);
        };
        let Ok(Object::Dictionary(ef)) = self.resolve(ef_obj) else {
            return Ok(None);
        };
        let Some(stream_obj) = ef
            .get(&Name::from("F"))
            .or_else(|| ef.get(&Name::from("UF")))
        else {
            return Ok(None);
        };
        let Ok(Object::Stream(stream)) = self.resolve(stream_obj) else {
            return Ok(None);
        };
        let data = self.decode_stream(&stream)?;

        let mime = stream
            .dict()
            .get_name(&Name::from("Subtype"))
            .map(|n| String::from_utf8_lossy(n.as_bytes()).into_owned());
        let name = self
            .text_string_entry(filespec, "UF")
            .or_else(|| self.text_string_entry(filespec, "F"))
            .unwrap_or_else(|| decode_text_string(key));
        let relationship = filespec
            .get_name(&Name::from("AFRelationship"))
            .map(|n| String::from_utf8_lossy(n.as_bytes()).into_owned());
        let description = self.text_string_entry(filespec, "Desc");

        Ok(Some(ExtractedAttachment {
            name,
            data,
            mime,
            relationship,
            description,
        }))
    }

    /// Resolve `dict[key]` to a PDF text string and decode it (§7.9.2.2), or `None`.
    fn text_string_entry(&self, dict: &Dictionary, key: &str) -> Option<String> {
        match self.resolve(dict.get(&Name::from(key))?).ok()? {
            Object::String(s) => Some(decode_text_string(s.as_bytes())),
            _ => None,
        }
    }
}

/// Decode a PDF text string (§7.9.2.2): UTF-16BE when it opens with a `FE FF` BOM, **UTF-8** when it
/// opens with the PDF 2.0 `EF BB BF` BOM, otherwise PDFDocEncoding approximated as Latin-1 (the
/// three agree across the printable ASCII range). Language escape sequences (§7.9.2.2.2 — an
/// `ESC` pair bracketing an ISO 639 + optional ISO 3166 tag, valid in the Unicode forms) annotate
/// language rather than carry text, and are stripped from the result.
pub fn decode_text_string(bytes: &[u8]) -> String {
    if let Some(units) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let iter = units
            .chunks(2)
            .map(|c| (u16::from(c[0]) << 8) | u16::from(c.get(1).copied().unwrap_or(0)));
        let decoded: String = char::decode_utf16(iter)
            .map(|r| r.unwrap_or('\u{FFFD}'))
            .collect();
        strip_language_escapes(&decoded)
    } else if let Some(utf8) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        // PDF 2.0 UTF-8 text string (§7.9.2.2): decode the bytes after the BOM, replacing invalid
        // sequences rather than failing.
        strip_language_escapes(&String::from_utf8_lossy(utf8))
    } else {
        bytes.iter().map(|&b| char::from(b)).collect()
    }
}

/// Remove language escape sequences (§7.9.2.2.2) from a decoded text string: each `U+001B … U+001B`
/// run brackets a language tag, not text. Best-effort on malformed input — an unpaired trailing
/// escape drops only the escape character itself, never the text after it.
fn strip_language_escapes(s: &str) -> String {
    if !s.contains('\u{1B}') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find('\u{1B}') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('\u{1B}') {
            Some(end) => rest = &after[end + 1..],
            None => rest = after, // unpaired: drop only the escape character
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_ascii_utf16_and_utf8_text_strings() {
        assert_eq!(decode_text_string(b"invoice.xml"), "invoice.xml");
        // UTF-16BE BOM + "AB".
        assert_eq!(
            decode_text_string(&[0xFE, 0xFF, 0x00, 0x41, 0x00, 0x42]),
            "AB"
        );
        // PDF 2.0 UTF-8 BOM (EF BB BF) + "é" (C3 A9) → decoded as UTF-8, not Latin-1.
        assert_eq!(decode_text_string(&[0xEF, 0xBB, 0xBF, 0xC3, 0xA9]), "é");
    }

    #[test]
    fn strips_language_escape_sequences() {
        // §7.9.2.2.2: ESC + "enUS" + ESC brackets a language tag inside a UTF-16BE string — the
        // tag annotates language and must not surface as text.
        let mut bytes = vec![0xFE, 0xFF];
        for unit in "\u{1B}enUS\u{1B}Hello \u{1B}de\u{1B}Welt".encode_utf16() {
            bytes.extend_from_slice(&unit.to_be_bytes());
        }
        assert_eq!(decode_text_string(&bytes), "Hello Welt");

        // Same in the PDF 2.0 UTF-8 form, and an unpaired escape drops only itself.
        let mut utf8 = vec![0xEF, 0xBB, 0xBF];
        utf8.extend_from_slice("\u{1B}frFR\u{1B}Bonjour".as_bytes());
        assert_eq!(decode_text_string(&utf8), "Bonjour");
        let mut lone = vec![0xFE, 0xFF];
        for unit in "A\u{1B}B".encode_utf16() {
            lone.extend_from_slice(&unit.to_be_bytes());
        }
        assert_eq!(decode_text_string(&lone), "AB");
    }
}
