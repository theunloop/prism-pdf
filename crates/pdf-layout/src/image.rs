//! Images for authored documents (ISO 32000-1 §8.9): wrap encoded bytes as an image XObject ready
//! to place with [`Flow::image`](crate::Flow::image).
//!
//! A JPEG is embedded as-is via `DCTDecode` (its geometry/colour space read from the frame header);
//! raw 8-bit samples are embedded uncompressed. The placement (scaling, position) is the flow/
//! content layer's job.

use pdf_document::{ImageColorSpace, ImageFilter, ImageXObject};
use zune_png::PngDecoder;
use zune_png::zune_core::bytestream::ZCursor;
use zune_png::zune_core::colorspace::ColorSpace;
use zune_png::zune_core::options::DecoderOptions;
use zune_png::zune_core::result::DecodingResult;

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
        for px in rgba.as_chunks::<4>().0 {
            rgb.extend_from_slice(&px[..3]);
            alpha.push(px[3]);
        }
        let mut image = Self::from_rgb(width, height, rgb)?;
        let smask = Self::from_gray(width, height, alpha)?;
        image.xobject.smask = Some(Box::new(smask.xobject));
        Some(image)
    }

    /// Decode a complete PNG file and wrap it (§8.9.5): greyscale and RGB samples are embedded
    /// uncompressed exactly as [`Self::from_gray`] and [`Self::from_rgb`] embed them; an alpha
    /// channel becomes the `/SMask` soft mask [`Self::from_rgba`] produces (§11.6.5.2); a palette is
    /// expanded to its colours and 16-bit samples are reduced to 8. `None` when the bytes are not a
    /// PNG the decoder accepts. The decoder bounds its own allocations (DESIGN.md §3.4); nothing
    /// here trusts the header beyond the geometry the decoded sample count already agrees with.
    #[must_use]
    pub fn from_png(bytes: &[u8]) -> Option<Image> {
        let options = DecoderOptions::default().png_set_strip_to_8bit(true);
        let mut decoder = PngDecoder::new_with_options(ZCursor::new(bytes), options);
        let DecodingResult::U8(samples) = decoder.decode().ok()? else {
            return None;
        };
        let (width, height) = decoder.dimensions()?;
        let (width, height) = (u32::try_from(width).ok()?, u32::try_from(height).ok()?);
        match decoder.colorspace()? {
            ColorSpace::Luma => Self::from_gray(width, height, samples),
            ColorSpace::LumaA => {
                let (gray, alpha): (Vec<u8>, Vec<u8>) = samples
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|px| (px[0], px[1]))
                    .unzip();
                let mut image = Self::from_gray(width, height, gray)?;
                image.xobject.smask =
                    Some(Box::new(Self::from_gray(width, height, alpha)?.xobject));
                Some(image)
            }
            ColorSpace::RGB => Self::from_rgb(width, height, samples),
            ColorSpace::RGBA => Self::from_rgba(width, height, samples),
            _ => None,
        }
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

    // PNG fixtures written by a stdlib encoder (IHDR/PLTE/IDAT/IEND, zlib, CRC): 2×2 RGBA with a
    // half-transparent and a transparent pixel; a 2×1 8-bit palette; a 2×1 16-bit greyscale; a 2×1
    // grey+alpha.
    const RGBA_2X2: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x06, 0x00, 0x00, 0x00, 0x72,
        0xb6, 0x0d, 0x24, 0x00, 0x00, 0x00, 0x14, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8,
        0xcf, 0xc0, 0xf0, 0x1f, 0x08, 0x1b, 0x18, 0xc0, 0x34, 0x10, 0x00, 0x00, 0x3f, 0xd7, 0x08,
        0x79, 0x8f, 0x13, 0x8a, 0x8a, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42,
        0x60, 0x82,
    ];
    const PALETTE_2X1: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x01, 0x08, 0x03, 0x00, 0x00, 0x00, 0xc3,
        0xfc, 0x8f, 0xb8, 0x00, 0x00, 0x00, 0x06, 0x50, 0x4c, 0x54, 0x45, 0xff, 0x00, 0x00, 0x00,
        0x00, 0xff, 0x6c, 0xa1, 0xfd, 0x8e, 0x00, 0x00, 0x00, 0x0b, 0x49, 0x44, 0x41, 0x54, 0x78,
        0x9c, 0x63, 0x60, 0x60, 0x04, 0x00, 0x00, 0x04, 0x00, 0x02, 0xbf, 0x7a, 0x3f, 0x4a, 0x00,
        0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];
    const GRAY16_2X1: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x01, 0x10, 0x00, 0x00, 0x00, 0x00, 0x81,
        0xd9, 0xfc, 0x15, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x60,
        0x60, 0xf8, 0xff, 0x1f, 0x00, 0x03, 0x02, 0x01, 0xff, 0xe6, 0x77, 0x0b, 0xae, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];
    const GRAY_ALPHA_2X1: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x01, 0x08, 0x04, 0x00, 0x00, 0x00, 0x5e,
        0x2b, 0xb7, 0x01, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xe0,
        0xfa, 0x7f, 0x82, 0x01, 0x00, 0x04, 0xba, 0x01, 0xd2, 0xa1, 0x11, 0x5f, 0xa8, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    /// The raw-sample paths store their samples FlateDecode-compressed; read them back.
    fn samples(image: &ImageXObject) -> Vec<u8> {
        assert_eq!(image.filter, Some(ImageFilter::Flate));
        pdf_filters::flate_decode(&image.data, None, 1 << 16).expect("inflates")
    }

    #[test]
    fn png_rgba_becomes_rgb_with_a_soft_mask() {
        let image = Image::from_png(RGBA_2X2).expect("decodes");
        assert_eq!((image.xobject.width, image.xobject.height), (2, 2));
        assert_eq!(image.xobject.color_space, ImageColorSpace::Rgb);
        assert_eq!(
            samples(&image.xobject),
            [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255]
        );
        let smask = image.xobject.smask.as_deref().expect("alpha → /SMask");
        assert_eq!(samples(smask), [255, 128, 0, 255]);
    }

    #[test]
    fn png_palette_is_expanded_and_16_bit_reduced() {
        let palette = Image::from_png(PALETTE_2X1).expect("decodes");
        assert_eq!(palette.xobject.color_space, ImageColorSpace::Rgb);
        assert_eq!(samples(&palette.xobject), [255, 0, 0, 0, 0, 255]);
        assert!(palette.xobject.smask.is_none());

        let gray = Image::from_png(GRAY16_2X1).expect("decodes");
        assert_eq!(gray.xobject.color_space, ImageColorSpace::Gray);
        assert_eq!(gray.xobject.bits_per_component, 8);
        assert_eq!(samples(&gray.xobject), [0, 255]);
    }

    #[test]
    fn png_grey_alpha_splits_into_grey_and_a_soft_mask() {
        let image = Image::from_png(GRAY_ALPHA_2X1).expect("decodes");
        assert_eq!(image.xobject.color_space, ImageColorSpace::Gray);
        assert_eq!(samples(&image.xobject), [10, 200]);
        assert_eq!(samples(image.xobject.smask.as_deref().unwrap()), [255, 0]);
    }

    #[test]
    fn png_refuses_what_is_not_a_png() {
        assert!(Image::from_png(b"not a png").is_none());
        assert!(Image::from_png(&RGBA_2X2[..40]).is_none(), "truncated");
        assert!(Image::from_png(&[]).is_none());
    }
}
