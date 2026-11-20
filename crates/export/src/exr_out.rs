//! Single-part and multipart OpenEXR scan-line frames.
//!
//! Scan lines, not tiles: `image`'s encoder wrote 64×64 RLE tiles, which FilmCraft and other
//! scan-line readers refuse (#2805). Each compression keeps its own block height (1 line for
//! None, RLE and ZIPS; 16 for ZIP and PXR24; 32 for PIZ, B44, B44A and DWAA; 256 for DWAB) and the
//! `exr` crate's default levels for ZIP and DWA. Half-float samples are rounded from the 32-bit
//! render to the nearest half, ties to even. The lossy methods (PXR24 on float, B44 / B44A on half,
//! DWAA / DWAB at either depth) keep every linear sample within 0.03 + 6 % of its value in the
//! tests below. Half float holds at most ±65 504: larger samples are written as that limit, not
//! as infinity.

use std::collections::{HashMap, HashSet};
use std::io::{Seek, Write};

use craft_exr::decode::{Compression, Error, Limits, Result, SampleDepth, SampleType, Window, f16};
use craft_exr::write::{Channel, Metadata, Part, RowSamples, write_scanlines_with_metadata};
use effectcraft_color::BlendMode;
use effectcraft_project::render_queue::{ExrBitDepth, ExrCompression};
use effectcraft_project::{ItemId, Layer, LayerId, Project};
use effectcraft_time::Tick;

/// Full channel names allow 255 bytes, including the two-byte component suffix.
const MAX_GROUP_NAME: usize = 253;
const BEAUTY_PART: &str = "Beauty";
const DEPTH_PART: &str = "Depth";

/// Interleaved rendered pixels shared by the single-part and multipart scan-line path.
pub struct Channels {
    size: (u32, u32),
    multipart: bool,
    groups: Vec<Group>,
    names: Vec<String>,
}

struct Group {
    part: Option<String>,
    pixels: Pixels,
}

enum Pixels {
    Rgba(Vec<[f32; 4]>, ExrBitDepth),
    Depth(Vec<f32>),
}

impl Pixels {
    fn channels(&self) -> (&'static [&'static str], SampleType) {
        match self {
            Self::Rgba(_, ExrBitDepth::Half) => (&["R", "G", "B", "A"], SampleType::F16),
            Self::Rgba(_, ExrBitDepth::Float) => (&["R", "G", "B", "A"], SampleType::F32),
            Self::Depth(_) => (&["Z"], SampleType::F32),
        }
    }

    fn fill_row(&self, start: usize, component: usize, samples: RowSamples<'_>) -> Result<usize> {
        let width = samples.len();
        let range = start..start.saturating_add(width);
        match (self, samples) {
            (Self::Rgba(pixels, _), RowSamples::F32(out)) => {
                let row = pixels.get(range).ok_or_else(invalid_row)?;
                for (dst, px) in out.iter_mut().zip(row) {
                    *dst = *px.get(component).ok_or_else(invalid_row)?;
                }
            }
            (Self::Rgba(pixels, _), RowSamples::F16(out)) => {
                let row = pixels.get(range).ok_or_else(invalid_row)?;
                for (dst, px) in out.iter_mut().zip(row) {
                    *dst = f16::from_f32(px.get(component).ok_or_else(invalid_row)?.clamp(-65_504.0, 65_504.0));
                }
            }
            (Self::Depth(pixels), RowSamples::F32(out)) => out.copy_from_slice(pixels.get(range).ok_or_else(invalid_row)?),
            _ => return Err(invalid_row()),
        }
        Ok(width)
    }
}

fn invalid_row() -> Error {
    Error::Encode("invalid OpenEXR channel row".into())
}

impl Channels {
    pub fn new(size: (u32, u32)) -> Self {
        Self { size, multipart: false, groups: Vec::new(), names: Vec::new() }
    }

    pub fn new_multipart(size: (u32, u32)) -> Self {
        Self { multipart: true, ..Self::new(size) }
    }

    /// Validate total output channels before rendering or allocating any planes.
    pub fn check_channel_count(&self, count: u64) -> Result<()> {
        let (w, h) = self.size;
        let pixels = u64::from(w) * u64::from(h);
        if w == 0 || h == 0 || count == 0 {
            return Err(Error::Encode("empty picture".into()));
        }
        let limits = Limits::default();
        let bytes = pixels.saturating_mul(count).saturating_mul(4);
        if w > limits.max_width || h > limits.max_height || pixels > limits.max_pixels || bytes > limits.max_bytes || usize::try_from(bytes).is_err() {
            return Err(Error::LimitExceeded("the selected channels exceed the OpenEXR frame size limit".into()));
        }
        Ok(())
    }

