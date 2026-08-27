//! Color spaces (ISO 32000-1 §8.6), to the extent needed to interpret image samples.
//!
//! This first slice models the device spaces and reduces other spaces to a component count, which
//! is all image extraction needs to know (how many samples per pixel). [`Separation`] additionally
//! captures the **Separation**/**DeviceN** special spaces (§8.6.6): their colorant names, the
//! alternate device space, and the tint-transform [`Function`](crate::Function) that maps tint
//! values into it.

use pdf_cos::{Array, Dictionary, Name, Object, Stream};

use crate::{Function, parse_function};

/// A color space, identified by how many components each sample carries (§8.6).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColorSpace {
    /// `DeviceGray` / `CalGray` — 1 component (§8.6.4.2).
    DeviceGray,
    /// `DeviceRGB` / `CalRGB` — 3 components (§8.6.4.3).
    DeviceRgb,
    /// `DeviceCMYK` — 4 components (§8.6.4.4).
    DeviceCmyk,
    /// Any other space (Indexed, Separation, DeviceN, …) reduced to its component count.
    Other(u8),
}

impl ColorSpace {
    /// Map a color-space *name* (`DeviceRGB`, `G`, …) to a space, or `None` if unrecognised.
    #[must_use]
    pub fn from_name(name: &[u8]) -> Option<Self> {
        Some(match name {
            b"DeviceGray" | b"CalGray" | b"G" => ColorSpace::DeviceGray,
            b"DeviceRGB" | b"CalRGB" | b"RGB" => ColorSpace::DeviceRgb,
            b"DeviceCMYK" | b"CMYK" => ColorSpace::DeviceCmyk,
            _ => return None,
        })
    }

    /// Map a component count to the matching device space, else [`ColorSpace::Other`]. Used for
    /// `ICCBased` spaces, whose `/N` gives the component count (§8.6.5.5).
    #[must_use]
    pub fn from_components(n: u8) -> Self {
        match n {
            1 => ColorSpace::DeviceGray,
            3 => ColorSpace::DeviceRgb,
            4 => ColorSpace::DeviceCmyk,
            other => ColorSpace::Other(other),
        }
    }

    /// The number of components per sample.
    #[must_use]
    pub fn components(self) -> u8 {
        match self {
            ColorSpace::DeviceGray => 1,
            ColorSpace::DeviceRgb => 3,
            ColorSpace::DeviceCmyk => 4,
            ColorSpace::Other(n) => n,
        }
    }
}

/// A **Separation** or **DeviceN** colour space (§8.6.6): one or more named colorants whose tint
/// values are converted into an `alternate` device space by a tint-transform function.
///
/// Separation is the single-colorant case (`components() == 1`); DeviceN carries *N* colorants.
/// [`Separation::to_alternate`] runs the tint transform, yielding the alternate space's components
/// (e.g. CMYK values when `alternate()` is [`ColorSpace::DeviceCmyk`]).
#[derive(Clone, Debug)]
pub struct Separation {
    names: Vec<String>,
    alternate: ColorSpace,
    tint_transform: Function,
}

impl Separation {
    /// Build a Separation/DeviceN space from its colorant names, alternate space, and tint transform.
    #[must_use]
    pub fn new(names: Vec<String>, alternate: ColorSpace, tint_transform: Function) -> Self {
        Separation {
            names,
            alternate,
            tint_transform,
        }
    }

    /// Number of input colorants (1 for Separation, *N* for DeviceN).
    #[must_use]
    pub fn components(&self) -> usize {
        self.names.len()
    }

    /// The colorant names (e.g. `["Cyan"]`, or `["All"]`, or custom spot-colour names).
    #[must_use]
    pub fn colorant_names(&self) -> &[String] {
        &self.names
    }

    /// The alternate device space the tint transform produces (§8.6.6.3).
    #[must_use]
    pub fn alternate(&self) -> ColorSpace {
        self.alternate
    }

    /// Convert `tints` (one value per colorant, each typically in `[0, 1]`) into the alternate
    /// space's component values by running the tint-transform function (§8.6.6.4).
    #[must_use]
    pub fn to_alternate(&self, tints: &[f64]) -> Vec<f64> {
        self.tint_transform.eval(tints)
    }
}

