//! `JPXDecode` (§7.4.9) — JPEG 2000 codestream/JP2 header parsing.
//!
//! Full JPEG 2000 *pixel* decoding (tier-2 packet parsing → EBCOT tier-1 with the MQ arithmetic
//! coder → dequantization → inverse wavelet transform) is a heavy codec and is **not** implemented:
//! [`crate::decode_stream`] still reports `JPXDecode` as [`FilterError::Unsupported`], and the image
//! layer passes the codestream through verbatim.
//!
//! What *is* implemented here is the structural front matter: walk the JP2 box wrapper (if any),
//! locate the codestream, and parse its main-header markers (SIZ, COD) to recover the image
//! geometry, component count, bit depth, tiling, and wavelet/progression parameters. PDF lets a
//! `JPXDecode` image omit `/BitsPerComponent` and `/ColorSpace` (§7.4.9), so this metadata is what
//! lets a consumer interpret the passed-through stream. Input is untrusted (DESIGN.md §3.4): every
//! read is bounds-checked and malformed data yields an error, never a panic.

use crate::error::{FilterError, Result};

const JPX: &str = "JPXDecode";

/// Start-of-codestream / start-of-image marker (`0xFF4F`).
const SOC: u16 = 0xFF4F;
/// Image-and-tile-size marker (`0xFF51`).
const SIZ: u16 = 0xFF51;
/// Coding-style-default marker (`0xFF52`).
const COD: u16 = 0xFF52;
/// Start-of-tile-part marker (`0xFF90`) — the main header ends here.
const SOT: u16 = 0xFF90;
/// Start-of-data marker (`0xFF93`).
const SOD: u16 = 0xFF93;
/// End-of-codestream marker (`0xFFD9`).
const EOC: u16 = 0xFFD9;

/// Structural metadata recovered from a JPEG 2000 codestream's main header (§7.4.9).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct JpxInfo {
    /// Image width in samples (`Xsiz − XOsiz`).
    pub width: u32,
    /// Image height in samples (`Ysiz − YOsiz`).
    pub height: u32,
    /// Number of components (`Csiz`).
    pub components: u16,
    /// Bit depth of the first component (`(Ssiz & 0x7F) + 1`).
    pub bit_depth: u8,
    /// Whether the first component is signed (`Ssiz & 0x80`).
    pub signed: bool,
    /// Nominal tile width (`XTsiz`).
    pub tile_width: u32,
    /// Nominal tile height (`YTsiz`).
    pub tile_height: u32,
    /// Decomposition (wavelet) levels from the COD marker, if present.
    pub decomposition_levels: Option<u8>,
    /// Wavelet transform from COD: `Some(true)` = 5/3 reversible, `Some(false)` = 9/7 irreversible.
    pub reversible: Option<bool>,
    /// Progression order from COD (0 = LRCP, 1 = RLCP, 2 = RPCL, 3 = PCRL, 4 = CPRL).
    pub progression_order: Option<u8>,
    /// Number of quality layers from COD, if present.
    pub layers: Option<u16>,
    /// Whether a multiple-component transform is applied (COD `SGcod` MCT flag).
    pub multi_component_transform: Option<bool>,
}

/// Parse the main-header metadata of a `JPXDecode` stream: either a bare JPEG 2000 codestream or a
/// JP2-boxed file. Returns [`FilterError::Corrupt`] if no codestream / SIZ marker can be found.
pub fn jpx_info(input: &[u8]) -> Result<JpxInfo> {
    let codestream = locate_codestream(input).ok_or_else(corrupt)?;
    parse_main_header(codestream)
}

fn corrupt() -> FilterError {
    FilterError::Corrupt { filter: JPX }
}

/// Find the JPEG 2000 codestream within `input`: a bare codestream (starts with SOC), the contents
/// of the JP2 `jp2c` box, or — as a last resort — wherever an SOC marker appears.
fn locate_codestream(input: &[u8]) -> Option<&[u8]> {
    if input.len() >= 2 && be_u16(input, 0) == Some(SOC) {
        return Some(input);
    }
    if let Some(cs) = find_jp2c_box(input) {
        return Some(cs);
    }
    // Lenient fallback: scan for an SOC marker (some producers prepend junk or use odd wrappers).
    input
        .windows(2)
        .position(|w| w == [0xFF, 0x4F])
        .map(|i| &input[i..])
}

