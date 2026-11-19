//! Auxiliary channels from multi-layer OpenEXR files (depth, object / material IDs, normals,
//! Cryptomatte, …) for the 3D Channel effects, read with `craft-exr`.
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

use craft_exr::decode::PartInfo;
use effectcraft_raster::{AuxChannels, Image};
use effectcraft_render::reads::FileStamp;

use crate::convert::{AlphaOp, float_to_image};
use crate::exr_index::ExrIndex;
use crate::exr_read::{ReadRequest, read_planes};

/// The largest display window decoded (16384²; vector footage and Continuously Rasterize
/// stop at the same size).
pub(crate) const MAX_DISPLAY_PIXELS: usize = 1 << 28;

// Channel naming and the Cryptomatte manifest are `craft-exr`'s.
pub(crate) use craft_exr::channels::{is_depth, part_channel_name as channel_name};
pub use craft_exr::cryptomatte::parse_manifest;

/// The part name channels are prefixed with (`None`: unnamed).
pub(crate) fn part_name(part: &PartInfo) -> Option<String> {
    part.name.clone().filter(|s| !s.is_empty())
}

/// Why an OpenEXR file's pixels can't be decoded when its headers name a compression this build
/// can't decompress (HTJ2K, OpenEXR 3.4's High-Throughput JPEG 2000): the decoders only said
/// "no non-deep rgb channels" or showed a transparent frame (#482). `None` for other files.
pub fn unsupported_compression(bytes: &[u8]) -> Option<&'static str> {
    let parts = craft_exr::decode::parts(bytes, &craft_exr::decode::Limits::none()).ok()?;
    parts.iter().any(|p| p.compression.starts_with("HTJ2K")).then_some(crate::exr_index::HTJ2K)
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
    let index = crate::exr_index::build(&mut src, FileStamp::Virtual).ok()?;
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

/// The main picture of an OpenEXR file's bytes ([`crate::exr_index::MainPicture`]), linear,
/// straight RGB(A) as the file stores it (a `Y` picture as grey); a file without one (named
/// layers only, as Blender's `ViewLayer.Combined.R`) is black at its display window's size, as
/// After Effects shows it (its layers are reached by name, EXtractoR). Only the picture's
/// channels are decoded (in a multi-part file the other parts aren't read). `None` when it is
/// not a readable EXR.
pub fn picture_image(bytes: &[u8]) -> Option<image::DynamicImage> {
    let mut src = std::io::Cursor::new(bytes);
    let index = crate::exr_index::build(&mut src, FileStamp::Virtual).ok()?;
    let (w, h) = index.size();
    let (w, h) = (u32::try_from(w).ok()?, u32::try_from(h).ok()?);
    let n = (w as usize).checked_mul(h as usize)?;
    // (the caller converts it to a float RGBA picture: a still's bound)
    if n as u64 > crate::probe::MAX_STILL_PIXELS {
        return None;
    }
    let Some(picture) = &index.picture else {
        let len = n.checked_mul(3)?;
        let mut data: Vec<f32> = Vec::new();
        data.try_reserve_exact(len).ok()?;
        data.resize(len, 0.0);
        return image::Rgb32FImage::from_raw(w, h, data).map(image::DynamicImage::ImageRgb32F);
    };
    let ([r, g, b], a) = picture.rgba()?;
    let ids = index.picture_ids();
    let (planes, _) = read_planes(&mut src, &index, &ReadRequest::full(ids.clone()));
    let planes = planes.ok()?;
    let plane = |id: usize| ids.iter().position(|i| *i == id).and_then(|at| planes.get(at));
    let (r, g, b) = (plane(r)?, plane(g)?, plane(b)?);
    let a = match a {
        Some(a) => Some(plane(a)?),
        None => None,
    };
    let mut data = Vec::new();
    data.try_reserve_exact(n.checked_mul(if a.is_some() { 4 } else { 3 })?).ok()?;
    // (0 outside the picture part's data window where its planes hold another fill)
    let width = w as usize;
    let shown = |i: usize| picture.inside.is_none_or(|[x0, y0, x1, y1]| (x0..x1).contains(&(i % width.max(1))) && (y0..y1).contains(&(i / width.max(1))));
    for i in 0..n {
        let on = shown(i);
        let pick = |v: &f32| if on { *v } else { 0.0 };
        data.extend([r.get(i)?, g.get(i)?, b.get(i)?].map(pick));
        if let Some(a) = a {
            data.push(pick(a.get(i)?));
        }
    }
    match a {
        Some(_) => image::Rgba32FImage::from_raw(w, h, data).map(image::DynamicImage::ImageRgba32F),
        None => image::Rgb32FImage::from_raw(w, h, data).map(image::DynamicImage::ImageRgb32F),
    }
}

