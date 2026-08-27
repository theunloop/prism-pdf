use super::*;

/// A Standard-14 font dictionary (§9.6.2.1).
pub(super) fn font_dict(font: StdFont) -> Dictionary {
    let mut dict = Dictionary::new();
    dict.insert(Name::from("Type"), Object::Name(Name::from("Font")));
    dict.insert(Name::from("Subtype"), Object::Name(Name::from("Type1")));
    dict.insert(
        Name::from("BaseFont"),
        Object::Name(Name::from(font.base_name())),
    );
    if font.uses_win_ansi() {
        dict.insert(
            Name::from("Encoding"),
            Object::Name(Name::from("WinAnsiEncoding")),
        );
    }
    dict
}

/// Emit an image XObject and its mask sub-images (§8.9.5), returning the main image's id. A soft mask
/// (`smask`) and a stencil mask (`mask`) are emitted as their own image objects first and referenced
/// by `/SMask` / `/Mask`, so an image may carry alpha (§11.6.5.2) or a stencil (§8.9.6.3).
pub(super) fn emit_image(
    image: &ImageXObject,
    alloc: &mut dyn FnMut() -> ObjectId,
    objects: &mut Vec<(ObjectId, Object)>,
) -> ObjectId {
    let smask_id = image.smask.as_ref().map(|m| emit_image(m, alloc, objects));
    let mask_id = image.mask.as_ref().map(|m| emit_image(m, alloc, objects));
    let id = alloc();
    objects.push((id, Object::Stream(image_stream(image, smask_id, mask_id))));
    id
}

/// Emit an embedded file (§7.11.4) and its file specification (§7.11.3) for `attachment`, returning
/// the filespec object's id. The filespec carries `/AFRelationship` (§14.13) so it can be referenced
/// from a catalog- or page-level `/AF` array.
pub(super) fn emit_filespec(
    attachment: &Attachment,
    alloc: &mut dyn FnMut() -> ObjectId,
    objects: &mut Vec<(ObjectId, Object)>,
) -> ObjectId {
    let ef_id = alloc();
    let mut params = Dictionary::new();
    params.insert(
        Name::from("Size"),
        Object::Integer(attachment.data.len() as i64),
    );
    if let Some(date) = &attachment.mod_date {
        params.insert(
            Name::from("ModDate"),
            Object::String(PdfString::from(date.as_bytes().to_vec())),
        );
    }
    let mut ef_dict = Dictionary::new();
    ef_dict.insert(Name::from("Type"), Object::Name(Name::from("EmbeddedFile")));
    ef_dict.insert(
        Name::from("Subtype"),
        Object::Name(Name::from(attachment.mime.as_str())),
    );
    ef_dict.insert(Name::from("Params"), Object::Dictionary(params));
    objects.push((
        ef_id,
        Object::Stream(Stream::new(ef_dict, attachment.data.clone())),
    ));

    let fs_id = alloc();
    let mut ef = Dictionary::new();
    ef.insert(Name::from("F"), Object::Reference(ef_id));
    ef.insert(Name::from("UF"), Object::Reference(ef_id));
    let mut fs = Dictionary::new();
    fs.insert(Name::from("Type"), Object::Name(Name::from("Filespec")));
    fs.insert(
        Name::from("F"),
        Object::String(PdfString::from(text_string(&attachment.name))),
    );
    fs.insert(
        Name::from("UF"),
        Object::String(PdfString::from(text_string(&attachment.name))),
    );
    fs.insert(Name::from("EF"), Object::Dictionary(ef));
    fs.insert(
        Name::from("AFRelationship"),
        Object::Name(Name::from(attachment.relationship.as_str())),
    );
    if let Some(desc) = &attachment.description {
        fs.insert(
            Name::from("Desc"),
            Object::String(PdfString::from(text_string(desc))),
        );
    }
    objects.push((fs_id, Object::Dictionary(fs)));
    fs_id
}

