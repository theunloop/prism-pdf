//! Simple tables for authored documents (ISO 32000-1 §9.4 text + §8.5 path borders).
//!
//! A [`Table`] is column widths plus rows of cell text, with per-cell padding, an optional grid
//! border, and a font/size/alignment. It is rendered by [`Flow::table`](crate::Flow::table), which
//! wraps each cell, sizes rows to their tallest cell, and breaks rows onto new pages (optionally
//! repeating a header row). Column widths are treated as proportions and scaled to the content
//! width.

use crate::text::Align;

/// A table to render into a [`Flow`](crate::Flow). Build it fluently:
///
/// ```ignore
/// let t = Table::new(vec![1.0, 2.0])      // column width ratios
///     .font("F1", "Helvetica").size(11.0)
///     .header_row(true)
///     .row(["Name", "Description"])
///     .row(["Foo", "the foo widget"]);
/// ```
#[derive(Clone, Debug)]
pub struct Table {
    pub(crate) columns: Vec<f64>,
    pub(crate) rows: Vec<Vec<String>>,
    pub(crate) font_resource: String,
    pub(crate) base_font: String,
    pub(crate) size: f64,
    pub(crate) leading: f64,
    pub(crate) padding: f64,
    pub(crate) border: f64,
    pub(crate) align: Align,
    pub(crate) header: bool,
}

impl Table {
    /// A new table with the given column width ratios (scaled to the page content width on render).
    /// Defaults: Helvetica resource `"F1"` at 11 pt, 13 pt leading, 4 pt padding, a 0.5 pt grid,
    /// left-aligned cells, no repeated header.
    #[must_use]
    pub fn new(columns: Vec<f64>) -> Self {
        Table {
            columns,
            rows: Vec::new(),
            font_resource: "F1".to_string(),
            base_font: "Helvetica".to_string(),
            size: 11.0,
            leading: 13.0,
            padding: 4.0,
            border: 0.5,
            align: Align::Left,
            header: false,
        }
    }

    /// Set the cell font: `resource` is the page font name (matching a [`Flow`](crate::Flow)
    /// registration), `base_font` its Standard-14 name for measurement.
    #[must_use]
    pub fn font(mut self, resource: &str, base_font: &str) -> Self {
        self.font_resource = resource.to_string();
        self.base_font = base_font.to_string();
        self
    }

    /// Set the font size in points (leading defaults to follow if not set explicitly afterwards).
    #[must_use]
    pub fn size(mut self, size: f64) -> Self {
        self.size = size;
        self.leading = size * 1.18;
        self
    }

    /// Set the line leading in points.
    #[must_use]
    pub fn leading(mut self, leading: f64) -> Self {
        self.leading = leading;
        self
    }

    /// Set the cell padding in points.
    #[must_use]
    pub fn padding(mut self, padding: f64) -> Self {
        self.padding = padding;
        self
    }

    /// Set the grid line width in points (0 disables borders).
    #[must_use]
    pub fn border(mut self, width: f64) -> Self {
        self.border = width;
        self
    }

    /// Set the cell text alignment.
    #[must_use]
    pub fn align(mut self, align: Align) -> Self {
        self.align = align;
        self
    }

    /// Whether to repeat the first row as a header at the top of each page the table spans.
    #[must_use]
    pub fn header_row(mut self, on: bool) -> Self {
        self.header = on;
        self
    }

    /// Append a row of cell texts (cells beyond the column count are ignored; missing cells are
    /// blank). A cell may contain newlines for multi-line content.
    #[must_use]
    pub fn row<S: Into<String>>(mut self, cells: impl IntoIterator<Item = S>) -> Self {
        self.rows.push(cells.into_iter().map(Into::into).collect());
        self
    }
}