/// The size and alpha of an OpenEXR file's main picture ([`picture_image`]) from its headers
/// alone (import; a file without one is its display window, opaque). `None` when it is not a
/// readable EXR.
pub(crate) fn picture_size(bytes: &[u8]) -> Option<(u32, u32, bool)> {
    let mut src = std::io::Cursor::new(bytes);
    let index = crate::exr_index::build(&mut src, FileStamp::Virtual).ok()?;
    let (w, h) = index.size();
    Some((u32::try_from(w).ok()?, u32::try_from(h).ok()?, index.picture.as_ref().is_some_and(|p| p.has_alpha())))
}

/// The black picture of a file without a main picture, `w` × `h`, opaque (no alpha channel).
pub(crate) fn black_picture(w: u32, h: u32, op: AlphaOp) -> Image {
    float_to_image(w, h, op, |_| [0.0, 0.0, 0.0, 1.0])
}

/// The picture made of planes `rgb` and `a` (opaque without one) of `aux`, as
/// [`crate::convert::dynamic_to_image`] makes it of the same values. `None` when a plane is
/// missing or short.
pub(crate) fn planes_picture(aux: &AuxChannels, rgb: [&[f32]; 3], a: Option<&[f32]>, inside: Option<[usize; 4]>, op: AlphaOp) -> Option<Image> {
    let w = aux.width as usize;
    let n = w.checked_mul(aux.height as usize)?;
    if rgb.iter().chain(&a).any(|p| p.len() < n) {
        return None;
    }
    let [r, g, b] = rgb;
    // (0 outside `inside`, the picture part's data window, where the planes hold another fill)
    let shown = |i: usize| inside.is_none_or(|[x0, y0, x1, y1]| (x0..x1).contains(&(i % w.max(1))) && (y0..y1).contains(&(i / w.max(1))));
    let at = |p: &[f32], i: usize| if shown(i) { p.get(i).copied().unwrap_or(0.0) } else { 0.0 };
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
        // (another test raises the process-wide decode limit far above the request: not now)
        let _only = crate::exr_read::LIMIT_TESTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let list: Vec<AnyChannel<FlatSamples>> = (0..40).map(|i| AnyChannel::new(format!("pass{i:02}.Y").as_str(), FlatSamples::F32(vec![0.5]))).collect();
        let layer = Layer::new((1, 1), LayerAttributes::default(), Encoding::FAST_LOSSLESS, AnyChannels::sort(list.into()));
        let bytes = file(vec![layer], IntegerBounds::new((0, 0), (16384, 16384)));
        let mut src = std::io::Cursor::new(bytes.as_slice());
        let index = crate::exr_index::build(&mut src, FileStamp::Virtual).unwrap();
        // (the fixed limit, whatever the process-wide one)
        let all = ReadRequest { max_bytes: crate::exr_read::MAX_DECODE_BYTES, ..ReadRequest::full((0..index.channels.len()).collect()) };
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

    /// A file of named layers only (Blender's single-part multilayer) imports at its display
    /// window's size and shows black, opaque, as After Effects does; its layers stay readable by
    /// effects (EXtractoR, Cryptomatte) and nothing is an error.
    #[test]
    fn layered_exr_without_an_unnamed_rgb_layer_imports_black() {
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
        let img = pool.frame_at(&f, effectcraft_time::Tick::ZERO).unwrap();
        assert_eq!((img.width, img.height), (4, 2));
        assert_eq!(img.get(1, 1), [0.0, 0.0, 0.0, 1.0], "black, opaque");
        use effectcraft_render::FootageSource;
        let effects = pool.aux(effectcraft_project::ItemId(1), &f, effectcraft_time::Tick::ZERO).expect("the layers, for effects");
        assert_eq!(effects.get("diffuse.R").unwrap(), [1.0; 8].as_slice());
        assert_eq!(effects.get("depth.Z").unwrap(), [7.0; 8].as_slice());
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

    /// Straight RGBA of [`picture_image`] at pixel 0 for a file of single-value channels.
    fn picture<S: AsRef<str>>(channels: &[(S, f32)]) -> Vec<f32> {
        use exr::prelude::*;
        let (w, h) = (2usize, 2usize);
        let list: Vec<AnyChannel<FlatSamples>> = channels.iter().map(|(n, v)| AnyChannel::new(n.as_ref(), FlatSamples::F32(vec![*v; w * h]))).collect();
        let image = Image::from_layer(Layer::new((w, h), LayerAttributes::default(), Encoding::FAST_LOSSLESS, AnyChannels::sort(list.into())));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write().to_buffered(&mut bytes).unwrap();
        let img = picture_image(bytes.get_ref()).unwrap().to_rgba32f();
        assert_eq!(img.dimensions(), (2, 2), "the display window's size");
        img.get_pixel(0, 0).0.to_vec()
    }

    /// The main picture is After Effects': the unprefixed `R`, `G`, `B` (`A`), else `Y` (`A`) as
    /// grey. A file of named layers only (Blender's multilayer `ViewLayer.Combined.R`, a
    /// compositor's `Image.R`, Cryptomatte, depth) has none and shows black, opaque (#412 chose a
    /// beauty pass by name before; its layers are reached by name, EXtractoR).
    #[test]
    fn the_picture_is_the_unprefixed_channels_else_black() {
        let mut blender = vec![];
        for layer in ["CryptoMaterial00", "CryptoMaterial01", "CryptoMaterial02"] {
            blender.extend(["r", "g", "b", "a"].map(|c| (format!("{layer}.{c}"), 0.9)));
        }
        for (layer, v) in [("GlossCol", 0.3), ("GlossDir", 0.4), ("Image", 0.5), ("VolDir", 0.6)] {
            blender.extend(["R", "G", "B", "A"].map(|c| (format!("{layer}.{c}"), v)));
        }
        blender.push(("Mist.Z".into(), 0.7));
        let black = [0.0, 0.0, 0.0, 1.0];
        assert_eq!(picture(&blender), black, "named layers only: no Image or beauty pass by name");
        assert_eq!(
            picture(&[
                ("ViewLayer.CryptoObject00.r", 0.9),
                ("ViewLayer.CryptoObject00.g", 0.9),
                ("ViewLayer.CryptoObject00.b", 0.9),
                ("ViewLayer.Combined.R", 0.2),
                ("ViewLayer.Combined.G", 0.2),
                ("ViewLayer.Combined.B", 0.2)
            ]),
            black,
            "Blender's Combined pass"
        );
        assert_eq!(picture(&[("Depth.Z", 9.0), ("AO.Y", 0.25)]), black, "a layer's Y is not the picture");
        // Unprefixed channels are, whatever the named layers beside them.
        assert_eq!(picture(&[("R", 0.1), ("G", 0.2), ("B", 0.3), ("Image.R", 0.9)]), [0.1, 0.2, 0.3, 1.0]);
        assert_eq!(picture(&[("R", 0.1), ("G", 0.2), ("B", 0.3), ("A", 0.5)]), [0.1, 0.2, 0.3, 0.5]);
        // Luminance: grey, with its alpha.
        assert_eq!(picture(&[("Y", 0.25)]), [0.25, 0.25, 0.25, 1.0]);
        assert_eq!(picture(&[("A", 0.5), ("Y", 0.25), ("diffuse.R", 0.9)]), [0.25, 0.25, 0.25, 0.5]);
    }
}
