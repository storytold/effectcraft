//! OpenEXR sequence frames: one single-part scan-line file per frame, RGBA, with the Output
//! Module's compression and sample type (#2911).
//!
//! Scan lines, not tiles: `image`'s encoder wrote 64×64 RLE tiles, which FilmCraft and other
//! scan-line readers refuse (#2805). Each compression keeps its own block height (1 line for
//! None, RLE and ZIPS; 16 for ZIP and PXR24; 32 for PIZ, B44, B44A and DWAA; 256 for DWAB) and the
//! `exr` crate's default levels for ZIP and DWA. Half-float samples are rounded from the 32-bit
//! render to the nearest half, ties to even. The lossy methods (PXR24 on float, B44 / B44A on half,
//! DWAA / DWAB at either depth) keep every linear sample within 0.03 + 6 % of its value in the
//! tests below. Half float holds at most ±65 504: larger samples are written as that limit, not
//! as infinity.

use std::io::{Seek, Write};

use effectcraft_project::render_queue::{ExrBitDepth, ExrCompression};
use exr::meta::attribute::Chromaticities;
use exr::prelude::{Blocks, Compression, Encoding, Image, Layer, LayerAttributes, LineOrder, SpecificChannels, Vec2, WritableImage, f16};

/// The largest finite half float.
const HALF_MAX: f32 = 65_504.0;

/// A sample as half float: rounded to the nearest half (ties to even), held to ±65 504 so an
/// over-range render doesn't turn into infinity. NaN stays NaN.
fn to_half(v: f32) -> f16 {
    f16::from_f32(v.clamp(-HALF_MAX, HALF_MAX))
}

/// The `exr` crate's compression for an Output Module setting.
pub fn compression(c: ExrCompression) -> Compression {
    match c {
        ExrCompression::None => Compression::Uncompressed,
        ExrCompression::Rle => Compression::RLE,
        ExrCompression::Zips => Compression::ZIP1,
        ExrCompression::Zip => Compression::ZIP16,
        ExrCompression::Piz => Compression::PIZ,
        ExrCompression::Pxr24 => Compression::PXR24,
        ExrCompression::B44 => Compression::B44,
        ExrCompression::B44a => Compression::B44A,
        ExrCompression::Dwaa => Compression::DWAA(None),
        ExrCompression::Dwab => Compression::DWAB(None),
    }
}

/// Scan lines, top to bottom, with compression `c`.
fn encoding(c: ExrCompression) -> Encoding {
    Encoding { compression: compression(c), blocks: Blocks::ScanLines, line_order: LineOrder::Increasing }
}

