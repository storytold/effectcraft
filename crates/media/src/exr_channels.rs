//! Auxiliary channels from multi-layer OpenEXR files (depth, object / material IDs, normals,
//! Cryptomatte, …) for the 3D Channel effects, read with the pure-Rust `exr` crate.
//!
//! Every channel of every layer becomes a plane named `layer.channel` (or just `channel` for
//! the unnamed layer), placed in the display window (pixels outside a layer's data window are
//! 0, or the background depth for depth channels). Cryptomatte manifests come from the
//! `cryptomatte/<key>/name` and `cryptomatte/<key>/manifest` header attributes (the published
//! Cryptomatte metadata convention: the manifest is a JSON object of name → hex hash).
//!
//! The files are indexed from their headers ([`crate::exr_index`]), read chunk by chunk
//! ([`crate::exr_read`]) and their planes kept in the plane store ([`crate::exr_planes`]),
//! which the picture of an EXR footage item shares with EXtractoR / Cryptomatte (#474, #3271).
//! This module holds the naming and picture rules they share, and whole-file reads of bytes.

use effectcraft_raster::channels3d::split_channel;
use effectcraft_raster::{AuxChannels, Image};
use effectcraft_render::reads::FileStamp;
use exr::meta::header::Header;

use crate::convert::{AlphaOp, float_to_image};
use crate::exr_index::ExrIndex;
use crate::exr_read::{ReadRequest, read_planes};

/// The largest display window decoded (16384²; vector footage and Continuously Rasterize
/// stop at the same size).
pub(crate) const MAX_DISPLAY_PIXELS: usize = 1 << 28;

pub(crate) fn is_depth(name: &str) -> bool {
    let last = name.rsplit('.').next().unwrap_or(name);
    last.eq_ignore_ascii_case("z") || name.to_ascii_lowercase().contains("depth")
}

/// Full plane name of a channel in a part: `part.channel`, except that Blender 5 writes one part
/// per pass whose channels already carry the full name (part `ViewLayer.Depth` holds
/// `ViewLayer.Depth.Z`), which is kept as is instead of doubling the prefix.
pub(crate) fn channel_name(part: Option<&str>, channel: &str) -> String {
    match part {
        Some(p) if channel.strip_prefix(p).is_some_and(|rest| rest.starts_with('.')) => channel.to_string(),
        Some(p) => format!("{p}.{channel}"),
        None => channel.to_string(),
    }
}

/// The part name channels are prefixed with (`None`: unnamed).
pub(crate) fn part_name(header: &Header) -> Option<String> {
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
    let meta = exr::meta::MetaData::read_from_buffered(std::io::Cursor::new(bytes), false).ok()?;
    crate::exr_index::unsupported(&meta)
}

/// How the `image` crate's OpenEXR decoder would show a file: the first part with exact `R`, `G`
/// and `B` channels (alpha `A` when it has one, else opaque), placed in the display window with
/// 0 outside the data window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Rgb {
    /// No part has R, G and B: the decoder refuses the file (a multi-layer file, shown with
    /// [`layered_image`]'s rule).
    None,
    /// The channels (indices into [`ExrIndex::channels`]) hold exactly what it would show.
    Planes([usize; 3], Option<usize>),
    /// The planes can't stand in for it (a depth-named channel, whose plane is filled with the
    /// background depth, or another display window): decode with the `image` crate.
    Unsupported,
}

/// How many bytes of decoded planes the plane store keeps by itself until the caches' budget is
/// set (then it follows the footage frames' cap, see [`crate::exr_planes::set_budget`]: the whole
/// common budget on the desktop, #3298), like the frame cache's default
/// ([`crate::DEFAULT_BUDGET`]): two 2560×1440 files of 32 channels (470 MB each, as every plane
/// is f32). A soft bound: the newest file is kept whatever its size (so a frame's picture and its
/// effects share its decode), and files still in use are never dropped (planes an effect holds,
/// pictures waiting for their effects, decodes in progress), so memory can go above it while
/// they are.
pub const CACHE_BUDGET: usize = if cfg!(target_arch = "wasm32") { WEB_CACHE_CAP } else { crate::DEFAULT_BUDGET };

/// The most decoded planes the plane store keeps in the browser: the browser's largest footage
/// budget (Footage Frame Cache, frames and planes together, `WEB_MEDIA_CACHE` in the engine).
pub const WEB_CACHE_CAP: usize = 256 << 20;

