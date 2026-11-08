//! The OpenEXR index, reader and plane store (#3271) on synthetic files written here.

use std::sync::atomic::{AtomicU64, Ordering};

use effectcraft_project::{Footage, ItemId};
use effectcraft_raster::channels3d::{AuxLookup, AuxNeed};
use effectcraft_render::reads::FileStamp;
use effectcraft_render::{FloatCurve, FootageSource};
use effectcraft_time::Tick;
use exr::prelude::{
    AnyChannel, AnyChannels, Blocks, Compression, Encoding, FlatSamples, ImageAttributes, IntegerBounds, Layer, LayerAttributes, LineOrder, WritableImage, f16,
};

use super::*;
use crate::exr_read::ExrSource;

/// A deterministic sample for pixel `i` of channel `c` (an `A` channel gets 0..=1).
fn value(i: usize, c: usize, alpha: bool) -> f32 {
    if alpha { ((i * 3 + c) % 11) as f32 / 10.0 } else { ((i * 7 + c * 29) % 97) as f32 / 40.0 - 0.3 }
}

/// A part named `name` (`None`: unnamed) of `channels` ((name, half)) over `size`, compressed
/// with `compression` in scan lines.
fn part(name: Option<&str>, channels: &[(&str, bool)], size: (usize, usize), compression: Compression) -> Layer<AnyChannels<FlatSamples>> {
    let n = size.0 * size.1;
    let list: Vec<AnyChannel<FlatSamples>> = channels
        .iter()
        .enumerate()
        .map(|(c, (ch, half))| {
            let alpha = ch.rsplit('.').next().is_some_and(|l| l == "A");
            let samples = if *half {
                FlatSamples::F16((0..n).map(|i| f16::from_f32(value(i, c, alpha))).collect())
            } else {
                FlatSamples::F32((0..n).map(|i| value(i, c, alpha)).collect())
            };
            AnyChannel::new(*ch, samples)
        })
        .collect();
    let mut attrs = name.map_or_else(LayerAttributes::default, LayerAttributes::named);
    if name == Some("Crypto_Material") {
        attrs
            .other
            .insert(exr::prelude::Text::from("cryptomatte/abc1234/name"), exr::prelude::AttributeValue::Text(exr::prelude::Text::from("Crypto_Material")));
        attrs.other.insert(
            exr::prelude::Text::from("cryptomatte/abc1234/manifest"),
            exr::prelude::AttributeValue::Text(exr::prelude::Text::from(r#"{"bunny":"13851a76"}"#)),
        );
    }
    let encoding = Encoding { compression, blocks: Blocks::ScanLines, line_order: LineOrder::Increasing };
    Layer::new(size, attrs, encoding, AnyChannels::sort(list.into()))
}

/// The bytes of a file of `parts` over a display window of `size`.
fn write(parts: Vec<Layer<AnyChannels<FlatSamples>>>, size: (usize, usize)) -> Vec<u8> {
    let image = exr::prelude::Image::from_layers(ImageAttributes::new(IntegerBounds::from_dimensions(size)), parts);
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write().non_parallel().to_buffered(&mut bytes).unwrap();
    bytes.into_inner()
}

const SIZE: (usize, usize) = (64, 64);
const RGBA: [(&str, bool); 4] = [("R", true), ("G", true), ("B", true), ("A", true)];
const CRYPTO: [(&str, bool); 4] =
    [("Crypto_Material00.R", false), ("Crypto_Material00.G", false), ("Crypto_Material00.B", false), ("Crypto_Material00.A", false)];

/// A multi-part file as renderers write one part per pass: the beauty, a Cryptomatte layer whose
/// channels keep their full names, and depth; one scan line per chunk.
fn multipart() -> Vec<u8> {
    write(
        vec![
            part(Some("beauty"), &RGBA, SIZE, Compression::ZIP1),
            part(Some("Crypto_Material"), &CRYPTO, SIZE, Compression::ZIP1),
            part(Some("depth"), &[("Z", false)], SIZE, Compression::ZIP1),
        ],
        SIZE,
    )
}

/// A single-part ZIPS file with a picture and many passes: every chunk holds every channel.
fn single_part_zips() -> Vec<u8> {
    let mut list: Vec<(String, bool)> = RGBA.iter().map(|(n, h)| (n.to_string(), *h)).collect();
    for pass in ["diffuse", "specular", "normal", "emit"] {
        list.extend(["R", "G", "B"].map(|c| (format!("{pass}.{c}"), true)));
    }
    list.extend(["R", "G", "B", "A"].map(|c| (format!("CryptoObject00.{c}"), false)));
    let names: Vec<(&str, bool)> = list.iter().map(|(n, h)| (n.as_str(), *h)).collect();
    write(vec![part(None, &names, SIZE, Compression::ZIP1)], SIZE)
}

/// `bytes` written to a temporary file named after `name` (removed by [`Temp`]).
struct Temp(std::path::PathBuf);

impl Temp {
    fn new(name: &str, bytes: &[u8]) -> Temp {
        let path = std::env::temp_dir().join(format!("ec-3271-{}-{name}.exr", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        Temp(path)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn bits(img: &Image) -> Vec<u32> {
    img.data.iter().flat_map(|p| p.map(f32::to_bits)).collect()
}

/// The picture the `image` crate's decoder shows of `bytes`, converted as the pool converts it.
fn image_crates(bytes: &[u8], f: &Footage) -> Image {
    let img = crate::probe::load_oriented(bytes, Some(image::ImageFormat::OpenExr)).unwrap();
    crate::convert::dynamic_to_image(&img, AlphaOp::of(f, FloatCurve::Srgb))
}

/// The chunks a decode fetched in all.
fn chunks(r: &DecodeRecord) -> usize {
    r.stats.chunks.iter().sum()
}

/// #3271: the picture of a multi-part file reads the beauty part's chunks and nothing else,
/// converts only its channels, and is the `image` crate's picture bit for bit.
#[test]
fn a_picture_reads_only_its_part() {
    let bytes = multipart();
    let file = Temp::new("picture-part", &bytes);
    let f = crate::probe(&file.0).unwrap();
    let path = f.path.clone();
    let pool = crate::MediaPool::new();
    let img = pool.frame_at(&f, Tick::ZERO).unwrap();
    assert_eq!(bits(&img), bits(&image_crates(&bytes, &f)), "the image crate's picture");
    let d = decodes_of(&path);
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0].channels, ["beauty.A", "beauty.B", "beauty.G", "beauty.R"]);
    assert_eq!(d[0].stats.chunks, [SIZE.1, 0, 0], "only the beauty part's chunks");
    let index = crate::exr_index::cached(&path).unwrap();
    let beauty: u64 = index.parts[0].chunks.iter().map(|c| c.len).sum();
    assert_eq!(d[0].stats.bytes, beauty, "the beauty part's bytes and nothing else");
}

/// #3271: EXtractoR reading 3 channels of a multi-part file reads only their part and converts
/// only those channels; the stored names find them (`Crypto_Material00.R` in part
/// `Crypto_Material`), and the manifest and layer resolve as the file names them.
#[test]
fn three_aux_channels_read_only_their_part() {
    let bytes = multipart();
    let file = Temp::new("aux-part", &bytes);
    let f = crate::probe(&file.0).unwrap();
    let path = f.path.clone();
    let pool = crate::MediaPool::new();
    let need = AuxNeed::Lookups(["Crypto_Material00.R", "Crypto_Material00.G", "Crypto_Material00.B"].map(|n| AuxLookup::Name(n.into())).to_vec());
    let aux = pool.aux_for(ItemId(1), &f, Tick::ZERO, &need).unwrap();
    let d = decodes_of(&path);
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].channels.len(), 3, "{:?}", d[0].channels);
    assert_eq!(d[0].stats.chunks, [0, SIZE.1, 0], "only the Cryptomatte part's chunks");
    let r = aux.get("Crypto_Material00.R").unwrap();
    assert_eq!(r.len(), SIZE.0 * SIZE.1);
    assert_eq!(r.get(5).copied(), Some(value(5, 0, false)));
    assert!(aux.get("Crypto_Material00.A").is_none(), "not read");
    assert!(aux.listed.iter().any(|n| n == "depth.Z"), "every name listed");
}

/// #3271: lookups by the name a multi-part file stores a channel under work on all channels too
/// (EXtractoR's `Crypto_Material00.R`, Cryptomatte's Layer Name `Crypto_Material`), while the
/// channel list keeps showing `Crypto_Material.Crypto_Material00.R`.
#[test]
fn multipart_channels_are_found_by_their_stored_names() {
    let bytes = multipart();
    let path = "/3271-stored-names.exr";
    let pool = crate::MediaPool::new();
    pool.add_bytes(path, bytes.as_slice().into());
    let f = crate::probe_bytes(path, bytes.as_slice().into()).unwrap();
    let aux = pool.aux(ItemId(1), &f, Tick::ZERO).unwrap();
    assert!(aux.names().contains(&"Crypto_Material.Crypto_Material00.R"), "{:?}", aux.names());
    assert!(aux.get("Crypto_Material00.R").is_some());
    let layer = effectcraft_raster::channels3d::crypto_layer_named(&aux, "Crypto_Material", 0).unwrap();
    assert_eq!(layer, "Crypto_Material.Crypto_Material");
    assert_eq!(aux.crypto_rank_planes(&layer).len(), 2);
    assert_eq!(aux.manifest(&layer).map(<[_]>::len), Some(1));
    // The names alone, from the headers.
    let names = pool.aux_names(ItemId(1), &f, Tick::ZERO).unwrap();
    assert_eq!(names.names(), aux.names());
    assert!(names.channels.iter().all(|(_, p)| p.is_empty()), "no pixels decoded");
}

/// #3271: footage whose effects read 3 channels decodes its picture with them in one pass: one
/// read of the file's chunks, one decode, for the picture and the effect.
#[test]
fn picture_and_aux_share_one_read_and_one_decode() {
    let bytes = single_part_zips();
    let file = Temp::new("one-decode", &bytes);
    let f = crate::probe(&file.0).unwrap();
    let path = f.path.clone();
    let pool = crate::MediaPool::new();
    let need = AuxNeed::Lookups(vec![AuxLookup::Name("normal.R".into()), AuxLookup::Crypto { name: String::new(), index: 0 }]);
    pool.set_aux_footage(std::slice::from_ref(&f.path));
    pool.set_aux_needs(&[(f.path.clone(), need.clone())]);
    let img = pool.frame_at(&f, Tick::ZERO).unwrap();
    let aux = pool.aux_for(ItemId(1), &f, Tick::ZERO, &need).unwrap();
    let d = decodes_of(&path);
    assert_eq!(d.len(), 1, "one decode: {d:?}");
    assert_eq!(chunks(&d[0]), SIZE.1, "each chunk read once");
    assert_eq!(d[0].channels.len(), 4 + 1 + 4, "the picture's, normal.R and the rank planes: {:?}", d[0].channels);
    assert!(aux.get("normal.R").is_some() && aux.crypto_rank_planes("CryptoObject").len() == 2);
    assert_eq!(bits(&img), bits(&image_crates(&bytes, &f)));
}

/// The index and an in-memory source of `bytes`.
fn indexed(bytes: &[u8]) -> (Arc<ExrIndex>, std::io::Cursor<Vec<u8>>) {
    let mut src = std::io::Cursor::new(bytes.to_vec());
    let ix = crate::exr_index::build(&mut src, bytes.len() as u64, FileStamp::Virtual).unwrap();
    (Arc::new(ix), src)
}

/// #3271: a row range decompresses only the chunks holding those rows: with one scan line per
/// chunk (ZIPS) exactly those lines, with 16 (ZIP) the chunks around them; a reduced level
/// skips every other line on ZIPS. The rows read are the full read's, the rest is fill.
#[test]
fn rows_and_levels_skip_lines() {
    for (compression, lines) in [(Compression::ZIP1, 10), (Compression::ZIP16, 32)] {
        let bytes = write(vec![part(None, &RGBA, (32, 64), compression)], (32, 64));
        let (ix, mut src) = indexed(&bytes);
        let all = ReadRequest::full(vec![0, 1, 2, 3]);
        let (full, full_stats) = crate::exr_read::read_planes(&mut src, &ix, &all);
        let full = full.unwrap();
        assert_eq!(full_stats.lines, 64);
        let band = ReadRequest { rows: Some(10..20), ..all.clone() };
        let (got, stats) = crate::exr_read::read_planes(&mut src, &ix, &band);
        let got = got.unwrap();
        assert_eq!(stats.lines, lines, "{compression:?}");
        for (g, f) in got.iter().zip(&full) {
            for y in 0..64 {
                let row = |p: &Vec<f32>| p[y * 32..(y + 1) * 32].to_vec();
                if (10..20).contains(&y) {
                    assert_eq!(row(g), row(f), "row {y}");
                } else {
                    assert!(row(g).iter().all(|v| *v == 0.0), "row {y} not read");
                }
            }
        }
    }
    // Half resolution on ZIPS: half the lines, every other row and column of the full read.
    let bytes = write(vec![part(None, &RGBA, (32, 64), Compression::ZIP1)], (32, 64));
    let (ix, mut src) = indexed(&bytes);
    let (full, _) = crate::exr_read::read_planes(&mut src, &ix, &ReadRequest::full(vec![3]));
    let (half, stats) = crate::exr_read::read_planes(&mut src, &ix, &ReadRequest { level: 1, ..ReadRequest::full(vec![3]) });
    let (full, half) = (full.unwrap(), half.unwrap());
    assert_eq!(stats.lines, 32);
    assert_eq!(crate::exr_read::level_size((32, 64), 1), (16, 32));
    for y in 0..32 {
        for x in 0..16 {
            assert_eq!(half[0][y * 16 + x].to_bits(), full[0][y * 2 * 32 + x * 2].to_bits(), "({x}, {y})");
        }
    }
}

/// #474: requests for a file at the same time decode it once; the others wait for that decode
/// and share its planes.
#[test]
fn concurrent_requests_decode_a_file_once() {
    let bytes: Arc<[u8]> = single_part_zips().into();
    let path = "/3271-concurrent.exr";
    let source = ExrSource::Bytes(bytes.clone());
    let store = PlaneStore::new(CACHE_BUDGET);
    let index = crate::exr_index::get(path, &source).unwrap();
    let barrier = std::sync::Barrier::new(8);
    let got: Vec<Arc<Batch>> = std::thread::scope(|s| {
        let jobs: Vec<_> = (0..8)
            .map(|_| {
                s.spawn(|| {
                    barrier.wait();
                    store.get(path, &source, &index, &Want { all: true, ids: Default::default() }, Use::Other).unwrap()
                })
            })
            .collect();
        jobs.into_iter().map(|j| j.join().unwrap()).collect()
    });
    assert!(got.iter().all(|b| Arc::ptr_eq(b, &got[0])), "every caller has the same planes");
    assert_eq!(decodes_of(path).len(), 1, "one decode");
}

/// #3271: a request for channels a decode in progress doesn't read waits for it, then decodes
/// only what is still missing; the planes it gets hold both.
#[test]
fn a_request_not_covered_decodes_only_what_is_missing() {
    let bytes: Arc<[u8]> = single_part_zips().into();
    let path = "/3271-follow-up.exr";
    let source = ExrSource::Bytes(bytes);
    let store = PlaneStore::new(CACHE_BUDGET);
    let index = crate::exr_index::get(path, &source).unwrap();
    let picture: BTreeSet<usize> = index.picture_ids().unwrap().into_iter().collect();
    let normal: BTreeSet<usize> = index.ids_named(["normal.R", "normal.G"]).into_iter().collect();
    store.get(path, &source, &index, &Want { all: false, ids: picture.clone() }, Use::Other).unwrap();
    let b = store.get(path, &source, &index, &Want { all: false, ids: normal.clone() }, Use::Other).unwrap();
    let d = decodes_of(path);
    assert_eq!(d.len(), 2);
    assert_eq!(d[1].channels, ["normal.G", "normal.R"], "only the missing channels");
    assert!(picture.iter().chain(&normal).all(|id| b.plane(*id).is_some()), "both");
    // Covered now: no decode.
    store.get(path, &source, &index, &Want { all: false, ids: picture }, Use::Other).unwrap();
    assert_eq!(decodes_of(path).len(), 2);
}

/// #474: the store is bounded by bytes, least recently used out first; the newest file stays
/// even when it alone is over the budget, and unreadable files are remembered without bytes
/// (not read again) until forgotten.
#[test]
fn the_store_evicts_least_recently_used_by_bytes() {
    let bytes: Arc<[u8]> = write(vec![part(None, &RGBA, (32, 32), Compression::ZIP1)], (32, 32)).into();
    let one = 4 * (32 * 32 * 4 + 1);
    let store = PlaneStore::new(2 * one);
    let all = Want { all: true, ids: Default::default() };
    let get = |p: &str| {
        let source = ExrSource::Bytes(bytes.clone());
        let index = crate::exr_index::get(p, &source).unwrap();
        store.get(p, &source, &index, &all, Use::Other).map(|_| ())
    };
    get("/3271-evict-a.exr").unwrap();
    get("/3271-evict-b.exr").unwrap();
    get("/3271-evict-a.exr").unwrap(); // a is now more recent than b
    get("/3271-evict-c.exr").unwrap();
    assert_eq!(store.cached(), ["/3271-evict-a.exr", "/3271-evict-c.exr"], "b was least recently used");
    let small = PlaneStore::new(one / 2);
    for p in ["/3271-evict-big.exr", "/3271-evict-big2.exr"] {
        let source = ExrSource::Bytes(bytes.clone());
        let index = crate::exr_index::get(p, &source).unwrap();
        assert!(small.get(p, &source, &index, &all, Use::Other).is_ok());
        assert_eq!(small.cached(), [p], "the newest stays");
    }
    // An unreadable file (its chunk table points past the end): remembered, not read again.
    let mut broken = bytes.to_vec();
    broken.truncate(broken.len() - 64);
    let source = ExrSource::Bytes(broken.into());
    let path = "/3271-evict-broken.exr";
    let index = crate::exr_index::get(path, &source).unwrap();
    assert!(store.get(path, &source, &index, &all, Use::Other).is_err());
    assert!(store.get(path, &source, &index, &all, Use::Other).is_err());
    assert_eq!(decodes_of(path).len(), 1, "not read again");
    store.forget(path);
    assert!(!store.cached().contains(&path.to_string()));
}

/// #474 / #3191: a file forgotten (Reload Footage) while it decodes keeps nothing of that decode,
/// and the request asks again: no planes made before the reload are kept or served.
#[test]
fn a_file_forgotten_while_it_decodes_is_not_kept() {
    let bytes: Arc<[u8]> = write(vec![part(None, &RGBA, (8, 8), Compression::ZIP1)], (8, 8)).into();
    let path = "/3271-forgotten.exr";
    let source = ExrSource::Bytes(bytes);
    let store = Arc::new(PlaneStore::new(CACHE_BUDGET));
    let index = crate::exr_index::get(path, &source).unwrap();
    let forgot = Arc::new(AtomicU64::new(0));
    {
        let (store, forgot) = (store.clone(), forgot.clone());
        set_before_read(
            path,
            Some(Arc::new(move |p: &str| {
                if p == "/3271-forgotten.exr" && forgot.fetch_add(1, Ordering::SeqCst) == 0 {
                    store.forget(p);
                }
            })),
        );
    }
    let got = store.get(path, &source, &index, &Want { all: true, ids: Default::default() }, Use::Other);
    set_before_read(path, None);
    assert!(got.is_ok());
    assert_eq!(decodes_of(path).len(), 2, "decoded again after the reload");
    assert_eq!(store.cached(), [path], "only the second decode is kept");
}

/// #474: the planes of a picture whose effects read the footage's channels are kept for them
/// (the renderer asks for the picture, then the channels) until the channels are asked for.
#[test]
fn a_pictures_planes_are_held_for_its_effects() {
    let bytes: Arc<[u8]> = single_part_zips().into();
    let path = "/3271-held.exr";
    let source = ExrSource::Bytes(bytes);
    let store = PlaneStore::new(1);
    let index = crate::exr_index::get(path, &source).unwrap();
    let all = Want { all: true, ids: Default::default() };
    drop(store.get(path, &source, &index, &all, Use::Picture).unwrap());
    assert!(store.is_held(path));
    drop(store.get(path, &source, &index, &all, Use::Other).unwrap());
    assert!(!store.is_held(path), "the channels were asked for");
    assert_eq!(decodes_of(path).len(), 1);
}

/// #474: footage the project says effects read the channels of decodes once from its first frame
/// (the picture with the channels). Learned only when `aux()` first asks, the picture was read
/// alone before, and the channels then read only what it didn't.
#[test]
fn given_aux_footage_decodes_once_from_the_first_frame() {
    use crate::exr_channels::take_decodes;
    let bytes: Arc<[u8]> = single_part_zips().into();
    let (given, learned) = ("/3271-given.exr", "/3271-learned.exr");
    let pool = crate::MediaPool::new();
    let mut f = std::collections::HashMap::new();
    for p in [given, learned] {
        pool.add_bytes(p, bytes.clone());
        f.insert(p, crate::probe_bytes(p, bytes.clone()).unwrap());
    }
    pool.set_aux_footage(&[given.to_string()]);
    take_decodes();
    pool.frame_at(&f[given], Tick::ZERO).unwrap();
    assert!(pool.aux(ItemId(1), &f[given], Tick::ZERO).unwrap().get("diffuse.R").is_some());
    assert_eq!(take_decodes(), [0, 1], "the picture and the channels share one decode");
    pool.frame_at(&f[learned], Tick::ZERO).unwrap();
    pool.aux(ItemId(2), &f[learned], Tick::ZERO).unwrap();
    assert_eq!(take_decodes(), [0, 2]);
    let d = decodes_of(learned);
    assert_eq!(d[0].channels.len(), 4, "the picture alone");
    assert_eq!(d[1].channels.len(), crate::exr_index::cached(learned).unwrap().channels.len(), "all: a picture read alone isn't kept twice");
}

/// #474: the picture made of the planes of a file with exact R, G, B channels is the `image`
/// crate's, bit for bit (first such part, alpha or opaque, data window placed with 0 around it),
/// in every alpha interpretation and float curve.
#[test]
fn picture_from_planes_is_the_image_crates() {
    use effectcraft_project::AlphaMode;
    let display = (5usize, 4usize);
    let files = [
        write(vec![part(None, &[("R", true), ("G", true), ("B", true), ("A", false), ("Z", false)], display, Compression::ZIP16)], display),
        write(vec![part(None, &[("R", false), ("G", false), ("B", false), ("Image.R", true)], display, Compression::RLE)], display),
        write(
            vec![
                part(Some("diffuse"), &[("R", true), ("G", true), ("B", true)], display, Compression::ZIP1),
                part(Some("beauty"), &RGBA, display, Compression::PIZ),
            ],
            display,
        ),
    ];
    for (k, bytes) in files.iter().enumerate() {
        let path = format!("/3271-planes-{k}.exr");
        let source = ExrSource::Bytes(bytes.as_slice().into());
        assert!(matches!(crate::exr_index::get(&path, &source).unwrap().rgb, Rgb::Planes(..)), "file {k}");
        let img = crate::probe::load_oriented(bytes, Some(image::ImageFormat::OpenExr)).unwrap();
        for alpha in [AlphaMode::Straight, AlphaMode::Premultiplied, AlphaMode::Ignore] {
            for curve in [FloatCurve::Srgb, FloatCurve::Linear, FloatCurve::Gamma24] {
                let op = AlphaOp::of(&Footage { alpha, premul_color: [0.2, 0.4, 0.6], ..Default::default() }, curve);
                let want = crate::convert::dynamic_to_image(&img, op);
                let Picture::Done(Ok(got)) = picture(&path, &source, op, None) else { panic!("file {k}: a picture") };
                assert_eq!(bits(&got), bits(&want), "file {k} {alpha:?} {curve:?}");
            }
        }
    }
}

/// #474 / #3271: a decode that stops (a panic, on the decode threads or on the thread asking)
/// leaves nothing behind: the entry isn't "decoding" or "unreadable", the next request decodes.
#[test]
fn a_decode_that_panics_is_not_left_in_the_store() {
    let bytes: Arc<[u8]> = write(vec![part(None, &RGBA, (8, 8), Compression::ZIP1)], (8, 8)).into();
    let all = Want { all: true, ids: Default::default() };
    for (k, inline) in [false, true].into_iter().enumerate() {
        let path = format!("/3271-panics-{k}.exr");
        let source = ExrSource::Bytes(bytes.clone());
        let store = PlaneStore::new(CACHE_BUDGET);
        let index = crate::exr_index::get(&path, &source).unwrap();
        let panicked = Arc::new(AtomicU64::new(0));
        {
            let (watch, panicked) = (path.clone(), panicked.clone());
            set_before_read(
                &path,
                Some(Arc::new(move |p: &str| {
                    if p == watch && panicked.fetch_add(1, Ordering::SeqCst) == 0 {
                        panic!("a decoder bug");
                    }
                })),
            );
        }
        let ask = || store.get(&path, &source, &index, &all, Use::Other);
        if inline {
            // On a decode thread the decode runs right there, and the panic comes through.
            let threads = crate::pool::decode_threads().unwrap();
            let r = threads.install(|| std::panic::catch_unwind(std::panic::AssertUnwindSafe(ask)));
            assert!(r.is_err(), "the panic came through");
            assert!(store.cached().is_empty(), "nothing left");
            assert!(ask().is_ok(), "decodes again");
        } else {
            // Handed to the decode threads: the request asks again by itself.
            assert!(ask().is_ok());
        }
        set_before_read(&path, None);
        assert_eq!(panicked.load(Ordering::SeqCst), 2, "inline: {inline}");
        assert_eq!(store.cached(), std::slice::from_ref(&path));
    }
}

/// #474: a file counts as used when its decode finishes: one asked for first but decoded after
/// another is the newer of the two, so concurrent frames don't evict each other's files before
/// their effects read them.
#[test]
fn a_file_is_used_as_of_its_decode() {
    let bytes: Arc<[u8]> = write(vec![part(None, &RGBA, (32, 32), Compression::ZIP1)], (32, 32)).into();
    let one = 4 * (32 * 32 * 4 + 1);
    let store = Arc::new(PlaneStore::new(2 * one));
    let all = Want { all: true, ids: Default::default() };
    let get = {
        let (store, bytes, all) = (store.clone(), bytes.clone(), all.clone());
        move |p: &str| {
            let source = ExrSource::Bytes(bytes.clone());
            let index = crate::exr_index::get(p, &source).unwrap();
            store.get(p, &source, &index, &all, Use::Other).map(|_| ())
        }
    };
    {
        let get = get.clone();
        let nested = Arc::new(AtomicU64::new(0));
        set_before_read(
            "/3271-used-first.exr",
            Some(Arc::new(move |p: &str| {
                // Another frame's file is asked for and decoded while this one decodes.
                if p == "/3271-used-first.exr" && nested.fetch_add(1, Ordering::SeqCst) == 0 {
                    get("/3271-used-second.exr").unwrap();
                }
            })),
        );
    }
    get("/3271-used-first.exr").unwrap();
    set_before_read("/3271-used-first.exr", None);
    get("/3271-used-third.exr").unwrap();
    assert_eq!(store.cached(), ["/3271-used-first.exr", "/3271-used-third.exr"], "the file decoded first went first");
}

/// #474: frames rendered at once (each asks for its picture, then its effects for the channels
/// while other frames decode) decode each file once, though the budget holds only one: a
/// picture's planes are kept for its effects, and planes in use are never dropped.
#[test]
fn concurrent_frames_decode_each_file_once() {
    let rgbz = [("R", false), ("G", false), ("B", false), ("Z", false)];
    let bytes: Arc<[u8]> = write(vec![part(None, &rgbz, (64, 64), Compression::ZIP1)], (64, 64)).into();
    let store = PlaneStore::new(4 * (64 * 64 * 4 + 1));
    let all = Want { all: true, ids: Default::default() };
    let (next, frames) = (std::sync::atomic::AtomicUsize::new(0), 24);
    std::thread::scope(|s| {
        for _ in 0..4 {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= frames {
                        break;
                    }
                    let path = format!("/3271-frames/{i:04}.exr");
                    let source = ExrSource::Bytes(bytes.clone());
                    let index = crate::exr_index::get(&path, &source).unwrap();
                    drop(store.get(&path, &source, &index, &all, Use::Picture).unwrap());
                    std::thread::sleep(std::time::Duration::from_millis(3));
                    let b = store.get(&path, &source, &index, &all, Use::Other).unwrap();
                    std::thread::sleep(std::time::Duration::from_millis(2));
                    assert_eq!(b.aux.get("Z").and_then(|z| z.first()).copied(), Some(value(0, 3, false)));
                }
            });
        }
    });
    for i in 0..frames {
        assert_eq!(decodes_of(&format!("/3271-frames/{i:04}.exr")).len(), 1, "frame {i}: one decode");
    }
}

