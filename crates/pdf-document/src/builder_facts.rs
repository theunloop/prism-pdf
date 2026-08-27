use super::*;

/// A neutral snapshot of authored document features used by standards and diagnostic layers.
///
/// This describes the document rather than encoding PDF/A or PDF/UA rules. A conformance layer
/// decides what each count, flag, or structure type means for its target standard.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct DocumentFacts {
    /// Number of named Standard-14 font resources across all pages (§9.6.2.2).
    pub standard_14_font_resources: usize,
    /// Number of embedded files across every attachment-capable authoring surface (§7.11.4).
    pub embedded_files: usize,
    /// Number of authored images carrying a soft mask (`/SMask`, §11.6.5.2).
    pub soft_mask_images: usize,
    /// Whether authored text is known to reference glyph zero (`.notdef`, §9.7.6.3).
    pub notdef_glyph_referenced: bool,
    /// Intra-document links targeting a page rather than a structure destination (§12.3).
    pub direct_page_links: usize,
    /// Files whose file specification would have no non-empty `/Desc` (§7.11.3), across every
    /// attachment-capable authoring surface.
    pub undescribed_files: usize,
    /// Logical structure elements in depth-first order (§14.7).
    pub structure_elements: Vec<StructureElementFact>,
    /// Namespace role mappings authored for the structure tree (§14.7.4).
    pub role_maps: Vec<RoleMapEntry>,
}

/// Conformance-relevant authored properties of one logical structure element (§14.7).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct StructureElementFact {
    /// Structure type (`/S`).
    pub tag: String,
    /// Explicit namespace URI (`/NS`); `None` means the default PDF 1.7 namespace.
    pub namespace: Option<String>,
    /// Whether `/Alt` is present and non-empty.
    pub has_alt: bool,
    /// Whether `/ActualText` is present and non-empty.
    pub has_actual_text: bool,
}

impl Builder {
    /// Capture the authored features needed by standards and diagnostic layers.
    #[must_use]
    pub fn facts(&self) -> DocumentFacts {
        let mut facts = DocumentFacts {
            standard_14_font_resources: self.pages.iter().map(|page| page.fonts.len()).sum(),
            soft_mask_images: self
                .pages
                .iter()
                .flat_map(|page| &page.images)
                .filter(|(_, image)| image.smask.is_some())
                .count(),
            notdef_glyph_referenced: self.notdef_reference,
            direct_page_links: self
                .annotations
                .iter()
                .filter(|(_, spec, _)| {
                    matches!(
                        spec,
                        AnnotationSpec::Link {
                            target: LinkTarget::Page(_),
                            ..
                        }
                    )
                })
                .count(),
            role_maps: self.role_maps.clone(),
            ..DocumentFacts::default()
        };

        fn is_undescribed(attachment: &Attachment) -> bool {
            attachment.description.as_deref().unwrap_or("").is_empty()
        }
        let files = self
            .attachments
            .iter()
            .chain(
                self.page_attachments
                    .iter()
                    .map(|(_, attachment)| attachment),
            )
            .chain(self.content_af_props.iter().flat_map(|(_, _, files)| files))
            .chain(self.annotations.iter().flat_map(|(_, _, files)| files))
            .chain(self.form_fields.iter().flat_map(|(_, _, files)| files))
            .chain(
                self.pages
                    .iter()
                    .flat_map(|page| &page.forms)
                    .flat_map(|form| &form.files),
            )
            .chain(self.ns_schemas.iter().map(|(_, attachment)| attachment));
        for file in files {
            facts.embedded_files += 1;
            facts.undescribed_files += usize::from(is_undescribed(file));
        }

        fn walk(element: &StructElem, facts: &mut DocumentFacts) {
            facts.structure_elements.push(StructureElementFact {
                tag: element.tag.clone(),
                namespace: element.ns.clone(),
                has_alt: !element.alt.as_deref().unwrap_or("").is_empty(),
                has_actual_text: !element.actual_text.as_deref().unwrap_or("").is_empty(),
            });
            facts.embedded_files += element.af.len();
            facts.undescribed_files += element.af.iter().filter(|a| is_undescribed(a)).count();
            for kid in &element.kids {
                if let StructKid::Child(child) = kid {
                    walk(child, facts);
                }
            }
        }
        for element in &self.structure {
            walk(element, &mut facts);
        }
        facts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attachment(name: &str, described: bool) -> Attachment {
        Attachment {
            name: name.to_string(),
            mime: "application/octet-stream".to_string(),
            relationship: "Data".to_string(),
            description: described.then(|| "description".to_string()),
            mod_date: None,
            data: vec![1],
        }
    }

    fn gray_image(smask: Option<Box<ImageXObject>>) -> ImageXObject {
        ImageXObject {
            width: 1,
            height: 1,
            color_space: ImageColorSpace::Gray,
            bits_per_component: 8,
            filter: None,
            data: vec![0],
            smask,
            mask: None,
            image_mask: false,
        }
    }

    #[test]
    fn snapshot_describes_features_and_every_attachment_surface() {
        let mut builder = Builder::new();
        builder.add_page(PageSpec::new(Vec::new()).standard_font("F1", StdFont::Helvetica));
        builder.pages[0].images.push((
            "Im0".to_string(),
            gray_image(Some(Box::new(gray_image(None)))),
        ));

        builder.attachments.push(attachment("catalog", false));
        builder.attachments.push(attachment("described", true));
        builder
            .page_attachments
            .push((0, attachment("page", false)));
        builder.content_af_props.push((
            0,
            "AF0".to_string(),
            vec![attachment("marked-content", false)],
        ));
        builder.annotations.push((
            0,
            AnnotationSpec::Link {
                rect: [0.0; 4],
                target: LinkTarget::Page(0),
                contents: None,
            },
            vec![attachment("annotation", false)],
        ));
        builder.form_fields.push((
            0,
            FormFieldSpec::Checkbox {
                rect: [0.0; 4],
                name: "check".to_string(),
                checked: false,
                tooltip: None,
            },
            vec![attachment("field", false)],
        ));
        builder.pages[0].forms.push(FormXObjectSpec {
            name: "Fx".to_string(),
            bbox: [0.0; 4],
            content: Vec::new(),
            files: vec![attachment("xobject", false)],
        });
        builder
            .ns_schemas
            .push(("urn:example".to_string(), attachment("schema", false)));

        let mut section = StructElem::new("Sect").associate_file(attachment("structure", false));
        section.push_child(StructElem::new("Figure").actual_text("replacement"));
        builder.structure.push(section);
        builder.role_maps.push(RoleMapEntry {
            ns: "urn:example".to_string(),
            custom: "Custom".to_string(),
            target: "P".to_string(),
            target_ns: None,
        });
        builder.notdef_reference = true;

        let facts = builder.facts();
        assert_eq!(facts.standard_14_font_resources, 1);
        assert_eq!(facts.embedded_files, 9);
        assert_eq!(facts.soft_mask_images, 1);
        assert!(facts.notdef_glyph_referenced);
        assert_eq!(facts.direct_page_links, 1);
        assert_eq!(facts.undescribed_files, 8);
        assert_eq!(facts.role_maps, builder.role_maps);
        assert_eq!(facts.structure_elements.len(), 2);
        assert_eq!(facts.structure_elements[1].tag, "Figure");
        assert!(!facts.structure_elements[1].has_alt);
        assert!(facts.structure_elements[1].has_actual_text);
    }
}
