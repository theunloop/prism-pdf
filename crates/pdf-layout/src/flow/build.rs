use super::*;

impl Flow {
    /// Serialise the flowed document to PDF bytes (§7.5): `self.into_builder().build()`.
    #[must_use]
    pub fn build(self) -> Vec<u8> {
        self.into_builder().build()
    }

    /// Assemble the flowed document into a [`Builder`] without serialising — one page per accumulated
    /// page, embedding any fonts that were used. This is the composition point for post-processing
    /// the document before it is written, e.g. PDF/A production (`pdf-standards::make_pdfa`).
    #[must_use]
    pub fn into_builder(mut self) -> Builder {
        if self.open {
            self.finished.push((
                self.current.into_bytes(),
                self.current_images,
                self.current_embedded,
            ));
        }
        if self.finished.is_empty() {
            self.finished.push((Vec::new(), Vec::new(), Vec::new())); // always at least one page
        }

        let mut builder = Builder::new();
        builder.media_box([0.0, 0.0, self.style.size[0], self.style.size[1]]);
        for (key, value) in &self.info {
            builder.info(key, value);
        }

        // Finalise each embedded font from the glyphs that were actually drawn.
        for slot in &self.embedded {
            if slot.used.is_empty() {
                continue;
            }
            let mut widths: Vec<(u16, u16)> =
                slot.used.iter().map(|(g, (a, _))| (*g, *a)).collect();
            widths.sort_unstable();
            let to_unicode: Vec<(u16, char)> =
                slot.used.iter().map(|(g, (_, c))| (*g, *c)).collect();
            let flags = if slot.info.italic { 4 | 64 } else { 4 }; // Symbolic [+ Italic]

            // Subset to just the glyphs used. The content already shows the original glyph IDs as
            // codes/CIDs, so keep those and supply a CIDToGIDMap to the renumbered subset. If
            // subsetting fails, embed the whole program with an Identity map instead.
            let used_gids: Vec<u16> = slot.used.keys().copied().collect();
            let (program, cid_to_gid) = match pdf_fonts::subset_with_map(&slot.program, &used_gids)
            {
                Some((subset, map)) => (subset, Some(cid_to_gid_map(&map))),
                None => (slot.program.clone(), None),
            };

            builder.embed_cid_font(
                &slot.resource,
                CidFont {
                    program,
                    postscript_name: slot.info.postscript_name.clone(),
                    ascent: slot.info.ascent,
                    descent: slot.info.descent,
                    cap_height: slot.info.cap_height,
                    bbox: slot.info.bbox,
                    italic_angle: slot.info.italic_angle,
                    flags,
                    default_width: 1000,
                    widths,
                    to_unicode,
                    cid_to_gid,
                },
            );
        }

        let fonts: Vec<(&str, StdFont)> =
            self.fonts.iter().map(|(n, f)| (n.as_str(), *f)).collect();
        // Running header/footer baselines, centred vertically in the top/bottom margins.
        let header_y = self.style.size[1] - self.style.margins[2] * 0.5;
        let footer_y = self.style.margins[3] * 0.5;
        let pages = self.finished.len();
        for (i, (content, images, embedded)) in self.finished.into_iter().enumerate() {
            let mut page = Vec::new();
            // Running headers/footers are pagination artifacts (§14.8.2.2) when the document is
            // tagged — bracket them so they stay outside the logical structure.
            if let Some(h) = &self.header {
                if self.tagged {
                    page.extend_from_slice(b"/Artifact BMC\n");
                }
                page.extend(h.render(&self.style, header_y, i + 1, pages));
                if self.tagged {
                    page.extend_from_slice(b"EMC\n");
                }
            }
            page.extend(content);
            if let Some(f) = &self.footer {
                if self.tagged {
                    page.extend_from_slice(b"/Artifact BMC\n");
                }
                page.extend(f.render(&self.style, footer_y, i + 1, pages));
                if self.tagged {
                    page.extend_from_slice(b"EMC\n");
                }
            }
            let mut page = PageSpec::new(page);
            for (name, font) in &fonts {
                page = page.standard_font(*name, *font);
            }
            for name in embedded {
                page = page.embedded_font(name);
            }
            for (name, image) in images {
                page = page.image(name, image);
            }
            builder.add_page(page);
        }
        for (title, page_index) in &self.bookmarks {
            builder.outline(title, *page_index);
        }
        if self.tagged {
            if let Some(lang) = &self.lang {
                builder.lang(lang);
            }
            builder.structure(self.structure);
        }
        if self.notdef_used {
            // Shaping fell back to `.notdef` somewhere: let the PDF/UA passes reject this.
            builder.flag_notdef_reference();
        }
        builder
    }
}