/// #474: once the project no longer names footage (its effect removed), later frames decode
/// only their picture's channels again; what `aux()` learned is dropped with each new set too.
#[test]
fn a_new_aux_footage_set_returns_to_picture_only() {
    let bytes: Arc<[u8]> = single_part_zips().into();
    let paths: Vec<String> = (1..=4).map(|i| format!("/3271-unset_{i:04}.exr")).collect();
    let pool = crate::MediaPool::new();
    for p in &paths {
        pool.add_bytes(p, bytes.clone());
    }
    let mut f = crate::probe_bytes(&paths[0], bytes.clone()).unwrap();
    f.kind = effectcraft_project::FootageKind::Sequence;
    f.sequence = paths.clone();
    let at = |i: i64| f.frame_rate.tick_of(i);
    let all = crate::exr_index::get(&paths[0], &ExrSource::Bytes(bytes.clone())).unwrap().channels.len();
    // The channels the frame's decode read.
    let frame = |i: usize| {
        pool.frame_at(&f, at(i as i64)).unwrap();
        decodes_of(&paths[i]).first().map(|d| d.channels.len())
    };
    pool.set_aux_footage(std::slice::from_ref(&f.path));
    assert_eq!(frame(0), Some(all), "given: the picture with the channels");
    pool.set_aux_footage(&[]);
    assert!(!shared().is_held(&paths[0]), "no longer kept once no effect reads it");
    assert_eq!(frame(1), Some(4), "no longer given: the picture alone");
    // Learned by `aux()`, then a new set without it: picture only again.
    pool.aux(ItemId(1), &f, at(1)).unwrap();
    assert_eq!(frame(2), Some(all), "learned");
    pool.set_aux_footage(&[]);
    assert_eq!(frame(3), Some(4), "a new set drops what was learned");
}

