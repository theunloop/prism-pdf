//! Minimum PDF header version required by an object set (§7.5.2 + the "Adobe extensions" feature
//! lineage that ISO 32000-2 consolidates).
//!
//! This is the single feature→version table shared by the **producer** (Builder auto-stamp and the
//! `build_for`/`save_as` construct gate, M17) and the **checker** (`Document::min_pdf_version` /
//! the `verify_base` harness, M18 Phase 2). The rule: declaring a version *higher* than needed is
//! harmless; declaring one *lower* than a construct requires is the conformance violation. We floor
//! at 1.4 (the lowest target in the roadmap's per-standard table) — features below 1.4 don't lower
//! the stamp further.
//!
//! Detection is deliberately limited to constructs Prism PDF can actually emit or commonly reads;
//! an unrecognised construct never raises the floor (so the result is a sound *lower bound*, and
//! the producer's stamp is always ≥ the true minimum for what we write). Writer-choice features
//! that are not visible in the object set — the cross-reference *stream* form (≥1.5) and the
//! encryption method (AES-128 ≥1.6, AES-256 ≥2.0) — are applied as additional floors by the
//! callers that make those choices, not here.

use pdf_cos::{Dictionary, Name, Object, ObjectId};

/// One construct in an object set together with the minimum PDF version it requires — the
/// diagnostic unit of the M17 construct gate ("which feature forces which version").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VersionRequirement {
    /// The minimum `(major, minor)` header version the construct is valid at.
    pub version: (u8, u8),
    /// A short human-readable name of the construct, citing its ISO 32000 section.
    pub construct: &'static str,
}

/// The lowest PDF version whose feature set covers every construct in `objects`, floored at (1, 4).
#[must_use]
pub fn min_version(objects: &[(ObjectId, Object)]) -> (u8, u8) {
    requirements(objects)
        .iter()
        .fold((1u8, 4u8), |v, r| v.max(r.version))
}

/// The worst construct in `objects` that exceeds `target`, if any — the diagnostic for a
/// version-targeted producer (M17 Phase 3): `None` means the object set fits inside `target`.
/// When several constructs exceed the target, the one with the highest requirement is returned
/// (ties: first found), so the error names what actually forces the version up.
#[must_use]
pub fn version_violation(
    objects: &[(ObjectId, Object)],
    target: (u8, u8),
) -> Option<VersionRequirement> {
    requirements(objects)
        .into_iter()
        .filter(|r| r.version > target)
        .max_by_key(|r| r.version)
}

/// Every version-raising construct found in `objects` (unrecognised constructs are not reported —
/// see the module docs on why this stays a sound lower bound).
fn requirements(objects: &[(ObjectId, Object)]) -> Vec<VersionRequirement> {
    let mut out = Vec::new();
    for (_, obj) in objects {
        match obj {
            Object::Stream(s) => scan_dict(&mut out, s.dict()),
            Object::Dictionary(d) => scan_dict(&mut out, d),
            _ => {}
        }
        // A UTF-8 text string (BOM `EF BB BF`, §7.9.2.2) is a PDF 2.0 construct — scan anywhere it
        // can nest (Info/outline/Alt values live in dicts and arrays).
        if has_utf8_text_string(obj) {
            found(&mut out, (2, 0), "UTF-8 text string (§7.9.2.2)");
        }
    }
    out
}

/// Whether `obj` (or anything nested within it) is a UTF-8 text string — a PDF string carrying the
/// UTF-8 byte-order mark `EF BB BF` (§7.9.2.2), which only PDF 2.0 permits.
fn has_utf8_text_string(obj: &Object) -> bool {
    match obj {
        Object::String(s) => s.as_bytes().starts_with(&[0xEF, 0xBB, 0xBF]),
        Object::Array(a) => a.iter().any(has_utf8_text_string),
        Object::Dictionary(d) => d.iter().any(|(_, val)| has_utf8_text_string(val)),
        Object::Stream(s) => s.dict().iter().any(|(_, val)| has_utf8_text_string(val)),
        _ => false,
    }
}

