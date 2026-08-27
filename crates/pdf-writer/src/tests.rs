use super::*;
use pdf_cos::{Dictionary, Name};

#[test]
fn bytes_needed_covers_the_range() {
    assert_eq!(bytes_needed(0), 1);
    assert_eq!(bytes_needed(255), 1);
    assert_eq!(bytes_needed(256), 2);
    assert_eq!(bytes_needed(0xFFFF), 2);
    assert_eq!(bytes_needed(0x01_0000), 3);
}

#[test]
fn writes_a_cross_reference_stream() {
    let mut catalog = Dictionary::new();
    catalog.insert(Name::from("Type"), Object::Name(Name::from("Catalog")));
    let objects = vec![(ObjectId::new(1, 0), Object::Dictionary(catalog))];
    let bytes = write_document_xref_stream(&objects, ObjectId::new(1, 0), None, (1, 5), None);

    let text = String::from_utf8_lossy(&bytes);
    assert!(text.starts_with("%PDF-1.5\n"));
    // The cross-reference is an object 2 stream of /Type /XRef, not a classic table.
    assert!(!text.contains("trailer"));
    assert!(text.contains("2 0 obj\n"));
    assert!(text.contains("/Type /XRef"));
    assert!(text.contains("/W [1 "));
    assert!(text.contains("/Root 1 0 R"));
    assert!(text.contains("/Size 3"));
    // startxref points at the xref stream object (object 2).
    let at = text.rfind("startxref\n").unwrap() + "startxref\n".len();
    let offset: usize = text[at..].lines().next().unwrap().parse().unwrap();
    assert_eq!(&bytes[offset..offset + 7], b"2 0 obj");
    assert!(text.trim_end().ends_with("%%EOF"));
}

#[test]
fn writes_a_well_formed_file() {
    let mut catalog = Dictionary::new();
    catalog.insert(Name::from("Type"), Object::Name(Name::from("Catalog")));
    let objects = vec![(ObjectId::new(1, 0), Object::Dictionary(catalog))];
    let bytes = write_document(&objects, ObjectId::new(1, 0), None, (1, 7), None);

    let text = String::from_utf8_lossy(&bytes);
    assert!(text.starts_with("%PDF-1.7\n"));
    assert!(text.contains("1 0 obj\n"));
    assert!(text.contains("xref\n0 2\n"));
    assert!(text.contains("/Root 1 0 R"));
    assert!(text.contains("/Size 2"));
    assert!(text.trim_end().ends_with("%%EOF"));
    // The startxref offset points at the `xref` keyword.
    let at = text.rfind("startxref\n").unwrap() + "startxref\n".len();
    let offset: usize = text[at..].lines().next().unwrap().parse().unwrap();
    assert_eq!(&bytes[offset..offset + 4], b"xref");
}

#[test]
fn xref_entries_are_twenty_bytes_and_gaps_are_free() {
    // Objects 1 and 3 present, 2 missing → object 2's row must be a free entry.
    let objects = vec![
        (ObjectId::new(1, 0), Object::Integer(1)),
        (ObjectId::new(3, 0), Object::Integer(3)),
    ];
    let bytes = write_document(&objects, ObjectId::new(1, 0), None, (1, 7), None);

    // Search the raw bytes (the binary marker makes lossy-string indices unreliable).
    let header = b"xref\n0 4\n";
    let pos = bytes
        .windows(header.len())
        .position(|w| w == header)
        .unwrap();
    let table_start = pos + header.len();
    let table = &bytes[table_start..table_start + 4 * 20];
    for row in 0..4 {
        // Every entry line is exactly 20 bytes ending in " \n".
        assert_eq!(&table[row * 20 + 18..row * 20 + 20], b" \n");
    }
    // Row 2 (object 2) is free; rows 1 and 3 are in-use. The type byte is at offset 17.
    assert_eq!(&table[20 + 17..20 + 18], b"n"); // object 1
    assert_eq!(&table[2 * 20 + 17..2 * 20 + 18], b"f"); // object 2 (gap)
    assert_eq!(&table[3 * 20 + 17..3 * 20 + 18], b"n"); // object 3
}