/// [`part`] with its data window at `pos`.
fn part_at(name: Option<&str>, channels: &[(&str, bool)], size: (usize, usize), pos: (i32, i32), compression: Compression) -> Layer<AnyChannels<FlatSamples>> {
    let mut p = part(name, channels, size, compression);
    p.attributes.layer_position = exr::prelude::Vec2(pos.0, pos.1);
    p
}

/// The bytes of a file of `parts` over the display window `display`.
fn write_in(parts: Vec<Layer<AnyChannels<FlatSamples>>>, display: IntegerBounds) -> Vec<u8> {
    let image = exr::prelude::Image::from_layers(ImageAttributes::new(display), parts);
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write().non_parallel().to_buffered(&mut bytes).unwrap();
    bytes.into_inner()
}

/// #474 review: data windows placed inside and around the display window make the `image`
/// crate's picture too.
#[test]
fn picture_from_planes_with_data_window_offsets_is_the_image_crates() {
    let display = IntegerBounds::new((1, -1), (5, 4));
    let files = [
        write_in(vec![part_at(None, &[("R", true), ("G", true), ("B", true), ("A", false), ("Z", false)], (5, 4), (1, -1), Compression::ZIP16)], display),
        write_in(vec![part_at(None, &[("R", false), ("G", false), ("B", false), ("Image.R", true)], (3, 2), (0, 0), Compression::RLE)], display),
        write_in(
            vec![
                part_at(Some("diffuse"), &[("R", true), ("G", true), ("B", true)], (5, 4), (1, -1), Compression::ZIP1),
                part_at(Some("beauty"), &RGBA, (2, 2), (2, 0), Compression::PIZ),
            ],
            display,
        ),
    ];
    for (k, bytes) in files.iter().enumerate() {
        let path = format!("/3271-offsets-{k}.exr");
        let source = ExrSource::Bytes(bytes.as_slice().into());
        assert!(matches!(crate::exr_index::get(&path, &source).unwrap().rgb, Rgb::Planes(..)), "file {k}");
        let f = Footage::default();
        let Picture::Done(Ok(got)) = picture(&path, &source, AlphaOp::of(&f, FloatCurve::Linear), None) else { panic!("file {k}: a picture") };
        let img = crate::probe::load_oriented(bytes, Some(image::ImageFormat::OpenExr)).unwrap();
        assert_eq!(bits(&got), bits(&crate::convert::dynamic_to_image(&img, AlphaOp::of(&f, FloatCurve::Linear))), "file {k}");
    }
}

