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