/// Build one developer extensions dictionary (§7.12, Table 49). `typed` adds the
/// `/Type /DeveloperExtensions` marker (used in the PDF 2.0 array form, per the spec's examples).
pub(super) fn developer_extension_dict(
    ext: &crate::DeveloperExtension,
    typed: bool,
    utf8: bool,
) -> Dictionary {
    let mut d = Dictionary::new();
    if typed {
        d.insert(
            Name::from("Type"),
            Object::Name(Name::from("DeveloperExtensions")),
        );
    }
    d.insert(
        Name::from("BaseVersion"),
        Object::Name(Name::from(
            format!("{}.{}", ext.base_version.0, ext.base_version.1).as_str(),
        )),
    );
    d.insert(
        Name::from("ExtensionLevel"),
        Object::Integer(ext.extension_level),
    );
    if let Some(url) = &ext.url {
        // A URL string (§7.9.5), plain bytes — not a text string.
        d.insert(
            Name::from("URL"),
            Object::String(PdfString::from(url.as_bytes().to_vec())),
        );
    }
    if let Some(revision) = &ext.revision {
        d.insert(
            Name::from("ExtensionRevision"),
            Object::String(PdfString::from(text_string_maybe_utf8(revision, utf8))),
        );
    }
    d
}

/// Emit one OutputIntent (§14.11.5) — the ICC profile stream plus the `/GTS_PDFA1` intent
/// dictionary referencing it — returning the intent's object id. Shared by the catalog-level
/// (PDF/A, M7) and page-level (PDF 2.0) placements.
pub(super) fn emit_output_intent(
    oi: &OutputIntentSpec,
    alloc: &mut dyn FnMut() -> ObjectId,
    objects: &mut Vec<(ObjectId, Object)>,
) -> ObjectId {
    let profile_id = alloc();
    let mut pdict = Dictionary::new();
    pdict.insert(Name::from("N"), Object::Integer(i64::from(oi.n)));
    objects.push((
        profile_id,
        Object::Stream(Stream::new(pdict, oi.icc.clone())),
    ));
    let id = alloc();
    let mut d = Dictionary::new();
    d.insert(Name::from("Type"), Object::Name(Name::from("OutputIntent")));
    d.insert(Name::from("S"), Object::Name(Name::from("GTS_PDFA1")));
    d.insert(
        Name::from("OutputConditionIdentifier"),
        Object::String(PdfString::from(oi.identifier.as_bytes().to_vec())),
    );
    d.insert(
        Name::from("Info"),
        Object::String(PdfString::from(oi.identifier.as_bytes().to_vec())),
    );
    d.insert(
        Name::from("DestOutputProfile"),
        Object::Reference(profile_id),
    );
    objects.push((id, Object::Dictionary(d)));
    id
}

/// Emit a filespec per file in `files`, record each in the `/EmbeddedFiles` name-tree `sink`
/// (`(name, id)`), and return an `/AF` array (§14.13) of references to them — or `None` if `files`
/// is empty. Shared by the page/annotation/XObject/struct-element `/AF` placements (§14.13.4–.9).
pub(super) fn emit_af_array(
    files: &[Attachment],
    alloc: &mut dyn FnMut() -> ObjectId,
    objects: &mut Vec<(ObjectId, Object)>,
    sink: &mut Vec<(String, ObjectId)>,
) -> Option<Object> {
    if files.is_empty() {
        return None;
    }
    let refs: Vec<Object> = files
        .iter()
        .map(|file| {
            let fs_id = emit_filespec(file, alloc, objects);
            sink.push((file.name.clone(), fs_id));
            Object::Reference(fs_id)
        })
        .collect();
    Some(Object::Array(Array::from(refs)))
}