/// #3271 review: a request that failed (a broken part) doesn't hide what the store holds, nor
/// fail smaller requests: the picture read before is served, and read again when asked alone.
#[test]
fn a_failed_request_fails_only_requests_as_large() {
    let mut bytes = multipart();
    // The depth part is last: its chunks now point past the end of the file.
    bytes.truncate(bytes.len() - 200);
    let path = "/3271-broken-part.exr";
    let source = ExrSource::Bytes(bytes.into());
    let store = PlaneStore::new(CACHE_BUDGET);
    let index = crate::exr_index::get(path, &source).unwrap();
    let picture = Want { all: false, ids: index.picture_ids().unwrap().into_iter().collect() };
    let all = Want { all: true, ids: Default::default() };
    let held = store.get(path, &source, &index, &picture, Use::Picture).unwrap();
    assert_eq!(store.get(path, &source, &index, &all, Use::Other).err(), Some(Refused::Unreadable));
    assert!(Arc::ptr_eq(&store.get(path, &source, &index, &picture, Use::Other).unwrap(), &held), "served from the store");
    assert_eq!(store.get(path, &source, &index, &all, Use::Other).err(), Some(Refused::Unreadable), "remembered");
    assert_eq!(decodes_of(path).len(), 2);
    // Another store: the picture alone after the failure decodes.
    let other = PlaneStore::new(CACHE_BUDGET);
    assert!(other.get(path, &source, &index, &all, Use::Other).is_err());
    assert!(other.get(path, &source, &index, &picture, Use::Other).is_ok(), "a smaller request isn't refused");
}

