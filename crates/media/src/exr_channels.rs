//! Auxiliary channels from multi-layer OpenEXR files (depth, object / material IDs, normals,
//! Cryptomatte, …) for the 3D Channel effects, read with the pure-Rust `exr` crate.
//!
//! Every channel of every layer becomes a plane named `layer.channel` (or just `channel` for
//! the unnamed layer), placed in the display window (pixels outside a layer's data window are
//! 0, or the background depth for depth channels). Cryptomatte manifests come from the
//! `cryptomatte/<key>/name` and `cryptomatte/<key>/manifest` header attributes (the published
//! Cryptomatte metadata convention: the manifest is a JSON object of name → hex hash).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use effectcraft_raster::AuxChannels;
use effectcraft_raster::channels3d::{BACKGROUND_DEPTH, split_channel};

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

/// Read all channels of an EXR file's bytes. `None` when it is not a readable EXR.
pub fn read_exr_channels(bytes: &[u8]) -> Option<AuxChannels> {
    use exr::prelude::*;
    let img = read().no_deep_data().largest_resolution_level().all_channels().all_layers().all_attributes().from_buffered(std::io::Cursor::new(bytes)).ok()?;
    let dw = img.attributes.display_window;
    let (w, h) = (dw.size.0, dw.size.1);
    if w == 0 || h == 0 {
        return None;
    }
    let mut aux = AuxChannels::new(w as u32, h as u32, 1.0);
    let mut crypto: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
    let mut collect_meta = |other: &HashMap<Text, AttributeValue>| {
        for (k, v) in other {
            let key = k.to_string();
            let Some(rest) = key.strip_prefix("cryptomatte/") else { continue };
            let Some((id, field)) = rest.split_once('/') else { continue };
            let AttributeValue::Text(t) = v else { continue };
            let e = crypto.entry(id.to_string()).or_default();
            match field {
                "name" => e.0 = Some(t.to_string()),
                "manifest" => e.1 = Some(t.to_string()),
                _ => {}
            }
        }
    };
    collect_meta(&img.attributes.other);
    // Each channel is a display-window plane: a 60000² window over a 1×1 data window was a
    // multi-gigabyte allocation per channel (#2141).
    let n = w.checked_mul(h).filter(|n| *n <= MAX_DISPLAY_PIXELS)?;
    // The decoded image is consumed channel by channel, so each channel's samples are freed (or
    // moved) as its plane is made instead of the whole file being held twice (#474).
    for layer in img.layer_data {
        collect_meta(&layer.attributes.other);
        let prefix = layer.attributes.layer_name.as_ref().map(|t| t.to_string()).filter(|s| !s.is_empty());
        let size = (layer.size.0, layer.size.1);
        let pos = layer.attributes.layer_position;
        let offset = (i64::from(pos.0) - i64::from(dw.position.0), i64::from(pos.1) - i64::from(dw.position.1));
        for ch in layer.channel_data.list {
            let name = channel_name(prefix.as_deref(), &ch.name.to_string());
            let fill = if is_depth(&name) { BACKGROUND_DEPTH } else { 0.0 };
            if let Some(plane) = place(ch.sample_data, size, offset, (w, h), n, fill) {
                aux.channels.push((name, plane));
            }
        }
    }
    for (_, (name, manifest)) in crypto {
        if let (Some(n), Some(m)) = (name, manifest) {
            aux.manifests.push((n, parse_manifest(&m)));
        }
    }
    aux.manifests.sort();
    Some(aux)
}