    fn check_samples(&self, len: usize, count: u64) -> Result<()> {
        let (w, h) = self.size;
        if usize::try_from(u64::from(w) * u64::from(h)).ok() != Some(len) {
            return Err(Error::Encode("a channel has the wrong number of samples".into()));
        }
        self.check_channel_count((self.names.len() as u64).saturating_add(count))
    }

    /// Add linear, premultiplied RGBA. An empty prefix denotes the composite.
    pub fn push_rgba(&mut self, prefix: &str, pixels: Vec<[f32; 4]>, depth: ExrBitDepth) -> Result<()> {
        self.check_samples(pixels.len(), 4)?;
        if !prefix.is_ascii() || prefix.len() > MAX_GROUP_NAME || prefix.bytes().any(|c| c.is_ascii_control()) {
            return Err(Error::Encode("invalid OpenEXR channel group name".into()));
        }
        if self.multipart && matches!(prefix, BEAUTY_PART | DEPTH_PART) {
            return Err(Error::Encode("a layer uses a reserved OpenEXR part name".into()));
        }
        let part = self.multipart.then(|| if prefix.is_empty() { BEAUTY_PART } else { prefix }.to_owned());
        self.push_group(prefix, part, Pixels::Rgba(pixels, depth));
        Ok(())
    }

    /// Add camera-space Z as FLOAT at every colour sample depth.
    pub fn push_depth(&mut self, samples: Vec<f32>) -> Result<()> {
        self.check_samples(samples.len(), 1)?;
        self.push_group("", self.multipart.then(|| DEPTH_PART.into()), Pixels::Depth(samples));
        Ok(())
    }

    fn push_group(&mut self, prefix: &str, part: Option<String>, pixels: Pixels) {
        let qualifier = part.as_deref().unwrap_or(prefix);
        self.names.extend(pixels.channels().0.iter().map(
            |component| {
                if qualifier.is_empty() { (*component).into() } else { format!("{qualifier}.{component}") }
            },
        ));
        self.groups.push(Group { part, pixels });
    }

    /// Names in group insertion order. The file stores its channel list in EXR sort order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.names.iter().map(String::as_str)
    }

    pub fn write<W: Write + Seek>(&self, file: &mut W, c: ExrCompression, chroma: Option<[[f32; 2]; 4]>) -> Result<()> {
        let window = Window { x: 0, y: 0, width: self.size.0, height: self.size.1 };
        let c = compression(c);
        let metadata = Metadata { chromaticities: chroma, ..Metadata::default() };
        let row_start = |row: usize| row.checked_mul(self.size.0 as usize).ok_or_else(invalid_row);
        if self.multipart {
            let channels: Vec<Vec<_>> = self
                .groups
                .iter()
                .map(|g| {
                    let (components, sample) = g.pixels.channels();
                    components.iter().map(|&name| Channel { name, sample }).collect()
                })
                .collect();
            let parts: Vec<_> = self
                .groups
                .iter()
                .zip(&channels)
                .map(|(g, channels)| Part { name: g.part.as_deref(), data_window: window, channels, compression: c })
                .collect();
            write_scanlines_with_metadata(file, window, &parts, &metadata, |part, row, channel, samples| {
                self.groups.get(part).ok_or_else(invalid_row)?.pixels.fill_row(row_start(row)?, channel, samples)
            })
        } else {
            let slots: Vec<_> = self
                .groups
                .iter()
                .flat_map(|g| {
                    let (components, sample) = g.pixels.channels();
                    (0..components.len()).map(move |component| (&g.pixels, component, sample))
                })
                .collect();
            let channels: Vec<_> = self.names().zip(&slots).map(|(name, &(_, _, sample))| Channel { name, sample }).collect();
            let part = Part { name: None, data_window: window, channels: &channels, compression: c };
            write_scanlines_with_metadata(file, window, &[part], &metadata, |_, row, channel, samples| {
                let &(pixels, component, _) = slots.get(channel).ok_or_else(invalid_row)?;
                pixels.fill_row(row_start(row)?, component, samples)
            })
        }
    }
}

/// Allocate unique group names in layer stack order. Dots delimit channels and OpenEXR text
/// requires storeable characters, so replace dots and non-printable/non-ASCII characters.
pub fn layer_names<'a>(layers: impl IntoIterator<Item = &'a Layer>, multipart: bool) -> HashMap<LayerId, String> {
    let (mut used, mut next) = (HashSet::new(), HashMap::new());
    if multipart {
        used.extend([BEAUTY_PART.to_owned(), DEPTH_PART.to_owned()]);
    }
    layers.into_iter().map(|l| (l.id, exr_name(&l.name, &mut used, &mut next))).collect()
}

