//! Reading document-level XMP metadata (ISO 32000-1 §14.3.2): the catalog `/Metadata` stream, an
//! XML (XMP) packet. The read counterpart to producing XMP in `pdf-standards` (M7).
//!
//! This returns the raw XMP packet (UTF-8 XML); structured field extraction is left to the caller,
//! who can parse the XML. Reading is best-effort (DESIGN.md §3.4): a missing or undecodable stream
//! yields `None` rather than an error.

use pdf_cos::{Name, Object, PdfDate};

use crate::{Document, Result};

impl Document {
    /// The document's creation date (`/Info /CreationDate`, §14.3.3), parsed as a PDF date
    /// string (§7.9.4); `None` when absent or unparsable (best-effort, like all metadata).
    pub fn creation_date(&self) -> Result<Option<PdfDate>> {
        self.info_date("CreationDate")
    }

    /// The document's modification date (`/Info /ModDate`, §14.3.3), parsed per §7.9.4.
    pub fn modification_date(&self) -> Result<Option<PdfDate>> {
        self.info_date("ModDate")
    }

    /// Resolve an `/Info` entry and parse it as a date string (§7.9.4).
    fn info_date(&self, key: &str) -> Result<Option<PdfDate>> {
        let Some(info) = self.info()? else {
            return Ok(None);
        };
        let Some(value) = info.get(&Name::from(key)) else {
            return Ok(None);
        };
        match self.resolve(value)? {
            Object::String(s) => Ok(PdfDate::parse(s.as_bytes())),
            _ => Ok(None),
        }
    }

    /// The document's XMP metadata packet (§14.3.2) as raw XML text, decoded through its filter
    /// chain, or `None` if the document has no `/Metadata` stream.
    pub fn xmp_metadata(&self) -> Result<Option<String>> {
        let catalog = self.catalog()?;
        let Some(metadata) = catalog.get(&Name::from("Metadata")) else {
            return Ok(None);
        };
        let Ok(Object::Stream(stream)) = self.resolve(metadata) else {
            return Ok(None);
        };
        match self.decode_stream(&stream) {
            Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
            Err(_) => Ok(None),
        }
    }
}