/// #3271 review: when the picture with the channels its effects read is more than one file may
/// allocate, the picture is read alone instead of failing.
#[test]
fn a_picture_too_large_with_its_channels_is_read_alone() {
    let bytes: Arc<[u8]> = single_part_zips().into();
    let path = "/3271-too-large-with-aux.exr";
    let pool = crate::MediaPool::new();
    pool.add_bytes(path, bytes.clone());
    let f = crate::probe_bytes(path, bytes.clone()).unwrap();
    // Room for the picture's 4 planes, not for all of them.
    set_cap(path, 5 * SIZE.0 * SIZE.1 * 4);
    pool.set_aux_footage(&[path.to_string()]);
    let img = pool.frame_at(&f, Tick::ZERO).unwrap();
    assert_eq!(bits(&img), bits(&image_crates(&bytes, &f)));
    let d = decodes_of(path);
    assert_eq!(d.last().map(|d| d.channels.len()), Some(4), "{d:?}");
}

/// #474: at most CACHE_FILES entries are kept, however small.
#[test]
fn the_store_keeps_a_bounded_number_of_files() {
    let bytes: Arc<[u8]> = write(vec![part(None, &[("R", true)], (2, 2), Compression::ZIP1)], (2, 2)).into();
    let store = PlaneStore::new(CACHE_BUDGET);
    let all = Want { all: true, ids: Default::default() };
    for i in 0..CACHE_FILES + 10 {
        let path = format!("/3271-many/{i}.exr");
        let source = ExrSource::Bytes(bytes.clone());
        let index = crate::exr_index::get(&path, &source).unwrap();
        store.get(&path, &source, &index, &all, Use::Other).unwrap();
    }
    assert!(store.cached().len() <= CACHE_FILES);
}

