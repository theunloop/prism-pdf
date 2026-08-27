//! Reading and filling interactive form fields (ISO 32000-1 §12.7, AcroForm).
//!
//! A field is a node in a tree (§12.7.3.1): non-terminal nodes group children that carry their own
//! partial name `/T`; terminal nodes hold the value `/V` and may merge with their widget annotation.
//! The **fully-qualified name** (§12.7.3.2) joins a parent's with the child's `/T` by `.`; `/FT`
//! (type) and `/V` (value) are inherited down the tree. [`Document::form_fields`] flattens this to
//! the terminal fields; [`Document::fill_form`] sets values and re-emits the document.
//!
//! Reading is best-effort and bounded against hostile input (DESIGN.md §3.4): depth, field count
//! and `/Kids` cycles are all capped.

use std::collections::BTreeSet;

use pdf_cos::{Dictionary, Name, Object, ObjectId, PdfString};

use crate::builder::text_string;
use crate::names::decode_text_string;
use crate::{Document, Result, RewriteMode, SignatureEffect, StructureEffect, TransformReport};

/// Maximum field-tree depth and total fields collected (anti-DoS).
const MAX_FIELD_DEPTH: usize = 64;
const MAX_FIELDS: usize = 1 << 16;

/// One terminal interactive form field (§12.7.4).
#[derive(Clone, PartialEq, Debug)]
pub struct FormField {
    /// The fully-qualified field name (§12.7.3.2), e.g. `"address.city"`.
    pub name: String,
    /// The field type from `/FT` (`Tx` text, `Btn` button/checkbox/radio, `Ch` choice, `Sig`
    /// signature), inherited from an ancestor when the terminal field omits it. Empty if unknown.
    pub field_type: String,
    /// The current value `/V` as text: the string for `Tx`/`Ch`, the selected state name for `Btn`,
    /// comma-joined entries for a multi-select `Ch`. `None` when unset or non-textual (e.g. a `Sig`).
    pub value: Option<String>,
}

/// A terminal field captured during the tree walk, retaining its object id for editing.
struct TerminalField {
    name: String,
    field_type: Option<Vec<u8>>,
    value: Option<Object>,
    id: Option<ObjectId>,
}

/// Field attributes inherited down the field tree (§12.7.3.1): `/FT` and `/V`.
#[derive(Clone, Default)]
struct Inherited {
    field_type: Option<Vec<u8>>,
    value: Option<Object>,
}

/// Mutable accumulator threaded through the field-tree walk: collected fields and the visited-id
/// cycle guard.
struct FieldWalk {
    out: Vec<TerminalField>,
    visited: BTreeSet<ObjectId>,
}

impl Document {
    /// Read the document's interactive form fields (§12.7): one [`FormField`] per terminal field.
    /// Empty when the document has no AcroForm.
    pub fn form_fields(&self) -> Result<Vec<FormField>> {
        Ok(self
            .collect_terminal_fields()?
            .into_iter()
            .map(|field| FormField {
                name: field.name,
                field_type: field
                    .field_type
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .unwrap_or_default(),
                value: field.value.as_ref().and_then(|v| self.field_value(v)),
            })
            .collect())
    }

    /// Fill form fields by fully-qualified name and re-emit the document as an **incremental update**
    /// (§7.5.6), returning the new bytes. Text/choice values become text strings, button values a
    /// state name. Unknown names are ignored. When any **non-button** field changes,
    /// `/NeedAppearances` is set so viewers regenerate its appearance (this does not synthesize
    /// `/AP` streams itself); a fill touching only buttons skips it — their state switches via
    /// `/AS` against the existing `/AP`, and the key is deprecated in PDF 2.0 (§12.7.3).
    pub fn fill_form(&self, values: &[(&str, &str)]) -> Result<Vec<u8>> {
        let terminals = self.collect_terminal_fields()?;
        let mut changes: Vec<(ObjectId, Object)> = Vec::new();
        let mut needs_regen = false;

        for &(name, new_value) in values {
            let Some(field) = terminals.iter().find(|f| f.name == name) else {
                continue;
            };
            let Some(id) = field.id else {
                continue; // an inline (direct) field can't be overridden by object number
            };
            let Ok(Object::Dictionary(mut dict)) = self.get(id) else {
                continue;
            };
            apply_field_value(&mut dict, field.field_type.as_deref(), new_value);
            // A button switches states via /AS against its existing /AP subdictionary — no
            // appearance regeneration needed. Text/choice values do need it (their /AP, if any,
            // still paints the old value).
            if field.field_type.as_deref() != Some(b"Btn".as_slice()) {
                needs_regen = true;
            }
            changes.push((id, Object::Dictionary(dict)));
        }

        if needs_regen {
            self.request_appearance_regen(&mut changes)?;
        }
        self.save_incremental(&changes)
    }

    /// Form fill with an explicit report that the operation appends an incremental revision.
    pub fn fill_form_with_report(&self, values: &[(&str, &str)]) -> Result<TransformReport> {
        Ok(TransformReport::new(
            self.fill_form(values)?,
            RewriteMode::Incremental,
            SignatureEffect::Preserved,
            StructureEffect::Preserved,
        ))
    }

