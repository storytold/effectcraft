//! Auxiliary channels from multi-layer OpenEXR files (depth, object / material IDs, normals,
//! Cryptomatte, …) for the 3D Channel effects, read with the pure-Rust `exr` crate.
//!
//! Every channel of every layer becomes a plane named `layer.channel` (or just `channel` for
//! the unnamed layer), placed in the display window (pixels outside a layer's data window are
//! 0, or the background depth for depth channels). Cryptomatte manifests come from the
//! `cryptomatte/<key>/name` and `cryptomatte/<key>/manifest` header attributes (the published
//! Cryptomatte metadata convention: the manifest is a JSON object of name → hex hash).
//!
//! Decoded files are kept in a small cache ([`CACHE_BUDGET`]) that the picture of an EXR
//! footage item shares with EXtractoR / Cryptomatte once an effect has read its channels
//! (#474); a file is decoded once however many callers ask for it at the same time.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use effectcraft_raster::channels3d::{BACKGROUND_DEPTH, split_channel};
use effectcraft_raster::{AuxChannels, Image};
use effectcraft_render::reads::{ReadFile, ReadFiles};
use exr::block::UncompressedBlock;
use exr::block::chunk::TileCoordinates;
use exr::image::read::image::ReadImage;
use exr::image::read::layers::{ChannelsReader, ReadAllLayers, ReadChannels};
use exr::meta::attribute::SampleType;
use exr::meta::header::Header;

use crate::convert::{AlphaOp, float_to_image};

/// The largest display window decoded (16384²; vector footage and Continuously Rasterize
/// stop at the same size).
const MAX_DISPLAY_PIXELS: usize = 1 << 28;

fn is_depth(name: &str) -> bool {
    let last = name.rsplit('.').next().unwrap_or(name);
    last.eq_ignore_ascii_case("z") || name.to_ascii_lowercase().contains("depth")
}

/// Full plane name of a channel in a part: `part.channel`, except that Blender 5 writes one part
/// per pass whose channels already carry the full name (part `ViewLayer.Depth` holds
/// `ViewLayer.Depth.Z`), which is kept as is instead of doubling the prefix.
fn channel_name(part: Option<&str>, channel: &str) -> String {
    match part {
        Some(p) if channel.strip_prefix(p).is_some_and(|rest| rest.starts_with('.')) => channel.to_string(),
        Some(p) => format!("{p}.{channel}"),
        None => channel.to_string(),
    }
}

/// The part name channels are prefixed with (`None`: unnamed).
fn part_name(header: &Header) -> Option<String> {
    header.own_attributes.layer_name.as_ref().map(|t| t.to_string()).filter(|s| !s.is_empty())
}

/// Parse a Cryptomatte manifest (`{"name": "hexhash", …}`).
pub fn parse_manifest(json: &str) -> Vec<(String, u32)> {
    let Ok(serde_json::Value::Object(m)) = serde_json::from_str::<serde_json::Value>(json) else { return vec![] };
    let mut v: Vec<(String, u32)> = m.iter().filter_map(|(k, h)| Some((k.clone(), u32::from_str_radix(h.as_str()?.trim(), 16).ok()?))).collect();
    v.sort();
    v
}

/// Why an OpenEXR file's pixels can't be decoded when its headers name a compression this build
/// can't decompress (HTJ2K, OpenEXR 3.4's High-Throughput JPEG 2000): the decoders only said
/// "no non-deep rgb channels" or showed a transparent frame (#482). `None` for other files.
pub fn unsupported_compression(bytes: &[u8]) -> Option<&'static str> {
    use exr::compression::Compression;
    let meta = exr::meta::MetaData::read_from_buffered(std::io::Cursor::new(bytes), false).ok()?;
    meta.headers
        .iter()
        .any(|h| matches!(h.compression, Compression::HTJ2K32 | Compression::HTJ2K256))
        .then_some("OpenEXR HTJ2K compression isn't supported yet: re-save the file with ZIP, PIZ or DWAA compression")
}

/// Which channels a decode keeps, by plane name, and the display window they are placed in
/// (the first part's, as the `exr` crate reports the image's).
struct Planes<'k> {
    keep: &'k dyn Fn(&str) -> bool,
    display: exr::meta::attribute::IntegerBounds,
}

/// One part's kept channels while it is decoded: each line of samples goes straight into its
/// channel's display-window plane (no copy of the part's samples in their own type first).
struct PlanesReader {
    /// Per channel of the part: its plane name, sample type and plane (`None`: not kept).
    planes: Vec<Option<(String, SampleType, Vec<f32>)>>,
    /// The data window's position in the display window, and the data window's and display
    /// window's sizes.
    offset: (i64, i64),
    layer: (usize, usize),
    display: (usize, usize),
    f16s: Vec<exr::prelude::f16>,
    u32s: Vec<u32>,
    f32s: Vec<f32>,
    /// The part's channel indices of exact `R`, `G`, `B` and `A`, when it has R, G and B.
    rgb: Option<([usize; 3], Option<usize>)>,
    /// The part's display window differs from the first part's.
    own_display: bool,
}

/// A decoded part: its kept planes (in channel order) and what [`PlanesReader`] found.
struct PartPlanes {
    planes: Vec<(String, Vec<f32>)>,
    /// Positions in `planes` of the part's R, G, B (and A) channels, when it has them and keeps
    /// them all.
    rgb: Option<([usize; 3], Option<usize>)>,
    has_rgb: bool,
    exact: bool,
}

impl<'s> ReadChannels<'s> for Planes<'_> {
    type Reader = PlanesReader;

    fn create_channels_reader(&'s self, header: &Header) -> exr::error::Result<PlanesReader> {
        if header.deep {
            return Err(exr::error::Error::NotSupported("deep data".into()));
        }
        let dw = self.display;
        let display = (dw.size.0, dw.size.1);
        // Each channel is a display-window plane: a 60000² window over a 1×1 data window was a
        // multi-gigabyte allocation per channel (#2141).
        let n = display.0.checked_mul(display.1).filter(|n| *n > 0 && *n <= MAX_DISPLAY_PIXELS).ok_or(exr::error::Error::Invalid("display window".into()))?;
        let prefix = part_name(header);
        let pos = header.own_attributes.layer_position;
        let offset = (i64::from(pos.0) - i64::from(dw.position.0), i64::from(pos.1) - i64::from(dw.position.1));
        let planes = header
            .channels
            .list
            .iter()
            .map(|ch| {
                let name = channel_name(prefix.as_deref(), &ch.name.to_string());
                let fill = if is_depth(&name) { BACKGROUND_DEPTH } else { 0.0 };
                (self.keep)(&name).then(|| (name, ch.sample_type, vec![fill; n]))
            })
            .collect();
        let find = |c: &str| header.channels.find_index_of_channel(&exr::meta::attribute::Text::from(c));
        let rgb = match (find("R"), find("G"), find("B")) {
            (Some(r), Some(g), Some(b)) => Some(([r, g, b], find("A"))),
            _ => None,
        };
        Ok(PlanesReader {
            planes,
            offset,
            layer: (header.layer_size.0, header.layer_size.1),
            display,
            f16s: vec![],
            u32s: vec![],
            f32s: vec![],
            rgb,
            own_display: header.shared_attributes.display_window != dw,
        })
    }
}

impl ChannelsReader for PlanesReader {
    type Channels = PartPlanes;

    fn filter_block(&self, tile: TileCoordinates) -> bool {
        tile.is_largest_resolution_level() && self.planes.iter().any(Option::is_some)
    }

