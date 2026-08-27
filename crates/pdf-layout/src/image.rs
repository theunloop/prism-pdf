//! Images for authored documents (ISO 32000-1 §8.9): wrap encoded bytes as an image XObject ready
//! to place with [`Flow::image`](crate::Flow::image).
//!
//! A JPEG is embedded as-is via `DCTDecode` (its geometry/colour space read from the frame header);
//! raw 8-bit samples are embedded uncompressed. The placement (scaling, position) is the flow/
//! content layer's job.

use pdf_document::{ImageColorSpace, ImageFilter, ImageXObject};

/// An image ready to embed: its intrinsic pixel size and the [`ImageXObject`] payload.
#[derive(Clone, Debug)]
pub struct Image {
    pub(crate) xobject: ImageXObject,
}

impl Image {
    /// Wrap a complete JPEG (embedded via `DCTDecode`). Reads width/height and colour space from the
    /// frame header; returns `None` if the data is not a JPEG with 1/3/4 components.
    #[must_use]
    pub fn from_jpeg(bytes: Vec<u8>) -> Option<Image> {
        let (width, height, components, bits) = jpeg_frame(&bytes)?;
        let color_space = match components {
            1 => ImageColorSpace::Gray,
            3 => ImageColorSpace::Rgb,
            4 => ImageColorSpace::Cmyk,
            _ => return None,
        };
        Some(Image {
            xobject: ImageXObject {
                width,
                height,
                color_space,
                bits_per_component: bits,
                filter: Some(ImageFilter::Dct),
                data: bytes,
                smask: None,
                mask: None,
                image_mask: false,
            },
        })
    }

    /// Wrap raw 8-bit interleaved RGB samples (`width * height * 3` bytes), embedded uncompressed.
    #[must_use]
    pub fn from_rgb(width: u32, height: u32, rgb: Vec<u8>) -> Option<Image> {
        Self::from_raw(width, height, ImageColorSpace::Rgb, 3, rgb)
    }

    /// Wrap raw 8-bit grayscale samples (`width * height` bytes), embedded uncompressed.
    #[must_use]
    pub fn from_gray(width: u32, height: u32, gray: Vec<u8>) -> Option<Image> {
        Self::from_raw(width, height, ImageColorSpace::Gray, 1, gray)
    }

    /// Wrap raw 8-bit interleaved RGBA samples (`width * height * 4` bytes): the RGB channels become
    /// the base image and the alpha channel a `DeviceGray` **soft mask** (`/SMask`, §11.6.5.2) — so
    /// the image carries per-pixel transparency (the PNG-with-alpha case). Returns `None` on a length
    /// mismatch.
    #[must_use]
    pub fn from_rgba(width: u32, height: u32, rgba: Vec<u8>) -> Option<Image> {
        let pixels = u64::from(width) * u64::from(height);
        if width == 0 || height == 0 || rgba.len() as u64 != pixels * 4 {
            return None;
        }
        let mut rgb = Vec::with_capacity(rgba.len() / 4 * 3);
        let mut alpha = Vec::with_capacity(rgba.len() / 4);
        for px in rgba.chunks_exact(4) {
            rgb.extend_from_slice(&px[..3]);
            alpha.push(px[3]);
        }
        let mut image = Self::from_rgb(width, height, rgb)?;
        let smask = Self::from_gray(width, height, alpha)?;
        image.xobject.smask = Some(Box::new(smask.xobject));
        Some(image)
    }

    /// Attach a 1-bit **stencil mask** (`/Mask`, §8.9.6.3) of its own `mask_width × mask_height`:
    /// `bits` is packed 1-bpp, MSB-first, each row padded to a byte boundary; a `1` bit marks a
    /// sample that is *masked out* (not painted). Returns `None` on a length mismatch.
    #[must_use]
    pub fn with_stencil_mask(
        mut self,
        mask_width: u32,
        mask_height: u32,
        bits: Vec<u8>,
    ) -> Option<Image> {
        let row_bytes = u64::from(mask_width).div_ceil(8);
        if mask_width == 0
            || mask_height == 0
            || bits.len() as u64 != row_bytes * u64::from(mask_height)
        {
            return None;
        }
        self.xobject.mask = Some(Box::new(ImageXObject {
            width: mask_width,
            height: mask_height,
            color_space: ImageColorSpace::Gray, // ignored: image_mask omits /ColorSpace
            bits_per_component: 1,
            filter: None,
            data: bits,
            smask: None,
            mask: None,
            image_mask: true,
        }));
        Some(self)
    }

    fn from_raw(
        width: u32,
        height: u32,
        color_space: ImageColorSpace,
        components: u64,
        data: Vec<u8>,
    ) -> Option<Image> {
        let expected = u64::from(width) * u64::from(height) * components;
        if width == 0 || height == 0 || data.len() as u64 != expected {
            return None;
        }
        // Raw samples are bulky, so store them FlateDecode-compressed.
        Some(Image {
            xobject: ImageXObject {
                width,
                height,
                color_space,
                bits_per_component: 8,
                filter: Some(ImageFilter::Flate),
                data: pdf_filters::flate_encode(&data),
                smask: None,
                mask: None,
                image_mask: false,
            },
        })
    }