/// The index of an EXR file's bytes and the planes of `ids` (all when `None`). `None` when it is
/// not a readable EXR or the planes need too much memory.
fn read_bytes(bytes: &[u8], ids: Option<Vec<usize>>) -> Option<(ExrIndex, Vec<usize>, Vec<Vec<f32>>)> {
    let mut src = std::io::Cursor::new(bytes);
    let index = crate::exr_index::build(&mut src, bytes.len() as u64, FileStamp::Virtual).ok()?;
    let ids = ids.unwrap_or_else(|| (0..index.channels.len()).collect());
    let (planes, _) = read_planes(&mut src, &index, &ReadRequest::full(ids.clone()));
    Some((index, ids, planes.ok()?))
}

/// The channels `ids` of `index` with their `planes` (as [`read_bytes`] reads them).
fn aux_of(index: &ExrIndex, ids: &[usize], planes: Vec<Vec<f32>>) -> AuxChannels {
    let (w, h) = index.size();
    let mut aux = AuxChannels::new(w as u32, h as u32, 1.0);
    aux.channels = ids.iter().zip(planes).filter_map(|(id, p)| Some((index.channels.get(*id)?.name.clone(), p))).collect();
    aux.manifests = index.names.manifests.clone();
    aux.aliases = index.names.aliases.clone();
    aux
}