    fn read_block(&mut self, header: &Header, block: UncompressedBlock) -> exr::error::UnitResult {
        let ((lw, lh), (w, h)) = (self.layer, self.display);
        for line in block.lines(&header.channels) {
            let Some(Some((_, kind, plane))) = self.planes.get_mut(line.location.channel) else { continue };
            let (x0, y, count) = (line.location.position.0, line.location.position.1, line.location.sample_count);
            // The samples of data-window row `y`, columns `x0..x0 + count`, at display row `ty`
            // from display column `tx0`; clipped to the display window.
            if y >= lh || x0.checked_add(count).is_none_or(|end| end > lw) {
                continue;
            }
            let Some(ty) = usize::try_from(y as i64 + self.offset.1).ok().filter(|ty| *ty < h) else { continue };
            let tx0 = x0 as i64 + self.offset.0;
            let skip = usize::try_from(-tx0).unwrap_or(0).min(count);
            let tx = usize::try_from(tx0.max(0)).unwrap_or(usize::MAX);
            let take = (count - skip).min(w.saturating_sub(tx));
            let start = ty.checked_mul(w).and_then(|row| row.checked_add(tx));
            let Some(dst) = start.filter(|_| take > 0).and_then(|s| plane.get_mut(s..s.checked_add(take)?)) else { continue };
            match kind {
                SampleType::F16 => {
                    self.f16s.resize(count, exr::prelude::f16::ZERO);
                    line.read_samples_into_slice(&mut self.f16s)?;
                    for (d, s) in dst.iter_mut().zip(self.f16s.iter().skip(skip)) {
                        *d = s.to_f32();
                    }
                }
                SampleType::F32 => {
                    self.f32s.resize(count, 0.0);
                    line.read_samples_into_slice(&mut self.f32s)?;
                    for (d, s) in dst.iter_mut().zip(self.f32s.iter().skip(skip)) {
                        *d = *s;
                    }
                }
                SampleType::U32 => {
                    self.u32s.resize(count, 0);
                    line.read_samples_into_slice(&mut self.u32s)?;
                    for (d, s) in dst.iter_mut().zip(self.u32s.iter().skip(skip)) {
                        *d = *s as f32;
                    }
                }
            }
        }
        Ok(())
    }

    fn into_channels(self) -> PartPlanes {
        // Positions of the kept channels among the kept planes.
        let mut at = vec![None; self.planes.len()];
        let mut planes = Vec::with_capacity(self.planes.len());
        for (i, p) in self.planes.into_iter().enumerate() {
            if let Some((name, _, data)) = p {
                if let Some(slot) = at.get_mut(i) {
                    *slot = Some(planes.len());
                }
                planes.push((name, data));
            }
        }
        let kept = |i: usize| at.get(i).copied().flatten();
        let has_rgb = self.rgb.is_some();
        let rgb = self.rgb.and_then(|([r, g, b], a)| {
            let a = match a {
                Some(a) => Some(kept(a)?),
                None => None,
            };
            Some(([kept(r)?, kept(g)?, kept(b)?], a))
        });
        PartPlanes { planes, rgb, has_rgb, exact: !self.own_display }
    }
}

/// How the `image` crate's OpenEXR decoder would show a file: the first part with exact `R`, `G`
/// and `B` channels (alpha `A` when it has one, else opaque), placed in the display window with
/// 0 outside the data window.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Rgb {
    /// No part has R, G and B: the decoder refuses the file (a multi-layer file, shown with
    /// [`layered_image`]'s rule).
    None,
    /// The channels (indices in [`AuxChannels::channels`]) hold exactly what it would show.
    Planes([usize; 3], Option<usize>),
    /// The planes can't stand in for it (a depth-named channel, whose plane is filled with the
    /// background depth, or another display window): decode with the `image` crate.
    Unsupported,
}

/// A decoded file: its channels and how its picture is made of them.
pub(crate) struct Decoded {
    pub(crate) aux: Arc<AuxChannels>,
    rgb: Rgb,
}

impl Decoded {
    /// Bytes held (the planes; names and manifests are small).
    fn bytes(&self) -> usize {
        self.aux.channels.iter().map(|(n, p)| n.len() + std::mem::size_of_val(p.as_slice())).sum()
    }
}

/// The most bytes of planes one decode allocates (every kept channel is a display-window plane
/// of f32, allocated before any pixels are read): a header can ask for far more than the file
/// holds.
#[cfg(target_pointer_width = "64")]
const MAX_DECODE_BYTES: usize = 8 << 30;
#[cfg(not(target_pointer_width = "64"))]
const MAX_DECODE_BYTES: usize = 512 << 20;

/// Why a file's channels weren't decoded.
#[derive(Debug, PartialEq)]
enum Refused {
    /// Not a readable OpenEXR file.
    Unreadable,
    /// Its kept channels need more memory than [`MAX_DECODE_BYTES`] (the reason, to show).
    TooLarge(String),
}

/// Bytes the planes of the channels `keep` names would take (`None`: not a readable header).
fn planes_bytes(meta: &exr::meta::MetaData, keep: &dyn Fn(&str) -> bool) -> Option<usize> {
    let dw = meta.headers.first()?.shared_attributes.display_window;
    let n = dw.size.0.checked_mul(dw.size.1)?;
    let mut kept = 0usize;
    for header in meta.headers.iter() {
        let prefix = part_name(header);
        kept = kept.saturating_add(header.channels.list.iter().filter(|ch| keep(&channel_name(prefix.as_deref(), &ch.name.to_string()))).count());
    }
    Some(n.saturating_mul(kept).saturating_mul(std::mem::size_of::<f32>()))
}

/// Decode the channels of an EXR file's bytes that `keep` names (by plane name).
fn decode(bytes: &[u8], keep: &dyn Fn(&str) -> bool) -> Result<Decoded, Refused> {
    let meta = exr::meta::MetaData::read_from_buffered(std::io::Cursor::new(bytes), false).map_err(|_| Refused::Unreadable)?;
    let display = meta.headers.first().ok_or(Refused::Unreadable)?.shared_attributes.display_window;
    let need = planes_bytes(&meta, keep).ok_or(Refused::Unreadable)?;
    if need > MAX_DECODE_BYTES {
        let gb = |b: usize| b as f64 / f64::from(1u32 << 30);
        return Err(Refused::TooLarge(format!(
            "its {}×{} OpenEXR channels need {:.1} GB of memory, more than the {:.1} GB one file may use",
            display.size.0,
            display.size.1,
            gb(need),
            gb(MAX_DECODE_BYTES)
        )));
    }
    let read = ReadImage::new(ReadAllLayers { read_channels: Planes { keep, display } }, |_| {});
    let img = read.from_buffered(std::io::Cursor::new(bytes)).map_err(|_| Refused::Unreadable)?;
    let dw = img.attributes.display_window;
    let mut aux = AuxChannels::new(dw.size.0 as u32, dw.size.1 as u32, 1.0);
    let mut crypto: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
    let mut collect_meta = |other: &HashMap<exr::meta::attribute::Text, exr::meta::attribute::AttributeValue>| {
        for (k, v) in other {
            let key = k.to_string();
            let Some(rest) = key.strip_prefix("cryptomatte/") else { continue };
            let Some((id, field)) = rest.split_once('/') else { continue };
            let exr::meta::attribute::AttributeValue::Text(t) = v else { continue };
            let e = crypto.entry(id.to_string()).or_default();
            match field {
                "name" => e.0 = Some(t.to_string()),
                "manifest" => e.1 = Some(t.to_string()),
                _ => {}
            }
        }
    };
    collect_meta(&img.attributes.other);
    let mut rgb = Rgb::None;
    for layer in img.layer_data {
        collect_meta(&layer.attributes.other);
        let part = layer.channel_data;
        if rgb == Rgb::None && part.has_rgb {
            let base = aux.channels.len();
            rgb = match part.rgb {
                Some(([r, g, b], a)) if part.exact => {
                    let picked = [Some(r), Some(g), Some(b), a];
                    let depth = picked.iter().flatten().any(|i| part.planes.get(*i).is_some_and(|(n, _)| is_depth(n)));
                    if depth { Rgb::Unsupported } else { Rgb::Planes([base + r, base + g, base + b], a.map(|a| base + a)) }
                }
                _ => Rgb::Unsupported,
            };
        }
        aux.channels.extend(part.planes);
    }
    for (_, (name, manifest)) in crypto {
        if let (Some(n), Some(m)) = (name, manifest) {
            aux.manifests.push((n, parse_manifest(&m)));
        }
    }
    aux.manifests.sort();
    Ok(Decoded { aux: Arc::new(aux), rgb })
}

/// Read all channels of an EXR file's bytes. `None` when it is not a readable EXR.
pub fn read_exr_channels(bytes: &[u8]) -> Option<AuxChannels> {
    let aux = decode(bytes, &|_| true).ok()?.aux;
    Some(Arc::try_unwrap(aux).unwrap_or_else(|a| (*a).clone()))
}

/// Names of the beauty pass, most preferred first, matched against the last part of a layer's
/// name (Blender's `ViewLayer.Combined` is `Combined`): Blender's Combined pass and its
/// compositor's File Output default `Image`, the beauty / rgba / rgb / color of Nuke, Arnold and
/// V-Ray.
const BEAUTY: [&str; 7] = ["combined", "beauty", "image", "rgba", "rgb", "color", "colour"];

