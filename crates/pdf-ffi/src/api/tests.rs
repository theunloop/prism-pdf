use super::*;

// Defined by `tests/c/compose_invoice.c`, which `build.rs` compiles and links only under the
// `c-acceptance` feature (CI runs `--all-features`; see that build script for why it is opt-in).
#[cfg(feature = "c-acceptance")]
unsafe extern "C" {
    fn prismpdf_c_invoice_acceptance(out_data: *mut *mut u8, out_len: *mut usize) -> i32;
}
use std::ffi::CStr;

use prismpdf::{AnnotationSpec, Builder, FormFieldSpec, LinkTarget};

/// A document exercising the collection surfaces: two annotations on page 0 (a described link
/// and a note), one checkbox form field, and two top-level bookmarks.
fn rich_pdf() -> Vec<u8> {
    let mut builder = Builder::new();
    builder.add_page(PageSpec::new(b"BT ET".to_vec()));
    builder.add_annotation(
        0,
        AnnotationSpec::Link {
            rect: [10.0, 20.0, 30.0, 40.0],
            target: LinkTarget::Uri("https://example.org/".to_string()),
            contents: Some("described link".to_string()),
        },
        Vec::new(),
    );
    builder.add_annotation(
        0,
        AnnotationSpec::Note {
            rect: [50.0, 60.0, 70.0, 80.0],
            contents: "a note body".to_string(),
        },
        Vec::new(),
    );
    builder.add_form_field(
        0,
        FormFieldSpec::Checkbox {
            rect: [1.0, 2.0, 3.0, 4.0],
            name: "agree".to_string(),
            checked: true,
            tooltip: None,
        },
        Vec::new(),
    );
    builder.outline("First", 0);
    builder.outline("Second", 0);
    builder.build()
}

/// Open `bytes` into a handle, or panic — test-only convenience.
fn open(bytes: &[u8]) -> *mut PrismPdfDocument {
    let mut doc: *mut PrismPdfDocument = std::ptr::null_mut();
    let status = unsafe { prismpdf_document_open(bytes.as_ptr(), bytes.len(), &mut doc) };
    assert_eq!(status, PrismPdfStatus::Ok);
    doc
}

/// Take an owned C string out of an out-param and free it.
fn take_string(ptr: *mut c_char) -> String {
    assert!(!ptr.is_null());
    let text = unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned();
    unsafe { prismpdf_string_free(ptr) };
    text
}

#[test]
fn annotations_expose_every_field() {
    let doc = open(&rich_pdf());
    let mut list: *mut PrismPdfAnnotationList = std::ptr::null_mut();
    assert_eq!(
        unsafe { prismpdf_page_annotations(doc, 0, &mut list) },
        PrismPdfStatus::Ok
    );

    let mut len = 0usize;
    assert_eq!(
        unsafe { prismpdf_annotation_list_len(list, &mut len) },
        PrismPdfStatus::Ok
    );
    // Two annotations plus the checkbox widget, which is also an /Annots entry.
    assert_eq!(len, 3);

    let mut item: *const PrismPdfAnnotation = std::ptr::null();
    assert_eq!(
        unsafe { prismpdf_annotation_list_get(list, 0, &mut item) },
        PrismPdfStatus::Ok
    );

    let mut text: *mut c_char = std::ptr::null_mut();
    assert_eq!(
        unsafe { prismpdf_annotation_subtype(item, &mut text) },
        PrismPdfStatus::Ok
    );
    assert_eq!(take_string(text), "Link");

    let mut rect = [0.0f64; 4];
    assert_eq!(
        unsafe { prismpdf_annotation_rect(item, rect.as_mut_ptr()) },
        PrismPdfStatus::Ok
    );
    assert_eq!(rect, [10.0, 20.0, 30.0, 40.0]);

    assert_eq!(
        unsafe { prismpdf_annotation_contents(item, &mut text) },
        PrismPdfStatus::Ok
    );
    assert_eq!(take_string(text), "described link");

    assert_eq!(
        unsafe { prismpdf_annotation_uri(item, &mut text) },
        PrismPdfStatus::Ok
    );
    assert_eq!(take_string(text), "https://example.org/");

    // A URI link has no in-document destination: the Option maps to NotFound, not an error.
    let mut page = 0usize;
    assert_eq!(
        unsafe { prismpdf_annotation_dest_page(item, &mut page) },
        PrismPdfStatus::NotFound
    );

    // Past the end is NotFound, and leaves the out-param null.
    assert_eq!(
        unsafe { prismpdf_annotation_list_get(list, 99, &mut item) },
        PrismPdfStatus::NotFound
    );
    assert!(item.is_null());

    unsafe { prismpdf_annotation_list_free(list) };
    unsafe { prismpdf_document_free(doc) };
}

#[test]
fn note_annotation_has_contents_but_no_uri() {
    let doc = open(&rich_pdf());
    let mut list: *mut PrismPdfAnnotationList = std::ptr::null_mut();
    unsafe { prismpdf_page_annotations(doc, 0, &mut list) };

    let mut item: *const PrismPdfAnnotation = std::ptr::null();
    unsafe { prismpdf_annotation_list_get(list, 1, &mut item) };

    let mut text: *mut c_char = std::ptr::null_mut();
    unsafe { prismpdf_annotation_subtype(item, &mut text) };
    assert_eq!(take_string(text), "Text");

    unsafe { prismpdf_annotation_contents(item, &mut text) };
    assert_eq!(take_string(text), "a note body");

    assert_eq!(
        unsafe { prismpdf_annotation_uri(item, &mut text) },
        PrismPdfStatus::NotFound
    );
    assert!(text.is_null());

    unsafe { prismpdf_annotation_list_free(list) };
    unsafe { prismpdf_document_free(doc) };
}

#[test]
fn annotations_on_a_page_without_any_yield_an_empty_list() {
    let doc = open(&sample_pdf());
    let mut list: *mut PrismPdfAnnotationList = std::ptr::null_mut();
    assert_eq!(
        unsafe { prismpdf_page_annotations(doc, 0, &mut list) },
        PrismPdfStatus::Ok
    );
    let mut len = 99usize;
    unsafe { prismpdf_annotation_list_len(list, &mut len) };
    assert_eq!(len, 0);
    unsafe { prismpdf_annotation_list_free(list) };
    unsafe { prismpdf_document_free(doc) };
}

#[test]
fn form_fields_round_trip_through_fill() {
    let doc = open(&rich_pdf());
    let mut list: *mut PrismPdfFormFieldList = std::ptr::null_mut();
    assert_eq!(
        unsafe { prismpdf_document_form_fields(doc, &mut list) },
        PrismPdfStatus::Ok
    );

    let mut len = 0usize;
    unsafe { prismpdf_form_field_list_len(list, &mut len) };
    assert_eq!(len, 1);

    let mut field: *const PrismPdfFormField = std::ptr::null();
    unsafe { prismpdf_form_field_list_get(list, 0, &mut field) };

    let mut text: *mut c_char = std::ptr::null_mut();
    unsafe { prismpdf_form_field_name(field, &mut text) };
    assert_eq!(take_string(text), "agree");
    unsafe { prismpdf_form_field_type(field, &mut text) };
    assert_eq!(take_string(text), "Btn");
    unsafe { prismpdf_form_field_value(field, &mut text) };
    assert_eq!(take_string(text), "On");

    unsafe { prismpdf_form_field_list_free(list) };

    // Flip the checkbox off through the C entry point and re-read it.
    let name = CString::new("agree").unwrap();
    let value = CString::new("Off").unwrap();
    let names = [name.as_ptr()];
    let values = [value.as_ptr()];
    let mut data: *mut u8 = std::ptr::null_mut();
    let mut out_len = 0usize;
    assert_eq!(
        unsafe {
            prismpdf_document_fill_form(
                doc,
                names.as_ptr(),
                values.as_ptr(),
                1,
                &mut data,
                &mut out_len,
            )
        },
        PrismPdfStatus::Ok
    );
    assert!(out_len > 0);

    let filled = unsafe { std::slice::from_raw_parts(data, out_len) }.to_vec();
    unsafe { prismpdf_bytes_free(data, out_len) };
    unsafe { prismpdf_document_free(doc) };

    let refilled = open(&filled);
    let mut list2: *mut PrismPdfFormFieldList = std::ptr::null_mut();
    unsafe { prismpdf_document_form_fields(refilled, &mut list2) };
    let mut field2: *const PrismPdfFormField = std::ptr::null();
    unsafe { prismpdf_form_field_list_get(list2, 0, &mut field2) };
    let mut text2: *mut c_char = std::ptr::null_mut();
    unsafe { prismpdf_form_field_value(field2, &mut text2) };
    assert_eq!(take_string(text2), "Off");
    unsafe { prismpdf_form_field_list_free(list2) };
    unsafe { prismpdf_document_free(refilled) };
}

#[test]
fn flatten_form_drops_the_fields() {
    let doc = open(&rich_pdf());
    let mut data: *mut u8 = std::ptr::null_mut();
    let mut out_len = 0usize;
    assert_eq!(
        unsafe { prismpdf_document_flatten_form(doc, &mut data, &mut out_len) },
        PrismPdfStatus::Ok
    );
    let flattened = unsafe { std::slice::from_raw_parts(data, out_len) }.to_vec();
    unsafe { prismpdf_bytes_free(data, out_len) };
    unsafe { prismpdf_document_free(doc) };

    let after = open(&flattened);
    let mut list: *mut PrismPdfFormFieldList = std::ptr::null_mut();
    unsafe { prismpdf_document_form_fields(after, &mut list) };
    let mut len = 99usize;
    unsafe { prismpdf_form_field_list_len(list, &mut len) };
    assert_eq!(len, 0);
    unsafe { prismpdf_form_field_list_free(list) };
    unsafe { prismpdf_document_free(after) };
}

#[test]
fn outline_walks_titles_and_children() {
    let doc = open(&rich_pdf());
    let mut list: *mut PrismPdfOutlineList = std::ptr::null_mut();
    assert_eq!(
        unsafe { prismpdf_document_outline(doc, &mut list) },
        PrismPdfStatus::Ok
    );

    let mut len = 0usize;
    unsafe { prismpdf_outline_list_len(list, &mut len) };
    assert_eq!(len, 2);

    let mut item: *const PrismPdfOutlineItem = std::ptr::null();
    unsafe { prismpdf_outline_list_get(list, 1, &mut item) };

    let mut text: *mut c_char = std::ptr::null_mut();
    unsafe { prismpdf_outline_item_title(item, &mut text) };
    assert_eq!(take_string(text), "Second");

    let mut page = 99usize;
    assert_eq!(
        unsafe { prismpdf_outline_item_dest_page(item, &mut page) },
        PrismPdfStatus::Ok
    );
    assert_eq!(page, 0);

    // Flat outline: no children, and asking for one is NotFound rather than an error.
    let mut kids = 99usize;
    assert_eq!(
        unsafe { prismpdf_outline_item_child_count(item, &mut kids) },
        PrismPdfStatus::Ok
    );
    assert_eq!(kids, 0);
    let mut child: *const PrismPdfOutlineItem = std::ptr::null();
    assert_eq!(
        unsafe { prismpdf_outline_item_child(item, 0, &mut child) },
        PrismPdfStatus::NotFound
    );

    unsafe { prismpdf_outline_list_free(list) };
    unsafe { prismpdf_document_free(doc) };
}

/// A document exercising the read-side surfaces: an attached file, `/Info` entries, an XMP
/// packet and a Standard-14 (deliberately *not* embedded) font.
fn read_side_pdf() -> Vec<u8> {
    use prismpdf::{Attachment, StdFont};
    let mut builder = Builder::new();
    builder.add_page(
        PageSpec::new(b"BT /F1 12 Tf (hi) Tj ET".to_vec()).standard_font("F1", StdFont::Helvetica),
    );
    builder.attach_file(Attachment {
        name: "notes.txt".to_string(),
        mime: "text/plain".to_string(),
        relationship: "Supplement".to_string(),
        description: Some("release notes".to_string()),
        mod_date: None,
        data: b"attached bytes".to_vec(),
    });
    builder.title("Read Side");
    builder.info("CreationDate", "D:20260818120000Z");
    builder.metadata_xmp(b"<?xpacket?><x:xmpmeta/>".to_vec());
    builder.build()
}

#[test]
fn attachments_expose_metadata_and_lend_bytes() {
    let doc = open(&read_side_pdf());
    let mut list: *mut PrismPdfAttachmentList = std::ptr::null_mut();
    assert_eq!(
        unsafe { prismpdf_document_attachments(doc, &mut list) },
        PrismPdfStatus::Ok
    );

    let mut len = 0usize;
    unsafe { prismpdf_attachment_list_len(list, &mut len) };
    assert_eq!(len, 1);

    let mut att: *const PrismPdfAttachment = std::ptr::null();
    unsafe { prismpdf_attachment_list_get(list, 0, &mut att) };

    let mut text: *mut c_char = std::ptr::null_mut();
    unsafe { prismpdf_attachment_name(att, &mut text) };
    assert_eq!(take_string(text), "notes.txt");
    unsafe { prismpdf_attachment_mime(att, &mut text) };
    assert_eq!(take_string(text), "text/plain");
    unsafe { prismpdf_attachment_relationship(att, &mut text) };
    assert_eq!(take_string(text), "Supplement");
    unsafe { prismpdf_attachment_description(att, &mut text) };
    assert_eq!(take_string(text), "release notes");

    // The payload is lent, not copied: no free, and it matches what went in.
    let mut data: *const u8 = std::ptr::null();
    let mut data_len = 0usize;
    assert_eq!(
        unsafe { prismpdf_attachment_data(att, &mut data, &mut data_len) },
        PrismPdfStatus::Ok
    );
    assert_eq!(
        unsafe { std::slice::from_raw_parts(data, data_len) },
        b"attached bytes"
    );

    unsafe { prismpdf_attachment_list_free(list) };
    unsafe { prismpdf_document_free(doc) };
}

#[test]
fn fonts_report_standard_14_as_not_embedded() {
    let doc = open(&read_side_pdf());
    let mut list: *mut PrismPdfFontList = std::ptr::null_mut();
    assert_eq!(
        unsafe { prismpdf_document_fonts(doc, &mut list) },
        PrismPdfStatus::Ok
    );

    let mut len = 0usize;
    unsafe { prismpdf_font_list_len(list, &mut len) };
    assert_eq!(len, 1);

    let mut font: *const PrismPdfFont = std::ptr::null();
    unsafe { prismpdf_font_list_get(list, 0, &mut font) };

    let mut text: *mut c_char = std::ptr::null_mut();
    unsafe { prismpdf_font_base_font(font, &mut text) };
    assert_eq!(take_string(text), "Helvetica");
    unsafe { prismpdf_font_subtype(font, &mut text) };
    assert_eq!(take_string(text), "Type1");

    // Not embedded: every program-dependent getter reports NotFound rather than erroring.
    let mut format = PrismPdfFontFormat::Type1;
    assert_eq!(
        unsafe { prismpdf_font_program_format(font, &mut format) },
        PrismPdfStatus::NotFound
    );
    let mut data: *const u8 = std::ptr::null();
    let mut data_len = 0usize;
    assert_eq!(
        unsafe { prismpdf_font_program(font, &mut data, &mut data_len) },
        PrismPdfStatus::NotFound
    );
    let (mut upem, mut glyphs) = (1u16, 1u16);
    assert_eq!(
        unsafe { prismpdf_font_metrics(font, &mut upem, &mut glyphs) },
        PrismPdfStatus::NotFound
    );
    assert_eq!(
        unsafe { prismpdf_font_family_name(font, &mut text) },
        PrismPdfStatus::NotFound
    );

    unsafe { prismpdf_font_list_free(list) };

    // Subsetting a Standard-14-only document still round-trips.
    let mut out: *mut u8 = std::ptr::null_mut();
    let mut out_len = 0usize;
    assert_eq!(
        unsafe { prismpdf_document_subset_fonts(doc, &mut out, &mut out_len) },
        PrismPdfStatus::Ok
    );
    assert!(out_len > 0);
    unsafe { prismpdf_bytes_free(out, out_len) };
    unsafe { prismpdf_document_free(doc) };
}

#[test]
fn images_expose_geometry_colour_and_payload() {
    use prismpdf::{Flow, Image, PageStyle};
    let rgb = vec![0x7Fu8; 4 * 3 * 3]; // 4x3 pixels, 3 bytes each
    let image = Image::from_rgb(4, 3, rgb).unwrap();
    let mut flow = Flow::new(PageStyle::letter(36.0), &[]);
    flow.image(&image, 40.0, 30.0);
    let doc = open(&flow.build());

    let mut list: *mut PrismPdfImageList = std::ptr::null_mut();
    assert_eq!(
        unsafe { prismpdf_page_images(doc, 0, &mut list) },
        PrismPdfStatus::Ok
    );
    let mut len = 0usize;
    unsafe { prismpdf_image_list_len(list, &mut len) };
    assert_eq!(len, 1);

    let mut img: *const PrismPdfImage = std::ptr::null();
    unsafe { prismpdf_image_list_get(list, 0, &mut img) };

    let (mut w, mut h, mut bpc) = (0u32, 0u32, 0u8);
    assert_eq!(
        unsafe { prismpdf_image_info(img, &mut w, &mut h, &mut bpc) },
        PrismPdfStatus::Ok
    );
    assert_eq!((w, h, bpc), (4, 3, 8));

    let mut space = PrismPdfColorSpace::Other;
    unsafe { prismpdf_image_color_space(img, &mut space) };
    assert_eq!(space, PrismPdfColorSpace::DeviceRgb);

    let mut components = 0u8;
    unsafe { prismpdf_image_components(img, &mut components) };
    assert_eq!(components, 3);

    let mut kind = PrismPdfImageKind::Jpeg;
    unsafe { prismpdf_image_kind(img, &mut kind) };
    assert_eq!(kind, PrismPdfImageKind::Raw);

    let mut data: *const u8 = std::ptr::null();
    let mut data_len = 0usize;
    assert_eq!(
        unsafe { prismpdf_image_data(img, &mut data, &mut data_len) },
        PrismPdfStatus::Ok
    );
    assert_eq!(data_len, 4 * 3 * 3);

    unsafe { prismpdf_image_list_free(list) };
    unsafe { prismpdf_document_free(doc) };
}

#[test]
fn page_spec_exposes_precision_resources_and_transfer_ownership() {
    let builder = prismpdf_builder_new();
    let content = prismpdf_content_new();
    let f_body = CString::new("FBody").unwrap();
    let f_bold = CString::new("FBold").unwrap();
    let im_red = CString::new("ImRed").unwrap();
    let im_blue = CString::new("ImBlue").unwrap();

    assert_eq!(
        unsafe { prismpdf_content_begin_text(content) },
        PrismPdfStatus::Ok
    );
    assert_eq!(
        unsafe { prismpdf_content_set_font(content, f_body.as_ptr(), 12.0) },
        PrismPdfStatus::Ok
    );
    let hello = b"precision page";
    assert_eq!(
        unsafe { prismpdf_content_show_text(content, hello.as_ptr(), hello.len()) },
        PrismPdfStatus::Ok
    );
    assert_eq!(
        unsafe { prismpdf_content_end_text(content) },
        PrismPdfStatus::Ok
    );
    assert_eq!(
        unsafe { prismpdf_content_do_xobject(content, im_red.as_ptr()) },
        PrismPdfStatus::Ok
    );
    assert_eq!(
        unsafe { prismpdf_content_do_xobject(content, im_blue.as_ptr()) },
        PrismPdfStatus::Ok
    );

    let page = unsafe { prismpdf_page_spec_new(content) };
    assert!(!page.is_null());
    assert_eq!(
        unsafe { prismpdf_page_spec_set_media_box(page, 10.0, 20.0, 310.0, 420.0) },
        PrismPdfStatus::Ok
    );
    assert_eq!(
        unsafe {
            prismpdf_page_spec_add_standard_font(page, f_body.as_ptr(), PrismPdfStdFont::Helvetica)
        },
        PrismPdfStatus::Ok
    );
    assert_eq!(
        unsafe {
            prismpdf_page_spec_add_standard_font(
                page,
                f_bold.as_ptr(),
                PrismPdfStdFont::HelveticaBold,
            )
        },
        PrismPdfStatus::Ok
    );

    let red = [255, 0, 0];
    let blue = [0, 0, 255];
    let red_image = unsafe { prismpdf_image_source_from_rgb(1, 1, red.as_ptr(), red.len()) };
    let blue_image = unsafe { prismpdf_image_source_from_rgb(1, 1, blue.as_ptr(), blue.len()) };
    assert_eq!(
        unsafe { prismpdf_page_spec_add_image(page, im_red.as_ptr(), red_image) },
        PrismPdfStatus::Ok
    );
    assert_eq!(
        unsafe { prismpdf_page_spec_add_image(page, im_blue.as_ptr(), blue_image) },
        PrismPdfStatus::Ok
    );

    // A rejected transfer retains ownership of the page specification.
    assert_eq!(
        unsafe { prismpdf_builder_add_page_spec(std::ptr::null_mut(), page) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_builder_add_page_spec(builder, page) },
        PrismPdfStatus::Ok
    );

    let mut data = std::ptr::null_mut();
    let mut len = 0usize;
    assert_eq!(
        unsafe { prismpdf_builder_build(builder, &mut data, &mut len) },
        PrismPdfStatus::Ok
    );
    let bytes = unsafe { std::slice::from_raw_parts(data, len) };
    let text = String::from_utf8_lossy(bytes);
    assert!(text.contains("/MediaBox"));
    assert!(text.contains("310"));
    assert!(text.contains("420"));
    assert!(text.contains("/FBody"));
    assert!(text.contains("/FBold"));

    let doc = open(bytes);
    let mut images = std::ptr::null_mut();
    assert_eq!(
        unsafe { prismpdf_page_images(doc, 0, &mut images) },
        PrismPdfStatus::Ok
    );
    let mut image_count = 0;
    assert_eq!(
        unsafe { prismpdf_image_list_len(images, &mut image_count) },
        PrismPdfStatus::Ok
    );
    assert_eq!(image_count, 2);

    unsafe {
        prismpdf_image_list_free(images);
        prismpdf_document_free(doc);
        prismpdf_bytes_free(data, len);
        prismpdf_image_source_free(red_image);
        prismpdf_image_source_free(blue_image);
        prismpdf_content_free(content);
        prismpdf_builder_free(builder);
    }
}

