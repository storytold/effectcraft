//! The OpenEXR index, reader and plane store (#3271) on synthetic files written here.

use std::sync::atomic::{AtomicU64, Ordering};

use effectcraft_project::{Footage, ItemId};
use effectcraft_raster::channels3d::{AuxLookup, AuxNeed};
use effectcraft_render::reads::FileStamp;
use effectcraft_render::{FloatCurve, FootageSource};
use effectcraft_time::Tick;
use exr::prelude::{AnyChannel, AnyChannels, Blocks, Compression, Encoding, FlatSamples, ImageAttributes, IntegerBounds, Layer, LayerAttributes, LineOrder, WritableImage, f16};

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
        attrs.other.insert(exr::prelude::Text::from("cryptomatte/abc1234/name"), exr::prelude::AttributeValue::Text(exr::prelude::Text::from("Crypto_Material")));
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
const CRYPTO: [(&str, bool); 4] = [("Crypto_Material00.R", false), ("Crypto_Material00.G", false), ("Crypto_Material00.B", false), ("Crypto_Material00.A", false)];

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
    fn path(&self) -> String {
        self.0.to_string_lossy().to_string()
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
    let path = file.path();
    let f = crate::probe(&file.0).unwrap();
    let pool = crate::MediaPool::new();
    let img = pool.frame_at(&f, Tick::ZERO).unwrap();
    assert_eq!(bits(&img), bits(&image_crates(&bytes, &f)), "the image crate's picture");
    let d = decodes_of(&path);
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0].channels, ["beauty.A", "beauty.B", "beauty.G", "beauty.R"]);
    assert_eq!(d[0].stats.chunks, [SIZE.1, 0, 0], "only the beauty part's chunks");
    assert!(d[0].stats.bytes * 2 < bytes.len() as u64, "{} of {} bytes", d[0].stats.bytes, bytes.len());
}

/// #3271: EXtractoR reading 3 channels of a multi-part file reads only their part and converts
/// only those channels; the stored names find them (`Crypto_Material00.R` in part
/// `Crypto_Material`), and the manifest and layer resolve as the file names them.
#[test]
fn three_aux_channels_read_only_their_part() {
    let bytes = multipart();
    let file = Temp::new("aux-part", &bytes);
    let path = file.path();
    let f = crate::probe(&file.0).unwrap();
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
    let path = file.path();
    let f = crate::probe(&file.0).unwrap();
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
        set_before_read(Some(Arc::new(move |p: &str| {
            if p == "/3271-forgotten.exr" && forgot.fetch_add(1, Ordering::SeqCst) == 0 {
                store.forget(p);
            }
        })));
    }
    let got = store.get(path, &source, &index, &Want { all: true, ids: Default::default() }, Use::Other);
    set_before_read(None);
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
    assert_eq!(d[1].channels.len() + 4, crate::exr_index::cached(learned).unwrap().channels.len(), "the rest");
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
            vec![part(Some("diffuse"), &[("R", true), ("G", true), ("B", true)], display, Compression::ZIP1), part(Some("beauty"), &RGBA, display, Compression::PIZ)],
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