/// A data pass, never the picture: Cryptomatte (`CryptoMaterial00`), depth, mist, normals,
/// motion vectors, positions, UVs and object / material indices.
fn is_data_layer(layer: &str) -> bool {
    let last = layer.rsplit('.').next().unwrap_or(layer).to_ascii_lowercase();
    last.starts_with("crypto")
        || ["z", "uv", "id", "mist", "indexob", "indexma"].contains(&last.as_str())
        || ["depth", "normal", "vector", "position", "velocity", "motion", "objectid", "materialid"].iter().any(|k| last.contains(k))
}

/// The layer whose colour a multi-layer file shows (see [`layered_image`]).
fn picture_layer<'a>(aux: &AuxChannels, layers: &[&'a str]) -> Option<&'a str> {
    // Colour layers: three different channels (not one channel, such as depth, shown in all).
    let rgb: Vec<&str> = layers
        .iter()
        .copied()
        .filter(|l| {
            let [r, g, b, _] = aux.layer_rgba(l);
            !r.is_empty() && !g.is_empty() && !b.is_empty() && r != g
        })
        .collect();
    let last = |l: &str| l.rsplit('.').next().unwrap_or(l).to_ascii_lowercase();
    let pictures = || rgb.iter().filter(|l| !is_data_layer(l));
    // The unnamed layer (plain R, G, B), then the beauty pass by name.
    rgb.iter()
        .find(|l| l.is_empty())
        .or_else(|| BEAUTY.iter().find_map(|k| rgb.iter().find(|l| last(l) == *k)))
        .or_else(|| pictures().find(|l| ["combined", "beauty"].iter().any(|k| last(l).contains(k))))
        .or_else(|| pictures().next())
        .or_else(|| layers.iter().find(|l| !is_data_layer(l)))
        .or_else(|| rgb.first())
        .or_else(|| layers.first())
        .copied()
}

/// The planes a multi-layer file's picture is made of (see [`layered_image`]): red, green, blue
/// (one channel thrice for grey) and alpha, by name. Only the names of `aux` are read.
fn picture_names(aux: &AuxChannels) -> Option<([String; 3], Option<String>)> {
    let layers = aux.layers();
    let colour = picture_layer(aux, &layers)?;
    let names = aux.layer_rgba(colour);
    // A layer without R, G and B channels shows its first channel as grey.
    let first = aux.names().into_iter().find(|n| split_channel(n).0 == colour).unwrap_or_default();
    let pick = |k: usize| names.get(k).map(String::as_str).filter(|n| !n.is_empty()).unwrap_or(first).to_string();
    let a = names.get(3).filter(|n| !n.is_empty() && aux.get(n).is_some()).cloned();
    Some(([pick(0), pick(1), pick(2)], a))
}

/// The channels of the picture of a multi-layer file and nothing else (names first from the
/// headers, then only those channels decoded; in a multi-part file the other parts' pixels are
/// not even decompressed), with the names of its planes.
fn picture_only(bytes: &[u8]) -> Result<(Decoded, [String; 3], Option<String>), Refused> {
    let meta = exr::meta::MetaData::read_from_buffered(std::io::Cursor::new(bytes), false).map_err(|_| Refused::Unreadable)?;
    let mut names = AuxChannels::new(0, 0, 1.0);
    for header in meta.headers.iter() {
        let prefix = part_name(header);
        names.channels.extend(header.channels.list.iter().map(|ch| (channel_name(prefix.as_deref(), &ch.name.to_string()), vec![])));
    }
    let (rgb, a) = picture_names(&names).ok_or(Refused::Unreadable)?;
    // Planes are looked up by name ignoring case, so every channel that could answer is kept.
    let keep = |n: &str| rgb.iter().chain(&a).any(|k| k.eq_ignore_ascii_case(n));
    let decoded = decode(bytes, &keep)?;
    Ok((decoded, rgb, a))
}

/// The picture of a multi-layer OpenEXR file without an unnamed RGB layer (Blender's
/// `ViewLayer.Combined.R`, the compositor's `Image.R`, Nuke's `beauty.R`, …), which the image
/// decoder rejects. After Effects shows a file's main RGBA channels and leaves the other layers
/// to EXtractoR; here that is the beauty pass by name (`BEAUTY`), else the first colour layer
/// that is not a data pass (never Cryptomatte, depth or normals, #412), else the first channel
/// as grey. Linear, straight RGBA as the file stores it. `None` when it is not a readable EXR.
pub fn layered_image(bytes: &[u8]) -> Option<image::DynamicImage> {
    let (d, rgb, a) = picture_only(bytes).ok()?;
    let aux = &d.aux;
    let [r, g, b] = [aux.get(&rgb[0])?, aux.get(&rgb[1])?, aux.get(&rgb[2])?];
    let a = match &a {
        Some(n) => Some(aux.get(n)?),
        None => None,
    };
    let n = (aux.width as usize).checked_mul(aux.height as usize)?;
    let mut data = Vec::with_capacity(n.checked_mul(4)?);
    for i in 0..n {
        data.extend([r.get(i)?, g.get(i)?, b.get(i)?].map(|v| *v));
        if let Some(a) = a {
            data.push(*a.get(i)?);
        }
    }
    match a {
        Some(_) => image::Rgba32FImage::from_raw(aux.width, aux.height, data).map(image::DynamicImage::ImageRgba32F),
        None => image::Rgb32FImage::from_raw(aux.width, aux.height, data).map(image::DynamicImage::ImageRgb32F),
    }
}

/// The picture made of planes `rgb` and `a` (opaque without one) of `aux`, as
/// [`crate::convert::dynamic_to_image`] makes it of the same values. `None` when a plane is
/// missing or short.
fn planes_picture(aux: &AuxChannels, rgb: [&[f32]; 3], a: Option<&[f32]>, op: AlphaOp) -> Option<Image> {
    let n = (aux.width as usize).checked_mul(aux.height as usize)?;
    if rgb.iter().chain(&a).any(|p| p.len() < n) {
        return None;
    }
    let [r, g, b] = rgb;
    let at = |p: &[f32], i: usize| p.get(i).copied().unwrap_or(0.0);
    Some(float_to_image(aux.width, aux.height, op, |i| [at(r, i), at(g, i), at(b, i), a.map_or(1.0, |a| at(a, i))]))
}

/// The picture of a decoded file by [`layered_image`]'s rule.
fn layered_picture_of(aux: &AuxChannels, op: AlphaOp) -> Option<Image> {
    let (rgb, a) = picture_names(aux)?;
    let a = match &a {
        Some(n) => Some(aux.get(n)?),
        None => None,
    };
    planes_picture(aux, [aux.get(&rgb[0])?, aux.get(&rgb[1])?, aux.get(&rgb[2])?], a, op)
}

/// The picture of the multi-layer EXR at `path` ([`layered_image`]'s rule) as an [`Image`]:
/// from the channel cache when `shared` (an effect reads the footage's channels, so they are
/// decoded anyway), else of the picture's channels alone (also when the cache has no channels
/// for the file: a read that failed, or a broken part the picture doesn't use). `Err(Some(why))`
/// when the file is too large to decode, `Err(None)` when it isn't a readable multi-layer EXR.
pub(crate) fn layered_picture(path: &str, bytes: Arc<[u8]>, file: &ReadFile, op: AlphaOp, shared: bool) -> Result<Image, Option<String>> {
    if shared && let Some(d) = cache().get(path, Use::Picture, || read_as(file, bytes.clone())) {
        return layered_picture_of(&d.aux, op).ok_or(None);
    }
    let (d, rgb, a) = picture_only(&bytes).map_err(|r| match r {
        Refused::TooLarge(why) => Some(why),
        Refused::Unreadable => None,
    })?;
    let aux = &d.aux;
    let plane = |n: &str| aux.get(n).ok_or(None);
    let a = match &a {
        Some(n) => Some(plane(n)?),
        None => None,
    };
    planes_picture(aux, [plane(&rgb[0])?, plane(&rgb[1])?, plane(&rgb[2])?], a, op).ok_or(None)
}

/// The picture of the EXR at `path` from the channel cache (#474: EXtractoR / Cryptomatte read
/// this footage's channels, so the picture shares their decode): what the `image` crate's
/// decoder shows for a file it reads, [`layered_image`]'s rule for one it refuses. `None` when
/// the planes can't stand in for the `image` crate's picture (decode with it), or the file is
/// not a readable EXR.
pub(crate) fn cached_picture(path: &str, bytes: Arc<[u8]>, file: &ReadFile, op: AlphaOp) -> Option<Image> {
    let d = cache().get(path, Use::Picture, || read_as(file, bytes))?;
    let aux = &d.aux;
    match d.rgb {
        Rgb::None => layered_picture_of(aux, op),
        Rgb::Planes([r, g, b], a) => {
            let plane = |i: usize| aux.channels.get(i).map(|(_, p)| p.as_slice());
            let a = match a {
                Some(a) => Some(plane(a)?),
                None => None,
            };
            planes_picture(aux, [plane(r)?, plane(g)?, plane(b)?], a, op)
        }
        Rgb::Unsupported => None,
    }
}

