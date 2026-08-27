//! Image XObject extraction (ISO 32000-1 §8.9): turn an image stream into pixel data plus the
//! metadata needed to interpret it.
//!
//! Transport filters (Flate, ASCII, RunLength) and `DCTDecode` (JPEG) are decoded to raw samples
//! via [`pdf_filters`]. A JPEG the decoder cannot handle is passed through as a complete `.jpg`,
//! and `JPXDecode` (JPEG 2000) is passed through too (no pixel decoder yet) — but its main header is
//! parsed to recover the bit depth PDF lets a JPX image omit from its dictionary (§7.4.9).
//! `JBIG2Decode` images are decoded to 1-bpp samples (§7.4.7) when the caller supplies any
//! `/JBIG2Globals`; an image the decoder cannot handle is passed through verbatim.

use pdf_cos::{Dictionary, Name, Object, Stream};
use pdf_filters::{DEFAULT_MAX_DECODED, Filter, decode_stream, jbig2_decode, jpx_info};

use crate::color::ColorSpace;

/// Metadata describing an extracted image (§8.9.5).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ImageInfo {
    /// Width in samples (`/Width`).
    pub width: u32,
    /// Height in samples (`/Height`).
    pub height: u32,
    /// Bits per component (`/BitsPerComponent`).
    pub bits_per_component: u8,
    /// The image's color space (§8.6).
    pub color_space: ColorSpace,
}

/// The image's payload, tagged with how it is encoded.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ImageData {
    /// Decoded raster samples (row-major, `components × bits_per_component` per pixel).
    Raw(Vec<u8>),
    /// A complete JPEG file (`DCTDecode`), passed through verbatim.
    Jpeg(Vec<u8>),
    /// A complete JPEG 2000 file (`JPXDecode`), passed through verbatim.
    Jpeg2000(Vec<u8>),
    /// A JBIG2 (`JBIG2Decode`) bi-level image that could not be decoded (e.g. its `/JBIG2Globals`
    /// were unavailable, or the codestream was wrapped in an unhandled transport filter), passed
    /// through verbatim. A decodable JBIG2 image becomes [`ImageData::Raw`] 1-bpp samples instead.
    Jbig2(Vec<u8>),
}

/// An extracted image XObject.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ExtractedImage {
    /// Image metadata.
    pub info: ImageInfo,
    /// Image payload.
    pub data: ImageData,
}

/// Extract the image carried by `stream`, using the already-resolved `color_space` and
/// `jbig2_globals` (§8.6 spaces and `/JBIG2Globals` can reference indirect objects the caller must
/// resolve first; `jbig2_globals` is the decoded bytes of the `/JBIG2Globals` stream, or `None`).
///
/// Transport filters are decoded; a terminal image codec is detected and either decoded
/// (`DCTDecode`, `JBIG2Decode`) or its bytes passed through (`JPXDecode`, or anything undecodable).
#[must_use]
pub fn extract_image(
    stream: &Stream,
    color_space: ColorSpace,
    jbig2_globals: Option<&[u8]>,
) -> ExtractedImage {
    let dict = stream.dict();
    let width = dict.get_integer(&Name::from("Width")).unwrap_or(0).max(0) as u32;
    let height = dict.get_integer(&Name::from("Height")).unwrap_or(0).max(0) as u32;
    let bpc_in_dict = dict.get_integer(&Name::from("BitsPerComponent"));
    let mut bits_per_component = bpc_in_dict.unwrap_or(8).clamp(0, 16) as u8;

    // Flate/ASCII/RunLength and DCTDecode (JPEG) decode to raw samples via the filter layer. A
    // JPEG the decoder cannot handle, or a JPEG 2000 stream (no pixel decoder yet), is passed
    // through as a complete image file.
    let data = match terminal_codec(dict) {
        Some(Filter::Jpx) => {
            // §7.4.9: a JPX image may omit /BitsPerComponent — recover it from the codestream.
            if bpc_in_dict.is_none()
                && let Ok(info) = jpx_info(stream.raw())
            {
                bits_per_component = info.bit_depth;
            }
            ImageData::Jpeg2000(stream.raw().to_vec())
        }
        // §7.4.7: decode JBIG2 to 1-bpp samples (always monochrome). The codestream is the raw
        // stream bytes when JBIG2 is the sole filter (the usual case); a stream wrapped in a
        // transport filter, or one whose globals are missing, fails to decode and passes through.
        Some(Filter::Jbig2) => {
            match jbig2_decode(stream.raw(), jbig2_globals, DEFAULT_MAX_DECODED) {
                Ok(samples) => {
                    bits_per_component = 1;
                    ImageData::Raw(samples)
                }
                Err(_) => ImageData::Jbig2(stream.raw().to_vec()),
            }
        }
        Some(Filter::Dct) => match decode_stream(stream) {
            Ok(samples) => ImageData::Raw(samples),
            Err(_) => ImageData::Jpeg(stream.raw().to_vec()),
        },
        _ => match decode_stream(stream) {
            Ok(samples) => ImageData::Raw(samples),
            Err(_) => ImageData::Raw(stream.raw().to_vec()),
        },
    };

    let info = ImageInfo {
        width,
        height,
        bits_per_component,
        color_space,
    };
    ExtractedImage { info, data }
}

