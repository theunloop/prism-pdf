# Prism PDF C ABI

The C ABI (`pdf-ffi`, EPIC 10) is Prism PDF's universal interop boundary: language bindings
(.NET, Go, Swift, …) link against it rather than the Rust crates. It is the **only** place
`unsafe` is allowed in the project (DESIGN.md §3.7).

**Policy: capability parity with the `prismpdf` facade** (DESIGN.md §6.1). Every capability the Rust
facade offers gets a C entry point — reading, authoring, editing, signing, conformance. What the
boundary does *not* preserve is signature shape: C has no `Result`, `Option`, `String`, `Vec` or
generics, so the conventions below translate them. If you can do it in Rust and not in C, that is
a gap to close, not a design decision.

This file is the boundary's *contract*; how a language binding should project it into an
idiomatic API — object model, naming rules, conformance suite — is [`BINDINGS.md`](BINDINGS.md).

The header `crates/pdf-ffi/include/prismpdf.h` is generated from the Rust source by
[`cbindgen`](https://github.com/mozilla/cbindgen); see “Regenerating the header” below.

## Design contract (DESIGN.md §6.1)

- **Handle-based.** A document is an opaque `PrismPdfDocument*` obtained from
  `prismpdf_document_open` and passed back to every other call. C code never dereferences it.
- **No unwinding.** Every function wraps its body in `catch_unwind`; a Rust panic is reported as
  `PrismPdfStatus_Internal` and never crosses the boundary.
- **Stable status codes.** Functions return a `PrismPdfStatus` whose integer values are stable and
  must never be renumbered (only appended to). Results travel through out-parameters.
- **Explicit ownership.** Memory the library allocates is released only by the matching `*_free`
  function — never with the C runtime's `free`. Freeing `NULL` is a no-op.

## Status codes

| Value | Name | Meaning |
|------:|------|---------|
| 0 | `PrismPdfStatus_Ok` | Success. |
| 1 | `PrismPdfStatus_NullArgument` | A required pointer argument was null. |
| 2 | `PrismPdfStatus_Parse` | The document could not be parsed (even after recovery). |
| 3 | `PrismPdfStatus_NotFound` | The requested item does not exist (page index out of range, no header version). |
| 4 | `PrismPdfStatus_Internal` | Internal error, including a caught panic. |
| 5 | `PrismPdfStatus_Password` | The document is encrypted and the supplied password is wrong (§7.6); retry via `prismpdf_document_open_with_password`. |
| 6 | `PrismPdfStatus_Conformance` | A conformance pass refused the document. Nothing is malformed — a standard's rule is unmet. The rule arrives in the call's `out_issue` parameter as a `PrismPdfConformanceIssue`. |
| 7 | `PrismPdfStatus_InvalidUse` | A mutable handle is stale, its owning composition was released, or the composition was already finalised. |
| 8 | `PrismPdfStatus_Layout` | Declarative composition rejected geometry or could not paginate the element tree. |

### Structured failure diagnostics

Status codes remain the stable control-flow contract. After a guarded call fails,
`prismpdf_last_error` clones that thread's diagnostic into an owned `PrismPdfErrorInfo`; its status
matches the failed call and its message carries parser/serializer detail when available. A later
successful guarded call clears the thread-local slot, but previously cloned snapshots remain valid.

| Function | Ownership |
|----------|-----------|
| `prismpdf_last_error(out_error)` | Returns an owned snapshot, or `NotFound` after success/no failure. |
| `prismpdf_error_info_status(error, out_status)` | Reads the stable status. |
| `prismpdf_error_info_message(error, out_message)` | Copies an owned string; release it with `prismpdf_string_free`. |
| `prismpdf_error_info_free(error)` | Releases the snapshot; null is ignored. |

Argument checks that reject before entering a guarded engine operation report their status directly
and do not replace the diagnostic slot. This prevents a secondary cleanup mistake from erasing the
actionable parser, layout, conformance, or serialization failure that preceded it.

## Type and ownership conventions

How each Rust shape crosses the boundary. These are the rules every new entry point follows.

| Rust | C | Freed with |
|------|---|-----------|
| `Result<T, E>` | `PrismPdfStatus` return; `T` via out-param | — |
| `Option<T>` (absent) | `PrismPdfStatus_NotFound`, out-param left null/zero | — |
| `String` / `&str` out | owned `char *` out-param, NUL-terminated UTF-8 | `prismpdf_string_free` |
| `&str` in | `const char *`, NUL-terminated UTF-8 | caller's own |
| `Vec<u8>` returned by a call | owned `(uint8_t *, uintptr_t)` out-pair | `prismpdf_bytes_free` |
| `Vec<u8>` *inside a borrowed item* | **borrowed** `(const uint8_t *, uintptr_t)` view | nothing — dies with the list |
| `[f64; N]` | `double *` out-param, caller provides `N` slots | — |
| `Vec<Item>` | owned **list handle** + borrowed item pointers | `*_list_free` |
| `&[(&str, &str)]` in | two parallel `const char *const *` arrays + `count` | caller's own |
| fieldless `enum` | `#[repr(C)]` integer enum | — |

### Handle-claiming calls

A call that takes a handle it may claim says so in its own doc comment with one of three markers,
and there is no fourth shape. A binding generator can check the marker mechanically; a binding
author must not infer the shape from the call's name.

| Marker | Exports | Meaning |
|--------|---------|---------|
| **Consumes on success** | `prismpdf_edit_commit`, `prismpdf_builder_add_page_spec`, `prismpdf_builder_add_structure_node`, `prismpdf_struct_node_add_child` | Ownership transfers only on `Ok`. A failure leaves the handle caller-owned and still freeable. |
| **Consumes always** | `prismpdf_flow_build`, `prismpdf_flow_into_builder` | The box is taken as the call is entered, so the handle is dead on failure too. Freeing it afterwards is a double free. |
| **Finalises** | `prismpdf_composition_build` | Not a transfer: the handle becomes immutable on success and on failure (later calls return `InvalidUse`), but freeing it stays the caller's job. |

### Collections

The facade returns owned Rust vectors whose items carry `String` and `Option` fields. C has
neither, so every collection crosses the same way:

1. A producer (`prismpdf_page_annotations`, `prismpdf_document_form_fields`, …) writes an owned
   **list handle** to an out-param.
2. `*_list_len` reports the item count.
3. `*_list_get` lends a **borrowed** item pointer, valid only while the list handle lives. Borrowed
   items are never freed by the caller — passing one to a `*_free` is a bug.
4. Per-field getters read one field off a borrowed item. A field that is `None` returns
   `PrismPdfStatus_NotFound` and leaves the out-param null/zero, which is *not* an error.
5. A getter for a **byte payload** (`prismpdf_attachment_data`, `prismpdf_font_program`,
   `prismpdf_image_data`) lends a `const uint8_t *` view into the list's own allocation instead of
   copying. Never pass one to `prismpdf_bytes_free`; it dies with the list like any borrowed item.
   An empty payload lends a null pointer with length 0 rather than a dangling one.
6. `*_list_free` releases the list and invalidates every pointer lent from it.

Nested collections (the outline tree, and later the structure tree) need no per-level handle: the
root list owns the whole tree, and a child is lent straight out of its parent, so recursion to any
depth allocates nothing.

```c
PrismPdfAnnotationList *annots = NULL;
if (prismpdf_page_annotations(doc, 0, &annots) == PrismPdfStatus_Ok) {
    uintptr_t n = 0;
    prismpdf_annotation_list_len(annots, &n);
    for (uintptr_t i = 0; i < n; i++) {
        const PrismPdfAnnotation *a = NULL;
        prismpdf_annotation_list_get(annots, i, &a);

        char *subtype = NULL;
        prismpdf_annotation_subtype(a, &subtype);
        printf("%s", subtype);
        prismpdf_string_free(subtype);

        char *uri = NULL;
        if (prismpdf_annotation_uri(a, &uri) == PrismPdfStatus_Ok) {
            printf(" -> %s", uri);
            prismpdf_string_free(uri);
        }                       /* NotFound simply means "no URI on this annotation" */
        printf("\n");
    }
    prismpdf_annotation_list_free(annots);   /* every `a` above is dangling from here */
}
```

## Current surface

Two result conventions, each with its own deallocator: **strings** are NUL-terminated UTF-8 released
with `prismpdf_string_free`; **documents** come back as a `(data, len)` byte buffer released with
`prismpdf_bytes_free` (both values must be passed back, since the buffer is not NUL-terminated).

### Lifecycle & read path (M1/M3)

| Function | Purpose |
|----------|---------|
| `prismpdf_document_open(data, len, out_doc)` | Open a PDF from an in-memory buffer (copied). |
| `prismpdf_document_open_with_password(data, len, password, password_len, out_doc)` | Open an encrypted document (§7.6); the password is tried as both user and owner. Wrong password → `PrismPdfStatus_Password`. |
| `prismpdf_open_options_new` / setters / `_free` | Configure reusable opaque anti-DoS limits and a copied password without freezing a by-value struct layout. |
| `prismpdf_document_open_with_options(data, len, options, out_doc)` | Open from a snapshot of reusable options; returned documents do not borrow the handle. |
| `prismpdf_document_open_report(doc, out_report)` | Clone an owned report recording strict versus recovered open. |
| `prismpdf_open_report_mode` / `_diagnostic_count` / `_diagnostic` / `_free` | Inspect and release bounded recovery diagnostics; offsets distinguish xref parse failures from catalog-reachability recovery. |
| `prismpdf_document_free(doc)` | Release a document handle. |
| `prismpdf_document_page_count(doc, out_count)` | Number of pages. |
| `prismpdf_document_version(doc, out_major, out_minor)` | Header version. |
| `prismpdf_page_text(doc, index, out_text)` | Extract one page's text as a UTF-8 C string. |
| `prismpdf_document_text(doc, out_text)` | Extract the whole document's text as a UTF-8 C string. |
| `prismpdf_string_free(text)` | Release a string the library returned. |
| `prismpdf_version()` | Library version (static string, never freed). |

`prismpdf_document_open_with_limits` is deprecated in 0.2.0 but remains available for source and
binary compatibility through the pre-1.0 compatibility window. New C code should prefer
`PrismPdfOpenOptions`: adding a future option does not change that opaque type's layout, and one
handle can safely be reused for multiple sequential opens. Password bytes are copied by the setter
and may be released immediately by the caller. Migration is mechanical: allocate options, copy
each nonzero `PrismPdfLimits` field with its matching setter, open, then free the options after the
last open.

### Expert COS inspection (§7.3)

`PrismPdfObject` is an independently owned, read-only COS value. Catalog, page, indirect-object,
array-element, dictionary-value, and resolved-reference results are clones, so they remain valid
after their parent object is freed. `prismpdf_object_free` releases each clone. Byte views returned
for strings, names, and raw encoded stream data borrow only from that object handle.

| Function family | Purpose |
|-----------------|---------|
| `prismpdf_document_catalog_object` / `_page_object` / `_get_object` | Obtain owned entry objects. Missing/free indirect objects follow PDF §7.3.10 and produce a null COS object. |
| `prismpdf_document_resolve_object` | Follow an indirect-reference chain with the document's bounded cycle protection. |
| `prismpdf_object_kind` | Distinguish all ten direct/reference variants without numeric coercion. |
| `prismpdf_object_boolean` / `_integer` / `_real` / `_reference` | Read exact scalar values; a wrong variant returns `InvalidUse`. |
| `prismpdf_object_bytes` | Lend binary-safe string or name bytes. PDF names are not assumed to be UTF-8. |
| `prismpdf_object_array_len` / `_array_get` | Inspect arrays; returned elements are owned clones. |
| `prismpdf_object_dictionary_len` / `_dictionary_get` | Inspect dictionary or stream-dictionary entries using binary-safe name bytes. |
| `prismpdf_object_stream_raw` | Lend raw, still-encoded stream bytes; `/Length` is not trusted as the byte authority. |

Owned objects returned by inspection may also be cloned and edited. Constructors cover every COS
variant; arrays append cloned values, dictionaries/stream dictionaries set binary-safe keys, and a
new stream copies its raw encoded bytes. `PrismPdfEdit` ties changes to the live document handle used
to create it. Reusing the transaction with another document returns `InvalidUse`.

| Edit operation | Purpose |
|----------------|---------|
| `prismpdf_edit_new(doc)` / `_free(edit)` | Start or abandon an owned transaction. The source document must remain live. |
| `prismpdf_edit_set_object(edit, number, generation, value)` | Clone a changed or new indirect value; the last assignment to an identity wins. |
| `prismpdf_edit_commit(edit, doc, Incremental, out_report)` | Append only changed objects (§7.5.6), retaining original signed ranges and structure bytes. |
| `prismpdf_edit_commit(edit, doc, FullRewrite, out_report)` | Normalize the graph with replacements, invalidating existing signature coverage while preserving the structure graph. |

A successful commit consumes the edit and returns an owned `PrismPdfTransformReport`. A validation
or serialization failure leaves it caller-owned, so it can be corrected or explicitly freed.

### Write & transform path (M2, EPIC 10)

Every function here serialises a **new** document into `*out_data`/`*out_len` and leaves the input
handle untouched — the boundary is immutable, so a binding never has to reason about aliasing.

| Function | Purpose |
|----------|---------|
| `prismpdf_document_save(doc, out_data, out_len)` | Full rewrite with a classic xref table (§7.5.4); normalises and repairs. |
| `prismpdf_document_save_compact(doc, out_data, out_len)` | Full rewrite with a cross-reference *stream* (§7.5.8, PDF 1.5+). |
| `prismpdf_document_save_encrypted(doc, user_password, user_len, owner_password, owner_len, algorithm, out_data, out_len)` | Encrypted full rewrite (§7.6). `algorithm`: `0` = RC4-128, `1` = AES-128, `2` = AES-256 (any other value → `PrismPdfStatus_NullArgument`). An empty owner password defaults to the user password. |
| `prismpdf_document_extract_pages(doc, indices, count, out_data, out_len)` | New document with only the 0-based `indices`, in the given order (§7.7.3; duplicates allowed) — split, subset and reorder. |
| `prismpdf_document_rotate_page(doc, index, degrees, out_data, out_len)` | New document with page `index` rotated by `degrees` (a multiple of 90, §7.7.3.3). |
| `prismpdf_merge(docs, count, out_data, out_len)` | Concatenate `count` live handles, in order, into one new document (§7.7.3). |
| `prismpdf_bytes_free(data, len)` | Release a byte buffer the library returned. |

The additive `*_report` variants for classic/compact/packed/version-targeted save, rotate,
extract-pages, merge, form-fill/flatten, and font subsetting return an owned
`PrismPdfTransformReport` instead of a bare allocation. Its byte view is borrowed until
`prismpdf_transform_report_free`; the report separately exposes:

- `Incremental`, `FullRewrite`, or `Reconstructed` serialization;
- `Preserved`, `Invalidated`, or `Removed` source-signature effects;
- `Preserved`, `Invalidated`, or `Removed` logical-structure effects.

The original byte-returning functions remain the concise compatibility path.

### Declarative composition (M25, Phase 5 complete)

Composition uses two opaque handle types. `PrismPdfComposition` owns an arena; every
`PrismPdfCompositionContainer` carries the arena identity, a stable slot id, and its generation.
Filling a slot consumes that container generation. Releasing the composition invalidates surviving
container handles without leaving them pointing at freed memory. Build is one-way finalisation on
both success and failure; later mutation/build calls return `PrismPdfStatus_InvalidUse`.

The current lifecycle/protocol slice exposes:

| Function | Purpose |
|----------|---------|
| `prismpdf_composition_new` / `_free` | Create/release an owned composition arena. |
| `prismpdf_composition_add_page` | Add page geometry and return its empty content slot. |
| `prismpdf_composition_page_set_header` / `_footer` | Add a repeating region to a page design; text expands `{page}` and `{pages}`. |
| `prismpdf_composition_set_tagged_language` | Enable tagged output and set the document language. |
| `prismpdf_composition_container_set_column` | Consume a slot as a column and return the new-generation column handle. |
| `prismpdf_composition_column_add_item` | Append and return an empty child slot. |
| `prismpdf_composition_container_set_row` | Consume a slot as a row and return its append handle. |
| `prismpdf_composition_row_add_fixed` / `_relative` / `_auto` | Append a row child with the corresponding width policy. |
| `prismpdf_composition_container_set_padding` / `_alignment` / `_width` / `_height` / `_extend` | Consume a slot as a one-child layout decorator and return the empty child. |
| `prismpdf_composition_container_set_border` / `_background` | Consume a slot as a painted decorator using `PrismPdfCompositionColor`. |
| `prismpdf_composition_container_set_semantic` / `_heading` / `_link` / `_figure` | Wrap a child in the corresponding §14.7–§14.8 logical-structure role. |
| `prismpdf_composition_container_set_table` | Consume a slot as a paginating element-tree table. |
| `prismpdf_composition_table_add_fixed_column` / `_relative_column` / `_auto_column` | Add a table column width policy. |
| `prismpdf_composition_table_set_header` / `_add_row` / `_row_add_cell` | Construct repeating header and body row cell trees using stable handles. |
| `prismpdf_composition_container_set_image` | Clone an existing `PrismPdfImageSource` into fit, fill, or exact-size layout. |
| `prismpdf_composition_container_set_text` | Consume a slot as wrapping Helvetica text. |
| `prismpdf_composition_container_set_page_break` | Consume a slot as an explicit break. |
| `prismpdf_composition_build` | Finalise once and return owned PDF bytes. |
| `prismpdf_composition_container_free` | Release only the scoped handle; the arena retains its node. |

The complete Phase 4 element set is projected. A compiled C fixture includes this generated header,
builds the multipage acceptance invoice without Rust callbacks, and is round-trip checked by the
`pdf-ffi` test harness. This closes the Phase 5 Rust/C capability-parity gate.

### Annotations (§12.5)

| Function | Purpose |
|----------|---------|
| `prismpdf_page_annotations(doc, index, out_list)` | Read page `index`'s annotations into an owned list. A page with no `/Annots` yields an empty list, not an error. |
| `prismpdf_annotation_list_len(list, out_len)` | Item count. |
| `prismpdf_annotation_list_get(list, index, out_item)` | Lend item `index`; past the end is `NotFound`. |
| `prismpdf_annotation_list_free(list)` | Release the list. |
| `prismpdf_annotation_subtype(annot, out_text)` | `/Subtype` — `Link`, `Text`, `Widget`, `Highlight`, … |
| `prismpdf_annotation_rect(annot, out_rect)` | `/Rect` as four `double`s `[llx lly urx ury]`. |
| `prismpdf_annotation_contents(annot, out_text)` | `/Contents`, or `NotFound`. |
| `prismpdf_annotation_uri(annot, out_text)` | The URI of a link with a URI action (§12.6.4.7), or `NotFound`. |
| `prismpdf_annotation_dest_page(annot, out_index)` | 0-based target page of an in-document link (§12.3.2), or `NotFound`. |

### Interactive forms (§12.7)

| Function | Purpose |
|----------|---------|
| `prismpdf_document_form_fields(doc, out_list)` | Read terminal form fields into an owned list; empty when there is no AcroForm. |
| `prismpdf_form_field_list_len(list, out_len)` | Item count. |
| `prismpdf_form_field_list_get(list, index, out_item)` | Lend item `index`. |
| `prismpdf_form_field_list_free(list)` | Release the list. |
| `prismpdf_form_field_name(field, out_text)` | Fully-qualified field name (§12.7.3.2). |
| `prismpdf_form_field_type(field, out_text)` | `/FT` — `Tx`, `Btn`, `Ch`, `Sig`; empty when unknown. |
| `prismpdf_form_field_value(field, out_text)` | Current `/V` as text, or `NotFound` when unset or non-textual. |
| `prismpdf_document_fill_form(doc, names, values, count, out_data, out_len)` | Fill fields by name and re-emit as an incremental update (§7.5.6). `names`/`values` are parallel C-string arrays; unknown names are ignored. |
| `prismpdf_document_flatten_form(doc, out_data, out_len)` | Stamp widget appearances into page content, drop `/AcroForm`, return the rewritten PDF. |

### Outline / bookmarks (§12.3.3)

| Function | Purpose |
|----------|---------|
| `prismpdf_document_outline(doc, out_list)` | Read the outline tree's top level; empty without `/Outlines`. |
| `prismpdf_outline_list_len(list, out_len)` | Top-level entry count. |
| `prismpdf_outline_list_get(list, index, out_item)` | Lend top-level entry `index`. |
| `prismpdf_outline_list_free(list)` | Release the tree — every borrowed entry, nested ones included, dies with it. |
| `prismpdf_outline_item_title(item, out_text)` | `/Title` (§7.9.2.2). |
| `prismpdf_outline_item_dest_page(item, out_index)` | 0-based destination page, or `NotFound` when it does not resolve. |
| `prismpdf_outline_item_child_count(item, out_len)` | Number of directly nested bookmarks. |
| `prismpdf_outline_item_child(item, index, out_child)` | Lend child `index`, borrowed from the same allocation as its parent. |

### Embedded files (§7.11)

| Function | Purpose |
|----------|---------|
| `prismpdf_document_attachments(doc, out_list)` | Read the `/EmbeddedFiles` name tree (§7.7.4), decoding each file through its filter chain. |
| `prismpdf_attachment_list_len` / `_get` / `_free` | Standard collection triple. |
| `prismpdf_attachment_name(att, out_text)` | File name — `/UF` preferred, else `/F`, else the name-tree key. |
| `prismpdf_attachment_data(att, out_data, out_len)` | **Borrowed** view of the decoded bytes. |
| `prismpdf_attachment_mime(att, out_text)` | `/EmbeddedFile /Subtype`, or `NotFound`. |
| `prismpdf_attachment_relationship(att, out_text)` | `/AFRelationship` (§14.13), or `NotFound`. |
| `prismpdf_attachment_description(att, out_text)` | `/Desc`, or `NotFound`. |

### Fonts (§9.5–§9.7, §9.9)

| Function | Purpose |
|----------|---------|
| `prismpdf_document_fonts(doc, out_list)` | Every font the pages reference, with its embedded program where present. |
| `prismpdf_font_list_len` / `_get` / `_free` | Standard collection triple. |
| `prismpdf_font_base_font(font, out_text)` | `/BaseFont`, often with a subset tag like `ABCDEF+`. |
| `prismpdf_font_subtype(font, out_text)` | `/Subtype` — `Type1`, `TrueType`, `Type0`, … |
| `prismpdf_font_program_format(font, out_format)` | `PrismPdfFontFormat`, or `NotFound` when not embedded — the PDF/A pre-flight check. |
| `prismpdf_font_program(font, out_data, out_len)` | **Borrowed** program bytes, or `NotFound`. |
| `prismpdf_font_metrics(font, out_units_per_em, out_glyph_count)` | Parsed sfnt metrics, or `NotFound` for Type1/CFF and unparseable programs. |
| `prismpdf_font_family_name(font, out_text)` | Family name from the program, or `NotFound`. |
| `prismpdf_document_subset_fonts(doc, out_data, out_len)` | Subset every embedded font to the glyphs actually used; returns a new PDF. |

`PrismPdfFontFormat`: `Type1` = 0, `TrueType` = 1, `Cff` = 2, `OpenType` = 3.

### Images (§8.6, §8.9)

| Function | Purpose |
|----------|---------|
| `prismpdf_page_images(doc, index, out_list)` | Images page `index` draws, recursing into form XObjects (§8.10). |
| `prismpdf_image_list_len` / `_get` / `_free` | Standard collection triple. |
| `prismpdf_image_info(img, out_width, out_height, out_bits_per_component)` | `/Width`, `/Height`, `/BitsPerComponent`. |
| `prismpdf_image_color_space(img, out_space)` | `PrismPdfColorSpace`. |
| `prismpdf_image_components(img, out_components)` | Components per sample — needed to walk `Raw` bytes, and the only way to size an `Other` space. |
| `prismpdf_image_kind(img, out_kind)` | How the payload is encoded. |
| `prismpdf_image_data(img, out_data, out_len)` | **Borrowed** payload: decoded samples for `Raw`, a complete container file otherwise. |

`PrismPdfColorSpace`: `DeviceGray` = 0, `DeviceRgb` = 1, `DeviceCmyk` = 2, `Other` = 3.
`PrismPdfImageKind`: `Raw` = 0, `Jpeg` = 1, `Jpeg2000` = 2, `Jbig2` = 3.

### Metadata (§14.3) and positioned text (§9.4)

| Function | Purpose |
|----------|---------|
| `prismpdf_document_xmp(doc, out_text)` | The XMP packet (§14.3.2) as raw XML, or `NotFound`. |
| `prismpdf_document_info(doc, key, out_text)` | One `/Info` entry by key, decoded per §7.9.2.2 — so UTF-16BE and PDF 2.0 UTF-8 values come back as UTF-8. `NotFound` when absent or non-string. |
| `prismpdf_document_creation_date(doc, out_date)` | `/CreationDate` as a `PrismPdfDate`, or `NotFound`. |
| `prismpdf_document_modification_date(doc, out_date)` | `/ModDate` as a `PrismPdfDate`, or `NotFound`. |
| `prismpdf_page_text_positioned(doc, index, out_text)` | Text with layout preserved — line breaks and gaps from the text matrix, rather than the reading-order run `prismpdf_page_text` returns. |

`PrismPdfDate` is a `#[repr(C)]` struct: `year` (`uint16_t`), `month`/`day`/`hour`/`minute`/`second`
(`uint8_t`), `has_utc_offset` (`bool`) and `utc_offset_minutes` (`int16_t`). §7.9.4 permits a date
that declares no relationship to UTC; that is `has_utc_offset == false`, and the offset field then
carries no meaning.

### Access permissions (§7.6.3.2)

`Permissions` is a newtype over the `/P` flag word, so it crosses as a raw `int32_t` composed by
these functions: start from `restricted` (nothing allowed) or `all`, then grant one operation at a
time. Each returns the widened word.

| Function | Grants |
|----------|--------|
| `prismpdf_permissions_restricted()` | nothing — the starting point |
| `prismpdf_permissions_all()` | everything (`-1`) |
| `prismpdf_permissions_allow_print(p)` | printing (bit 3) |
| `prismpdf_permissions_allow_modify(p)` | modifying contents (bit 4) |
| `prismpdf_permissions_allow_copy(p)` | copying text and graphics (bit 5) |
| `prismpdf_permissions_allow_annotate(p)` | adding/modifying annotations (bit 6) |
| `prismpdf_permissions_allow_fill_forms(p)` | filling form fields (bit 9) |
| `prismpdf_permissions_allow_accessibility(p)` | extraction for accessibility (bit 10) |
| `prismpdf_permissions_allow_assemble(p)` | insert/rotate/delete pages (bit 11) |
| `prismpdf_permissions_allow_print_high_res(p)` | full-quality printing (bit 12) |

Note that granting all eight yields `-4`, not `Permissions::ALL` (`-1`): `ALL` also sets reserved
bits 1–2, which §7.6.3.2 requires to be zero. Both are accepted on write; the composed word is the
spec-shaped one.

### Encryption, completed (§7.6)

| Function | Purpose |
|----------|---------|
| `prismpdf_document_save_encrypted_with(doc, user, user_len, owner, owner_len, permissions, encrypt_metadata, algorithm, out_data, out_len)` | The complete form of `prismpdf_document_save_encrypted`, which always grants everything. `encrypt_metadata` false leaves `/Metadata` in clear text, as PDF/A requires. |
| `prismpdf_document_save_encrypted_public_key(doc, certs, cert_lens, count, permissions, encrypt_metadata, algorithm, out_data, out_len)` | Certificate encryption (§7.6.5): any recipient's private key opens the result. |
| `prismpdf_document_save_encrypted_with_mac(…)` | As above plus a **PDF MAC** (ISO/TS 32004), so tampering is detectable rather than merely undecryptable. |
| `prismpdf_document_verify_pdf_mac(doc, password, password_len, out_valid)` | Verify the MAC. `NotFound` means the document carries none — an unprotected file, not a failure. |

`algorithm`: `0` = RC4-128, `1` = AES-128, `2` = AES-256, **`3` = AES-256-GCM** (newly exposed;
the original `prismpdf_document_save_encrypted` now accepts it too).

### Signing (§12.8)

Signing takes more optional inputs than one C signature can carry, so `SignSettings` crosses as a
**mutable handle** — the first in the ABI, and the pattern `Builder` will reuse.

| Function | Purpose |
|----------|---------|
| `prismpdf_sign_settings_new()` / `_free(settings)` | Create/release the handle. |
| `prismpdf_sign_settings_set_name` / `_reason` / `_location` / `_contact_info` | The `/Name`, `/Reason`, `/Location`, `/ContactInfo` entries (§12.8.1). |
| `prismpdf_sign_settings_set_signing_time(settings, unix_time)` | Pin the clock instead of reading it — what a reproducible build or a test needs. |
| `prismpdf_sign_settings_set_pades(settings, pades)` | Produce a PAdES (ETSI EN 319 142) signature. |
| `prismpdf_sign_settings_set_appearance(settings, page_index, rect, text)` | A visible widget on `page_index` at `rect` (4 floats), optionally captioned. Null `text` gives an unlabelled box. |
| `prismpdf_sign_settings_set_timestamp(settings, cert, cert_len, key, key_len, gen_time, serial)` | Embed a signature timestamp (§12.8.3.3). |
| `prismpdf_document_sign(doc, cert, cert_len, key, key_len, out_data, out_len)` | Sign with DER certificate + key, returning an incremental update (§7.5.6). |
| `prismpdf_document_sign_with(…, settings, …)` | Sign with the settings above. |
| `prismpdf_document_sign_with_mac(…, settings, password, password_len, …)` | Sign an encrypted document and refresh its PDF MAC in the same revision. |
| `prismpdf_document_timestamp(doc, tsa_cert, cert_len, tsa_key, key_len, gen_time, has_gen_time, out_data, out_len)` | A document timestamp (§12.8.5) — proof of existence, no signer identity. `has_gen_time` false takes the current clock. |

### Verification (§12.8, §12.8.4)

| Function | Purpose |
|----------|---------|
| `prismpdf_document_verify_signatures(doc, out_list)` | Integrity only: CMS validity and byte coverage. Trust is not evaluated. |
| `prismpdf_document_verify_signatures_with(doc, roots, root_lens, count, out_list)` | …plus trust against DER root certificates, so `trusted` becomes meaningful. |
| `prismpdf_document_verify_signatures_ltv(doc, roots, root_lens, count, out_list)` | …plus long-term validation: DSS revocation material feeds the check. |
| `prismpdf_signature_list_len` / `_get` / `_free` | Standard collection triple. |
| `prismpdf_signature_valid(sig, out_valid)` | CMS verifies and covers what it claims. **Integrity, not trust** — a self-signed certificate can be valid. |
| `prismpdf_signature_signer(sig, out_text)` | Signer DN, or `NotFound`. |
| `prismpdf_signature_covered_bytes(sig, out_bytes)` | Compare against file length to detect content appended after signing. |
| `prismpdf_signature_signing_time(sig, out_time)` | Claimed signing time (Unix), or `NotFound`. |
| `prismpdf_signature_timestamp_time(sig, out_time)` | Timestamp-token time, or `NotFound`. |
| `prismpdf_signature_trusted(sig, out_trusted)` | `NotFound` = trust was never evaluated; `Ok` + `false` = evaluated and *not* trusted. The distinction matters. |
| `prismpdf_signature_pades(sig, out_pades)` | Whether it is a PAdES signature. |
| `prismpdf_signature_revocation(sig, out_revocation)` | `PrismPdfRevocation`, or `NotFound` when revocation was not evaluated or no chain could be built. |

`PrismPdfRevocation`: `Good` = 0, `Revoked` = 1, `Incomplete` = 2.

### Authoring: content streams (§8.2–§8.6, §9.4)

`Content` is a byte builder for a page's operator stream, so it crosses as a mutable handle. All
40 operators are exposed as `prismpdf_content_<operator>`; each takes numbers, strings or slices and
returns a status.

| Function | Purpose |
|----------|---------|
| `prismpdf_content_new()` / `_free(content)` | Create/release the handle. |
| `prismpdf_content_bytes(content, out_data, out_len)` | **Borrowed** view of the assembled operators — pass straight to `prismpdf_builder_add_page`. |
| `_save` `_restore` `_transform` `_set_line_width` | Graphics state (§8.4). |
| `_set_fill_gray` `_set_stroke_gray` `_set_fill_rgb` `_set_stroke_rgb` `_set_fill_cmyk` `_set_fill_color_space` `_set_fill_color` | Colour (§8.6). |
| `_move_to` `_line_to` `_curve_to` `_rect` `_close_path` `_stroke` `_fill` `_fill_and_stroke` | Paths (§8.5). |
| `_begin_text` `_end_text` `_set_font` `_set_char_spacing` `_set_word_spacing` `_set_leading` `_text_move` `_set_text_matrix` `_next_line` `_show_text` `_show_str` `_show_glyphs` | Text (§9.3–§9.4). |
| `_do_xobject` `_inline_image` | Images and forms (§8.8, §8.9.7). |
| `_begin_marked_content` `_begin_af_marked_content` `_begin_artifact` `_end_marked_content` | Marked content (§14.6, §14.8.2.2). |

### Authoring: the document builder (§7.7)

`Builder` mutates in place and `build(&self)` does not consume it, so it crosses as a mutable
handle and can be built repeatedly.

| Function | Purpose |
|----------|---------|
| `prismpdf_builder_new()` / `_free(builder)` | Create/release the handle. |
| `prismpdf_builder_build(builder, out_data, out_len)` | Serialise, stamping the minimum version the content requires (§7.5.2). |
| `prismpdf_builder_build_for(builder, major, minor, …)` | Serialise at an exact target version; constructs above it are **refused**, not downgraded. |
| `prismpdf_builder_add_page(builder, content, content_len, font_names, fonts, font_count)` | Append a page. `font_names`/`fonts` are parallel arrays naming Standard-14 fonts in `/Resources /Font`. |
| `prismpdf_page_spec_new(content)` / `_free(page)` | Copy a content stream into an owned precision page description. |
| `prismpdf_page_spec_set_media_box(page, …)` | Set a page-specific box instead of the builder default. |
| `prismpdf_page_spec_add_standard_font` / `_add_embedded_font` / `_add_image` | Add exactly the named resources referenced by the page's content. Image sources are copied and remain caller-owned. |
| `prismpdf_builder_add_page_spec(builder, page)` | Transfer the page to the builder. A successful call consumes the page handle; a null-argument rejection leaves it caller-owned. |
| `prismpdf_builder_set_media_box(builder, media_box)` | Default page box, 4 doubles (§7.7.3.3). |
| `prismpdf_builder_set_version(builder, major, minor)` | Pin the header version — a floor, never below what the content needs. |
| `prismpdf_builder_set_title` / `_author` / `_subject` / `_keywords` / `_creator` | `/Info` conveniences (§14.3.3). |
| `prismpdf_builder_set_info(builder, key, value)` / `_clear_info(builder)` | Arbitrary `/Info` entry; clear all (PDF/A-4 and 2.0 prefer XMP alone). |
| `prismpdf_builder_set_metadata_xmp(builder, xmp, len)` | The `/Metadata` packet (§14.3.2). |
| `prismpdf_builder_set_lang(builder, code)` | `/Lang` (§14.9.2) — PDF/UA requires it. |
| `prismpdf_builder_set_display_doc_title(builder, on)` | `/ViewerPreferences /DisplayDocTitle` (§12.2) — PDF/UA requires it. |
| `prismpdf_builder_set_file_id(builder, id, len)` | Permanent `/ID` element 1 (§14.4). |
| `prismpdf_builder_set_utf8_text_strings(builder)` | UTF-8 text strings (§7.9.2.2, PDF 2.0). |
| `prismpdf_builder_add_outline(builder, title, page_index)` | A top-level bookmark (§12.3.3). |
| `prismpdf_builder_attach_file(builder, name, mime, relationship, description, data, data_len)` | Embed a file (§7.11); `description` may be null. |

`PrismPdfStdFont`: `Helvetica` = 0 … `ZapfDingbats` = 13, in the order of §9.6.2.2.
`Content` + `PageSpec` + `Builder` is the low-level precision/escape-hatch layer below `Flow` and
declarative `Composition`: callers own operator order and resource names; the higher layers own
measurement and pagination.

### Raw logical structure (§14.7)

`PrismPdfStructNode` projects the facade's arbitrary `StructElem` tree for expert Tagged-PDF
authoring. A node owns its children, attributes, references, and associated files. Adding a child
to a parent or a top-level node to a builder transfers ownership only after argument validation;
successful transfer consumes the child handle.

| Function family | Purpose |
|-----------------|---------|
| `prismpdf_struct_node_new` / `_free` | Create or release an untransferred element with an arbitrary `/S` tag. |
| `_set_alt` / `_set_actual_text` / `_set_lang` / `_set_namespace` / `_set_id` | Set accessibility, language, PDF 2.0 namespace, and ID-tree properties. |
| `_add_reference` | Add an element-ID `/Ref` relation; IDs resolve when the builder emits the tree. |
| `_add_name_attribute` / `_add_integer_attribute` / `_add_text_attribute` | Build arbitrary owner-grouped `/A` dictionaries. |
| `_add_content` / `_add_widget` / `_add_annotation` | Add MCID `/MCR` or annotation `/OBJR` children in reading order. |
| `_add_child` | Transfer a nested element into its parent. |
| `_associate_file` | Add a structure-associated file (`/AF`, PDF 2.0). |
| `prismpdf_builder_add_structure_node` | Transfer one top-level element below the implicit `Document` root. |
| `prismpdf_builder_set_structure_namespace` | Namespace the implicit root for PDF 2.0/PDF/UA-2. |

The content stream must contain matching `BDC`/`EMC` sequences, authored with
`prismpdf_content_begin_marked_content`. Declarative composition semantic wrappers remain the
preferred path when an application does not need direct MCID and object-reference control.

### Authoring: annotations and fields — the flattening rule

`AnnotationSpec`, `FormFieldSpec` and `LinkTarget` are Rust enums **carrying payloads**, which C
cannot represent. Rather than invent spec handles with move semantics that a C caller would have to
track (was it consumed? may I still free it?), the **shallow** enums are flattened: one entry point
per variant, taking that variant's fields directly. There is no intermediate object and therefore
no ownership question.

Handles are reserved for the one place recursion makes them unavoidable — the structure tree
(`StructElem` / `StructKid`), which is not yet exposed.

| Function | Variant |
|----------|---------|
| `prismpdf_builder_add_link_uri(builder, page_index, rect, uri, contents)` | `Link` + `LinkTarget::Uri` (§12.6.4.7) |
| `prismpdf_builder_add_link_page(builder, page_index, rect, target_page, contents)` | `Link` + `LinkTarget::Page` (§12.3.2) |
| `prismpdf_builder_add_link_element(builder, page_index, rect, element_id, contents)` | `Link` + `LinkTarget::Element` — a structure destination (§12.3.2.2, PDF 2.0) |
| `prismpdf_builder_add_link_document_part(builder, page_index, rect, part_index, contents)` | `Link` + `LinkTarget::DocumentPart` (§14.12, PDF 2.0) |
| `prismpdf_builder_add_note(builder, page_index, rect, contents)` | `Note` (§12.5.6.4) |
| `prismpdf_builder_add_checkbox(builder, page_index, rect, name, checked, tooltip)` | `FormFieldSpec::Checkbox` (§12.7.4.2.3) |

In each, `rect` is 4 doubles `[llx lly urx ury]` and `contents`/`tooltip` may be null. A non-null
argument that is not valid UTF-8 is **refused** (`PrismPdfStatus_NullArgument`) rather than lossily
substituted.

### Conformance production: PDF/A and PDF/UA (§14, ISO 19005 / 14289)

These finalise a `Builder`, so they arrive with authoring. A conformance failure is **not** a parse
failure: it returns `PrismPdfStatus_Conformance` and writes the specific broken rule to `out_issue`,
so a caller learns *which* rule failed rather than only that something did. Pass a null `out_issue`
if you do not want it.

| Function | Purpose |
|----------|---------|
| `prismpdf_builder_make_pdfa(builder, conformance, meta, out_issue)` | Finalise as PDF/A: XMP, an sRGB OutputIntent (§14.11.5), a file `/ID`. |
| `prismpdf_builder_make_pdfa_with_output_intent(builder, conformance, meta, icc, icc_len, n, identifier, out_issue)` | …with a caller-chosen ICC profile — e.g. a CMYK printing condition so `DeviceCMYK` content is conformant under §6.2.4.3. `n` is 1 = Gray, 3 = RGB, 4 = CMYK. |
| `prismpdf_builder_make_pdfua(builder, meta, lang, out_issue)` | Finalise as PDF/UA-1 (ISO 14289-1). |
| `prismpdf_builder_make_pdfua2(builder, meta, lang, out_issue)` | Finalise as PDF/UA-2 (ISO 14289-2, on PDF 2.0) — stricter: no `Note`, no generic `H`, attachments need descriptions. |
| `prismpdf_builder_set_output_intent(builder, icc, icc_len, n, identifier)` | Set an OutputIntent directly, without a conformance pass. |
| `prismpdf_pdfa_part(conformance)` | The ISO 19005 part: 1, 2, 3 or 4. |
| `prismpdf_pdfa_allows_attachments(conformance)` | Whether embedded files are permitted — only part 3 and 4f (§6.8). Check before attaching. |
| `prismpdf_pdfa_code(conformance, out_text)` | The XMP conformance code, e.g. `2u`. |

`PrismPdfPdfAConformance`: `A1b` = 0, `A1a` = 1, `A2b` = 2, `A2u` = 3, `A2a` = 4, `A3b` = 5,
`A3u` = 6, `A3a` = 7, `A4` = 8, `A4e` = 9, `A4f` = 10.

`PrismPdfConformanceIssue`: `UnembeddedFont` = 0, `AttachmentRequiresPdfA3` = 1,
`LevelARequiresTagging` = 2, `TransparencyRequiresPdfA2` = 3, `NotTagged` = 4, `MissingTitle` = 5,
`MissingLanguage` = 6, `FigureWithoutAlt` = 7, `NoteForbidden` = 8, `GenericHeadingForbidden` = 9,
`AttachmentWithoutDesc` = 10, `LinkWithoutStructureDest` = 11, `UnknownStructureType` = 12,
`NotdefGlyph` = 13.

### XMP metadata (§14.3.2)

A mutable handle feeding the conformance passes.

| Function | Purpose |
|----------|---------|
| `prismpdf_xmp_metadata_new()` / `_free(meta)` | Create/release. |
| `prismpdf_xmp_metadata_set_title` / `_subject` / `_keywords` / `_creator_tool` / `_producer` / `_create_date` / `_modify_date` | Single-value fields. |
| `prismpdf_xmp_metadata_add_author(meta, author)` | Append an author — `dc:creator` is the one list-valued field, so call it repeatedly. |

### High-level layout (§9.4)

The most binding-friendly API in the engine: pour content and let it break pages. Three things
needed adapting for C, and the adaptations are worth knowing:

- **`TextBlock<'a>` borrows its font names**, which DESIGN.md §6.4 forbids at an FFI point. The C
  handle **owns** its strings and materialises the borrowed view per call.
- **`Table`'s builder methods take `self` by value.** `Table` is `Clone`, so each setter clones,
  applies and stores back. Invisible to the caller, who just mutates a handle.
- **`Flow::build` and `into_builder` consume the flow.** So do their C counterparts: after either
  call the handle is dead, exactly like `fclose`. Do **not** free it again.

| Function | Purpose |
|----------|---------|
| `prismpdf_text_block_new(font_resource, base_font, size, leading, align)` / `_free` | A text style. `font_resource` names the page resource; `base_font` is the PostScript name used for metrics. |
| `prismpdf_measure_text(block, text, out_width)` | Rendered width in points. `NotFound` when `base_font` is not a Standard-14 — no built-in metrics, not a failure. |
| `prismpdf_wrap_text(block, text, width, out_list)` | Word-wrap to a column; returns a `PrismPdfStringList`. |
| `prismpdf_image_source_from_jpeg` / `_from_rgb` / `_from_gray` / `_from_rgba` | An image to **place** — distinct from `PrismPdfImage`, which is one *extracted*. `_from_rgba` turns alpha into an `/SMask` (§11.6.5.2). Return null on a bad length or unusable JPEG. |
| `prismpdf_image_source_size` / `_free` | Pixel dimensions; release. |
| `prismpdf_table_new(columns, count)` / `_free` | A table over fixed column widths in points. |
| `prismpdf_table_set_font` / `_size` / `_leading` / `_padding` / `_border` / `_align` / `_header_row` | Table style. A header row repeats on each page. |
| `prismpdf_table_add_row(table, cells, count)` | Append a row, in column order. |
| `prismpdf_flow_new(size, margins, font_names, fonts, font_count)` / `_free` | A flow over a page size (`[w h]`) and margins (`[top right bottom left]`). |
| `prismpdf_flow_build(flow, out_data, out_len)` | **Consumes** the flow and serialises. |
| `prismpdf_flow_into_builder(flow, out_builder)` | **Consumes** the flow into a `PrismPdfBuilder` — the composition point for running a conformance pass, attaching files or adding annotations afterwards. |
| `prismpdf_flow_set_tagged(flow, lang)` | Emit logical structure — the prerequisite for PDF/UA and PDF/A level A. |
| `prismpdf_flow_embed_font(flow, resource, program, len)` | Replace a Standard-14 font with a real program; `Parse` when it is not a usable sfnt. |
| `prismpdf_flow_set_title` / `_author` / `_set_info` | Document metadata. |
| `prismpdf_flow_set_header` / `_set_footer` | Running header and footer, drawn as artifacts. |
| `prismpdf_flow_text` / `_heading` / `_list` / `_table` | Body content. `_heading` tags `H1`…`H6`. |
| `prismpdf_flow_image` / `_image_fit` | Place an image as an untagged artifact. |
| `prismpdf_flow_figure` / `_figure_fit` / `_figure_with_caption` | Place it as a **tagged `Figure`** with alt text — the difference between decoration and an accessible document (PDF/UA §7.3). Alt text is required, not optional. |
| `prismpdf_flow_note` / `_fenote` | Footnotes. `Note` for PDF/UA-1; `FENote` for UA-2, which forbids `Note` (14289-2 §8.2.5.14). |
| `prismpdf_flow_title_element` / `_formula` | A tagged `Title`; a tagged `Formula` carrying `/ActualText`. |
| `prismpdf_flow_add_bookmark` / `_space` / `_page_break` | Navigation and spacing. |
| `prismpdf_flow_page_count` / `_cursor_y` | Where the flow has got to — for deciding whether the next block fits. |

`PrismPdfAlign`: `Left` = 0, `Center` = 1, `Right` = 2, `Justify` = 3.
`PrismPdfListStyle`: `Bullet` = 0, `Numbered` = 1.

### Remaining `Document` entry points

| Function | Purpose |
|----------|---------|
| `prismpdf_document_open_with_limits(data, len, limits, out_doc)` | Open with explicit anti-DoS limits (DESIGN.md §3.5) — the knob a service parsing untrusted uploads needs. A null `limits`, or a zero field, means the default. |
| `prismpdf_document_open_with_private_key(data, len, cert, cert_len, key, key_len, out_doc)` | Open a certificate-encrypted document (§7.6.5). |
| `prismpdf_document_min_version(doc, out_major, out_minor)` | The **minimum** version the content requires, which can be below the declared header version. |
| `prismpdf_document_save_as(doc, major, minor, …)` | Full rewrite at an exact version, refusing constructs above it. |
| `prismpdf_document_save_packed(doc, …)` | Full rewrite using object streams (§7.5.7) — the smallest of the three save modes. |
| `prismpdf_document_structure_namespaces(doc, out_list)` | Declared structure namespaces (§14.7.4). |
| `prismpdf_document_signature_vri_keys(doc, out_list)` | `/VRI` keys in the DSS (§12.8.4.3). |

`PrismPdfLimits` is a `#[repr(C)]` struct of three `uintptr_t`: `max_depth`, `max_objstm_objects`,
`max_objects`.

### String lists

`Vec<String>` returns cannot lend borrowed views the way byte payloads do — a C string needs a NUL
terminator the Rust `String` does not carry — so `prismpdf_string_list_get` **copies**.

| Function | Purpose |
|----------|---------|
| `prismpdf_string_list_len(list, out_len)` | Entry count. |
| `prismpdf_string_list_get(list, index, out_text)` | Copy entry `index` as an owned C string — release with `prismpdf_string_free`. |
| `prismpdf_string_list_free(list)` | Release the list. |

### Not yet crossing

The authoring core is in place; these remain before the C ABI reaches full parity with the facade:

- **Namespace role maps and schemas** (`RoleMapEntry`, namespace schema attachments). Arbitrary raw
  structure trees, attributes, namespaces, references, MCIDs, OBJRs, and associated files are
  available through owned `PrismPdfStructNode` handles.
- **The rest of `Builder`**: form XObjects, `embed_cid_font`, the colour
  space constructors (`add_separation`, `add_icc_based`, `add_indexed`, `add_lab`), page labels,
  document parts (only *links* to them cross, via `prismpdf_builder_add_link_document_part`),
  developer extensions, encrypted payloads. Output intents cross via
  `prismpdf_builder_set_output_intent` and `prismpdf_builder_make_pdfa_with_output_intent`.
- **Bulk COS enumeration helpers**: `live_objects`, `page_content_bytes`, and `page_entries`.
  Catalog/page/object lookup, recursive inspection, mutable COS construction, and explicit
  incremental/full-rewrite transactions are available through owned object/edit handles.
- **Font helpers**: `subset_sfnt`, `glyphs_for_text`, `shape_text`, `draw_text_block`.
- **Name trees** (`names`) and `document_parts`.
- **Writing DSS validation material** (`add_validation_info`, `validation_info`) takes and returns
  `ValidationData`, `SignatureValidation` and `DssInfo`, none of which the `prismpdf` facade
  re-exports — a facade gap to close before the ABI can follow. LTV *verification* is unaffected and
works.

## Minimal C usage

```c
#include "prismpdf.h"

PrismPdfDocument *doc = NULL;
if (prismpdf_document_open(buf, len, &doc) != PrismPdfStatus_Ok) { /* handle error */ }

uintptr_t pages = 0;
prismpdf_document_page_count(doc, &pages);

char *text = NULL;
if (prismpdf_page_text(doc, 0, &text) == PrismPdfStatus_Ok) {
    puts(text);
    prismpdf_string_free(text);
}

/* Keep only the first page, then hand the bytes back. */
uintptr_t keep[] = { 0 };
uint8_t *out = NULL;
uintptr_t out_len = 0;
if (prismpdf_document_extract_pages(doc, keep, 1, &out, &out_len) == PrismPdfStatus_Ok) {
    fwrite(out, 1, out_len, f);
    prismpdf_bytes_free(out, out_len);
}
prismpdf_document_free(doc);
```

Link against `libpdf_ffi.a` (static) or `libpdf_ffi.so`/`.dylib`/`.dll` (dynamic), both emitted by
the `pdf-ffi` crate (`crate-type = ["cdylib", "staticlib", "rlib"]`).

## Versioning policy

- The status-code numbering and existing function signatures are **append-only**: new codes and
  new functions may be added; existing ones are never renumbered or re-signatured.
- Breaking changes bump the major version and are recorded here and in `CHANGELOG.md`.

## Regenerating the header

```bash
cargo install cbindgen   # once
cd crates/pdf-ffi
cbindgen --config cbindgen.toml --crate prismpdf-ffi --output include/prismpdf.h
```

The generated `prismpdf.h` is committed so consumers don't need `cbindgen`; regenerate it whenever
the `extern "C"` surface changes.