/// How many bytes of decoded channels the cache keeps, like the frame cache's default
/// ([`crate::DEFAULT_BUDGET`]): two 2560×1440 files of 32 channels (470 MB each, as every plane
/// is f32). A soft bound: the newest file is kept whatever its size (so a frame's picture and its
/// effects share its decode), and files still in use are never dropped (planes an effect holds,
/// [`HELD_FILES`] pictures waiting for their effects, decodes in progress), so memory can go
/// above it while they are. (Measured on such a sequence decoded one frame at a time: 1 GiB held
/// peak memory about 1 GB lower than 2 GiB, at the same speed.)
#[cfg(not(target_arch = "wasm32"))]
pub const CACHE_BUDGET: usize = 1 << 30;
#[cfg(target_arch = "wasm32")]
pub const CACHE_BUDGET: usize = 512 << 20;

/// At most this many files are cached (unreadable ones hold no bytes).
const CACHE_FILES: usize = 64;

/// At most this many files whose picture was made of their planes are kept for their effects
/// (the renderer asks for a frame's picture, then its channels): about as many frames as are
/// rendered at once. The oldest stops being kept when another comes.
const HELD_FILES: usize = 4;

/// A file's decode: done once, by the first caller; the others wait for it. With the files read
/// for it (see [`crate::reads`]), noted again on every hit.
type Slot = Arc<OnceLock<(Option<Arc<Decoded>>, ReadFiles)>>;

/// Bytes the caller already read from `file`, given to the cache as read from it (so the entry
/// keeps the file's identity).
fn read_as(file: &ReadFile, bytes: Arc<[u8]>) -> Option<Arc<[u8]>> {
    crate::reads::note(file.clone());
    Some(bytes)
}

struct Entry {
    path: String,
    slot: Slot,
    /// Bytes of the finished decode, or the planes it will allocate while it decodes.
    bytes: usize,
    /// When it was last asked for or finished decoding (a counter).
    used: u64,
}

impl Entry {
    /// Dropping it would free nothing or lose a decode in progress: an effect (or a picture
    /// waiting for its effects) still holds its planes, or it is decoding.
    fn in_use(&self) -> bool {
        match self.slot.get() {
            None => true,
            Some((None, _)) => false,
            Some((Some(d), _)) => Arc::strong_count(d) > 1 || Arc::strong_count(&d.aux) > 1,
        }
    }
}

struct State {
    entries: Vec<Entry>,
    clock: u64,
    /// Files whose picture was made of their planes and whose channels haven't been asked for
    /// since, oldest first (at most [`HELD_FILES`]).
    held: std::collections::VecDeque<(String, Arc<Decoded>)>,
}

/// Who asks for a file's channels.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Use {
    /// The picture of a footage item whose effects read its channels next: kept for them.
    Picture,
    /// The channels themselves (EXtractoR, Cryptomatte, the 3D Channel effects).
    Channels,
}

/// Decoded EXR files by path, bounded by bytes, least recently used out first.
pub(crate) struct ChannelCache {
    state: Mutex<State>,
    budget: usize,
    /// Files decoded (for tests).
    decodes: AtomicU64,
}

/// Removes a decode's entry if the decode panics, so the file doesn't stay "decoding" (never
/// dropped, its estimated bytes counted) until someone asks for it again.
struct Abandon<'a> {
    cache: &'a ChannelCache,
    slot: &'a Slot,
}

impl Drop for Abandon<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.cache.lock().entries.retain(|e| !Arc::ptr_eq(&e.slot, self.slot));
        }
    }
}