/// `bytes` with the data window's right edge (`xMax`) moved to `x_max`.
fn with_data_window_x_max(mut bytes: Vec<u8>, x_max: i32) -> Vec<u8> {
    let tag = b"dataWindow\0box2i\0\x10\0\0\0";
    let at = bytes.windows(tag.len()).position(|w| w == tag).unwrap() + tag.len() + 8;
    bytes[at..at + 4].copy_from_slice(&x_max.to_le_bytes());
    bytes
}

/// #3271 review (never crash): a tiny file whose data window is a billion pixels wide over one
/// chunk made the decompressor reserve gigabytes for a block (an abort, not an error). Every
/// way in refuses it with an error: import, the picture, the channels, the names.
#[test]
fn a_hostile_data_window_is_refused_not_allocated() {
    for compression in [Compression::PIZ, Compression::ZIP1, Compression::B44] {
        let small = write(vec![part(None, &[("R", true)], (1, 1), compression)], (1, 1));
        let bytes = with_data_window_x_max(small, (1 << 30) - 3);
        let path = format!("/3271-hostile-{compression:?}.exr");
        assert!(!crate::exr_index::safe_to_decode(&bytes));
        assert!(crate::exr_index::build(&mut std::io::Cursor::new(bytes.as_slice()), bytes.len() as u64, FileStamp::Virtual).is_err());
        assert!(crate::exr_channels::read_exr_channels(&bytes).is_none());
        assert!(crate::exr_channels::layered_image(&bytes).is_none());
        let b: Arc<[u8]> = bytes.into();
        assert!(crate::probe_bytes(&path, b.clone()).is_err(), "import refuses it");
        let pool = crate::MediaPool::new();
        pool.add_bytes(&path, b);
        let f = Footage { path: path.clone(), kind: effectcraft_project::FootageKind::Still, has_video: true, width: 1, height: 1, ..Default::default() };
        assert!(pool.frame_at(&f, Tick::ZERO).is_err());
        assert!(pool.aux(ItemId(1), &f, Tick::ZERO).is_none());
        assert!(pool.aux_names(ItemId(1), &f, Tick::ZERO).is_none());
    }
}

