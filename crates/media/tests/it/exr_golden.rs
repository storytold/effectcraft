//! OpenEXR pictures and aux channels pinned to exact hashes (#474).
//!
//! Synthetic files (single-layer, multi-layer with and without an unnamed RGB layer, multi-part,
//! data windows smaller and larger than the display window, half / float / uint channels, RLE,
//! ZIP, PIZ and DWAA) are decoded through the media pool as the renderer asks for them: the
//! picture in each alpha interpretation, and the channels EXtractoR and Cryptomatte read. The
//! hashes were taken before the picture and the channels started sharing one decode, so a change
//! to how either is decoded must leave every bit as it was. Only bit-exact float steps are
//! pinned (linear light, no `powf`), so the hashes hold on every platform; DWAA (whose decoder
//! uses `powf`) and the sRGB / gamma 2.4 curves are checked for agreement between the two ways a
//! frame can be decoded instead. Re-pin with `EXR_GOLDEN_PRINT=1` after an intended change.

use std::sync::Arc;

use effectcraft_media::{MediaPool, probe_bytes};
use effectcraft_project::{AlphaMode, Footage, ItemId};
use effectcraft_render::{FloatCurve, FootageSource};
use effectcraft_time::Tick;
use exr::prelude::*;

const W: usize = 37;
const H: usize = 23;

/// A deterministic value for pixel (x, y) of channel `c`: negative, in range and above 1.
fn value(x: usize, y: usize, c: usize) -> f32 {
    ((x * 7 + y * 13 + c * 29) % 97) as f32 / 40.0 - 0.3
}

/// A deterministic alpha in 0..=1 (0 included, for the premultiplied divide).
fn alpha(x: usize, y: usize) -> f32 {
    ((x * 3 + y * 5) % 11) as f32 / 10.0
}

#[derive(Clone, Copy)]
enum Kind {
    Half,
    Float,
    Uint,
}

/// A channel `name` of `size` filled from [`value`] (or [`alpha`] for an `A` / `a` channel).
fn channel(name: &str, kind: Kind, size: (usize, usize), c: usize) -> AnyChannel<FlatSamples> {
    let is_alpha = name.rsplit('.').next().is_some_and(|n| n.eq_ignore_ascii_case("a"));
    let px = |i: usize| {
        let (x, y) = (i % size.0, i / size.0);
        if is_alpha { alpha(x, y) } else { value(x, y, c) }
    };
    let n = size.0 * size.1;
    let samples = match kind {
        Kind::Half => FlatSamples::F16((0..n).map(|i| f16::from_f32(px(i))).collect()),
        Kind::Float => FlatSamples::F32((0..n).map(px).collect()),
        Kind::Uint => FlatSamples::U32((0..n).map(|i| (i as u32).wrapping_mul(2_654_435_761) >> 8).collect()),
    };
    AnyChannel::new(name, samples)
}

fn channels(list: &[(&str, Kind)], size: (usize, usize)) -> AnyChannels<FlatSamples> {
    AnyChannels::sort(list.iter().enumerate().map(|(c, (n, k))| channel(n, *k, size, c)).collect::<Vec<_>>().into())
}

fn encoding(compression: Compression) -> Encoding {
    Encoding { compression, blocks: Blocks::ScanLines, line_order: LineOrder::Increasing }
}