/// Emit a colour-space object into `objects`, returning its id — dispatching on [`ColorSpaceKind`].
pub(super) fn emit_color_space(
    kind: &ColorSpaceKind,
    alloc: &mut dyn FnMut() -> ObjectId,
    objects: &mut Vec<(ObjectId, Object)>,
) -> ObjectId {
    match kind {
        ColorSpaceKind::Separation(sep) => emit_separation(sep, alloc, objects),
        ColorSpaceKind::Icc { icc, n } => {
            // [/ICCBased <profile stream>] — the stream's /N is the component count (§8.6.5.5).
            let mut pdict = Dictionary::new();
            pdict.insert(Name::from("N"), Object::Integer(i64::from(*n)));
            pdict.insert(
                Name::from("Filter"),
                Object::Name(Name::from("FlateDecode")),
            );
            let profile_id = alloc();
            objects.push((
                profile_id,
                Object::Stream(Stream::new(pdict, flate_encode(icc))),
            ));
            let cs = Array::from(vec![
                Object::Name(Name::from("ICCBased")),
                Object::Reference(profile_id),
            ]);
            let cs_id = alloc();
            objects.push((cs_id, Object::Array(cs)));
            cs_id
        }
        ColorSpaceKind::Indexed { base, palette } => {
            // [/Indexed base hival lookup] — hival is the highest index, lookup the palette bytes.
            let components = match base {
                ImageColorSpace::Gray => 1,
                ImageColorSpace::Rgb => 3,
                ImageColorSpace::Cmyk => 4,
            };
            let hival = (palette.len() / components).saturating_sub(1) as i64;
            let cs = Array::from(vec![
                Object::Name(Name::from("Indexed")),
                Object::Name(Name::from(base.name())),
                Object::Integer(hival),
                Object::String(PdfString::from(palette.clone())),
            ]);
            let cs_id = alloc();
            objects.push((cs_id, Object::Array(cs)));
            cs_id
        }
        ColorSpaceKind::Lab { white_point, range } => {
            // [/Lab << /WhitePoint [Xw Yw Zw] /Range [amin amax bmin bmax] >>]
            let reals = |xs: &[f64]| {
                Object::Array(Array::from(
                    xs.iter().map(|&v| Object::Real(v)).collect::<Vec<_>>(),
                ))
            };
            let mut params = Dictionary::new();
            params.insert(Name::from("WhitePoint"), reals(white_point));
            params.insert(Name::from("Range"), reals(range));
            let cs = Array::from(vec![
                Object::Name(Name::from("Lab")),
                Object::Dictionary(params),
            ]);
            let cs_id = alloc();
            objects.push((cs_id, Object::Array(cs)));
            cs_id
        }
    }
}

/// Emit a Separation colour-space array (§8.6.6) plus its linear tint-transform function (§7.10,
/// type 2), returning the colour-space object's id. The transform maps tint `t ∈ [0, 1]` from the
/// alternate's white (`t = 0`) to `sep.full` (`t = 1`).
pub(super) fn emit_separation(
    sep: &SeparationSpec,
    alloc: &mut dyn FnMut() -> ObjectId,
    objects: &mut Vec<(ObjectId, Object)>,
) -> ObjectId {
    let white: &[f64] = match sep.alternate {
        ImageColorSpace::Gray => &[1.0],
        ImageColorSpace::Rgb => &[1.0, 1.0, 1.0],
        ImageColorSpace::Cmyk => &[0.0, 0.0, 0.0, 0.0],
    };
    let reals = |xs: &[f64]| {
        Object::Array(Array::from(
            xs.iter().map(|&v| Object::Real(v)).collect::<Vec<_>>(),
        ))
    };

    // Tint transform: type-2 (exponential) with N = 1 ⇒ linear interpolation white → full.
    let mut func = Dictionary::new();
    func.insert(Name::from("FunctionType"), Object::Integer(2));
    func.insert(Name::from("Domain"), reals(&[0.0, 1.0]));
    func.insert(Name::from("C0"), reals(white));
    func.insert(Name::from("C1"), reals(&sep.full));
    func.insert(Name::from("N"), Object::Real(1.0));
    let fn_id = alloc();
    objects.push((fn_id, Object::Dictionary(func)));

    // [/Separation /Colorant <alternate> <tintFn>]
    let cs = Array::from(vec![
        Object::Name(Name::from("Separation")),
        Object::Name(Name::from(sep.colorant.as_str())),
        Object::Name(Name::from(sep.alternate.name())),
        Object::Reference(fn_id),
    ]);
    let cs_id = alloc();
    objects.push((cs_id, Object::Array(cs)));
    cs_id
}

