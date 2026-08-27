//! Reading the document outline / bookmarks (ISO 32000-1 §12.3.3): the `/Outlines` tree in the
//! catalog, flattened into a nested [`OutlineItem`] hierarchy with each item's title and target
//! page. The read counterpart to authoring outlines via `Builder::outline`.
//!
//! The outline is a doubly-linked tree (`/First`/`/Last`, `/Next`/`/Prev`, `/Parent`); this walks
//! the `/First`→`/Next` chains depth-first. Best-effort and bounded against hostile input
//! (DESIGN.md §3.4): depth, item count and `/First`/`/Next` reference cycles are all capped.

use std::collections::BTreeSet;

use pdf_cos::{Dictionary, Name, Object, ObjectId};

use crate::names::decode_text_string;
use crate::{Document, Result};

/// Maximum outline depth and total items collected (anti-DoS).
const MAX_OUTLINE_DEPTH: usize = 64;
const MAX_OUTLINE_ITEMS: usize = 1 << 16;

/// One outline (bookmark) entry (§12.3.3).
#[derive(Clone, PartialEq, Debug)]
pub struct OutlineItem {
    /// The bookmark title (`/Title`, decoded text string §7.9.2.2).
    pub title: String,
    /// The 0-based page the bookmark jumps to (`/Dest` or a `GoTo` action), if it resolves.
    pub dest_page: Option<usize>,
    /// Child bookmarks nested under this one.
    pub children: Vec<OutlineItem>,
}

impl Document {
    /// Read the document outline (§12.3.3) as a nested list of top-level [`OutlineItem`]s. Empty
    /// when the document has no `/Outlines`.
    pub fn outline(&self) -> Result<Vec<OutlineItem>> {
        let catalog = self.catalog()?;
        let Some(outlines) = catalog.get(&Name::from("Outlines")) else {
            return Ok(Vec::new());
        };
        let Ok(Object::Dictionary(root)) = self.resolve(outlines) else {
            return Ok(Vec::new());
        };
        let page_index = self.page_index_map()?;
        let mut visited = BTreeSet::new();
        Ok(self.read_outline_children(&root, &page_index, &mut visited, 0))
    }

    /// Collect a parent's children by following `/First` then the `/Next` sibling chain (§12.3.3).
    fn read_outline_children(
        &self,
        parent: &Dictionary,
        page_index: &std::collections::HashMap<ObjectId, usize>,
        visited: &mut BTreeSet<ObjectId>,
        depth: usize,
    ) -> Vec<OutlineItem> {
        let mut items = Vec::new();
        if depth > MAX_OUTLINE_DEPTH {
            return items;
        }
        let mut current = parent.get(&Name::from("First")).cloned();
        while let Some(entry) = current {
            if items.len() >= MAX_OUTLINE_ITEMS {
                break;
            }
            if let Object::Reference(id) = &entry {
                if !visited.insert(*id) {
                    break; // a cycle in the /First or /Next chain
                }
            }
            let Ok(Object::Dictionary(item)) = self.resolve(&entry) else {
                break;
            };
            let title = self.outline_title(&item).unwrap_or_default();
            let dest_page = self.link_dest_page(&item, page_index);
            let children = self.read_outline_children(&item, page_index, visited, depth + 1);
            items.push(OutlineItem {
                title,
                dest_page,
                children,
            });
            current = item.get(&Name::from("Next")).cloned();
        }
        items
    }

    /// An outline item's `/Title` decoded as a text string (§7.9.2.2).
    fn outline_title(&self, item: &Dictionary) -> Option<String> {
        match self.resolve(item.get(&Name::from("Title"))?).ok()? {
            Object::String(s) => Some(decode_text_string(s.as_bytes())),
            _ => None,
        }
    }
}
