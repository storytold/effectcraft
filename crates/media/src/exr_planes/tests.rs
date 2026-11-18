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
use crate::exr_read::{ExrSource, LIMIT_TESTS};

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

/// Each part's chunks of the OpenEXR `bytes`, in offset-table order: where each starts and how
/// many bytes it takes (up to the next chunk, or the end of the file). Read with the `exr` crate:
/// `craft-exr`'s index keeps them to itself.
fn chunk_table(bytes: &[u8]) -> Vec<Vec<(u64, u64)>> {
    let mut src = bytes;
    let meta = exr::meta::MetaData::read_from_buffered(&mut src, false).unwrap();
    let tables = exr::meta::MetaData::read_offset_tables(&mut exr::io::PeekRead::new(&mut src), &meta.headers).unwrap();
    let mut starts: Vec<u64> = tables.iter().flatten().copied().collect();
    starts.sort_unstable();
    let end = |at: u64| starts.iter().copied().find(|s| *s > at).unwrap_or(bytes.len() as u64);
    tables.iter().map(|t| t.iter().map(|at| (*at, end(*at) - at)).collect()).collect()
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
    let beauty: u64 = chunk_table(&bytes)[0].iter().map(|(_, len)| len).sum();
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

/// The main picture is the first part with unprefixed `R`, `G`, `B`: a depth part before it
/// isn't the picture (nor read for it), the second part's RGBA is.
#[test]
fn a_depth_part_first_leaves_the_picture_to_the_rgba_part() {
    let bytes = write(vec![part(Some("depth"), &[("Z", false)], SIZE, Compression::ZIP1), part(Some("beauty"), &RGBA, SIZE, Compression::ZIP1)], SIZE);
    let file = Temp::new("depth-first", &bytes);
    let f = crate::probe(&file.0).unwrap();
    assert_eq!((f.width, f.height), (SIZE.0 as u32, SIZE.1 as u32));
    let index = crate::exr_index::get(&f.path, &ExrSource::Bytes(bytes.as_slice().into())).unwrap();
    let names: Vec<&str> = index.picture.as_ref().unwrap().ids.iter().map(|i| index.channels[*i].name.as_str()).collect();
    assert_eq!(names, ["beauty.R", "beauty.G", "beauty.B", "beauty.A"]);
    let img = crate::MediaPool::new().frame_at(&f, Tick::ZERO).unwrap();
    assert_eq!(bits(&img), bits(&image_crates(&bytes, &f)), "the RGBA part, as the image crate shows it");
    let d = decodes_of(&f.path);
    assert_eq!(d.last().unwrap().stats.chunks, [0, SIZE.1], "only the RGBA part's chunks");
}

/// The black picture of a file without a main picture is allocated whole like any picture: one
/// larger than one read of the file may allocate is refused with the reason, not allocated (an
/// abort in the browser); within it, it is black at the file's size.
#[test]
fn a_black_picture_too_large_is_refused_not_allocated() {
    let bytes = write(vec![part(None, &[("depth.Z", false)], (8, 8), Compression::ZIP1)], (8, 8));
    let path = "/no-picture-too-large.exr";
    let source = ExrSource::Bytes(bytes.as_slice().into());
    let op = AlphaOp::of(&Footage::default(), FloatCurve::Linear);
    set_cap(path, 8 * 8 * 16 - 1);
    match picture(path, &source, op, None) {
        Picture::Done(Err(why)) => assert!(why.contains("8×8") && why.contains("GB"), "{why}"),
        _ => panic!("refused"),
    }
    let fits = "/no-picture-fits.exr";
    set_cap(fits, 8 * 8 * 16);
    let Picture::Done(Ok(img)) = picture(fits, &source, op, None) else { panic!("a black picture") };
    assert_eq!((img.width, img.height), (8, 8));
    assert!(img.data.iter().all(|p| *p == [0.0, 0.0, 0.0, 1.0]));
}

/// A luminance file (`Y` only, no `R`, `G`, `B`) is grey, as before.
#[test]
fn a_luminance_file_is_grey() {
    let bytes = write(vec![part(None, &[("Y", true)], SIZE, Compression::ZIP1)], SIZE);
    let file = Temp::new("luminance", &bytes);
    let f = crate::probe(&file.0).unwrap();
    let path = f.path.clone();
    let source = ExrSource::Bytes(bytes.as_slice().into());
    let Picture::Done(Ok(img)) = picture(&path, &source, AlphaOp::of(&f, FloatCurve::Linear), None) else { panic!("a picture") };
    for (i, px) in img.data.iter().enumerate() {
        let y = f16::from_f32(value(i, 0, false)).to_f32();
        assert_eq!([px[0], px[1], px[2], px[3]], [y, y, y, 1.0], "pixel {i}");
    }
    assert_eq!(
        bits(&crate::MediaPool::new().frame_at(&f, Tick::ZERO).unwrap()),
        bits(&crate::convert::dynamic_to_image(&crate::exr_channels::picture_image(&bytes).unwrap(), AlphaOp::of(&f, FloatCurve::Srgb)))
    );
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
    let ix = crate::exr_index::build(&mut src, FileStamp::Virtual).unwrap();
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
                    store.get(path, &source, &index, &Want { all: true, ids: (0..4).collect() }, Use::Other).unwrap()
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
    let picture: BTreeSet<usize> = index.picture_ids().into_iter().collect();
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
    let all = Want { all: true, ids: (0..4).collect() };
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

/// #3271 CI: a decode thread that is slow to finish its job after handing the planes over (a
/// busy machine) doesn't make them look in use: the store still drops the least recently used
/// file, not another one.
#[test]
fn a_decode_thread_finishing_late_does_not_pin_the_planes() {
    let bytes: Arc<[u8]> = write(vec![part(None, &RGBA, (32, 32), Compression::ZIP1)], (32, 32)).into();
    let one = 4 * (32 * 32 * 4 + 1);
    let store = PlaneStore::new(2 * one);
    let all = Want { all: true, ids: (0..4).collect() };
    let get = |p: &str| {
        let source = ExrSource::Bytes(bytes.clone());
        let index = crate::exr_index::get(p, &source).unwrap();
        store.get(p, &source, &index, &all, Use::Other).map(|_| ())
    };
    let (a, b, c) = ("/3271-late-a.exr", "/3271-late-b.exr", "/3271-late-c.exr");
    // b's decode thread stays in its job (at most 10 s) after it sent the planes.
    let (go, wait) = std::sync::mpsc::channel::<()>();
    let wait = std::sync::Mutex::new(wait);
    SENT.lock().unwrap().push((
        b.to_string(),
        Arc::new(move |_: &str| {
            let _ = wait.lock().unwrap().recv_timeout(std::time::Duration::from_secs(10));
        }),
    ));
    get(a).unwrap();
    get(b).unwrap();
    get(a).unwrap();
    get(c).unwrap();
    let cached = store.cached();
    let _ = go.send(());
    SENT.lock().unwrap().retain(|(p, _)| p != b);
    assert_eq!(cached, [a, c], "b was least recently used");
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
    let got = store.get(path, &source, &index, &Want { all: true, ids: (0..4).collect() }, Use::Other);
    set_before_read(path, None);
    assert!(got.is_ok());
    assert_eq!(decodes_of(path).len(), 2, "decoded again after the reload");
    assert_eq!(store.cached(), [path], "only the second decode is kept");
}

/// #3270 review (the pool's flights): a request made after a file was forgotten while its decode
/// is in flight doesn't join that decode (it would get the old file's planes): the flight lives
/// in the file's entry, which forget drops, so the request decodes the file anew at once.
#[test]
fn a_request_after_forget_does_not_join_the_old_decode() {
    let bytes: Arc<[u8]> = write(vec![part(None, &RGBA, (8, 8), Compression::ZIP1)], (8, 8)).into();
    let path = "/3271-forget-join.exr";
    let source = ExrSource::Bytes(bytes);
    let store = Arc::new(PlaneStore::new(CACHE_BUDGET));
    let index = crate::exr_index::get(path, &source).unwrap();
    let all = Want { all: true, ids: (0..4).collect() };
    let calls = Arc::new(AtomicU64::new(0));
    let (tx, rx) = std::sync::mpsc::channel();
    {
        let (store, calls, source, index, all) = (store.clone(), calls.clone(), source.clone(), index.clone(), all.clone());
        let tx = std::sync::Mutex::new(Some(tx));
        set_before_read(
            path,
            Some(Arc::new(move |p: &str| {
                if p != "/3271-forget-join.exr" || calls.fetch_add(1, Ordering::SeqCst) > 0 {
                    return;
                }
                // The old decode is in flight (this thread runs it): the file changes.
                store.forget(p);
                let (store, source, index, all) = (store.clone(), source.clone(), index.clone(), all.clone());
                let tx = tx.lock().unwrap().take();
                let asked = std::thread::spawn(move || store.get("/3271-forget-join.exr", &source, &index, &all, Use::Other).map(|_| ()));
                // A request that joined the old decode would wait on this very thread.
                let done = std::time::Instant::now() + std::time::Duration::from_secs(5);
                while !asked.is_finished() && std::time::Instant::now() < done {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                let finished = asked.is_finished();
                if let Some(tx) = tx {
                    let _ = tx.send((finished, if finished { asked.join().ok() } else { None }));
                }
            })),
        );
    }
    let got = store.get(path, &source, &index, &all, Use::Other);
    set_before_read(path, None);
    let (finished, asked) = rx.recv().unwrap();
    assert!(finished, "the request after forget decoded on its own, without waiting for the old decode");
    assert!(asked.is_some_and(|r| r.is_ok()));
    assert!(got.is_ok());
    assert!(decodes_of(path).len() >= 2, "the file was decoded anew after forget");
}

/// #3270 review (CodeRabbit): forgetting one file leaves the decodes of other files alone: a
/// decode of file A in flight while file B is forgotten is still kept and joined (staleness is
/// per file, not one store-wide counter).
#[test]
fn forgetting_another_file_keeps_a_decode_in_flight() {
    let bytes: Arc<[u8]> = write(vec![part(None, &RGBA, (8, 8), Compression::ZIP1)], (8, 8)).into();
    let (a, b) = ("/3271-forget-other-a.exr", "/3271-forget-other-b.exr");
    let store = Arc::new(PlaneStore::new(CACHE_BUDGET));
    let all = Want { all: true, ids: (0..4).collect() };
    let source = ExrSource::Bytes(bytes);
    let index = crate::exr_index::get(a, &source).unwrap();
    // B is in the store first.
    let index_b = crate::exr_index::get(b, &source).unwrap();
    store.get(b, &source, &index_b, &all, Use::Other).unwrap();
    let joiner = Arc::new(std::sync::Mutex::new(None));
    {
        let (store, joiner, source, index, all) = (store.clone(), joiner.clone(), source.clone(), index.clone(), all.clone());
        let calls = AtomicU64::new(0);
        set_before_read(
            a,
            Some(Arc::new(move |p: &str| {
                if p != "/3271-forget-other-a.exr" || calls.fetch_add(1, Ordering::SeqCst) > 0 {
                    return;
                }
                // A's decode is in flight: B changes on disk, and another request asks for A.
                store.forget("/3271-forget-other-b.exr");
                let (store, source, index, all) = (store.clone(), source.clone(), index.clone(), all.clone());
                *joiner.lock().unwrap() =
                    Some(std::thread::spawn(move || store.get("/3271-forget-other-a.exr", &source, &index, &all, Use::Other).map(|_| ())));
            })),
        );
    }
    let got = store.get(a, &source, &index, &all, Use::Other);
    set_before_read(a, None);
    assert!(got.is_ok());
    let joined = joiner.lock().unwrap().take().unwrap().join().unwrap();
    assert!(joined.is_ok());
    assert_eq!(decodes_of(a).len(), 1, "the other request joined A's decode (or found it kept)");
    assert_eq!(store.cached(), [a], "A's decode is kept, B is forgotten");
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
    let all = Want { all: true, ids: (0..4).collect() };
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
    assert!(pool.aux_for(ItemId(1), &f[given], Tick::ZERO, &AuxNeed::All).unwrap().get("diffuse.R").is_some());
    assert_eq!(take_decodes(), [0, 1], "the picture and the channels share one decode");
    pool.frame_at(&f[learned], Tick::ZERO).unwrap();
    pool.aux_for(ItemId(2), &f[learned], Tick::ZERO, &AuxNeed::All).unwrap();
    assert_eq!(take_decodes(), [0, 2]);
    let d = decodes_of(learned);
    assert_eq!(d[0].channels.len(), 4, "the picture alone");
    assert_eq!(d[1].channels.len(), crate::exr_index::cached(learned).unwrap().aux_ids().len(), "all but the picture: a picture read alone isn't kept twice");
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
        assert!(crate::exr_index::get(&path, &source).unwrap().picture.is_some(), "file {k}");
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
    let all = Want { all: true, ids: (0..4).collect() };
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
    let all = Want { all: true, ids: (0..4).collect() };
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
    let all = Want { all: true, ids: (0..4).collect() };
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
    // Learned by `aux_for()`, then a new set without it: picture only again.
    pool.aux_for(ItemId(1), &f, at(1), &AuxNeed::All).unwrap();
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
        assert!(crate::exr_index::get(&path, &source).unwrap().picture.is_some(), "file {k}");
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
    let picture = Want { all: false, ids: index.picture_ids().into_iter().collect() };
    let all = Want { all: true, ids: (0..4).collect() };
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

/// #3750 review: two requests that each fit under the read's limit, merged into one decode that
/// doesn't, each get their planes: a refusal for the merged size is not handed to a request
/// whose own planes fit (they are read again for it alone).
#[test]
fn a_merged_decode_too_large_leaves_each_request_that_fits_its_planes() {
    let bytes: Arc<[u8]> = single_part_zips().into();
    let path = "/3750-merged-too-large.exr";
    let source = ExrSource::Bytes(bytes);
    let store = Arc::new(PlaneStore::new(CACHE_BUDGET));
    let index = crate::exr_index::get(path, &source).unwrap();
    let picture: BTreeSet<usize> = index.picture_ids().into_iter().collect();
    let first = Want { all: false, ids: picture.clone() };
    // (10 passes: with the picture's 4 planes 14, over the limit of 12)
    let passes: BTreeSet<usize> = (0..index.channels.len()).filter(|i| !picture.contains(i)).take(10).collect();
    let second = Want { all: false, ids: passes.clone() };
    set_cap(path, 12 * SIZE.0 * SIZE.1 * 4);
    // Before the first request's decode reads: the second request joins it.
    let (tx, rx) = std::sync::mpsc::channel();
    let joined = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let merged_seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let (store, source, index, second, joined, merged_seen) =
            (store.clone(), source.clone(), index.clone(), second.clone(), joined.clone(), merged_seen.clone());
        let hook: Hook = Arc::new(move |_: &str| {
            if joined.swap(true, Ordering::SeqCst) {
                return;
            }
            let (s, src, idx, want, tx) = (store.clone(), source.clone(), index.clone(), second.clone(), tx.clone());
            std::thread::spawn(move || {
                let _ = tx.send(s.get(path, &src, &idx, &want, Use::Other).map(|b| b.ids.clone()));
            });
            let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while std::time::Instant::now() < until {
                let merged = store
                    .lock()
                    .entries
                    .iter()
                    .find(|e| e.path == path)
                    .and_then(|e| e.flight.clone())
                    .is_some_and(|f| second.ids.iter().all(|id| f.state().want.ids.contains(id)));
                if merged {
                    merged_seen.store(true, Ordering::SeqCst);
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        });
        set_before_read(path, Some(hook));
    }
    let got_first = store.get(path, &source, &index, &first, Use::Other).map(|b| b.ids.clone());
    let got_second = rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap();
    set_before_read(path, None);
    assert!(joined.load(Ordering::SeqCst));
    assert!(merged_seen.load(Ordering::SeqCst), "the second request joined the first one's decode");
    let got_first = got_first.expect("the picture alone fits");
    let got_second = got_second.expect("the passes alone fit");
    assert!(picture.iter().all(|id| got_first.contains(id)), "{got_first:?}");
    assert!(passes.iter().all(|id| got_second.contains(id)), "{got_second:?}");
}

/// #474: at most CACHE_FILES entries are kept, however small.
#[test]
fn the_store_keeps_a_bounded_number_of_files() {
    let bytes: Arc<[u8]> = write(vec![part(None, &[("R", true)], (2, 2), Compression::ZIP1)], (2, 2)).into();
    let store = PlaneStore::new(CACHE_BUDGET);
    let all = Want { all: true, ids: (0..4).collect() };
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
        assert!(crate::exr_index::build(&mut std::io::Cursor::new(bytes.as_slice()), FileStamp::Virtual).is_err());
        assert!(crate::exr_channels::read_exr_channels(&bytes).is_none());
        assert!(crate::exr_channels::picture_image(&bytes).is_none());
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
        // (the good file indexes and reads whole through the same path)
        let (ix, mut src) = indexed(good);
        let all = ReadRequest::full((0..ix.channels.len()).collect());
        assert!(crate::exr_read::read_planes(&mut src, &ix, &all).0.is_ok());
        assert!(crate::exr_channels::read_exr_channels(good).is_some());
        let start = chunk_table(good).iter().flatten().map(|(at, _)| *at).min().unwrap() as usize;
        // From a little before the chunks (the offset tables) to the end, in steps.
        let from = start.saturating_sub(64);
        let (mut reads, mut read_ok) = (0, 0);
        for (i, at) in (from..good.len()).step_by(((good.len() - from) / 150).max(1)).enumerate() {
            let mut bad = good.clone();
            bad[at] ^= [0xff, 0x80, 0x01, 0x7f][i % 4];
            let _ = crate::exr_channels::read_exr_channels(&bad);
            let _ = crate::exr_channels::picture_image(&bad);
            let mut src = std::io::Cursor::new(bad.as_slice());
            if let Ok(ix) = crate::exr_index::build(&mut src, FileStamp::Virtual) {
                let req = ReadRequest { rows: Some(3..9), level: 1, ..ReadRequest::full((0..ix.channels.len()).collect()) };
                reads += 1;
                read_ok += usize::from(crate::exr_read::read_planes(&mut src, &ix, &req).0.is_ok());
            }
        }
        // Most damage lies in the chunks: the index stands and the reader meets the damage.
        assert!(reads > 50 && read_ok > 0, "{reads} damaged files indexed, {read_ok} read");
    }
}

/// #3271 review (never crash): damaged headers are refused before a decoder sees them
/// ([`crate::exr_index::safe_to_decode`] guards the `image` crate's decoder), and so is a part
/// larger than this reader takes; a file the reader doesn't cover but whose headers are sound
/// (HTJ2K) is left to the decoders to report.
#[test]
fn damaged_headers_are_not_safe_to_decode() {
    let good = write(vec![part(None, &RGBA, (8, 8), Compression::ZIP1)], (8, 8));
    assert!(crate::exr_index::safe_to_decode(&good));
    // Cut inside the headers.
    let cut = good.windows(11).position(|w| w == b"compression").unwrap();
    assert!(!crate::exr_index::safe_to_decode(&good[..cut]), "headers cut short");
    // Not an OpenEXR file.
    let mut magic = good.clone();
    magic[0] ^= 0xff;
    assert!(!crate::exr_index::safe_to_decode(&magic), "no signature");
    // Offset tables longer than the file.
    let first_chunk = chunk_table(&good)[0][0].0 as usize;
    assert!(!crate::exr_index::safe_to_decode(&good[..first_chunk - 8]), "tables cut short");
    assert!(!crate::exr_index::safe_to_decode(&with_data_window_x_max(good.clone(), (1 << 30) - 3)), "a part too large");
    // HTJ2K (compression 11): sound headers, left to the decoders.
    let mut htj2k = good.clone();
    let tag = b"compression\0compression\0\x01\0\0\0";
    let at = htj2k.windows(tag.len()).position(|w| w == tag).unwrap() + tag.len();
    htj2k[at] = 11;
    assert_eq!(
        crate::exr_index::build(&mut std::io::Cursor::new(htj2k.as_slice()), FileStamp::Virtual).err(),
        Some(crate::exr_index::IndexError::Unsupported(crate::exr_index::HTJ2K))
    );
    assert!(crate::exr_index::safe_to_decode(&htj2k));
}

/// #3271 review (never crash): the headers are read leniently, so a channel list the `exr`
/// crate's validation refused (a zero sampling factor, which its decompressors divide by, or
/// channels out of order) is refused when indexing, before any chunk is decompressed.
#[test]
fn an_invalid_channel_list_is_refused() {
    let good = write(vec![part(None, &[("G", true), ("R", true)], (4, 4), Compression::ZIP1)], (4, 4));
    let tag = b"channels\0chlist\0";
    let list = good.windows(tag.len()).position(|w| w == tag).unwrap() + tag.len() + 4;
    // Each entry: name, NUL, pixel type (4), linear (1), reserved (3), x and y sampling (4 + 4).
    let mut zero = good.clone();
    let x_sampling = list + 2 + 4 + 1 + 3;
    zero[x_sampling..x_sampling + 4].copy_from_slice(&0i32.to_le_bytes());
    let mut swapped = good.clone();
    swapped[list] = b'R';
    swapped[list + 2 + 16] = b'G';
    assert!(crate::exr_index::build(&mut std::io::Cursor::new(good.as_slice()), FileStamp::Virtual).is_ok());
    for bad in [zero, swapped] {
        assert!(crate::exr_index::build(&mut std::io::Cursor::new(bad.as_slice()), FileStamp::Virtual).is_err());
        assert!(crate::exr_channels::read_exr_channels(&bad).is_none());
        assert!(crate::exr_channels::picture_image(&bad).is_none());
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

/// #3271: decoded channels are kept in the disk cache, one entry per channel: once RAM has let
/// them go they are read back from there, bit for bit (Cryptomatte IDs included), whatever was
/// asked for before (the picture first, then the channels, then all of it after an eviction);
/// Reload Footage drops them.
/// The tests that set the process-wide disk cache take turns (one would switch off another's).
static DISK_TESTS: Mutex<()> = Mutex::new(());

#[test]
fn decoded_planes_are_kept_on_disk_and_read_back_exactly() {
    let _disk = DISK_TESTS.lock().unwrap_or_else(PoisonError::into_inner);
    use effectcraft_render::disk_cache::{DiskCache, Kind};
    let dir = std::env::temp_dir().join(format!("ec-3271-disk-planes-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("crypto_0001.exr");
    std::fs::write(&file, multipart()).unwrap();
    let dc = DiskCache::open(dir.join("cache"), 1 << 30).unwrap();
    // (every decode is written, however quick: not decided by this machine's speed)
    dc.set_read_us_per_mb(0);
    // (only this test's files use it)
    use_disk_for(&dir.to_string_lossy());
    set_disk_cache(Some(&dc));
    let f = crate::probe(&file).unwrap();
    let pool = crate::MediaPool::new();
    // The picture first, then every channel: 4, then the other 5 read from the file.
    pool.frame_at(&f, Tick::ZERO).unwrap();
    let first = pool.aux(ItemId(1), &f, Tick::ZERO).unwrap();
    dc.flush();
    assert_eq!(dc.keys(Kind::Plane).len(), 9, "one entry per channel: {:?}", dc.stats());
    // RAM lets go: all of it again comes from disk, without reading the file.
    pool.forget(&f.path);
    let again = pool.aux(ItemId(1), &f, Tick::ZERO).unwrap();
    let bits = |a: &AuxChannels| a.channels.iter().map(|(n, p)| (n.clone(), p.iter().map(|v| v.to_bits()).collect::<Vec<_>>())).collect::<Vec<_>>();
    assert_eq!(bits(&again), bits(&first), "bit for bit");
    // Reload Footage: the entries go, the file is decoded again.
    dc.drop_files([f.path.clone()]);
    dc.flush();
    pool.forget(&f.path);
    pool.aux(ItemId(1), &f, Tick::ZERO).unwrap();
    // #3750 review: a disk cache too small for one plane gets no copies of the planes at all.
    dc.flush();
    dc.set_max_bytes(1024);
    let small = dir.join("crypto_0002.exr");
    std::fs::write(&small, multipart()).unwrap();
    let g = crate::probe(&small).unwrap();
    pool.aux(ItemId(2), &g, Tick::ZERO).unwrap();
    set_disk_cache(None);
    let copies = DISK_COPIES.lock().unwrap().iter().filter(|p| **p == g.path).count();
    assert_eq!(copies, 0, "no plane copied for a cache it can't go in");
    assert!(DISK_COPIES.lock().unwrap().contains(&f.path), "copied while it could");
    let from_disk: Vec<bool> = decodes_of(&f.path).iter().map(|d| d.from_disk).collect();
    assert_eq!(from_disk, [false, false, true, false]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// #3271: a decode thread asking for planes whose decode is still queued on the decode threads
/// (an aux request on a frame worker queued it, then the picture's decode asks) takes it over
/// instead of decoding a copy: one decode, and both requests get its planes. The queued job is
/// held back until the claim (a hook), so the order is the same every time.
#[test]
fn a_decode_thread_takes_over_a_queued_decode() {
    let bytes: Arc<[u8]> = single_part_zips().into();
    let path = "/3271-claimed.exr";
    let source = ExrSource::Bytes(bytes);
    let store = Arc::new(PlaneStore::new(CACHE_BUDGET));
    let index = crate::exr_index::get(path, &source).unwrap();
    let all = Want { all: true, ids: (0..4).collect() };
    let threads = crate::pool::decode_threads().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    // When the asker queues its decode: a decode thread asks for the same planes.
    {
        let (store, source, index, all, tx) = (store.clone(), source.clone(), index.clone(), all.clone(), tx.clone());
        let hook: Hook = Arc::new(move |_: &str| {
            let (store, source, index, all, tx) = (store.clone(), source.clone(), index.clone(), all.clone(), tx.clone());
            threads.spawn(move || {
                let _ = tx.send(store.get(path, &source, &index, &all, Use::Other).map(|b| b.ids.len()));
            });
        });
        QUEUING.lock().unwrap().push((path.to_string(), hook));
    }
    // The queued job waits until that decode thread claimed it.
    let hold: Hook = Arc::new(|p: &str| {
        let claims = CLAIMS.lock().unwrap();
        let _ = CLAIM_SIGNAL.wait_timeout_while(claims, std::time::Duration::from_secs(10), |c| !c.iter().any(|c| c == p));
    });
    STARTING.lock().unwrap().push((path.to_string(), hold));
    let asked = store.get(path, &source, &index, &all, Use::Other).map(|b| b.ids.len());
    let claimed = rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap();
    QUEUING.lock().unwrap().retain(|(p, _)| p != path);
    STARTING.lock().unwrap().retain(|(p, _)| p != path);
    assert!(CLAIMS.lock().unwrap().iter().any(|p| p == path), "claimed");
    assert!(asked.is_ok() && claimed == asked, "{claimed:?} {asked:?}");
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
        fn oldest(&self) -> Option<effectcraft_render::memory::Candidate> {
            self.0.oldest_candidate()
        }
        fn drop_oldest(&self) -> usize {
            self.0.drop_candidate()
        }
    }
    let bytes: Arc<[u8]> = write(vec![part(None, &RGBA, (32, 32), Compression::ZIP1)], (32, 32)).into();
    let store = Arc::new(PlaneStore::new(CACHE_BUDGET));
    let all = Want { all: true, ids: (0..4).collect() };
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

/// #3271: with the localizer on, a network OpenEXR is copied in the background and then read
/// from its local copy, with the same pixels; what is read is still named by the share's path
/// and identity (caches key it the same either way).
#[test]
fn a_localized_network_file_is_read_from_its_copy() {
    let _serial = crate::localize::TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let root = std::env::temp_dir().join(format!("ec-3271-localized-{}", std::process::id()));
    let (share, local) = (root.join("share"), root.join("local"));
    std::fs::create_dir_all(&share).unwrap();
    crate::localize::treat_as_network(&share);
    let file = share.join("plate_0001.exr");
    std::fs::write(&file, multipart()).unwrap();
    let f = crate::probe(&file).unwrap();
    let pool = crate::MediaPool::new();
    pool.set_localizer(Some(effectcraft_render::LocalizeConfig { folder: local.clone(), max_bytes: 1 << 30, network: vec![], disk_space: None }));
    let from_share = pool.frame_at(&f, Tick::ZERO).unwrap();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while crate::localize::status().is_some_and(|s| s.files == 0) && std::time::Instant::now() < until {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(pool.localize_status().is_some_and(|s| s.files == 1), "{:?}", pool.localize_status());
    pool.forget(&f.path);
    let before = crate::opens_under(&local);
    let (img, read) = crate::reads::captured(|| pool.frame_at(&f, Tick::ZERO).unwrap());
    assert!(crate::opens_under(&local) > before, "read from the local copy");
    assert_eq!(bits(&img), bits(&from_share));
    let stamp = FileStamp::of_metadata(&std::fs::metadata(&file).unwrap());
    assert!(read.files().iter().any(|r| *r.path == *f.path && r.stamp == stamp), "named by the share's file: {read:?}");
    pool.set_localizer(None);
    let _ = std::fs::remove_dir_all(&root);
}

/// #3271: a viewer region of interest reads only the footage rows it shows of an OpenEXR layer
/// (with one scan line per chunk, only those lines are decompressed), and the region renders
/// bit for bit what reading the whole frame renders, at full and reduced resolution and at
/// several layer scales. In a collapsed precomp the whole frame is read (its layers are placed
/// through the precomp layer too).
#[test]
fn a_region_of_interest_reads_only_its_rows() {
    use effectcraft_project::{Comp, ItemKind, LayerSource, Project, Value, build};
    use effectcraft_render::{RenderOpts, Renderer};
    use effectcraft_time::FrameRate;
    /// A pool reading whole frames (the region's rows ignored).
    struct Whole<'a>(&'a crate::MediaPool);
    impl FootageSource for Whole<'_> {
        fn frame(&self, item: ItemId, f: &Footage, t: Tick) -> Option<Arc<Image>> {
            self.0.frame(item, f, t)
        }
        fn frame_with(&self, item: ItemId, f: &Footage, t: Tick, size: Option<(u32, u32)>, float: FloatCurve) -> Option<Arc<Image>> {
            self.0.frame_with(item, f, t, size, float)
        }
    }
    let bytes: Arc<[u8]> = single_part_zips().into();
    let path = "/3271-roi.exr";
    let fresh = || {
        let pool = crate::MediaPool::new();
        pool.add_bytes(path, bytes.clone());
        pool
    };
    let f = crate::probe_bytes(path, bytes.clone()).unwrap();
    let mut p = Project::default();
    let comp = Comp::new(96, 96, FrameRate::FPS_30, Tick::from_seconds_f64(1.0));
    let cid = p.add_item("Comp", effectcraft_color::Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    let fid = p.add_item("plate", effectcraft_color::Label::Aqua, None, ItemKind::Footage(f));
    let mut l = build::layer(&mut p, &comp, "plate", LayerSource::Footage { item: fid }, (64, 64), None);
    l.props.prop_mut("transform/position").unwrap().value = Value::Vec3([50.0, 46.0, 0.0]);
    l.props.prop_mut("transform/rotation").unwrap().value = Value::Scalar(12.0);
    p.comp_mut(cid).unwrap().layers = vec![l.clone()].into();
    // The same layer inside a collapsed precomp, moved by the precomp layer.
    let inner = p.add_item("Inner", effectcraft_color::Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    p.comp_mut(inner).unwrap().layers = vec![l].into();
    let outer = p.add_item("Outer", effectcraft_color::Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    let mut pre = build::layer(&mut p, &comp, "Inner", LayerSource::Comp { item: inner }, (96, 96), None);
    pre.switches.collapse = true;
    pre.props.prop_mut("transform/position").unwrap().value = Value::Vec3([40.0, 60.0, 0.0]);
    p.comp_mut(outer).unwrap().layers = vec![pre].into();
    for (layer_scale, scale) in [(100.0, 1.0), (100.0, 0.5), (30.0, 0.25), (30.0, 1.0), (250.0, 0.5)] {
        for c in [cid, inner] {
            p.comp_mut(c).unwrap().layers[0].props.prop_mut("transform/scale").unwrap().value = Value::Vec3([layer_scale * 0.9, layer_scale * 1.1, 100.0]);
        }
        let opts = RenderOpts { scale, roi: Some([38.0, 40.0, 24.0, 10.0]), ..Default::default() };
        let whole = Renderer::new(&p, &Whole(&fresh()), opts).comp_frame(cid, Tick::ZERO);
        let before = decodes_of(path).len();
        let rows = Renderer::new(&p, &fresh(), opts).comp_frame(cid, Tick::ZERO);
        assert_eq!(bits(&rows), bits(&whole), "{layer_scale}% at {scale}");
        // (a layer shrunk this far shows all of its rows in the region, grown by the warp's reach)
        if layer_scale >= 100.0 {
            let d = decodes_of(path);
            let last = d.get(before..).and_then(|d| d.last()).expect("a read of the region's rows");
            assert!(last.stats.lines < SIZE.1, "{layer_scale}% at {scale}: {} lines", last.stats.lines);
        }
        // Collapsed: the whole frame.
        let whole = Renderer::new(&p, &Whole(&fresh()), opts).comp_frame(outer, Tick::ZERO);
        let before = decodes_of(path).len();
        let collapsed = Renderer::new(&p, &fresh(), opts).comp_frame(outer, Tick::ZERO);
        assert_eq!(bits(&collapsed), bits(&whole), "collapsed, {layer_scale}% at {scale}");
        assert!(decodes_of(path).get(before..).unwrap_or_default().iter().all(|d| d.stats.lines == SIZE.1), "collapsed: the whole frame");
    }
}

/// A source that calls the request off after `cancel_after` reads, or fails every read from
/// byte `fail_from` on (tests of a read in several batches).
struct Flaky {
    inner: std::io::Cursor<Vec<u8>>,
    reads: usize,
    cancel_after: Option<(usize, Arc<std::sync::atomic::AtomicBool>)>,
    fail_from: Option<u64>,
}

impl std::io::Read for Flaky {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.reads += 1;
        if let Some((n, flag)) = &self.cancel_after
            && self.reads > *n
        {
            flag.store(true, Ordering::SeqCst);
        }
        if self.fail_from.is_some_and(|at| self.inner.position() >= at) {
            return Err(std::io::Error::other("the share went away"));
        }
        self.inner.read(buf)
    }
}

impl std::io::Seek for Flaky {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

/// #3271: a file of more chunks than one batch is read in several (the next batch is read while
/// the one before decompresses, adjacent chunks in one read): the planes are the file's values,
/// also for a row range across batch boundaries; called off between batches it stops; a read
/// failing in a later batch is an I/O error.
#[test]
fn a_read_in_several_batches_is_whole_and_stops_between_them() {
    let (w, h) = (8usize, 700usize);
    let channels = [("A", true), ("B", false), ("G", true), ("R", true)];
    let bytes = write(vec![part(None, &channels, (w, h), Compression::ZIP1)], (w, h));
    let (ix, mut src) = indexed(&bytes);
    let expect = |c: usize, i: usize| {
        let (name, half) = channels[c];
        let v = value(i, c, name == "A");
        if half { f16::from_f32(v).to_f32() } else { v }
    };
    let all = ReadRequest::full(vec![0, 1, 2, 3]);
    let (planes, stats) = crate::exr_read::read_planes(&mut src, &ix, &all);
    let planes = planes.unwrap();
    assert_eq!(stats.lines, h, "every line, in three batches");
    for (c, p) in planes.iter().enumerate() {
        assert!(p.iter().enumerate().all(|(i, v)| v.to_bits() == expect(c, i).to_bits()), "channel {c}");
    }
    let band = ReadRequest { rows: Some(250..520), ..all.clone() };
    let (got, stats) = crate::exr_read::read_planes(&mut src, &ix, &band);
    let got = got.unwrap();
    assert_eq!(stats.lines, 270);
    for (c, p) in got.iter().enumerate() {
        for (i, v) in p.iter().enumerate() {
            let want = if (250..520).contains(&(i / w)) { expect(c, i) } else { 0.0 };
            assert_eq!(v.to_bits(), want.to_bits(), "channel {c}, sample {i}");
        }
    }
    // Called off while the first batch is read: it stops before the next.
    let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut flaky = Flaky { inner: std::io::Cursor::new(bytes.clone()), reads: 0, cancel_after: Some((1, flag.clone())), fail_from: None };
    let (got, _) = filmcraft_media::cancel::with_cancel(&flag, || crate::exr_read::read_planes(&mut flaky, &ix, &all));
    assert_eq!(got.err(), Some(ReadError::Cancelled));
    // Chunks that follow each other in the file are read in one go, not one read per chunk.
    let mut counted = Flaky { inner: std::io::Cursor::new(bytes.clone()), reads: 0, cancel_after: None, fail_from: None };
    assert!(crate::exr_read::read_planes(&mut counted, &ix, &all).0.is_ok());
    assert!(counted.reads <= 12, "{} reads for {h} chunks", counted.reads);
    // (a failed read has no stats: the reads it made tell how far it went)
    assert!(flaky.reads < counted.reads, "stopped early: {} of {} reads", flaky.reads, counted.reads);
    // A read failing in a later batch (at chunk 300): an I/O error after the first was read.
    let at = chunk_table(&bytes)[0][300].0;
    let mut flaky = Flaky { inner: std::io::Cursor::new(bytes), reads: 0, cancel_after: None, fail_from: Some(at) };
    let (got, _) = crate::exr_read::read_planes(&mut flaky, &ix, &all);
    assert!(matches!(got, Err(ReadError::Io(_))), "{got:?}");
    assert!(flaky.reads > 1, "the batches before it were read");
}

/// #3271: dropping entries for bytes (a decode over the budget, the memory accountant) drops
/// only entries that hold bytes: an unreadable file stays remembered (and isn't read again)
/// until there are too many entries.
#[test]
fn trimming_for_bytes_keeps_remembered_failures() {
    let bytes: Arc<[u8]> = write(vec![part(None, &RGBA, (32, 32), Compression::ZIP1)], (32, 32)).into();
    let one = 4 * (32 * 32 * 4 + 1);
    let store = PlaneStore::new(one);
    let all = Want { all: true, ids: (0..4).collect() };
    let mut broken = bytes.to_vec();
    broken.truncate(broken.len() - 64);
    let broken = ExrSource::Bytes(broken.into());
    let path = "/3271-trim-broken.exr";
    let index = crate::exr_index::get(path, &broken).unwrap();
    assert!(store.get(path, &broken, &index, &all, Use::Other).is_err());
    for p in ["/3271-trim-a.exr", "/3271-trim-b.exr"] {
        let source = ExrSource::Bytes(bytes.clone());
        let index = crate::exr_index::get(p, &source).unwrap();
        assert!(store.get(p, &source, &index, &all, Use::Other).is_ok());
    }
    while store.drop_candidate() > 0 {}
    assert_eq!(store.cached(), [path], "the planes went, the failure stays");
    assert!(store.get(path, &broken, &index, &all, Use::Other).is_err());
    assert_eq!(decodes_of(path).len(), 1, "not read again");
}

/// #3271 (user test): the decoded planes have no budget of their own; the plane store follows
/// the footage frames' cap (#3298: the whole common budget on the desktop). A cap of 10000 MB
/// lets the plane store hold far more than 1 GB, and a change resizes it at once.
#[test]
fn the_footage_budget_sizes_the_plane_store() {
    use effectcraft_render::FootageSource;
    let pool = crate::MediaPool::new();
    pool.set_cache_budget(10_000 << 20);
    assert_eq!(super::budget(), 10_000 << 20);
    assert!(super::budget() > 1 << 30, "more than the old fixed 1 GB");
    pool.set_cache_budget(3_000 << 20);
    assert_eq!(super::budget(), 3_000 << 20, "resized at run time");
    // (back to the default for the tests beside it)
    pool.set_cache_budget(CACHE_BUDGET);
}

/// #3271 (user decision): the footage frames and the decoded planes have one least-recently-used
/// order: filling the frame cache drops the oldest plane sets, filling the plane store drops the
/// oldest frames; neither side has a share of its own. (#3298: they share the one budget with
/// every cache, and the accountant orders by value; at the same cost per byte, as here, that is
/// least recently used first.)
#[test]
fn frames_and_planes_make_room_for_each_other() {
    use effectcraft_render::memory::{Accountant, Candidate, MemoryHolder};
    struct Planes(Arc<PlaneStore>);
    impl MemoryHolder for Planes {
        fn name(&self) -> &'static str {
            "planes"
        }
        fn bytes(&self) -> usize {
            self.0.lock().entries.iter().map(|e| e.bytes).sum()
        }
        fn oldest(&self) -> Option<Candidate> {
            self.0.oldest_candidate()
        }
        fn drop_oldest(&self) -> usize {
            self.0.drop_candidate()
        }
    }
    /// A holder whose items all cost the same per byte (decode times vary from run to run).
    struct SameCost(Arc<dyn MemoryHolder>);
    impl MemoryHolder for SameCost {
        fn name(&self) -> &'static str {
            self.0.name()
        }
        fn bytes(&self) -> usize {
            self.0.bytes()
        }
        fn oldest(&self) -> Option<Candidate> {
            self.0.oldest().map(|c| Candidate { cost_us: c.bytes as u64, ..c })
        }
        fn drop_oldest(&self) -> usize {
            self.0.drop_oldest()
        }
    }
    let bytes: Arc<[u8]> = write(vec![part(None, &RGBA, (32, 32), Compression::ZIP1)], (32, 32)).into();
    let one = 4 * (32 * 32 * 4 + 1);
    let budget = 5 * one;
    let store = Arc::new(PlaneStore::new(budget));
    let pool = crate::MediaPool::with_budget(budget);
    let acc = Accountant::new(budget);
    let holders: Vec<Arc<dyn MemoryHolder>> = vec![Arc::new(SameCost(Arc::new(Planes(store.clone())))), Arc::new(SameCost(pool.memory_holder()))];
    for h in &holders {
        acc.register(h);
    }
    let all = Want { all: true, ids: (0..4).collect() };
    let planes = |p: &str| {
        let source = ExrSource::Bytes(bytes.clone());
        let index = crate::exr_index::get(p, &source).unwrap();
        drop(store.get(p, &source, &index, &all, Use::Other).unwrap());
    };
    // Plane sets first, then frames: the frames push the oldest plane sets out.
    planes("/3271-share-p1.exr");
    planes("/3271-share-p2.exr");
    planes("/3271-share-p3.exr");
    for f in ["/f1", "/f2", "/f3"] {
        pool.keep_test_frame(f, 32, 32);
        acc.balance();
    }
    assert_eq!(pool.test_frames(), ["/f1", "/f2", "/f3"], "the frames stay");
    assert!(!store.cached().contains(&"/3271-share-p1.exr".to_string()), "the oldest plane set went: {:?}", store.cached());
    assert!(store.cached().contains(&"/3271-share-p3.exr".to_string()));
    // Now plane sets: they push the oldest frames out.
    for p in ["/3271-share-p4.exr", "/3271-share-p5.exr", "/3271-share-p6.exr"] {
        planes(p);
        acc.balance();
    }
    let frames = pool.test_frames();
    assert!(!frames.contains(&"/f1".to_string()), "the oldest frame went: {frames:?}");
    assert!(store.cached().contains(&"/3271-share-p6.exr".to_string()), "the newest plane set stays");
    assert!(acc.total() <= budget, "{} of {budget}", acc.total());
}

/// #3271 (rule: picture channels are never kept twice): once the picture is converted (the
/// media pool keeps it), its R, G, B, A planes leave the plane store; the channels the effects
/// read stay, and a picture channel an effect names itself (EXtractoR reading "R") stays too.
#[test]
fn the_picture_planes_are_not_kept_beside_the_picture() {
    let bytes: Arc<[u8]> =
        write(vec![part(None, &[("R", true), ("G", true), ("B", true), ("A", true), ("Z", false), ("N", false)], (16, 8), Compression::ZIP1)], (16, 8)).into();
    let source = ExrSource::Bytes(bytes);
    let op = crate::convert::AlphaOp::of(&Footage::default(), FloatCurve::Linear);
    let plane = 16 * 8 * 4;
    for (path, need, kept) in [
        ("/3271-slim-aux.exr", AuxNeed::Lookups(vec![AuxLookup::Name("Z".into())]), vec!["Z"]),
        ("/3271-slim-named.exr", AuxNeed::Lookups(vec![AuxLookup::Name("Z".into()), AuxLookup::Name("R".into())]), vec!["R", "Z"]),
        ("/3271-slim-all.exr", AuxNeed::All, vec!["N", "Z"]),
    ] {
        let Picture::Done(Ok(_)) = picture(path, &source, op, Some(&need)) else { panic!("{path}: a picture") };
        let (names, bytes) = shared().kept(path).expect("the effects' channels are kept");
        assert_eq!(names, kept, "{path}");
        assert_eq!(bytes, kept.iter().map(|n| n.len() + plane).sum::<usize>(), "{path}");
        forget(path);
    }
    // The picture alone keeps nothing.
    let path = "/3271-slim-none.exr";
    let Picture::Done(Ok(_)) = picture(path, &source, op, None) else { panic!("a picture") };
    assert!(shared().kept(path).is_none());
    forget(path);
}

/// #3750 review: slimming a set someone else still holds copies the planes it keeps; when the
/// system can't give the memory for the copy, the set stays as it was (nothing aborts).
#[test]
fn a_shared_set_stays_whole_when_its_copy_finds_no_memory() {
    let bytes: Arc<[u8]> =
        write(vec![part(None, &[("R", true), ("G", true), ("B", true), ("A", true), ("Z", false)], (16, 8), Compression::ZIP1)], (16, 8)).into();
    let path = "/3750-slim-no-memory.exr";
    let source = ExrSource::Bytes(bytes);
    let store = PlaneStore::new(CACHE_BUDGET);
    let index = crate::exr_index::get(path, &source).unwrap();
    let all = Want { all: true, ids: index.picture_ids().into_iter().collect() };
    let whole = store.get(path, &source, &index, &all, Use::Other).unwrap();
    let z: BTreeSet<usize> = index.aux_ids().into_iter().collect();
    COPIES_REFUSED.lock().unwrap().push(path.to_string());
    // (`whole` is still held here: the planes kept would be copied)
    store.keep_only(path, whole.clone(), &z);
    COPIES_REFUSED.lock().unwrap().retain(|p| p != path);
    let kept = store.lock().entries.iter().find(|e| e.path == path).and_then(|e| e.batch.clone());
    assert!(kept.is_some_and(|k| Arc::ptr_eq(&k, &whole)), "the whole set is kept");
    // With memory for the copy: the kept channel alone.
    store.keep_only(path, whole.clone(), &z);
    assert_eq!(store.kept(path).map(|(n, _)| n), Some(vec!["Z".to_string()]));
}

/// #3750 review: a frame of another size than its footage's (a sequence frame larger than the
/// first, shown scaled) is read whole under a region of interest, so the region shows what the
/// whole frame shows; the region's rows are of the footage's size, not of that frame.
#[test]
fn a_frame_of_another_size_is_read_whole_under_a_region() {
    use effectcraft_project::{Comp, ItemKind, LayerSource, Project, build};
    use effectcraft_render::{RenderOpts, Renderer};
    use effectcraft_time::FrameRate;
    struct Whole<'a>(&'a crate::MediaPool);
    impl FootageSource for Whole<'_> {
        fn frame(&self, item: ItemId, f: &Footage, t: Tick) -> Option<Arc<Image>> {
            self.0.frame(item, f, t)
        }
        fn frame_with(&self, item: ItemId, f: &Footage, t: Tick, size: Option<(u32, u32)>, float: FloatCurve) -> Option<Arc<Image>> {
            self.0.frame_with(item, f, t, size, float)
        }
    }
    let bytes: Arc<[u8]> = single_part_zips().into();
    let path = "/3750-roi-other-size.exr";
    let fresh = || {
        let pool = crate::MediaPool::new();
        pool.add_bytes(path, bytes.clone());
        pool
    };
    // (the footage's size is half the file's: a later, larger frame)
    let mut f = crate::probe_bytes(path, bytes.clone()).unwrap();
    f.width = SIZE.0 as u32 / 2;
    f.height = SIZE.1 as u32 / 2;
    let mut p = Project::default();
    let comp = Comp::new(96, 96, FrameRate::FPS_30, Tick::from_seconds_f64(1.0));
    let cid = p.add_item("Comp", effectcraft_color::Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    let fid = p.add_item("plate", effectcraft_color::Label::Aqua, None, ItemKind::Footage(f));
    let l = build::layer(&mut p, &comp, "plate", LayerSource::Footage { item: fid }, (SIZE.0 as u32 / 2, SIZE.1 as u32 / 2), None);
    p.comp_mut(cid).unwrap().layers = vec![l].into();
    let opts = RenderOpts { roi: Some([30.0, 40.0, 30.0, 8.0]), ..Default::default() };
    let whole = Renderer::new(&p, &Whole(&fresh()), opts).comp_frame(cid, Tick::ZERO);
    let before = decodes_of(path).len();
    let rows = Renderer::new(&p, &fresh(), opts).comp_frame(cid, Tick::ZERO);
    assert_eq!(bits(&rows), bits(&whole));
    let after = decodes_of(path).get(before..).unwrap_or_default().to_vec();
    assert!(!after.is_empty() && after.iter().all(|d| d.stats.lines == SIZE.1), "read whole: {after:?}");
}

/// #3271 review (#3301): a remembered failure belongs to the file as it was. A file that was
/// unreadable and is then replaced on disk (no Reload Footage: the new index comes from its new
/// stamp) is read again, not refused with the old file's failure.
#[test]
fn a_failure_is_forgotten_once_the_file_changes() {
    let good: Arc<[u8]> = write(vec![part(None, &RGBA, (8, 8), Compression::ZIP1)], (8, 8)).into();
    let mut broken = good.to_vec();
    broken.truncate(broken.len() - 64);
    let path = "/3271-failed-then-fixed.exr";
    let store = PlaneStore::new(CACHE_BUDGET);
    let all = Want { all: true, ids: (0..4).collect() };
    let source = ExrSource::Bytes(broken.into());
    let old = crate::exr_index::get(path, &source).unwrap();
    assert!(store.get(path, &source, &old, &all, Use::Other).is_err());
    assert!(store.get(path, &source, &old, &all, Use::Other).is_err(), "remembered for the same file");
    // The file is replaced: another index (here read under another name, as the file's new stamp
    // would give it).
    let source = ExrSource::Bytes(good);
    let new = crate::exr_index::get("/3271-failed-then-fixed-new.exr", &source).unwrap();
    assert!(!Arc::ptr_eq(&old, &new));
    assert!(store.get(path, &source, &new, &all, Use::Other).is_ok(), "the new file is read");
}

/// #3271 review: a failed picture request (too large here) doesn't refuse a later request for
/// the other channels: "all channels" is every channel but the picture's, so it doesn't ask for
/// what failed.
/// Memory the system refuses during a read is a read that failed just now, not a file too large:
/// it isn't remembered, and the next request reads again (a file too large is remembered).
#[test]
fn an_allocation_failure_is_not_remembered() {
    use crate::exr_read::{ReadError, read_error};
    assert!(matches!(read_error(craft_exr::seek::Error::OutOfMemory("planes".into())), ReadError::OutOfMemory(why) if why.contains("planes")));
    assert!(matches!(read_error(craft_exr::seek::Error::LimitExceeded("planes".into())), ReadError::TooLarge(_)));
    // A read that failed just now (here: the file can't be opened) is a stop, and the next
    // request for the same channels reads them.
    let bytes: Arc<[u8]> = single_part_zips().into();
    let path = "/oom-not-remembered.exr";
    let store = PlaneStore::new(CACHE_BUDGET);
    let index = crate::exr_index::get(path, &ExrSource::Bytes(bytes.clone())).unwrap();
    let want = Want { all: false, ids: index.picture_ids().into_iter().collect() };
    let gone = ExrSource::File(std::env::temp_dir().join("ec-oom-not-remembered-missing.exr"));
    assert_eq!(store.get(path, &gone, &index, &want, Use::Other).err(), Some(Refused::Stopped));
    assert!(store.get(path, &ExrSource::Bytes(bytes), &index, &want, Use::Other).is_ok(), "read again, not refused from memory");
}

#[test]
fn a_failed_picture_leaves_the_other_channels_readable() {
    let bytes: Arc<[u8]> =
        write(vec![part(None, &[("R", true), ("G", true), ("B", true), ("A", true), ("Z", false)], (8, 8), Compression::ZIP1)], (8, 8)).into();
    let path = "/3271-failed-picture.exr";
    // (room for three planes: the picture's four don't fit, the depth plane does)
    set_cap(path, 3 * 8 * 8 * 4);
    let store = PlaneStore::new(CACHE_BUDGET);
    let source = ExrSource::Bytes(bytes);
    let index = crate::exr_index::get(path, &source).unwrap();
    let picture = Want { all: false, ids: index.picture_ids().into_iter().collect() };
    assert!(matches!(store.get(path, &source, &index, &picture, Use::Other), Err(Refused::TooLarge(_))));
    assert!(store.get(path, &source, &index, &picture, Use::Other).is_err(), "remembered");
    let rest = store.get(path, &source, &index, &Want { all: true, ids: Default::default() }, Use::Other).expect("the other channels");
    assert!(rest.aux.get("Z").is_some());
}

/// #3749: a read over the fixed floor is allowed up to what the system's memory gives (the
/// session's figure), never under the floor, and refused with a reason beyond it. The floor is
/// small here: the real one is 8 GiB.
#[test]
fn a_read_over_the_floor_is_allowed_up_to_the_systems_memory() {
    let _only = LIMIT_TESTS.lock().unwrap_or_else(PoisonError::into_inner);
    let bytes = write(vec![part(None, &RGBA, (8, 8), Compression::ZIP1)], (8, 8));
    let mut src = std::io::Cursor::new(bytes.as_slice());
    let index = crate::exr_index::build(&mut src, FileStamp::Virtual).unwrap();
    // Four channels of 8×8 f32 are 1024 bytes; the floor is 512.
    let request = ReadRequest { floor: 512, ..ReadRequest::full(vec![0, 1, 2, 3]) };
    let read = |src: &mut std::io::Cursor<&[u8]>| crate::exr_read::read_planes(src, &index, &request).0;
    effectcraft_render::memory::set_decode_limit(None);
    effectcraft_render::memory::set_decode_limit_source(None);
    assert!(matches!(read(&mut src), Err(crate::exr_read::ReadError::TooLarge(_))), "over the floor, memory unknown");
    effectcraft_render::memory::set_decode_limit(Some(2048));
    assert_eq!(read(&mut src).map(|p| p.len()).ok(), Some(4), "the system's memory allows it");
    effectcraft_render::memory::set_decode_limit(Some(800));
    assert!(matches!(read(&mut src), Err(crate::exr_read::ReadError::TooLarge(_))), "still more than the system's memory");
    effectcraft_render::memory::set_decode_limit(None);
}

/// #3749: a refusal for the request's size is remembered only while the decode limit stays as it
/// was: when the system's memory figure arrives or changes, the request is decided anew.
#[test]
fn a_refusal_for_size_is_decided_anew_when_the_decode_limit_changes() {
    let _only = LIMIT_TESTS.lock().unwrap_or_else(PoisonError::into_inner);
    let bytes: Arc<[u8]> = write(vec![part(None, &RGBA, (8, 8), Compression::ZIP1)], (8, 8)).into();
    let path = "/3749-refusal-generation.exr";
    // Room for one plane of 8×8 f32: four are refused.
    set_cap(path, 8 * 8 * 4);
    let store = PlaneStore::new(CACHE_BUDGET);
    let source = ExrSource::Bytes(bytes);
    let index = crate::exr_index::get(path, &source).unwrap();
    let want = Want { all: false, ids: (0..4).collect() };
    // (the refusal is made before anything is read: the decodes are counted as they begin)
    let begun = Arc::new(AtomicU64::new(0));
    {
        let begun = begun.clone();
        set_before_read(
            path,
            Some(Arc::new(move |_: &str| {
                begun.fetch_add(1, Ordering::SeqCst);
            })),
        );
    }
    let refused = || matches!(store.get(path, &source, &index, &want, Use::Other), Err(Refused::TooLarge(_)));
    assert!(refused());
    assert!(refused());
    assert_eq!(begun.load(Ordering::SeqCst), 1, "the second refusal came from memory");
    // (figures under the fixed floor: only the change counts, no other test's limit moves)
    effectcraft_render::memory::set_decode_limit(Some(1000));
    effectcraft_render::memory::set_decode_limit(Some(2000));
    assert!(refused(), "still over the caller's own limit");
    assert_eq!(begun.load(Ordering::SeqCst), 2, "decided anew after the limit changed");
    set_before_read(path, None);
    effectcraft_render::memory::set_decode_limit(None);
}

/// A file of an 8×8 picture and a depth plane: five planes of 256 bytes.
fn picture_and_depth() -> Vec<u8> {
    write(vec![part(None, &[("R", true), ("G", true), ("B", true), ("A", true), ("Z", false)], (8, 8), Compression::ZIP1)], (8, 8))
}

/// #3749 (#3301 review): the limit is for the planes a request ends with, not for the ones read
/// just now: channels added to the planes held count with them. When together they are over
/// the limit the planes held are let go and the request is read on its own (it fits alone); a
/// request that doesn't fit alone is refused, whatever is held.
#[test]
fn channels_added_to_the_planes_held_count_toward_the_limit() {
    let path = "/3749-held-and-added.exr";
    // Room for four planes: the picture fits, one more with it is five.
    set_cap(path, 4 * 8 * 8 * 4);
    let store = PlaneStore::new(CACHE_BUDGET);
    let source = ExrSource::Bytes(picture_and_depth().into());
    let index = crate::exr_index::get(path, &source).unwrap();
    let picture = Want { all: false, ids: index.picture_ids().into_iter().collect() };
    store.get(path, &source, &index, &picture, Use::Other).expect("the picture fits");
    let rest = Want { all: true, ids: Default::default() };
    let depth = store.get(path, &source, &index, &rest, Use::Other).expect("the depth plane fits alone");
    assert!(depth.aux.get("Z").is_some());
    assert!(depth.aux.get("R").is_none(), "the picture's planes were let go: four held and one more are five");
    // (nothing else holds the depth plane: let go, it is gone, see
    // `planes_let_go_but_still_held_count_toward_the_limit`)
    drop(depth);
    // The picture again: it fits alone, the depth plane is let go in turn.
    let again = store.get(path, &source, &index, &picture, Use::Other).expect("the picture fits alone");
    assert!(again.aux.get("R").is_some() && again.aux.get("Z").is_none());
    // Everything at once is five planes: refused, however it is asked.
    let everything = Want { all: true, ids: (0..5).collect() };
    assert!(matches!(store.get(path, &source, &index, &everything, Use::Other).err(), Some(Refused::TooLarge(_))));
}

/// #3750 review: when the planes held are let go because they don't fit the limit with the ones
/// a request adds, they are let go before the request is read: the held planes and the new ones
/// are never in memory together (the store, the decode and its flight all let go of them). Also
/// for a picture's planes held for its effects.
#[test]
fn planes_let_go_are_gone_before_the_request_is_read() {
    let path = "/3750-let-go-before-read.exr";
    // Room for four planes: the picture fits, one more with it is five.
    set_cap(path, 4 * 8 * 8 * 4);
    let store = PlaneStore::new(CACHE_BUDGET);
    let source = ExrSource::Bytes(picture_and_depth().into());
    let index = crate::exr_index::get(path, &source).unwrap();
    let picture = Want { all: false, ids: index.picture_ids().into_iter().collect() };
    let held = Arc::downgrade(&store.get(path, &source, &index, &picture, Use::Picture).expect("the picture fits"));
    assert!(held.upgrade().is_some() && store.is_held(path), "kept for the effects");
    // Each decode that reads notes whether the picture's planes are still in memory.
    let alive = Arc::new(Mutex::new(Vec::new()));
    {
        let (alive, held) = (alive.clone(), held.clone());
        let hook: Hook = Arc::new(move |_: &str| alive.lock().unwrap().push(held.upgrade().is_some()));
        set_before_read(path, Some(hook));
    }
    let rest = Want { all: true, ids: Default::default() };
    let depth = store.get(path, &source, &index, &rest, Use::Other).expect("the depth plane fits alone");
    set_before_read(path, None);
    assert!(depth.aux.get("Z").is_some() && depth.aux.get("R").is_none());
    let alive = alive.lock().unwrap().clone();
    assert_eq!(alive.last(), Some(&false), "the picture's planes were gone when the depth plane was read: {alive:?}");
    assert_eq!(decodes_of(path).last().map(|d| d.channels.len()), Some(1), "the depth plane alone was read");
}

/// Notes, for each read of `path`'s planes, whether `old` was still in memory then.
fn alive_at_reads(path: &str, old: std::sync::Weak<AuxChannels>) -> Arc<Mutex<Vec<bool>>> {
    let alive = Arc::new(Mutex::new(Vec::new()));
    let noted = alive.clone();
    set_read_error(
        path,
        Some(Arc::new(move |_: &[usize]| {
            noted.lock().unwrap().push(old.strong_count() > 0);
            None
        })),
    );
    alive
}

/// #3750 review: planes the store let go that a caller still holds (an effect, a Cryptomatte
/// pick) are in memory: they count toward the limit, and a request that doesn't fit beside them
/// is refused for now (not remembered) instead of read beside them; once they are gone it is
/// read.
#[test]
fn planes_let_go_but_still_held_count_toward_the_limit() {
    let path = "/3750-let-go-still-held.exr";
    // Room for four planes: the picture fits, one more with it is five.
    set_cap(path, 4 * 8 * 8 * 4);
    let store = PlaneStore::new(CACHE_BUDGET);
    let source = ExrSource::Bytes(picture_and_depth().into());
    let index = crate::exr_index::get(path, &source).unwrap();
    let picture = Want { all: false, ids: index.picture_ids().into_iter().collect() };
    // A caller keeps the picture's planes.
    let held = store.get(path, &source, &index, &picture, Use::Other).expect("the picture fits").aux.clone();
    let alive = alive_at_reads(path, Arc::downgrade(&held));
    let rest = Want { all: true, ids: Default::default() };
    let refused = store.get(path, &source, &index, &rest, Use::Other).err();
    assert!(matches!(&refused, Some(Refused::NoMemory(why)) if why.contains("still in use")), "{refused:?}");
    drop(held);
    let depth = store.get(path, &source, &index, &rest, Use::Other).expect("read once they are gone: not remembered");
    set_read_error(path, None);
    assert!(depth.aux.get("Z").is_some() && depth.aux.get("R").is_none());
    let alive = alive.lock().unwrap().clone();
    assert_eq!(alive, vec![false], "read once, after the planes held were gone");
}

/// #3750 review: a decode thread that took over a decode whose held planes are let go asks
/// again while the request that queued the decode still holds that decode's flight: the flight
/// no longer keeps the planes, so they are gone, not read beside, when the request is read.
#[test]
fn a_claimed_decode_is_not_read_beside_the_planes_it_let_go() {
    let path = "/3750-claimed-let-go.exr";
    set_cap(path, 4 * 8 * 8 * 4);
    let source = ExrSource::Bytes(picture_and_depth().into());
    let store = Arc::new(PlaneStore::new(CACHE_BUDGET));
    let index = crate::exr_index::get(path, &source).unwrap();
    let picture = Want { all: false, ids: index.picture_ids().into_iter().collect() };
    let old = Arc::downgrade(&store.get(path, &source, &index, &picture, Use::Other).expect("the picture fits").aux);
    let alive = alive_at_reads(path, old.clone());
    let rest = Want { all: true, ids: Default::default() };
    let threads = crate::pool::decode_threads().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    // When the asker queues its decode: a decode thread asks for the same planes.
    {
        let (store, source, index, rest, tx, done) = (store.clone(), source.clone(), index.clone(), rest.clone(), tx.clone(), done.clone());
        let hook: Hook = Arc::new(move |_: &str| {
            let (store, source, index, rest, tx, done) = (store.clone(), source.clone(), index.clone(), rest.clone(), tx.clone(), done.clone());
            threads.spawn(move || {
                let got = store.get(path, &source, &index, &rest, Use::Other);
                done.store(true, Ordering::SeqCst);
                let _ = tx.send(got.map(|b| b.ids.len()));
            });
        });
        QUEUING.lock().unwrap().push((path.to_string(), hook));
    }
    // The queued job waits until that decode thread claimed it...
    let hold: Hook = Arc::new(|p: &str| {
        let claims = CLAIMS.lock().unwrap();
        let _ = CLAIM_SIGNAL.wait_timeout_while(claims, std::time::Duration::from_secs(10), |c| !c.iter().any(|c| c == p));
    });
    STARTING.lock().unwrap().push((path.to_string(), hold));
    // ...and the asker, handed the let-go, holds its flight until the decode thread has asked
    // again.
    {
        let done = done.clone();
        let wait: Hook = Arc::new(move |_: &str| {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !done.load(Ordering::SeqCst) && std::time::Instant::now() < until {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        });
        HANDED.lock().unwrap().push((path.to_string(), wait));
    }
    let asked = store.get(path, &source, &index, &rest, Use::Other).map(|b| b.ids.len());
    let claimed = rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap();
    QUEUING.lock().unwrap().retain(|(p, _)| p != path);
    STARTING.lock().unwrap().retain(|(p, _)| p != path);
    HANDED.lock().unwrap().retain(|(p, _)| p != path);
    set_read_error(path, None);
    assert!(CLAIMS.lock().unwrap().iter().any(|p| p == path), "claimed");
    assert_eq!(claimed.ok(), Some(1), "the decode thread reads the depth plane alone");
    assert_eq!(asked.ok(), Some(1), "so does the asker");
    assert!(old.upgrade().is_none());
    let alive = alive.lock().unwrap().clone();
    assert!(!alive.is_empty() && !alive.contains(&true), "no read beside the planes let go: {alive:?}");
}

/// #3749 (#3301 review): planes the disk cache gives back count toward the limit of a request
/// like the ones read from the file.
#[test]
fn planes_from_the_disk_cache_count_toward_the_limit() {
    let _disk = DISK_TESTS.lock().unwrap_or_else(PoisonError::into_inner);
    use effectcraft_render::disk_cache::DiskCache;
    let dir = std::env::temp_dir().join(format!("ec-3749-disk-union-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("union_0001.exr");
    std::fs::write(&file, picture_and_depth()).unwrap();
    let dc = DiskCache::open(dir.join("cache"), 1 << 30).unwrap();
    dc.set_read_us_per_mb(0);
    use_disk_for(&dir.to_string_lossy());
    set_disk_cache(Some(&dc));
    let path = file.to_string_lossy().to_string();
    let source = ExrSource::File(file.clone());
    let index = crate::exr_index::get(&path, &source).unwrap();
    let all = Want { all: true, ids: (0..5).collect() };
    let first = PlaneStore::new(CACHE_BUDGET).get(&path, &source, &index, &all, Use::Other).is_ok();
    dc.flush();
    // All five planes are in the disk cache (a store without them in RAM reads them back)...
    let warm = PlaneStore::new(CACHE_BUDGET).get(&path, &source, &index, &all, Use::Other).is_ok();
    let from_disk = decodes_of(&path).last().is_some_and(|d| d.from_disk);
    // ...and a request may use four.
    set_cap(&path, 4 * 8 * 8 * 4);
    let refused = PlaneStore::new(CACHE_BUDGET).get(&path, &source, &index, &all, Use::Other).err();
    set_disk_cache(None);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(first && warm && from_disk, "decoded without a limit, then read back from the disk cache");
    assert!(matches!(refused, Some(Refused::TooLarge(_))), "{refused:?}");
}

/// #3749: a plane the system cannot give is not an aborted process: `craft-exr` asks for the
/// planes' memory first and reports what it can't get, memory refused just now (not remembered,
/// not read again at once); a plane over the read's limit (the decode limit handed to it) is a
/// refusal for size with `craft-exr`'s reason.
#[test]
fn a_plane_the_system_cannot_give_is_refused_not_allocated() {
    use crate::exr_read::{ReadError, read_error};
    use craft_exr::seek::Error;
    let gone = read_error(Error::OutOfMemory("4096 bytes of planes".into()));
    assert!(matches!(&gone, ReadError::OutOfMemory(why) if why.contains("4096 bytes of planes")), "{gone:?}");
    let over = read_error(Error::LimitExceeded("the requested rows need 4096 bytes".into()));
    assert!(matches!(&over, ReadError::TooLarge(why) if why.contains("4096 bytes")), "{over:?}");
}

/// A source that counts the bytes read from it.
struct Counted<R> {
    inner: R,
    read: usize,
}

impl<R: std::io::Read> std::io::Read for Counted<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read += n;
        Ok(n)
    }
}

impl<R: std::io::Seek> std::io::Seek for Counted<R> {
    fn seek(&mut self, to: std::io::SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(to)
    }
}

/// #3749 review: one decompressed block may take 8 GiB at most, whatever memory the read may
/// use: a file whose block is larger (a data window a billion pixels wide over one large chunk,
/// so that the chunk could hold it) is refused for its size before anything is read, also with a
/// decode limit far above the block.
#[test]
fn a_block_over_the_fixed_limit_is_refused_whatever_the_decode_limit() {
    let _only = LIMIT_TESTS.lock().unwrap_or_else(PoisonError::into_inner);
    let f32s = [("R", false), ("G", false), ("B", false), ("A", false)];
    let small = write(vec![part(None, &f32s, (1, 1), Compression::ZIP1)], (1, 1));
    // One line of 2^30 - 2 pixels of four f32 channels: a block of 16 GiB...
    let mut bytes = with_data_window_x_max(small, (1 << 30) - 3);
    // ...over a chunk long enough to hold it at craft-exr's ratio (2^16 bytes a byte).
    bytes.extend(std::iter::repeat_n(0u8, 300 << 10));
    let index = crate::exr_index::build(&mut std::io::Cursor::new(bytes.as_slice()), FileStamp::Virtual).expect("indexed: the chunk can hold the block");
    // The read may use 64 GiB (the floor is tiny, the system's figure high).
    effectcraft_render::memory::set_decode_limit(Some(64 << 30));
    let request = ReadRequest { floor: 8, ..ReadRequest::full(vec![0, 1, 2, 3]) };
    let mut src = Counted { inner: std::io::Cursor::new(bytes.as_slice()), read: 0 };
    let (read, _) = crate::exr_read::read_planes(&mut src, &index, &request);
    effectcraft_render::memory::set_decode_limit(None);
    assert!(matches!(&read, Err(crate::exr_read::ReadError::TooLarge(why)) if why.contains("one block of this read may use")), "{read:?}");
    assert_eq!(src.read, 0, "refused before anything was read");
}

/// Reads of `path` fail for lack of memory when `fails` says so for the channels read; the reads
/// asked for are counted.
fn no_memory_when(path: &str, fails: impl Fn(&[usize]) -> bool + Send + Sync + 'static) -> Arc<AtomicU64> {
    let asked = Arc::new(AtomicU64::new(0));
    let counted = asked.clone();
    set_read_error(
        path,
        Some(Arc::new(move |ids: &[usize]| {
            counted.fetch_add(1, Ordering::SeqCst);
            fails(ids).then(|| crate::exr_read::ReadError::OutOfMemory("no memory just now".into()))
        })),
    );
    asked
}

/// #3749 review: memory the system refuses during a read is neither read again at once (the
/// store's retries, a copy) nor remembered: the request is refused with the reason, and the next
/// request reads again.
#[test]
fn memory_the_system_cannot_give_is_neither_retried_nor_remembered() {
    let path = "/3749-no-memory.exr";
    let source = ExrSource::Bytes(picture_and_depth().into());
    let store = PlaneStore::new(CACHE_BUDGET);
    let index = crate::exr_index::get(path, &source).unwrap();
    let want = Want { all: true, ids: (0..5).collect() };
    let asked = no_memory_when(path, |_| true);
    let refused = store.get(path, &source, &index, &want, Use::Other).err();
    assert!(matches!(&refused, Some(Refused::NoMemory(why)) if why.contains("no memory just now")), "{refused:?}");
    assert_eq!(asked.load(Ordering::SeqCst), 1, "read once, not again at once");
    let asked = no_memory_when(path, |_| false);
    assert!(store.get(path, &source, &index, &want, Use::Other).is_ok(), "not remembered: the memory is there now");
    assert_eq!(asked.load(Ordering::SeqCst), 1);
    set_read_error(path, None);
}

/// #3750 review: a decode thread that took over a queued decode the system has no memory for
/// hands the refusal to the request that queued it and answers its own request with it too: the
/// file is read once, not again at once for a copy.
#[test]
fn a_claimed_decode_without_memory_is_not_read_again() {
    let path = "/3750-claimed-no-memory.exr";
    let source = ExrSource::Bytes(picture_and_depth().into());
    let store = Arc::new(PlaneStore::new(CACHE_BUDGET));
    let index = crate::exr_index::get(path, &source).unwrap();
    let want = Want { all: true, ids: (0..5).collect() };
    let asked = no_memory_when(path, |_| true);
    let threads = crate::pool::decode_threads().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    // When the asker queues its decode: a decode thread asks for the same planes.
    {
        let (store, source, index, want, tx) = (store.clone(), source.clone(), index.clone(), want.clone(), tx.clone());
        let hook: Hook = Arc::new(move |_: &str| {
            let (store, source, index, want, tx) = (store.clone(), source.clone(), index.clone(), want.clone(), tx.clone());
            threads.spawn(move || {
                let _ = tx.send(store.get(path, &source, &index, &want, Use::Other).err());
            });
        });
        QUEUING.lock().unwrap().push((path.to_string(), hook));
    }
    // The queued job waits until that decode thread claimed it.
    let hold: Hook = Arc::new(|p: &str| {
        let claims = CLAIMS.lock().unwrap();
        let _ = CLAIM_SIGNAL.wait_timeout_while(claims, std::time::Duration::from_secs(10), |c| !c.iter().any(|c| c == p));
    });
    STARTING.lock().unwrap().push((path.to_string(), hold));
    let queued = store.get(path, &source, &index, &want, Use::Other).err();
    let claimed = rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap();
    QUEUING.lock().unwrap().retain(|(p, _)| p != path);
    STARTING.lock().unwrap().retain(|(p, _)| p != path);
    set_read_error(path, None);
    assert!(CLAIMS.lock().unwrap().iter().any(|p| p == path), "claimed");
    assert!(matches!(&claimed, Some(Refused::NoMemory(_))), "{claimed:?}");
    assert!(matches!(&queued, Some(Refused::NoMemory(_))), "{queued:?}");
    assert_eq!(asked.load(Ordering::SeqCst), 1, "read once, not again for a copy");
}

/// #3749 review: a picture whose channels the system has no memory for just now is read alone
/// without the effects' channels, and refused with the reason when even that doesn't fit; never
/// handed to the whole file's decode (which would ask for more memory, infallibly).
#[test]
fn a_picture_without_memory_is_read_alone_or_refused_never_decoded_whole() {
    let path = "/3749-no-memory-picture.exr";
    let source = ExrSource::Bytes(picture_and_depth().into());
    let op = AlphaOp::of(&Footage::default(), FloatCurve::Linear);
    // With the depth plane it is five planes, more than the system gives: the picture alone.
    no_memory_when(path, |ids| ids.len() > 4);
    let Picture::Done(Ok(img)) = picture(path, &source, op, Some(&AuxNeed::All)) else { panic!("the picture alone") };
    assert_eq!((img.width, img.height), (8, 8));
    // Not even the picture: refused with the reason, also for some of its rows.
    no_memory_when(path, |_| true);
    match picture(path, &source, op, None) {
        Picture::Done(Err(why)) => assert!(why.contains("no memory just now"), "{why}"),
        _ => panic!("refused with the reason"),
    }
    match picture_rows(path, &source, op, (0, 4), (8, 8)) {
        Picture::Done(Err(why)) => assert!(why.contains("no memory just now"), "{why}"),
        _ => panic!("rows refused with the reason"),
    }
    set_read_error(path, None);
}

/// #3749 review: only a refusal for size is decided anew when the decode limit changes; an
/// unreadable request stays refused, also when another request fails under the new limit.
#[test]
fn an_unreadable_request_stays_refused_when_the_decode_limit_changes() {
    let _only = LIMIT_TESTS.lock().unwrap_or_else(PoisonError::into_inner);
    let path = "/3749-unreadable-generation.exr";
    let source = ExrSource::Bytes(picture_and_depth().into());
    let store = PlaneStore::new(CACHE_BUDGET);
    let index = crate::exr_index::get(path, &source).unwrap();
    let asked = Arc::new(AtomicU64::new(0));
    {
        let counted = asked.clone();
        set_read_error(
            path,
            Some(Arc::new(move |_: &[usize]| {
                counted.fetch_add(1, Ordering::SeqCst);
                Some(crate::exr_read::ReadError::Unreadable("damaged".into()))
            })),
        );
    }
    let red = Want { all: false, ids: [0].into_iter().collect() };
    let unreadable = |want: &Want| matches!(store.get(path, &source, &index, want, Use::Other), Err(Refused::Unreadable));
    assert!(unreadable(&red));
    // (figures under the fixed floor: only the change counts, no other test's limit moves)
    effectcraft_render::memory::set_decode_limit(Some(1000));
    effectcraft_render::memory::set_decode_limit(Some(2000));
    assert!(unreadable(&red));
    assert_eq!(asked.load(Ordering::SeqCst), 1, "remembered across the change");
    // Another request fails under the new limit: the first refusal is still remembered.
    assert!(unreadable(&Want { all: false, ids: [4].into_iter().collect() }));
    assert!(unreadable(&red));
    assert_eq!(asked.load(Ordering::SeqCst), 2, "only the other request was read");
    effectcraft_render::memory::set_decode_limit(None);
    set_read_error(path, None);
}

/// A two-rank Cryptomatte file of `size` with a picture: "cube" and "sphere" meet along a
/// diagonal whose pixels hold both IDs (rank 00 the nearer, 55-75 %, rank 01 the other one with
/// the rest), IDs and coverage stored as 32-bit floats.
fn crypto_edges(size: (usize, usize)) -> Vec<u8> {
    use effectcraft_raster::channels3d::{crypto_float, crypto_hash};
    let (w, h) = size;
    let (cube, sphere) = (crypto_float(crypto_hash("cube")), crypto_float(crypto_hash("sphere")));
    let mut ch: Vec<Vec<f32>> = vec![vec![0.0; w * h]; 4];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let d = x as f64 + 0.7 * y as f64 - w as f64 * 0.6;
            let edge = d.abs() < 1.5;
            let (first, second) = if d < 0.0 { (cube, sphere) } else { (sphere, cube) };
            let c = if edge { 0.55 + 0.1 * ((x * 3 + y) % 3) as f32 } else { 1.0 };
            ch[0][i] = first;
            ch[1][i] = c;
            ch[2][i] = if edge { second } else { 0.0 };
            ch[3][i] = if edge { 1.0 - c } else { 0.0 };
        }
    }
    let mut list: Vec<AnyChannel<FlatSamples>> = ["CryptoObject00.R", "CryptoObject00.G", "CryptoObject00.B", "CryptoObject00.A"]
        .iter()
        .zip(ch)
        .map(|(n, v)| AnyChannel::new(*n, FlatSamples::F32(v)))
        .collect();
    for c in ["R", "G", "B", "A"] {
        list.push(AnyChannel::new(c, FlatSamples::F16(vec![f16::from_f32(0.5); w * h])));
    }
    let encoding = Encoding { compression: Compression::ZIP1, blocks: Blocks::ScanLines, line_order: LineOrder::Increasing };
    write(vec![Layer::new(size, LayerAttributes::default(), encoding, AnyChannels::sort(list.into()))], size)
}

/// The effects' view of a footage layer's channels (tests).
struct ChannelsHost(Arc<AuxChannels>);

impl effectcraft_effects::EffectHost for ChannelsHost {
    fn layer(&self, _: u64, _: bool) -> Option<effectcraft_effects::LayerPixels> {
        None
    }
    fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
        None
    }
    fn aux(&self) -> Option<Arc<AuxChannels>> {
        Some(self.0.clone())
    }
}

/// #3334, on an OpenEXR written here and read through the pool: Cryptomatte at half resolution
/// is the full-resolution output box-filtered down (2 × 2 pixels averaged, bit for bit), along
/// edges where two IDs share a pixel; the channels the effect reads are the same set for every
/// request of the frame (the GPU keeps its matte by them).
#[test]
fn cryptomatte_at_half_resolution_is_the_full_resolution_matte_box_filtered() {
    let size = (23usize, 17usize);
    let file = Temp::new("crypto-half", &crypto_edges(size));
    let f = crate::probe(&file.0).unwrap();
    let pool = crate::MediaPool::new();
    let need = AuxNeed::All;
    let aux = pool.aux_for(ItemId(1), &f, Tick::ZERO, &need).unwrap();
    assert!(Arc::ptr_eq(&aux, &pool.aux_for(ItemId(1), &f, Tick::ZERO, &need).unwrap()), "the same channels for the frame");
    assert_eq!(aux.crypto_rank_planes("CryptoObject").len(), 2);
    let host = ChannelsHost(aux);
    let spec = effectcraft_effects::find("ec.3d.cryptomatte").unwrap();
    let layer = [size.0 as f64, size.1 as f64];
    let mut params =
        effectcraft_effects::Params { values: spec.params.iter().map(|ps| (ps.id.to_string(), effectcraft_effects::default_value(ps, layer))).collect() };
    params.values.insert("selection".into(), effectcraft_project::Value::Str("cube".into()));
    params.values.insert("matteOnly".into(), effectcraft_project::Value::Bool(true));
    let run = |k: u32| {
        let ctx = effectcraft_effects::EffectCtx {
            params: &params,
            time: 0.0,
            layer_size: layer,
            seed: 0,
            adjustment: false,
            env: effectcraft_effects::EffectEnv { host: Some(&host), ..Default::default() },
        };
        let (w, h) = ((size.0 as u32).div_ceil(k), (size.1 as u32).div_ceil(k));
        (spec.render)(&ctx, effectcraft_effects::Buf { img: Image::filled(w, h, [1.0; 4]), offset: [0.0; 2], scale: 1.0 / k as f64 }).img
    };
    let (full, half) = (run(1), run(2));
    let mut partial = 0;
    for y in 0..half.height {
        for x in 0..half.width {
            let mut sum = 0.0f64;
            for sy in y * 2..y * 2 + 2 {
                for sx in x * 2..x * 2 + 2 {
                    if sx < full.width && sy < full.height {
                        sum += full.get(sx as i64, sy as i64)[0] as f64;
                    }
                }
            }
            let m = (sum / 4.0) as f32;
            assert_eq!(half.get(x as i64, y as i64), [m, m, m, 1.0], "pixel {x}, {y}");
            partial += usize::from(m > 0.0 && m < 1.0);
        }
    }
    assert!(partial > 4, "edges: {partial}");
}

/// #3334: a warm frame checks its file's identity on disk at most once: repeated requests for
/// the channels within the stamp's lifetime use the identity read before, and Reload Footage
/// (forget) reads it again.
#[test]
fn a_warm_frame_checks_its_file_at_most_once() {
    /// Removes the folder when dropped, also when an assertion fails.
    struct TempDir(std::path::PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let dir = std::env::temp_dir().join(format!("ec-3334-stamps-{}", std::process::id()));
    // (a run that stopped before its cleanup left the folder: start from an empty one)
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let _cleanup = TempDir(dir.clone());
    let file = dir.join("crypto_0001.exr");
    std::fs::write(&file, crypto_edges((8, 8))).unwrap();
    let f = crate::probe(&file).unwrap();
    let pool = crate::MediaPool::new();
    let need = AuxNeed::All;
    pool.aux_for(ItemId(1), &f, Tick::ZERO, &need).unwrap();
    let started = std::time::Instant::now();
    let before = crate::stamp_checks_under(&dir);
    for _ in 0..8 {
        pool.aux_for(ItemId(1), &f, Tick::ZERO, &need).unwrap();
        pool.aux(ItemId(1), &f, Tick::ZERO).unwrap();
    }
    let checks = crate::stamp_checks_under(&dir) - before;
    // (a slow machine may cross the stamp's lifetime once meanwhile)
    let lifetimes = (started.elapsed().as_secs_f64() / crate::exr_read::STAMP_TTL.as_secs_f64()).ceil() as usize;
    assert!(checks <= lifetimes.max(1), "{checks} identity checks for 16 requests");
    assert!(crate::exr_read::stamp_kept(&file));
    // Reload Footage: the identity read goes with the index; the file is read again at once.
    pool.forget(&f.path);
    assert!(!crate::exr_read::stamp_kept(&file), "forgotten");
    std::fs::write(&file, crypto_edges((8, 9))).unwrap();
    let again = pool.aux_for(ItemId(1), &f, Tick::ZERO, &need).unwrap();
    assert_eq!(again.height, 9, "the new file");
}