/// An Indexed colour space resolved from `[/Indexed base hival lookup]` (§8.6.6.3).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct IndexedColorSpace {
    /// The base colour space the palette entries use.
    pub base: ColorSpace,
    /// The highest valid palette index.
    pub hival: u8,
    /// Flat `(hival + 1) × base.components()` palette bytes.
    pub lookup: Vec<u8>,
}

impl IndexedColorSpace {
    /// Return the base-space components for palette index `i`.
    #[must_use]
    pub fn entry(&self, i: u8) -> Option<&[u8]> {
        let components = self.base.components() as usize;
        let start = (i as usize).checked_mul(components)?;
        self.lookup.get(start..start + components)
    }
}

/// Resolve an image dictionary's `/ColorSpace` to its component model (§8.6). An absent entry is
/// DeviceGray. `resolve` supplies indirect-object resolution without coupling graphics to a DOM.
pub fn resolve_image_color_space<E>(
    image: &Dictionary,
    resolve: &impl Fn(&Object) -> Result<Object, E>,
) -> Result<ColorSpace, E> {
    match image.get(&Name::from("ColorSpace")) {
        Some(object) => resolve_color_space(object, resolve),
        None => Ok(ColorSpace::DeviceGray),
    }
}

/// Resolve a name or array colour-space object to its component model (§8.6).
pub fn resolve_color_space<E>(
    object: &Object,
    resolve: &impl Fn(&Object) -> Result<Object, E>,
) -> Result<ColorSpace, E> {
    match resolve(object)? {
        Object::Name(name) => {
            Ok(ColorSpace::from_name(name.as_bytes()).unwrap_or(ColorSpace::Other(1)))
        }
        Object::Array(family) => resolve_color_space_array(&family, resolve),
        _ => Ok(ColorSpace::DeviceGray),
    }
}

fn resolve_color_space_array<E>(
    family: &Array,
    resolve: &impl Fn(&Object) -> Result<Object, E>,
) -> Result<ColorSpace, E> {
    let name = match family.first() {
        Some(Object::Name(name)) => name.as_bytes(),
        _ => return Ok(ColorSpace::Other(1)),
    };
    Ok(match name {
        b"ICCBased" => match family.get(1).map(resolve).transpose()? {
            Some(Object::Stream(icc)) => {
                let components = icc
                    .dict()
                    .get_integer(&Name::from("N"))
                    .unwrap_or(3)
                    .clamp(1, 8) as u8;
                ColorSpace::from_components(components)
            }
            _ => ColorSpace::Other(3),
        },
        b"CalRGB" => ColorSpace::DeviceRgb,
        b"CalGray" => ColorSpace::DeviceGray,
        b"DeviceN" => match family.get(1).map(resolve).transpose()? {
            Some(Object::Array(names)) => ColorSpace::Other(names.len().clamp(1, 32) as u8),
            _ => ColorSpace::Other(1),
        },
        b"Lab" => ColorSpace::Other(3),
        b"Indexed" | b"I" | b"Separation" => ColorSpace::Other(1),
        _ => ColorSpace::Other(1),
    })
}

/// Resolve an Indexed colour space (§8.6.6.3). `decode` handles a stream lookup table while
/// keeping filter policy in the caller.
pub fn resolve_indexed<E>(
    object: &Object,
    resolve: &impl Fn(&Object) -> Result<Object, E>,
    decode: &impl Fn(&Stream) -> Result<Vec<u8>, E>,
) -> Result<Option<IndexedColorSpace>, E> {
    let Object::Array(family) = resolve(object)? else {
        return Ok(None);
    };
    if !matches!(family.first(), Some(Object::Name(n)) if matches!(n.as_bytes(), b"Indexed" | b"I"))
        || family.len() < 4
    {
        return Ok(None);
    }
    let base = resolve_color_space(&family[1], resolve)?;
    let Some(hival) = resolve(&family[2])?.as_integer() else {
        return Ok(None);
    };
    let lookup = match resolve(&family[3])? {
        Object::String(string) => string.as_bytes().to_vec(),
        Object::Stream(stream) => decode(&stream)?,
        _ => return Ok(None),
    };
    Ok(Some(IndexedColorSpace {
        base,
        hival: hival.clamp(0, 255) as u8,
        lookup,
    }))
}