/// Read all channels of an EXR file's bytes. `None` when it is not a readable EXR.
pub fn read_exr_channels(bytes: &[u8]) -> Option<AuxChannels> {
    let (index, ids, planes) = read_bytes(bytes, None)?;
    Some(aux_of(&index, &ids, planes))
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
pub(crate) fn picture_names(aux: &AuxChannels) -> Option<([String; 3], Option<String>)> {
    let layers = aux.layers();
    let colour = picture_layer(aux, &layers)?;
    let names = aux.layer_rgba(colour);
    // A layer without R, G and B channels shows its first channel as grey.
    let first = aux.names().into_iter().find(|n| split_channel(n).0 == colour).unwrap_or_default();
    let pick = |k: usize| names.get(k).map(String::as_str).filter(|n| !n.is_empty()).unwrap_or(first).to_string();
    let a = names.get(3).filter(|n| !n.is_empty() && aux.get(n).is_some()).cloned();
    Some(([pick(0), pick(1), pick(2)], a))
}

/// The picture of a multi-layer OpenEXR file without an unnamed RGB layer (Blender's
/// `ViewLayer.Combined.R`, the compositor's `Image.R`, Nuke's `beauty.R`, …), which the image
/// decoder rejects. After Effects shows a file's main RGBA channels and leaves the other layers
/// to EXtractoR; here that is the beauty pass by name (`BEAUTY`), else the first colour layer
/// that is not a data pass (never Cryptomatte, depth or normals, #412), else the first channel
/// as grey. Linear, straight RGBA as the file stores it. `None` when it is not a readable EXR.
/// Only the picture's channels are decoded (in a multi-part file the other parts aren't read).
pub fn layered_image(bytes: &[u8]) -> Option<image::DynamicImage> {
    let mut src = std::io::Cursor::new(bytes);
    let index = crate::exr_index::build(&mut src, bytes.len() as u64, FileStamp::Virtual).ok()?;
    let (ids, rgb, a) = index.picture.clone()?;
    let (planes, _) = read_planes(&mut src, &index, &ReadRequest::full(ids.clone()));
    let aux = aux_of(&index, &ids, planes.ok()?);
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

/// The size and alpha of a layered file's picture ([`layered_image`]) from its headers alone
/// (import). `None` when it is not a readable EXR.
pub(crate) fn layered_size(bytes: &[u8]) -> Option<(u32, u32, bool)> {
    let mut src = std::io::Cursor::new(bytes);
    let index = crate::exr_index::build(&mut src, bytes.len() as u64, FileStamp::Virtual).ok()?;
    let (_, _, a) = index.picture.as_ref()?;
    let (w, h) = index.size();
    Some((w as u32, h as u32, a.is_some()))
}

/// The picture made of planes `rgb` and `a` (opaque without one) of `aux`, as
/// [`crate::convert::dynamic_to_image`] makes it of the same values. `None` when a plane is
/// missing or short.
pub(crate) fn planes_picture(aux: &AuxChannels, rgb: [&[f32]; 3], a: Option<&[f32]>, op: AlphaOp) -> Option<Image> {
    let n = (aux.width as usize).checked_mul(aux.height as usize)?;
    if rgb.iter().chain(&a).any(|p| p.len() < n) {
        return None;
    }
    let [r, g, b] = rgb;
    let at = |p: &[f32], i: usize| p.get(i).copied().unwrap_or(0.0);
    Some(float_to_image(aux.width, aux.height, op, |i| [at(r, i), at(g, i), at(b, i), a.map_or(1.0, |a| at(a, i))]))
}

#[cfg(test)]
thread_local! {
    /// Decodes of a still's pixels on this thread ([`count_decode`]): the `image` crate's, then
    /// the plane store's.
    static DECODES: std::cell::Cell<[u64; 2]> = const { std::cell::Cell::new([0; 2]) };
}

/// Count a decode of a file's pixels on this thread (tests): by the plane store (`channels`)
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
/// and by the plane store.
#[cfg(test)]
pub(crate) fn take_decodes() -> [u64; 2] {
    DECODES.with(|d| d.replace([0; 2]))
}

/// Run `f` and return the decodes [`count_decode`] counted on this thread meanwhile, leaving
/// this thread's count as it was (a decode thread may run another request's decode inside
/// this one's).
#[cfg(test)]
pub(crate) fn counting<R>(f: impl FnOnce() -> R) -> (R, [u64; 2]) {
    let before = DECODES.with(std::cell::Cell::get);
    let r = f();
    let after = DECODES.with(|d| d.replace(before));
    (r, [after[0].saturating_sub(before[0]), after[1].saturating_sub(before[1])])
}

/// Count `n` decodes ([`counting`]) on this thread: made for it on another.
#[cfg(test)]
pub(crate) fn add_decodes(n: [u64; 2]) {
    DECODES.with(|d| {
        let c = d.get();
        d.set([c[0] + n[0], c[1] + n[1]]);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use effectcraft_raster::channels3d::BACKGROUND_DEPTH;
    use std::sync::Arc;

    /// An EXR file of `layers` (with their display window `display`).
    fn file(layers: Vec<exr::prelude::Layer<exr::prelude::AnyChannels<exr::prelude::FlatSamples>>>, display: exr::prelude::IntegerBounds) -> Vec<u8> {
        use exr::prelude::*;
        let image = Image::from_layers(ImageAttributes::new(display), layers);
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write().to_buffered(&mut bytes).unwrap();
        bytes.into_inner()
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

    /// #474: a header asking for more plane memory than one decode may allocate (16384² display
    /// window, 40 channels: 43 GB) is refused with a reason before anything is allocated.
    #[test]
    fn a_header_asking_for_too_much_memory_is_refused() {
        use exr::prelude::*;
        let list: Vec<AnyChannel<FlatSamples>> = (0..40).map(|i| AnyChannel::new(format!("pass{i:02}.Y").as_str(), FlatSamples::F32(vec![0.5]))).collect();
        let layer = Layer::new((1, 1), LayerAttributes::default(), Encoding::FAST_LOSSLESS, AnyChannels::sort(list.into()));
        let bytes = file(vec![layer], IntegerBounds::new((0, 0), (16384, 16384)));
        let mut src = std::io::Cursor::new(bytes.as_slice());
        let index = crate::exr_index::build(&mut src, bytes.len() as u64, FileStamp::Virtual).unwrap();
        let all = ReadRequest::full((0..index.channels.len()).collect());
        match read_planes(&mut src, &index, &all).0 {
            Err(crate::exr_read::ReadError::TooLarge(why)) => assert!(why.contains("16384×16384") && why.contains("GB"), "{why}"),
            Err(e) => panic!("readable: {e:?}"),
            Ok(_) => panic!("refused"),
        }
        assert!(read_exr_channels(&bytes).is_none());
        let pool = crate::MediaPool::new();
        pool.add_bytes("/474-too-large.exr", bytes.as_slice().into());
        let f = effectcraft_project::Footage {
            path: "/474-too-large.exr".into(),
            kind: effectcraft_project::FootageKind::Still,
            has_video: true,
            width: 16384,
            height: 16384,
            ..Default::default()
        };
        use effectcraft_render::FootageSource;
        assert!(pool.aux(effectcraft_project::ItemId(1), &f, effectcraft_time::Tick::ZERO).is_none());
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