#[test]
fn id_is_written_as_two_equal_hex_strings() {
    let objects = vec![(ObjectId::new(1, 0), Object::Integer(1))];
    let id = [0xDE, 0xAD, 0xBE, 0xEF];
    let bytes = write_document(&objects, ObjectId::new(1, 0), None, (1, 7), Some(&id));
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("/ID [<deadbeef> <deadbeef>]"), "{text}");
    // No /Encrypt for the unencrypted /ID variant.
    assert!(!text.contains("/Encrypt"));
    assert!(text.contains("/Root 1 0 R"));
}

/// §7.5.5 (ISO 32000-2, Table 15) makes the trailer `/ID` **required** in PDF 2.0, so a 2.0
/// header with no caller-supplied identifier must still come out with one.
#[test]
fn pdf_2_0_trailer_always_carries_an_id() {
    let objects = vec![(ObjectId::new(1, 0), Object::Integer(1))];
    let bytes = write_document(&objects, ObjectId::new(1, 0), None, (2, 0), None);
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.starts_with("%PDF-2.0"));
    // Two 16-byte identifiers, hex-encoded: 32 hex digits each.
    let id = text
        .split("/ID [<")
        .nth(1)
        .unwrap_or_else(|| panic!("no /ID in a 2.0 trailer: {text}"));
    let (first, rest) = id.split_once("> <").unwrap();
    let (second, _) = rest.split_once(">]").unwrap();
    assert_eq!(first.len(), FILE_ID_LEN * 2, "{text}");
    assert_eq!(first, second, "a fresh revision has both elements equal");
    assert!(first.bytes().all(|b| b.is_ascii_hexdigit()), "{text}");
}

/// Before 2.0 `/ID` is only "strongly recommended", and adding one unasked would change every
/// existing file's bytes — so the pre-2.0 default stays as it was.
#[test]
fn pre_2_0_trailer_has_no_id_unless_asked() {
    let objects = vec![(ObjectId::new(1, 0), Object::Integer(1))];
    for version in [(1, 4), (1, 5), (1, 6), (1, 7)] {
        let bytes = write_document(&objects, ObjectId::new(1, 0), None, version, None);
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("/ID"), "{version:?}: {text}");
    }
}

/// The identifier is derived from content (§14.4), so the writer stays deterministic: equal
/// input → equal bytes, different input → different identifier.
#[test]
fn synthesized_id_is_deterministic_and_content_derived() {
    let write = |value: i64| {
        let objects = vec![(ObjectId::new(1, 0), Object::Integer(value))];
        write_document(&objects, ObjectId::new(1, 0), None, (2, 0), None)
    };
    assert_eq!(write(1), write(1), "same document → same bytes");
    assert_ne!(write(1), write(2), "different content → different /ID");
}

/// An xref stream's dictionary *is* the trailer dictionary (§7.5.8.2), so the 2.0 `/ID`
/// requirement applies there too — in both stream-xref writers.
#[test]
fn pdf_2_0_xref_stream_dictionaries_carry_an_id() {
    let objects = vec![(ObjectId::new(1, 0), Object::Integer(1))];
    for bytes in [
        write_document_xref_stream(&objects, ObjectId::new(1, 0), None, (2, 0), None),
        write_document_object_streams(&objects, ObjectId::new(1, 0), None, (2, 0), None),
    ] {
        // The dictionary is in the clear; only the entry table is Flate-compressed.
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("/Type /XRef"), "{text}");
        assert!(
            text.contains("/ID ["),
            "2.0 xref stream without /ID: {text}"
        );
    }
}

#[test]
fn info_reference_is_written_when_present() {
    let bytes = write_document(
        &[],
        ObjectId::new(1, 0),
        Some(ObjectId::new(9, 0)),
        (2, 0),
        None,
    );
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("/Info 9 0 R"));
    assert!(text.starts_with("%PDF-2.0"));
}