/// Record a construct that requires version `v`.
fn found(out: &mut Vec<VersionRequirement>, version: (u8, u8), construct: &'static str) {
    out.push(VersionRequirement { version, construct });
}

/// Inspect one dictionary (a bare dict or a stream's dict) for version-raising keys.
fn scan_dict(out: &mut Vec<VersionRequirement>, d: &Dictionary) {
    // Filters: JPXDecode (JPEG 2000) is the only Prism PDF-relevant ≥1.5 filter. JBIG2Decode and
    // the rest are ≤1.4. (§7.4)
    if let Some(filter) = d.get(&Name::from("Filter")) {
        for name in names_of(filter) {
            if name == b"JPXDecode" {
                found(out, (1, 5), "JPXDecode (JPEG 2000) filter (§7.4.9)");
            }
        }
    }

    // Object type markers.
    if let Some(ty) = d.get_name(&Name::from("Type")) {
        match ty.as_bytes() {
            // object stream §7.5.7
            b"ObjStm" => found(out, (1, 5), "object stream (§7.5.7)"),
            // document parts §14.12 (PDF 2.0)
            b"DPart" | b"DPartRoot" => found(out, (2, 0), "document part (§14.12)"),
            // structure namespaces §14.7.4 (PDF 2.0)
            b"Namespace" => found(out, (2, 0), "structure namespace (§14.7.4)"),
            // Associated files (/AF, §14.13) placed on a page/annotation/XObject/struct-element is a
            // PDF 2.0 feature; catalog-level /AF predates it as a PDF/A-3 (1.7) extension (not here).
            b"Page" | b"Annot" | b"XObject" | b"StructElem"
                if d.get(&Name::from("AF")).is_some() =>
            {
                found(
                    out,
                    (2, 0),
                    "associated file (/AF) on a page/annotation/XObject/structure element (§14.13)",
                );
            }
            // An encrypted payload dictionary on a filespec (§7.6.7, Table 28) marks an
            // unencrypted wrapper document — a PDF 2.0 feature.
            b"Filespec" if d.get(&Name::from("EP")).is_some() => {
                found(out, (2, 0), "encrypted payload filespec (/EP, §7.6.7)");
            }
            _ => {}
        }
        // Page /Tabs (annotation tab order, Table 30) is PDF 1.5.
        if ty.as_bytes() == b"Page" && d.get(&Name::from("Tabs")).is_some() {
            found(out, (1, 5), "page /Tabs annotation tab order (Table 30)");
        }
        // A page-level OutputIntent (§14.11.5) is PDF 2.0 (catalog-level predates it).
        if ty.as_bytes() == b"Page" && d.get(&Name::from("OutputIntents")).is_some() {
            found(out, (2, 0), "page-level OutputIntent (§14.11.5)");
        }
        // A structure element's /Ref (element-to-element references, Table 355) is PDF 2.0.
        if ty.as_bytes() == b"StructElem" && d.get(&Name::from("Ref")).is_some() {
            found(out, (2, 0), "structure-element /Ref (Table 355)");
        }
        // A GoTo action carrying a structure destination (/SD, §12.3.2.3) is PDF 2.0. The action
        // dictionary is nested in the annotation's /A, so inspect it from the Annot object.
        if ty.as_bytes() == b"Annot"
            && let Some(Object::Dictionary(action)) = d.get(&Name::from("A"))
        {
            if action.get(&Name::from("SD")).is_some() {
                found(out, (2, 0), "structure destination (/SD, §12.3.2.3)");
            }
            // The GoToDp (go to document part) action is new in PDF 2.0 (§12.6.4.5).
            if action.get_name(&Name::from("S")).map(Name::as_bytes) == Some(b"GoToDp") {
                found(out, (2, 0), "GoToDp action (§12.6.4.5)");
            }
        }
        // Structure attributes (§14.8.5) carried in a StructElem's /A: the table-header /Scope key
        // is PDF 1.5 (Table 349); the PrintField attribute owner is PDF 1.7 (§14.8.5.6).
        if ty.as_bytes() == b"StructElem"
            && let Some(a) = d.get(&Name::from("A"))
        {
            for attr in attr_dicts_of(a) {
                if attr.get(&Name::from("Scope")).is_some() {
                    found(out, (1, 5), "table-header /Scope attribute (Table 349)");
                }
                if attr.get_name(&Name::from("O")).map(Name::as_bytes) == Some(b"PrintField") {
                    found(out, (1, 7), "PrintField attribute owner (§14.8.5.6)");
                }
            }
        }
    }

    // Embedded OpenType font program (FontFile3 /Subtype /OpenType) — PDF 1.6 (§9.9).
    if d.get_name(&Name::from("Subtype")).map(Name::as_bytes) == Some(b"OpenType") {
        found(out, (1, 6), "embedded OpenType font program (§9.9)");
    }

    // Catalog-level markers.
    if d.get(&Name::from("OCProperties")).is_some() {
        found(out, (1, 5), "optional content (/OCProperties, §8.11)");
    }
    if d.get(&Name::from("Collection")).is_some() {
        found(out, (1, 7), "collection dictionary (§12.3.5)");
    }
    if d.get(&Name::from("DPartRoot")).is_some() {
        found(out, (2, 0), "document parts (/DPartRoot, §14.12)");
    }
    if d.get(&Name::from("Namespaces")).is_some() {
        found(out, (2, 0), "structure namespaces (/Namespaces, §14.7.4)");
    }
    // Developer extensions (§7.12): the /Extensions dictionary itself is PDF 1.7 (Table 48);
    // the array-of-dictionaries form and the URL/ExtensionRevision keys are PDF 2.0; and each
    // declaration's /BaseVersion is a floor on the header (§7.12.4: BaseVersion ≤ header).
    if let Some(Object::Dictionary(extensions)) = d.get(&Name::from("Extensions")) {
        found(out, (1, 7), "developer extensions dictionary (§7.12)");
        let check_entry = |entry: &Dictionary, out: &mut Vec<VersionRequirement>| {
            if entry.get(&Name::from("URL")).is_some()
                || entry.get(&Name::from("ExtensionRevision")).is_some()
            {
                found(
                    out,
                    (2, 0),
                    "developer extension /URL or /ExtensionRevision (§7.12, Table 49)",
                );
            }
            if let Some(base) = entry
                .get_name(&Name::from("BaseVersion"))
                .and_then(|n| parse_version(n.as_bytes()))
            {
                found(out, base, "developer extension /BaseVersion (§7.12.4)");
            }
        };
        for (key, value) in extensions.iter() {
            if key.as_bytes() == b"Type" {
                continue;
            }
            match value {
                Object::Dictionary(entry) => check_entry(entry, out),
                Object::Array(entries) => {
                    found(
                        out,
                        (2, 0),
                        "array of developer extensions dictionaries (§7.12, Table 48)",
                    );
                    for entry in entries.iter().filter_map(Object::as_dict) {
                        check_entry(entry, out);
                    }
                }
                _ => {}
            }
        }
    }
    // An explicit catalog /Version override (Name like `/1.7`) is authoritative if higher.
    if let Some(ver) = d
        .get_name(&Name::from("Version"))
        .and_then(|n| parse_version(n.as_bytes()))
    {
        found(&mut *out, ver, "catalog /Version override (§7.7.2)");
    }
}

