//! PDF/A OutputIntent and the bundled sRGB ICC profile (ISO 32000-1 §14.11.5).
//!
//! PDF/A requires a destination output profile so device-dependent colour (DeviceRGB/Gray) is
//! reproducible. We bundle a small public-domain sRGB ICC profile and expose helpers to wrap it as
//! an ICC profile stream and to build the `GTS_PDFA1` OutputIntent dictionary that references it.
//! The conformant-output pass allocates the object IDs and assembles these into the catalog.

use pdf_cos::{Dictionary, Name, Object, ObjectId, PdfString, Stream};

/// A minimal, accurate sRGB (IEC 61966-2.1) ICC v2 profile, embedded as the OutputIntent's
/// `/DestOutputProfile`. Three colour components (RGB). CC0; provenance is recorded in the
/// repository's `THIRD-PARTY-NOTICES.md` §2.1.
pub const SRGB_ICC: &[u8] = include_bytes!("../assets/sRGB-v2-magic.icc");

/// The number of colour components in [`SRGB_ICC`].
pub const SRGB_ICC_N: u32 = 3;

/// Build the `/DestOutputProfile` ICC stream (§14.11.5 / §8.6.5.5): the raw ICC bytes with the
/// component count `/N`. Left uncompressed (small profile; no filter dependency).
#[must_use]
pub fn icc_profile_stream(profile: &[u8], n: u32) -> Stream {
    let mut dict = Dictionary::new();
    dict.insert(Name::from("N"), Object::Integer(i64::from(n)));
    Stream::new(dict, profile.to_vec())
}

/// The sRGB ICC profile stream ready to embed.
#[must_use]
pub fn srgb_icc_stream() -> Stream {
    icc_profile_stream(SRGB_ICC, SRGB_ICC_N)
}

/// The destination profile a PDF/A `GTS_PDFA1` OutputIntent characterises (§14.11.5): the ICC bytes,
/// their colour-component count `n` (1 = Gray, 3 = RGB, 4 = CMYK), and the output-condition
/// identifier (e.g. `"sRGB"`, `"FOGRA39"`).
///
/// The choice is what makes a device colour space admissible under PDF/A §6.2.4.3: `DeviceRGB`
/// content needs an RGB OutputIntent (or a `/DefaultRGB`), `DeviceCMYK` a CMYK one, etc. Use
/// [`OutputIntentProfile::srgb`] for the bundled sRGB profile; supply your own bytes (e.g. a CMYK
/// printing-condition profile) with [`OutputIntentProfile::new`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputIntentProfile {
    icc: Vec<u8>,
    n: u32,
    identifier: String,
}

impl OutputIntentProfile {
    /// A profile from raw ICC `icc` bytes with `n` colour components and output-condition
    /// `identifier`. The caller is responsible for `n` matching the profile's data colour space and
    /// for the content's device colour spaces matching the chosen profile (PDF/A §6.2.4.3).
    #[must_use]
    pub fn new(icc: impl Into<Vec<u8>>, n: u32, identifier: impl Into<String>) -> Self {
        Self {
            icc: icc.into(),
            n,
            identifier: identifier.into(),
        }
    }

    /// The bundled sRGB (DeviceRGB) profile — the default for documents using DeviceRGB/Gray colour.
    #[must_use]
    pub fn srgb() -> Self {
        Self::new(SRGB_ICC.to_vec(), SRGB_ICC_N, "sRGB")
    }

    /// The raw ICC profile bytes.
    #[must_use]
    pub fn icc(&self) -> &[u8] {
        &self.icc
    }

    /// The colour-component count (`/N`).
    #[must_use]
    pub fn n(&self) -> u32 {
        self.n
    }

    /// The output-condition identifier.
    #[must_use]
    pub fn identifier(&self) -> &str {
        &self.identifier
    }
}

/// Build a `GTS_PDFA1` OutputIntent dictionary (§14.11.5) referencing the ICC profile object
/// `dest_output_profile`. `identifier` is the `/OutputConditionIdentifier` (e.g. `"sRGB"`).
#[must_use]
pub fn output_intent_dict(dest_output_profile: ObjectId, identifier: &str) -> Dictionary {
    let mut d = Dictionary::new();
    d.insert(Name::from("Type"), Object::Name(Name::from("OutputIntent")));
    d.insert(Name::from("S"), Object::Name(Name::from("GTS_PDFA1")));
    d.insert(
        Name::from("OutputConditionIdentifier"),
        Object::String(PdfString::from(identifier.as_bytes().to_vec())),
    );
    d.insert(
        Name::from("Info"),
        Object::String(PdfString::from(identifier.as_bytes().to_vec())),
    );
    d.insert(
        Name::from("DestOutputProfile"),
        Object::Reference(dest_output_profile),
    );
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_profile_is_a_valid_icc_header() {
        // ICC: 'acsp' signature at offset 36, 'RGB ' data colour space at offset 16.
        assert_eq!(&SRGB_ICC[36..40], b"acsp");
        assert_eq!(&SRGB_ICC[16..20], b"RGB ");
        assert_eq!(SRGB_ICC_N, 3);
    }

    #[test]
    fn icc_stream_carries_component_count() {
        let stream = srgb_icc_stream();
        assert_eq!(
            stream.dict().get(&Name::from("N")),
            Some(&Object::Integer(3))
        );
        assert_eq!(stream.raw().as_ref(), SRGB_ICC);
    }

    #[test]
    fn srgb_profile_wraps_the_bundled_asset() {
        let p = OutputIntentProfile::srgb();
        assert_eq!(p.icc(), SRGB_ICC);
        assert_eq!(p.n(), SRGB_ICC_N);
        assert_eq!(p.identifier(), "sRGB");
    }

    #[test]
    fn custom_profile_carries_its_fields() {
        // A stand-in for a future CMYK printing-condition profile (4 components).
        let p = OutputIntentProfile::new(vec![1, 2, 3, 4], 4, "FOGRA39");
        assert_eq!(p.icc(), &[1, 2, 3, 4]);
        assert_eq!(p.n(), 4);
        assert_eq!(p.identifier(), "FOGRA39");
    }

    #[test]
    fn output_intent_has_required_keys() {
        let d = output_intent_dict(ObjectId::new(7, 0), "sRGB");
        assert_eq!(
            d.get(&Name::from("S")),
            Some(&Object::Name(Name::from("GTS_PDFA1")))
        );
        assert_eq!(
            d.get(&Name::from("DestOutputProfile")),
            Some(&Object::Reference(ObjectId::new(7, 0)))
        );
        assert_eq!(
            d.get(&Name::from("OutputConditionIdentifier")),
            Some(&Object::String(PdfString::from(b"sRGB".to_vec())))
        );
    }
}