fn with_manifest(mut attrs: LayerAttributes) -> LayerAttributes {
    attrs.other.insert(Text::from("cryptomatte/abc1234/name"), AttributeValue::Text(Text::from("CryptoObject")));
    attrs.other.insert(Text::from("cryptomatte/abc1234/manifest"), AttributeValue::Text(Text::from(r#"{"bunny":"13851a76","floor":"0a1b2c3d"}"#)));
    attrs
}

/// One part: `list` channels over a data window of `size` at `pos`.
fn part(name: Option<&str>, list: &[(&str, Kind)], size: (usize, usize), pos: (i32, i32), compression: Compression) -> Layer<AnyChannels<FlatSamples>> {
    part_with(name, list, size, pos, encoding(compression))
}

/// [`part`] with its own block layout (tiles) and line order.
fn part_with(name: Option<&str>, list: &[(&str, Kind)], size: (usize, usize), pos: (i32, i32), encoding: Encoding) -> Layer<AnyChannels<FlatSamples>> {
    let mut attrs = name.map_or_else(LayerAttributes::default, LayerAttributes::named);
    attrs.layer_position = Vec2(pos.0, pos.1);
    Layer::new(size, with_manifest(attrs), encoding, channels(list, size))
}

/// A tiled single-part file of `list` with mip levels down to 1×1 (each level its own values);
/// the picture and the channels are its largest level.
fn mipmapped(list: &[(&str, Kind)], compression: Compression) -> Vec<u8> {
    let levels = |name: &str, kind: Kind, c: usize| {
        let (mut data, mut size) = (vec![], (W, H));
        loop {
            data.push(channel(name, kind, size, c + data.len()).sample_data);
            if size == (1, 1) {
                break;
            }
            size = ((size.0 / 2).max(1), (size.1 / 2).max(1));
        }
        AnyChannel::new(name, Levels::Mip { rounding_mode: exr::math::RoundingMode::Down, level_data: data })
    };
    let list: Vec<AnyChannel<Levels<FlatSamples>>> = list.iter().enumerate().map(|(c, (n, k))| levels(n, *k, c)).collect();
    let encoding = Encoding { compression, blocks: Blocks::Tiles(Vec2(16, 16)), line_order: LineOrder::Increasing };
    let layer = Layer::new((W, H), with_manifest(LayerAttributes::default()), encoding, AnyChannels::sort(list.into()));
    let mut bytes = std::io::Cursor::new(Vec::new());
    Image::from_layer(layer).write().non_parallel().to_buffered(&mut bytes).unwrap();
    bytes.into_inner()
}

/// Encode `layers` with the display window `display` (position, size).
fn write(layers: Vec<Layer<AnyChannels<FlatSamples>>>, display: ((i32, i32), (usize, usize))) -> Vec<u8> {
    let mut attrs = ImageAttributes::new(IntegerBounds::new(display.0, display.1));
    attrs.pixel_aspect = 1.0;
    let image = Image::from_layers(attrs, layers);
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write().non_parallel().to_buffered(&mut bytes).unwrap();
    bytes.into_inner()
}

const RGBA_HALF: [(&str, Kind); 4] = [("R", Kind::Half), ("G", Kind::Half), ("B", Kind::Half), ("A", Kind::Half)];
const CRYPTO: [(&str, Kind); 4] =
    [("CryptoObject00.r", Kind::Float), ("CryptoObject00.g", Kind::Float), ("CryptoObject00.b", Kind::Float), ("CryptoObject00.a", Kind::Float)];

/// The synthetic files: (name, bytes, whether the hashes are pinned).
fn files() -> Vec<(&'static str, Vec<u8>, bool)> {
    let full = (W, H);
    let origin = ((0, 0), full);
    let cat = |a: &[(&'static str, Kind)], b: &[(&'static str, Kind)]| -> Vec<(&'static str, Kind)> { a.iter().chain(b).copied().collect() };
    let layered: Vec<(&str, Kind)> = cat(
        &[
            ("Image.R", Kind::Half),
            ("Image.G", Kind::Half),
            ("Image.B", Kind::Half),
            ("Image.A", Kind::Half),
            ("depth.Z", Kind::Float),
            ("ObjectID", Kind::Uint),
        ],
        &CRYPTO,
    );
    let unnamed_plus: Vec<(&str, Kind)> =
        cat(&cat(&RGBA_HALF, &[("Diffuse.R", Kind::Half), ("Diffuse.G", Kind::Half), ("Diffuse.B", Kind::Half), ("Z", Kind::Float)]), &CRYPTO);
    let rgb_float = [("R", Kind::Float), ("G", Kind::Float), ("B", Kind::Float), ("depth.Z", Kind::Float)];
    vec![
        ("rgba_half_zip16", write(vec![part(None, &RGBA_HALF, full, (0, 0), Compression::ZIP16)], origin), true),
        (
            "rgb_float_piz",
            write(vec![part(None, &[("R", Kind::Float), ("G", Kind::Float), ("B", Kind::Float), ("Z", Kind::Float)], full, (0, 0), Compression::PIZ)], origin),
            true,
        ),
        ("layered_rle", write(vec![part(None, &layered, full, (0, 0), Compression::RLE)], origin), true),
        ("unnamed_rgb_plus_layers_zip1", write(vec![part(None, &unnamed_plus, full, (0, 0), Compression::ZIP1)], origin), true),
        (
            "multipart_zip16",
            write(
                vec![
                    part(Some("Image"), &RGBA_HALF, full, (0, 0), Compression::ZIP16),
                    part(Some("depth"), &[("Z", Kind::Float)], full, (0, 0), Compression::ZIP16),
                    part(
                        Some("CryptoObject00"),
                        &[("r", Kind::Float), ("g", Kind::Float), ("b", Kind::Float), ("a", Kind::Float)],
                        full,
                        (0, 0),
                        Compression::ZIP16,
                    ),
                ],
                origin,
            ),
            true,
        ),
        (
            // The first part with R, G and B is not the beauty pass.
            "multipart_diffuse_then_beauty_zip1",
            write(
                vec![
                    part(Some("diffuse"), &[("R", Kind::Half), ("G", Kind::Half), ("B", Kind::Half)], full, (0, 0), Compression::ZIP1),
                    part(Some("beauty"), &RGBA_HALF, (20, 10), (5, 7), Compression::ZIP1),
                    part(Some("depth"), &[("Z", Kind::Float)], full, (0, 0), Compression::ZIP1),
                ],
                origin,
            ),
            true,
        ),
        (
            "multipart_blender5_rle",
            write(
                vec![
                    part(Some("ViewLayer.Depth"), &[("ViewLayer.Depth.Z", Kind::Float)], full, (0, 0), Compression::RLE),
                    part(
                        Some("ViewLayer.Combined"),
                        &[
                            ("ViewLayer.Combined.R", Kind::Half),
                            ("ViewLayer.Combined.G", Kind::Half),
                            ("ViewLayer.Combined.B", Kind::Half),
                            ("ViewLayer.Combined.A", Kind::Half),
                        ],
                        full,
                        (0, 0),
                        Compression::RLE,
                    ),
                ],
                origin,
            ),
            true,
        ),
        ("unnamed_rgb_window_smaller_zip16", write(vec![part(None, &unnamed_plus, (20, 10), (5, 7), Compression::ZIP16)], origin), true),
        ("rgb_float_window_larger_piz", write(vec![part(None, &rgb_float, (45, 30), (-1, -5), Compression::PIZ)], ((3, -2), full)), true),
        ("layered_window_smaller_zip16", write(vec![part(None, &layered, (20, 10), (5, 7), Compression::ZIP16)], origin), true),
        ("layered_window_larger_piz", write(vec![part(None, &layered, (45, 30), (-4, -3), Compression::PIZ)], origin), true),
        ("unnamed_rgb_plus_layers_dwaa", write(vec![part(None, &unnamed_plus, full, (0, 0), Compression::DWAA(None))], origin), false),
        ("layered_dwaa", write(vec![part(None, &layered, full, (0, 0), Compression::DWAA(None))], origin), false),
        (
            "unnamed_rgb_plus_layers_tiled_zip16",
            write(
                vec![part_with(None, &unnamed_plus, (30, 20), (3, 2), Encoding { blocks: Blocks::Tiles(Vec2(16, 8)), ..encoding(Compression::ZIP16) })],
                origin,
            ),
            true,
        ),
        (
            "layered_tiled_rle",
            write(vec![part_with(None, &layered, full, (0, 0), Encoding { blocks: Blocks::Tiles(Vec2(8, 8)), ..encoding(Compression::RLE) })], origin),
            true,
        ),
        ("unnamed_rgb_plus_layers_mipmapped_zip16", mipmapped(&unnamed_plus, Compression::ZIP16), true),
        ("layered_mipmapped_piz", mipmapped(&layered, Compression::PIZ), true),
        (
            "unnamed_rgb_plus_layers_decreasing_zip1",
            write(
                vec![part_with(None, &unnamed_plus, (20, 10), (5, 7), Encoding { line_order: LineOrder::Decreasing, ..encoding(Compression::ZIP1) })],
                origin,
            ),
            true,
        ),
        (
            "layered_decreasing_rle",
            write(vec![part_with(None, &layered, full, (0, 0), Encoding { line_order: LineOrder::Decreasing, ..encoding(Compression::RLE) })], origin),
            true,
        ),
        ("unnamed_rgb_plus_layers_uncompressed", write(vec![part(None, &unnamed_plus, (20, 10), (-3, 4), Compression::Uncompressed)], origin), true),
        ("layered_uncompressed", write(vec![part(None, &layered, full, (0, 0), Compression::Uncompressed)], origin), true),
        (
            "uint_rgb_zip16",
            write(
                vec![part(
                    None,
                    &[("R", Kind::Uint), ("G", Kind::Uint), ("B", Kind::Uint), ("A", Kind::Half), ("depth.Z", Kind::Float)],
                    full,
                    (0, 0),
                    Compression::ZIP16,
                )],
                origin,
            ),
            true,
        ),
        (
            // The image crate shows the part "depth" (the first with R, G and B), whose planes are
            // filled with the background depth outside its data window: decoded by the image crate.
            "multipart_depth_named_rgb_zip16",
            write(
                vec![
                    part(Some("depth"), &[("R", Kind::Half), ("G", Kind::Half), ("B", Kind::Half)], (20, 10), (5, 7), Compression::ZIP16),
                    part(Some("beauty"), &RGBA_HALF, full, (0, 0), Compression::ZIP16),
                ],
                origin,
            ),
            true,
        ),
    ]
}

/// FNV-1a over 32-bit words.
fn fnv(words: impl IntoIterator<Item = u32>) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for w in words {
        for b in w.to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

fn picture_hash(pool: &MediaPool, f: &Footage, float: FloatCurve, size: Option<(u32, u32)>) -> u64 {
    let img = pool.frame_with(ItemId(1), f, Tick::ZERO, size, float).expect("picture");
    fnv([img.width, img.height].into_iter().chain(img.data.iter().flat_map(|p| p.map(f32::to_bits))))
}

fn aux_hash(pool: &MediaPool, f: &Footage) -> u64 {
    let Some(aux) = pool.aux(ItemId(1), f, Tick::ZERO) else { return 0 };
    let mut words = vec![aux.width, aux.height, aux.scale.to_bits() as u32];
    for (name, plane) in &aux.channels {
        words.extend(name.bytes().map(u32::from));
        words.extend(plane.iter().map(|v| v.to_bits()));
    }
    for (name, objects) in &aux.manifests {
        words.extend(name.bytes().map(u32::from));
        for (o, h) in objects {
            words.extend(o.bytes().map(u32::from));
            words.push(*h);
        }
    }
    fnv(words)
}

/// The interpretations a picture is decoded in: (label, footage, curve, size, pinned).
fn variants(f: &Footage) -> Vec<(String, Footage, FloatCurve, Option<(u32, u32)>, bool)> {
    let mut out = vec![];
    for (mode, label) in
        [(None, "probed"), (Some(AlphaMode::Straight), "straight"), (Some(AlphaMode::Premultiplied), "premul"), (Some(AlphaMode::Ignore), "ignore")]
    {
        let mut g = f.clone();
        if let Some(m) = mode {
            g.alpha = m;
        }
        g.premul_color = [0.2, 0.4, 0.6];
        out.push((format!("{label}/linear"), g.clone(), FloatCurve::Linear, None, true));
        out.push((format!("{label}/srgb"), g.clone(), FloatCurve::Srgb, None, false));
        out.push((format!("{label}/gamma24"), g.clone(), FloatCurve::Gamma24, None, false));
    }
    let preserve = Footage { preserve_rgb: true, ..f.clone() };
    out.push(("preserve_rgb".into(), preserve, FloatCurve::Srgb, None, true));
    out.push(("half_size/srgb".into(), f.clone(), FloatCurve::Srgb, Some((f.width.div_ceil(2), f.height.div_ceil(2))), false));
    out
}

/// Every hash of `file`: the size and alpha import finds, each picture and the channels, decoded
/// picture first (the footage alone) or channels first (an effect reads them).
fn hashes(name: &str, bytes: &[u8], aux_first: bool) -> Vec<(String, u64, bool)> {
    let order = if aux_first { "aux-first" } else { "picture-first" };
    let path = format!("/exr-golden-474/{order}/{name}.exr");
    let b: Arc<[u8]> = bytes.into();
    let pool = MediaPool::new();
    pool.add_bytes(&path, b.clone());
    let f = probe_bytes(&path, b).expect("probe");
    let mut out = vec![("probe".to_string(), fnv([f.width, f.height, f.alpha as u32]), true)];
    let aux = if aux_first { Some(aux_hash(&pool, &f)) } else { None };
    for (label, g, float, size, pinned) in variants(&f) {
        out.push((label, picture_hash(&pool, &g, float, size), pinned));
    }
    out.push(("aux".into(), aux.unwrap_or_else(|| aux_hash(&pool, &f)), true));
    out
}

/// Hashes taken on main before #474's shared decode (`EXR_GOLDEN_PRINT=1`).
const PINNED: &[(&str, &str, u64)] = &[
    ("rgba_half_zip16", "probe", 0x0a32e074a28c1f66),
    ("rgba_half_zip16", "probed/linear", 0xa84600283dce98f6),
    ("rgba_half_zip16", "straight/linear", 0xf028e1367a2fbb30),
    ("rgba_half_zip16", "premul/linear", 0xa84600283dce98f6),
    ("rgba_half_zip16", "ignore/linear", 0xa90a57f9a67f7468),
    ("rgba_half_zip16", "preserve_rgb", 0xc8e0050f5efe75b7),
    ("rgba_half_zip16", "aux", 0xdd555e6cf67d75f3),
    ("rgb_float_piz", "probe", 0x6a38346c4c5662d5),
    ("rgb_float_piz", "probed/linear", 0xa6e8fbbad371a2ae),
    ("rgb_float_piz", "straight/linear", 0xa6e8fbbad371a2ae),
    ("rgb_float_piz", "premul/linear", 0x515f476c176229c9),
    ("rgb_float_piz", "ignore/linear", 0xa6e8fbbad371a2ae),
    ("rgb_float_piz", "preserve_rgb", 0xa6e8fbbad371a2ae),
    ("rgb_float_piz", "aux", 0x554623d237c2e5ef),
    ("layered_rle", "probe", 0x0a32e074a28c1f66),
    ("layered_rle", "probed/linear", 0xa84600283dce98f6),
    ("layered_rle", "straight/linear", 0xf028e1367a2fbb30),
    ("layered_rle", "premul/linear", 0xa84600283dce98f6),
    ("layered_rle", "ignore/linear", 0xa90a57f9a67f7468),
    ("layered_rle", "preserve_rgb", 0xc8e0050f5efe75b7),
    ("layered_rle", "aux", 0xa6b9bd9f02c192ad),
    ("unnamed_rgb_plus_layers_zip1", "probe", 0x0a32e074a28c1f66),
    ("unnamed_rgb_plus_layers_zip1", "probed/linear", 0xa84600283dce98f6),
    ("unnamed_rgb_plus_layers_zip1", "straight/linear", 0xf028e1367a2fbb30),
    ("unnamed_rgb_plus_layers_zip1", "premul/linear", 0xa84600283dce98f6),
    ("unnamed_rgb_plus_layers_zip1", "ignore/linear", 0xa90a57f9a67f7468),
    ("unnamed_rgb_plus_layers_zip1", "preserve_rgb", 0xc8e0050f5efe75b7),
    ("unnamed_rgb_plus_layers_zip1", "aux", 0x27cc9b53d0862db1),
    ("multipart_zip16", "probe", 0x0a32e074a28c1f66),
    ("multipart_zip16", "probed/linear", 0xa84600283dce98f6),
    ("multipart_zip16", "straight/linear", 0xf028e1367a2fbb30),
    ("multipart_zip16", "premul/linear", 0xa84600283dce98f6),
    ("multipart_zip16", "ignore/linear", 0xa90a57f9a67f7468),
    ("multipart_zip16", "preserve_rgb", 0xc8e0050f5efe75b7),
    ("multipart_zip16", "aux", 0x567ad99a6690d03d),
    ("multipart_diffuse_then_beauty_zip1", "probe", 0x6a38346c4c5662d5),
    ("multipart_diffuse_then_beauty_zip1", "probed/linear", 0xa90a57f9a67f7468),
    ("multipart_diffuse_then_beauty_zip1", "straight/linear", 0xa90a57f9a67f7468),
    ("multipart_diffuse_then_beauty_zip1", "premul/linear", 0x7ab18932e25c5cc8),
    ("multipart_diffuse_then_beauty_zip1", "ignore/linear", 0xa90a57f9a67f7468),
    ("multipart_diffuse_then_beauty_zip1", "preserve_rgb", 0xa90a57f9a67f7468),
    ("multipart_diffuse_then_beauty_zip1", "aux", 0x6724d6dfd56a04cb),
    ("multipart_blender5_rle", "probe", 0x0a32e074a28c1f66),
    ("multipart_blender5_rle", "probed/linear", 0xa84600283dce98f6),
    ("multipart_blender5_rle", "straight/linear", 0xf028e1367a2fbb30),
    ("multipart_blender5_rle", "premul/linear", 0xa84600283dce98f6),
    ("multipart_blender5_rle", "ignore/linear", 0xa90a57f9a67f7468),
    ("multipart_blender5_rle", "preserve_rgb", 0xc8e0050f5efe75b7),
    ("multipart_blender5_rle", "aux", 0x522ae4819dd92e89),
    ("unnamed_rgb_window_smaller_zip16", "probe", 0x0a32e074a28c1f66),
    ("unnamed_rgb_window_smaller_zip16", "probed/linear", 0x19eb3b1a88d108a7),
    ("unnamed_rgb_window_smaller_zip16", "straight/linear", 0x2c73f9acaa936420),
    ("unnamed_rgb_window_smaller_zip16", "premul/linear", 0x19eb3b1a88d108a7),
    ("unnamed_rgb_window_smaller_zip16", "ignore/linear", 0x3b86057d18a16bf0),
    ("unnamed_rgb_window_smaller_zip16", "preserve_rgb", 0x5b999014e4a64bb2),
    ("unnamed_rgb_window_smaller_zip16", "aux", 0x8f78d3233450acf3),
    ("rgb_float_window_larger_piz", "probe", 0x6a38346c4c5662d5),
    ("rgb_float_window_larger_piz", "probed/linear", 0x8e3847d48f9967a6),
    ("rgb_float_window_larger_piz", "straight/linear", 0x8e3847d48f9967a6),
    ("rgb_float_window_larger_piz", "premul/linear", 0x0998eefc456dcdc9),
    ("rgb_float_window_larger_piz", "ignore/linear", 0x8e3847d48f9967a6),
    ("rgb_float_window_larger_piz", "preserve_rgb", 0x8e3847d48f9967a6),
    ("rgb_float_window_larger_piz", "aux", 0x7ccc9f86e6ff6140),
    ("layered_window_smaller_zip16", "probe", 0x0a32e074a28c1f66),
    ("layered_window_smaller_zip16", "probed/linear", 0x19eb3b1a88d108a7),
    ("layered_window_smaller_zip16", "straight/linear", 0x2c73f9acaa936420),
    ("layered_window_smaller_zip16", "premul/linear", 0x19eb3b1a88d108a7),
    ("layered_window_smaller_zip16", "ignore/linear", 0x3b86057d18a16bf0),
    ("layered_window_smaller_zip16", "preserve_rgb", 0x5b999014e4a64bb2),
    ("layered_window_smaller_zip16", "aux", 0x443480a1d789e63c),
    ("layered_window_larger_piz", "probe", 0x0a32e074a28c1f66),
    ("layered_window_larger_piz", "probed/linear", 0x443afe4029748836),
    ("layered_window_larger_piz", "straight/linear", 0xb32b44880f746826),
    ("layered_window_larger_piz", "premul/linear", 0x443afe4029748836),
    ("layered_window_larger_piz", "ignore/linear", 0xaa01706accce2cc4),
    ("layered_window_larger_piz", "preserve_rgb", 0xb2cbb40b10bc56b4),
    ("layered_window_larger_piz", "aux", 0xb7d91c8eea56a772),
    ("unnamed_rgb_plus_layers_tiled_zip16", "probe", 0x0a32e074a28c1f66),
    ("unnamed_rgb_plus_layers_tiled_zip16", "probed/linear", 0x7612f20634fe957e),
    ("unnamed_rgb_plus_layers_tiled_zip16", "straight/linear", 0xaa5f42cb6a31c78a),
    ("unnamed_rgb_plus_layers_tiled_zip16", "premul/linear", 0x7612f20634fe957e),
    ("unnamed_rgb_plus_layers_tiled_zip16", "ignore/linear", 0xbbf236635dcaac90),
    ("unnamed_rgb_plus_layers_tiled_zip16", "preserve_rgb", 0x15b555a243d79613),
    ("unnamed_rgb_plus_layers_tiled_zip16", "aux", 0x3d1439eed2202e41),
    ("layered_tiled_rle", "probe", 0x0a32e074a28c1f66),
    ("layered_tiled_rle", "probed/linear", 0xa84600283dce98f6),
    ("layered_tiled_rle", "straight/linear", 0xf028e1367a2fbb30),
    ("layered_tiled_rle", "premul/linear", 0xa84600283dce98f6),
    ("layered_tiled_rle", "ignore/linear", 0xa90a57f9a67f7468),
    ("layered_tiled_rle", "preserve_rgb", 0xc8e0050f5efe75b7),
    ("layered_tiled_rle", "aux", 0xa6b9bd9f02c192ad),
    ("unnamed_rgb_plus_layers_mipmapped_zip16", "probe", 0x0a32e074a28c1f66),
    ("unnamed_rgb_plus_layers_mipmapped_zip16", "probed/linear", 0xa84600283dce98f6),
    ("unnamed_rgb_plus_layers_mipmapped_zip16", "straight/linear", 0xf028e1367a2fbb30),
    ("unnamed_rgb_plus_layers_mipmapped_zip16", "premul/linear", 0xa84600283dce98f6),
    ("unnamed_rgb_plus_layers_mipmapped_zip16", "ignore/linear", 0xa90a57f9a67f7468),
    ("unnamed_rgb_plus_layers_mipmapped_zip16", "preserve_rgb", 0xc8e0050f5efe75b7),
    ("unnamed_rgb_plus_layers_mipmapped_zip16", "aux", 0x27cc9b53d0862db1),
    ("layered_mipmapped_piz", "probe", 0x0a32e074a28c1f66),
    ("layered_mipmapped_piz", "probed/linear", 0xa84600283dce98f6),
    ("layered_mipmapped_piz", "straight/linear", 0xf028e1367a2fbb30),
    ("layered_mipmapped_piz", "premul/linear", 0xa84600283dce98f6),
    ("layered_mipmapped_piz", "ignore/linear", 0xa90a57f9a67f7468),
    ("layered_mipmapped_piz", "preserve_rgb", 0xc8e0050f5efe75b7),
    ("layered_mipmapped_piz", "aux", 0xa6b9bd9f02c192ad),
    ("unnamed_rgb_plus_layers_decreasing_zip1", "probe", 0x0a32e074a28c1f66),
    ("unnamed_rgb_plus_layers_decreasing_zip1", "probed/linear", 0x19eb3b1a88d108a7),
    ("unnamed_rgb_plus_layers_decreasing_zip1", "straight/linear", 0x2c73f9acaa936420),
    ("unnamed_rgb_plus_layers_decreasing_zip1", "premul/linear", 0x19eb3b1a88d108a7),
    ("unnamed_rgb_plus_layers_decreasing_zip1", "ignore/linear", 0x3b86057d18a16bf0),
    ("unnamed_rgb_plus_layers_decreasing_zip1", "preserve_rgb", 0x5b999014e4a64bb2),
    ("unnamed_rgb_plus_layers_decreasing_zip1", "aux", 0x8f78d3233450acf3),
    ("layered_decreasing_rle", "probe", 0x0a32e074a28c1f66),
    ("layered_decreasing_rle", "probed/linear", 0xa84600283dce98f6),
    ("layered_decreasing_rle", "straight/linear", 0xf028e1367a2fbb30),
    ("layered_decreasing_rle", "premul/linear", 0xa84600283dce98f6),
    ("layered_decreasing_rle", "ignore/linear", 0xa90a57f9a67f7468),
    ("layered_decreasing_rle", "preserve_rgb", 0xc8e0050f5efe75b7),
    ("layered_decreasing_rle", "aux", 0xa6b9bd9f02c192ad),
    ("unnamed_rgb_plus_layers_uncompressed", "probe", 0x0a32e074a28c1f66),
    ("unnamed_rgb_plus_layers_uncompressed", "probed/linear", 0xa27668bf31d9426b),
    ("unnamed_rgb_plus_layers_uncompressed", "straight/linear", 0xfc73fe89457ce029),
    ("unnamed_rgb_plus_layers_uncompressed", "premul/linear", 0xa27668bf31d9426b),
    ("unnamed_rgb_plus_layers_uncompressed", "ignore/linear", 0xb3276cc5126776d4),
    ("unnamed_rgb_plus_layers_uncompressed", "preserve_rgb", 0xdee1105fa6e6822a),
    ("unnamed_rgb_plus_layers_uncompressed", "aux", 0x854a5e84e7aaa5c2),
    ("layered_uncompressed", "probe", 0x0a32e074a28c1f66),
    ("layered_uncompressed", "probed/linear", 0xa84600283dce98f6),
    ("layered_uncompressed", "straight/linear", 0xf028e1367a2fbb30),
    ("layered_uncompressed", "premul/linear", 0xa84600283dce98f6),
    ("layered_uncompressed", "ignore/linear", 0xa90a57f9a67f7468),
    ("layered_uncompressed", "preserve_rgb", 0xc8e0050f5efe75b7),
    ("layered_uncompressed", "aux", 0xa6b9bd9f02c192ad),
    ("uint_rgb_zip16", "probe", 0x0a32e074a28c1f66),
    ("uint_rgb_zip16", "probed/linear", 0xed89310ef0cdb9fd),
    ("uint_rgb_zip16", "straight/linear", 0xf9fe11ab8ed97cc8),
    ("uint_rgb_zip16", "premul/linear", 0xed89310ef0cdb9fd),
    ("uint_rgb_zip16", "ignore/linear", 0xf78cd263c0d74f12),
    ("uint_rgb_zip16", "preserve_rgb", 0x2a19d67b07cf4359),
    ("uint_rgb_zip16", "aux", 0x03e42411f8521856),
    ("multipart_depth_named_rgb_zip16", "probe", 0x6a38346c4c5662d5),
    ("multipart_depth_named_rgb_zip16", "probed/linear", 0x3b86057d18a16bf0),
    ("multipart_depth_named_rgb_zip16", "straight/linear", 0x3b86057d18a16bf0),
    ("multipart_depth_named_rgb_zip16", "premul/linear", 0x869374e9b0384cea),
    ("multipart_depth_named_rgb_zip16", "ignore/linear", 0x3b86057d18a16bf0),
    ("multipart_depth_named_rgb_zip16", "preserve_rgb", 0x3b86057d18a16bf0),
    ("multipart_depth_named_rgb_zip16", "aux", 0x0fa56884e83b4417),
];

#[test]
fn exr_pictures_and_channels_are_bit_identical() {
    let print = std::env::var_os("EXR_GOLDEN_PRINT").is_some();
    let mut failures = vec![];
    for (name, bytes, pinned_file) in files() {
        let first = hashes(name, &bytes, false);
        let second = hashes(name, &bytes, true);
        for ((label, h, pinned), (_, h2, _)) in first.iter().zip(&second) {
            if h != h2 {
                failures.push(format!("{name} {label}: picture first {h:#018x}, channels first {h2:#018x}"));
            }
            if print {
                println!("    (\"{name}\", \"{label}\", {h:#018x}),{}", if *pinned && pinned_file { "" } else { " // not pinned" });
                continue;
            }
            if !(*pinned && pinned_file) {
                continue;
            }
            match PINNED.iter().find(|(n, l, _)| *n == name && l == label) {
                Some((_, _, want)) if want == h => {}
                Some((_, _, want)) => failures.push(format!("{name} {label}: {h:#018x}, pinned {want:#018x}")),
                None => failures.push(format!("{name} {label}: {h:#018x} is not pinned")),
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