#[test]
fn raw_structure_nodes_build_nested_tagged_pdf20() {
    unsafe {
        let builder = prismpdf_builder_new();
        let content = prismpdf_content_new();
        let h1_tag = CString::new("H1").unwrap();
        let p_tag = CString::new("P").unwrap();
        assert_eq!(
            prismpdf_content_begin_marked_content(content, h1_tag.as_ptr(), 0),
            PrismPdfStatus::Ok
        );
        assert_eq!(
            prismpdf_content_end_marked_content(content),
            PrismPdfStatus::Ok
        );
        assert_eq!(
            prismpdf_content_begin_marked_content(content, p_tag.as_ptr(), 1),
            PrismPdfStatus::Ok
        );
        assert_eq!(
            prismpdf_content_end_marked_content(content),
            PrismPdfStatus::Ok
        );
        let (mut bytes, mut len) = (std::ptr::null(), 0);
        assert_eq!(
            prismpdf_content_bytes(content, &mut bytes, &mut len),
            PrismPdfStatus::Ok
        );
        assert_eq!(
            prismpdf_builder_add_page(builder, bytes, len, std::ptr::null(), std::ptr::null(), 0),
            PrismPdfStatus::Ok
        );

        let root = prismpdf_struct_node_new(h1_tag.as_ptr());
        let child = prismpdf_struct_node_new(p_tag.as_ptr());
        let title = CString::new("Raw heading").unwrap();
        let replacement = CString::new("Heading text").unwrap();
        let lang = CString::new("en-US").unwrap();
        let namespace = CString::new(prismpdf::PDF2_STRUCT_NS).unwrap();
        let root_id = CString::new("heading-id").unwrap();
        let child_id = CString::new("paragraph-id").unwrap();
        assert_eq!(
            prismpdf_struct_node_set_alt(root, title.as_ptr()),
            PrismPdfStatus::Ok
        );
        assert_eq!(
            prismpdf_struct_node_set_actual_text(root, replacement.as_ptr()),
            PrismPdfStatus::Ok
        );
        assert_eq!(
            prismpdf_struct_node_set_lang(root, lang.as_ptr()),
            PrismPdfStatus::Ok
        );
        assert_eq!(
            prismpdf_struct_node_set_namespace(root, namespace.as_ptr()),
            PrismPdfStatus::Ok
        );
        assert_eq!(
            prismpdf_struct_node_set_id(root, root_id.as_ptr()),
            PrismPdfStatus::Ok
        );
        assert_eq!(
            prismpdf_struct_node_set_id(child, child_id.as_ptr()),
            PrismPdfStatus::Ok
        );
        assert_eq!(
            prismpdf_struct_node_add_reference(root, child_id.as_ptr()),
            PrismPdfStatus::Ok
        );
        assert_eq!(
            prismpdf_struct_node_add_reference(child, root_id.as_ptr()),
            PrismPdfStatus::Ok
        );
        assert_eq!(
            prismpdf_struct_node_add_content(root, 0, 0),
            PrismPdfStatus::Ok
        );
        assert_eq!(
            prismpdf_struct_node_add_content(child, 0, 1),
            PrismPdfStatus::Ok
        );

        let owner = CString::new("Layout").unwrap();
        let key_name = CString::new("Placement").unwrap();
        let value_name = CString::new("Block").unwrap();
        let key_int = CString::new("ColSpan").unwrap();
        let key_text = CString::new("Summary").unwrap();
        let value_text = CString::new("A summary").unwrap();
        assert_eq!(
            prismpdf_struct_node_add_name_attribute(
                root,
                owner.as_ptr(),
                key_name.as_ptr(),
                value_name.as_ptr()
            ),
            PrismPdfStatus::Ok
        );
        assert_eq!(
            prismpdf_struct_node_add_integer_attribute(root, owner.as_ptr(), key_int.as_ptr(), 2),
            PrismPdfStatus::Ok
        );
        assert_eq!(
            prismpdf_struct_node_add_text_attribute(
                root,
                owner.as_ptr(),
                key_text.as_ptr(),
                value_text.as_ptr()
            ),
            PrismPdfStatus::Ok
        );

        let file_name = CString::new("structure.txt").unwrap();
        let mime = CString::new("text/plain").unwrap();
        let relationship = CString::new("Supplement").unwrap();
        let payload = b"structure schema note";
        assert_eq!(
            prismpdf_struct_node_associate_file(
                root,
                file_name.as_ptr(),
                mime.as_ptr(),
                relationship.as_ptr(),
                std::ptr::null(),
                payload.as_ptr(),
                payload.len()
            ),
            PrismPdfStatus::Ok
        );

        assert_eq!(
            prismpdf_struct_node_add_child(std::ptr::null_mut(), child),
            PrismPdfStatus::NullArgument
        );
        assert_eq!(
            prismpdf_struct_node_add_child(root, child),
            PrismPdfStatus::Ok
        );
        assert_eq!(
            prismpdf_builder_set_structure_namespace(builder, namespace.as_ptr()),
            PrismPdfStatus::Ok
        );
        assert_eq!(
            prismpdf_builder_add_structure_node(std::ptr::null_mut(), root),
            PrismPdfStatus::NullArgument
        );
        assert_eq!(
            prismpdf_builder_add_structure_node(builder, root),
            PrismPdfStatus::Ok
        );

        let (mut output, mut output_len) = (std::ptr::null_mut(), 0);
        assert_eq!(
            prismpdf_builder_build(builder, &mut output, &mut output_len),
            PrismPdfStatus::Ok
        );
        let pdf_bytes = std::slice::from_raw_parts(output, output_len);
        assert!(pdf_bytes.starts_with(b"%PDF-2.0"));
        let serialized = String::from_utf8_lossy(pdf_bytes);
        assert!(serialized.contains("/StructTreeRoot"));
        assert!(serialized.contains("/ActualText"));
        assert!(serialized.contains("/ColSpan 2"));

        let doc = open(pdf_bytes);
        let mut namespaces = std::ptr::null_mut();
        assert_eq!(
            prismpdf_document_structure_namespaces(doc, &mut namespaces),
            PrismPdfStatus::Ok
        );
        let mut namespace_count = 0;
        assert_eq!(
            prismpdf_string_list_len(namespaces, &mut namespace_count),
            PrismPdfStatus::Ok
        );
        assert_eq!(namespace_count, 1);
        let mut attachments = std::ptr::null_mut();
        assert_eq!(
            prismpdf_document_attachments(doc, &mut attachments),
            PrismPdfStatus::Ok
        );
        let mut attachment_count = 0;
        assert_eq!(
            prismpdf_attachment_list_len(attachments, &mut attachment_count),
            PrismPdfStatus::Ok
        );
        assert_eq!(attachment_count, 1);

        prismpdf_attachment_list_free(attachments);
        prismpdf_string_list_free(namespaces);
        prismpdf_document_free(doc);
        prismpdf_bytes_free(output, output_len);
        prismpdf_content_free(content);
        prismpdf_builder_free(builder);
        prismpdf_struct_node_free(std::ptr::null_mut());
    }
}

#[test]
fn metadata_reads_info_xmp_and_dates() {
    let doc = open(&read_side_pdf());

    let mut text: *mut c_char = std::ptr::null_mut();
    let key = CString::new("Title").unwrap();
    assert_eq!(
        unsafe { prismpdf_document_info(doc, key.as_ptr(), &mut text) },
        PrismPdfStatus::Ok
    );
    assert_eq!(take_string(text), "Read Side");

    // An absent key is NotFound, not an error.
    let missing = CString::new("Producer").unwrap();
    assert_eq!(
        unsafe { prismpdf_document_info(doc, missing.as_ptr(), &mut text) },
        PrismPdfStatus::NotFound
    );

    assert_eq!(
        unsafe { prismpdf_document_xmp(doc, &mut text) },
        PrismPdfStatus::Ok
    );
    assert!(take_string(text).contains("xmpmeta"));

    let mut date = PrismPdfDate {
        year: 0,
        month: 0,
        day: 0,
        hour: 0,
        minute: 0,
        second: 0,
        has_utc_offset: false,
        utc_offset_minutes: 0,
    };
    assert_eq!(
        unsafe { prismpdf_document_creation_date(doc, &mut date) },
        PrismPdfStatus::Ok
    );
    assert_eq!((date.year, date.month, date.day), (2026, 8, 18));
    assert!(date.has_utc_offset);
    assert_eq!(date.utc_offset_minutes, 0);

    // No /ModDate was written.
    assert_eq!(
        unsafe { prismpdf_document_modification_date(doc, &mut date) },
        PrismPdfStatus::NotFound
    );

    unsafe { prismpdf_document_free(doc) };
}

#[test]
fn positioned_text_reconstructs_layout() {
    let doc = open(&sample_pdf());
    let mut text: *mut c_char = std::ptr::null_mut();
    assert_eq!(
        unsafe { prismpdf_page_text_positioned(doc, 0, &mut text) },
        PrismPdfStatus::Ok
    );
    assert!(take_string(text).contains("Hello FFI"));

    assert_eq!(
        unsafe { prismpdf_page_text_positioned(doc, 9, &mut text) },
        PrismPdfStatus::NotFound
    );
    unsafe { prismpdf_document_free(doc) };
}

/// The throwaway self-signed RSA-2048 signer used by the `prismpdf` crate's own examples.
const TEST_CERT: &[u8] = include_bytes!("../../../pdf/examples/test-signer/cert.der");
/// Its private key.
const TEST_KEY: &[u8] = include_bytes!("../../../pdf/examples/test-signer/key.der");

/// Build a complete document using **only** C entry points, then read it back through them.
/// This is the parity proof: nothing here touches the Rust facade directly.
#[test]
fn a_document_is_authored_and_read_back_entirely_through_c() {
    // --- draw a page -------------------------------------------------------------------
    let content = prismpdf_content_new();
    assert!(!content.is_null());
    unsafe {
        prismpdf_content_save(content);
        prismpdf_content_set_fill_rgb(content, 0.1, 0.2, 0.3);
        prismpdf_content_rect(content, 50.0, 50.0, 120.0, 80.0);
        prismpdf_content_fill(content);
        prismpdf_content_set_line_width(content, 2.0);
        prismpdf_content_set_stroke_gray(content, 0.0);
        prismpdf_content_move_to(content, 50.0, 200.0);
        prismpdf_content_line_to(content, 300.0, 200.0);
        prismpdf_content_stroke(content);
        prismpdf_content_restore(content);
    }
    let font_name = CString::new("F1").unwrap();
    let hello = CString::new("Hello from C").unwrap();
    unsafe {
        prismpdf_content_begin_text(content);
        prismpdf_content_set_font(content, font_name.as_ptr(), 24.0);
        prismpdf_content_text_move(content, 72.0, 700.0);
        prismpdf_content_show_str(content, hello.as_ptr());
        prismpdf_content_end_text(content);
    }

    let mut stream: *const u8 = std::ptr::null();
    let mut stream_len = 0usize;
    assert_eq!(
        unsafe { prismpdf_content_bytes(content, &mut stream, &mut stream_len) },
        PrismPdfStatus::Ok
    );
    assert!(stream_len > 0);

    // --- assemble the document ----------------------------------------------------------
    let builder = prismpdf_builder_new();
    assert!(!builder.is_null());

    let a4 = [0.0f64, 0.0, 595.0, 842.0];
    assert_eq!(
        unsafe { prismpdf_builder_set_media_box(builder, a4.as_ptr()) },
        PrismPdfStatus::Ok
    );

    let title = CString::new("Authored in C").unwrap();
    let author = CString::new("Ada").unwrap();
    unsafe {
        prismpdf_builder_set_title(builder, title.as_ptr());
        prismpdf_builder_set_author(builder, author.as_ptr());
        prismpdf_builder_set_display_doc_title(builder, true);
    }
    let lang = CString::new("en-GB").unwrap();
    assert_eq!(
        unsafe { prismpdf_builder_set_lang(builder, lang.as_ptr()) },
        PrismPdfStatus::Ok
    );

    let names = [font_name.as_ptr()];
    let fonts = [PrismPdfStdFont::Helvetica];
    assert_eq!(
        unsafe {
            prismpdf_builder_add_page(
                builder,
                stream,
                stream_len,
                names.as_ptr(),
                fonts.as_ptr(),
                1,
            )
        },
        PrismPdfStatus::Ok
    );
    unsafe { prismpdf_content_free(content) };

    // Bookmark, link, note, checkbox and an attachment — one of each authoring shape.
    let bookmark = CString::new("Start").unwrap();
    assert_eq!(
        unsafe { prismpdf_builder_add_outline(builder, bookmark.as_ptr(), 0) },
        PrismPdfStatus::Ok
    );

    let link_rect = [10.0f64, 10.0, 100.0, 30.0];
    let uri = CString::new("https://example.org/").unwrap();
    let alt = CString::new("example site").unwrap();
    assert_eq!(
        unsafe {
            prismpdf_builder_add_link_uri(
                builder,
                0,
                link_rect.as_ptr(),
                uri.as_ptr(),
                alt.as_ptr(),
            )
        },
        PrismPdfStatus::Ok
    );

    let note_rect = [200.0f64, 10.0, 240.0, 40.0];
    let note = CString::new("a note from C").unwrap();
    assert_eq!(
        unsafe { prismpdf_builder_add_note(builder, 0, note_rect.as_ptr(), note.as_ptr()) },
        PrismPdfStatus::Ok
    );

    let box_rect = [300.0f64, 10.0, 320.0, 30.0];
    let field = CString::new("agree").unwrap();
    let tip = CString::new("Accept the terms").unwrap();
    assert_eq!(
        unsafe {
            prismpdf_builder_add_checkbox(
                builder,
                0,
                box_rect.as_ptr(),
                field.as_ptr(),
                true,
                tip.as_ptr(),
            )
        },
        PrismPdfStatus::Ok
    );

    let att_name = CString::new("readme.txt").unwrap();
    let mime = CString::new("text/plain").unwrap();
    let rel = CString::new("Supplement").unwrap();
    let desc = CString::new("a description").unwrap();
    let payload = b"attached from C";
    assert_eq!(
        unsafe {
            prismpdf_builder_attach_file(
                builder,
                att_name.as_ptr(),
                mime.as_ptr(),
                rel.as_ptr(),
                desc.as_ptr(),
                payload.as_ptr(),
                payload.len(),
            )
        },
        PrismPdfStatus::Ok
    );

    let xmp = b"<?xpacket?><x:xmpmeta/>";
    unsafe { prismpdf_builder_set_metadata_xmp(builder, xmp.as_ptr(), xmp.len()) };

    let mut data: *mut u8 = std::ptr::null_mut();
    let mut len = 0usize;
    assert_eq!(
        unsafe { prismpdf_builder_build(builder, &mut data, &mut len) },
        PrismPdfStatus::Ok
    );
    let pdf = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
    unsafe { prismpdf_bytes_free(data, len) };
    unsafe { prismpdf_builder_free(builder) };
    assert!(pdf.starts_with(b"%PDF-"));

    // --- read it back through the same ABI ----------------------------------------------
    let doc = open(&pdf);

    let mut pages = 0usize;
    unsafe { prismpdf_document_page_count(doc, &mut pages) };
    assert_eq!(pages, 1);

    let mut text: *mut c_char = std::ptr::null_mut();
    unsafe { prismpdf_page_text(doc, 0, &mut text) };
    assert!(take_string(text).contains("Hello from C"));

    let key = CString::new("Title").unwrap();
    unsafe { prismpdf_document_info(doc, key.as_ptr(), &mut text) };
    assert_eq!(take_string(text), "Authored in C");

    // Link, note and the checkbox widget all land in /Annots.
    let mut annots: *mut PrismPdfAnnotationList = std::ptr::null_mut();
    unsafe { prismpdf_page_annotations(doc, 0, &mut annots) };
    let mut n = 0usize;
    unsafe { prismpdf_annotation_list_len(annots, &mut n) };
    assert_eq!(n, 3);
    let mut annot: *const PrismPdfAnnotation = std::ptr::null();
    unsafe { prismpdf_annotation_list_get(annots, 0, &mut annot) };
    unsafe { prismpdf_annotation_uri(annot, &mut text) };
    assert_eq!(take_string(text), "https://example.org/");
    unsafe { prismpdf_annotation_list_free(annots) };

    let mut fields: *mut PrismPdfFormFieldList = std::ptr::null_mut();
    unsafe { prismpdf_document_form_fields(doc, &mut fields) };
    unsafe { prismpdf_form_field_list_len(fields, &mut n) };
    assert_eq!(n, 1);
    let mut field_ref: *const PrismPdfFormField = std::ptr::null();
    unsafe { prismpdf_form_field_list_get(fields, 0, &mut field_ref) };
    unsafe { prismpdf_form_field_name(field_ref, &mut text) };
    assert_eq!(take_string(text), "agree");
    unsafe { prismpdf_form_field_list_free(fields) };

    let mut outline: *mut PrismPdfOutlineList = std::ptr::null_mut();
    unsafe { prismpdf_document_outline(doc, &mut outline) };
    unsafe { prismpdf_outline_list_len(outline, &mut n) };
    assert_eq!(n, 1);
    let mut item: *const PrismPdfOutlineItem = std::ptr::null();
    unsafe { prismpdf_outline_list_get(outline, 0, &mut item) };
    unsafe { prismpdf_outline_item_title(item, &mut text) };
    assert_eq!(take_string(text), "Start");
    unsafe { prismpdf_outline_list_free(outline) };

    let mut atts: *mut PrismPdfAttachmentList = std::ptr::null_mut();
    unsafe { prismpdf_document_attachments(doc, &mut atts) };
    unsafe { prismpdf_attachment_list_len(atts, &mut n) };
    assert_eq!(n, 1);
    let mut att: *const PrismPdfAttachment = std::ptr::null();
    unsafe { prismpdf_attachment_list_get(atts, 0, &mut att) };
    let mut view: *const u8 = std::ptr::null();
    let mut view_len = 0usize;
    unsafe { prismpdf_attachment_data(att, &mut view, &mut view_len) };
    assert_eq!(
        unsafe { std::slice::from_raw_parts(view, view_len) },
        b"attached from C"
    );
    unsafe { prismpdf_attachment_list_free(atts) };

    unsafe { prismpdf_document_xmp(doc, &mut text) };
    assert!(take_string(text).contains("xmpmeta"));

    unsafe { prismpdf_document_free(doc) };
}

#[test]
fn build_for_stamps_the_requested_version() {
    let builder = prismpdf_builder_new();
    unsafe {
        prismpdf_builder_add_page(
            builder,
            b"BT ET".as_ptr(),
            5,
            std::ptr::null(),
            std::ptr::null(),
            0,
        )
    };

    for (major, minor, header) in [(1u8, 7u8, &b"%PDF-1.7"[..]), (2, 0, &b"%PDF-2.0"[..])] {
        let mut data: *mut u8 = std::ptr::null_mut();
        let mut len = 0usize;
        assert_eq!(
            unsafe { prismpdf_builder_build_for(builder, major, minor, &mut data, &mut len) },
            PrismPdfStatus::Ok
        );
        let pdf = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
        unsafe { prismpdf_bytes_free(data, len) };
        assert!(
            pdf.starts_with(header),
            "build_for({major}, {minor}) must stamp {}",
            String::from_utf8_lossy(header)
        );
    }

    // The refusal path (a construct above the target) is not exercised here: every PDF 2.0-only
    // construct — document parts, structure namespaces, page-level /AF — is still absent from
    // the C ABI, so none can be built to trip the gate. Cover it when those land.
    unsafe { prismpdf_builder_free(builder) };
}