/// Write `w`×`h` RGBA pixels (`data`: four samples per pixel as they are to be written,
/// premultiplied, rows top to bottom) as one OpenEXR frame, its blocks compressed in parallel as before. A pixel `data`
/// doesn't cover is written as transparent black.
///
/// `chroma` is the primaries and white the samples are in (red, green, blue, white as CIE xy), written as the
/// file's `chromaticities` attribute for Nuke, After Effects and Blender; `None` writes no attribute.
pub fn write_rgba<W: Write + Seek>(
    file: &mut W,
    data: &[f32],
    (w, h): (usize, usize),
    (c, depth): (ExrCompression, ExrBitDepth),
    chroma: Option<[[f32; 2]; 4]>,
) -> exr::error::UnitResult {
    let chroma = chroma.map(|[red, green, blue, white]| {
        let v = |[x, y]: [f32; 2]| Vec2(x, y);
        Chromaticities { red: v(red), green: v(green), blue: v(blue), white: v(white) }
    });
    let row = w.saturating_mul(4);
    let px = move |p: Vec2<usize>| -> [f32; 4] {
        let o = p.y().saturating_mul(row).saturating_add(p.x().saturating_mul(4));
        match data.get(o..o.saturating_add(4)) {
            Some(&[r, g, b, a]) => [r, g, b, a],
            _ => [0.0; 4],
        }
    };
    match depth {
        ExrBitDepth::Float => {
            let mut image = Image::from_layer(Layer::new(
                (w, h),
                LayerAttributes::default(),
                encoding(c),
                SpecificChannels::rgba(move |p: Vec2<usize>| {
                    let [r, g, b, a] = px(p);
                    (r, g, b, a)
                }),
            ));
            image.attributes.chromaticities = chroma;
            image.write().to_buffered(file)
        }
        ExrBitDepth::Half => {
            let mut image = Image::from_layer(Layer::new(
                (w, h),
                LayerAttributes::default(),
                encoding(c),
                SpecificChannels::rgba(move |p: Vec2<usize>| {
                    let [r, g, b, a] = px(p).map(to_half);
                    (r, g, b, a)
                }),
            ));
            image.attributes.chromaticities = chroma;
            image.write().to_buffered(file)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use exr::prelude::{ReadChannels, ReadLayers};
    use exr::prelude::{SampleType, read};
    use std::io::Cursor;

    /// A frame with a smooth gradient, grain, over-range highlights, a hard-edged high-contrast
    /// block and a soft alpha ramp (premultiplied), like a render with text over a plate.
    fn frame(w: usize, h: usize) -> Vec<f32> {
        let mut s = 0x9e37_79b9u32;
        let mut grain = move || {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            (s as f32 / u32::MAX as f32 - 0.5) * 0.02
        };
        let mut d = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
                let a = ((1.0 - fx) * 3.0).clamp(0.0, 1.0);
                let text = (w / 3..2 * w / 3).contains(&x) && (h / 3..h / 2).contains(&y) && (x % 6 < 2 || y % 7 == 0);
                let c = if text { [6.0; 3] } else { [0.18 + 0.8 * fx, 0.05 + 0.6 * fy, 0.4 * (1.0 - fx) + 0.2 * fy] };
                let g = grain();
                d.extend_from_slice(&[(c[0] + g).max(0.0) * a, (c[1] + g).max(0.0) * a, (c[2] + g).max(0.0) * a, a]);
            }
        }
        d
    }

    fn write(data: &[f32], size: (usize, usize), c: ExrCompression, depth: ExrBitDepth) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        write_rgba(&mut buf, data, size, (c, depth), None).unwrap();
        buf.into_inner()
    }

    /// The samples of an RGBA file as f32, and the sample type it stores.
    fn read_back(bytes: &[u8]) -> (Vec<f32>, SampleType) {
        let image = read()
            .no_deep_data()
            .largest_resolution_level()
            .rgba_channels(
                |res, _| (res.width(), vec![0f32; res.width() * res.height() * 4]),
                |(w, v): &mut (usize, Vec<f32>), p: Vec2<usize>, (r, g, b, a): (f32, f32, f32, f32)| {
                    let o = (p.y() * *w + p.x()) * 4;
                    v[o..o + 4].copy_from_slice(&[r, g, b, a]);
                },
            )
            .first_valid_layer()
            .all_attributes()
            .from_buffered(Cursor::new(bytes))
            .unwrap();
        let ty = image.layer_data.channel_data.channels.0.sample_type;
        (image.layer_data.channel_data.pixels.1, ty)
    }

    /// Every compression at both depths reads back: the lossless ones exactly (half: exactly the
    /// f16 rounding of the render), the lossy ones within the tolerance the docs give. The project
    /// crate's lossy table matches the `exr` crate's own.
    #[test]
    fn every_compression_and_depth_round_trips() {
        let (w, h) = (97, 61);
        let src = frame(w, h);
        for depth in ExrBitDepth::ALL {
            let want: Vec<f32> = match depth {
                ExrBitDepth::Float => src.clone(),
                ExrBitDepth::Half => src.iter().map(|v| f16::from_f32(*v).to_f32()).collect(),
            };
            let ty = if depth == ExrBitDepth::Half { SampleType::F16 } else { SampleType::F32 };
            for c in ExrCompression::ALL {
                assert_eq!(c.is_lossy(depth), !compression(c).is_lossless_for(ty), "{c:?} {depth:?}");
                let bytes = write(&src, (w, h), c, depth);
                if c.is_lossy(depth) {
                    assert!(bytes.len() < write(&src, (w, h), ExrCompression::None, depth).len(), "{c:?} {depth:?}: smaller than None");
                }
                let (got, stored) = read_back(&bytes);
                assert_eq!(stored, ty, "{c:?} {depth:?}");
                assert_eq!(got.len(), want.len());
                if c.is_lossy(depth) {
                    for (i, (g, s)) in got.iter().zip(&want).enumerate() {
                        assert!((g - s).abs() <= 0.03 + 0.06 * s.abs(), "{c:?} {depth:?} sample {i}: {g} vs {s}");
                    }
                } else {
                    assert!(got.iter().zip(&want).all(|(g, s)| g.to_bits() == s.to_bits()), "{c:?} {depth:?} is not exact");
                }
            }
        }
    }

    /// Files are single-part scan-line images (no tile bit in the version field) with the chosen
    /// compression, also when the frame spans several blocks and ends in a partial one.
    #[test]
    fn frames_are_scan_lines_with_the_chosen_compression() {
        let (w, h) = (80, 300);
        let src = frame(w, h);
        for depth in ExrBitDepth::ALL {
            for c in ExrCompression::ALL {
                let bytes = write(&src, (w, h), c, depth);
                assert_eq!(&bytes[..4], &[0x76, 0x2f, 0x31, 0x01], "magic");
                assert_eq!(&bytes[4..6], &[2, 0], "{c:?} {depth:?}: version 2, no tile / multi-part flags");
                let meta = exr::meta::MetaData::read_from_buffered(Cursor::new(&bytes), false).unwrap();
                assert_eq!(meta.headers.len(), 1);
                assert_eq!(meta.headers[0].blocks, exr::meta::BlockDescription::ScanLines, "{c:?}");
                assert_eq!(meta.headers[0].compression, compression(c), "{c:?}");
                assert_eq!(meta.headers[0].line_order, LineOrder::Increasing);
            }
        }
    }

    /// The default (ZIP, 32-bit float) writes the `exr` crate's `SMALL_LOSSLESS` preset (scan
    /// lines, ZIP), the layout the Render Queue has written since #2812. The reference is that
    /// preset, not files from before #2812, which were tiled RLE.
    #[test]
    fn the_default_matches_the_small_lossless_preset_byte_for_byte() {
        let (w, h) = (64, 48);
        let src = frame(w, h);
        let pixels = SpecificChannels::rgba(|p: Vec2<usize>| {
            let o = p.y() * w * 4 + p.x() * 4;
            (src[o], src[o + 1], src[o + 2], src[o + 3])
        });
        let layer = Layer::new((w, h), LayerAttributes::default(), Encoding::SMALL_LOSSLESS, pixels);
        let mut before = Cursor::new(Vec::new());
        Image::from_layer(layer).write().to_buffered(&mut before).unwrap();
        let default = (ExrCompression::default(), ExrBitDepth::default());
        assert_eq!(default, (ExrCompression::Zip, ExrBitDepth::Float));
        assert_eq!(write(&src, (w, h), default.0, default.1), before.into_inner());
    }

    /// Half-float files hold samples above the half range at its largest finite value, not as
    /// infinity; the 32-bit file keeps them.
    #[test]
    fn half_clamps_over_range_samples_to_the_largest_half() {
        let data = [1.0e6, 70_000.0, 65_504.0, 1.0, -1.0e6, 0.5, f32::NAN, 1.0];
        let (half, _) = read_back(&write(&data, (2, 1), ExrCompression::Zip, ExrBitDepth::Half));
        assert_eq!(&half[..6], &[65_504.0, 65_504.0, 65_504.0, 1.0, -65_504.0, 0.5]);
        assert!(half[6].is_nan() && half.iter().all(|v| !v.is_infinite()));
        let (float, _) = read_back(&write(&data, (2, 1), ExrCompression::Zip, ExrBitDepth::Float));
        assert_eq!(&float[..2], &[1.0e6, 70_000.0]);
    }

    /// Data shorter than the frame (never produced by the Render Queue) writes transparent black
    /// for the missing pixels instead of panicking.
    #[test]
    fn short_data_is_padded_not_a_panic() {
        let (got, _) = read_back(&write(&[1.0, 0.5, 0.25, 1.0], (3, 2), ExrCompression::Zip, ExrBitDepth::Half));
        assert_eq!(&got[..4], &[1.0, 0.5, 0.25, 1.0]);
        assert!(got[4..].iter().all(|v| *v == 0.0));
    }

    /// The chromaticities attribute is written when given and absent otherwise.
    #[test]
    fn chromaticities_are_written_only_when_given() {
        let rec709 = [[0.64, 0.33], [0.30, 0.60], [0.15, 0.06], [0.3127, 0.3290]];
        for depth in [ExrBitDepth::Float, ExrBitDepth::Half] {
            let mut with = Cursor::new(Vec::new());
            write_rgba(&mut with, &[0.5; 4], (1, 1), (ExrCompression::Zip, depth), Some(rec709)).unwrap();
            let meta = exr::meta::MetaData::read_from_buffered(Cursor::new(with.into_inner()), false).unwrap();
            let got = meta.headers[0].shared_attributes.chromaticities.map(|c| [c.red, c.green, c.blue, c.white].map(|v| [v.0, v.1]));
            assert_eq!(got, Some(rec709));
            let meta = exr::meta::MetaData::read_from_buffered(Cursor::new(write(&[0.5; 4], (1, 1), ExrCompression::Zip, depth)), false).unwrap();
            assert!(meta.headers[0].shared_attributes.chromaticities.is_none());
        }
    }
}