#[test]
fn incremental_appends_preserving_original() {
    // A base file ending in startxref/%%EOF, then an incremental change to object 4.
    let base = write_document(
        &[(ObjectId::new(1, 0), Object::Integer(1))],
        ObjectId::new(1, 0),
        None,
        (1, 7),
        None,
    );
    let updated = write_incremental(
        &base,
        &[(ObjectId::new(4, 0), Object::Integer(42))],
        ObjectId::new(1, 0),
        None,
        5,
    );

    // The original bytes are preserved verbatim as a prefix (append-only).
    assert!(updated.starts_with(&base));
    let tail = String::from_utf8_lossy(&updated[base.len()..]);
    assert!(tail.contains("4 0 obj\n42\nendobj"));
    assert!(tail.contains("xref\n4 1\n")); // one subsection starting at object 4
    assert!(tail.contains("/Prev ")); // chains back to the base section
    assert!(tail.contains("/Size 5"));
    assert!(tail.trim_end().ends_with("%%EOF"));
}

#[test]
fn incremental_carries_id_forward() {
    // PDF/A (ISO 19005, 6.1.3) requires /ID in every trailer; an incremental update must reuse
    // the original's rather than dropping it.
    let original = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n\
        trailer\n<< /Size 2 /Root 1 0 R /ID [<AABB><CCDD>] >>\nstartxref\n9\n%%EOF\n";
    let updated = write_incremental(
        original,
        &[(ObjectId::new(4, 0), Object::Integer(42))],
        ObjectId::new(1, 0),
        None,
        5,
    );
    let tail = String::from_utf8_lossy(&updated[original.len()..]);
    assert!(
        tail.contains("/ID [<AABB><CCDD>]"),
        "incremental trailer must carry /ID forward: {tail}"
    );
}

#[test]
fn find_trailer_id_picks_the_last_array() {
    assert_eq!(
        find_trailer_id(b"trailer << /ID [<AA><BB>] >>"),
        Some(&b"[<AA><BB>]"[..])
    );
    // With several /ID keys, the last one (the active trailer's) wins.
    assert_eq!(
        find_trailer_id(b"/ID [<00><00>] junk /ID [<11><22>]"),
        Some(&b"[<11><22>]"[..])
    );
    assert_eq!(find_trailer_id(b"no identifier here"), None);
}

/// A literal-string identifier (§7.3.4.2) is arbitrary binary, so it may well contain a `]`, a
/// balanced `(`/`)` pair or an escaped `\)`. Stopping at the first `]` would truncate the array
/// and corrupt every incremental update of such a file.
#[test]
fn find_trailer_id_handles_literal_string_identifiers() {
    assert_eq!(
        find_trailer_id(br"trailer << /ID [(a]b) (c]d)] >>"),
        Some(&br"[(a]b) (c]d)]"[..])
    );
    assert_eq!(
        find_trailer_id(br"trailer << /ID [(a\)b) (x)] >>"),
        Some(&br"[(a\)b) (x)]"[..])
    );
    assert_eq!(
        find_trailer_id(b"trailer << /ID [(a(nested)b) (x)] >>"),
        Some(&b"[(a(nested)b) (x)]"[..])
    );
    // An unterminated string is not a usable array.
    assert_eq!(find_trailer_id(b"trailer << /ID [(unclosed"), None);
}

/// A malformed file must not make the candidate walk quadratic (DESIGN.md §3.4): an unclosed
/// `/ID [` array is abandoned after [`MAX_FILE_ID_ARRAY`] bytes, however many candidates there
/// are, and an earlier well-formed array is still found.
#[test]
fn find_trailer_id_bounds_the_forward_scan() {
    let mut bytes = b"trailer << /ID [<AA><BB>] >>".to_vec();
    for _ in 0..500 {
        bytes.extend_from_slice(b" /ID [");
        bytes.resize(bytes.len() + MAX_FILE_ID_ARRAY, b' ');
    }
    assert_eq!(find_trailer_id(&bytes), Some(&b"[<AA><BB>]"[..]));
}

/// `/ID` is a prefix of other names — `/IDTree` (§14.7.4.5) is emitted by the tagged-PDF path —
/// and struct elements carry their own `/ID` *string*. Neither may shadow the trailer's array.
#[test]
fn find_trailer_id_ignores_longer_names_and_non_arrays() {
    assert_eq!(
        find_trailer_id(b"/ID [<AA><BB>] ... /StructTreeRoot << /IDTree 7 0 R >>"),
        Some(&b"[<AA><BB>]"[..])
    );
    assert_eq!(
        find_trailer_id(b"/ID [<AA><BB>] ... << /Type /StructElem /ID (node-1) >>"),
        Some(&b"[<AA><BB>]"[..])
    );
    assert_eq!(find_trailer_id(b"<< /IDTree 7 0 R >>"), None);
}