/// #3271 review (never crash): damaged files (bytes changed in the offset table, the chunk
/// headers or the pixels) read as an error or as some pixels, never a panic.
#[test]
fn damaged_files_never_panic() {
    let files = [single_part_zips(), multipart(), write(vec![part(None, &RGBA, (16, 40), Compression::PIZ)], (16, 40))];
    for good in &files {
        let start = crate::exr_index::build(&mut std::io::Cursor::new(good.as_slice()), good.len() as u64, FileStamp::Virtual)
            .unwrap()
            .parts
            .iter()
            .flat_map(|p| p.chunks.iter().map(|c| c.offset))
            .min()
            .unwrap() as usize;
        // From a little before the chunks (the offset tables) to the end, in steps.
        let from = start.saturating_sub(64);
        for (i, at) in (from..good.len()).step_by(((good.len() - from) / 150).max(1)).enumerate() {
            let mut bad = good.clone();
            bad[at] ^= [0xff, 0x80, 0x01, 0x7f][i % 4];
            let _ = crate::exr_channels::read_exr_channels(&bad);
            let _ = crate::exr_channels::layered_image(&bad);
            let mut src = std::io::Cursor::new(bad.as_slice());
            if let Ok(ix) = crate::exr_index::build(&mut src, bad.len() as u64, FileStamp::Virtual) {
                let req = ReadRequest { rows: Some(3..9), level: 1, ..ReadRequest::full((0..ix.channels.len()).collect()) };
                let _ = crate::exr_read::read_planes(&mut src, &ix, &req);
            }
        }
    }
}

/// #3271 review: rows and levels on every compression: the rows read are the full read's,
/// whole chunks are decompressed (1, 16, 32 or 256 lines), and a level is the full read's
/// every other row and column.
#[test]
fn rows_and_levels_on_every_compression() {
    let cases = [
        (Compression::Uncompressed, 1),
        (Compression::RLE, 1),
        (Compression::ZIP1, 1),
        (Compression::ZIP16, 16),
        (Compression::PXR24, 16),
        (Compression::PIZ, 32),
        (Compression::B44, 32),
        (Compression::DWAA(None), 32),
        (Compression::DWAB(None), 256),
    ];
    for (compression, per_chunk) in cases {
        let bytes = write(vec![part(None, &RGBA, (32, 64), compression)], (32, 64));
        let (ix, mut src) = indexed(&bytes);
        let all = ReadRequest::full(vec![0, 1, 2, 3]);
        let full = crate::exr_read::read_planes(&mut src, &ix, &all).0.unwrap();
        let (band, stats) = crate::exr_read::read_planes(&mut src, &ix, &ReadRequest { rows: Some(10..20), ..all.clone() });
        let band = band.unwrap();
        // The chunks holding rows 10..20, each `per_chunk` lines (the last one shorter).
        let lines: usize = (0..64usize).step_by(per_chunk).filter(|y0| *y0 < 20 && y0 + per_chunk > 10).map(|y0| per_chunk.min(64 - y0)).sum();
        assert_eq!(stats.lines, lines, "{compression:?}");
        for (b, f) in band.iter().zip(&full) {
            assert_eq!(
                b[10 * 32..20 * 32].iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                f[10 * 32..20 * 32].iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                "{compression:?}"
            );
        }
        let (half, _) = crate::exr_read::read_planes(&mut src, &ix, &ReadRequest { level: 1, ..all.clone() });
        let half = half.unwrap();
        for y in 0..32 {
            for x in 0..16 {
                assert_eq!(half[0][y * 16 + x].to_bits(), full[0][y * 2 * 32 + x * 2].to_bits(), "{compression:?} ({x}, {y})");
            }
        }
    }
    // Tiles: rows read as in the full read.
    let mut tiled = part(None, &RGBA, (32, 64), Compression::ZIP16);
    tiled.encoding.blocks = Blocks::Tiles(exr::prelude::Vec2(16, 16));
    let bytes = write(vec![tiled], (32, 64));
    let (ix, mut src) = indexed(&bytes);
    let all = ReadRequest::full(vec![0, 1, 2, 3]);
    let full = crate::exr_read::read_planes(&mut src, &ix, &all).0.unwrap();
    let (band, stats) = crate::exr_read::read_planes(&mut src, &ix, &ReadRequest { rows: Some(10..20), ..all.clone() });
    assert_eq!(stats.chunks, [4], "two rows of two tiles");
    assert_eq!(band.unwrap()[0][10 * 32..20 * 32], full[0][10 * 32..20 * 32]);
}

