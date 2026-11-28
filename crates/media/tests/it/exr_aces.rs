//! An OpenEXR file tagged with the ACES primaries (its `chromaticities` attribute) imports in that
//! ACES space, so its linear values are converted from AP0 / AP1 instead of read as Rec. 709;
//! every other file keeps the default interpretation.

use std::io::Cursor;

use effectcraft_media::probe_bytes;
use effectcraft_project::ColorSpace;
use exr::meta::attribute::Chromaticities;
use exr::prelude::*;

fn layer(
    name: Option<&str>,
) -> Layer<
    SpecificChannels<impl Fn(Vec2<usize>) -> (f32, f32, f32, f32) + Sync, (ChannelDescription, ChannelDescription, ChannelDescription, ChannelDescription)>,
> {
    let attrs = name.map_or_else(LayerAttributes::default, LayerAttributes::named);
    Layer::new((4, 4), attrs, Encoding::UNCOMPRESSED, SpecificChannels::rgba(|_| (0.8f32, 0.3, 0.1, 1.0)))
}

fn tagged(space: Option<ColorSpace>) -> Option<Chromaticities> {
    space.map(|s| {
        let [r, g, b, w] = s.exr_chromaticities().map(|[x, y]| Vec2(x, y));
        Chromaticities { red: r, green: g, blue: b, white: w }
    })
}

/// A single-part RGBA file whose header names `space`'s primaries (none when `None`).
fn single(space: Option<ColorSpace>) -> Vec<u8> {
    let mut image = Image::from_layer(layer(None));
    image.attributes.chromaticities = tagged(space);
    let mut out = Vec::new();
    image.write().to_buffered(Cursor::new(&mut out)).unwrap();
    out
}

fn profile(bytes: Vec<u8>) -> Option<ColorSpace> {
    probe_bytes("render.exr", bytes.into()).unwrap().color_profile
}

#[test]
fn exr_tagged_with_aces_primaries_imports_in_that_space() {
    assert_eq!(profile(single(Some(ColorSpace::Aces2065))), Some(ColorSpace::Aces2065), "AP0");
    assert_eq!(profile(single(Some(ColorSpace::AcesCg))), Some(ColorSpace::AcesCg), "AP1");
    // Untagged files (Rec. 709 by the OpenEXR convention) and other primaries keep the default.
    assert_eq!(profile(single(None)), None);
    assert_eq!(profile(single(Some(ColorSpace::Rec709))), None);
    assert_eq!(profile(single(Some(ColorSpace::Rec2020))), None);
    // A multi-part file still imports; its tag isn't read yet.
    let mut multi = Image::empty(ImageAttributes::new(IntegerBounds::from_dimensions((4, 4)))).with_layer(layer(Some("left"))).with_layer(layer(Some("right")));
    multi.attributes.chromaticities = tagged(Some(ColorSpace::Aces2065));
    let mut bytes = Vec::new();
    multi.write().to_buffered(Cursor::new(&mut bytes)).unwrap();
    assert_eq!(profile(bytes), None);
}