/// An image XObject stream (§8.9.5): geometry, colour space and (optional) filter, plus the bytes;
/// with `/ImageMask`, `/SMask` and `/Mask` wired when applicable.
pub(super) fn image_stream(
    image: &ImageXObject,
    smask_id: Option<ObjectId>,
    mask_id: Option<ObjectId>,
) -> Stream {
    let mut dict = Dictionary::new();
    dict.insert(Name::from("Type"), Object::Name(Name::from("XObject")));
    dict.insert(Name::from("Subtype"), Object::Name(Name::from("Image")));
    dict.insert(Name::from("Width"), Object::Integer(i64::from(image.width)));
    dict.insert(
        Name::from("Height"),
        Object::Integer(i64::from(image.height)),
    );
    if image.image_mask {
        // A stencil mask (§8.9.6.2): 1-bit, no colour space; BitsPerComponent must be 1 (PDF/A
        // §6.2.8 t5).
        dict.insert(Name::from("ImageMask"), Object::Boolean(true));
        dict.insert(Name::from("BitsPerComponent"), Object::Integer(1));
    } else {
        dict.insert(
            Name::from("BitsPerComponent"),
            Object::Integer(i64::from(image.bits_per_component)),
        );
        dict.insert(
            Name::from("ColorSpace"),
            Object::Name(Name::from(image.color_space.name())),
        );
    }
    if let Some(filter) = image.filter {
        dict.insert(
            Name::from("Filter"),
            Object::Name(Name::from(filter.name())),
        );
    }
    if let Some(id) = smask_id {
        dict.insert(Name::from("SMask"), Object::Reference(id));
    }
    if let Some(id) = mask_id {
        dict.insert(Name::from("Mask"), Object::Reference(id));
    }
    Stream::new(dict, image.data.clone())
}

/// The `FontFile2` stream (§9.9): the whole sfnt program, FlateDecode-compressed, with `/Length1`
/// the uncompressed length.
pub(super) fn fontfile2_stream(program: &[u8]) -> Stream {
    let mut dict = Dictionary::new();
    dict.insert(Name::from("Length1"), Object::Integer(program.len() as i64));
    dict.insert(
        Name::from("Filter"),
        Object::Name(Name::from("FlateDecode")),
    );
    Stream::new(dict, flate_encode(program))
}

/// The `/FontDescriptor` (§9.8.1) for an embedded composite font.
pub(super) fn font_descriptor_dict(font: &CidFont, fontfile: ObjectId) -> Dictionary {
    let mut dict = Dictionary::new();
    dict.insert(
        Name::from("Type"),
        Object::Name(Name::from("FontDescriptor")),
    );
    dict.insert(
        Name::from("FontName"),
        Object::Name(Name::from(font.postscript_name.as_str())),
    );
    dict.insert(Name::from("Flags"), Object::Integer(i64::from(font.flags)));
    dict.insert(
        Name::from("FontBBox"),
        Object::Array(Array::from(
            font.bbox
                .iter()
                .map(|&v| Object::Integer(i64::from(v)))
                .collect::<Vec<_>>(),
        )),
    );
    dict.insert(
        Name::from("ItalicAngle"),
        Object::Real((font.italic_angle * 100.0).round() / 100.0),
    );
    dict.insert(
        Name::from("Ascent"),
        Object::Integer(i64::from(font.ascent)),
    );
    dict.insert(
        Name::from("Descent"),
        Object::Integer(i64::from(font.descent)),
    );
    dict.insert(
        Name::from("CapHeight"),
        Object::Integer(i64::from(font.cap_height)),
    );
    dict.insert(Name::from("StemV"), Object::Integer(80)); // a reasonable default; unused for rendering
    dict.insert(Name::from("FontFile2"), Object::Reference(fontfile));
    dict
}