/// The attribute dictionaries in a structure element's `/A` value (§14.7.6) — a single dictionary
/// or an array of them (revision-number array entries are skipped).
fn attr_dicts_of(obj: &Object) -> Vec<&Dictionary> {
    match obj {
        Object::Dictionary(d) => vec![d],
        Object::Array(a) => a.iter().filter_map(Object::as_dict).collect(),
        _ => Vec::new(),
    }
}

/// The names referenced by a `/Filter` value — either a single `Name` or an array of `Name`s.
fn names_of(obj: &Object) -> Vec<&[u8]> {
    match obj {
        Object::Name(n) => vec![n.as_bytes()],
        Object::Array(a) => a
            .iter()
            .filter_map(Object::as_name)
            .map(Name::as_bytes)
            .collect(),
        _ => Vec::new(),
    }
}

/// Parse a `major.minor` version token (e.g. `b"1.7"`).
fn parse_version(bytes: &[u8]) -> Option<(u8, u8)> {
    let s = std::str::from_utf8(bytes).ok()?;
    let (maj, min) = s.split_once('.')?;
    Some((maj.parse().ok()?, min.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdf_cos::{Array, Stream};

    fn id() -> ObjectId {
        ObjectId::new(1, 0)
    }

    fn with_filter(name: &str) -> Vec<(ObjectId, Object)> {
        let mut d = Dictionary::new();
        d.insert(Name::from("Filter"), Object::Name(Name::from(name)));
        vec![(id(), Object::Stream(Stream::new(d, &b"x"[..])))]
    }

    #[test]
    fn plain_objects_floor_at_1_4() {
        let mut d = Dictionary::new();
        d.insert(Name::from("Type"), Object::Name(Name::from("Catalog")));
        assert_eq!(min_version(&[(id(), Object::Dictionary(d))]), (1, 4));
    }

    #[test]
    fn jpx_filter_requires_1_5() {
        assert_eq!(min_version(&with_filter("JPXDecode")), (1, 5));
    }

    #[test]
    fn jbig2_and_flate_stay_1_4() {
        assert_eq!(min_version(&with_filter("JBIG2Decode")), (1, 4));
        assert_eq!(min_version(&with_filter("FlateDecode")), (1, 4));
    }

    #[test]
    fn filter_array_is_scanned() {
        let mut d = Dictionary::new();
        let arr = Array::from(vec![
            Object::Name(Name::from("ASCII85Decode")),
            Object::Name(Name::from("JPXDecode")),
        ]);
        d.insert(Name::from("Filter"), Object::Array(arr));
        assert_eq!(
            min_version(&[(id(), Object::Stream(Stream::new(d, &b"x"[..])))]),
            (1, 5)
        );
    }

    #[test]
    fn object_stream_requires_1_5() {
        let mut d = Dictionary::new();
        d.insert(Name::from("Type"), Object::Name(Name::from("ObjStm")));
        assert_eq!(
            min_version(&[(id(), Object::Stream(Stream::new(d, &b"x"[..])))]),
            (1, 5)
        );
    }

    #[test]
    fn opentype_font_requires_1_6() {
        let mut d = Dictionary::new();
        d.insert(Name::from("Subtype"), Object::Name(Name::from("OpenType")));
        assert_eq!(min_version(&[(id(), Object::Dictionary(d))]), (1, 6));
    }

    #[test]
    fn document_parts_require_2_0() {
        let mut d = Dictionary::new();
        d.insert(Name::from("DPartRoot"), Object::Boolean(true));
        assert_eq!(min_version(&[(id(), Object::Dictionary(d))]), (2, 0));
    }

    #[test]
    fn utf8_text_string_requires_2_0() {
        // A UTF-8 text string (EF BB BF BOM) nested in an Info-style dict raises the floor to 2.0.
        let mut d = Dictionary::new();
        let mut bytes = vec![0xEFu8, 0xBB, 0xBF];
        bytes.extend_from_slice("café".as_bytes());
        d.insert(
            Name::from("Title"),
            Object::String(pdf_cos::PdfString::from(bytes)),
        );
        assert_eq!(min_version(&[(id(), Object::Dictionary(d))]), (2, 0));
    }

    #[test]
    fn utf16_and_ascii_strings_stay_1_4() {
        // UTF-16BE (FE FF) and plain-ASCII strings are pre-2.0 and must not raise the floor.
        let mut d = Dictionary::new();
        d.insert(
            Name::from("Author"),
            Object::String(pdf_cos::PdfString::from(vec![0xFE, 0xFF, 0x00, 0x41])),
        );
        d.insert(
            Name::from("Title"),
            Object::String(pdf_cos::PdfString::from(b"Plain".to_vec())),
        );
        assert_eq!(min_version(&[(id(), Object::Dictionary(d))]), (1, 4));
    }

    #[test]
    fn page_tabs_requires_1_5() {
        let mut d = Dictionary::new();
        d.insert(Name::from("Type"), Object::Name(Name::from("Page")));
        d.insert(Name::from("Tabs"), Object::Name(Name::from("S")));
        assert_eq!(min_version(&[(id(), Object::Dictionary(d))]), (1, 5));
    }

    /// A StructElem whose `/A` is `attr` (a dict or array of dicts).
    fn struct_elem_with_attr(attr: Object) -> Vec<(ObjectId, Object)> {
        let mut d = Dictionary::new();
        d.insert(Name::from("Type"), Object::Name(Name::from("StructElem")));
        d.insert(Name::from("A"), attr);
        vec![(id(), Object::Dictionary(d))]
    }

    #[test]
    fn th_scope_attribute_requires_1_5() {
        let mut a = Dictionary::new();
        a.insert(Name::from("O"), Object::Name(Name::from("Table")));
        a.insert(Name::from("Scope"), Object::Name(Name::from("Column")));
        assert_eq!(
            min_version(&struct_elem_with_attr(Object::Dictionary(a))),
            (1, 5)
        );
    }

    #[test]
    fn printfield_attribute_owner_requires_1_7() {
        // In array-of-owners form, and mixed with a version-neutral List dict.
        let mut list = Dictionary::new();
        list.insert(Name::from("O"), Object::Name(Name::from("List")));
        list.insert(
            Name::from("ListNumbering"),
            Object::Name(Name::from("Decimal")),
        );
        let mut pf = Dictionary::new();
        pf.insert(Name::from("O"), Object::Name(Name::from("PrintField")));
        pf.insert(Name::from("Role"), Object::Name(Name::from("cb")));
        let arr = Array::from(vec![Object::Dictionary(list), Object::Dictionary(pf)]);
        assert_eq!(
            min_version(&struct_elem_with_attr(Object::Array(arr))),
            (1, 7)
        );
    }

    #[test]
    fn struct_elem_ref_requires_2_0() {
        let mut d = Dictionary::new();
        d.insert(Name::from("Type"), Object::Name(Name::from("StructElem")));
        d.insert(Name::from("Ref"), Object::Array(Array::from(Vec::new())));
        assert_eq!(min_version(&[(id(), Object::Dictionary(d))]), (2, 0));
    }

    #[test]
    fn goto_structure_destination_requires_2_0() {
        let mut action = Dictionary::new();
        action.insert(Name::from("S"), Object::Name(Name::from("GoTo")));
        action.insert(Name::from("SD"), Object::Array(Array::from(Vec::new())));
        let mut d = Dictionary::new();
        d.insert(Name::from("Type"), Object::Name(Name::from("Annot")));
        d.insert(Name::from("A"), Object::Dictionary(action));
        assert_eq!(min_version(&[(id(), Object::Dictionary(d))]), (2, 0));

        // A plain GoTo (explicit /D destination) stays 1.4.
        let mut action = Dictionary::new();
        action.insert(Name::from("S"), Object::Name(Name::from("GoTo")));
        action.insert(Name::from("D"), Object::Array(Array::from(Vec::new())));
        let mut d = Dictionary::new();
        d.insert(Name::from("Type"), Object::Name(Name::from("Annot")));
        d.insert(Name::from("A"), Object::Dictionary(action));
        assert_eq!(min_version(&[(id(), Object::Dictionary(d))]), (1, 4));
    }

    #[test]
    fn list_numbering_attribute_stays_1_4() {
        let mut a = Dictionary::new();
        a.insert(Name::from("O"), Object::Name(Name::from("List")));
        a.insert(
            Name::from("ListNumbering"),
            Object::Name(Name::from("Decimal")),
        );
        assert_eq!(
            min_version(&struct_elem_with_attr(Object::Dictionary(a))),
            (1, 4)
        );
    }

    #[test]
    fn developer_extensions_raise_the_floor() {
        // The /Extensions dictionary is 1.7 (Table 48)…
        let mut entry = Dictionary::new();
        entry.insert(Name::from("BaseVersion"), Object::Name(Name::from("1.7")));
        entry.insert(Name::from("ExtensionLevel"), Object::Integer(3));
        let mut extensions = Dictionary::new();
        extensions.insert(Name::from("ADBE"), Object::Dictionary(entry.clone()));
        let mut catalog = Dictionary::new();
        catalog.insert(Name::from("Extensions"), Object::Dictionary(extensions));
        assert_eq!(min_version(&[(id(), Object::Dictionary(catalog))]), (1, 7));

        // …the URL key and the array form are 2.0 (Table 48/49)…
        let mut with_url = entry.clone();
        with_url.insert(
            Name::from("URL"),
            Object::String(pdf_cos::PdfString::from(b"https://x".to_vec())),
        );
        let mut extensions = Dictionary::new();
        extensions.insert(Name::from("ADBE"), Object::Dictionary(with_url));
        let mut catalog = Dictionary::new();
        catalog.insert(Name::from("Extensions"), Object::Dictionary(extensions));
        assert_eq!(min_version(&[(id(), Object::Dictionary(catalog))]), (2, 0));

        // …and a 2.0 /BaseVersion floors the header at 2.0 (§7.12.4), with the violation named.
        let mut base20 = Dictionary::new();
        base20.insert(Name::from("BaseVersion"), Object::Name(Name::from("2.0")));
        base20.insert(Name::from("ExtensionLevel"), Object::Integer(1));
        let mut extensions = Dictionary::new();
        extensions.insert(Name::from("ISO_"), Object::Dictionary(base20));
        let mut catalog = Dictionary::new();
        catalog.insert(Name::from("Extensions"), Object::Dictionary(extensions));
        let objs = vec![(id(), Object::Dictionary(catalog))];
        assert_eq!(min_version(&objs), (2, 0));
        let v = version_violation(&objs, (1, 7)).expect("BaseVersion above target");
        assert!(v.construct.contains("BaseVersion"), "got {}", v.construct);
    }

    #[test]
    fn catalog_version_override_is_honoured() {
        let mut d = Dictionary::new();
        d.insert(Name::from("Version"), Object::Name(Name::from("1.7")));
        assert_eq!(min_version(&[(id(), Object::Dictionary(d))]), (1, 7));
    }

    #[test]
    fn highest_construct_wins() {
        let mut objs = with_filter("JPXDecode"); // 1.5
        let mut d = Dictionary::new();
        d.insert(Name::from("Namespaces"), Object::Boolean(true)); // 2.0
        objs.push((ObjectId::new(2, 0), Object::Dictionary(d)));
        assert_eq!(min_version(&objs), (2, 0));
    }

    #[test]
    fn violation_names_the_culprit_construct() {
        // An object stream (1.5) inside a 1.4 target: the violation names §7.5.7.
        let mut d = Dictionary::new();
        d.insert(Name::from("Type"), Object::Name(Name::from("ObjStm")));
        let objs = vec![(id(), Object::Stream(Stream::new(d, &b"x"[..])))];
        let v = version_violation(&objs, (1, 4)).expect("must exceed 1.4");
        assert_eq!(v.version, (1, 5));
        assert!(v.construct.contains("object stream"));
        // The same set fits a 1.5 (or higher) target.
        assert_eq!(version_violation(&objs, (1, 5)), None);
        assert_eq!(version_violation(&objs, (2, 0)), None);
    }

    #[test]
    fn violation_reports_the_highest_offender() {
        // 1.5 (JPX) + 2.0 (namespaces) against a 1.4 target: the 2.0 construct is reported,
        // because fixing lesser offenders alone would not make the target reachable.
        let mut objs = with_filter("JPXDecode");
        let mut d = Dictionary::new();
        d.insert(Name::from("Namespaces"), Object::Boolean(true));
        objs.push((ObjectId::new(2, 0), Object::Dictionary(d)));
        let v = version_violation(&objs, (1, 4)).expect("must exceed 1.4");
        assert_eq!(v.version, (2, 0));
        assert!(v.construct.contains("Namespaces") || v.construct.contains("namespace"));
        // Against a 1.7 target only the 2.0 construct offends.
        let v = version_violation(&objs, (1, 7)).expect("must exceed 1.7");
        assert_eq!(v.version, (2, 0));
    }

    #[test]
    fn empty_set_never_violates_the_floor() {
        assert_eq!(version_violation(&[], (1, 4)), None);
        assert_eq!(min_version(&[]), (1, 4));
    }
}
