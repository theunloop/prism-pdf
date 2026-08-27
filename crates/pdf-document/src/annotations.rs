//! Reading page annotations (ISO 32000-1 §12.5): the interactive overlays — links, notes, form
//! widgets, highlights — that a page lists in its `/Annots` array.
//!
//! This is the read half of M12: it surfaces each annotation's subtype, rectangle, text contents
//! (the note body or accessibility text §12.5.2) and, for link annotations, the external URI of a
//! `URI` action (§12.6.4.7) or the target page of an internal `/Dest` / `GoTo` destination
//! (§12.3.2) — recovering text and links a page carries outside its content stream. Parsing is
//! best-effort (DESIGN.md §3): a malformed annotation contributes nothing.

use std::collections::HashMap;

use pdf_cos::{Dictionary, Name, Object, ObjectId};

use crate::names::decode_text_string;
use crate::{Document, Result};

/// One page annotation (§12.5).
#[derive(Clone, PartialEq, Debug)]
pub struct Annotation {
    /// The annotation `/Subtype` (`Link`, `Text`, `Widget`, `Highlight`, …).
    pub subtype: String,
    /// The annotation rectangle `[llx lly urx ury]` in default user space (`/Rect`, §12.5.2).
    pub rect: [f64; 4],
    /// The text contents (`/Contents`, §12.5.2) — a note's body, or accessibility text — if present.
    pub contents: Option<String>,
    /// For a link annotation with a `URI` action (§12.6.4.7), the external URI it points to.
    pub uri: Option<String>,
    /// For a link annotation that jumps within the document (`/Dest` or a `GoTo` action, §12.3.2),
    /// the 0-based index of the target page.
    pub dest_page: Option<usize>,
}

impl Document {
    /// Read the annotations of `page` (§12.5): resolve its `/Annots` array and describe each entry.
    /// Returns an empty vector when the page has none. Malformed entries are skipped.
    pub fn annotations(&self, page: &Dictionary) -> Result<Vec<Annotation>> {
        let Some(annots) = page.get(&Name::from("Annots")) else {
            return Ok(Vec::new());
        };
        let Object::Array(array) = self.resolve(annots)? else {
            return Ok(Vec::new());
        };
        let dicts: Vec<Dictionary> = array
            .iter()
            .filter_map(|entry| match self.resolve(entry) {
                Ok(Object::Dictionary(annot)) => Some(annot),
                _ => None,
            })
            .collect();
        // Resolving an internal link's target needs a page-object → index map; build it once, and
        // only when a link with a destination is actually present (most pages have none).
        let page_index = if dicts.iter().any(has_destination) {
            Some(self.page_index_map()?)
        } else {
            None
        };

        let mut out = Vec::with_capacity(dicts.len());
        for annot in &dicts {
            out.push(self.read_annotation(annot, page_index.as_ref())?);
        }
        Ok(out)
    }

    /// A map from each page's object id to its 0-based index (§7.7.3), for resolving link targets.
    pub(crate) fn page_index_map(&self) -> Result<HashMap<ObjectId, usize>> {
        Ok(self
            .page_entries()?
            .into_iter()
            .enumerate()
            .filter_map(|(index, (id, _))| id.map(|id| (id, index)))
            .collect())
    }

    /// Describe one resolved annotation dictionary.
    fn read_annotation(
        &self,
        annot: &Dictionary,
        page_index: Option<&HashMap<ObjectId, usize>>,
    ) -> Result<Annotation> {
        let subtype = annot
            .get_name(&Name::from("Subtype"))
            .map(|n| String::from_utf8_lossy(n.as_bytes()).into_owned())
            .unwrap_or_default();
        let rect = self.rect(annot)?;
        let contents = self.text_entry(annot, "Contents");
        let uri = self.link_uri(annot)?;
        let dest_page = page_index.and_then(|map| self.link_dest_page(annot, map));
        Ok(Annotation {
            subtype,
            rect,
            contents,
            uri,
            dest_page,
        })
    }

    /// The 0-based target page of a dictionary's internal destination (§12.3.2), if any: an explicit
    /// `/Dest`, or a `GoTo` action's `/D`, resolving a named destination through the `/Dests` name
    /// tree or catalog dictionary. Shared by link annotations and outline items.
    pub(crate) fn link_dest_page(
        &self,
        annot: &Dictionary,
        page_index: &HashMap<ObjectId, usize>,
    ) -> Option<usize> {
        let dest = self.destination(annot)?;
        // An explicit destination is `[page /Fit …]`: its first element is the target page, either
        // a page reference (resolved via the index map) or a 0-based page number (remote-style).
        match dest.as_array()?.iter().next()? {
            Object::Reference(id) => page_index.get(id).copied(),
            Object::Integer(n) => usize::try_from(*n).ok(),
            _ => None,
        }
    }