    /// Intrinsic width in samples.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.xobject.width
    }

    /// Intrinsic height in samples.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.xobject.height
    }

    /// Clone the low-level image XObject for use with the precision authoring layer
    /// ([`pdf_document::PageSpec`], ISO 32000-1 §8.9).
    ///
    /// High-level composition normally places an `Image` directly. This conversion is the
    /// escape hatch for callers that assemble their own content stream and page resources.
    #[must_use]
    pub fn to_xobject(&self) -> ImageXObject {
        self.xobject.clone()
    }
}

/// Read `(width, height, components, bits)` from a JPEG's start-of-frame marker (the only fields an
/// embedder needs). `None` if `bytes` is not a JPEG or has no frame header.
fn jpeg_frame(bytes: &[u8]) -> Option<(u32, u32, u8, u8)> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return None;
    }
    let mut i = 2;
    while i + 4 <= bytes.len() {
        if bytes[i] != 0xFF {
            i += 1; // skip fill bytes between segments
            continue;
        }
        let marker = bytes[i + 1];
        // Standalone markers (no length): SOI/EOI, restart markers, TEM.
        if marker == 0xD8 || marker == 0xD9 || (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            i += 2;
            continue;
        }
        let len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
        // Start-of-frame markers (baseline/progressive/etc), excluding DHT(C4)/JPG(C8)/DAC(CC).
        let is_sof = matches!(
            marker,
            0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF
        );
        if is_sof {
            if i + 10 > bytes.len() {
                return None;
            }
            let bits = bytes[i + 4];
            let height = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as u32;
            let width = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]) as u32;
            let components = bytes[i + 9];
            return Some((width, height, components, bits));
        }
        i += 2 + len; // skip this segment
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2×2 RGB JPEG (ImageMagick), base64-encoded.
    const JPEG_2X2: &str = "/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAMCAgICAgMCAgIDAwMDBAYEBAQEBAgGBgUGCQgKCgkICQkKDA8MCgsOCwkJDRENDg8QEBEQCgwSExIQEw8QEBD/2wBDAQMDAwQDBAgEBAgQCwkLEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBD/wAARCAACAAIDAREAAhEBAxEB/8QAFAABAAAAAAAAAAAAAAAAAAAACP/EABQQAQAAAAAAAAAAAAAAAAAAAAD/xAAVAQEBAAAAAAAAAAAAAAAAAAAHCf/EABQRAQAAAAAAAAAAAAAAAAAAAAD/2gAMAwEAAhEDEQA/ADoDFU3/2Q==";

    fn base64(s: &str) -> Vec<u8> {
        let val = |c: u8| match c {
            b'A'..=b'Z' => (c - b'A') as i32,
            b'a'..=b'z' => (c - b'a' + 26) as i32,
            b'0'..=b'9' => (c - b'0' + 52) as i32,
            b'+' => 62,
            b'/' => 63,
            _ => -1,
        };
        let (mut acc, mut bits, mut out) = (0i32, 0, Vec::new());
        for &c in s.as_bytes() {
            let v = val(c);
            if v < 0 {
                continue;
            }
            acc = (acc << 6) | v;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((acc >> bits) as u8);
            }
        }
        out
    }

    #[test]
    fn jpeg_dimensions_and_colorspace() {
        let img = Image::from_jpeg(base64(JPEG_2X2)).unwrap();
        assert_eq!((img.width(), img.height()), (2, 2));
        assert_eq!(img.xobject.color_space, ImageColorSpace::Rgb);
        assert_eq!(img.xobject.filter, Some(ImageFilter::Dct));
    }

    #[test]
    fn raw_rgb_validates_length() {
        assert!(Image::from_rgb(2, 1, vec![0; 6]).is_some());
        assert!(Image::from_rgb(2, 1, vec![0; 5]).is_none());
        assert!(Image::from_gray(2, 2, vec![0; 4]).is_some());
        assert!(Image::from_jpeg(b"not a jpeg".to_vec()).is_none());
    }

    #[test]
    fn rgba_splits_into_rgb_base_and_gray_smask() {
        // 2×2 RGBA: the alpha channel becomes an 8-bit DeviceGray soft mask.
        assert!(Image::from_rgba(2, 2, vec![0; 15]).is_none()); // wrong length
        let img = Image::from_rgba(2, 2, vec![10; 16]).unwrap();
        assert_eq!(img.xobject.color_space, ImageColorSpace::Rgb);
        let smask = img.xobject.smask.as_ref().expect("soft mask present");
        assert_eq!(smask.color_space, ImageColorSpace::Gray);
        assert_eq!(smask.bits_per_component, 8);
        assert_eq!((smask.width, smask.height), (2, 2));
        assert!(!smask.image_mask);
    }

    #[test]
    fn stencil_mask_is_one_bit_image_mask() {
        let base = Image::from_gray(8, 1, vec![0; 8]).unwrap();
        // 8×1 stencil = 1 byte per row.
        assert!(
            base.clone()
                .with_stencil_mask(8, 1, vec![0xFF, 0x00])
                .is_none()
        ); // wrong length
        let masked = base.with_stencil_mask(8, 1, vec![0b1010_1010]).unwrap();
        let mask = masked.xobject.mask.as_ref().expect("stencil mask present");
        assert!(mask.image_mask, "/ImageMask true");
        assert_eq!(mask.bits_per_component, 1);
    }
}