/// The CIDFontType2 descendant font (§9.7.4): per-glyph widths in `/W` and a `/CIDToGIDMap` that is
/// either the `Identity` name (CID == glyph ID) or a reference to a remap stream (subsetted font).
pub(super) fn cid_font_dict(
    font: &CidFont,
    descriptor: ObjectId,
    cid_to_gid: Option<ObjectId>,
) -> Dictionary {
    let mut system_info = Dictionary::new();
    system_info.insert(
        Name::from("Registry"),
        Object::String(PdfString::from(b"Adobe".to_vec())),
    );
    system_info.insert(
        Name::from("Ordering"),
        Object::String(PdfString::from(b"Identity".to_vec())),
    );
    system_info.insert(Name::from("Supplement"), Object::Integer(0));

    // /W as `cid [w]` entries (§9.7.4.3).
    let mut w = Vec::with_capacity(font.widths.len() * 2);
    for &(gid, advance) in &font.widths {
        w.push(Object::Integer(i64::from(gid)));
        w.push(Object::Array(Array::from(vec![Object::Integer(
            i64::from(advance),
        )])));
    }

    let mut dict = Dictionary::new();
    dict.insert(Name::from("Type"), Object::Name(Name::from("Font")));
    dict.insert(
        Name::from("Subtype"),
        Object::Name(Name::from("CIDFontType2")),
    );
    dict.insert(
        Name::from("BaseFont"),
        Object::Name(Name::from(font.postscript_name.as_str())),
    );
    dict.insert(Name::from("CIDSystemInfo"), Object::Dictionary(system_info));
    dict.insert(Name::from("FontDescriptor"), Object::Reference(descriptor));
    dict.insert(
        Name::from("CIDToGIDMap"),
        match cid_to_gid {
            Some(id) => Object::Reference(id),
            None => Object::Name(Name::from("Identity")),
        },
    );
    dict.insert(
        Name::from("DW"),
        Object::Integer(i64::from(font.default_width)),
    );
    dict.insert(Name::from("W"), Object::Array(Array::from(w)));
    dict
}

/// The Type0 (composite) font (§9.7.3) wrapping `descendant`, with `Identity-H` and `/ToUnicode`.
pub(super) fn type0_dict(font: &CidFont, descendant: ObjectId, to_unicode: ObjectId) -> Dictionary {
    let mut dict = Dictionary::new();
    dict.insert(Name::from("Type"), Object::Name(Name::from("Font")));
    dict.insert(Name::from("Subtype"), Object::Name(Name::from("Type0")));
    dict.insert(
        Name::from("BaseFont"),
        Object::Name(Name::from(font.postscript_name.as_str())),
    );
    dict.insert(
        Name::from("Encoding"),
        Object::Name(Name::from("Identity-H")),
    );
    dict.insert(
        Name::from("DescendantFonts"),
        Object::Array(Array::from(vec![Object::Reference(descendant)])),
    );
    dict.insert(Name::from("ToUnicode"), Object::Reference(to_unicode));
    dict
}

/// A `/ToUnicode` CMap stream (§9.10.3) mapping each 2-byte glyph code to its Unicode value, so the
/// authored text still extracts. FlateDecode-compressed.
pub(super) fn tounicode_stream(entries: &[(u16, char)]) -> Stream {
    let mut s = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
         /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
         /CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n\
         1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );
    for chunk in entries.chunks(100) {
        s.push_str(&format!("{} beginbfchar\n", chunk.len()));
        for &(gid, ch) in chunk {
            let mut units = [0u16; 2];
            let hex: String = ch
                .encode_utf16(&mut units)
                .iter()
                .map(|u| format!("{u:04X}"))
                .collect();
            s.push_str(&format!("<{gid:04X}> <{hex}>\n"));
        }
        s.push_str("endbfchar\n");
    }
    s.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");

    let mut dict = Dictionary::new();
    dict.insert(
        Name::from("Filter"),
        Object::Name(Name::from("FlateDecode")),
    );
    Stream::new(dict, flate_encode(s.as_bytes()))
}
