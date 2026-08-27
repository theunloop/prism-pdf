//! Font-resource decoding for content-stream text extraction (§9.6–§9.10).

use std::collections::HashMap;

use pdf_content::{GlyphDecoder, Latin1Decoder};
use pdf_cos::{Dictionary, Name, Object, Stream};

use crate::{CMap, Encoding, ToUnicode, glyph_to_unicode};

enum FontDecoder {
    ToUnicode(ToUnicode),
    Cid(CidTextDecoder),
    Simple(Box<Encoding>),
}

struct CidTextDecoder {
    cmap: CMap,
    cid_to_text: HashMap<u32, char>,
}

impl CidTextDecoder {
    fn decode(&self, bytes: &[u8]) -> String {
        self.cmap
            .codes_to_cids(bytes)
            .into_iter()
            .filter_map(|cid| self.cid_to_text.get(&cid).copied())
            .collect()
    }
}

/// A page or Form XObject's font-resource decoder (§7.8.3, §9.6–§9.10).
///
/// Object resolution and stream decoding are supplied by the caller, keeping the fonts layer
/// independent of a particular document model and filter implementation.
pub struct ResourceDecoder {
    fonts: HashMap<String, FontDecoder>,
}

impl ResourceDecoder {
    /// Build a decoder from a `/Resources` dictionary.
    ///
    /// # Errors
    /// Returns the caller's error when an indirect object or a required composite-font stream
    /// cannot be resolved or decoded.
    pub fn from_resources<E>(
        resources: &Dictionary,
        resolve: &impl Fn(&Object) -> Result<Object, E>,
        decode: &impl Fn(&Stream) -> Result<Vec<u8>, E>,
    ) -> Result<Self, E> {
        let fonts_dict = subdict(resources, "Font", resolve)?;
        let mut fonts = HashMap::new();
        for (name, font_ref) in fonts_dict.iter() {
            let Object::Dictionary(font) = resolve(font_ref)? else {
                continue;
            };
            let key = String::from_utf8_lossy(name.as_bytes()).into_owned();
            let decoder = if let Some(map) = to_unicode_of(&font, resolve, decode) {
                FontDecoder::ToUnicode(map)
            } else if is_type0(&font) {
                FontDecoder::Cid(cid_decoder(&font, resolve, decode)?)
            } else {
                FontDecoder::Simple(Box::new(Encoding::from_font_dict(&font)))
            };
            fonts.insert(key, decoder);
        }
        Ok(Self { fonts })
    }
}

impl GlyphDecoder for ResourceDecoder {
    fn decode(&self, font: Option<&str>, bytes: &[u8]) -> String {
        match font.and_then(|name| self.fonts.get(name)) {
            Some(FontDecoder::ToUnicode(map)) => map.decode(bytes),
            Some(FontDecoder::Cid(cid)) => cid.decode(bytes),
            Some(FontDecoder::Simple(encoding)) => encoding.decode(bytes),
            None => Latin1Decoder.decode(font, bytes),
        }
    }
}

fn subdict<E>(
    dictionary: &Dictionary,
    key: &str,
    resolve: &impl Fn(&Object) -> Result<Object, E>,
) -> Result<Dictionary, E> {
    match dictionary.get(&Name::from(key)) {
        Some(object) => match resolve(object)? {
            Object::Dictionary(dictionary) => Ok(dictionary),
            _ => Ok(Dictionary::new()),
        },
        None => Ok(Dictionary::new()),
    }
}

fn to_unicode_of<E>(
    font: &Dictionary,
    resolve: &impl Fn(&Object) -> Result<Object, E>,
    decode: &impl Fn(&Stream) -> Result<Vec<u8>, E>,
) -> Option<ToUnicode> {
    let Object::Stream(stream) = resolve(font.get(&Name::from("ToUnicode"))?).ok()? else {
        return None;
    };
    let map = ToUnicode::parse(&decode(&stream).ok()?);
    (!map.is_empty()).then_some(map)
}

fn is_type0(font: &Dictionary) -> bool {
    font.get_name(&Name::from("Subtype")).map(Name::as_bytes) == Some(b"Type0")
}

fn cid_decoder<E>(
    font: &Dictionary,
    resolve: &impl Fn(&Object) -> Result<Object, E>,
    decode: &impl Fn(&Stream) -> Result<Vec<u8>, E>,
) -> Result<CidTextDecoder, E> {
    let (cmap, trusted) = match font.get(&Name::from("Encoding")) {
        Some(encoding) => match resolve(encoding)? {
            Object::Name(name) => match CMap::from_predefined(name.as_bytes()) {
                Some(cmap) => (cmap, true),
                None => (CMap::identity(), false),
            },
            Object::Stream(stream) => (CMap::parse(&decode(&stream)?), true),
            _ => (CMap::identity(), false),
        },
        None => (CMap::identity(), false),
    };
    let cid_to_text = if trusted {
        cid_to_unicode(font, resolve, decode)?
    } else {
        HashMap::new()
    };
    Ok(CidTextDecoder { cmap, cid_to_text })
}

