//! The files a render reads, as the media pool reports them (#3184): exactly the files the frame
//! was made of, also when its pixels come from memory.

use std::path::{Path, PathBuf};

use effectcraft_color::Label;
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{Comp, Footage, FootageKind, ItemId, ItemKind, Layer, LayerSource, Project, Solid, Value};
use effectcraft_render::reads::{FileStamp, ReadFiles, ReadLog};
use effectcraft_render::{LayerCache, RenderOpts, Renderer};
use effectcraft_time::FrameRate;

use crate::MediaPool;

const RATE: FrameRate = FrameRate::FPS_30;
const SIZE: (u32, u32) = (16, 8);

/// A fresh folder for test `name` (on the build drive, not the system temp folder).
fn folder(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../target/test-reads-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A PNG at `path`, a flat grey of `level`.
fn png(path: &Path, level: u8) -> String {
    image::RgbaImage::from_pixel(SIZE.0, SIZE.1, image::Rgba([level, level, level, 255])).save(path).unwrap();
    path.to_string_lossy().to_string()
}

/// An image sequence of `n` PNG frames in `dir`.
fn sequence(dir: &Path, n: usize) -> Footage {
    let files: Vec<String> = (0..n).map(|i| png(&dir.join(format!("seq_{i:04}.png")), 20 + i as u8 * 10)).collect();
    let mut f = Footage { kind: FootageKind::Sequence, sequence: files, frame_rate: RATE, ..crate::probe(dir.join("seq_0000.png")).unwrap() };
    f.sync_sequence_duration();
    f
}

/// A still footage item of `path`.
fn still(path: &Path, level: u8) -> Footage {
    png(path, level);
    crate::probe(path).unwrap()
}

fn comp() -> Comp {
    Comp::new(SIZE.0, SIZE.1, RATE, RATE.tick_of(10))
}

fn footage_layer(p: &mut Project, comp: &Comp, name: &str, f: Footage) -> Layer {
    let item = p.add_item(name, Label::Aqua, None, ItemKind::Footage(f));
    build::layer(p, comp, name, LayerSource::Footage { item }, SIZE, Some(comp.duration))
}

fn add_comp(p: &mut Project, name: &str, mut comp: Comp, layers: Vec<Layer>) -> ItemId {
    comp.layers = layers.into();
    p.add_item(name, Label::Sandstone, None, ItemKind::Comp(comp.into()))
}

/// Render frame `frame` of `comp` with `pool` and `cache`: the picture's top-left pixel and the
/// files the render reports.
fn render(p: &Project, pool: &MediaPool, cache: &LayerCache, comp: ItemId, frame: i64) -> ([f32; 4], ReadFiles) {
    let log = ReadLog::new();
    let mut r = Renderer::new(p, pool, RenderOpts::default());
    r.cache = Some(cache);
    r.reads = Some(&log);
    let img = r.comp_frame(comp, RATE.tick_of(frame));
    (img.get(0, 0), log.into_files())
}

fn paths(files: &ReadFiles) -> Vec<String> {
    files.files().iter().map(|f| f.path.to_string()).collect()
}

/// Every listed file has the identity it has on disk now.
fn assert_stamps_match_disk(files: &ReadFiles) {
    for f in files.files() {
        assert_eq!(Some(f.stamp), FileStamp::native(&f.path), "{}", f.path);
    }
}

/// (a) One frame of a comp with an image-sequence layer reads that frame's file and nothing else
/// of the sequence; a large sequence in the project that no layer uses adds nothing, not even a
/// look at its files.
#[test]
fn a_sequence_frame_reports_only_its_own_file() {
    let dir = folder("sequence");
    let unused_dir = dir.join("unused");
    let mut p = Project::default();
    let c = comp();
    let seq = sequence(&dir, 5);
    let (frame_2, frame_4) = (seq.sequence[2].clone(), seq.sequence[4].clone());
    let layer = footage_layer(&mut p, &c, "Seq", seq);
    // 5000 frames nobody shows (they don't even exist).
    let unused = Footage {
        kind: FootageKind::Sequence,
        sequence: (0..5000).map(|i| unused_dir.join(format!("big_{i:05}.exr")).to_string_lossy().to_string()).collect(),
        frame_rate: RATE,
        width: SIZE.0,
        height: SIZE.1,
        has_video: true,
        ..Default::default()
    };
    p.add_item("Unused", Label::Aqua, None, ItemKind::Footage(unused));
    let cid = add_comp(&mut p, "C", c, vec![layer]);
    let (pool, cache) = (MediaPool::new(), LayerCache::default());
    let opened = crate::opens_under(&dir);
    let (px, files) = render(&p, &pool, &cache, cid, 2);
    assert!((px[0] - 40.0 / 255.0).abs() < 0.01, "frame 2 shows: {px:?}");
    assert_eq!(paths(&files), [frame_2]);
    assert!(files.is_complete());
    assert_stamps_match_disk(&files);
    assert_eq!(crate::opens_under(&dir) - opened, 1, "one file opened: the frame's");
    assert_eq!(crate::opens_under(&unused_dir), 0, "the unused sequence was not looked at");
    // Another frame reads its own file, also through the viewer's wrapper of the pool.
    let viewer = effectcraft_render::CancellableFootage { inner: &pool, cancel: Default::default() };
    let log = ReadLog::new();
    let mut r = Renderer::new(&p, &viewer, RenderOpts::default());
    r.cache = Some(&cache);
    r.reads = Some(&log);
    r.comp_frame(cid, RATE.tick_of(4));
    let files = log.into_files();
    assert_eq!(paths(&files), [frame_4]);
    assert!(files.is_complete());
    let _ = std::fs::remove_dir_all(&dir);
}

/// (b) A precomp reports the files of what it draws, and an effect that reads another layer
/// reports that layer's files (the layer itself is hidden: its pixels come only through the
/// effect).
#[test]
fn precomps_and_layers_read_by_effects_report_their_files() {
    let dir = folder("nested");
    let mut p = Project::default();
    let c = comp();
    let seq = sequence(&dir, 4);
    let nested_frame = seq.sequence[3].clone();
    let inner_layer = footage_layer(&mut p, &c, "Seq", seq);
    let inner = add_comp(&mut p, "Inner", c.clone(), vec![inner_layer]);
    let pre = build::layer(&mut p, &c, "Inner", LayerSource::Comp { item: inner }, SIZE, Some(c.duration));
    let matte_path = dir.join("matte.png");
    let mut matte = footage_layer(&mut p, &c, "Matte", still(&matte_path, 255));
    matte.switches.video = false;
    let sid = p.add_item("S", Label::Red, None, ItemKind::Solid(Solid { color: [1.0, 0.0, 0.0], width: SIZE.0, height: SIZE.1, pixel_aspect: 1.0 }));
    let mut solid = build::layer(&mut p, &c, "S", LayerSource::Solid { item: sid }, SIZE, Some(c.duration));
    let spec = effectcraft_effects::find("ec.channel.setmatte").unwrap();
    let mut next = p.next_id;
    let mut fx = effectcraft_effects::instantiate(spec, &mut Ids(&mut next), spec.name, [SIZE.0 as f64, SIZE.1 as f64]);
    p.next_id = next;
    fx.prop_mut("takeMatteFromLayer").unwrap().value = Value::Layer(Some(matte.id.0));
    solid.props.sub_mut("effects").unwrap().children.push(fx.into());
    let cid = add_comp(&mut p, "Outer", c, vec![solid, matte, pre]);
    let (pool, cache) = (MediaPool::new(), LayerCache::default());
    let (_, files) = render(&p, &pool, &cache, cid, 3);
    let mut want = vec![matte_path.to_string_lossy().to_string(), nested_frame];
    want.sort();
    assert_eq!(paths(&files), want);
    assert!(files.is_complete());
    assert_stamps_match_disk(&files);
    let _ = std::fs::remove_dir_all(&dir);
}

/// (c) A frame rendered again comes from memory (the sequence frame from the media pool, the
/// still layer from the layer cache) and reports the same files, with the identities they had
/// when read, without opening anything: here the files are even gone by then.
#[test]
fn a_frame_served_from_memory_reports_its_files_without_touching_them() {
    let dir = folder("memory");
    let mut p = Project::default();
    let c = comp();
    // The sequence at half opacity over the still: both show.
    let mut seq = footage_layer(&mut p, &c, "Seq", sequence(&dir, 3));
    seq.props.prop_mut("transform/opacity").unwrap().value = Value::Scalar(50.0);
    let plate = footage_layer(&mut p, &c, "Plate", still(&dir.join("plate.png"), 128));
    let cid = add_comp(&mut p, "C", c, vec![seq, plate]);
    let (pool, cache) = (MediaPool::new(), LayerCache::default());
    let (px, first) = render(&p, &pool, &cache, cid, 1);
    assert_eq!(first.len(), 2, "{:?}", paths(&first));
    assert_stamps_match_disk(&first);
    std::fs::remove_dir_all(&dir).unwrap();
    let opened = crate::opens_under(&dir);
    let hits = cache.stats().hits;
    let (again_px, again) = render(&p, &pool, &cache, cid, 1);
    assert_eq!(again_px, px, "the same picture");
    assert_eq!(again, first, "the same files, as they were when read");
    assert!(cache.stats().hits > hits, "the still layer came from the layer cache");
    // The media pool alone (a new layer cache) reports them too.
    let (_, from_pool) = render(&p, &pool, &LayerCache::default(), cid, 1);
    assert_eq!(from_pool, first);
    assert_eq!(crate::opens_under(&dir), opened, "nothing was opened on disk");
}

// ---------------------------------------------------------------- layer entries on disk (#3184)

use std::sync::Arc;

use effectcraft_render::disk_cache::{DiskCache, Kind};

/// A "session": a new media pool and layer cache over the disk cache in `cache` (opened anew,
/// as after a restart), writing every layer buffer however quick to render.
fn session(cache: &Path) -> (MediaPool, LayerCache, Arc<DiskCache>) {
    let dc = DiskCache::open(cache, 1 << 30).unwrap();
    dc.set_min_layer_ms(0);
    let lc = LayerCache::default();
    lc.set_disk(Some(dc.clone()));
    (MediaPool::new(), lc, dc)
}

/// Give `path` a later modification time, so a rewrite within the clock's resolution counts.
fn touch_later(path: &Path) {
    let t = std::fs::metadata(path).unwrap().modified().unwrap() + std::time::Duration::from_secs(7);
    std::fs::File::options().append(true).open(path).unwrap().set_modified(t).unwrap();
}

/// Two still plates (the upper one at half opacity, so both show), and a 5000-frame sequence
/// in the project that no layer uses (its files don't even exist).
fn plates(dir: &Path) -> (Project, ItemId, PathBuf) {
    let mut p = Project::default();
    let c = comp();
    let a = dir.join("a.png");
    let mut top = footage_layer(&mut p, &c, "A", still(&a, 60));
    top.props.prop_mut("transform/opacity").unwrap().value = Value::Scalar(50.0);
    let bottom = footage_layer(&mut p, &c, "B", still(&dir.join("b.png"), 200));
    let unused = dir.join("unused");
    let seq = Footage {
        kind: FootageKind::Sequence,
        sequence: (0..5000).map(|i| unused.join(format!("big_{i:05}.exr")).to_string_lossy().to_string()).collect(),
        frame_rate: RATE,
        width: SIZE.0,
        height: SIZE.1,
        has_video: true,
        ..Default::default()
    };
    p.add_item("Unused", Label::Aqua, None, ItemKind::Footage(seq));
    let cid = add_comp(&mut p, "C", c, vec![top, bottom]);
    (p, cid, a)
}

/// Correct and no scan: after a restart, only the layer buffer whose source file changed is
/// rendered again (only that file is opened); the other comes from disk with its file list, so
/// the frame's list is complete again (its frame entry can be stored). The disk cache checks
/// exactly the files its entries list, never the unused sequence. The re-rendered buffer
/// replaces its entry: the next restart opens nothing.
#[test]
fn a_changed_file_renders_only_the_layer_buffers_that_read_it_again() {
    let dir = folder("layer-disk");
    let (src, cache) = (dir.join("src"), dir.join("cache"));
    std::fs::create_dir_all(&src).unwrap();
    let (p, cid, a) = plates(&src);
    let (pool, lc, dc) = session(&cache);
    let (px, files) = render(&p, &pool, &lc, cid, 0);
    assert!(files.is_complete() && files.len() == 2, "{:?}", paths(&files));
    dc.flush();
    let entries = dc.keys(Kind::Layer).len();
    assert!(entries >= 2, "both plates' buffers are on disk: {entries}");
    drop((pool, lc, dc));

    // Restart without a change: everything from disk, nothing opened, a complete list.
    let (pool, lc, dc) = session(&cache);
    let opened = crate::opens_under(&src);
    let (again, files_again) = render(&p, &pool, &lc, cid, 0);
    assert_eq!(again, px);
    assert_eq!(files_again, files, "the files of the entries, as they were read");
    assert!(files_again.is_complete(), "a frame made of buffers from disk can be stored");
    assert_eq!(crate::opens_under(&src), opened, "nothing opened");
    let checks = dc.file_checks();
    assert!(checks >= 2 && checks as usize <= entries, "one check per entry used: {checks} of {entries}");
    drop((pool, lc, dc));

    // a.png gets new pixels: only the top plate renders again.
    png(&a, 120);
    touch_later(&a);
    let (pool, lc, dc) = session(&cache);
    let opened = crate::opens_under(&src);
    let (changed, files_changed) = render(&p, &pool, &lc, cid, 0);
    assert_ne!(changed, px, "the new pixels show");
    assert_eq!(crate::opens_under(&src) - opened, 1, "only the changed file is read");
    assert!(files_changed.is_complete() && files_changed.len() == 2);
    assert_stamps_match_disk(&files_changed);
    assert!(dc.file_checks() as usize <= entries, "only listed files are checked");
    assert_eq!(crate::opens_under(&src.join("unused")), 0);
    dc.flush();
    drop((pool, lc, dc));

    // Its entry was replaced: the next restart opens nothing and shows the new pixels.
    let (pool, lc, _dc) = session(&cache);
    let opened = crate::opens_under(&src);
    let (last, _) = render(&p, &pool, &lc, cid, 0);
    assert_eq!(last, changed);
    assert_eq!(crate::opens_under(&src), opened, "both from disk");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Two projects whose footage items have the same ids but name different files never share a
/// layer entry: the layer key names the item's path and settings.
#[test]
fn projects_with_the_same_item_ids_never_share_a_layer_entry() {
    let dir = folder("layer-disk-projects");
    let (src, cache) = (dir.join("src"), dir.join("cache"));
    std::fs::create_dir_all(&src).unwrap();
    let mut p = Project::default();
    let c = comp();
    let layer = footage_layer(&mut p, &c, "Plate", still(&src.join("one.png"), 40));
    let LayerSource::Footage { item } = layer.source else { panic!("a footage layer") };
    let cid = add_comp(&mut p, "C", c, vec![layer]);
    let mut other = p.clone();
    other.item_mut(item).unwrap().kind = ItemKind::Footage(still(&src.join("two.png"), 220));
    let (pool, lc, dc) = session(&cache);
    let (one, _) = render(&p, &pool, &lc, cid, 0);
    dc.flush();
    drop((pool, lc, dc));
    let (pool, lc, dc) = session(&cache);
    let hits = dc.stats().hits;
    let (two, files) = render(&other, &pool, &lc, cid, 0);
    assert!((two[0] - 220.0 / 255.0).abs() < 0.01, "the other project's own file shows: {two:?}");
    assert_ne!(two, one);
    assert_eq!(dc.stats().hits, hits, "no entry of the first project is used");
    assert_eq!(paths(&files), [src.join("two.png").to_string_lossy().to_string()]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Safe: a damaged layer entry, a source file that is missing, one replaced by something
/// unreadable (a folder), and one back with other pixels never serve a stale buffer after a
/// restart; the damaged entry is rendered again, replaced and then used.
#[test]
fn missing_unreadable_files_and_damaged_layer_entries_never_serve_stale_buffers() {
    let dir = folder("layer-disk-safe");
    let (src, cache) = (dir.join("src"), dir.join("cache"));
    std::fs::create_dir_all(&src).unwrap();
    let mut p = Project::default();
    let c = comp();
    let path = src.join("plate.png");
    let layer = footage_layer(&mut p, &c, "Plate", still(&path, 90));
    let cid = add_comp(&mut p, "C", c, vec![layer]);
    let (pool, lc, dc) = session(&cache);
    let (shown, _) = render(&p, &pool, &lc, cid, 0);
    assert!(shown[3] > 0.99, "the plate shows: {shown:?}");
    dc.flush();
    drop((pool, lc, dc));

    // A damaged entry: a miss, deleted, rendered again and stored anew.
    let layers = cache.join("v1").join(Kind::Layer.dir());
    for shard in std::fs::read_dir(&layers).unwrap().flatten() {
        for f in std::fs::read_dir(shard.path()).unwrap().flatten() {
            let mut d = std::fs::read(f.path()).unwrap();
            let n = d.len() / 2;
            d[n] ^= 0x5a;
            std::fs::write(f.path(), d).unwrap();
        }
    }
    let (pool, lc, dc) = session(&cache);
    let opened = crate::opens_under(&src);
    let (again, files) = render(&p, &pool, &lc, cid, 0);
    assert_eq!(again, shown);
    assert_eq!(crate::opens_under(&src) - opened, 1, "read again, not served from the damaged entry");
    assert!(files.is_complete());
    dc.flush();
    drop((pool, lc, dc));
    let (pool, lc, dc) = session(&cache);
    let hits = dc.stats().hits;
    assert_eq!(render(&p, &pool, &lc, cid, 0).0, shown);
    assert!(dc.stats().hits > hits, "the rewritten entry is used");
    drop((pool, lc, dc));

    // The file is gone: not the old plate.
    std::fs::remove_file(&path).unwrap();
    let (pool, lc, dc) = session(&cache);
    let hits = dc.stats().hits;
    let (gone, files) = render(&p, &pool, &lc, cid, 0);
    assert!(gone[3] < 0.01, "a missing file shows nothing, never the cached plate: {gone:?}");
    assert_eq!(dc.stats().hits, hits);
    assert!(files.files().iter().all(|f| f.stamp == FileStamp::Missing), "{files:?}");
    dc.flush();
    drop((pool, lc, dc));

    // Something unreadable at its path (a folder): still not the old plate.
    std::fs::create_dir(&path).unwrap();
    let (pool, lc, _dc) = session(&cache);
    let (unreadable, _) = render(&p, &pool, &lc, cid, 0);
    assert!(unreadable[3] < 0.01, "{unreadable:?}");
    std::fs::remove_dir(&path).unwrap();

    // Back with other pixels: those show, not the entry made of the old file.
    png(&path, 240);
    touch_later(&path);
    let (pool, lc, _dc) = session(&cache);
    let (back, _) = render(&p, &pool, &lc, cid, 0);
    assert!((back[0] - 240.0 / 255.0).abs() < 0.01, "{back:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