fn exr_name(name: &str, used: &mut HashSet<String>, next: &mut HashMap<String, usize>) -> String {
    let base: String = name.chars().take(MAX_GROUP_NAME).map(|c| if c == '.' || !(c == ' ' || c.is_ascii_graphic()) { '_' } else { c }).collect();
    let base = if base.trim().is_empty() { "Layer".to_string() } else { base };
    if used.insert(base.clone()) {
        return base;
    }
    // Remember where each base's search resumes so repeated names don't make this quadratic.
    let k = next.entry(base.clone()).or_insert(2);
    loop {
        let suffix = format!(" {k}");
        let mut n: String = base.chars().take(MAX_GROUP_NAME.saturating_sub(suffix.len())).collect();
        n.push_str(&suffix);
        *k = k.saturating_add(1);
        if used.insert(n.clone()) {
            return n;
        }
    }
}

/// A project snapshot for rendering one isolated layer, with its original layer metadata.
pub struct IsolatedLayer {
    pub project: Project,
    pub name: String,
    pub blend: BlendMode,
    pub opacity: f64,
}

/// Prepare one layer to render solo at Normal and full opacity, retaining the comp's camera,
/// lights and matte sources.
pub fn isolate_layer(project: &Project, cid: ItemId, id: LayerId, t: Tick) -> Option<IsolatedLayer> {
    let mut project = project.clone();
    let comp = project.comp_mut(cid)?;
    for l in &mut comp.layers {
        l.switches.solo = l.id == id;
    }
    let layer = comp.layer_mut(id)?;
    let name = layer.name.clone();
    let blend = layer.blend_mode;
    let lt = layer.layer_time(t);
    let opacity = if let Some(prop) = layer.props.prop_mut("transform/opacity") {
        let opacity = prop.value_at(lt).as_f64() / 100.0;
        prop.keys.clear();
        prop.expr = None;
        prop.value = effectcraft_project::Value::Scalar(100.0);
        opacity.clamp(0.0, 1.0)
    } else {
        1.0
    };
    layer.blend_mode = BlendMode::Normal;
    Some(IsolatedLayer { project, name, blend, opacity })
}

/// The `craft-exr` compression for an Output Module setting.
pub fn compression(c: ExrCompression) -> Compression {
    match c {
        ExrCompression::None => Compression::None,
        ExrCompression::Rle => Compression::Rle,
        ExrCompression::Zips => Compression::Zip1,
        ExrCompression::Zip => Compression::Zip16,
        ExrCompression::Piz => Compression::Piz,
        ExrCompression::Pxr24 => Compression::Pxr24,
        ExrCompression::B44 => Compression::B44,
        ExrCompression::B44a => Compression::B44a,
        ExrCompression::Dwaa => Compression::Dwaa,
        ExrCompression::Dwab => Compression::Dwab,
    }
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
) -> Result<()> {
    let sample_depth = match depth {
        ExrBitDepth::Float => SampleDepth::Float,
        ExrBitDepth::Half => SampleDepth::Half,
    };
    craft_exr::decode::write_rgba(file, data, w as u32, h as u32, sample_depth, compression(c), chroma)
}

#[cfg(test)]
mod tests {
    use super::*;
    use exr::prelude::{
        Encoding, Image, Layer, LayerAttributes, LineOrder, ReadChannels, ReadLayers, SampleType, SpecificChannels, Vec2, WritableImage, f16, read,
    };
    use std::io::Cursor;

    #[test]
    fn many_layers_of_one_name_take_a_name_each_in_order() {
        let (mut used, mut next) = (HashSet::new(), HashMap::new());
        // `A 3` is taken by a layer of its own before the repeats reach it.
        let names: Vec<String> = ["A", "A", "A 3", "A", "A.b"].iter().map(|n| exr_name(n, &mut used, &mut next)).collect();
        assert_eq!(names, ["A", "A 2", "A 3", "A 4", "A_b"]);
        // 20 000 layers of one name: each found without retrying the suffixes before it.
        for _ in 0..20_000 {
            exr_name("Same", &mut used, &mut next);
        }
        assert_eq!(next["Same"], 20_001);
    }

    fn exr_compression(c: ExrCompression) -> exr::compression::Compression {
        use exr::compression::Compression as C;
        match c {
            ExrCompression::None => C::Uncompressed,
            ExrCompression::Rle => C::RLE,
            ExrCompression::Zips => C::ZIP1,
            ExrCompression::Zip => C::ZIP16,
            ExrCompression::Piz => C::PIZ,
            ExrCompression::Pxr24 => C::PXR24,
            ExrCompression::B44 => C::B44,
            ExrCompression::B44a => C::B44A,
            ExrCompression::Dwaa => C::DWAA(None),
            ExrCompression::Dwab => C::DWAB(None),
        }
    }

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
                assert_eq!(c.is_lossy(depth), !exr_compression(c).is_lossless_for(ty), "{c:?} {depth:?}");
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
                assert_eq!(meta.headers[0].compression, exr_compression(c), "{c:?}");
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