/// Walk the top-level JP2 boxes (ISO/IEC 15444-1 §I.4) and return the contents of the `jp2c`
/// (contiguous codestream) box.
fn find_jp2c_box(data: &[u8]) -> Option<&[u8]> {
    let mut pos = 0usize;
    while pos + 8 <= data.len() {
        let lbox = be_u32(data, pos)? as usize;
        let tbox = &data[pos + 4..pos + 8];
        let (header_len, box_len) = if lbox == 1 {
            // Extended length: an 8-byte XLBox follows TBox.
            let xl = be_u64(data, pos + 8)? as usize;
            (16usize, xl)
        } else if lbox == 0 {
            // Runs to the end of the file.
            (8usize, data.len() - pos)
        } else {
            (8usize, lbox)
        };
        if box_len < header_len {
            return None; // malformed length
        }
        let body_start = pos + header_len;
        let end = pos.checked_add(box_len)?;
        if tbox == b"jp2c" {
            // Tolerate a truncated/over-long jp2c by clamping to what we actually have.
            let stop = end.min(data.len());
            return data.get(body_start..stop);
        }
        if end <= pos || end > data.len() {
            return None; // no progress or out of range
        }
        pos = end;
    }
    None
}

/// Parse the codestream main header up to the first tile-part, extracting SIZ (required) and the
/// first COD (optional).
fn parse_main_header(data: &[u8]) -> Result<JpxInfo> {
    let mut pos = 0usize;
    if be_u16(data, 0) == Some(SOC) {
        pos = 2;
    }

    let mut siz: Option<Siz> = None;
    let mut cod: Option<Cod> = None;

    while pos + 2 <= data.len() {
        let marker = be_u16(data, pos).ok_or_else(corrupt)?;
        if marker >> 8 != 0xFF {
            return Err(corrupt()); // lost marker alignment
        }
        pos += 2;

        match marker {
            SOC => continue,          // delimiting marker, no segment
            SOT | SOD | EOC => break, // main header ends at the first tile-part / data / end
            _ => {}
        }

        // All remaining markers carry a 2-byte segment length (which counts itself).
        let len = be_u16(data, pos).ok_or_else(corrupt)? as usize;
        if len < 2 {
            return Err(corrupt());
        }
        let seg_start = pos + 2;
        let seg_end = seg_start + (len - 2);
        let seg = data.get(seg_start..seg_end).ok_or_else(corrupt)?;
        pos = seg_end;

        match marker {
            SIZ => siz = Some(parse_siz(seg)?),
            COD if cod.is_none() => cod = Some(parse_cod(seg).ok_or_else(corrupt)?),
            _ => {} // QCD, COC, RGN, POC, comments, … not needed for header metadata
        }
    }

    let siz = siz.ok_or_else(corrupt)?;
    Ok(JpxInfo {
        width: siz.width,
        height: siz.height,
        components: siz.components,
        bit_depth: siz.bit_depth,
        signed: siz.signed,
        tile_width: siz.tile_width,
        tile_height: siz.tile_height,
        decomposition_levels: cod.map(|c| c.decomposition_levels),
        reversible: cod.map(|c| c.reversible),
        progression_order: cod.map(|c| c.progression_order),
        layers: cod.map(|c| c.layers),
        multi_component_transform: cod.map(|c| c.mct),
    })
}

/// Decoded SIZ marker fields (§I.5.1).
struct Siz {
    width: u32,
    height: u32,
    components: u16,
    bit_depth: u8,
    signed: bool,
    tile_width: u32,
    tile_height: u32,
}

/// `seg` is the SIZ segment after its length field (i.e. starting at `Rsiz`).
fn parse_siz(seg: &[u8]) -> Result<Siz> {
    // Rsiz(2) Xsiz(4) Ysiz(4) XOsiz(4) YOsiz(4) XTsiz(4) YTsiz(4) XTOsiz(4) YTOsiz(4) Csiz(2)
    // then Csiz × [ Ssiz(1) XRsiz(1) YRsiz(1) ].
    let xsiz = be_u32(seg, 2).ok_or_else(corrupt)?;
    let ysiz = be_u32(seg, 6).ok_or_else(corrupt)?;
    let xosiz = be_u32(seg, 10).ok_or_else(corrupt)?;
    let yosiz = be_u32(seg, 14).ok_or_else(corrupt)?;
    let xtsiz = be_u32(seg, 18).ok_or_else(corrupt)?;
    let ytsiz = be_u32(seg, 22).ok_or_else(corrupt)?;
    let components = be_u16(seg, 34).ok_or_else(corrupt)?;
    // First component's Ssiz lives at offset 36.
    let ssiz = *seg.get(36).ok_or_else(corrupt)?;
    if components == 0 || xsiz <= xosiz || ysiz <= yosiz {
        return Err(corrupt());
    }
    Ok(Siz {
        width: xsiz - xosiz,
        height: ysiz - yosiz,
        components,
        bit_depth: (ssiz & 0x7F) + 1,
        signed: ssiz & 0x80 != 0,
        tile_width: xtsiz,
        tile_height: ytsiz,
    })
}