/// #3271: a new frame of an OpenEXR on disk opens its file once: indexing it and decoding the
/// picture share the open file.
#[test]
fn a_new_frame_opens_its_file_once() {
    let dir = std::env::temp_dir().join(format!("ec-3271-open-once-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("plate_0001.exr");
    std::fs::write(&file, multipart()).unwrap();
    let f = crate::probe(&file).unwrap();
    let before = crate::opens_under(&dir);
    let pool = crate::MediaPool::new();
    pool.frame_at(&f, Tick::ZERO).unwrap();
    assert_eq!(crate::opens_under(&dir) - before, 1, "one open for the index and the decode");
    assert_eq!(decodes_of(&f.path).len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// #3271: decoded channels are kept in the disk cache: once RAM has let them go they are read
/// back from there, bit for bit (Cryptomatte IDs included), instead of decoding the file; Reload
/// Footage drops them.
#[test]
fn decoded_planes_are_kept_on_disk_and_read_back_exactly() {
    use effectcraft_render::disk_cache::{DiskCache, Kind};
    let dir = std::env::temp_dir().join(format!("ec-3271-disk-planes-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("crypto_0001.exr");
    std::fs::write(&file, multipart()).unwrap();
    let dc = DiskCache::open(dir.join("cache"), 1 << 30).unwrap();
    set_disk_cache(Some(&dc));
    let f = crate::probe(&file).unwrap();
    let pool = crate::MediaPool::new();
    let first = pool.aux(ItemId(1), &f, Tick::ZERO).unwrap();
    dc.flush();
    assert_eq!(dc.keys(Kind::Plane).len(), 1, "{:?}", dc.stats());
    forget(&f.path);
    let again = pool.aux(ItemId(1), &f, Tick::ZERO).unwrap();
    let bits = |a: &AuxChannels| a.channels.iter().map(|(n, p)| (n.clone(), p.iter().map(|v| v.to_bits()).collect::<Vec<_>>())).collect::<Vec<_>>();
    assert_eq!(bits(&again), bits(&first), "bit for bit");
    // Reload Footage: the entry goes, the file is decoded again.
    dc.drop_files([f.path.clone()]);
    dc.flush();
    forget(&f.path);
    pool.aux(ItemId(1), &f, Tick::ZERO).unwrap();
    set_disk_cache(None);
    let from_disk: Vec<bool> = decodes_of(&f.path).iter().map(|d| d.from_disk).collect();
    assert_eq!(from_disk, [false, true, false]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// #3271: a decode thread asking for planes whose decode is still queued on the decode threads
/// (an aux request on a frame worker queued it, then the picture's decode asks) takes it over
/// instead of decoding a copy: one decode, and both requests get its planes.
#[test]
fn a_decode_thread_takes_over_a_queued_decode() {
    let bytes: Arc<[u8]> = single_part_zips().into();
    let path = "/3271-claimed.exr";
    let source = ExrSource::Bytes(bytes);
    let store = Arc::new(PlaneStore::new(CACHE_BUDGET));
    let index = crate::exr_index::get(path, &source).unwrap();
    let all = Want { all: true, ids: Default::default() };
    let threads = crate::pool::decode_threads().unwrap();
    let n = threads.current_num_threads();
    // Every decode thread busy: n - 1 hold until released, one waits for the queued decode.
    let (release, held) = std::sync::mpsc::channel::<()>();
    let held = Arc::new(Mutex::new(held));
    for _ in 1..n {
        let held = held.clone();
        threads.spawn(move || {
            let _ = held.lock().unwrap().recv_timeout(std::time::Duration::from_secs(20));
        });
    }
    let (tx, rx) = std::sync::mpsc::channel();
    {
        let (store, source, index, all, tx) = (store.clone(), source.clone(), index.clone(), all.clone(), tx.clone());
        threads.spawn(move || {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !store.in_flight(path) && std::time::Instant::now() < until {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            let _ = tx.send(store.get(path, &source, &index, &all, Use::Other).map(|b| b.ids.len()));
        });
    }
    // The other request, from a thread of its own: its decode is queued behind them.
    let asker = {
        let (store, source, index, all) = (store.clone(), source.clone(), index.clone(), all.clone());
        std::thread::spawn(move || store.get(path, &source, &index, &all, Use::Other).map(|b| b.ids.len()))
    };
    let claimed = rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap();
    let asked = asker.join().unwrap();
    for _ in 1..n {
        let _ = release.send(());
    }
    assert!(claimed.is_ok() && claimed == asked, "{claimed:?} {asked:?}");
    assert_eq!(decodes_of(path).len(), 1, "decoded once");
}

/// #3271: the plane store in the common memory budget: trimmed least recently used first,
/// never the planes a picture holds for its effects or planes still in use.
#[test]
fn the_plane_store_is_trimmed_by_the_accountant() {
    use effectcraft_render::memory::{Accountant, MemoryHolder};
    struct Local(Arc<PlaneStore>);
    impl MemoryHolder for Local {
        fn name(&self) -> &'static str {
            "planes"
        }
        fn bytes(&self) -> usize {
            self.0.lock().entries.iter().map(|e| e.bytes).sum()
        }
        fn priority(&self) -> u8 {
            1
        }
        fn trim_to(&self, bytes: usize) {
            self.0.trim_to(bytes);
        }
    }
    let bytes: Arc<[u8]> = write(vec![part(None, &RGBA, (32, 32), Compression::ZIP1)], (32, 32)).into();
    let store = Arc::new(PlaneStore::new(CACHE_BUDGET));
    let all = Want { all: true, ids: Default::default() };
    let get = |p: &str, by: Use| {
        let source = ExrSource::Bytes(bytes.clone());
        let index = crate::exr_index::get(p, &source).unwrap();
        store.get(p, &source, &index, &all, by).unwrap()
    };
    drop(get("/3271-acc-a.exr", Use::Other));
    drop(get("/3271-acc-b.exr", Use::Picture));
    let in_use = get("/3271-acc-c.exr", Use::Other);
    let acc = Accountant::new(1);
    let holder: Arc<dyn MemoryHolder> = Arc::new(Local(store.clone()));
    acc.register(&holder);
    acc.balance();
    assert_eq!(store.cached(), ["/3271-acc-b.exr", "/3271-acc-c.exr"], "held for effects, and in use");
    drop(in_use);
}