    /// Walk the `/AcroForm /Fields` tree and capture every terminal field.
    fn collect_terminal_fields(&self) -> Result<Vec<TerminalField>> {
        let catalog = self.catalog()?;
        let Some(acroform) = catalog.get(&Name::from("AcroForm")) else {
            return Ok(Vec::new());
        };
        let Ok(Object::Dictionary(acroform)) = self.resolve(acroform) else {
            return Ok(Vec::new());
        };
        let Some(fields) = acroform.get(&Name::from("Fields")) else {
            return Ok(Vec::new());
        };
        let Ok(Object::Array(fields)) = self.resolve(fields) else {
            return Ok(Vec::new());
        };

        let mut walk = FieldWalk {
            out: Vec::new(),
            visited: BTreeSet::new(),
        };
        for field in fields.iter() {
            self.walk_field(field, "", &Inherited::default(), &mut walk, 0);
        }
        Ok(walk.out)
    }

    /// Visit one field node, emitting it if terminal or recursing into its child fields (§12.7.3.1).
    fn walk_field(
        &self,
        field: &Object,
        prefix: &str,
        inherited: &Inherited,
        walk: &mut FieldWalk,
        depth: usize,
    ) {
        if depth > MAX_FIELD_DEPTH || walk.out.len() >= MAX_FIELDS {
            return;
        }
        let id = match field {
            Object::Reference(id) => Some(*id),
            _ => None,
        };
        if let Some(id) = id {
            if !walk.visited.insert(id) {
                return; // a /Kids cycle
            }
        }
        let Ok(Object::Dictionary(dict)) = self.resolve(field) else {
            return;
        };

        // Fully-qualified name: append this node's partial name /T (§12.7.3.2).
        let name = match self.partial_name(&dict) {
            Some(t) if prefix.is_empty() => t,
            Some(t) => format!("{prefix}.{t}"),
            None => prefix.to_string(),
        };
        // Type and value inherit from the nearest ancestor that sets them.
        let here = Inherited {
            field_type: dict
                .get_name(&Name::from("FT"))
                .map(|n| n.as_bytes().to_vec())
                .or_else(|| inherited.field_type.clone()),
            value: dict
                .get(&Name::from("V"))
                .cloned()
                .or_else(|| inherited.value.clone()),
        };

        // Child *fields* are kids that carry their own /T; kids without one are widget annotations,
        // which do not extend the name tree (so this node is then terminal).
        let field_kids = self.field_children(&dict);
        if field_kids.is_empty() {
            walk.out.push(TerminalField {
                name,
                field_type: here.field_type,
                value: here.value,
                id,
            });
        } else {
            for kid in field_kids {
                self.walk_field(&kid, &name, &here, walk, depth + 1);
            }
        }
    }

    /// Set `/NeedAppearances true` on the AcroForm so viewers regenerate field appearances after a
    /// fill (§12.7.4.3). No-op for an inline AcroForm (rewriting the catalog is out of scope here).
    fn request_appearance_regen(&self, changes: &mut Vec<(ObjectId, Object)>) -> Result<()> {
        let catalog = self.catalog()?;
        if let Some(Object::Reference(id)) = catalog.get(&Name::from("AcroForm")) {
            if let Ok(Object::Dictionary(mut acroform)) = self.get(*id) {
                acroform.insert(Name::from("NeedAppearances"), Object::Boolean(true));
                changes.push((*id, Object::Dictionary(acroform)));
            }
        }
        Ok(())
    }

    /// This field's partial name `/T` decoded as a text string (§12.7.3.2), if it has one.
    fn partial_name(&self, dict: &Dictionary) -> Option<String> {
        match self.resolve(dict.get(&Name::from("T"))?).ok()? {
            Object::String(s) => Some(decode_text_string(s.as_bytes())),
            _ => None,
        }
    }

    /// The `/Kids` entries that are themselves fields (carry a `/T`), resolved to objects.
    fn field_children(&self, dict: &Dictionary) -> Vec<Object> {
        let Some(kids) = dict.get(&Name::from("Kids")) else {
            return Vec::new();
        };
        let Ok(Object::Array(kids)) = self.resolve(kids) else {
            return Vec::new();
        };
        kids.iter()
            .filter(|kid| {
                matches!(self.resolve(kid), Ok(Object::Dictionary(d)) if d.get(&Name::from("T")).is_some())
            })
            .cloned()
            .collect()
    }

    /// Decode a field value `/V` (§12.7.4) into text (see [`FormField::value`]).
    fn field_value(&self, value: &Object) -> Option<String> {
        match self.resolve(value).ok()? {
            Object::String(s) => Some(decode_text_string(s.as_bytes())),
            Object::Name(n) => Some(String::from_utf8_lossy(n.as_bytes()).into_owned()),
            Object::Array(items) => {
                let parts: Vec<String> = items
                    .iter()
                    .filter_map(|item| match self.resolve(item).ok()? {
                        Object::String(s) => Some(decode_text_string(s.as_bytes())),
                        _ => None,
                    })
                    .collect();
                (!parts.is_empty()).then(|| parts.join(", "))
            }
            _ => None,
        }
    }
}

/// Set a terminal field's value `/V` (§12.7.4) for a fill: a state name for a `Btn` (also mirrored
/// to `/AS` so a merged widget shows it), otherwise a text string.
fn apply_field_value(dict: &mut Dictionary, field_type: Option<&[u8]>, value: &str) {
    match field_type {
        Some(b"Btn") => {
            let state = Name::from(value);
            dict.insert(Name::from("V"), Object::Name(state.clone()));
            dict.insert(Name::from("AS"), Object::Name(state));
        }
        _ => {
            dict.insert(
                Name::from("V"),
                Object::String(PdfString::from(text_string(value))),
            );
        }
    }
}