    /// Resolve a link's destination to an explicit destination array (§12.3.2.2): from `/Dest` or a
    /// `GoTo` action's `/D`, dereferencing a named destination and unwrapping a `<< /D … >>` holder.
    fn destination(&self, annot: &Dictionary) -> Option<Object> {
        let raw = match annot.get(&Name::from("Dest")) {
            Some(dest) => dest.clone(),
            None => {
                // /A → a GoTo action carrying the destination in /D.
                let Object::Dictionary(action) = self.resolve(annot.get(&Name::from("A"))?).ok()?
                else {
                    return None;
                };
                match action.get_name(&Name::from("S")).map(Name::as_bytes) {
                    Some(b"GoTo") => action.get(&Name::from("D"))?.clone(),
                    // GoToDp (§12.6.4.5, PDF 2.0): the target is the /Start page of the
                    // referenced document part (§14.12) — synthesise an explicit destination.
                    Some(b"GoToDp") => {
                        let Object::Dictionary(dpart) =
                            self.resolve(action.get(&Name::from("Dp"))?).ok()?
                        else {
                            return None;
                        };
                        return Some(Object::Array(pdf_cos::Array::from(vec![
                            dpart.get(&Name::from("Start"))?.clone(),
                            Object::Name(Name::from("Fit")),
                        ])));
                    }
                    _ => return None,
                }
            }
        };
        self.resolve_destination(&raw)
    }

    /// Turn a destination value — an explicit array, a named destination (name or string), or a
    /// `<< /D … >>` holder — into its explicit destination array (§12.3.2.2/§12.3.2.3).
    fn resolve_destination(&self, value: &Object) -> Option<Object> {
        match self.resolve(value).ok()? {
            array @ Object::Array(_) => Some(array),
            // `<< /D [array] >>` holder, used in the name tree / Dests dictionary.
            Object::Dictionary(holder) => self.resolve(holder.get(&Name::from("D"))?).ok(),
            // A named destination: a name keys the catalog `/Dests` dict; a string keys the
            // `/Names /Dests` name tree (§12.3.2.3).
            Object::Name(name) => self.named_dest_from_catalog(name.as_bytes()),
            Object::String(key) => self.named_dest_from_tree(key.as_bytes()),
            _ => None,
        }
    }

    /// Look up a name-keyed destination in the catalog `/Dests` dictionary (§12.3.2.3).
    fn named_dest_from_catalog(&self, name: &[u8]) -> Option<Object> {
        let catalog = self.catalog().ok()?;
        let Object::Dictionary(dests) = self.resolve(catalog.get(&Name::from("Dests"))?).ok()?
        else {
            return None;
        };
        self.resolve_destination(dests.get(&Name::new(name.to_vec()))?)
    }

    /// Look up a string-keyed destination in the `/Names /Dests` name tree (§7.7.4 / §12.3.2.3).
    fn named_dest_from_tree(&self, key: &[u8]) -> Option<Object> {
        let (_, value) = self
            .names("Dests")
            .ok()?
            .into_iter()
            .find(|(k, _)| k == key)?;
        self.resolve_destination(&value)
    }

    /// Resolve the `/Rect` array to four numbers (§12.5.2); a missing/short one yields zeros.
    fn rect(&self, annot: &Dictionary) -> Result<[f64; 4]> {
        let mut rect = [0.0; 4];
        if let Some(obj) = annot.get(&Name::from("Rect"))
            && let Object::Array(array) = self.resolve(obj)?
        {
            for (slot, item) in rect.iter_mut().zip(array.iter()) {
                if let Some(value) = self.resolve(item)?.as_f64() {
                    *slot = value;
                }
            }
        }
        Ok(rect)
    }

    /// The URI of a link annotation's `URI` action (`/A` with `/S /URI`, §12.6.4.7), if any.
    fn link_uri(&self, annot: &Dictionary) -> Result<Option<String>> {
        let Some(action) = annot.get(&Name::from("A")) else {
            return Ok(None);
        };
        let Object::Dictionary(action) = self.resolve(action)? else {
            return Ok(None);
        };
        if action.get_name(&Name::from("S")).map(Name::as_bytes) != Some(b"URI") {
            return Ok(None);
        }
        Ok(self.text_entry(&action, "URI"))
    }

    /// Resolve `dict[key]` to a PDF text string and decode it (§7.9.2.2), or `None`.
    fn text_entry(&self, dict: &Dictionary, key: &str) -> Option<String> {
        match self.resolve(dict.get(&Name::from(key))?).ok()? {
            Object::String(s) => Some(decode_text_string(s.as_bytes())),
            _ => None,
        }
    }
}

/// Whether an annotation might carry an internal destination (`/Dest`, or an action in `/A` that
/// could be `GoTo`) — a cheap pre-check that avoids building the page-index map when unneeded.
fn has_destination(annot: &Dictionary) -> bool {
    annot.get(&Name::from("Dest")).is_some() || annot.get(&Name::from("A")).is_some()
}