/// The last filter in the image's `/Filter` chain (the image codec, if any).
fn terminal_codec(dict: &Dictionary) -> Option<Filter> {
    match dict.get(&Name::from("Filter")) {
        Some(Object::Name(name)) => Filter::from_name(name),
        Some(Object::Array(filters)) => filters.iter().rev().find_map(|f| match f {
            Object::Name(name) => Filter::from_name(name),
            _ => None,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdf_cos::{Dictionary, Object};

    fn image_stream(filter: Option<&str>, bytes: &[u8]) -> Stream {
        let mut dict = Dictionary::new();
        dict.insert(Name::from("Type"), Object::Name(Name::from("XObject")));
        dict.insert(Name::from("Subtype"), Object::Name(Name::from("Image")));
        dict.insert(Name::from("Width"), Object::Integer(2));
        dict.insert(Name::from("Height"), Object::Integer(1));
        dict.insert(Name::from("BitsPerComponent"), Object::Integer(8));
        if let Some(f) = filter {
            dict.insert(Name::from("Filter"), Object::Name(Name::from(f)));
        }
        Stream::new(dict, bytes.to_vec())
    }

    #[test]
    fn unfiltered_rgb_samples_are_raw() {
        // 2×1 RGB image: two pixels, six bytes.
        let samples = [255u8, 0, 0, 0, 255, 0];
        let image = extract_image(&image_stream(None, &samples), ColorSpace::DeviceRgb, None);
        assert_eq!(image.info.width, 2);
        assert_eq!(image.info.height, 1);
        assert_eq!(image.info.bits_per_component, 8);
        assert_eq!(image.info.color_space, ColorSpace::DeviceRgb);
        assert_eq!(image.data, ImageData::Raw(samples.to_vec()));
    }

    #[test]
    fn real_dct_image_decodes_to_samples() {
        // A 2×2 RGB JPEG decodes to 12 raw samples (not passed through).
        let jpeg = base64(JPEG_2X2);
        let image = extract_image(
            &image_stream(Some("DCTDecode"), &jpeg),
            ColorSpace::DeviceRgb,
            None,
        );
        match image.data {
            ImageData::Raw(samples) => assert_eq!(samples.len(), 2 * 2 * 3),
            other => panic!("expected decoded samples, got {other:?}"),
        }
    }

    #[test]
    fn undecodable_dct_image_falls_back_to_passthrough() {
        // Bytes that are not a valid JPEG cannot be decoded, so they are passed through as-is.
        let jpeg = b"\xFF\xD8\xFF\xE0 fake jpeg \xFF\xD9";
        let image = extract_image(
            &image_stream(Some("DCTDecode"), jpeg),
            ColorSpace::DeviceRgb,
            None,
        );
        assert_eq!(image.data, ImageData::Jpeg(jpeg.to_vec()));
    }

    const JPEG_2X2: &str = "/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAMCAgICAgMCAgIDAwMDBAYEBAQEBAgGBgUGCQgKCgkICQkKDA8MCgsOCwkJDRENDg8QEBEQCgwSExIQEw8QEBD/2wBDAQMDAwQDBAgEBAgQCwkLEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBD/wAARCAACAAIDAREAAhEBAxEB/8QAFAABAAAAAAAAAAAAAAAAAAAACP/EABQQAQAAAAAAAAAAAAAAAAAAAAD/xAAVAQEBAAAAAAAAAAAAAAAAAAAHCf/EABQRAQAAAAAAAAAAAAAAAAAAAAD/2gAMAwEAAhEDEQA/ADoDFU3/2Q==";

    fn base64(s: &str) -> Vec<u8> {
        let val = |c: u8| -> i32 {
            match c {
                b'A'..=b'Z' => (c - b'A') as i32,
                b'a'..=b'z' => (c - b'a' + 26) as i32,
                b'0'..=b'9' => (c - b'0' + 52) as i32,
                b'+' => 62,
                b'/' => 63,
                _ => -1,
            }
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
    fn jpx_image_is_passed_through() {
        let jpx = b"\x00\x00\x00\x0CjP fake jpeg2000";
        let image = extract_image(
            &image_stream(Some("JPXDecode"), jpx),
            ColorSpace::Other(3),
            None,
        );
        assert_eq!(image.data, ImageData::Jpeg2000(jpx.to_vec()));
    }

    #[test]
    fn undecodable_jbig2_image_falls_back_to_passthrough() {
        // Bytes that are not a valid JBIG2 stream cannot be decoded, so they pass through as
        // ImageData::Jbig2 rather than being mislabelled Raw.
        let jbig2 = b"\x00\x00\x00\x01 fake jbig2 segment";
        let image = extract_image(
            &image_stream(Some("JBIG2Decode"), jbig2),
            ColorSpace::DeviceGray,
            None,
        );
        assert_eq!(image.data, ImageData::Jbig2(jbig2.to_vec()));
    }

    #[test]
    fn real_jbig2_image_decodes_to_one_bpp_samples() {
        // The ISO 32000-1 §7.4.7 worked example (52×66, symbol dictionary in a globals stream).
        let img = unhex(JBIG2_IMAGE);
        let globals = unhex(JBIG2_GLOBALS);
        let mut dict = Dictionary::new();
        dict.insert(Name::from("Subtype"), Object::Name(Name::from("Image")));
        dict.insert(Name::from("Width"), Object::Integer(52));
        dict.insert(Name::from("Height"), Object::Integer(66));
        dict.insert(Name::from("BitsPerComponent"), Object::Integer(1));
        dict.insert(
            Name::from("Filter"),
            Object::Name(Name::from("JBIG2Decode")),
        );
        let stream = Stream::new(dict, img);
        let image = extract_image(&stream, ColorSpace::DeviceGray, Some(&globals));
        assert_eq!(image.info.bits_per_component, 1);
        // 52 px → 7 bytes/row × 66 rows of packed 1-bpp samples.
        match image.data {
            ImageData::Raw(samples) => assert_eq!(samples.len(), 7 * 66),
            other => panic!("expected decoded 1-bpp samples, got {other:?}"),
        }
        // Without the globals the same image can't resolve its symbols → passthrough.
        let image2 = extract_image(
            &image_stream(Some("JBIG2Decode"), &unhex(JBIG2_IMAGE)),
            ColorSpace::DeviceGray,
            None,
        );
        assert!(matches!(image2.data, ImageData::Jbig2(_)));
    }

    /// Hex-decode ignoring whitespace (test fixtures only).
    fn unhex(s: &str) -> Vec<u8> {
        let h: Vec<u8> = s.bytes().filter(u8::is_ascii_hexdigit).collect();
        h.as_chunks::<2>()
            .0
            .iter()
            .map(|&[hi, lo]| {
                let hi = (hi as char).to_digit(16).unwrap();
                let lo = (lo as char).to_digit(16).unwrap();
                (hi * 16 + lo) as u8
            })
            .collect()
    }

    const JBIG2_IMAGE: &str = "000000013000010000001300000034000000420000000000\
        00000040000000000002062000010000001e000000340000\
        004200000000000000000200100000000231db51ce51ffac";
    const JBIG2_GLOBALS: &str = "0000000000010000000032000003fffdff02fefefe000000\
        01000000012ae225aea9a5a538b4d9999c5c8e56ef0f872\
        7f2b53d4e37ef795cc5506dffac";

    #[test]
    fn filter_array_selects_terminal_codec_and_transport_filter_falls_back() {
        use pdf_cos::Array;
        // /Filter as an array carrying a non-name element (skipped) and a transport filter
        // (FlateDecode). The terminal codec is the transport filter, so the bytes go through the
        // generic decode path; undecodable data falls back to a raw passthrough rather than panicking.
        let mut dict = Dictionary::new();
        dict.insert(Name::from("Subtype"), Object::Name(Name::from("Image")));
        dict.insert(Name::from("Width"), Object::Integer(2));
        dict.insert(Name::from("Height"), Object::Integer(1));
        dict.insert(Name::from("BitsPerComponent"), Object::Integer(8));
        dict.insert(
            Name::from("Filter"),
            Object::Array(Array::from(vec![
                Object::Integer(0), // non-name → ignored by terminal_codec
                Object::Name(Name::from("FlateDecode")),
            ])),
        );
        let raw = b"not valid deflate data";
        let image = extract_image(
            &Stream::new(dict, raw.to_vec()),
            ColorSpace::DeviceGray,
            None,
        );
        assert_eq!(image.data, ImageData::Raw(raw.to_vec()));
    }

    #[test]
    fn jpx_recovers_bit_depth_from_codestream() {
        // A JPX image whose dictionary omits /BitsPerComponent: the value comes from the SIZ marker.
        let cs = j2k_codestream(8, 8, 1, 12);
        let mut dict = Dictionary::new();
        dict.insert(Name::from("Subtype"), Object::Name(Name::from("Image")));
        dict.insert(Name::from("Width"), Object::Integer(8));
        dict.insert(Name::from("Height"), Object::Integer(8));
        dict.insert(Name::from("Filter"), Object::Name(Name::from("JPXDecode")));
        let stream = Stream::new(dict, cs.clone());
        let image = extract_image(&stream, ColorSpace::Other(1), None);
        assert_eq!(image.info.bits_per_component, 12);
        assert_eq!(image.data, ImageData::Jpeg2000(cs));
    }

    /// Minimal JPEG 2000 codestream (SOC + SIZ + EOC) for the metadata-recovery test.
    fn j2k_codestream(width: u32, height: u32, components: u16, bit_depth: u8) -> Vec<u8> {
        let mut out = vec![0xFF, 0x4F]; // SOC
        let mut siz = Vec::new();
        siz.extend_from_slice(&0u16.to_be_bytes()); // Rsiz
        siz.extend_from_slice(&width.to_be_bytes()); // Xsiz
        siz.extend_from_slice(&height.to_be_bytes()); // Ysiz
        siz.extend_from_slice(&[0u8; 16]); // XOsiz, YOsiz, XTsiz, YTsiz placeholders
        siz.extend_from_slice(&[0u8; 8]); // XTOsiz, YTOsiz
        siz.extend_from_slice(&components.to_be_bytes()); // Csiz
        for _ in 0..components {
            siz.push(bit_depth - 1); // Ssiz
            siz.push(1);
            siz.push(1);
        }
        out.extend_from_slice(&[0xFF, 0x51]); // SIZ
        out.extend_from_slice(&((siz.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&siz);
        out.extend_from_slice(&[0xFF, 0xD9]); // EOC
        out
    }
}