/// Decoded COD marker fields (§I.5.6) needed for metadata.
#[derive(Clone, Copy)]
struct Cod {
    progression_order: u8,
    layers: u16,
    mct: bool,
    decomposition_levels: u8,
    reversible: bool,
}

/// `seg` is the COD segment after its length field (i.e. starting at `Scod`).
fn parse_cod(seg: &[u8]) -> Option<Cod> {
    // Scod(1) | SGcod: progression(1) layers(2) MCT(1) | SPcod: levels(1) cbw(1) cbh(1) style(1)
    // transform(1) [precincts…]. We only read up to the transform byte.
    let progression_order = *seg.get(1)?;
    let layers = be_u16(seg, 2)?;
    let mct = *seg.get(4)? != 0;
    let decomposition_levels = *seg.get(5)?;
    let transform = *seg.get(9)?;
    Some(Cod {
        progression_order,
        layers,
        mct,
        decomposition_levels,
        reversible: transform == 1, // 1 = 5/3 reversible, 0 = 9/7 irreversible
    })
}

fn be_u16(data: &[u8], at: usize) -> Option<u16> {
    let b = data.get(at..at + 2)?;
    Some(u16::from_be_bytes([b[0], b[1]]))
}

fn be_u32(data: &[u8], at: usize) -> Option<u32> {
    let b = data.get(at..at + 4)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn be_u64(data: &[u8], at: usize) -> Option<u64> {
    let b = data.get(at..at + 8)?;
    Some(u64::from_be_bytes([
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal JPEG 2000 codestream: SOC + SIZ + COD + EOC.
    fn codestream(width: u32, height: u32, components: u16, bit_depth: u8) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&SOC.to_be_bytes());

        // --- SIZ ---
        let mut siz = Vec::new();
        siz.extend_from_slice(&0u16.to_be_bytes()); // Rsiz
        siz.extend_from_slice(&width.to_be_bytes()); // Xsiz
        siz.extend_from_slice(&height.to_be_bytes()); // Ysiz
        siz.extend_from_slice(&0u32.to_be_bytes()); // XOsiz
        siz.extend_from_slice(&0u32.to_be_bytes()); // YOsiz
        siz.extend_from_slice(&64u32.to_be_bytes()); // XTsiz
        siz.extend_from_slice(&64u32.to_be_bytes()); // YTsiz
        siz.extend_from_slice(&0u32.to_be_bytes()); // XTOsiz
        siz.extend_from_slice(&0u32.to_be_bytes()); // YTOsiz
        siz.extend_from_slice(&components.to_be_bytes()); // Csiz
        for _ in 0..components {
            siz.push(bit_depth - 1); // Ssiz (unsigned)
            siz.push(1); // XRsiz
            siz.push(1); // YRsiz
        }
        out.extend_from_slice(&SIZ.to_be_bytes());
        out.extend_from_slice(&((siz.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&siz);

        // --- COD --- (LRCP, 1 layer, MCT on, 5 levels, 5/3 reversible)
        let cod = [
            0u8, // Scod
            0,   // progression order = LRCP
            0, 1, // layers = 1
            1, // MCT = on
            5, // decomposition levels
            4, // code-block width exponent
            4, // code-block height exponent
            0, // code-block style
            1, // transform = 5/3 reversible
        ];
        out.extend_from_slice(&COD.to_be_bytes());
        out.extend_from_slice(&((cod.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(&cod);

        out.extend_from_slice(&EOC.to_be_bytes());
        out
    }

    #[test]
    fn parses_raw_codestream() {
        let cs = codestream(100, 200, 3, 8);
        let info = jpx_info(&cs).unwrap();
        assert_eq!(info.width, 100);
        assert_eq!(info.height, 200);
        assert_eq!(info.components, 3);
        assert_eq!(info.bit_depth, 8);
        assert!(!info.signed);
        assert_eq!(info.tile_width, 64);
        assert_eq!(info.tile_height, 64);
        assert_eq!(info.decomposition_levels, Some(5));
        assert_eq!(info.reversible, Some(true));
        assert_eq!(info.progression_order, Some(0));
        assert_eq!(info.layers, Some(1));
        assert_eq!(info.multi_component_transform, Some(true));
    }

    #[test]
    fn parses_signed_high_bit_depth() {
        let cs = codestream(8, 8, 1, 12);
        let mut cs = cs;
        // Flip the sign bit on the first component's Ssiz. Layout: SOC(2) + SIZ marker(2) + Lsiz(2)
        // + Rsiz(2) + 8×u32(32) + Csiz(2) ⇒ Ssiz at offset 2+2+2+2+32+2 = 42.
        cs[42] |= 0x80;
        let info = jpx_info(&cs).unwrap();
        assert_eq!(info.bit_depth, 12);
        assert!(info.signed);
    }

    #[test]
    fn parses_jp2_boxed_file() {
        let cs = codestream(50, 60, 4, 8);
        let mut jp2 = Vec::new();
        // Signature box: length 12, type 'jP  ', content 0D 0A 87 0A.
        jp2.extend_from_slice(&12u32.to_be_bytes());
        jp2.extend_from_slice(b"jP  ");
        jp2.extend_from_slice(&[0x0D, 0x0A, 0x87, 0x0A]);
        // jp2c (contiguous codestream) box wrapping the codestream.
        jp2.extend_from_slice(&((cs.len() + 8) as u32).to_be_bytes());
        jp2.extend_from_slice(b"jp2c");
        jp2.extend_from_slice(&cs);

        let info = jpx_info(&jp2).unwrap();
        assert_eq!(info.width, 50);
        assert_eq!(info.height, 60);
        assert_eq!(info.components, 4);
    }

    #[test]
    fn jp2_box_with_zero_length_runs_to_end() {
        let cs = codestream(20, 30, 1, 8);
        let mut jp2 = Vec::new();
        jp2.extend_from_slice(&12u32.to_be_bytes());
        jp2.extend_from_slice(b"jP  ");
        jp2.extend_from_slice(&[0x0D, 0x0A, 0x87, 0x0A]);
        // LBox = 0 ⇒ box extends to end of file.
        jp2.extend_from_slice(&0u32.to_be_bytes());
        jp2.extend_from_slice(b"jp2c");
        jp2.extend_from_slice(&cs);
        let info = jpx_info(&jp2).unwrap();
        assert_eq!(info.width, 20);
        assert_eq!(info.height, 30);
    }

    #[test]
    fn codestream_without_cod_still_parses_siz() {
        // Hand-build SOC + SIZ + EOC (no COD): COD-derived fields are None.
        let full = codestream(16, 16, 1, 8);
        // Truncate just after SIZ by rebuilding without COD.
        let mut out = Vec::new();
        out.extend_from_slice(&SOC.to_be_bytes());
        // Copy the SIZ marker+segment out of `full`: it starts at offset 2.
        let lsiz = be_u16(&full, 4).unwrap() as usize;
        out.extend_from_slice(&full[2..2 + 2 + lsiz]);
        out.extend_from_slice(&EOC.to_be_bytes());
        let info = jpx_info(&out).unwrap();
        assert_eq!(info.width, 16);
        assert_eq!(info.decomposition_levels, None);
        assert_eq!(info.reversible, None);
    }

    #[test]
    fn missing_siz_is_corrupt() {
        // SOC immediately followed by EOC — no SIZ.
        let mut out = Vec::new();
        out.extend_from_slice(&SOC.to_be_bytes());
        out.extend_from_slice(&EOC.to_be_bytes());
        assert_eq!(jpx_info(&out).unwrap_err(), corrupt());
    }

    #[test]
    fn garbage_is_corrupt_not_panic() {
        assert_eq!(jpx_info(b"not jpeg2000 at all").unwrap_err(), corrupt());
        assert_eq!(jpx_info(&[]).unwrap_err(), corrupt());
        // A truncated SIZ segment must error cleanly.
        let mut cs = codestream(10, 10, 1, 8);
        cs.truncate(10);
        let _ = jpx_info(&cs); // may be Ok(short) or Err — must not panic
    }

    #[test]
    fn truncated_siz_segment_errors() {
        let mut out = Vec::new();
        out.extend_from_slice(&SOC.to_be_bytes());
        out.extend_from_slice(&SIZ.to_be_bytes());
        out.extend_from_slice(&40u16.to_be_bytes()); // claims 40 bytes…
        out.extend_from_slice(&[0u8; 4]); // …but only 4 follow
        assert_eq!(jpx_info(&out).unwrap_err(), corrupt());
    }
}