/// One channel's samples (`size`, at `offset` in the `display` window of `n` pixels) as a
/// display-window plane, `fill` outside its data window. A channel covering exactly the display
/// window is converted in one pass, and an f32 one is moved without a copy. `None` when the
/// channel has fewer samples than its size.
fn place(samples: exr::image::FlatSamples, size: (usize, usize), offset: (i64, i64), display: (usize, usize), n: usize, fill: f32) -> Option<Vec<f32>> {
    use exr::image::FlatSamples;
    let (lw, lh) = size;
    if samples.len() < lw.checked_mul(lh)? {
        return None;
    }
    if size == display && offset == (0, 0) {
        return Some(match samples {
            FlatSamples::F32(mut v) => {
                v.truncate(n);
                v
            }
            FlatSamples::F16(v) => v.iter().take(n).map(|s| s.to_f32()).collect(),
            FlatSamples::U32(v) => v.iter().take(n).map(|s| *s as f32).collect(),
        });
    }
    let value = |i: usize| match &samples {
        FlatSamples::F32(v) => v.get(i).copied(),
        FlatSamples::F16(v) => v.get(i).map(|s| s.to_f32()),
        FlatSamples::U32(v) => v.get(i).map(|s| *s as f32),
    };
    let (w, h) = display;
    let mut plane = vec![fill; n];
    for y in 0..lh {
        let Some(ty) = usize::try_from(y as i64 + offset.1).ok().filter(|ty| *ty < h) else { continue };
        for x in 0..lw {
            let Some(tx) = usize::try_from(x as i64 + offset.0).ok().filter(|tx| *tx < w) else { continue };
            if let (Some(v), Some(p)) = (value(y * lw + x), plane.get_mut(ty * w + tx)) {
                *p = v;
            }
        }
    }
    Some(plane)
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

/// The picture of a multi-layer OpenEXR file without an unnamed RGB layer (Blender's
/// `ViewLayer.Combined.R`, the compositor's `Image.R`, Nuke's `beauty.R`, …), which the image
/// decoder rejects. After Effects shows a file's main RGBA channels and leaves the other layers
/// to EXtractoR; here that is the beauty pass by name (`BEAUTY`), else the first colour layer
/// that is not a data pass (never Cryptomatte, depth or normals, #412), else the first channel
/// as grey. Linear, straight RGBA as the file stores it. `None` when it is not a readable EXR.
pub fn layered_image(bytes: &[u8]) -> Option<image::DynamicImage> {
    picture_of(&read_exr_channels(bytes)?)
}

/// [`layered_image`] of the EXR at `path` from the channel cache, so the picture and EXtractoR /
/// Cryptomatte share one decode of the file (#474).
pub(crate) fn cached_layered_image(path: &str, bytes: Arc<[u8]>) -> Option<image::DynamicImage> {
    let aux = cached(path, || Some(bytes))?;
    picture_of(&aux)
}

/// The picture of decoded channels (see [`layered_image`]).
fn picture_of(aux: &AuxChannels) -> Option<image::DynamicImage> {
    let layers = aux.layers();
    let colour = picture_layer(aux, &layers)?;
    let names = aux.layer_rgba(colour);
    // A layer without R, G and B channels shows its first channel as grey.
    let first = aux.names().into_iter().find(|n| split_channel(n).0 == colour).unwrap_or_default();
    let plane = |k: usize| -> Option<&[f32]> { aux.get(names.get(k).map(String::as_str).filter(|n| !n.is_empty()).unwrap_or(first)) };
    let (r, g, b) = (plane(0)?, plane(1)?, plane(2)?);
    let a = names.get(3).filter(|n| !n.is_empty()).and_then(|n| aux.get(n));
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

type Cache = Mutex<Vec<(String, Option<Arc<AuxChannels>>)>>;

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

/// Drop the cached channels of `path` (the file changed on disk).
pub(crate) fn forget(path: &str) {
    if let Ok(mut c) = cache().lock() {
        c.retain(|(p, _)| p != path);
    }
}

/// Read (and cache, a few files) the channels of the EXR at `path` via `read`.
pub(crate) fn cached(path: &str, read: impl FnOnce() -> Option<Arc<[u8]>>) -> Option<Arc<AuxChannels>> {
    let cache = cache();
    if let Some(v) = cache.lock().ok().and_then(|c| c.iter().find(|(p, _)| p == path).map(|(_, v)| v.clone())) {
        return v;
    }
    let v = read().and_then(|b| read_exr_channels(&b)).map(Arc::new);
    if let Ok(mut c) = cache.lock() {
        if c.len() >= 4 {
            c.remove(0);
        }
        c.push((path.to_string(), v.clone()));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// #474: a full-window f32 channel (Cryptomatte ranks, …) becomes its plane without a copy,
    /// so a decode no longer holds the file's samples twice; other channels are placed in the
    /// display window as before.
    #[test]
    fn channels_become_planes_without_an_extra_copy() {
        use exr::image::FlatSamples;
        let v = vec![0.25f32; 6];
        let ptr = v.as_ptr();
        let plane = place(FlatSamples::F32(v), (3, 2), (0, 0), (3, 2), 6, 0.0).unwrap();
        assert_eq!(plane.as_ptr(), ptr, "moved, not copied");
        assert_eq!(plane, [0.25; 6]);
        let half = |x: f32| exr::prelude::f16::from_f32(x);
        let full = place(FlatSamples::F16(vec![half(0.5); 6]), (3, 2), (0, 0), (3, 2), 6, 0.0).unwrap();
        assert_eq!(full, [0.5; 6]);
        // A 2×1 data window at (1, 1) of a 3×2 display window; the rest is the fill.
        let part = place(FlatSamples::F16(vec![half(1.0), half(2.0)]), (2, 1), (1, 1), (3, 2), 6, 9.0).unwrap();
        assert_eq!(part, [9.0, 9.0, 9.0, 9.0, 1.0, 2.0]);
        // Partly outside the display window: clipped, never out of bounds.
        let off = place(FlatSamples::U32(vec![7, 8]), (2, 1), (2, -1), (3, 2), 6, 0.0).unwrap();
        assert_eq!(off, [0.0; 6]);
        let edge = place(FlatSamples::U32(vec![7, 8]), (2, 1), (2, 0), (3, 2), 6, 0.0).unwrap();
        assert_eq!(edge, [0.0, 0.0, 7.0, 0.0, 0.0, 0.0]);
        assert!(place(FlatSamples::F32(vec![1.0; 5]), (3, 2), (0, 0), (3, 2), 6, 0.0).is_none(), "too few samples");
    }

    /// #295: a multi-layer EXR whose colour is only in named layers (`diffuse.R`, as Blender and
    /// Nuke write them) failed to import ("does not contain non-deep rgb channels"); it now
    /// shows its colour layer and keeps every channel for EXtractoR.
    /// #474: the picture and EXtractoR / Cryptomatte share one decode of a multi-layer file:
    /// after the frame is decoded, the aux channels come from the cache without reading the file.
    #[test]
    fn picture_and_aux_channels_share_one_decode() {
        use exr::prelude::*;
        let (w, h) = (2usize, 2usize);
        let ch = |n: &str, v: f32| AnyChannel::new(n, FlatSamples::F32(vec![v; w * h]));
        let channels = AnyChannels::sort(vec![ch("Image.R", 0.5), ch("Image.G", 0.5), ch("Image.B", 0.5), ch("GlossDir.R", 0.25)].into());
        let image = Image::from_layer(Layer::new((w, h), LayerAttributes::default(), Encoding::FAST_LOSSLESS, channels));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write().to_buffered(&mut bytes).unwrap();
        let b: Arc<[u8]> = bytes.into_inner().into();
        let path = "/474-shared-decode.exr";
        let pool = crate::MediaPool::new();
        pool.add_bytes(path, b.clone());
        let f = crate::probe_bytes(path, b).unwrap();
        pool.frame_at(&f, effectcraft_time::Tick::ZERO).unwrap();
        let aux = cached(path, || panic!("the frame's decode is reused, the file is not read again")).unwrap();
        assert_eq!(aux.get("GlossDir.R").and_then(|p| p.first()).copied(), Some(0.25));
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
