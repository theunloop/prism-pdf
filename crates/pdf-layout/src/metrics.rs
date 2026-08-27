//! Font-agnostic text measurement shared by cursor flow and composition (§9.4/§9.6).

use pdf_fonts::FontInfo;

/// Metrics needed by layout. Implementations must be deterministic for the same text and size.
pub trait FontMetrics {
    /// Advance width in points, or `None` when the font cannot measure the text.
    fn width(&self, text: &str, size: f64) -> Option<f64>;

    /// Font ascent in points at `size`.
    fn ascent(&self, size: f64) -> f64;

    /// Font descent in points at `size` (normally negative).
    fn descent(&self, size: f64) -> f64;
}

/// Metrics for one of the Standard-14 fonts.
pub(crate) struct StandardMetrics<'a> {
    base_font: &'a str,
}

impl<'a> StandardMetrics<'a> {
    pub(crate) fn new(base_font: &'a str) -> Self {
        Self { base_font }
    }
}

impl FontMetrics for StandardMetrics<'_> {
    fn width(&self, text: &str, size: f64) -> Option<f64> {
        pdf_fonts::standard_text_width(self.base_font, text, size)
    }

    fn ascent(&self, size: f64) -> f64 {
        size * 0.8
    }

    fn descent(&self, size: f64) -> f64 {
        -size * 0.2
    }
}

/// Metrics for an embedded sfnt font.
pub(crate) struct EmbeddedMetrics<'a> {
    program: &'a [u8],
    info: &'a FontInfo,
}

impl<'a> EmbeddedMetrics<'a> {
    pub(crate) fn new(program: &'a [u8], info: &'a FontInfo) -> Self {
        Self { program, info }
    }
}

impl FontMetrics for EmbeddedMetrics<'_> {
    fn width(&self, text: &str, size: f64) -> Option<f64> {
        pdf_fonts::shape_text(self.program, text).map(|glyphs| {
            glyphs
                .iter()
                .map(|glyph| f64::from(glyph.advance))
                .sum::<f64>()
                * size
                / 1000.0
        })
    }

    fn ascent(&self, size: f64) -> f64 {
        f64::from(self.info.ascent) * size / 1000.0
    }

    fn descent(&self, size: f64) -> f64 {
        f64::from(self.info.descent) * size / 1000.0
    }
}

/// Greedily wrap a paragraph using any font metric source. Always returns at least one line.
pub(crate) fn wrap_paragraph_with(
    metrics: &dyn FontMetrics,
    paragraph: &str,
    size: f64,
    max_width: f64,
) -> Vec<String> {
    if metrics.width("", size).is_none() {
        return vec![paragraph.to_string()];
    }
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in paragraph.split_whitespace() {
        let candidate = if line.is_empty() {
            word.to_string()
        } else {
            format!("{line} {word}")
        };
        let width = metrics.width(&candidate, size).unwrap_or(f64::INFINITY);
        if width <= max_width || line.is_empty() {
            line = candidate;
        } else {
            lines.push(std::mem::take(&mut line));
            line = word.to_string();
        }
    }
    lines.push(line);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_metrics_scale_and_wrap() {
        let metrics = StandardMetrics::new("Courier");
        assert_eq!(metrics.width("hello", 12.0), Some(36.0));
        assert_eq!(metrics.ascent(10.0), 8.0);
        assert_eq!(metrics.descent(10.0), -2.0);
        assert_eq!(
            wrap_paragraph_with(&metrics, "one two three", 12.0, 45.0),
            vec!["one", "two", "three"]
        );
    }

    #[test]
    fn unsupported_standard_font_stays_unwrapped() {
        let metrics = StandardMetrics::new("Missing");
        assert_eq!(
            wrap_paragraph_with(&metrics, "one two", 12.0, 1.0),
            vec!["one two"]
        );
    }
}