fn cid_to_unicode<E>(
    font: &Dictionary,
    resolve: &impl Fn(&Object) -> Result<Object, E>,
    decode: &impl Fn(&Stream) -> Result<Vec<u8>, E>,
) -> Result<HashMap<u32, char>, E> {
    let Some(descriptor) = font_descriptor(font, resolve)? else {
        return Ok(HashMap::new());
    };
    let Some(program) = embedded_program(&descriptor, resolve, decode)? else {
        return Ok(HashMap::new());
    };
    let Some(glyph_text) = glyph_to_unicode(&program) else {
        return Ok(HashMap::new());
    };
    let mut output = HashMap::new();
    match cid_to_gid_map(font, resolve, decode)? {
        Some(map) => {
            for (cid, gid) in map.chunks_exact(2).enumerate() {
                let gid = u16::from_be_bytes([gid[0], gid[1]]);
                if let Some(&character) = glyph_text.get(&gid) {
                    output.insert(cid as u32, character);
                }
            }
        }
        None => {
            for (gid, character) in glyph_text {
                output.insert(u32::from(gid), character);
            }
        }
    }
    Ok(output)
}

fn font_descriptor<E>(
    font: &Dictionary,
    resolve: &impl Fn(&Object) -> Result<Object, E>,
) -> Result<Option<Dictionary>, E> {
    if let Some(descriptor) = font.get(&Name::from("FontDescriptor")) {
        return Ok(match resolve(descriptor)? {
            Object::Dictionary(dictionary) => Some(dictionary),
            _ => None,
        });
    }
    if let Some(descendants) = font.get(&Name::from("DescendantFonts")) {
        if let Object::Array(array) = resolve(descendants)? {
            if let Some(first) = array.first() {
                if let Object::Dictionary(cid_font) = resolve(first)? {
                    return font_descriptor(&cid_font, resolve);
                }
            }
        }
    }
    Ok(None)
}

fn embedded_program<E>(
    descriptor: &Dictionary,
    resolve: &impl Fn(&Object) -> Result<Object, E>,
    decode: &impl Fn(&Stream) -> Result<Vec<u8>, E>,
) -> Result<Option<Vec<u8>>, E> {
    for key in ["FontFile", "FontFile2", "FontFile3"] {
        let Some(entry) = descriptor.get(&Name::from(key)) else {
            continue;
        };
        if let Object::Stream(stream) = resolve(entry)? {
            return decode(&stream).map(Some);
        }
    }
    Ok(None)
}

fn cid_to_gid_map<E>(
    font: &Dictionary,
    resolve: &impl Fn(&Object) -> Result<Object, E>,
    decode: &impl Fn(&Stream) -> Result<Vec<u8>, E>,
) -> Result<Option<Vec<u8>>, E> {
    let Some(descendants) = font.get(&Name::from("DescendantFonts")) else {
        return Ok(None);
    };
    let Object::Array(array) = resolve(descendants)? else {
        return Ok(None);
    };
    let Some(first) = array.first() else {
        return Ok(None);
    };
    let Object::Dictionary(cid_font) = resolve(first)? else {
        return Ok(None);
    };
    match cid_font.get(&Name::from("CIDToGIDMap")) {
        Some(entry) => match resolve(entry)? {
            Object::Stream(stream) => decode(&stream).map(Some),
            _ => Ok(None),
        },
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resources(font: Dictionary) -> Dictionary {
        let mut fonts = Dictionary::new();
        fonts.insert(Name::from("F1"), Object::Dictionary(font));
        let mut resources = Dictionary::new();
        resources.insert(Name::from("Font"), Object::Dictionary(fonts));
        resources
    }

    #[test]
    fn decodes_to_unicode_and_falls_back_to_latin1() {
        let cmap = b"1 beginbfchar <41> <03A9> endbfchar".to_vec();
        let mut font = Dictionary::new();
        font.insert(
            Name::from("ToUnicode"),
            Object::Stream(Stream::new(Dictionary::new(), cmap)),
        );
        let resolve = |object: &Object| Ok::<_, ()>(object.clone());
        let decode = |stream: &Stream| Ok::<_, ()>(stream.raw().to_vec());
        let decoder = ResourceDecoder::from_resources(&resources(font), &resolve, &decode).unwrap();

        assert_eq!(decoder.decode(Some("F1"), b"A"), "Ω");
        assert_eq!(decoder.decode(Some("Missing"), &[0xE9]), "é");
    }

    #[test]
    fn simple_encoding_and_untrusted_type0_are_safe() {
        let simple = ResourceDecoder::from_resources(
            &resources(Dictionary::new()),
            &|object| Ok::<_, ()>(object.clone()),
            &|stream| Ok::<_, ()>(stream.raw().to_vec()),
        )
        .unwrap();
        assert_eq!(simple.decode(Some("F1"), b"Hi"), "Hi");

        let mut type0 = Dictionary::new();
        type0.insert(Name::from("Subtype"), Object::Name(Name::from("Type0")));
        type0.insert(
            Name::from("Encoding"),
            Object::Name(Name::from("Unknown-CMap")),
        );
        let composite = ResourceDecoder::from_resources(
            &resources(type0),
            &|object| Ok::<_, ()>(object.clone()),
            &|stream| Ok::<_, ()>(stream.raw().to_vec()),
        )
        .unwrap();
        assert_eq!(composite.decode(Some("F1"), &[0, 1]), "");
    }
}