impl ChannelCache {
    pub(crate) const fn new(budget: usize) -> Self {
        Self { state: Mutex::new(State { entries: Vec::new(), clock: 0, held: std::collections::VecDeque::new() }), budget, decodes: AtomicU64::new(0) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Set the bytes of the entry of `slot` (if it's still cached), mark it used now and drop
    /// least recently used files until the rest fit the budget.
    fn account(&self, slot: &Slot, bytes: usize) {
        let mut state = self.lock();
        state.clock += 1;
        let clock = state.clock;
        // (gone when the file was forgotten meanwhile) Used as of now: a file asked for before
        // another but decoded after it is not the older one.
        if let Some(e) = state.entries.iter_mut().find(|e| Arc::ptr_eq(&e.slot, slot)) {
            e.bytes = bytes;
            e.used = clock;
        }
        Self::evict(&mut state.entries, self.budget, slot);
    }

    /// The decoded file at `path`, read with `read` by the first caller; callers asking for the
    /// same file meanwhile wait for that decode instead of decoding it again.
    pub(crate) fn get(&self, path: &str, by: Use, read: impl FnOnce() -> Option<Arc<[u8]>>) -> Option<Arc<Decoded>> {
        let slot = {
            let mut state = self.lock();
            state.clock += 1;
            let clock = state.clock;
            match state.entries.iter_mut().find(|e| e.path == path) {
                Some(e) => {
                    e.used = clock;
                    e.slot.clone()
                }
                None => {
                    let slot = Slot::default();
                    state.entries.push(Entry { path: path.to_string(), slot: slot.clone(), bytes: 0, used: clock });
                    slot
                }
            }
        };
        let mut decoded_here = false;
        // Waiting here can block a rayon worker (which `Inner::frame_at` avoids for frames): that
        // is safe because the decode never waits on rayon's global pool (the exr crate
        // decompresses on a pool of its own and this thread only receives the blocks), so the
        // decoding thread can't be waiting for this one.
        let (v, files) = slot.get_or_init(|| {
            decoded_here = true;
            self.decodes.fetch_add(1, Ordering::Relaxed);
            #[cfg(test)]
            count_decode(true);
            let _abandon = Abandon { cache: self, slot: &slot };
            let (bytes, files) = crate::reads::captured(read);
            (bytes.and_then(|bytes| self.decode_file(path, &slot, &bytes)), files)
        });
        // The file(s) the decode was made of, read now or for an earlier caller.
        crate::reads::note_all(files);
        let v = v.clone();
        if decoded_here {
            self.account(&slot, v.as_ref().map_or(0, |d| d.bytes()));
        }
        let mut state = self.lock();
        match (by, &v) {
            (Use::Picture, Some(d)) => {
                if !state.held.iter().any(|(p, _)| p == path) {
                    state.held.push_back((path.to_string(), d.clone()));
                    if state.held.len() > HELD_FILES {
                        state.held.pop_front();
                    }
                }
            }
            (Use::Channels, _) => state.held.retain(|(p, _)| p != path),
            (Use::Picture, None) => {}
        }
        drop(state);
        v
    }

    /// Decode the file at `path` whose bytes are `bytes` for `slot` (see [`ChannelCache::get`]).
    fn decode_file(&self, path: &str, slot: &Slot, bytes: &[u8]) -> Option<Arc<Decoded>> {
        // The planes it will allocate count against the budget while it decodes.
        if let Ok(meta) = exr::meta::MetaData::read_from_buffered(std::io::Cursor::new(bytes), false)
            && let Some(need) = planes_bytes(&meta, &|_| true)
        {
            self.account(slot, need.min(MAX_DECODE_BYTES));
        }
        match decode(bytes, &|_| true) {
            Ok(d) => Some(Arc::new(d)),
            Err(Refused::TooLarge(why)) => {
                log::warn!("media: {path}: {why}");
                None
            }
            Err(Refused::Unreadable) => None,
        }
    }

    /// Drop least recently used files not in use until the rest fit the budget, keeping `newest`.
    fn evict(entries: &mut Vec<Entry>, budget: usize, newest: &Slot) {
        while entries.iter().map(|e| e.bytes).sum::<usize>() > budget || entries.len() > CACHE_FILES {
            let oldest = entries.iter().enumerate().filter(|(_, e)| !Arc::ptr_eq(&e.slot, newest) && !e.in_use()).min_by_key(|(_, e)| e.used).map(|(i, _)| i);
            let Some(i) = oldest else { break };
            entries.remove(i);
        }
    }

    pub(crate) fn forget(&self, path: &str) {
        let mut state = self.lock();
        state.entries.retain(|e| e.path != path);
        state.held.retain(|(p, _)| p != path);
    }

    #[cfg(test)]
    fn decodes(&self) -> u64 {
        self.decodes.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    fn cached(&self) -> Vec<String> {
        self.lock().entries.iter().map(|e| e.path.clone()).collect()
    }
}

fn cache() -> &'static ChannelCache {
    static CACHE: ChannelCache = ChannelCache::new(CACHE_BUDGET);
    &CACHE
}

/// Drop the cached channels of `path` (the file changed on disk).
pub(crate) fn forget(path: &str) {
    cache().forget(path);
}

/// Stop keeping the decodes of the last pictures for their effects ([`HELD_FILES`]): which
/// footage effects read changed, so they may wait for effects that are gone. Their entries stay
/// cached as any other, least recently used out first.
pub(crate) fn release_held() {
    cache().lock().held.clear();
}

/// Read (and cache, see [`CACHE_BUDGET`]) the channels of the EXR at `path` via `read`.
pub(crate) fn cached(path: &str, read: impl FnOnce() -> Option<Arc<[u8]>>) -> Option<Arc<AuxChannels>> {
    cache().get(path, Use::Channels, read).map(|d| d.aux.clone())
}

#[cfg(test)]
thread_local! {
    /// Decodes of a still's pixels on this thread ([`count_decode`]): the `image` crate's, then
    /// the channel cache's.
    static DECODES: std::cell::Cell<[u64; 2]> = const { std::cell::Cell::new([0; 2]) };
}

/// Count a decode of a file's pixels on this thread (tests): by the channel cache (`channels`)
/// or by the `image` crate.
#[cfg(test)]
pub(crate) fn count_decode(channels: bool) {
    DECODES.with(|d| {
        let mut n = d.get();
        n[usize::from(channels)] += 1;
        d.set(n);
    });
}

/// The decodes [`count_decode`] counted on this thread since the last call: by the `image` crate
/// and by the channel cache.
#[cfg(test)]
pub(crate) fn take_decodes() -> [u64; 2] {
    DECODES.with(|d| d.replace([0; 2]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An EXR file of `layers` (with their display window `display`).
    fn file(layers: Vec<exr::prelude::Layer<exr::prelude::AnyChannels<exr::prelude::FlatSamples>>>, display: exr::prelude::IntegerBounds) -> Vec<u8> {
        use exr::prelude::*;
        let image = Image::from_layers(ImageAttributes::new(display), layers);
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write().to_buffered(&mut bytes).unwrap();
        bytes.into_inner()
    }

    /// A single-part file of float channels named `names`, `w`×`h`, every sample `v`.
    fn flat(names: &[&str], (w, h): (usize, usize), v: f32) -> Vec<u8> {
        use exr::prelude::*;
        let list: Vec<AnyChannel<FlatSamples>> = names.iter().map(|n| AnyChannel::new(*n, FlatSamples::F32(vec![v; w * h]))).collect();
        let layer = Layer::new((w, h), LayerAttributes::default(), Encoding::FAST_LOSSLESS, AnyChannels::sort(list.into()));
        file(vec![layer], IntegerBounds::from_dimensions((w, h)))
    }

    #[test]
    fn reads_layered_channels_and_cryptomatte_metadata() {
        use exr::prelude::*;
        let (w, h) = (4usize, 2usize);
        let z: Vec<f32> = (0..w * h).map(|i| i as f32 * 10.0).collect();
        let id: Vec<f32> = (0..w * h).map(|i| (i % 2) as f32 + 1.0).collect();
        let r: Vec<f32> = vec![0.5; w * h];
        let channels = AnyChannels::sort(
            vec![
                AnyChannel::new("R", FlatSamples::F32(r)),
                AnyChannel::new("Z", FlatSamples::F32(z.clone())),
                AnyChannel::new("ObjectID", FlatSamples::F32(id.clone())),
            ]
            .into(),
        );
        let mut layer = Layer::new((w, h), LayerAttributes::named("depth"), Encoding::FAST_LOSSLESS, channels);
        layer.attributes.other.insert(Text::from("cryptomatte/abc1234/name"), AttributeValue::Text(Text::from("CryptoObject")));
        layer
            .attributes
            .other
            .insert(Text::from("cryptomatte/abc1234/manifest"), AttributeValue::Text(Text::from(r#"{"bunny":"13851a76","floor":"0a1b2c3d"}"#)));
        let image = Image::from_layer(layer);
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write().to_buffered(&mut bytes).unwrap();
        let aux = read_exr_channels(bytes.get_ref()).unwrap();
        assert_eq!((aux.width, aux.height), (4, 2));
        assert_eq!(aux.get("depth.Z").unwrap(), z.as_slice());
        assert_eq!(aux.depth().unwrap(), z.as_slice());
        assert_eq!(aux.object_id().unwrap(), id.as_slice());
        assert_eq!(aux.manifests, vec![("CryptoObject".to_string(), vec![("bunny".to_string(), 0x1385_1a76), ("floor".to_string(), 0x0a1b_2c3d)])]);
        assert!(read_exr_channels(b"not an exr").is_none());
    }

    /// #474: each line of samples goes straight into its display-window plane: a data window
    /// inside the display window is placed with the fill around it (the background depth for
    /// depth), one partly outside is clipped, never out of bounds; half and uint samples are
    /// converted as the file stores them.
    #[test]
    fn channels_are_placed_in_the_display_window() {
        use exr::prelude::*;
        let half = |x: f32| f16::from_f32(x);
        let part = |pos: (i32, i32), size: (usize, usize), list: Vec<AnyChannel<FlatSamples>>| {
            let attrs = LayerAttributes { layer_position: Vec2(pos.0, pos.1), ..Default::default() };
            Layer::new(size, attrs, Encoding::FAST_LOSSLESS, AnyChannels::sort(list.into()))
        };
        let display = IntegerBounds::from_dimensions((3, 2));
        // A 2×1 data window at (1, 1).
        let inside = file(
            vec![part(
                (1, 1),
                (2, 1),
                vec![AnyChannel::new("G", FlatSamples::F16(vec![half(1.0), half(2.0)])), AnyChannel::new("Z", FlatSamples::U32(vec![7, 8]))],
            )],
            display,
        );
        let aux = read_exr_channels(&inside).unwrap();
        assert_eq!(aux.get("G").unwrap(), [0.0, 0.0, 0.0, 0.0, 1.0, 2.0]);
        let bg = BACKGROUND_DEPTH;
        assert_eq!(aux.get("Z").unwrap(), [bg, bg, bg, bg, 7.0, 8.0]);
        // A 2×2 data window at (2, -1): only its bottom-left sample lands at (2, 0).
        let off = file(vec![part((2, -1), (2, 2), vec![AnyChannel::new("G", FlatSamples::F32(vec![1.0, 2.0, 3.0, 4.0]))])], display);
        assert_eq!(read_exr_channels(&off).unwrap().get("G").unwrap(), [0.0, 0.0, 3.0, 0.0, 0.0, 0.0]);
        // Entirely outside: all fill.
        let away = file(vec![part((-5, 0), (2, 2), vec![AnyChannel::new("G", FlatSamples::F32(vec![1.0; 4]))])], display);
        assert_eq!(read_exr_channels(&away).unwrap().get("G").unwrap(), [0.0; 6]);
    }

    /// #474: concurrent requests for a file that isn't cached decode it once; the others wait
    /// for that decode and share its planes.
    #[test]
    fn concurrent_requests_decode_a_file_once() {
        let bytes: Arc<[u8]> = flat(&["R", "G", "B", "Z"], (64, 64), 0.5).into();
        let cache = ChannelCache::new(CACHE_BUDGET);
        let reads = AtomicU64::new(0);
        let barrier = std::sync::Barrier::new(8);
        let got: Vec<Arc<Decoded>> = std::thread::scope(|s| {
            let jobs: Vec<_> = (0..8)
                .map(|_| {
                    s.spawn(|| {
                        barrier.wait();
                        cache
                            .get("/474-concurrent.exr", Use::Channels, || {
                                reads.fetch_add(1, Ordering::Relaxed);
                                // Long enough for the others to arrive while this one decodes.
                                std::thread::sleep(std::time::Duration::from_millis(50));
                                Some(bytes.clone())
                            })
                            .unwrap()
                    })
                })
                .collect();
            jobs.into_iter().map(|j| j.join().unwrap()).collect()
        });
        assert_eq!(cache.decodes(), 1, "one decode");
        assert_eq!(reads.load(Ordering::Relaxed), 1, "the file is read once");
        assert!(got.iter().all(|d| Arc::ptr_eq(d, &got[0])), "every caller has the same planes");
        assert_eq!(got[0].aux.get("Z").and_then(|z| z.first()).copied(), Some(0.5));
    }

    /// #474: the cache is bounded by the bytes of its planes, least recently used out first; the
    /// newest file stays even when it alone is over the budget, and unreadable files are cached
    /// (as such) without counting bytes.
    #[test]
    fn cache_evicts_least_recently_used_by_bytes() {
        // 4 float planes of 32×32: 16 KiB of samples (plus a few bytes of names) per file.
        let bytes: Arc<[u8]> = flat(&["R", "G", "B", "A"], (32, 32), 0.25).into();
        let one = 4 * 32 * 32 * 4 + 4;
        let cache = ChannelCache::new(2 * one);
        let get = |p: &str| cache.get(p, Use::Channels, || Some(bytes.clone())).unwrap();
        assert_eq!(get("/a.exr").bytes(), one);
        get("/b.exr");
        get("/a.exr"); // a is now more recent than b
        get("/c.exr");
        assert_eq!(cache.cached(), ["/a.exr", "/c.exr"], "b was least recently used");
        assert_eq!(cache.decodes(), 3);
        get("/a.exr");
        assert_eq!(cache.decodes(), 3, "a hit");
        // A file over the budget on its own is still kept while it is the newest.
        let small = ChannelCache::new(one / 2);
        assert!(small.get("/big.exr", Use::Channels, || Some(bytes.clone())).is_some());
        assert_eq!(small.cached(), ["/big.exr"]);
        small.get("/big2.exr", Use::Channels, || Some(bytes.clone()));
        assert_eq!(small.cached(), ["/big2.exr"]);
        // An unreadable file is remembered (not read again) without bytes, and forgotten on request.
        assert!(cache.get("/broken.exr", Use::Channels, || Some(Arc::from(&b"not an exr"[..]))).is_none());
        assert!(cache.get("/broken.exr", Use::Channels, || panic!("not read again")).is_none());
        cache.forget("/broken.exr");
        assert!(!cache.cached().contains(&"/broken.exr".to_string()));
        // At most CACHE_FILES files, however small.
        for i in 0..CACHE_FILES + 10 {
            cache.get(&format!("/none{i}.exr"), Use::Channels, || None);
        }
        assert!(cache.cached().len() <= CACHE_FILES);
    }

    /// #474: a file counts as used when its decode finishes: one asked for first but decoded
    /// after another is the newer of the two, so concurrent frames don't evict each other's
    /// files before their effects read them.
    #[test]
    fn a_file_is_used_as_of_its_decode() {
        let bytes: Arc<[u8]> = flat(&["R", "G", "B", "A"], (32, 32), 0.25).into();
        let one = 4 * 32 * 32 * 4 + 4;
        let cache = ChannelCache::new(2 * one);
        cache.get("/first.exr", Use::Channels, || {
            // Another frame's file is asked for and decoded while this one decodes.
            cache.get("/second.exr", Use::Channels, || Some(bytes.clone()));
            Some(bytes.clone())
        });
        cache.get("/third.exr", Use::Channels, || Some(bytes.clone()));
        assert_eq!(cache.cached(), ["/first.exr", "/third.exr"], "the file decoded first went first");
    }

    /// #474: frames rendered at once (each asks for its picture, then its effects for the
    /// channels while other frames decode) decode each file once, though the budget holds only
    /// one: a picture's file is kept for its effects, and files in use are never dropped.
    #[test]
    fn concurrent_frames_decode_each_file_once() {
        let bytes: Arc<[u8]> = flat(&["R", "G", "B", "Z"], (64, 64), 0.5).into();
        let cache = ChannelCache::new(4 * 64 * 64 * 4 + 4);
        let (next, reads, frames) = (std::sync::atomic::AtomicUsize::new(0), AtomicU64::new(0), 24);
        let read = || {
            reads.fetch_add(1, Ordering::Relaxed);
            Some(bytes.clone())
        };
        std::thread::scope(|s| {
            for _ in 0..4 {
                s.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= frames {
                            break;
                        }
                        let path = format!("/474-frames/{i:04}.exr");
                        drop(cache.get(&path, Use::Picture, read).unwrap());
                        std::thread::sleep(std::time::Duration::from_millis(3));
                        let aux = cache.get(&path, Use::Channels, read).unwrap();
                        std::thread::sleep(std::time::Duration::from_millis(2));
                        assert_eq!(aux.aux.get("Z").and_then(|z| z.first()).copied(), Some(0.5));
                    }
                });
            }
        });
        assert_eq!(cache.decodes(), frames as u64, "one decode per frame");
        assert_eq!(reads.load(Ordering::Relaxed), frames as u64);
    }

    /// #474: a decode that panics doesn't leave its file "decoding" in the cache (never dropped,
    /// its planes counted): the entry goes, and the next request decodes the file.
    #[test]
    fn a_decode_that_panics_is_not_left_in_the_cache() {
        let bytes: Arc<[u8]> = flat(&["R"], (8, 8), 1.0).into();
        let cache = ChannelCache::new(CACHE_BUDGET);
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cache.get("/474-panics.exr", Use::Channels, || panic!("the read failed"))));
        assert!(r.is_err());
        assert!(cache.cached().is_empty());
        assert!(cache.get("/474-panics.exr", Use::Channels, || Some(bytes.clone())).is_some());
        assert_eq!(cache.cached(), ["/474-panics.exr"]);
    }

    /// #474: the picture of a multi-layer file whose effects read its channels comes from the
    /// cache, and from the picture's own channels when the cache has none for the file (a read
    /// that failed, or a broken part the picture doesn't use).
    #[test]
    fn a_shared_picture_without_cached_channels_is_still_decoded() {
        use effectcraft_project::Footage;
        use effectcraft_render::FloatCurve;
        let bytes: Arc<[u8]> = flat(&["Image.R", "Image.G", "Image.B", "depth.Z"], (4, 2), 0.5).into();
        let path = "/474-shared-fallback.exr";
        assert!(cache().get(path, Use::Channels, || None).is_none(), "a read that failed");
        let op = AlphaOp::of(&Footage::default(), FloatCurve::Linear);
        let img = layered_picture(path, bytes, &ReadFile::new(path, effectcraft_render::reads::FileStamp::Virtual), op, true).unwrap();
        assert_eq!(img.get(1, 1), [0.5, 0.5, 0.5, 1.0]);
    }

    /// #474: a header asking for more plane memory than one decode may allocate (16384² display
    /// window, 40 channels: 43 GB) is refused with a reason before anything is allocated.
    #[test]
    fn a_header_asking_for_too_much_memory_is_refused() {
        use exr::prelude::*;
        let list: Vec<AnyChannel<FlatSamples>> = (0..40).map(|i| AnyChannel::new(format!("pass{i:02}.Y").as_str(), FlatSamples::F32(vec![0.5]))).collect();
        let layer = Layer::new((1, 1), LayerAttributes::default(), Encoding::FAST_LOSSLESS, AnyChannels::sort(list.into()));
        let bytes = file(vec![layer], IntegerBounds::new((0, 0), (16384, 16384)));
        match decode(&bytes, &|_| true) {
            Err(Refused::TooLarge(why)) => assert!(why.contains("16384×16384") && why.contains("GB"), "{why}"),
            Err(Refused::Unreadable) => panic!("readable"),
            Ok(_) => panic!("refused"),
        }
        assert!(read_exr_channels(&bytes).is_none());
        assert!(cached("/474-too-large.exr", || Some(bytes.as_slice().into())).is_none());
    }

    /// #474: a file forgotten (changed on disk) while it decodes isn't cached under its path when
    /// that decode finishes.
    #[test]
    fn a_file_forgotten_while_it_decodes_is_not_cached() {
        let bytes: Arc<[u8]> = flat(&["R"], (8, 8), 1.0).into();
        let cache = ChannelCache::new(CACHE_BUDGET);
        let d = cache.get("/changed.exr", Use::Channels, || {
            cache.forget("/changed.exr");
            Some(bytes.clone())
        });
        assert!(d.is_some());
        assert!(cache.cached().is_empty());
    }

    /// #295: a multi-layer EXR whose colour is only in named layers (`diffuse.R`, as Blender and
    /// Nuke write them) failed to import ("does not contain non-deep rgb channels"); it now
    /// shows its colour layer and keeps every channel for EXtractoR.
    /// #474: once an effect has read a footage item's channels, the picture of each later frame
    /// shares one decode with them: the frame's decode is cached and the channels come from it
    /// without reading the file again.
    #[test]
    fn picture_and_aux_channels_share_one_decode() {
        use exr::prelude::*;
        let (w, h) = (2usize, 2usize);
        let frame = |v: f32| -> Arc<[u8]> {
            let ch = |n: &str, v: f32| AnyChannel::new(n, FlatSamples::F32(vec![v; w * h]));
            let channels = AnyChannels::sort(vec![ch("Image.R", 0.5), ch("Image.G", 0.5), ch("Image.B", 0.5), ch("GlossDir.R", v)].into());
            let image = Image::from_layer(Layer::new((w, h), LayerAttributes::default(), Encoding::FAST_LOSSLESS, channels));
            let mut bytes = std::io::Cursor::new(Vec::new());
            image.write().to_buffered(&mut bytes).unwrap();
            bytes.into_inner().into()
        };
        let paths = ["/474-shared-decode_0001.exr", "/474-shared-decode_0002.exr"];
        let pool = crate::MediaPool::new();
        pool.add_bytes(paths[0], frame(0.25));
        pool.add_bytes(paths[1], frame(0.75));
        let mut f = crate::probe_bytes(paths[0], frame(0.25)).unwrap();
        f.kind = effectcraft_project::FootageKind::Sequence;
        f.sequence = paths.iter().map(|p| p.to_string()).collect();
        let t1 = f.frame_rate.tick_of(1);
        use effectcraft_render::FootageSource;
        // The first frame: the picture, then an effect reads the channels.
        pool.frame_at(&f, effectcraft_time::Tick::ZERO).unwrap();
        let aux = pool.aux(effectcraft_project::ItemId(1), &f, effectcraft_time::Tick::ZERO).unwrap();
        assert_eq!(aux.get("GlossDir.R").and_then(|p| p.first()).copied(), Some(0.25));
        // The next frame's picture decodes the file into the cache, so its channels are not read.
        let px = pool.frame_at(&f, t1).unwrap().get(0, 0);
        let aux = cached(paths[1], || panic!("the frame's decode is reused, the file is not read again")).unwrap();
        assert_eq!(aux.get("GlossDir.R").and_then(|p| p.first()).copied(), Some(0.75));
        assert_eq!(px, pool.frame_at(&f, effectcraft_time::Tick::ZERO).unwrap().get(0, 0), "the same picture");
    }

    /// An EXR file with an unnamed RGB layer and a named `GlossDir.R` of `v` (2×2).
    fn rgb_and_gloss(v: f32) -> Arc<[u8]> {
        flat(&["R", "G", "B", "GlossDir.R"], (2, 2), v).into()
    }

    /// #474: footage the project says effects read the channels of
    /// ([`FootageSource::set_aux_footage`]) costs one decode from its first frame on: a single
    /// still with EXtractoR decodes once. Without it the first frame decodes twice (the picture,
    /// then the channels once `aux()` asks).
    #[test]
    fn given_aux_footage_decodes_once_from_the_first_frame() {
        use effectcraft_render::FootageSource;
        let (given, learned) = ("/474-given-still.exr", "/474-learned-still.exr");
        let pool = crate::MediaPool::new();
        let mut f = std::collections::HashMap::new();
        for p in [given, learned] {
            pool.add_bytes(p, rgb_and_gloss(0.25));
            f.insert(p, crate::probe_bytes(p, rgb_and_gloss(0.25)).unwrap());
        }
        pool.set_aux_footage(&[given.to_string()]);
        take_decodes();
        let t = effectcraft_time::Tick::ZERO;
        pool.frame_at(&f[given], t).unwrap();
        let aux = pool.aux(effectcraft_project::ItemId(1), &f[given], t).unwrap();
        assert_eq!(aux.get("GlossDir.R").and_then(|p| p.first()).copied(), Some(0.25));
        assert_eq!(take_decodes(), [0, 1], "the picture and the channels share one decode");
        // Learned only at the first `aux()`: the picture was decoded on its own before.
        pool.frame_at(&f[learned], t).unwrap();
        pool.aux(effectcraft_project::ItemId(2), &f[learned], t).unwrap();
        assert_eq!(take_decodes(), [1, 1]);
    }

    /// #474: once the project no longer names footage (its effect removed), later frames decode
    /// only their picture again; what `aux()` learned is dropped with each new set too.
    #[test]
    fn a_new_aux_footage_set_returns_to_picture_only() {
        use effectcraft_render::FootageSource;
        let paths: Vec<String> = (1..=4).map(|i| format!("/474-unset_{i:04}.exr")).collect();
        let pool = crate::MediaPool::new();
        for p in &paths {
            pool.add_bytes(p, rgb_and_gloss(0.5));
        }
        let mut f = crate::probe_bytes(&paths[0], rgb_and_gloss(0.5)).unwrap();
        f.kind = effectcraft_project::FootageKind::Sequence;
        f.sequence = paths.clone();
        let at = |i: i64| f.frame_rate.tick_of(i);
        // (counted on this thread, so other tests sharing the channel cache don't change them)
        let frame = |i: i64| {
            take_decodes();
            pool.frame_at(&f, at(i)).unwrap();
            take_decodes()
        };
        pool.set_aux_footage(std::slice::from_ref(&f.path));
        assert_eq!(frame(0), [0, 1], "given: the picture comes from the channel decode");
        pool.set_aux_footage(&[]);
        assert!(!cache().lock().held.iter().any(|(p, _)| *p == paths[0]), "no longer kept once no effect reads it");
        assert_eq!(frame(1), [1, 0], "no longer given: the picture alone");
        // Learned by `aux()`, then a new set without it: picture only again.
        pool.aux(effectcraft_project::ItemId(1), &f, at(1)).unwrap();
        assert_eq!(frame(2), [0, 1], "learned");
        pool.set_aux_footage(&[]);
        assert_eq!(frame(3), [1, 0], "a new set drops what was learned");
    }

    /// #474: the picture made of the cached planes of a file with exact R, G, B channels is the
    /// `image` crate's, bit for bit (first such part, alpha or opaque, data window placed with 0
    /// around it), in every alpha interpretation and float curve.
    #[test]
    fn picture_from_planes_is_the_image_crates() {
        use effectcraft_project::{AlphaMode, Footage};
        use effectcraft_render::FloatCurve;
        use exr::prelude::*;
        let half = |n: &str, v: f32, len: usize| AnyChannel::new(n, FlatSamples::F16((0..len).map(|i| f16::from_f32(v + i as f32 * 0.37 - 1.0)).collect()));
        let float = |n: &str, v: f32, len: usize| AnyChannel::new(n, FlatSamples::F32((0..len).map(|i| ((i * 7) % 11) as f32 * v).collect()));
        let part = |name: Option<&str>, pos: (i32, i32), size: (usize, usize), list: Vec<AnyChannel<FlatSamples>>| {
            let mut attrs = name.map_or_else(LayerAttributes::default, LayerAttributes::named);
            attrs.layer_position = Vec2(pos.0, pos.1);
            Layer::new(size, attrs, Encoding::SMALL_LOSSLESS, AnyChannels::sort(list.into()))
        };
        let display = IntegerBounds::new((1, -1), (5, 4));
        let n = 20;
        let files = [
            file(
                vec![part(None, (1, -1), (5, 4), vec![half("R", 0.1, n), half("G", 0.2, n), half("B", 0.3, n), float("A", 0.1, n), float("Z", 2.0, n)])],
                display,
            ),
            file(vec![part(None, (0, 0), (3, 2), vec![float("R", 0.4, 6), float("G", 0.5, 6), float("B", 0.6, 6), half("Image.R", 0.1, 6)])], display),
            file(
                vec![
                    part(Some("diffuse"), (1, -1), (5, 4), vec![half("R", 0.1, n), half("G", 0.2, n), half("B", 0.3, n)]),
                    part(Some("beauty"), (2, 0), (2, 2), vec![half("R", 0.4, 4), half("G", 0.5, 4), half("B", 0.6, 4), half("A", 0.5, 4)]),
                ],
                display,
            ),
        ];
        let bits = |i: &effectcraft_raster::Image| i.data.iter().flat_map(|p| p.map(f32::to_bits)).collect::<Vec<_>>();
        for (k, bytes) in files.iter().enumerate() {
            let path = format!("/474-planes-{k}.exr");
            let d = cache().get(&path, Use::Channels, || Some(bytes.as_slice().into())).unwrap();
            assert!(matches!(d.rgb, Rgb::Planes(..)), "file {k}: {:?}", d.rgb);
            let img = crate::probe::load_oriented(bytes, Some(image::ImageFormat::OpenExr)).unwrap();
            for alpha in [AlphaMode::Straight, AlphaMode::Premultiplied, AlphaMode::Ignore] {
                for curve in [FloatCurve::Srgb, FloatCurve::Linear, FloatCurve::Gamma24] {
                    let op = AlphaOp::of(&Footage { alpha, premul_color: [0.2, 0.4, 0.6], ..Default::default() }, curve);
                    let want = crate::convert::dynamic_to_image(&img, op);
                    let got = cached_picture(&path, bytes.as_slice().into(), &ReadFile::new(path.as_str(), effectcraft_render::reads::FileStamp::Virtual), op)
                        .unwrap();
                    assert_eq!((got.width, got.height), (want.width, want.height));
                    assert_eq!(bits(&got), bits(&want), "file {k} {alpha:?} {curve:?}");
                }
            }
        }
    }

    /// #2141: one pixel of data in a 60000² display window is refused, not allocated per channel.
    #[test]
    fn a_huge_display_window_is_refused_not_allocated() {
        use exr::prelude::*;
        let channels = AnyChannels::sort(vec![AnyChannel::new("R", FlatSamples::F32(vec![0.5]))].into());
        let mut image = Image::from_layer(Layer::new((1, 1), LayerAttributes::default(), Encoding::FAST_LOSSLESS, channels));
        image.attributes.display_window = IntegerBounds::new((0, 0), (60000, 60000));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write().to_buffered(&mut bytes).unwrap();
        assert!(read_exr_channels(&bytes.into_inner()).is_none());
    }

    #[test]
    fn layered_exr_without_an_unnamed_rgb_layer_imports() {
        use exr::prelude::*;
        let (w, h) = (4usize, 2usize);
        let ch = |n: &str, v: f32| AnyChannel::new(n, FlatSamples::F32(vec![v; w * h]));
        let channels = AnyChannels::sort(vec![ch("diffuse.R", 1.0), ch("diffuse.G", 0.2), ch("diffuse.B", 0.0), ch("depth.Z", 7.0)].into());
        let image = Image::from_layer(Layer::new((w, h), LayerAttributes::default(), Encoding::FAST_LOSSLESS, channels));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write().to_buffered(&mut bytes).unwrap();
        let b: Arc<[u8]> = bytes.into_inner().into();
        let f = crate::probe_bytes("/layers.exr", b.clone()).unwrap();
        assert_eq!((f.width, f.height, f.alpha), (4, 2, effectcraft_project::AlphaMode::Ignore));
        let pool = crate::MediaPool::new();
        pool.add_bytes("/layers.exr", b.clone());
        let px = pool.frame_at(&f, effectcraft_time::Tick::ZERO).unwrap().get(1, 1);
        assert!((px[0] - 1.0).abs() < 1e-4 && px[1] > 0.3 && px[1] < 0.6 && px[2] < 1e-4 && px[3] == 1.0, "diffuse, sRGB-encoded: {px:?}");
        let aux = read_exr_channels(&b).unwrap();
        assert_eq!(aux.layers(), ["depth", "diffuse"]);
        assert_eq!(aux.layer_rgba("diffuse"), ["diffuse.R", "diffuse.G", "diffuse.B", ""].map(String::from));
    }

    /// #481: Blender 5 writes one part per pass, its channels keeping the full pass name
    /// (`ViewLayer.Depth.Z` in part `ViewLayer.Depth`); the name was doubled and the depth and
    /// Cryptomatte ranks were not found.
    #[test]
    fn part_name_is_not_doubled_when_channels_carry_it() {
        assert_eq!(channel_name(Some("ViewLayer.Depth"), "ViewLayer.Depth.Z"), "ViewLayer.Depth.Z");
        assert_eq!(channel_name(Some("depth"), "Z"), "depth.Z");
        assert_eq!(channel_name(Some("depth"), "depthZ"), "depth.depthZ");
        assert_eq!(channel_name(None, "R"), "R");
        assert_eq!(channel_name(None, "ViewLayer.Depth.Z"), "ViewLayer.Depth.Z");
    }

    #[test]
    fn blender5_multipart_file_keeps_pass_names() {
        use exr::prelude::*;
        let (w, h) = (2usize, 2usize);
        let part = |name: &str, chans: &[&str], v: f32| {
            let list: Vec<AnyChannel<FlatSamples>> = chans.iter().map(|c| AnyChannel::new(*c, FlatSamples::F32(vec![v; w * h]))).collect();
            Layer::new((w, h), LayerAttributes::named(name), Encoding::FAST_LOSSLESS, AnyChannels::sort(list.into()))
        };
        let layers =
            vec![part("ViewLayer.Depth", &["ViewLayer.Depth.Z"], 4.0), part("ViewLayer.Combined", &["ViewLayer.Combined.R", "ViewLayer.Combined.G"], 0.5)];
        let image = Image::from_layers(ImageAttributes::with_size((w, h)), layers);
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write().to_buffered(&mut bytes).unwrap();
        let aux = read_exr_channels(bytes.get_ref()).unwrap();
        assert_eq!(aux.get("ViewLayer.Depth.Z").unwrap(), [4.0; 4].as_slice());
        assert!(aux.get("ViewLayer.Depth.ViewLayer.Depth.Z").is_none());
        assert!(aux.get("ViewLayer.Combined.R").is_some());
    }

    /// Straight RGBA of [`layered_image`] at pixel 0 for a file of single-value channels.
    fn picture<S: AsRef<str>>(channels: &[(S, f32)]) -> Vec<f32> {
        use exr::prelude::*;
        let (w, h) = (2usize, 2usize);
        let list: Vec<AnyChannel<FlatSamples>> = channels.iter().map(|(n, v)| AnyChannel::new(n.as_ref(), FlatSamples::F32(vec![*v; w * h]))).collect();
        let image = Image::from_layer(Layer::new((w, h), LayerAttributes::default(), Encoding::FAST_LOSSLESS, AnyChannels::sort(list.into())));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write().to_buffered(&mut bytes).unwrap();
        let img = layered_image(bytes.get_ref()).unwrap().to_rgba32f();
        img.get_pixel(0, 0).0.to_vec()
    }

    /// #412: a Blender compositor File Output (multilayer) names its beauty pass `Image`, and its
    /// Cryptomatte layers (lowercase `r, g, b, a`) sort first. The picture was CryptoMaterial00;
    /// it is the Image layer, and data passes are never the picture.
    #[test]
    fn layered_exr_shows_the_beauty_pass_not_cryptomatte() {
        let mut blender = vec![];
        for layer in ["CryptoMaterial00", "CryptoMaterial01", "CryptoMaterial02"] {
            blender.extend(["r", "g", "b", "a"].map(|c| (format!("{layer}.{c}"), 0.9)));
        }
        for (layer, v) in [("GlossCol", 0.3), ("GlossDir", 0.4), ("Image", 0.5), ("VolDir", 0.6)] {
            blender.extend(["R", "G", "B", "A"].map(|c| (format!("{layer}.{c}"), v)));
        }
        blender.push(("Mist.Z".into(), 0.7));
        assert_eq!(picture(&blender), [0.5; 4], "the Image layer");
        // Blender's render-layer names and Nuke's beauty.
        assert_eq!(
            picture(&[
                ("ViewLayer.CryptoObject00.r", 0.9),
                ("ViewLayer.CryptoObject00.g", 0.9),
                ("ViewLayer.CryptoObject00.b", 0.9),
                ("ViewLayer.Combined.R", 0.2),
                ("ViewLayer.Combined.G", 0.2),
                ("ViewLayer.Combined.B", 0.2)
            ]),
            [0.2, 0.2, 0.2, 1.0]
        );
        // No beauty name: the first colour layer that isn't a data pass.
        let no_beauty: Vec<(String, f32)> = blender.iter().filter(|(n, _)| !n.starts_with("Image")).cloned().collect();
        assert_eq!(picture(&no_beauty), [0.3; 4], "GlossCol, not Cryptomatte");
        // Only data and a grey pass: the grey pass, not depth.
        assert_eq!(picture(&[("Depth.Z", 9.0), ("AO.Y", 0.25)]), [0.25, 0.25, 0.25, 1.0]);
    }
}