// --- §7.5.6: carrying /Encrypt into an incremental revision's trailer -------------------------

#[test]
fn trailer_encrypt_reference_accepts_only_a_real_reference() {
    // The value that follows the key, in the shapes a trailer really uses.
    assert_eq!(trailer_encrypt_reference(b" 9 0 R >>"), Some(&b"9 0 R"[..]));
    assert_eq!(
        trailer_encrypt_reference(b"\n  12   7   R\n"),
        Some(&b"12   7   R"[..])
    );

    // `/EncryptMetadata` is a real key in an encryption dictionary and shares the prefix. Its
    // value is a boolean, and the name does not end at the key — accepting it would emit a
    // `/Encrypt false` into the trailer and break the file we were trying to keep readable.
    assert_eq!(trailer_encrypt_reference(b"Metadata false"), None);

    // Values that are not `n g R`.
    assert_eq!(trailer_encrypt_reference(b" << /V 5 >>"), None); // direct dictionary
    assert_eq!(trailer_encrypt_reference(b" 9 0 X"), None); // not the R keyword
    assert_eq!(trailer_encrypt_reference(b" 9 R"), None); // generation missing
    assert_eq!(trailer_encrypt_reference(b" 9 0"), None); // truncated
    assert_eq!(trailer_encrypt_reference(b" 9"), None); // truncated harder
    assert_eq!(trailer_encrypt_reference(b" "), None); // whitespace only
    assert_eq!(trailer_encrypt_reference(b""), None); // nothing at all
}

#[test]
fn find_trailer_encrypt_takes_the_newest_revision() {
    // Two revisions: the newer trailer's /Encrypt wins (§7.5.6, newest trailer is authoritative).
    let file =
        b"%PDF-1.7\ntrailer\n<< /Encrypt 4 0 R >>\n%%EOF\ntrailer\n<< /Encrypt 9 0 R >>\n%%EOF\n";
    assert_eq!(find_trailer_encrypt(file), Some(&b"9 0 R"[..]));

    // A file with no /Encrypt at all, and one where every hit is really /EncryptMetadata.
    assert_eq!(find_trailer_encrypt(b"trailer\n<< /Root 1 0 R >>"), None);
    assert_eq!(
        find_trailer_encrypt(b"<< /EncryptMetadata false /EncryptMetadata true >>"),
        None
    );
}

#[test]
fn an_incremental_revision_carries_encrypt_forward() {
    // End to end through the writer: the added trailer keeps the previous trailer's entries
    // (§7.5.6). Dropping /Encrypt would leave a file that declares itself unencrypted while its
    // body objects are still ciphertext.
    let original = b"%PDF-1.7\n1 0 obj\n<< >>\nendobj\nxref\n0 1\n0000000000 65535 f \n\
trailer\n<< /Size 2 /Root 1 0 R /Encrypt 5 0 R /ID [<AA> <BB>] >>\nstartxref\n9\n%%EOF\n";
    let changed = vec![(ObjectId::new(1, 0), Object::Dictionary(Dictionary::new()))];
    let out = write_incremental(original, &changed, ObjectId::new(1, 0), None, 2);
    let tail = String::from_utf8_lossy(&out);
    let trailer = &tail[tail.rfind("trailer").expect("a trailer")..];
    assert!(trailer.contains("/Encrypt 5 0 R"), "got {trailer}");
    assert!(
        trailer.contains("/ID "),
        "the /ID carry still works: {trailer}"
    );

    // A caller that supplies its own /Encrypt in `trailer_extra` must not get a duplicate key.
    let out = write_incremental_signed_with_trailer(
        original,
        &[],
        (ObjectId::new(6, 0), b"<< /Type /Sig >>"),
        ObjectId::new(1, 0),
        None,
        7,
        " /Encrypt 5 0 R",
    );
    let tail = String::from_utf8_lossy(&out);
    let trailer = &tail[tail.rfind("trailer").expect("a trailer")..];
    assert_eq!(trailer.matches("/Encrypt").count(), 1, "got {trailer}");
}