/// Resolve a Separation or DeviceN colour space (§8.6.6), including its tint function.
pub fn resolve_separation<E>(
    object: &Object,
    resolve: &impl Fn(&Object) -> Result<Object, E>,
) -> Result<Option<Separation>, E> {
    let Object::Array(family) = resolve(object)? else {
        return Ok(None);
    };
    let kind = match family.first() {
        Some(Object::Name(name)) => name.as_bytes(),
        _ => return Ok(None),
    };
    let (names, alternate_index, tint_index) = match kind {
        b"Separation" => {
            let Some(Object::Name(name)) = family.get(1).map(resolve).transpose()? else {
                return Ok(None);
            };
            (
                vec![String::from_utf8_lossy(name.as_bytes()).into_owned()],
                2,
                3,
            )
        }
        b"DeviceN" => {
            let Some(Object::Array(array)) = family.get(1).map(resolve).transpose()? else {
                return Ok(None);
            };
            let names: Vec<String> = array
                .iter()
                .filter_map(|object| match object {
                    Object::Name(name) => {
                        Some(String::from_utf8_lossy(name.as_bytes()).into_owned())
                    }
                    _ => None,
                })
                .collect();
            if names.is_empty() {
                return Ok(None);
            }
            (names, 2, 3)
        }
        _ => return Ok(None),
    };
    let Some(alternate) = family.get(alternate_index) else {
        return Ok(None);
    };
    let alternate = resolve_color_space(alternate, resolve)?;
    let Some(tint) = family.get(tint_index) else {
        return Ok(None);
    };
    let Some(tint) = parse_function(tint, &|object| resolve(object).ok()) else {
        return Ok(None);
    };
    Ok(Some(Separation::new(names, alternate, tint)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdf_cos::Stream;

    #[test]
    fn names_and_components() {
        assert_eq!(
            ColorSpace::from_name(b"DeviceRGB"),
            Some(ColorSpace::DeviceRgb)
        );
        assert_eq!(ColorSpace::from_name(b"G"), Some(ColorSpace::DeviceGray));
        assert_eq!(
            ColorSpace::from_name(b"DeviceCMYK"),
            Some(ColorSpace::DeviceCmyk)
        );
        assert_eq!(ColorSpace::from_name(b"Bogus"), None);

        assert_eq!(ColorSpace::from_components(1), ColorSpace::DeviceGray);
        assert_eq!(ColorSpace::from_components(3), ColorSpace::DeviceRgb);
        assert_eq!(ColorSpace::from_components(4), ColorSpace::DeviceCmyk);
        assert_eq!(ColorSpace::from_components(2), ColorSpace::Other(2));
        assert_eq!(ColorSpace::DeviceGray.components(), 1);
        assert_eq!(ColorSpace::DeviceRgb.components(), 3);
        assert_eq!(ColorSpace::DeviceCmyk.components(), 4);
        assert_eq!(ColorSpace::Other(5).components(), 5);
    }

    #[test]
    fn resolves_devicen_components_and_tint_function() {
        let mut tint = Dictionary::new();
        tint.insert(Name::from("FunctionType"), Object::Integer(4));
        tint.insert(
            Name::from("Domain"),
            Object::Array(Array::from([0.0, 1.0, 0.0, 1.0].map(Object::Real).to_vec())),
        );
        tint.insert(
            Name::from("Range"),
            Object::Array(Array::from(
                [0.0, 1.0, 0.0, 1.0, 0.0, 1.0].map(Object::Real).to_vec(),
            )),
        );
        let device_n = Object::Array(Array::from(vec![
            Object::Name(Name::from("DeviceN")),
            Object::Array(Array::from(vec![
                Object::Name(Name::from("Spot1")),
                Object::Name(Name::from("Spot2")),
            ])),
            Object::Name(Name::from("DeviceRGB")),
            Object::Stream(Stream::new(tint, b"{ 0 }".to_vec())),
        ]));
        let resolve = |object: &Object| Ok::<_, ()>(object.clone());

        assert_eq!(
            resolve_color_space(&device_n, &resolve).unwrap(),
            ColorSpace::Other(2)
        );
        let separation = resolve_separation(&device_n, &resolve).unwrap().unwrap();
        assert_eq!(separation.components(), 2);
        assert_eq!(separation.alternate(), ColorSpace::DeviceRgb);
        assert_eq!(separation.to_alternate(&[0.3, 0.7]), vec![0.3, 0.7, 0.0]);
    }
}
