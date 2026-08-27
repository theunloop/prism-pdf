//! Text measurement, wrapping and single-block layout (ISO 32000-1 §9.3/§9.4/§9.6.2.2).
//!
//! These are the building blocks the multi-page `flow` engine sits on: measure a
//! string in a Standard-14 font, greedily wrap it to a width, and draw a wrapped + aligned block
//! into a [`Content`]. They draw operators (no graphics-state interpretation).

use pdf_content::Content;

use crate::metrics::{FontMetrics, StandardMetrics, wrap_paragraph_with};

/// Measure the advance width of `text` set in a Standard-14 font at `size` points (§9.6.2.2), using
/// the built-in AFM metrics. `base_font` is a Standard-14 `/BaseFont` name; `None` if unsupported.
#[must_use]
pub fn measure_text(base_font: &str, text: &str, size: f64) -> Option<f64> {
    StandardMetrics::new(base_font).width(text, size)
}

/// Greedily word-wrap `text` to lines no wider than `max_width` points in `base_font` at `size`.
/// Existing newlines start new paragraphs; an over-long word is kept whole; an unmeasurable font is
/// split only on its newlines.
#[must_use]
pub fn wrap_text(base_font: &str, text: &str, size: f64, max_width: f64) -> Vec<String> {
    text.split('\n')
        .flat_map(|p| wrap_paragraph(base_font, p, size, max_width))
        .collect()
}

/// Wrap a single paragraph (no embedded newlines). Always returns ≥ 1 line (empty for an empty
/// paragraph); an unmeasurable font yields the paragraph unwrapped.
#[must_use]
pub fn wrap_paragraph(base_font: &str, paragraph: &str, size: f64, max_width: f64) -> Vec<String> {
    wrap_paragraph_with(&StandardMetrics::new(base_font), paragraph, size, max_width)
}

/// Horizontal alignment for a text block (§9.4).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Align {
    /// Flush left (the default).
    #[default]
    Left,
    /// Centered within the block width.
    Center,
    /// Flush right.
    Right,
    /// Flush both edges by widening spaces (a paragraph's last line stays left-aligned).
    Justify,
}

/// How to set and lay out a run of text.
#[derive(Clone, Copy, Debug)]
pub struct TextBlock<'a> {
    /// The page resource name of the font (the `/Fn` key referenced by `Tf`).
    pub font_resource: &'a str,
    /// The font's Standard-14 `/BaseFont` name, used for width metrics (e.g. `"Helvetica"`).
    pub base_font: &'a str,
    /// Font size in points.
    pub size: f64,
    /// Baseline-to-baseline line spacing in points.
    pub leading: f64,
    /// Horizontal alignment within the block.
    pub align: Align,
}

/// Compute one line's `(x offset from the block's left edge, word-spacing)` for `align` within
/// `width`. `is_last` marks a paragraph's final line (never justified). Shared by [`draw_text_block`]
/// and the `flow` engine.
#[must_use]
pub(crate) fn line_layout(
    align: Align,
    line: &str,
    line_width: f64,
    width: f64,
    is_last: bool,
) -> (f64, f64) {
    match align {
        Align::Left => (0.0, 0.0),
        Align::Center => ((width - line_width) / 2.0, 0.0),
        Align::Right => (width - line_width, 0.0),
        Align::Justify => {
            let spaces = line.bytes().filter(|&b| b == b' ').count();
            if is_last || spaces == 0 {
                (0.0, 0.0)
            } else {
                (0.0, (width - line_width) / spaces as f64)
            }
        }
    }
}

/// Draw `text` as a wrapped, aligned block into `content`: each line fits within `width`, the first
/// line's baseline is at `(x, y)`, and successive lines step down by `block.leading`. Honours
/// existing newlines as paragraph breaks. Returns the baseline `y` *below* the last line. For
/// content that may exceed one page, use the `flow` engine instead.
pub fn draw_text_block(
    content: &mut Content,
    block: &TextBlock,
    x: f64,
    y: f64,
    width: f64,
    text: &str,
) -> f64 {
    content.begin_text();
    content.set_font(block.font_resource, block.size);

    let mut cur_y = y;
    let mut cur_tw = 0.0f64;
    for paragraph in text.split('\n') {
        let lines = wrap_paragraph(block.base_font, paragraph, block.size, width);
        let last = lines.len().saturating_sub(1);
        for (i, line) in lines.iter().enumerate() {
            let line_w = measure_text(block.base_font, line, block.size).unwrap_or(0.0);
            let (dx, word_space) = line_layout(block.align, line, line_w, width, i == last);
            if word_space != cur_tw {
                content.set_word_spacing(word_space);
                cur_tw = word_space;
            }
            content.set_text_matrix(1.0, 0.0, 0.0, 1.0, x + dx, cur_y);
            content.show_text(&pdf_fonts::winansi_encode(line));
            cur_y -= block.leading;
        }
    }
    if cur_tw != 0.0 {
        content.set_word_spacing(0.0);
    }
    content.end_text();
    cur_y
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_and_wraps() {
        assert_eq!(measure_text("Courier", "hello", 12.0), Some(36.0));
        assert!(measure_text("Helvetica", "Wm", 12.0).unwrap() > 0.0);
        assert!(measure_text("Bogus", "x", 12.0).is_none());

        let lines = wrap_text("Helvetica", "the quick brown fox jumps over", 12.0, 80.0);
        assert!(lines.len() > 1);
        for line in &lines {
            let within = measure_text("Helvetica", line, 12.0).unwrap() <= 80.0 + 1e-6;
            assert!(within || line.split_whitespace().count() == 1);
        }
        assert_eq!(wrap_text("Bogus", "a\nb", 12.0, 100.0), vec!["a", "b"]);
    }

    #[test]
    fn alignment_offsets() {
        // Left at 0; center and right push rightward (within a 200pt block).
        let (lx, _) = line_layout(Align::Left, "hi", 20.0, 200.0, false);
        let (cx, _) = line_layout(Align::Center, "hi", 20.0, 200.0, false);
        let (rx, _) = line_layout(Align::Right, "hi", 20.0, 200.0, false);
        assert_eq!(lx, 0.0);
        assert_eq!(cx, 90.0);
        assert_eq!(rx, 180.0);
        // Justify widens spaces on non-last lines, but not the last.
        let (_, tw) = line_layout(Align::Justify, "a b c", 50.0, 100.0, false);
        assert!(tw > 0.0);
        let (_, tw_last) = line_layout(Align::Justify, "a b c", 50.0, 100.0, true);
        assert_eq!(tw_last, 0.0);
    }

    #[test]
    fn draw_block_positions_lines() {
        let mut c = Content::new();
        let block = TextBlock {
            font_resource: "F1",
            base_font: "Helvetica",
            size: 12.0,
            leading: 14.0,
            align: Align::Left,
        };
        let end = draw_text_block(&mut c, &block, 10.0, 100.0, 200.0, "one two three");
        assert!(end < 100.0);
        let dump = String::from_utf8(c.into_bytes()).unwrap();
        assert!(dump.contains(" Tm\n"));
        assert!(dump.contains(") Tj\n"));
    }
}