#[test]
fn content_covers_the_remaining_operators() {
    let content = prismpdf_content_new();
    let cs = CString::new("Sep1").unwrap();
    let xobj = CString::new("Im1").unwrap();
    let tag = CString::new("P").unwrap();
    let prop = CString::new("AF1").unwrap();
    let inline_cs = CString::new("G").unwrap();
    let components = [0.5f64];
    let gids = [3u16, 7, 11];

    unsafe {
        prismpdf_content_transform(content, 1.0, 0.0, 0.0, 1.0, 10.0, 20.0);
        prismpdf_content_set_fill_gray(content, 0.5);
        prismpdf_content_set_stroke_rgb(content, 1.0, 0.0, 0.0);
        prismpdf_content_set_fill_cmyk(content, 0.0, 0.1, 0.2, 0.3);
        prismpdf_content_curve_to(content, 0.0, 0.0, 1.0, 1.0, 2.0, 2.0);
        prismpdf_content_close_path(content);
        prismpdf_content_fill_and_stroke(content);
        prismpdf_content_set_char_spacing(content, 0.5);
        prismpdf_content_set_word_spacing(content, 1.0);
        prismpdf_content_set_leading(content, 14.0);
        prismpdf_content_set_text_matrix(content, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
        prismpdf_content_next_line(content);
        prismpdf_content_begin_artifact(content);
        prismpdf_content_end_marked_content(content);
    }
    assert_eq!(
        unsafe { prismpdf_content_set_fill_color_space(content, cs.as_ptr()) },
        PrismPdfStatus::Ok
    );
    assert_eq!(
        unsafe { prismpdf_content_set_fill_color(content, components.as_ptr(), 1) },
        PrismPdfStatus::Ok
    );
    assert_eq!(
        unsafe { prismpdf_content_do_xobject(content, xobj.as_ptr()) },
        PrismPdfStatus::Ok
    );
    assert_eq!(
        unsafe {
            prismpdf_content_inline_image(
                content,
                2,
                2,
                inline_cs.as_ptr(),
                8,
                b"\x00\xFF\x80\x40".as_ptr(),
                4,
            )
        },
        PrismPdfStatus::Ok
    );
    assert_eq!(
        unsafe { prismpdf_content_show_text(content, b"raw".as_ptr(), 3) },
        PrismPdfStatus::Ok
    );
    assert_eq!(
        unsafe { prismpdf_content_show_glyphs(content, gids.as_ptr(), 3) },
        PrismPdfStatus::Ok
    );
    assert_eq!(
        unsafe { prismpdf_content_begin_marked_content(content, tag.as_ptr(), 0) },
        PrismPdfStatus::Ok
    );
    assert_eq!(
        unsafe { prismpdf_content_begin_af_marked_content(content, prop.as_ptr()) },
        PrismPdfStatus::Ok
    );

    let mut view: *const u8 = std::ptr::null();
    let mut view_len = 0usize;
    unsafe { prismpdf_content_bytes(content, &mut view, &mut view_len) };
    assert!(view_len > 0);
    unsafe { prismpdf_content_free(content) };

    // An empty stream lends a null pointer with length 0.
    let empty = prismpdf_content_new();
    let mut ptr: *const u8 = std::ptr::null();
    let mut n = 99usize;
    assert_eq!(
        unsafe { prismpdf_content_bytes(empty, &mut ptr, &mut n) },
        PrismPdfStatus::Ok
    );
    assert!(ptr.is_null());
    assert_eq!(n, 0);
    unsafe { prismpdf_content_free(empty) };
}

#[test]
fn builder_covers_the_remaining_setters_and_link_targets() {
    let builder = prismpdf_builder_new();
    unsafe {
        prismpdf_builder_add_page(
            builder,
            b"BT ET".as_ptr(),
            5,
            std::ptr::null(),
            std::ptr::null(),
            0,
        );
        prismpdf_builder_add_page(
            builder,
            b"BT ET".as_ptr(),
            5,
            std::ptr::null(),
            std::ptr::null(),
            0,
        );
    }

    let subject = CString::new("Parity").unwrap();
    let keywords = CString::new("pdf, ffi, parity").unwrap();
    let creator = CString::new("prismpdf tests").unwrap();
    assert_eq!(
        unsafe { prismpdf_builder_set_subject(builder, subject.as_ptr()) },
        PrismPdfStatus::Ok
    );
    assert_eq!(
        unsafe { prismpdf_builder_set_keywords(builder, keywords.as_ptr()) },
        PrismPdfStatus::Ok
    );
    assert_eq!(
        unsafe { prismpdf_builder_set_creator(builder, creator.as_ptr()) },
        PrismPdfStatus::Ok
    );

    let key = CString::new("Producer").unwrap();
    let value = CString::new("Prism PDF C ABI").unwrap();
    assert_eq!(
        unsafe { prismpdf_builder_set_info(builder, key.as_ptr(), value.as_ptr()) },
        PrismPdfStatus::Ok
    );
    assert_eq!(
        unsafe { prismpdf_builder_set_version(builder, 1, 7) },
        PrismPdfStatus::Ok
    );
    assert_eq!(
        unsafe { prismpdf_builder_set_file_id(builder, b"fixed-id-0123456".as_ptr(), 16) },
        PrismPdfStatus::Ok
    );
    assert_eq!(
        unsafe { prismpdf_builder_set_utf8_text_strings(builder) },
        PrismPdfStatus::Ok
    );

    // All four link targets.
    let rect = [10.0f64, 10.0, 60.0, 30.0];
    let element = CString::new("sect1").unwrap();
    assert_eq!(
        unsafe { prismpdf_builder_add_link_page(builder, 0, rect.as_ptr(), 1, std::ptr::null()) },
        PrismPdfStatus::Ok
    );
    assert_eq!(
        unsafe {
            prismpdf_builder_add_link_element(
                builder,
                0,
                rect.as_ptr(),
                element.as_ptr(),
                std::ptr::null(),
            )
        },
        PrismPdfStatus::Ok
    );
    assert_eq!(
        unsafe {
            prismpdf_builder_add_link_document_part(builder, 0, rect.as_ptr(), 0, std::ptr::null())
        },
        PrismPdfStatus::Ok
    );

    let mut data: *mut u8 = std::ptr::null_mut();
    let mut len = 0usize;
    assert_eq!(
        unsafe { prismpdf_builder_build(builder, &mut data, &mut len) },
        PrismPdfStatus::Ok
    );
    let pdf = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
    unsafe { prismpdf_bytes_free(data, len) };

    // The metadata really landed, read back through the ABI.
    let doc = open(&pdf);
    let mut text: *mut c_char = std::ptr::null_mut();
    unsafe { prismpdf_document_info(doc, key.as_ptr(), &mut text) };
    assert_eq!(take_string(text), "Prism PDF C ABI");
    let subject_key = CString::new("Subject").unwrap();
    unsafe { prismpdf_document_info(doc, subject_key.as_ptr(), &mut text) };
    assert_eq!(take_string(text), "Parity");
    unsafe { prismpdf_document_free(doc) };

    // `clear_info` drops everything set so far.
    assert_eq!(
        unsafe { prismpdf_builder_clear_info(builder) },
        PrismPdfStatus::Ok
    );
    assert_eq!(
        unsafe { prismpdf_builder_build(builder, &mut data, &mut len) },
        PrismPdfStatus::Ok
    );
    let cleared = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
    unsafe { prismpdf_bytes_free(data, len) };
    let doc = open(&cleared);
    assert_eq!(
        unsafe { prismpdf_document_info(doc, key.as_ptr(), &mut text) },
        PrismPdfStatus::NotFound
    );
    unsafe { prismpdf_document_free(doc) };

    unsafe { prismpdf_builder_free(builder) };
}

#[test]
fn authoring_rejects_invalid_utf8_arguments() {
    // A lone 0x80 continuation byte is not valid UTF-8; every string argument must refuse it
    // rather than lossily substituting.
    let bad = [0x80u8, 0x00];
    let bad_ptr = bad.as_ptr().cast::<c_char>();

    let builder = prismpdf_builder_new();
    assert_eq!(
        unsafe { prismpdf_builder_set_title(builder, bad_ptr) },
        PrismPdfStatus::NullArgument
    );
    let good = CString::new("k").unwrap();
    assert_eq!(
        unsafe { prismpdf_builder_set_info(builder, good.as_ptr(), bad_ptr) },
        PrismPdfStatus::NullArgument
    );
    let rect = [0.0f64; 4];
    assert_eq!(
        unsafe { prismpdf_builder_add_note(builder, 0, rect.as_ptr(), bad_ptr) },
        PrismPdfStatus::NullArgument
    );
    // An invalid *optional* argument is refused too, not silently dropped.
    let uri = CString::new("https://example.org/").unwrap();
    assert_eq!(
        unsafe { prismpdf_builder_add_link_uri(builder, 0, rect.as_ptr(), uri.as_ptr(), bad_ptr) },
        PrismPdfStatus::NullArgument
    );
    let field = CString::new("f").unwrap();
    assert_eq!(
        unsafe {
            prismpdf_builder_add_checkbox(builder, 0, rect.as_ptr(), field.as_ptr(), false, bad_ptr)
        },
        PrismPdfStatus::NullArgument
    );
    let name = CString::new("f.txt").unwrap();
    assert_eq!(
        unsafe {
            prismpdf_builder_attach_file(
                builder,
                name.as_ptr(),
                bad_ptr,
                bad_ptr,
                std::ptr::null(),
                std::ptr::null(),
                0,
            )
        },
        PrismPdfStatus::NullArgument
    );
    unsafe { prismpdf_builder_free(builder) };

    let content = prismpdf_content_new();
    assert_eq!(
        unsafe { prismpdf_content_show_str(content, bad_ptr) },
        PrismPdfStatus::NullArgument
    );
    // Empty slices are accepted where a slice is optional.
    assert_eq!(
        unsafe { prismpdf_content_set_fill_color(content, std::ptr::null(), 0) },
        PrismPdfStatus::Ok
    );
    assert_eq!(
        unsafe { prismpdf_content_show_glyphs(content, std::ptr::null(), 0) },
        PrismPdfStatus::Ok
    );
    // …but a null pointer with a non-zero count is a caller bug.
    assert_eq!(
        unsafe { prismpdf_content_set_fill_color(content, std::ptr::null(), 3) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_content_show_glyphs(content, std::ptr::null(), 3) },
        PrismPdfStatus::NullArgument
    );
    unsafe { prismpdf_content_free(content) };
}

/// A builder with one page of pure vector content — no fonts, so nothing blocks PDF/A.
fn vector_only_builder() -> *mut PrismPdfBuilder {
    let content = prismpdf_content_new();
    unsafe {
        prismpdf_content_set_fill_rgb(content, 0.2, 0.4, 0.6);
        prismpdf_content_rect(content, 20.0, 20.0, 100.0, 60.0);
        prismpdf_content_fill(content);
    }
    let mut stream: *const u8 = std::ptr::null();
    let mut stream_len = 0usize;
    unsafe { prismpdf_content_bytes(content, &mut stream, &mut stream_len) };

    let builder = prismpdf_builder_new();
    unsafe {
        prismpdf_builder_add_page(
            builder,
            stream,
            stream_len,
            std::ptr::null(),
            std::ptr::null(),
            0,
        );
        prismpdf_content_free(content);
    }
    builder
}

/// An XMP handle with every single-value field and two authors set.
fn full_xmp() -> *mut PrismPdfXmpMetadata {
    let meta = prismpdf_xmp_metadata_new();
    let values = [
        (
            prismpdf_xmp_metadata_set_title as unsafe extern "C" fn(_, _) -> PrismPdfStatus,
            "Archival Report",
        ),
        (prismpdf_xmp_metadata_set_subject, "A subject"),
        (prismpdf_xmp_metadata_set_keywords, "archive, pdfa"),
        (prismpdf_xmp_metadata_set_creator_tool, "prismpdf"),
        (prismpdf_xmp_metadata_set_producer, "prismpdf C ABI"),
        (
            prismpdf_xmp_metadata_set_create_date,
            "2026-08-18T12:00:00Z",
        ),
        (
            prismpdf_xmp_metadata_set_modify_date,
            "2026-08-18T12:00:00Z",
        ),
    ];
    for (setter, value) in values {
        let text = CString::new(value).unwrap();
        assert_eq!(unsafe { setter(meta, text.as_ptr()) }, PrismPdfStatus::Ok);
    }
    for author in ["Ada Lovelace", "Grace Hopper"] {
        let text = CString::new(author).unwrap();
        assert_eq!(
            unsafe { prismpdf_xmp_metadata_add_author(meta, text.as_ptr()) },
            PrismPdfStatus::Ok
        );
    }
    meta
}

/// A US Letter flow with a 54pt margin and one Standard-14 font.
fn letter_flow() -> (*mut PrismPdfFlow, CString) {
    let name = CString::new("F1").unwrap();
    let size = [612.0f64, 792.0];
    let margins = [54.0f64; 4];
    let names = [name.as_ptr()];
    let fonts = [PrismPdfStdFont::Helvetica];
    let flow = unsafe {
        prismpdf_flow_new(
            size.as_ptr(),
            margins.as_ptr(),
            names.as_ptr(),
            fonts.as_ptr(),
            1,
        )
    };
    assert!(!flow.is_null());
    (flow, name)
}

#[test]
fn a_flowed_report_is_composed_and_read_back_through_c() {
    let (flow, _font) = letter_flow();
    let resource = CString::new("F1").unwrap();
    let base = CString::new("Helvetica").unwrap();
    let body = unsafe {
        prismpdf_text_block_new(
            resource.as_ptr(),
            base.as_ptr(),
            11.0,
            14.0,
            PrismPdfAlign::Justify,
        )
    };
    let head = unsafe {
        prismpdf_text_block_new(
            resource.as_ptr(),
            base.as_ptr(),
            20.0,
            24.0,
            PrismPdfAlign::Left,
        )
    };
    assert!(!body.is_null() && !head.is_null());

    let lang = CString::new("en-GB").unwrap();
    assert_eq!(
        unsafe { prismpdf_flow_set_tagged(flow, lang.as_ptr()) },
        PrismPdfStatus::Ok
    );

    let title = CString::new("Quarterly Report").unwrap();
    assert_eq!(
        unsafe { prismpdf_flow_set_title(flow, title.as_ptr()) },
        PrismPdfStatus::Ok
    );
    let author = CString::new("Ada Lovelace").unwrap();
    assert_eq!(
        unsafe { prismpdf_flow_set_author(flow, author.as_ptr()) },
        PrismPdfStatus::Ok
    );
    let key = CString::new("Producer").unwrap();
    let value = CString::new("prismpdf flow").unwrap();
    assert_eq!(
        unsafe { prismpdf_flow_set_info(flow, key.as_ptr(), value.as_ptr()) },
        PrismPdfStatus::Ok
    );

    let running = CString::new("Confidential").unwrap();
    assert_eq!(
        unsafe { prismpdf_flow_set_header(flow, body, running.as_ptr()) },
        PrismPdfStatus::Ok
    );
    assert_eq!(
        unsafe { prismpdf_flow_set_footer(flow, body, running.as_ptr()) },
        PrismPdfStatus::Ok
    );

    assert_eq!(
        unsafe { prismpdf_flow_title_element(flow, head, title.as_ptr()) },
        PrismPdfStatus::Ok
    );
    let bookmark = CString::new("Overview").unwrap();
    assert_eq!(
        unsafe { prismpdf_flow_add_bookmark(flow, bookmark.as_ptr()) },
        PrismPdfStatus::Ok
    );
    let h1 = CString::new("Overview").unwrap();
    assert_eq!(
        unsafe { prismpdf_flow_heading(flow, 1, head, h1.as_ptr()) },
        PrismPdfStatus::Ok
    );

    let para = CString::new(
            "This paragraph is long enough to wrap across several lines when justified into a              letter-width text column, which is the point of a flowing layout engine.",
        )
        .unwrap();
    assert_eq!(
        unsafe { prismpdf_flow_text(flow, body, para.as_ptr()) },
        PrismPdfStatus::Ok
    );

    let items: Vec<CString> = ["First point", "Second point", "Third point"]
        .iter()
        .map(|s| CString::new(*s).unwrap())
        .collect();
    let item_ptrs: Vec<*const c_char> = items.iter().map(|s| s.as_ptr()).collect();
    assert_eq!(
        unsafe {
            prismpdf_flow_list(
                flow,
                body,
                item_ptrs.as_ptr(),
                item_ptrs.len(),
                PrismPdfListStyle::Numbered,
            )
        },
        PrismPdfStatus::Ok
    );

    // A table with a repeating header row.
    let columns = [200.0f64, 120.0, 120.0];
    let table = unsafe { prismpdf_table_new(columns.as_ptr(), 3) };
    assert!(!table.is_null());
    unsafe {
        prismpdf_table_set_font(table, resource.as_ptr(), base.as_ptr());
        prismpdf_table_set_size(table, 10.0);
        prismpdf_table_set_leading(table, 12.0);
        prismpdf_table_set_padding(table, 4.0);
        prismpdf_table_set_border(table, 0.5);
        prismpdf_table_set_align(table, PrismPdfAlign::Left);
        prismpdf_table_set_header_row(table, true);
    }
    for row in [
        ["Region", "Revenue", "Growth"],
        ["North", "1,240", "+8%"],
        ["South", "980", "+3%"],
    ] {
        let cells: Vec<CString> = row.iter().map(|s| CString::new(*s).unwrap()).collect();
        let ptrs: Vec<*const c_char> = cells.iter().map(|s| s.as_ptr()).collect();
        assert_eq!(
            unsafe { prismpdf_table_add_row(table, ptrs.as_ptr(), 3) },
            PrismPdfStatus::Ok
        );
    }
    assert_eq!(
        unsafe { prismpdf_flow_table(flow, table) },
        PrismPdfStatus::Ok
    );
    unsafe { prismpdf_table_free(table) };

    // A tagged figure with alt text, and a plain artifact image.
    let rgb = [0x40u8; 8 * 6 * 3];
    let image = unsafe { prismpdf_image_source_from_rgb(8, 6, rgb.as_ptr(), rgb.len()) };
    assert!(!image.is_null());
    let (mut iw, mut ih) = (0u32, 0u32);
    assert_eq!(
        unsafe { prismpdf_image_source_size(image, &mut iw, &mut ih) },
        PrismPdfStatus::Ok
    );
    assert_eq!((iw, ih), (8, 6));
    let alt = CString::new("A grey placeholder chart").unwrap();
    let caption = CString::new("Figure 1 — revenue by region").unwrap();
    assert_eq!(
        unsafe {
            prismpdf_flow_figure_with_caption(
                flow,
                image,
                120.0,
                90.0,
                alt.as_ptr(),
                body,
                caption.as_ptr(),
            )
        },
        PrismPdfStatus::Ok
    );
    assert_eq!(
        unsafe { prismpdf_flow_figure(flow, image, 60.0, 45.0, alt.as_ptr()) },
        PrismPdfStatus::Ok
    );
    assert_eq!(
        unsafe { prismpdf_flow_figure_fit(flow, image, 100.0, alt.as_ptr()) },
        PrismPdfStatus::Ok
    );
    assert_eq!(
        unsafe { prismpdf_flow_figure_fit(flow, image, 100.0, std::ptr::null()) },
        PrismPdfStatus::NullArgument,
        "a tagged figure without alt text is refused, not silently untagged"
    );
    assert_eq!(
        unsafe { prismpdf_flow_image(flow, image, 40.0, 30.0) },
        PrismPdfStatus::Ok
    );
    assert_eq!(
        unsafe { prismpdf_flow_image_fit(flow, image, 80.0) },
        PrismPdfStatus::Ok
    );
    unsafe { prismpdf_image_source_free(image) };

    // A footnote and a formula, then a deliberate page break.
    let note_text = CString::new("Figures are unaudited.").unwrap();
    let note_id = CString::new("fn1").unwrap();
    assert_eq!(
        unsafe { prismpdf_flow_note(flow, body, note_text.as_ptr(), note_id.as_ptr()) },
        PrismPdfStatus::Ok
    );
    let formula = CString::new("E = mc^2").unwrap();
    let actual = CString::new("E equals m c squared").unwrap();
    assert_eq!(
        unsafe { prismpdf_flow_formula(flow, body, formula.as_ptr(), actual.as_ptr()) },
        PrismPdfStatus::Ok
    );

    assert_eq!(
        unsafe { prismpdf_flow_space(flow, 24.0) },
        PrismPdfStatus::Ok
    );
    assert_eq!(
        unsafe { prismpdf_flow_page_break(flow) },
        PrismPdfStatus::Ok
    );

    let mut y = 0.0f64;
    assert_eq!(
        unsafe { prismpdf_flow_cursor_y(flow, &mut y) },
        PrismPdfStatus::Ok
    );
    assert!(y > 0.0);
    let mut pages = 0usize;
    assert_eq!(
        unsafe { prismpdf_flow_page_count(flow, &mut pages) },
        PrismPdfStatus::Ok
    );
    assert!(pages >= 1);

    // Build consumes the flow.
    let mut data: *mut u8 = std::ptr::null_mut();
    let mut len = 0usize;
    assert_eq!(
        unsafe { prismpdf_flow_build(flow, &mut data, &mut len) },
        PrismPdfStatus::Ok
    );
    let pdf = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
    unsafe { prismpdf_bytes_free(data, len) };
    unsafe { prismpdf_text_block_free(body) };
    unsafe { prismpdf_text_block_free(head) };

    let doc = open(&pdf);
    let mut count = 0usize;
    unsafe { prismpdf_document_page_count(doc, &mut count) };
    assert!(
        count >= 2,
        "the explicit page break must produce a 2nd page"
    );

    let mut text: *mut c_char = std::ptr::null_mut();
    unsafe { prismpdf_document_text(doc, &mut text) };
    let extracted = take_string(text);
    assert!(extracted.contains("Quarterly Report"));
    assert!(extracted.contains("Second point"));
    assert!(extracted.contains("Revenue"));

    let mut outline: *mut PrismPdfOutlineList = std::ptr::null_mut();
    unsafe { prismpdf_document_outline(doc, &mut outline) };
    let mut n = 0usize;
    unsafe { prismpdf_outline_list_len(outline, &mut n) };
    assert_eq!(n, 1);
    unsafe { prismpdf_outline_list_free(outline) };
    unsafe { prismpdf_document_free(doc) };
}

#[test]
fn a_flow_composes_into_a_builder_for_post_processing() {
    let (flow, _font) = letter_flow();
    let resource = CString::new("F1").unwrap();
    let base = CString::new("Helvetica").unwrap();
    let block = unsafe {
        prismpdf_text_block_new(
            resource.as_ptr(),
            base.as_ptr(),
            12.0,
            15.0,
            PrismPdfAlign::Left,
        )
    };
    let text = CString::new("Body copy.").unwrap();
    unsafe { prismpdf_flow_text(flow, block, text.as_ptr()) };

    // into_builder consumes the flow and hands back a builder.
    let mut builder: *mut PrismPdfBuilder = std::ptr::null_mut();
    assert_eq!(
        unsafe { prismpdf_flow_into_builder(flow, &mut builder) },
        PrismPdfStatus::Ok
    );
    assert!(!builder.is_null());

    // …which still accepts every builder operation.
    let att = CString::new("appendix.txt").unwrap();
    let mime = CString::new("text/plain").unwrap();
    let rel = CString::new("Supplement").unwrap();
    assert_eq!(
        unsafe {
            prismpdf_builder_attach_file(
                builder,
                att.as_ptr(),
                mime.as_ptr(),
                rel.as_ptr(),
                std::ptr::null(),
                b"appendix".as_ptr(),
                8,
            )
        },
        PrismPdfStatus::Ok
    );

    let mut data: *mut u8 = std::ptr::null_mut();
    let mut len = 0usize;
    assert_eq!(
        unsafe { prismpdf_builder_build(builder, &mut data, &mut len) },
        PrismPdfStatus::Ok
    );
    let pdf = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
    unsafe { prismpdf_bytes_free(data, len) };
    unsafe { prismpdf_builder_free(builder) };
    unsafe { prismpdf_text_block_free(block) };

    let doc = open(&pdf);
    let mut atts: *mut PrismPdfAttachmentList = std::ptr::null_mut();
    unsafe { prismpdf_document_attachments(doc, &mut atts) };
    let mut n = 0usize;
    unsafe { prismpdf_attachment_list_len(atts, &mut n) };
    assert_eq!(n, 1);
    unsafe { prismpdf_attachment_list_free(atts) };
    unsafe { prismpdf_document_free(doc) };
}

#[test]
fn text_measurement_and_wrapping_work_from_c() {
    let resource = CString::new("F1").unwrap();
    let base = CString::new("Helvetica").unwrap();
    let block = unsafe {
        prismpdf_text_block_new(
            resource.as_ptr(),
            base.as_ptr(),
            12.0,
            15.0,
            PrismPdfAlign::Left,
        )
    };

    let text = CString::new("Hello world").unwrap();
    let mut width = 0.0f64;
    assert_eq!(
        unsafe { prismpdf_measure_text(block, text.as_ptr(), &mut width) },
        PrismPdfStatus::Ok
    );
    assert!(width > 0.0);

    let long = CString::new(
        "The quick brown fox jumps over the lazy dog and keeps on running well past the margin",
    )
    .unwrap();
    let mut lines: *mut PrismPdfStringList = std::ptr::null_mut();
    assert_eq!(
        unsafe { prismpdf_wrap_text(block, long.as_ptr(), 120.0, &mut lines) },
        PrismPdfStatus::Ok
    );
    let mut n = 0usize;
    unsafe { prismpdf_string_list_len(lines, &mut n) };
    assert!(n > 1, "narrow column must wrap to several lines");
    let mut first: *mut c_char = std::ptr::null_mut();
    assert_eq!(
        unsafe { prismpdf_string_list_get(lines, 0, &mut first) },
        PrismPdfStatus::Ok
    );
    assert!(!take_string(first).is_empty());
    assert_eq!(
        unsafe { prismpdf_string_list_get(lines, 999, &mut first) },
        PrismPdfStatus::NotFound
    );
    unsafe { prismpdf_string_list_free(lines) };
    unsafe { prismpdf_text_block_free(block) };

    // A non-Standard-14 base font has no built-in metrics: NotFound, not an error.
    let unknown = CString::new("NoSuchFont").unwrap();
    let odd = unsafe {
        prismpdf_text_block_new(
            resource.as_ptr(),
            unknown.as_ptr(),
            12.0,
            15.0,
            PrismPdfAlign::Left,
        )
    };
    assert_eq!(
        unsafe { prismpdf_measure_text(odd, text.as_ptr(), &mut width) },
        PrismPdfStatus::NotFound
    );
    unsafe { prismpdf_text_block_free(odd) };
}

#[test]
fn the_remaining_document_entry_points_work() {
    let pdf = sample_pdf();

    // Explicit anti-DoS limits, and the default-filling behaviour of a zero field.
    let limits = PrismPdfLimits {
        max_depth: 8,
        max_objstm_objects: 0,
        max_objects: 0,
    };
    let mut doc: *mut PrismPdfDocument = std::ptr::null_mut();
    assert_eq!(
        unsafe { prismpdf_document_open_with_limits(pdf.as_ptr(), pdf.len(), &limits, &mut doc) },
        PrismPdfStatus::Ok
    );
    unsafe { prismpdf_document_free(doc) };

    // A null limits pointer means "defaults".
    assert_eq!(
        unsafe {
            prismpdf_document_open_with_limits(pdf.as_ptr(), pdf.len(), std::ptr::null(), &mut doc)
        },
        PrismPdfStatus::Ok
    );

    let (mut major, mut minor) = (0u8, 0u8);
    assert_eq!(
        unsafe { prismpdf_document_min_version(doc, &mut major, &mut minor) },
        PrismPdfStatus::Ok
    );
    assert_eq!(major, 1);

    for (m, n, header) in [(1u8, 7u8, &b"%PDF-1.7"[..]), (2, 0, &b"%PDF-2.0"[..])] {
        let mut data: *mut u8 = std::ptr::null_mut();
        let mut len = 0usize;
        assert_eq!(
            unsafe { prismpdf_document_save_as(doc, m, n, &mut data, &mut len) },
            PrismPdfStatus::Ok
        );
        let out = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
        unsafe { prismpdf_bytes_free(data, len) };
        assert!(out.starts_with(header));
    }

    let mut data: *mut u8 = std::ptr::null_mut();
    let mut len = 0usize;
    assert_eq!(
        unsafe { prismpdf_document_save_packed(doc, &mut data, &mut len) },
        PrismPdfStatus::Ok
    );
    let packed = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
    unsafe { prismpdf_bytes_free(data, len) };
    let repacked = open(&packed);
    let mut pages = 0usize;
    unsafe { prismpdf_document_page_count(repacked, &mut pages) };
    assert_eq!(pages, 1);
    unsafe { prismpdf_document_free(repacked) };

    // A 1.7 document declares no structure namespaces, and carries no DSS.
    let mut list: *mut PrismPdfStringList = std::ptr::null_mut();
    assert_eq!(
        unsafe { prismpdf_document_structure_namespaces(doc, &mut list) },
        PrismPdfStatus::Ok
    );
    let mut n = 99usize;
    unsafe { prismpdf_string_list_len(list, &mut n) };
    assert_eq!(n, 0);
    unsafe { prismpdf_string_list_free(list) };

    assert_eq!(
        unsafe { prismpdf_document_signature_vri_keys(doc, &mut list) },
        PrismPdfStatus::Ok
    );
    unsafe { prismpdf_string_list_len(list, &mut n) };
    assert_eq!(n, 0);
    unsafe { prismpdf_string_list_free(list) };
    unsafe { prismpdf_document_free(doc) };

    // A plain document is not certificate-encrypted, so the private-key path refuses it.
    let mut rejected: *mut PrismPdfDocument = std::ptr::null_mut();
    assert_eq!(
        unsafe {
            prismpdf_document_open_with_private_key(
                pdf.as_ptr(),
                pdf.len(),
                TEST_CERT.as_ptr(),
                TEST_CERT.len(),
                TEST_KEY.as_ptr(),
                TEST_KEY.len(),
                &mut rejected,
            )
        },
        PrismPdfStatus::Ok,
        "an unencrypted document opens regardless of the key supplied"
    );
    unsafe { prismpdf_document_free(rejected) };
}

#[test]
fn layout_entry_points_reject_null_arguments() {
    let mut width = 0.0f64;
    let mut list: *mut PrismPdfStringList = std::ptr::null_mut();
    let mut data: *mut u8 = std::ptr::null_mut();
    let mut len = 0usize;

    assert!(
        unsafe {
            prismpdf_text_block_new(
                std::ptr::null(),
                std::ptr::null(),
                1.0,
                1.0,
                PrismPdfAlign::Left,
            )
        }
        .is_null()
    );
    assert!(unsafe { prismpdf_table_new(std::ptr::null(), 0) }.is_null());
    assert!(unsafe { prismpdf_image_source_from_jpeg(std::ptr::null(), 0) }.is_null());
    // A length mismatch is refused rather than producing a corrupt image.
    assert!(unsafe { prismpdf_image_source_from_rgb(4, 4, b"short".as_ptr(), 5) }.is_null());
    assert!(
        unsafe {
            prismpdf_flow_new(
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                0,
            )
        }
        .is_null()
    );

    assert_eq!(
        unsafe { prismpdf_measure_text(std::ptr::null(), std::ptr::null(), &mut width) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_wrap_text(std::ptr::null(), std::ptr::null(), 10.0, &mut list) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_flow_text(std::ptr::null_mut(), std::ptr::null(), std::ptr::null()) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_flow_build(std::ptr::null_mut(), &mut data, &mut len) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_flow_into_builder(std::ptr::null_mut(), std::ptr::null_mut()) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_flow_page_break(std::ptr::null_mut()) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_table_set_size(std::ptr::null_mut(), 10.0) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_table_add_row(std::ptr::null_mut(), std::ptr::null(), 0) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe {
            prismpdf_image_source_size(std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut())
        },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe {
            prismpdf_document_open_with_limits(
                std::ptr::null(),
                0,
                std::ptr::null(),
                std::ptr::null_mut(),
            )
        },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe {
            prismpdf_document_min_version(
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_string_list_len(std::ptr::null(), &mut len) },
        PrismPdfStatus::NullArgument
    );

    unsafe { prismpdf_text_block_free(std::ptr::null_mut()) };
    unsafe { prismpdf_table_free(std::ptr::null_mut()) };
    unsafe { prismpdf_image_source_free(std::ptr::null_mut()) };
    unsafe { prismpdf_flow_free(std::ptr::null_mut()) };
    unsafe { prismpdf_string_list_free(std::ptr::null_mut()) };
}

#[test]
fn pdfa_conformance_queries_match_the_standard() {
    assert_eq!(prismpdf_pdfa_part(PrismPdfPdfAConformance::A1b), 1);
    assert_eq!(prismpdf_pdfa_part(PrismPdfPdfAConformance::A2u), 2);
    assert_eq!(prismpdf_pdfa_part(PrismPdfPdfAConformance::A3a), 3);
    assert_eq!(prismpdf_pdfa_part(PrismPdfPdfAConformance::A4f), 4);

    // Only part 3 and 4f may carry embedded files (§6.8).
    assert!(!prismpdf_pdfa_allows_attachments(
        PrismPdfPdfAConformance::A1b
    ));
    assert!(!prismpdf_pdfa_allows_attachments(
        PrismPdfPdfAConformance::A2b
    ));
    assert!(prismpdf_pdfa_allows_attachments(
        PrismPdfPdfAConformance::A3b
    ));
    assert!(prismpdf_pdfa_allows_attachments(
        PrismPdfPdfAConformance::A4f
    ));
    assert!(!prismpdf_pdfa_allows_attachments(
        PrismPdfPdfAConformance::A4
    ));

    let mut text: *mut c_char = std::ptr::null_mut();
    assert_eq!(
        unsafe { prismpdf_pdfa_code(PrismPdfPdfAConformance::A2u, &mut text) },
        PrismPdfStatus::Ok
    );
    assert_eq!(take_string(text), "2u");
}

#[test]
fn pdfa_production_succeeds_on_a_font_free_document() {
    let builder = vector_only_builder();
    let meta = full_xmp();

    let mut issue = PrismPdfConformanceIssue::UnembeddedFont;
    assert_eq!(
        unsafe {
            prismpdf_builder_make_pdfa(builder, PrismPdfPdfAConformance::A1b, meta, &mut issue)
        },
        PrismPdfStatus::Ok
    );

    let mut data: *mut u8 = std::ptr::null_mut();
    let mut len = 0usize;
    assert_eq!(
        unsafe { prismpdf_builder_build(builder, &mut data, &mut len) },
        PrismPdfStatus::Ok
    );
    let pdf = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
    unsafe { prismpdf_bytes_free(data, len) };

    // The pass wrote an XMP packet declaring the part and level.
    let doc = open(&pdf);
    let mut text: *mut c_char = std::ptr::null_mut();
    assert_eq!(
        unsafe { prismpdf_document_xmp(doc, &mut text) },
        PrismPdfStatus::Ok
    );
    let xmp = take_string(text);
    assert!(xmp.contains("pdfaid"), "XMP must declare pdfaid: {xmp}");
    assert!(xmp.contains("Archival Report"));
    assert!(xmp.contains("Ada Lovelace") && xmp.contains("Grace Hopper"));
    unsafe { prismpdf_document_free(doc) };

    unsafe { prismpdf_xmp_metadata_free(meta) };
    unsafe { prismpdf_builder_free(builder) };
}

#[test]
fn pdfa_refusals_name_the_rule_that_was_broken() {
    let meta = prismpdf_xmp_metadata_new();

    // A Standard-14 font is not embedded (§6.3.4).
    let builder = prismpdf_builder_new();
    let name = CString::new("F1").unwrap();
    let names = [name.as_ptr()];
    let fonts = [PrismPdfStdFont::Helvetica];
    unsafe {
        prismpdf_builder_add_page(
            builder,
            b"BT /F1 12 Tf (hi) Tj ET".as_ptr(),
            23,
            names.as_ptr(),
            fonts.as_ptr(),
            1,
        )
    };
    let mut issue = PrismPdfConformanceIssue::NotTagged;
    assert_eq!(
        unsafe {
            prismpdf_builder_make_pdfa(builder, PrismPdfPdfAConformance::A2b, meta, &mut issue)
        },
        PrismPdfStatus::Conformance
    );
    assert_eq!(issue, PrismPdfConformanceIssue::UnembeddedFont);
    unsafe { prismpdf_builder_free(builder) };

    // Attachments need part 3 or 4f (§6.8).
    let builder = vector_only_builder();
    let att = CString::new("data.bin").unwrap();
    let mime = CString::new("application/octet-stream").unwrap();
    let rel = CString::new("Unspecified").unwrap();
    unsafe {
        prismpdf_builder_attach_file(
            builder,
            att.as_ptr(),
            mime.as_ptr(),
            rel.as_ptr(),
            std::ptr::null(),
            b"x".as_ptr(),
            1,
        )
    };
    assert_eq!(
        unsafe {
            prismpdf_builder_make_pdfa(builder, PrismPdfPdfAConformance::A2b, meta, &mut issue)
        },
        PrismPdfStatus::Conformance
    );
    assert_eq!(issue, PrismPdfConformanceIssue::AttachmentRequiresPdfA3);
    // …and the same document is fine at A3b, which permits them.
    assert_eq!(
        unsafe {
            prismpdf_builder_make_pdfa(builder, PrismPdfPdfAConformance::A3b, meta, &mut issue)
        },
        PrismPdfStatus::Ok
    );
    unsafe { prismpdf_builder_free(builder) };

    // Level A needs logical structure (§6.9).
    let builder = vector_only_builder();
    assert_eq!(
        unsafe {
            prismpdf_builder_make_pdfa(builder, PrismPdfPdfAConformance::A1a, meta, &mut issue)
        },
        PrismPdfStatus::Conformance
    );
    assert_eq!(issue, PrismPdfConformanceIssue::LevelARequiresTagging);

    // A null out_issue is allowed — the status alone still tells you it failed.
    assert_eq!(
        unsafe {
            prismpdf_builder_make_pdfa(
                builder,
                PrismPdfPdfAConformance::A1a,
                meta,
                std::ptr::null_mut(),
            )
        },
        PrismPdfStatus::Conformance
    );
    unsafe { prismpdf_builder_free(builder) };

    unsafe { prismpdf_xmp_metadata_free(meta) };
}

#[test]
fn pdfua_refuses_an_untagged_document() {
    let builder = vector_only_builder();
    let meta = prismpdf_xmp_metadata_new();
    let lang = CString::new("en-GB").unwrap();

    let mut issue = PrismPdfConformanceIssue::UnembeddedFont;
    assert_eq!(
        unsafe { prismpdf_builder_make_pdfua(builder, meta, lang.as_ptr(), &mut issue) },
        PrismPdfStatus::Conformance
    );
    assert_eq!(issue, PrismPdfConformanceIssue::NotTagged);

    assert_eq!(
        unsafe { prismpdf_builder_make_pdfua2(builder, meta, lang.as_ptr(), &mut issue) },
        PrismPdfStatus::Conformance
    );
    assert_eq!(issue, PrismPdfConformanceIssue::NotTagged);

    unsafe { prismpdf_xmp_metadata_free(meta) };
    unsafe { prismpdf_builder_free(builder) };
}

#[test]
fn output_intents_are_settable_directly_and_through_pdfa() {
    // A minimal well-formed ICC header is enough for the writer; veraPDF in CI checks the real
    // profiles, this checks the plumbing.
    let icc = [0u8; 128];
    let identifier = CString::new("Custom CMYK").unwrap();

    let builder = vector_only_builder();
    assert_eq!(
        unsafe {
            prismpdf_builder_set_output_intent(
                builder,
                icc.as_ptr(),
                icc.len(),
                4,
                identifier.as_ptr(),
            )
        },
        PrismPdfStatus::Ok
    );
    unsafe { prismpdf_builder_free(builder) };

    let builder = vector_only_builder();
    let meta = full_xmp();
    let mut issue = PrismPdfConformanceIssue::NotTagged;
    assert_eq!(
        unsafe {
            prismpdf_builder_make_pdfa_with_output_intent(
                builder,
                PrismPdfPdfAConformance::A2b,
                meta,
                icc.as_ptr(),
                icc.len(),
                4,
                identifier.as_ptr(),
                &mut issue,
            )
        },
        PrismPdfStatus::Ok
    );
    let mut data: *mut u8 = std::ptr::null_mut();
    let mut len = 0usize;
    unsafe { prismpdf_builder_build(builder, &mut data, &mut len) };
    assert!(len > 0);
    unsafe { prismpdf_bytes_free(data, len) };
    unsafe { prismpdf_xmp_metadata_free(meta) };
    unsafe { prismpdf_builder_free(builder) };
}

#[test]
fn conformance_entry_points_reject_null_arguments() {
    let mut issue = PrismPdfConformanceIssue::NotTagged;
    let meta = prismpdf_xmp_metadata_new();
    let builder = prismpdf_builder_new();
    let lang = CString::new("en").unwrap();
    let bad = [0x80u8, 0x00];
    let bad_ptr = bad.as_ptr().cast::<c_char>();

    assert_eq!(
        unsafe {
            prismpdf_builder_make_pdfa(
                std::ptr::null_mut(),
                PrismPdfPdfAConformance::A1b,
                meta,
                &mut issue,
            )
        },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe {
            prismpdf_builder_make_pdfa(
                builder,
                PrismPdfPdfAConformance::A1b,
                std::ptr::null(),
                &mut issue,
            )
        },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_builder_make_pdfua(builder, meta, std::ptr::null(), &mut issue) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_builder_make_pdfua2(builder, meta, bad_ptr, &mut issue) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe {
            prismpdf_builder_make_pdfa_with_output_intent(
                builder,
                PrismPdfPdfAConformance::A2b,
                meta,
                std::ptr::null(),
                0,
                3,
                std::ptr::null(),
                &mut issue,
            )
        },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe {
            prismpdf_builder_set_output_intent(builder, std::ptr::null(), 0, 3, lang.as_ptr())
        },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_xmp_metadata_set_title(std::ptr::null_mut(), lang.as_ptr()) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_xmp_metadata_add_author(meta, bad_ptr) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_pdfa_code(PrismPdfPdfAConformance::A1b, std::ptr::null_mut()) },
        PrismPdfStatus::NullArgument
    );

    unsafe { prismpdf_xmp_metadata_free(std::ptr::null_mut()) };
    unsafe { prismpdf_xmp_metadata_free(meta) };
    unsafe { prismpdf_builder_free(builder) };
}

#[test]
fn authoring_entry_points_reject_null_arguments() {
    let mut data: *mut u8 = std::ptr::null_mut();
    let mut len = 0usize;
    let rect = [0.0f64; 4];

    assert_eq!(
        unsafe { prismpdf_content_save(std::ptr::null_mut()) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_content_set_font(std::ptr::null_mut(), std::ptr::null(), 12.0) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_content_bytes(std::ptr::null(), std::ptr::null_mut(), &mut len) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_builder_build(std::ptr::null(), &mut data, &mut len) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_builder_build_for(std::ptr::null(), 1, 7, &mut data, &mut len) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_builder_set_title(std::ptr::null_mut(), std::ptr::null()) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_builder_set_media_box(std::ptr::null_mut(), std::ptr::null()) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_builder_clear_info(std::ptr::null_mut()) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe {
            prismpdf_builder_add_note(std::ptr::null_mut(), 0, rect.as_ptr(), std::ptr::null())
        },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe {
            prismpdf_builder_add_link_page(
                std::ptr::null_mut(),
                0,
                rect.as_ptr(),
                0,
                std::ptr::null(),
            )
        },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe {
            prismpdf_builder_attach_file(
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                0,
            )
        },
        PrismPdfStatus::NullArgument
    );

    unsafe { prismpdf_content_free(std::ptr::null_mut()) };
    unsafe { prismpdf_builder_free(std::ptr::null_mut()) };
}

#[test]
fn permissions_compose_from_restricted() {
    let restricted = prismpdf_permissions_restricted();
    let all = prismpdf_permissions_all();
    assert_ne!(restricted, all);

    // Granting is additive and idempotent.
    let printable = prismpdf_permissions_allow_print(restricted);
    assert_ne!(printable, restricted);
    assert_eq!(prismpdf_permissions_allow_print(printable), printable);

    // Every grant widens the set, and the chain composes.
    let mut bits = restricted;
    for grant in [
        prismpdf_permissions_allow_print as extern "C" fn(i32) -> i32,
        prismpdf_permissions_allow_modify,
        prismpdf_permissions_allow_copy,
        prismpdf_permissions_allow_annotate,
        prismpdf_permissions_allow_fill_forms,
        prismpdf_permissions_allow_accessibility,
        prismpdf_permissions_allow_assemble,
        prismpdf_permissions_allow_print_high_res,
    ] {
        let widened = grant(bits);
        assert_eq!(widened | bits, widened, "a grant must only set bits");
        bits = widened;
    }
    // Granting every operation does *not* reach `ALL`: `ALL` is -1, which also sets reserved
    // bits 1-2 that §7.6.3.2 requires to be zero. Composing from RESTRICTED yields -4, the
    // spec-shaped word. Both are accepted on write; this pins the composed value's shape.
    assert_eq!(bits, -4);
    assert_eq!(bits | all, all, "the composed set is a subset of ALL");
}

#[test]
fn encryption_honours_permissions_and_algorithms() {
    let doc = open(&sample_pdf());
    let user = b"pw";
    let perms = prismpdf_permissions_allow_print(prismpdf_permissions_restricted());

    for algorithm in 0..=3u32 {
        let mut data: *mut u8 = std::ptr::null_mut();
        let mut out_len = 0usize;
        assert_eq!(
            unsafe {
                prismpdf_document_save_encrypted_with(
                    doc,
                    user.as_ptr(),
                    user.len(),
                    std::ptr::null(),
                    0,
                    perms,
                    true,
                    algorithm,
                    &mut data,
                    &mut out_len,
                )
            },
            PrismPdfStatus::Ok,
            "algorithm {algorithm} should be accepted"
        );
        assert!(out_len > 0);

        // It really is encrypted, and the password opens it.
        let bytes = unsafe { std::slice::from_raw_parts(data, out_len) }.to_vec();
        unsafe { prismpdf_bytes_free(data, out_len) };
        let mut reopened: *mut PrismPdfDocument = std::ptr::null_mut();
        assert_eq!(
            unsafe {
                prismpdf_document_open_with_password(
                    bytes.as_ptr(),
                    bytes.len(),
                    user.as_ptr(),
                    user.len(),
                    &mut reopened,
                )
            },
            PrismPdfStatus::Ok
        );
        unsafe { prismpdf_document_free(reopened) };
    }

    // An unknown algorithm is rejected rather than silently defaulted.
    let mut data: *mut u8 = std::ptr::null_mut();
    let mut out_len = 0usize;
    assert_eq!(
        unsafe {
            prismpdf_document_save_encrypted_with(
                doc,
                user.as_ptr(),
                user.len(),
                std::ptr::null(),
                0,
                perms,
                true,
                99,
                &mut data,
                &mut out_len,
            )
        },
        PrismPdfStatus::NullArgument
    );
    unsafe { prismpdf_document_free(doc) };
}

#[test]
fn pdf_mac_round_trips_and_reports_absence() {
    let doc = open(&sample_pdf());

    // A plain document carries no MAC at all — NotFound, not a verification failure.
    let mut valid = true;
    assert_eq!(
        unsafe { prismpdf_document_verify_pdf_mac(doc, b"pw".as_ptr(), 2, &mut valid) },
        PrismPdfStatus::NotFound
    );

    let mut data: *mut u8 = std::ptr::null_mut();
    let mut out_len = 0usize;
    assert_eq!(
        unsafe {
            prismpdf_document_save_encrypted_with_mac(
                doc,
                b"pw".as_ptr(),
                2,
                std::ptr::null(),
                0,
                prismpdf_permissions_all(),
                true,
                2, // AES-256; a MAC needs a V5 handler
                &mut data,
                &mut out_len,
            )
        },
        PrismPdfStatus::Ok
    );
    let bytes = unsafe { std::slice::from_raw_parts(data, out_len) }.to_vec();
    unsafe { prismpdf_bytes_free(data, out_len) };
    unsafe { prismpdf_document_free(doc) };

    let mut protected: *mut PrismPdfDocument = std::ptr::null_mut();
    unsafe {
        prismpdf_document_open_with_password(
            bytes.as_ptr(),
            bytes.len(),
            b"pw".as_ptr(),
            2,
            &mut protected,
        )
    };
    let mut ok = false;
    assert_eq!(
        unsafe { prismpdf_document_verify_pdf_mac(protected, b"pw".as_ptr(), 2, &mut ok) },
        PrismPdfStatus::Ok
    );
    assert!(ok, "a freshly written MAC must verify");
    unsafe { prismpdf_document_free(protected) };
}

#[test]
fn signing_and_verification_round_trip() {
    let doc = open(&sample_pdf());

    // Settings handle: the first mutable handle in the ABI.
    let settings = prismpdf_sign_settings_new();
    assert!(!settings.is_null());
    let name = CString::new("Ada Lovelace").unwrap();
    let reason = CString::new("Approval").unwrap();
    let location = CString::new("London").unwrap();
    let contact = CString::new("ada@example.org").unwrap();
    assert_eq!(
        unsafe { prismpdf_sign_settings_set_name(settings, name.as_ptr()) },
        PrismPdfStatus::Ok
    );
    unsafe { prismpdf_sign_settings_set_reason(settings, reason.as_ptr()) };
    unsafe { prismpdf_sign_settings_set_location(settings, location.as_ptr()) };
    unsafe { prismpdf_sign_settings_set_contact_info(settings, contact.as_ptr()) };
    // Pin the clock so the signature is deterministic.
    unsafe { prismpdf_sign_settings_set_signing_time(settings, 1_755_000_000) };
    unsafe { prismpdf_sign_settings_set_pades(settings, false) };
    let rect = [10.0f32, 10.0, 200.0, 60.0];
    let caption = CString::new("Signed").unwrap();
    assert_eq!(
        unsafe {
            prismpdf_sign_settings_set_appearance(settings, 0, rect.as_ptr(), caption.as_ptr())
        },
        PrismPdfStatus::Ok
    );

    let mut data: *mut u8 = std::ptr::null_mut();
    let mut out_len = 0usize;
    assert_eq!(
        unsafe {
            prismpdf_document_sign_with(
                doc,
                TEST_CERT.as_ptr(),
                TEST_CERT.len(),
                TEST_KEY.as_ptr(),
                TEST_KEY.len(),
                settings,
                &mut data,
                &mut out_len,
            )
        },
        PrismPdfStatus::Ok
    );
    let signed = unsafe { std::slice::from_raw_parts(data, out_len) }.to_vec();
    unsafe { prismpdf_bytes_free(data, out_len) };
    unsafe { prismpdf_sign_settings_free(settings) };
    unsafe { prismpdf_document_free(doc) };

    // Verify integrity without trust anchors.
    let signed_doc = open(&signed);
    let mut list: *mut PrismPdfSignatureList = std::ptr::null_mut();
    assert_eq!(
        unsafe { prismpdf_document_verify_signatures(signed_doc, &mut list) },
        PrismPdfStatus::Ok
    );
    let mut len = 0usize;
    unsafe { prismpdf_signature_list_len(list, &mut len) };
    assert_eq!(len, 1);

    let mut sig: *const PrismPdfSignature = std::ptr::null();
    unsafe { prismpdf_signature_list_get(list, 0, &mut sig) };

    let mut valid = false;
    assert_eq!(
        unsafe { prismpdf_signature_valid(sig, &mut valid) },
        PrismPdfStatus::Ok
    );
    assert!(valid, "a freshly written signature must verify");

    let mut text: *mut c_char = std::ptr::null_mut();
    assert_eq!(
        unsafe { prismpdf_signature_signer(sig, &mut text) },
        PrismPdfStatus::Ok
    );
    assert!(!take_string(text).is_empty());

    let mut covered = 0usize;
    unsafe { prismpdf_signature_covered_bytes(sig, &mut covered) };
    assert!(covered > 0 && covered <= signed.len());

    let mut when = 0i64;
    assert_eq!(
        unsafe { prismpdf_signature_signing_time(sig, &mut when) },
        PrismPdfStatus::Ok
    );
    assert_eq!(when, 1_755_000_000);

    let mut pades = true;
    unsafe { prismpdf_signature_pades(sig, &mut pades) };
    assert!(!pades);

    // Trust and revocation were never evaluated on this path: NotFound, not false.
    let mut trusted = true;
    assert_eq!(
        unsafe { prismpdf_signature_trusted(sig, &mut trusted) },
        PrismPdfStatus::NotFound
    );
    let mut revocation = PrismPdfRevocation::Good;
    assert_eq!(
        unsafe { prismpdf_signature_revocation(sig, &mut revocation) },
        PrismPdfStatus::NotFound
    );
    let mut ts = 0i64;
    assert_eq!(
        unsafe { prismpdf_signature_timestamp_time(sig, &mut ts) },
        PrismPdfStatus::NotFound
    );

    assert_eq!(
        unsafe { prismpdf_signature_list_get(list, 99, &mut sig) },
        PrismPdfStatus::NotFound
    );
    unsafe { prismpdf_signature_list_free(list) };

    // With the signer's own certificate as a root, trust resolves.
    let roots = [TEST_CERT.as_ptr()];
    let root_lens = [TEST_CERT.len()];
    let mut trusted_list: *mut PrismPdfSignatureList = std::ptr::null_mut();
    assert_eq!(
        unsafe {
            prismpdf_document_verify_signatures_with(
                signed_doc,
                roots.as_ptr(),
                root_lens.as_ptr(),
                1,
                &mut trusted_list,
            )
        },
        PrismPdfStatus::Ok
    );
    let mut sig2: *const PrismPdfSignature = std::ptr::null();
    unsafe { prismpdf_signature_list_get(trusted_list, 0, &mut sig2) };
    let mut is_trusted = true;
    // The point of the roots argument is that trust becomes *evaluated* — Ok rather than
    // NotFound. The verdict itself is the chain builder's business: this throwaway
    // self-signed leaf carries no CA basic constraint, so it is not a usable anchor and the
    // answer is a definite `false`. Surfacing that distinction is exactly the ABI's job.
    assert_eq!(
        unsafe { prismpdf_signature_trusted(sig2, &mut is_trusted) },
        PrismPdfStatus::Ok
    );
    assert!(!is_trusted);
    unsafe { prismpdf_signature_list_free(trusted_list) };

    // The LTV path runs and reports a summary (Incomplete without DSS material).
    let mut ltv_list: *mut PrismPdfSignatureList = std::ptr::null_mut();
    assert_eq!(
        unsafe {
            prismpdf_document_verify_signatures_ltv(
                signed_doc,
                roots.as_ptr(),
                root_lens.as_ptr(),
                1,
                &mut ltv_list,
            )
        },
        PrismPdfStatus::Ok
    );
    let mut sig3: *const PrismPdfSignature = std::ptr::null();
    unsafe { prismpdf_signature_list_get(ltv_list, 0, &mut sig3) };
    let mut summary = PrismPdfRevocation::Revoked;
    // Revocation is only summarised once a chain is actually built. Here the anchor is
    // unusable and the document carries no DSS material, so there is nothing to summarise —
    // NotFound, which is the honest answer rather than a misleading `Good`. Exercising the
    // Good/Revoked/Incomplete verdicts needs a real CA-issued chain plus DSS material, which
    // this repo has no fixture for.
    assert_eq!(
        unsafe { prismpdf_signature_revocation(sig3, &mut summary) },
        PrismPdfStatus::NotFound
    );
    unsafe { prismpdf_signature_list_free(ltv_list) };
    unsafe { prismpdf_document_free(signed_doc) };
}

#[test]
fn plain_sign_and_timestamp_produce_output() {
    let doc = open(&sample_pdf());

    let mut data: *mut u8 = std::ptr::null_mut();
    let mut out_len = 0usize;
    assert_eq!(
        unsafe {
            prismpdf_document_sign(
                doc,
                TEST_CERT.as_ptr(),
                TEST_CERT.len(),
                TEST_KEY.as_ptr(),
                TEST_KEY.len(),
                &mut data,
                &mut out_len,
            )
        },
        PrismPdfStatus::Ok
    );
    assert!(out_len > 0);
    unsafe { prismpdf_bytes_free(data, out_len) };

    assert_eq!(
        unsafe {
            prismpdf_document_timestamp(
                doc,
                TEST_CERT.as_ptr(),
                TEST_CERT.len(),
                TEST_KEY.as_ptr(),
                TEST_KEY.len(),
                1_755_000_000,
                true,
                &mut data,
                &mut out_len,
            )
        },
        PrismPdfStatus::Ok
    );
    assert!(out_len > 0);
    unsafe { prismpdf_bytes_free(data, out_len) };
    unsafe { prismpdf_document_free(doc) };
}

#[test]
fn public_key_encryption_accepts_recipients() {
    let doc = open(&sample_pdf());
    let certs = [TEST_CERT.as_ptr()];
    let cert_lens = [TEST_CERT.len()];
    let mut data: *mut u8 = std::ptr::null_mut();
    let mut out_len = 0usize;
    assert_eq!(
        unsafe {
            prismpdf_document_save_encrypted_public_key(
                doc,
                certs.as_ptr(),
                cert_lens.as_ptr(),
                1,
                prismpdf_permissions_allow_print(prismpdf_permissions_restricted()),
                true,
                2,
                &mut data,
                &mut out_len,
            )
        },
        PrismPdfStatus::Ok
    );
    assert!(out_len > 0);
    unsafe { prismpdf_bytes_free(data, out_len) };

    // A null entry inside the recipient array is rejected, not dereferenced.
    let bad_certs = [std::ptr::null::<u8>()];
    assert_eq!(
        unsafe {
            prismpdf_document_save_encrypted_public_key(
                doc,
                bad_certs.as_ptr(),
                cert_lens.as_ptr(),
                1,
                prismpdf_permissions_all(),
                true,
                2,
                &mut data,
                &mut out_len,
            )
        },
        PrismPdfStatus::NullArgument
    );
    unsafe { prismpdf_document_free(doc) };
}

#[test]
fn a_timestamped_signature_reports_its_token_time() {
    let doc = open(&sample_pdf());
    let settings = prismpdf_sign_settings_new();
    unsafe { prismpdf_sign_settings_set_signing_time(settings, 1_755_000_000) };
    assert_eq!(
        unsafe {
            prismpdf_sign_settings_set_timestamp(
                settings,
                TEST_CERT.as_ptr(),
                TEST_CERT.len(),
                TEST_KEY.as_ptr(),
                TEST_KEY.len(),
                1_755_000_500,
                42,
            )
        },
        PrismPdfStatus::Ok
    );

    let mut data: *mut u8 = std::ptr::null_mut();
    let mut out_len = 0usize;
    assert_eq!(
        unsafe {
            prismpdf_document_sign_with(
                doc,
                TEST_CERT.as_ptr(),
                TEST_CERT.len(),
                TEST_KEY.as_ptr(),
                TEST_KEY.len(),
                settings,
                &mut data,
                &mut out_len,
            )
        },
        PrismPdfStatus::Ok
    );
    let signed = unsafe { std::slice::from_raw_parts(data, out_len) }.to_vec();
    unsafe { prismpdf_bytes_free(data, out_len) };
    unsafe { prismpdf_sign_settings_free(settings) };
    unsafe { prismpdf_document_free(doc) };

    let signed_doc = open(&signed);
    let mut list: *mut PrismPdfSignatureList = std::ptr::null_mut();
    unsafe { prismpdf_document_verify_signatures(signed_doc, &mut list) };
    let mut sig: *const PrismPdfSignature = std::ptr::null();
    unsafe { prismpdf_signature_list_get(list, 0, &mut sig) };

    let mut ts = 0i64;
    assert_eq!(
        unsafe { prismpdf_signature_timestamp_time(sig, &mut ts) },
        PrismPdfStatus::Ok
    );
    assert_eq!(ts, 1_755_000_500);

    unsafe { prismpdf_signature_list_free(list) };
    unsafe { prismpdf_document_free(signed_doc) };
}

#[test]
fn signing_an_encrypted_document_refreshes_its_mac() {
    // Build an AES-256 document carrying a MAC, then sign it through the MAC-aware path.
    let plain = open(&sample_pdf());
    let mut data: *mut u8 = std::ptr::null_mut();
    let mut out_len = 0usize;
    unsafe {
        prismpdf_document_save_encrypted_with_mac(
            plain,
            b"pw".as_ptr(),
            2,
            std::ptr::null(),
            0,
            prismpdf_permissions_all(),
            true,
            2,
            &mut data,
            &mut out_len,
        )
    };
    let encrypted = unsafe { std::slice::from_raw_parts(data, out_len) }.to_vec();
    unsafe { prismpdf_bytes_free(data, out_len) };
    unsafe { prismpdf_document_free(plain) };

    let mut doc: *mut PrismPdfDocument = std::ptr::null_mut();
    unsafe {
        prismpdf_document_open_with_password(
            encrypted.as_ptr(),
            encrypted.len(),
            b"pw".as_ptr(),
            2,
            &mut doc,
        )
    };

    let settings = prismpdf_sign_settings_new();
    unsafe { prismpdf_sign_settings_set_signing_time(settings, 1_755_000_000) };
    assert_eq!(
        unsafe {
            prismpdf_document_sign_with_mac(
                doc,
                TEST_CERT.as_ptr(),
                TEST_CERT.len(),
                TEST_KEY.as_ptr(),
                TEST_KEY.len(),
                settings,
                b"pw".as_ptr(),
                2,
                &mut data,
                &mut out_len,
            )
        },
        PrismPdfStatus::Ok
    );
    let signed = unsafe { std::slice::from_raw_parts(data, out_len) }.to_vec();
    unsafe { prismpdf_bytes_free(data, out_len) };
    unsafe { prismpdf_sign_settings_free(settings) };
    unsafe { prismpdf_document_free(doc) };

    // The MAC still covers the file after the signature revision was appended.
    let mut resigned: *mut PrismPdfDocument = std::ptr::null_mut();
    assert_eq!(
        unsafe {
            prismpdf_document_open_with_password(
                signed.as_ptr(),
                signed.len(),
                b"pw".as_ptr(),
                2,
                &mut resigned,
            )
        },
        PrismPdfStatus::Ok
    );
    let mut ok = false;
    assert_eq!(
        unsafe { prismpdf_document_verify_pdf_mac(resigned, b"pw".as_ptr(), 2, &mut ok) },
        PrismPdfStatus::Ok
    );
    assert!(ok, "the MAC must be refreshed by the signing revision");
    unsafe { prismpdf_document_free(resigned) };
}

#[test]
fn trust_entry_points_reject_null_arguments() {
    let mut data: *mut u8 = std::ptr::null_mut();
    let mut len = 0usize;
    let mut list: *mut PrismPdfSignatureList = std::ptr::null_mut();
    let mut text: *mut c_char = std::ptr::null_mut();
    let mut flag = false;

    assert_eq!(
        unsafe {
            prismpdf_document_sign(
                std::ptr::null(),
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                &mut data,
                &mut len,
            )
        },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_document_verify_signatures(std::ptr::null(), &mut list) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_signature_valid(std::ptr::null(), &mut flag) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_signature_signer(std::ptr::null(), &mut text) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_signature_covered_bytes(std::ptr::null(), &mut len) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_sign_settings_set_name(std::ptr::null_mut(), std::ptr::null()) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_sign_settings_set_pades(std::ptr::null_mut(), true) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe {
            prismpdf_sign_settings_set_appearance(
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
                std::ptr::null(),
            )
        },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe {
            prismpdf_document_verify_pdf_mac(std::ptr::null(), std::ptr::null(), 0, &mut flag)
        },
        PrismPdfStatus::NullArgument
    );
    // A public-key save needs at least one recipient.
    assert_eq!(
        unsafe {
            prismpdf_document_save_encrypted_public_key(
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                0,
                true,
                2,
                &mut data,
                &mut len,
            )
        },
        PrismPdfStatus::NullArgument
    );

    unsafe { prismpdf_sign_settings_free(std::ptr::null_mut()) };
    unsafe { prismpdf_signature_list_free(std::ptr::null_mut()) };
}

#[test]
fn read_side_lists_report_out_of_range_and_empty_payloads() {
    use prismpdf::Attachment;

    // An attachment with no bytes exercises the empty-payload branch of the lend helper.
    let mut builder = Builder::new();
    builder.add_page(PageSpec::new(b"BT ET".to_vec()));
    builder.attach_file(Attachment {
        name: "empty.bin".to_string(),
        mime: "application/octet-stream".to_string(),
        relationship: "Unspecified".to_string(),
        description: None,
        mod_date: None,
        data: Vec::new(),
    });
    let doc = open(&builder.build());

    let mut atts: *mut PrismPdfAttachmentList = std::ptr::null_mut();
    unsafe { prismpdf_document_attachments(doc, &mut atts) };
    let mut att: *const PrismPdfAttachment = std::ptr::null();
    unsafe { prismpdf_attachment_list_get(atts, 0, &mut att) };

    // Empty payload lends a null pointer with length 0, never a dangling one.
    let mut data: *const u8 = std::ptr::null();
    let mut data_len = 99usize;
    assert_eq!(
        unsafe { prismpdf_attachment_data(att, &mut data, &mut data_len) },
        PrismPdfStatus::Ok
    );
    assert!(data.is_null());
    assert_eq!(data_len, 0);

    // An absent optional field is NotFound, not an error.
    let mut text: *mut c_char = std::ptr::null_mut();
    assert_eq!(
        unsafe { prismpdf_attachment_description(att, &mut text) },
        PrismPdfStatus::NotFound
    );

    // Past the end on every new list type.
    assert_eq!(
        unsafe { prismpdf_attachment_list_get(atts, 99, &mut att) },
        PrismPdfStatus::NotFound
    );
    assert!(att.is_null());
    unsafe { prismpdf_attachment_list_free(atts) };

    let mut fonts: *mut PrismPdfFontList = std::ptr::null_mut();
    unsafe { prismpdf_document_fonts(doc, &mut fonts) };
    let mut font: *const PrismPdfFont = std::ptr::null();
    assert_eq!(
        unsafe { prismpdf_font_list_get(fonts, 99, &mut font) },
        PrismPdfStatus::NotFound
    );
    unsafe { prismpdf_font_list_free(fonts) };

    let mut images: *mut PrismPdfImageList = std::ptr::null_mut();
    unsafe { prismpdf_page_images(doc, 0, &mut images) };
    let mut img: *const PrismPdfImage = std::ptr::null();
    assert_eq!(
        unsafe { prismpdf_image_list_get(images, 99, &mut img) },
        PrismPdfStatus::NotFound
    );
    unsafe { prismpdf_image_list_free(images) };

    // A document with no /Metadata and no dates reports NotFound throughout.
    assert_eq!(
        unsafe { prismpdf_document_xmp(doc, &mut text) },
        PrismPdfStatus::NotFound
    );
    unsafe { prismpdf_document_free(doc) };
}

#[test]
fn read_side_entry_points_reject_null_arguments() {
    let mut text: *mut c_char = std::ptr::null_mut();
    let mut len = 0usize;
    let mut data: *const u8 = std::ptr::null();

    assert_eq!(
        unsafe { prismpdf_document_attachments(std::ptr::null(), std::ptr::null_mut()) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_attachment_name(std::ptr::null(), &mut text) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_attachment_data(std::ptr::null(), &mut data, &mut len) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_font_list_len(std::ptr::null(), &mut len) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_font_program(std::ptr::null(), &mut data, &mut len) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_image_list_len(std::ptr::null(), &mut len) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_image_data(std::ptr::null(), &mut data, &mut len) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_document_xmp(std::ptr::null(), &mut text) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_document_info(std::ptr::null(), std::ptr::null(), &mut text) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_document_creation_date(std::ptr::null(), std::ptr::null_mut()) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_page_text_positioned(std::ptr::null(), 0, &mut text) },
        PrismPdfStatus::NullArgument
    );

    unsafe { prismpdf_attachment_list_free(std::ptr::null_mut()) };
    unsafe { prismpdf_font_list_free(std::ptr::null_mut()) };
    unsafe { prismpdf_image_list_free(std::ptr::null_mut()) };
}

#[test]
fn collection_entry_points_reject_null_arguments() {
    let mut list: *mut PrismPdfAnnotationList = std::ptr::null_mut();
    assert_eq!(
        unsafe { prismpdf_page_annotations(std::ptr::null(), 0, &mut list) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_page_annotations(std::ptr::null(), 0, std::ptr::null_mut()) },
        PrismPdfStatus::NullArgument
    );

    let mut len = 0usize;
    assert_eq!(
        unsafe { prismpdf_annotation_list_len(std::ptr::null(), &mut len) },
        PrismPdfStatus::NullArgument
    );
    let mut text: *mut c_char = std::ptr::null_mut();
    assert_eq!(
        unsafe { prismpdf_annotation_subtype(std::ptr::null(), &mut text) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_annotation_rect(std::ptr::null(), std::ptr::null_mut()) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_form_field_name(std::ptr::null(), &mut text) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_outline_item_title(std::ptr::null(), &mut text) },
        PrismPdfStatus::NullArgument
    );
    assert_eq!(
        unsafe { prismpdf_outline_item_child_count(std::ptr::null(), &mut len) },
        PrismPdfStatus::NullArgument
    );

    // Freeing null is a documented no-op on every list type.
    unsafe { prismpdf_annotation_list_free(std::ptr::null_mut()) };
    unsafe { prismpdf_form_field_list_free(std::ptr::null_mut()) };
    unsafe { prismpdf_outline_list_free(std::ptr::null_mut()) };
}

/// A minimal one-page PDF with a content stream.
fn sample_pdf() -> Vec<u8> {
    let content: &[u8] = b"BT /F1 12 Tf (Hello FFI) Tj ET";
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec(),
        {
            let mut b = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
            b.extend_from_slice(content);
            b.extend_from_slice(b"\nendstream");
            b
        },
    ];
    let mut buf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(buf.len());
        buf.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        buf.extend_from_slice(body);
        buf.extend_from_slice(b"\nendobj\n");
    }
    let startxref = buf.len();
    buf.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for off in &offsets {
        buf.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    buf.extend_from_slice(
        format!("trailer\n<< /Size {} /Root 1 0 R >>\n", objects.len() + 1).as_bytes(),
    );
    buf.extend_from_slice(format!("startxref\n{startxref}\n%%EOF\n").as_bytes());
    buf
}

/// Open a sample document, returning the handle (asserting success).
unsafe fn open_sample() -> *mut PrismPdfDocument {
    let pdf = sample_pdf();
    let mut doc: *mut PrismPdfDocument = std::ptr::null_mut();
    let status = unsafe { prismpdf_document_open(pdf.as_ptr(), pdf.len(), &mut doc) };
    assert_eq!(status, PrismPdfStatus::Ok);
    assert!(!doc.is_null());
    doc
}

#[test]
fn open_count_version_text_roundtrip() {
    unsafe {
        let doc = open_sample();

        let mut count = 0usize;
        assert_eq!(
            prismpdf_document_page_count(doc, &mut count),
            PrismPdfStatus::Ok
        );
        assert_eq!(count, 1);

        let (mut major, mut minor) = (0u8, 0u8);
        assert_eq!(
            prismpdf_document_version(doc, &mut major, &mut minor),
            PrismPdfStatus::Ok
        );
        assert_eq!((major, minor), (1, 7));

        let mut text: *mut c_char = std::ptr::null_mut();
        assert_eq!(prismpdf_page_text(doc, 0, &mut text), PrismPdfStatus::Ok);
        assert!(!text.is_null());
        assert_eq!(CStr::from_ptr(text).to_str().unwrap(), "Hello FFI");
        prismpdf_string_free(text);

        prismpdf_document_free(doc);
    }
}

#[test]
fn open_reports_distinguish_strict_and_recovered_documents() {
    unsafe {
        let strict = open_sample();
        let mut report = std::ptr::null_mut();
        assert_eq!(
            prismpdf_document_open_report(strict, &mut report),
            PrismPdfStatus::Ok
        );
        let mut mode = PrismPdfOpenMode::Recovered;
        let mut count = usize::MAX;
        assert_eq!(
            prismpdf_open_report_mode(report, &mut mode),
            PrismPdfStatus::Ok
        );
        assert_eq!(mode, PrismPdfOpenMode::Strict);
        assert_eq!(
            prismpdf_open_report_diagnostic_count(report, &mut count),
            PrismPdfStatus::Ok
        );
        assert_eq!(count, 0);
        prismpdf_open_report_free(report);
        prismpdf_document_free(strict);

        let mut broken = sample_pdf();
        let startxref = broken
            .windows(9)
            .position(|window| window == b"startxref")
            .unwrap();
        broken.truncate(startxref);
        let mut recovered = std::ptr::null_mut();
        assert_eq!(
            prismpdf_document_open(broken.as_ptr(), broken.len(), &mut recovered),
            PrismPdfStatus::Ok
        );
        let mut report = std::ptr::null_mut();
        prismpdf_document_open_report(recovered, &mut report);
        assert_eq!(
            prismpdf_open_report_mode(report, &mut mode),
            PrismPdfStatus::Ok
        );
        assert_eq!(mode, PrismPdfOpenMode::Recovered);
        prismpdf_open_report_diagnostic_count(report, &mut count);
        assert_eq!(count, 1);
        let mut reason = PrismPdfRecoveryReason::UnreachableCatalog;
        let mut has_offset = false;
        let mut offset = 0;
        assert_eq!(
            prismpdf_open_report_diagnostic(report, 0, &mut reason, &mut has_offset, &mut offset),
            PrismPdfStatus::Ok
        );
        assert_eq!(reason, PrismPdfRecoveryReason::XrefParseFailure);
        assert!(has_offset);
        assert_eq!(
            prismpdf_open_report_diagnostic(report, 1, &mut reason, &mut has_offset, &mut offset),
            PrismPdfStatus::NotFound
        );
        prismpdf_open_report_free(report);
        prismpdf_document_free(recovered);
    }
}

#[test]
fn owned_open_options_are_reusable_and_combine_limits_with_passwords() {
    unsafe {
        let plain = sample_pdf();
        let encrypted = Document::open(plain.clone())
            .unwrap()
            .save_encrypted(b"secret", b"owner", prismpdf::Algorithm::Aes128)
            .unwrap();
        let options = prismpdf_open_options_new();
        assert!(!options.is_null());
        assert_eq!(
            prismpdf_open_options_set_max_depth(options, 32),
            PrismPdfStatus::Ok
        );
        assert_eq!(
            prismpdf_open_options_set_max_objstm_objects(options, 1024),
            PrismPdfStatus::Ok
        );
        assert_eq!(
            prismpdf_open_options_set_max_objects(options, 4096),
            PrismPdfStatus::Ok
        );
        assert_eq!(
            prismpdf_open_options_set_password(options, b"secret".as_ptr(), 6),
            PrismPdfStatus::Ok
        );

        for _ in 0..2 {
            let mut doc = std::ptr::null_mut();
            assert_eq!(
                prismpdf_document_open_with_options(
                    encrypted.as_ptr(),
                    encrypted.len(),
                    options,
                    &mut doc
                ),
                PrismPdfStatus::Ok
            );
            assert!(!doc.is_null());
            prismpdf_document_free(doc);
        }

        assert_eq!(
            prismpdf_open_options_set_password(options, std::ptr::null(), 0),
            PrismPdfStatus::Ok
        );
        let mut doc = std::ptr::null_mut();
        assert_eq!(
            prismpdf_document_open_with_options(
                encrypted.as_ptr(),
                encrypted.len(),
                options,
                &mut doc
            ),
            PrismPdfStatus::Password
        );
        assert!(doc.is_null());
        assert_eq!(
            prismpdf_document_open_with_options(plain.as_ptr(), plain.len(), options, &mut doc),
            PrismPdfStatus::Ok
        );
        prismpdf_document_free(doc);
        prismpdf_open_options_free(options);
        prismpdf_open_options_free(std::ptr::null_mut());
    }
}

#[test]
fn legacy_limits_opener_migrates_without_behavior_change() {
    unsafe {
        let bytes = sample_pdf();
        let limits = PrismPdfLimits {
            max_depth: 32,
            max_objstm_objects: 1024,
            max_objects: 4096,
        };
        let mut legacy = std::ptr::null_mut();
        assert_eq!(
            prismpdf_document_open_with_limits(bytes.as_ptr(), bytes.len(), &limits, &mut legacy),
            PrismPdfStatus::Ok
        );

        let options = prismpdf_open_options_new();
        assert_eq!(
            prismpdf_open_options_set_max_depth(options, limits.max_depth),
            PrismPdfStatus::Ok
        );
        assert_eq!(
            prismpdf_open_options_set_max_objstm_objects(options, limits.max_objstm_objects),
            PrismPdfStatus::Ok
        );
        assert_eq!(
            prismpdf_open_options_set_max_objects(options, limits.max_objects),
            PrismPdfStatus::Ok
        );
        let mut migrated = std::ptr::null_mut();
        assert_eq!(
            prismpdf_document_open_with_options(
                bytes.as_ptr(),
                bytes.len(),
                options,
                &mut migrated
            ),
            PrismPdfStatus::Ok
        );

        for doc in [legacy, migrated] {
            let mut pages = 0;
            assert_eq!(
                prismpdf_document_page_count(doc, &mut pages),
                PrismPdfStatus::Ok
            );
            assert_eq!(pages, 1);
            let mut report = std::ptr::null_mut();
            assert_eq!(
                prismpdf_document_open_report(doc, &mut report),
                PrismPdfStatus::Ok
            );
            let mut mode = PrismPdfOpenMode::Recovered;
            assert_eq!(
                prismpdf_open_report_mode(report, &mut mode),
                PrismPdfStatus::Ok
            );
            assert_eq!(mode, PrismPdfOpenMode::Strict);
            prismpdf_open_report_free(report);
            prismpdf_document_free(doc);
        }
        prismpdf_open_options_free(options);
    }
}

#[test]
fn owned_cos_objects_cover_catalog_pages_containers_and_resolution() {
    unsafe {
        let doc = open_sample();
        let mut catalog = std::ptr::null_mut();
        assert_eq!(
            prismpdf_document_catalog_object(doc, &mut catalog),
            PrismPdfStatus::Ok
        );
        let mut kind = PrismPdfObjectKind::Null;
        assert_eq!(prismpdf_object_kind(catalog, &mut kind), PrismPdfStatus::Ok);
        assert_eq!(kind, PrismPdfObjectKind::Dictionary);

        let mut pages_ref = std::ptr::null_mut();
        assert_eq!(
            prismpdf_object_dictionary_get(catalog, b"Pages".as_ptr(), 5, &mut pages_ref),
            PrismPdfStatus::Ok
        );
        assert_eq!(
            prismpdf_object_kind(pages_ref, &mut kind),
            PrismPdfStatus::Ok
        );
        assert_eq!(kind, PrismPdfObjectKind::Reference);
        let (mut number, mut generation) = (0, 0);
        assert_eq!(
            prismpdf_object_reference(pages_ref, &mut number, &mut generation),
            PrismPdfStatus::Ok
        );
        assert!(number > 0);

        let mut pages = std::ptr::null_mut();
        assert_eq!(
            prismpdf_document_resolve_object(doc, pages_ref, &mut pages),
            PrismPdfStatus::Ok
        );
        let mut dict_len = 0;
        assert_eq!(
            prismpdf_object_dictionary_len(pages, &mut dict_len),
            PrismPdfStatus::Ok
        );
        assert!(dict_len > 0);

        let mut page = std::ptr::null_mut();
        assert_eq!(
            prismpdf_document_page_object(doc, 0, &mut page),
            PrismPdfStatus::Ok
        );
        let mut kids = std::ptr::null_mut();
        assert_eq!(
            prismpdf_object_dictionary_get(pages, b"Kids".as_ptr(), 4, &mut kids),
            PrismPdfStatus::Ok
        );
        let mut array_len = 0;
        assert_eq!(
            prismpdf_object_array_len(kids, &mut array_len),
            PrismPdfStatus::Ok
        );
        assert_eq!(array_len, 1);
        let mut kid = std::ptr::null_mut();
        assert_eq!(
            prismpdf_object_array_get(kids, 0, &mut kid),
            PrismPdfStatus::Ok
        );
        assert_eq!(prismpdf_object_kind(kid, &mut kind), PrismPdfStatus::Ok);
        assert_eq!(kind, PrismPdfObjectKind::Reference);

        let mut count = std::ptr::null_mut();
        assert_eq!(
            prismpdf_object_dictionary_get(pages, b"Count".as_ptr(), 5, &mut count),
            PrismPdfStatus::Ok
        );
        let mut integer = 0;
        assert_eq!(
            prismpdf_object_integer(count, &mut integer),
            PrismPdfStatus::Ok
        );
        assert_eq!(integer, 1);

        let mut contents = std::ptr::null_mut();
        assert_eq!(
            prismpdf_object_dictionary_get(page, b"Contents".as_ptr(), 8, &mut contents),
            PrismPdfStatus::Ok
        );
        let mut stream = std::ptr::null_mut();
        assert_eq!(
            prismpdf_document_resolve_object(doc, contents, &mut stream),
            PrismPdfStatus::Ok
        );
        let (mut raw, mut raw_len) = (std::ptr::null(), 0);
        assert_eq!(
            prismpdf_object_stream_raw(stream, &mut raw, &mut raw_len),
            PrismPdfStatus::Ok
        );
        assert!(raw_len > 0);

        for object in [
            catalog, pages_ref, pages, page, kids, kid, count, contents, stream,
        ] {
            prismpdf_object_free(object);
        }
        prismpdf_document_free(doc);
    }
}

#[test]
fn owned_cos_scalar_accessors_are_binary_safe_and_typed() {
    unsafe {
        let boolean = Box::into_raw(Box::new(PrismPdfObject(Object::Boolean(true))));
        let real = Box::into_raw(Box::new(PrismPdfObject(Object::Real(1.25))));
        let string = Box::into_raw(Box::new(PrismPdfObject(Object::String(
            prismpdf::cos::PdfString::from(&b"a\0b"[..]),
        ))));
        let name = Box::into_raw(Box::new(PrismPdfObject(Object::Name(Name::from(
            b"A\0B".to_vec(),
        )))));
        let (mut bool_value, mut real_value) = (false, 0.0);
        assert_eq!(
            prismpdf_object_boolean(boolean, &mut bool_value),
            PrismPdfStatus::Ok
        );
        assert!(bool_value);
        assert_eq!(
            prismpdf_object_real(real, &mut real_value),
            PrismPdfStatus::Ok
        );
        assert_eq!(real_value, 1.25);
        assert_eq!(
            prismpdf_object_integer(real, &mut 0),
            PrismPdfStatus::InvalidUse
        );
        for (object, expected) in [(string, &b"a\0b"[..]), (name, &b"A\0B"[..])] {
            let (mut bytes, mut len) = (std::ptr::null(), 0);
            assert_eq!(
                prismpdf_object_bytes(object, &mut bytes, &mut len),
                PrismPdfStatus::Ok
            );
            assert_eq!(std::slice::from_raw_parts(bytes, len), expected);
            prismpdf_object_free(object);
        }
        prismpdf_object_free(boolean);
        prismpdf_object_free(real);
        prismpdf_object_free(std::ptr::null_mut());
    }
}

#[test]
fn cos_object_constructors_build_nested_mutable_values() {
    unsafe {
        let dict = prismpdf_object_new_dictionary();
        let array = prismpdf_object_new_array();
        let integer = prismpdf_object_new_integer(42);
        let name = prismpdf_object_new_name(b"Demo".as_ptr(), 4);
        assert_eq!(
            prismpdf_object_array_push(array, integer),
            PrismPdfStatus::Ok
        );
        assert_eq!(
            prismpdf_object_dictionary_set(dict, b"Items".as_ptr(), 5, array),
            PrismPdfStatus::Ok
        );
        assert_eq!(
            prismpdf_object_dictionary_set(dict, b"Kind".as_ptr(), 4, name),
            PrismPdfStatus::Ok
        );
        let stream = prismpdf_object_new_stream(dict, b"raw".as_ptr(), 3);
        assert!(!stream.is_null());
        let mut len = 0;
        assert_eq!(
            prismpdf_object_dictionary_len(stream, &mut len),
            PrismPdfStatus::Ok
        );
        assert_eq!(len, 2);
        let cloned = prismpdf_object_clone(stream);
        assert!(!cloned.is_null());
        assert!(prismpdf_object_new_real(f64::NAN).is_null());

        for object in [dict, array, integer, name, stream, cloned] {
            prismpdf_object_free(object);
        }
    }
}

#[test]
fn cos_edit_transactions_commit_incremental_and_full_rewrites() {
    unsafe {
        let source = sample_pdf();
        let doc = open(&source);
        let other = open(&source);

        for mode in [
            PrismPdfEditCommitMode::Incremental,
            PrismPdfEditCommitMode::FullRewrite,
        ] {
            let mut catalog = std::ptr::null_mut();
            assert_eq!(
                prismpdf_document_catalog_object(doc, &mut catalog),
                PrismPdfStatus::Ok
            );
            let marker = prismpdf_object_new_string(b"edited".as_ptr(), 6);
            assert_eq!(
                prismpdf_object_dictionary_set(catalog, b"PrismMarker".as_ptr(), 11, marker),
                PrismPdfStatus::Ok
            );
            let edit = prismpdf_edit_new(doc);
            assert_eq!(
                prismpdf_edit_set_object(edit, 1, 0, catalog),
                PrismPdfStatus::Ok
            );

            // A cross-document rejection does not consume the edit.
            let mut report = std::ptr::null_mut();
            assert_eq!(
                prismpdf_edit_commit(edit, other, mode, &mut report),
                PrismPdfStatus::InvalidUse
            );
            assert!(report.is_null());
            assert_eq!(
                prismpdf_edit_commit(edit, doc, mode, &mut report),
                PrismPdfStatus::Ok
            );
            let mut actual_mode = PrismPdfRewriteMode::Reconstructed;
            assert_eq!(
                prismpdf_transform_report_rewrite_mode(report, &mut actual_mode),
                PrismPdfStatus::Ok
            );
            assert_eq!(
                actual_mode,
                match mode {
                    PrismPdfEditCommitMode::Incremental => PrismPdfRewriteMode::Incremental,
                    PrismPdfEditCommitMode::FullRewrite => PrismPdfRewriteMode::FullRewrite,
                }
            );
            let (mut bytes, mut len) = (std::ptr::null(), 0);
            assert_eq!(
                prismpdf_transform_report_bytes(report, &mut bytes, &mut len),
                PrismPdfStatus::Ok
            );
            let output = std::slice::from_raw_parts(bytes, len);
            if mode == PrismPdfEditCommitMode::Incremental {
                assert!(output.starts_with(&source));
            }
            let reopened = open(output);
            let mut reopened_catalog = std::ptr::null_mut();
            assert_eq!(
                prismpdf_document_catalog_object(reopened, &mut reopened_catalog),
                PrismPdfStatus::Ok
            );
            let mut value = std::ptr::null_mut();
            assert_eq!(
                prismpdf_object_dictionary_get(
                    reopened_catalog,
                    b"PrismMarker".as_ptr(),
                    11,
                    &mut value
                ),
                PrismPdfStatus::Ok
            );
            let (mut marker_bytes, mut marker_len) = (std::ptr::null(), 0);
            assert_eq!(
                prismpdf_object_bytes(value, &mut marker_bytes, &mut marker_len),
                PrismPdfStatus::Ok
            );
            assert_eq!(
                std::slice::from_raw_parts(marker_bytes, marker_len),
                b"edited"
            );

            prismpdf_object_free(value);
            prismpdf_object_free(reopened_catalog);
            prismpdf_document_free(reopened);
            prismpdf_transform_report_free(report);
            prismpdf_object_free(marker);
            prismpdf_object_free(catalog);
        }
        prismpdf_edit_free(std::ptr::null_mut());
        prismpdf_document_free(other);
        prismpdf_document_free(doc);
    }
}

#[test]
fn out_of_range_page_is_not_found() {
    unsafe {
        let doc = open_sample();
        let mut text: *mut c_char = std::ptr::null_mut();
        assert_eq!(
            prismpdf_page_text(doc, 99, &mut text),
            PrismPdfStatus::NotFound
        );
        assert!(text.is_null());
        prismpdf_document_free(doc);
    }
}

#[test]
fn open_with_password_handles_plain_and_encrypted() {
    unsafe {
        // A plain document opens through the password entry point with a null password.
        let pdf = sample_pdf();
        let mut doc: *mut PrismPdfDocument = std::ptr::null_mut();
        assert_eq!(
            prismpdf_document_open_with_password(
                pdf.as_ptr(),
                pdf.len(),
                std::ptr::null(),
                0,
                &mut doc
            ),
            PrismPdfStatus::Ok
        );
        prismpdf_document_free(doc);

        // An AES-128 document encrypted with a non-empty password reports Password when opened
        // with the wrong one, and Ok with the right one.
        let plain = Document::open(sample_pdf()).unwrap();
        let enc = plain
            .save_encrypted(b"secret", b"", prismpdf::Algorithm::Aes128)
            .unwrap();

        let mut bad: *mut PrismPdfDocument = std::ptr::null_mut();
        assert_eq!(
            prismpdf_document_open_with_password(
                enc.as_ptr(),
                enc.len(),
                b"x".as_ptr(),
                1,
                &mut bad
            ),
            PrismPdfStatus::Password
        );
        assert!(bad.is_null());

        let mut good: *mut PrismPdfDocument = std::ptr::null_mut();
        assert_eq!(
            prismpdf_document_open_with_password(
                enc.as_ptr(),
                enc.len(),
                b"secret".as_ptr(),
                6,
                &mut good
            ),
            PrismPdfStatus::Ok
        );
        assert!(!good.is_null());
        prismpdf_document_free(good);
    }
}

#[test]
fn parse_error_on_garbage() {
    let garbage = b"not a pdf";
    let mut doc: *mut PrismPdfDocument = std::ptr::null_mut();
    let status = unsafe { prismpdf_document_open(garbage.as_ptr(), garbage.len(), &mut doc) };
    assert_eq!(status, PrismPdfStatus::Parse);
    assert!(doc.is_null());
}

#[test]
fn structured_last_error_is_thread_local_owned_and_cleared_by_success() {
    unsafe {
        let garbage = b"not a pdf";
        let mut doc = std::ptr::null_mut();
        assert_eq!(
            prismpdf_document_open(garbage.as_ptr(), garbage.len(), &mut doc),
            PrismPdfStatus::Parse
        );
        let mut error = std::ptr::null_mut();
        assert_eq!(prismpdf_last_error(&mut error), PrismPdfStatus::Ok);
        assert!(!error.is_null());
        let mut status = PrismPdfStatus::Ok;
        assert_eq!(
            prismpdf_error_info_status(error, &mut status),
            PrismPdfStatus::Ok
        );
        assert_eq!(status, PrismPdfStatus::Parse);
        let mut message = std::ptr::null_mut();
        assert_eq!(
            prismpdf_error_info_message(error, &mut message),
            PrismPdfStatus::Ok
        );
        let diagnostic = take_string(message);
        assert!(!diagnostic.is_empty());

        // A later successful guarded operation clears the thread-local slot, while the owned
        // snapshot remains independently readable.
        let valid = sample_pdf();
        assert_eq!(
            prismpdf_document_open(valid.as_ptr(), valid.len(), &mut doc),
            PrismPdfStatus::Ok
        );
        let mut none = std::ptr::null_mut();
        assert_eq!(prismpdf_last_error(&mut none), PrismPdfStatus::NotFound);
        assert!(none.is_null());
        assert_eq!(
            prismpdf_error_info_status(error, &mut status),
            PrismPdfStatus::Ok
        );
        assert_eq!(status, PrismPdfStatus::Parse);

        prismpdf_error_info_free(error);
        prismpdf_error_info_free(std::ptr::null_mut());
        prismpdf_document_free(doc);
    }
}

#[test]
fn null_arguments_are_rejected() {
    unsafe {
        assert_eq!(
            prismpdf_document_open(b"x".as_ptr(), 1, std::ptr::null_mut()),
            PrismPdfStatus::NullArgument
        );
        let mut count = 0usize;
        assert_eq!(
            prismpdf_document_page_count(std::ptr::null(), &mut count),
            PrismPdfStatus::NullArgument
        );
        // Freeing null must be a harmless no-op.
        prismpdf_document_free(std::ptr::null_mut());
        prismpdf_string_free(std::ptr::null_mut());
    }
}

#[test]
fn version_string_is_non_empty() {
    let ptr = prismpdf_version();
    assert!(!ptr.is_null());
    let version = unsafe { CStr::from_ptr(ptr) }.to_str().unwrap();
    assert!(!version.is_empty());
}

/// Collect a `(out_data, out_len)` buffer into an owned Vec and free the native allocation.
unsafe fn take_bytes(data: *mut u8, len: usize) -> Vec<u8> {
    assert!(!data.is_null());
    let copy = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
    unsafe { prismpdf_bytes_free(data, len) };
    copy
}

/// Open `pdf` (optionally with a password), returning the page count.
unsafe fn reopen_page_count(pdf: &[u8], password: Option<&[u8]>) -> usize {
    let mut doc: *mut PrismPdfDocument = std::ptr::null_mut();
    let status = match password {
        Some(pw) => unsafe {
            prismpdf_document_open_with_password(
                pdf.as_ptr(),
                pdf.len(),
                pw.as_ptr(),
                pw.len(),
                &mut doc,
            )
        },
        None => unsafe { prismpdf_document_open(pdf.as_ptr(), pdf.len(), &mut doc) },
    };
    assert_eq!(status, PrismPdfStatus::Ok);
    let mut count = 0usize;
    assert_eq!(
        unsafe { prismpdf_document_page_count(doc, &mut count) },
        PrismPdfStatus::Ok
    );
    unsafe { prismpdf_document_free(doc) };
    count
}

#[test]
fn save_round_trips() {
    unsafe {
        let doc = open_sample();
        for save in [prismpdf_document_save, prismpdf_document_save_compact] {
            let (mut data, mut len) = (std::ptr::null_mut(), 0usize);
            assert_eq!(save(doc, &mut data, &mut len), PrismPdfStatus::Ok);
            let bytes = take_bytes(data, len);
            assert!(bytes.starts_with(b"%PDF-"));
            assert_eq!(reopen_page_count(&bytes, None), 1);
        }
        prismpdf_document_free(doc);
    }
}

#[test]
fn save_encrypted_round_trips_and_validates_algorithm() {
    unsafe {
        let doc = open_sample();
        let (mut data, mut len) = (std::ptr::null_mut(), 0usize);
        // AES-256, empty passwords.
        assert_eq!(
            prismpdf_document_save_encrypted(
                doc,
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                2,
                &mut data,
                &mut len,
            ),
            PrismPdfStatus::Ok
        );
        let encrypted = take_bytes(data, len);
        assert!(encrypted.windows(8).any(|w| w == b"/Encrypt"));
        assert_eq!(reopen_page_count(&encrypted, Some(b"")), 1);

        // An out-of-range algorithm code is rejected.
        let (mut d2, mut l2) = (std::ptr::null_mut(), 0usize);
        assert_eq!(
            prismpdf_document_save_encrypted(
                doc,
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                99,
                &mut d2,
                &mut l2,
            ),
            PrismPdfStatus::NullArgument
        );
        prismpdf_document_free(doc);
    }
}

#[test]
fn extract_rotate_and_text() {
    unsafe {
        let doc = open_sample();

        let indices = [0usize];
        let (mut data, mut len) = (std::ptr::null_mut(), 0usize);
        assert_eq!(
            prismpdf_document_extract_pages(doc, indices.as_ptr(), 1, &mut data, &mut len),
            PrismPdfStatus::Ok
        );
        assert_eq!(reopen_page_count(&take_bytes(data, len), None), 1);

        let (mut rd, mut rl) = (std::ptr::null_mut(), 0usize);
        assert_eq!(
            prismpdf_document_rotate_page(doc, 0, 90, &mut rd, &mut rl),
            PrismPdfStatus::Ok
        );
        assert_eq!(reopen_page_count(&take_bytes(rd, rl), None), 1);

        let mut text: *mut c_char = std::ptr::null_mut();
        assert_eq!(prismpdf_document_text(doc, &mut text), PrismPdfStatus::Ok);
        let extracted = CStr::from_ptr(text).to_str().unwrap().to_string();
        prismpdf_string_free(text);
        assert!(extracted.contains("Hello FFI"));

        prismpdf_document_free(doc);
    }
}

#[test]
fn transform_reports_expose_rewrite_signature_and_structure_effects() {
    unsafe {
        let doc = open_sample();
        let mut report = std::ptr::null_mut();
        assert_eq!(
            prismpdf_document_save_report(doc, &mut report),
            PrismPdfStatus::Ok
        );
        let mut mode = PrismPdfRewriteMode::Incremental;
        let mut signatures = PrismPdfSignatureEffect::Preserved;
        let mut structure = PrismPdfStructureEffect::Removed;
        prismpdf_transform_report_rewrite_mode(report, &mut mode);
        prismpdf_transform_report_signature_effect(report, &mut signatures);
        prismpdf_transform_report_structure_effect(report, &mut structure);
        assert_eq!(mode, PrismPdfRewriteMode::FullRewrite);
        assert_eq!(signatures, PrismPdfSignatureEffect::Invalidated);
        assert_eq!(structure, PrismPdfStructureEffect::Preserved);
        let (mut bytes, mut len) = (std::ptr::null(), 0usize);
        assert_eq!(
            prismpdf_transform_report_bytes(report, &mut bytes, &mut len),
            PrismPdfStatus::Ok
        );
        assert!(std::slice::from_raw_parts(bytes, len).starts_with(b"%PDF-"));
        prismpdf_transform_report_free(report);

        let page = [0usize];
        assert_eq!(
            prismpdf_document_extract_pages_report(doc, page.as_ptr(), 1, &mut report),
            PrismPdfStatus::Ok
        );
        prismpdf_transform_report_rewrite_mode(report, &mut mode);
        prismpdf_transform_report_signature_effect(report, &mut signatures);
        prismpdf_transform_report_structure_effect(report, &mut structure);
        assert_eq!(mode, PrismPdfRewriteMode::Reconstructed);
        assert_eq!(signatures, PrismPdfSignatureEffect::Removed);
        assert_eq!(structure, PrismPdfStructureEffect::Removed);
        prismpdf_transform_report_free(report);

        assert_eq!(
            prismpdf_document_rotate_page_report(doc, 0, 90, &mut report),
            PrismPdfStatus::Ok
        );
        prismpdf_transform_report_free(report);

        macro_rules! assert_full_report {
            ($call:expr) => {{
                assert_eq!($call, PrismPdfStatus::Ok);
                prismpdf_transform_report_rewrite_mode(report, &mut mode);
                assert_eq!(mode, PrismPdfRewriteMode::FullRewrite);
                prismpdf_transform_report_free(report);
            }};
        }
        assert_full_report!(prismpdf_document_save_as_report(doc, 1, 7, &mut report));
        assert_full_report!(prismpdf_document_save_compact_report(doc, &mut report));
        assert_full_report!(prismpdf_document_save_packed_report(doc, &mut report));
        assert_full_report!(prismpdf_document_subset_fonts_report(doc, &mut report));

        assert_eq!(
            prismpdf_document_flatten_form_report(doc, &mut report),
            PrismPdfStatus::Ok
        );
        prismpdf_transform_report_structure_effect(report, &mut structure);
        assert_eq!(structure, PrismPdfStructureEffect::Invalidated);
        prismpdf_transform_report_free(report);

        let docs = [doc as *const PrismPdfDocument];
        assert_eq!(
            prismpdf_merge_report(docs.as_ptr(), 1, &mut report),
            PrismPdfStatus::Ok
        );
        prismpdf_transform_report_rewrite_mode(report, &mut mode);
        assert_eq!(mode, PrismPdfRewriteMode::Reconstructed);
        prismpdf_transform_report_free(report);

        assert_eq!(
            prismpdf_document_fill_form_report(
                doc,
                std::ptr::null(),
                std::ptr::null(),
                0,
                &mut report
            ),
            PrismPdfStatus::Ok
        );
        prismpdf_transform_report_rewrite_mode(report, &mut mode);
        prismpdf_transform_report_signature_effect(report, &mut signatures);
        assert_eq!(mode, PrismPdfRewriteMode::Incremental);
        assert_eq!(signatures, PrismPdfSignatureEffect::Preserved);
        prismpdf_transform_report_free(report);
        prismpdf_transform_report_free(std::ptr::null_mut());
        prismpdf_document_free(doc);
    }
}

#[test]
fn merge_concatenates_pages() {
    unsafe {
        let a = open_sample();
        let b = open_sample();
        let handles: [*const PrismPdfDocument; 2] = [a, b];
        let (mut data, mut len) = (std::ptr::null_mut(), 0usize);
        assert_eq!(
            prismpdf_merge(handles.as_ptr(), 2, &mut data, &mut len),
            PrismPdfStatus::Ok
        );
        assert_eq!(reopen_page_count(&take_bytes(data, len), None), 2);

        // A null handle in the list is rejected.
        let bad: [*const PrismPdfDocument; 2] = [a, std::ptr::null()];
        let (mut d2, mut l2) = (std::ptr::null_mut(), 0usize);
        assert_eq!(
            prismpdf_merge(bad.as_ptr(), 2, &mut d2, &mut l2),
            PrismPdfStatus::NullArgument
        );

        prismpdf_document_free(a);
        prismpdf_document_free(b);
    }
}

#[test]
fn bytes_free_null_is_noop() {
    unsafe { prismpdf_bytes_free(std::ptr::null_mut(), 0) };
}

#[test]
fn composition_vertical_slice_builds_and_finalises_once() {
    unsafe {
        let composition = prismpdf_composition_new();
        let page_style = PrismPdfCompositionPageStyle {
            width: 200.0,
            height: 100.0,
            margin_left: 10.0,
            margin_right: 10.0,
            margin_top: 10.0,
            margin_bottom: 10.0,
        };
        let mut content = std::ptr::null_mut();
        assert_eq!(
            prismpdf_composition_add_page(composition, &page_style, &mut content),
            PrismPdfStatus::Ok
        );
        let mut column = std::ptr::null_mut();
        assert_eq!(
            prismpdf_composition_container_set_column(content, 4.0, &mut column),
            PrismPdfStatus::Ok
        );
        // Filling a slot consumes its generation; the original scoped handle is stale.
        assert_eq!(
            prismpdf_composition_container_set_page_break(content),
            PrismPdfStatus::InvalidUse
        );

        let mut child = std::ptr::null_mut();
        assert_eq!(
            prismpdf_composition_column_add_item(column, &mut child),
            PrismPdfStatus::Ok
        );
        let text = CString::new("Hello composition FFI").unwrap();
        let text_style = PrismPdfCompositionTextStyle {
            size: 12.0,
            leading: 14.0,
        };
        assert_eq!(
            prismpdf_composition_container_set_text(child, text.as_ptr(), &text_style),
            PrismPdfStatus::Ok
        );

        let (mut data, mut len) = (std::ptr::null_mut(), 0usize);
        assert_eq!(
            prismpdf_composition_build(composition, &mut data, &mut len),
            PrismPdfStatus::Ok
        );
        let bytes = take_bytes(data, len);
        let document = Document::open(bytes).unwrap();
        assert_eq!(document.page_count().unwrap(), 1);
        assert!(
            prismpdf::page_text(&document, 0)
                .unwrap()
                .unwrap()
                .contains("Hello composition FFI")
        );

        assert_eq!(
            prismpdf_composition_build(composition, &mut data, &mut len),
            PrismPdfStatus::InvalidUse
        );
        let mut another = std::ptr::null_mut();
        assert_eq!(
            prismpdf_composition_column_add_item(column, &mut another),
            PrismPdfStatus::InvalidUse
        );

        prismpdf_composition_container_free(child);
        prismpdf_composition_container_free(column);
        prismpdf_composition_container_free(content);
        prismpdf_composition_free(composition);
    }
}

#[test]
fn composition_children_detect_a_released_owner_and_failed_build_finalises() {
    unsafe {
        let composition = prismpdf_composition_new();
        let style = PrismPdfCompositionPageStyle {
            width: 100.0,
            height: 100.0,
            margin_left: 10.0,
            margin_right: 10.0,
            margin_top: 10.0,
            margin_bottom: 10.0,
        };
        let mut content = std::ptr::null_mut();
        assert_eq!(
            prismpdf_composition_add_page(composition, &style, &mut content),
            PrismPdfStatus::Ok
        );
        prismpdf_composition_free(composition);
        assert_eq!(
            prismpdf_composition_container_set_page_break(content),
            PrismPdfStatus::InvalidUse
        );
        prismpdf_composition_container_free(content);

        let invalid = prismpdf_composition_new();
        let mut slot = std::ptr::null_mut();
        assert_eq!(
            prismpdf_composition_add_page(invalid, &style, &mut slot),
            PrismPdfStatus::Ok
        );
        let text = CString::new("bad").unwrap();
        let invalid_style = PrismPdfCompositionTextStyle {
            size: f64::NAN,
            leading: 14.0,
        };
        assert_eq!(
            prismpdf_composition_container_set_text(slot, text.as_ptr(), &invalid_style),
            PrismPdfStatus::Ok
        );
        let (mut data, mut len) = (std::ptr::null_mut(), 0usize);
        assert_eq!(
            prismpdf_composition_build(invalid, &mut data, &mut len),
            PrismPdfStatus::Layout
        );
        let mut another = std::ptr::null_mut();
        assert_eq!(
            prismpdf_composition_add_page(invalid, &style, &mut another),
            PrismPdfStatus::InvalidUse
        );
        prismpdf_composition_container_free(slot);
        prismpdf_composition_free(invalid);
    }
}

#[test]
fn composition_rows_and_decorators_replay_through_the_abi() {
    unsafe {
        let composition = prismpdf_composition_new();
        let style = PrismPdfCompositionPageStyle {
            width: 360.0,
            height: 240.0,
            margin_left: 20.0,
            margin_right: 20.0,
            margin_top: 20.0,
            margin_bottom: 20.0,
        };
        let text_style = PrismPdfCompositionTextStyle {
            size: 10.0,
            leading: 12.0,
        };
        let mut handles = Vec::new();

        let mut content = std::ptr::null_mut();
        assert_eq!(
            prismpdf_composition_add_page(composition, &style, &mut content),
            PrismPdfStatus::Ok
        );
        handles.push(content);
        let mut column = std::ptr::null_mut();
        assert_eq!(
            prismpdf_composition_container_set_column(content, 8.0, &mut column),
            PrismPdfStatus::Ok
        );
        handles.push(column);

        let mut row_slot = std::ptr::null_mut();
        assert_eq!(
            prismpdf_composition_column_add_item(column, &mut row_slot),
            PrismPdfStatus::Ok
        );
        handles.push(row_slot);
        let mut row = std::ptr::null_mut();
        assert_eq!(
            prismpdf_composition_container_set_row(row_slot, &mut row),
            PrismPdfStatus::Ok
        );
        handles.push(row);
        for (label, kind) in [("fixed", 0), ("relative", 1), ("auto", 2)] {
            let mut child = std::ptr::null_mut();
            let status = match kind {
                0 => prismpdf_composition_row_add_fixed(row, 70.0, &mut child),
                1 => prismpdf_composition_row_add_relative(row, 1.0, &mut child),
                _ => prismpdf_composition_row_add_auto(row, &mut child),
            };
            assert_eq!(status, PrismPdfStatus::Ok);
            handles.push(child);
            let label = CString::new(label).unwrap();
            assert_eq!(
                prismpdf_composition_container_set_text(child, label.as_ptr(), &text_style),
                PrismPdfStatus::Ok
            );
        }

        let mut decorated = std::ptr::null_mut();
        assert_eq!(
            prismpdf_composition_column_add_item(column, &mut decorated),
            PrismPdfStatus::Ok
        );
        handles.push(decorated);
        let color = PrismPdfCompositionColor {
            red: 0.2,
            green: 0.4,
            blue: 0.6,
        };
        macro_rules! wrap {
            ($call:expr) => {{
                let mut child = std::ptr::null_mut();
                assert_eq!($call(&mut child), PrismPdfStatus::Ok);
                handles.push(child);
                decorated = child;
            }};
        }
        wrap!(|out| prismpdf_composition_container_set_padding(decorated, 4.0, out));
        wrap!(|out| prismpdf_composition_container_set_border(decorated, 1.0, color, out));
        wrap!(|out| prismpdf_composition_container_set_background(decorated, color, out));
        wrap!(|out| prismpdf_composition_container_set_width(decorated, 180.0, out));
        wrap!(|out| prismpdf_composition_container_set_height(decorated, 60.0, out));
        wrap!(|out| prismpdf_composition_container_set_alignment(
            decorated,
            PrismPdfCompositionHorizontalAlign::Center,
            PrismPdfCompositionVerticalAlign::Center,
            out
        ));
        wrap!(|out| prismpdf_composition_container_set_extend(decorated, out));
        let text = CString::new("decorated").unwrap();
        assert_eq!(
            prismpdf_composition_container_set_text(decorated, text.as_ptr(), &text_style),
            PrismPdfStatus::Ok
        );

        let (mut data, mut len) = (std::ptr::null_mut(), 0usize);
        assert_eq!(
            prismpdf_composition_build(composition, &mut data, &mut len),
            PrismPdfStatus::Ok
        );
        let document = Document::open(take_bytes(data, len)).unwrap();
        let text = prismpdf::page_text(&document, 0).unwrap().unwrap();
        for expected in ["fixed", "relative", "auto", "decorated"] {
            assert!(text.contains(expected));
        }

        for handle in handles {
            prismpdf_composition_container_free(handle);
        }
        prismpdf_composition_free(composition);
    }
}

#[test]
fn composition_repeating_regions_expand_page_placeholders() {
    unsafe {
        let composition = prismpdf_composition_new();
        let style = PrismPdfCompositionPageStyle {
            width: 200.0,
            height: 100.0,
            margin_left: 10.0,
            margin_right: 10.0,
            margin_top: 10.0,
            margin_bottom: 10.0,
        };
        let text_style = PrismPdfCompositionTextStyle {
            size: 8.0,
            leading: 10.0,
        };
        let mut handles = Vec::new();
        let mut content = std::ptr::null_mut();
        assert_eq!(
            prismpdf_composition_add_page(composition, &style, &mut content),
            PrismPdfStatus::Ok
        );
        handles.push(content);

        let mut header = std::ptr::null_mut();
        assert_eq!(
            prismpdf_composition_page_set_header(composition, 0, &mut header),
            PrismPdfStatus::Ok
        );
        handles.push(header);
        let header_text = CString::new("Repeated header").unwrap();
        assert_eq!(
            prismpdf_composition_container_set_text(header, header_text.as_ptr(), &text_style),
            PrismPdfStatus::Ok
        );
        let mut duplicate = std::ptr::null_mut();
        assert_eq!(
            prismpdf_composition_page_set_header(composition, 0, &mut duplicate),
            PrismPdfStatus::InvalidUse
        );
        assert_eq!(
            prismpdf_composition_page_set_header(composition, 1, &mut duplicate),
            PrismPdfStatus::NotFound
        );

        let mut footer = std::ptr::null_mut();
        assert_eq!(
            prismpdf_composition_page_set_footer(composition, 0, &mut footer),
            PrismPdfStatus::Ok
        );
        handles.push(footer);
        let footer_text = CString::new("Page {page} of {pages}").unwrap();
        assert_eq!(
            prismpdf_composition_container_set_text(footer, footer_text.as_ptr(), &text_style),
            PrismPdfStatus::Ok
        );

        let mut column = std::ptr::null_mut();
        assert_eq!(
            prismpdf_composition_container_set_column(content, 0.0, &mut column),
            PrismPdfStatus::Ok
        );
        handles.push(column);
        for index in 0..18 {
            let mut item = std::ptr::null_mut();
            assert_eq!(
                prismpdf_composition_column_add_item(column, &mut item),
                PrismPdfStatus::Ok
            );
            handles.push(item);
            let line = CString::new(format!("Line {index}")).unwrap();
            assert_eq!(
                prismpdf_composition_container_set_text(item, line.as_ptr(), &text_style),
                PrismPdfStatus::Ok
            );
        }

        let (mut data, mut len) = (std::ptr::null_mut(), 0usize);
        assert_eq!(
            prismpdf_composition_build(composition, &mut data, &mut len),
            PrismPdfStatus::Ok
        );
        let document = Document::open(take_bytes(data, len)).unwrap();
        let count = document.page_count().unwrap();
        assert!(count > 1);
        for page in 0..count {
            let text = prismpdf::page_text(&document, page).unwrap().unwrap();
            assert!(text.contains("Repeated header"));
            assert!(text.contains(&format!("Page {} of {count}", page + 1)));
        }

        for handle in handles {
            prismpdf_composition_container_free(handle);
        }
        prismpdf_composition_free(composition);
    }
}

#[test]
fn composition_semantics_emit_a_tagged_structure_tree() {
    unsafe {
        let composition = prismpdf_composition_new();
        let lang = CString::new("en-US").unwrap();
        assert_eq!(
            prismpdf_composition_set_tagged_language(composition, lang.as_ptr()),
            PrismPdfStatus::Ok
        );
        let style = PrismPdfCompositionPageStyle {
            width: 240.0,
            height: 180.0,
            margin_left: 15.0,
            margin_right: 15.0,
            margin_top: 15.0,
            margin_bottom: 15.0,
        };
        let text_style = PrismPdfCompositionTextStyle {
            size: 10.0,
            leading: 12.0,
        };
        let mut handles = Vec::new();
        let mut content = std::ptr::null_mut();
        assert_eq!(
            prismpdf_composition_add_page(composition, &style, &mut content),
            PrismPdfStatus::Ok
        );
        handles.push(content);
        let mut column = std::ptr::null_mut();
        assert_eq!(
            prismpdf_composition_container_set_column(content, 3.0, &mut column),
            PrismPdfStatus::Ok
        );
        handles.push(column);

        let mut item = std::ptr::null_mut();
        prismpdf_composition_column_add_item(column, &mut item);
        handles.push(item);
        let mut semantic = std::ptr::null_mut();
        assert_eq!(
            prismpdf_composition_container_set_heading(item, 2, &mut semantic),
            PrismPdfStatus::Ok
        );
        handles.push(semantic);
        let heading = CString::new("Tagged heading").unwrap();
        assert_eq!(
            prismpdf_composition_container_set_text(semantic, heading.as_ptr(), &text_style),
            PrismPdfStatus::Ok
        );

        let mut item = std::ptr::null_mut();
        prismpdf_composition_column_add_item(column, &mut item);
        handles.push(item);
        let mut semantic = std::ptr::null_mut();
        assert_eq!(
            prismpdf_composition_container_set_semantic(
                item,
                PrismPdfCompositionSemantic::Paragraph,
                &mut semantic
            ),
            PrismPdfStatus::Ok
        );
        handles.push(semantic);
        let paragraph = CString::new("Tagged paragraph").unwrap();
        prismpdf_composition_container_set_text(semantic, paragraph.as_ptr(), &text_style);

        let mut item = std::ptr::null_mut();
        prismpdf_composition_column_add_item(column, &mut item);
        handles.push(item);
        let mut semantic = std::ptr::null_mut();
        let uri = CString::new("https://example.com").unwrap();
        let description = CString::new("Example link").unwrap();
        assert_eq!(
            prismpdf_composition_container_set_link(
                item,
                uri.as_ptr(),
                description.as_ptr(),
                &mut semantic
            ),
            PrismPdfStatus::Ok
        );
        handles.push(semantic);
        let link = CString::new("Open example").unwrap();
        prismpdf_composition_container_set_text(semantic, link.as_ptr(), &text_style);

        let mut item = std::ptr::null_mut();
        prismpdf_composition_column_add_item(column, &mut item);
        handles.push(item);
        let mut semantic = std::ptr::null_mut();
        let alt = CString::new("A textual figure").unwrap();
        assert_eq!(
            prismpdf_composition_container_set_figure(item, alt.as_ptr(), &mut semantic),
            PrismPdfStatus::Ok
        );
        handles.push(semantic);
        let figure = CString::new("Figure content").unwrap();
        prismpdf_composition_container_set_text(semantic, figure.as_ptr(), &text_style);

        let mut unused = std::ptr::null_mut();
        assert_eq!(
            prismpdf_composition_container_set_heading(item, 0, &mut unused),
            PrismPdfStatus::Layout
        );

        let (mut data, mut len) = (std::ptr::null_mut(), 0usize);
        assert_eq!(
            prismpdf_composition_build(composition, &mut data, &mut len),
            PrismPdfStatus::Ok
        );
        let bytes = take_bytes(data, len);
        let source = String::from_utf8_lossy(&bytes);
        assert!(source.contains("/StructTreeRoot"));
        assert!(source.contains("/Lang (en-US)"));
        assert!(source.contains("/Alt (A textual figure)"));

        for handle in handles {
            prismpdf_composition_container_free(handle);
        }
        prismpdf_composition_free(composition);
    }
}

#[test]
fn composition_tables_repeat_headers_across_fragments() {
    unsafe {
        let composition = prismpdf_composition_new();
        let style = PrismPdfCompositionPageStyle {
            width: 260.0,
            height: 110.0,
            margin_left: 10.0,
            margin_right: 10.0,
            margin_top: 10.0,
            margin_bottom: 10.0,
        };
        let text_style = PrismPdfCompositionTextStyle {
            size: 8.0,
            leading: 10.0,
        };
        let mut handles = Vec::new();
        let mut content = std::ptr::null_mut();
        assert_eq!(
            prismpdf_composition_add_page(composition, &style, &mut content),
            PrismPdfStatus::Ok
        );
        handles.push(content);
        let mut table = std::ptr::null_mut();
        assert_eq!(
            prismpdf_composition_container_set_table(content, &mut table),
            PrismPdfStatus::Ok
        );
        handles.push(table);
        assert_eq!(
            prismpdf_composition_table_add_fixed_column(table, 55.0),
            PrismPdfStatus::Ok
        );
        assert_eq!(
            prismpdf_composition_table_add_relative_column(table, 1.0),
            PrismPdfStatus::Ok
        );
        assert_eq!(
            prismpdf_composition_table_add_auto_column(table),
            PrismPdfStatus::Ok
        );

        let mut header = std::ptr::null_mut();
        assert_eq!(
            prismpdf_composition_table_set_header(table, &mut header),
            PrismPdfStatus::Ok
        );
        handles.push(header);
        for label in ["Code", "Description", "Qty"] {
            let mut cell = std::ptr::null_mut();
            assert_eq!(
                prismpdf_composition_table_row_add_cell(header, &mut cell),
                PrismPdfStatus::Ok
            );
            handles.push(cell);
            let label = CString::new(label).unwrap();
            assert_eq!(
                prismpdf_composition_container_set_text(cell, label.as_ptr(), &text_style),
                PrismPdfStatus::Ok
            );
        }
        let mut duplicate = std::ptr::null_mut();
        assert_eq!(
            prismpdf_composition_table_set_header(table, &mut duplicate),
            PrismPdfStatus::InvalidUse
        );

        for index in 0..16 {
            let mut row = std::ptr::null_mut();
            assert_eq!(
                prismpdf_composition_table_add_row(table, &mut row),
                PrismPdfStatus::Ok
            );
            handles.push(row);
            for value in [format!("P{index}"), format!("Item {index}"), "1".into()] {
                let mut cell = std::ptr::null_mut();
                prismpdf_composition_table_row_add_cell(row, &mut cell);
                handles.push(cell);
                let value = CString::new(value).unwrap();
                prismpdf_composition_container_set_text(cell, value.as_ptr(), &text_style);
            }
        }

        let (mut data, mut len) = (std::ptr::null_mut(), 0usize);
        assert_eq!(
            prismpdf_composition_build(composition, &mut data, &mut len),
            PrismPdfStatus::Ok
        );
        let document = Document::open(take_bytes(data, len)).unwrap();
        let count = document.page_count().unwrap();
        assert!(count > 1);
        for page in 0..count {
            let text = prismpdf::page_text(&document, page).unwrap().unwrap();
            assert!(text.contains("Code"));
            assert!(text.contains("Description"));
            assert!(text.contains("Qty"));
        }
        for handle in handles {
            prismpdf_composition_container_free(handle);
        }
        prismpdf_composition_free(composition);
    }
}

#[test]
fn composition_images_clone_sources_and_support_every_sizing_policy() {
    unsafe {
        let composition = prismpdf_composition_new();
        let style = PrismPdfCompositionPageStyle {
            width: 220.0,
            height: 180.0,
            margin_left: 10.0,
            margin_right: 10.0,
            margin_top: 10.0,
            margin_bottom: 10.0,
        };
        let mut handles = Vec::new();
        let mut content = std::ptr::null_mut();
        assert_eq!(
            prismpdf_composition_add_page(composition, &style, &mut content),
            PrismPdfStatus::Ok
        );
        handles.push(content);
        let mut column = std::ptr::null_mut();
        assert_eq!(
            prismpdf_composition_container_set_column(content, 4.0, &mut column),
            PrismPdfStatus::Ok
        );
        handles.push(column);
        let rgb = [255, 0, 0, 0, 0, 255];
        let image = prismpdf_image_source_from_rgb(2, 1, rgb.as_ptr(), rgb.len());
        assert!(!image.is_null());
        for sizing in [
            PrismPdfCompositionImageSizing::Fit,
            PrismPdfCompositionImageSizing::Fill,
            PrismPdfCompositionImageSizing::Exact,
        ] {
            let mut item = std::ptr::null_mut();
            prismpdf_composition_column_add_item(column, &mut item);
            handles.push(item);
            assert_eq!(
                prismpdf_composition_container_set_image(item, image, sizing, 80.0, 35.0),
                PrismPdfStatus::Ok
            );
        }
        prismpdf_image_source_free(image);

        let (mut data, mut len) = (std::ptr::null_mut(), 0usize);
        assert_eq!(
            prismpdf_composition_build(composition, &mut data, &mut len),
            PrismPdfStatus::Ok
        );
        let document = Document::open(take_bytes(data, len)).unwrap();
        assert_eq!(prismpdf::page_images(&document, 0).unwrap().len(), 3);
        for handle in handles {
            prismpdf_composition_container_free(handle);
        }
        prismpdf_composition_free(composition);
    }
}

#[cfg(feature = "c-acceptance")]
#[test]
fn standalone_c_consumer_builds_the_acceptance_invoice() {
    unsafe {
        let (mut data, mut len) = (std::ptr::null_mut(), 0usize);
        assert_eq!(
            prismpdf_c_invoice_acceptance(&mut data, &mut len),
            PrismPdfStatus::Ok as i32
        );
        let bytes = take_bytes(data, len);
        let source = String::from_utf8_lossy(&bytes);
        assert!(source.contains("/StructTreeRoot"));
        let document = Document::open(bytes).unwrap();
        let count = document.page_count().unwrap();
        assert!(count > 1);
        for page in 0..count {
            let text = prismpdf::page_text(&document, page).unwrap().unwrap();
            assert!(text.contains("Prism PDF Studio"));
            assert!(text.contains(&format!("Page {} of {count}", page + 1)));
        }
        let all_text = prismpdf::document_text(&document).unwrap();
        assert!(all_text.contains("INVOICE"));
        assert!(all_text.contains("Invoice 2026-0042"));
        assert!(all_text.contains("Document processing service 64"));
        assert!(all_text.contains("TOTAL 23,760.00"));
    }
}
