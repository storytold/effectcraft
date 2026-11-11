//! Apply Color LUT and Color Grading follow the `.cube` file they name (#3493): a file edited,
//! created, deleted or repaired on disk renders again on the next frame, without an edit or a
//! purge, through the layer cache in memory and on disk, a layer that reads a precomp of it, and
//! a frame kept on disk.

use std::path::Path;

use serde_json::json;

use crate::Session;
use crate::render::RenderOpts;
use effectcraft_project::ItemId;
use effectcraft_render::disk_cache::{DiskCache, comp_content_key, frame_key};
use effectcraft_render::reads::ReadLog;
use effectcraft_time::Tick;

const BLACK: &str = "LUT_1D_SIZE 2\n0 0 0\n0 0 0\n";
const WHITE: &str = "LUT_1D_SIZE 2\n1 1 1\n1 1 1\n";

/// Write `text` to `path`, modified `secs` after the epoch (set, so an edit to the same size is
/// told apart whatever the clock's resolution).
fn write_lut(path: &Path, text: &str, secs: u64) {
    std::fs::write(path, text).unwrap();
    let f = std::fs::File::options().write(true).open(path).unwrap();
    f.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs)).unwrap();
}

/// A session, with its disk cache in `disk` when given (every layer buffer written).
fn session(disk: Option<&Path>) -> Session {
    let mut s = Session::default();
    if let Some(dir) = disk {
        s.execute_checked("prefs.set", json!({"values": {"disk.diskCacheFolder": dir.to_string_lossy(), "disk.diskCacheEnabled": true}})).unwrap();
        s.disk_cache.as_ref().unwrap().set_min_layer_ms(0);
    }
    s
}

/// Comp "Inner": a grey solid through `effect` with `params`; pre-composed into "Main", where a
/// solid on top takes its colour from the precomp (Set Channels), so it is cached with what the
/// precomp shows. Returns the two comps.
fn graded(s: &mut Session, effect: &str, params: &[(&str, serde_json::Value)]) -> (ItemId, ItemId) {
    s.execute_checked("comp.new", json!({"name": "Main", "width": 16, "height": 16, "frameRate": 30, "duration": 1})).unwrap();
    let main = s.active_comp_id().unwrap();
    let solid = s.execute_checked("layer.newSolid", json!({"name": "Graded", "color": "#808080", "width": 16, "height": 16})).unwrap()["layer"].clone();
    s.execute_checked("effect.apply", json!({"layer": solid, "effect": effect})).unwrap();
    for (path, value) in params {
        s.execute_checked("prop.set", json!({"layer": solid, "path": format!("effects/#1/{path}"), "value": value})).unwrap();
    }
    let pre = s.execute_checked("layer.precompose", json!({"layers": [solid], "name": "Inner"})).unwrap();
    let (inner, precomp) = (ItemId(pre["comp"].as_u64().unwrap()), pre["layer"].clone());
    let reader = s.execute_checked("layer.newSolid", json!({"name": "Reader", "color": "#ff00ff", "width": 16, "height": 16})).unwrap()["layer"].clone();
    s.execute_checked("effect.apply", json!({"layer": reader, "effect": "Set Channels"})).unwrap();
    for i in 1..=3 {
        s.execute_checked("prop.set", json!({"layer": reader, "path": format!("effects/#1/sourceLayer{i}"), "value": precomp})).unwrap();
    }
    (inner, main)
}

/// The red of the centre pixel of `comp`'s first frame.
fn red(s: &Session, comp: ItemId) -> f32 {
    let img = s.render(comp, Tick::ZERO, RenderOpts::default());
    img.get(i64::from(img.width / 2), i64::from(img.height / 2))[0]
}

/// The red of both comps: the LUT layer's own, and the layer reading its precomp.
fn reds(s: &Session, (inner, main): (ItemId, ItemId)) -> [f32; 2] {
    [red(s, inner), red(s, main)]
}

fn near(got: [f32; 2], want: f32) -> bool {
    got.iter().all(|v| (v - want).abs() < 0.01)
}

const GREY: f32 = 128.0 / 255.0;

/// The same frame, rendered from warm caches, shows the edited file: the layer cache's entries
/// made with the old file are not used, a frame on disk made with it is refused, while an
/// unchanged file keeps every entry.
#[test]
fn an_edited_lut_file_renders_again_from_warm_caches() {
    let dir = tempfile::tempdir().unwrap();
    let lut = dir.path().join("grade.cube");
    write_lut(&lut, BLACK, 1_000);
    let mut s = session(None);
    let comps = graded(&mut s, "Apply Color LUT", &[("lut", json!(lut.to_string_lossy()))]);
    assert!(near(reds(&s, comps), 0.0), "{:?}", reds(&s, comps));
    let misses = s.layer_cache.stats().misses;
    assert!(near(reds(&s, comps), 0.0));
    assert_eq!(s.layer_cache.stats().misses, misses, "an unchanged LUT file keeps its cache entries");
    // A frame on disk lists the file it was made with.
    let frames = DiskCache::open(dir.path().join("frames"), 1 << 30).unwrap();
    let key = frame_key(comp_content_key(&s.project, comps.1), 0, 1000, 0);
    let log = ReadLog::new();
    let mut r = s.renderer(RenderOpts::default());
    r.reads = Some(&log);
    let _ = r.comp_frame(comps.1, Tick::ZERO);
    frames.put_frame_now(key, 1, 1, &[0; 4], &log.into_files());
    assert!(frames.get_frame(key, None).is_some());
    // Edited to the same size, nothing else changed.
    write_lut(&lut, WHITE, 2_000);
    assert!(near(reds(&s, comps), 1.0), "{:?}", reds(&s, comps));
    assert!(frames.get_frame(key, None).is_none(), "a frame on disk made with the old file");
}

/// A restart on the same disk cache doesn't use the layer buffers made with the old file.
#[test]
fn an_edited_lut_file_renders_again_from_the_disk_cache() {
    let dir = tempfile::tempdir().unwrap();
    let (lut, disk) = (dir.path().join("grade.cube"), dir.path().join("cache"));
    write_lut(&lut, BLACK, 1_000);
    let mut s = session(Some(&disk));
    let comps = graded(&mut s, "Apply Color LUT", &[("lut", json!(lut.to_string_lossy()))]);
    assert!(near(reds(&s, comps), 0.0));
    let dc = s.disk_cache.clone().unwrap();
    dc.flush();
    assert!(dc.stats().entries > 0, "layer buffers written");
    let project = s.project.clone();
    drop((s, dc));
    write_lut(&lut, WHITE, 2_000);
    let mut s = session(Some(&disk));
    s.project = project;
    s.bump();
    assert!(near(reds(&s, comps), 1.0), "{:?}", reds(&s, comps));
}

/// A file named before it exists applies once it is created, and stops when it is deleted.
#[test]
fn a_lut_file_created_or_deleted_renders_again() {
    let dir = tempfile::tempdir().unwrap();
    let lut = dir.path().join("later.cube");
    let mut s = session(None);
    let comps = graded(&mut s, "Apply Color LUT", &[("lut", json!(lut.to_string_lossy()))]);
    assert!(near(reds(&s, comps), GREY), "{:?}", reds(&s, comps));
    write_lut(&lut, WHITE, 1_000);
    assert!(near(reds(&s, comps), 1.0), "{:?}", reds(&s, comps));
    std::fs::remove_file(&lut).unwrap();
    assert!(near(reds(&s, comps), GREY), "{:?}", reds(&s, comps));
}

/// A file that failed to parse applies once it is repaired.
#[test]
fn a_repaired_lut_file_renders_again() {
    let dir = tempfile::tempdir().unwrap();
    let lut = dir.path().join("broken.cube");
    write_lut(&lut, "not a lut", 1_000);
    let mut s = session(None);
    let comps = graded(&mut s, "Apply Color LUT", &[("lut", json!(lut.to_string_lossy()))]);
    assert!(near(reds(&s, comps), GREY), "{:?}", reds(&s, comps));
    write_lut(&lut, WHITE, 2_000);
    assert!(near(reds(&s, comps), 1.0), "{:?}", reds(&s, comps));
}

/// Color Grading's Input LUT and Look files are followed the same way.
#[test]
fn color_grading_lut_files_render_again() {
    let dir = tempfile::tempdir().unwrap();
    let (input, look) = (dir.path().join("input.cube"), dir.path().join("look.cube"));
    write_lut(&input, BLACK, 1_000);
    let mut s = session(None);
    let params = [("basicCorrection/inputLut", json!(1)), ("basicCorrection/inputLutFile", json!(input.to_string_lossy()))];
    let comps = graded(&mut s, "Color Grading", &params);
    assert!(near(reds(&s, comps), 0.0), "{:?}", reds(&s, comps));
    write_lut(&input, WHITE, 2_000);
    assert!(near(reds(&s, comps), 1.0), "{:?}", reds(&s, comps));
    let mut s = session(None);
    write_lut(&look, WHITE, 1_000);
    let comps = graded(&mut s, "Color Grading", &[("creative/look", json!(1)), ("creative/lookFile", json!(look.to_string_lossy()))]);
    assert!(near(reds(&s, comps), 1.0), "{:?}", reds(&s, comps));
    write_lut(&look, BLACK, 2_000);
    assert!(near(reds(&s, comps), 0.0), "{:?}", reds(&s, comps));
}

/// The frames kept in memory are keyed by the project alone: the footage watcher's stamps (here
/// `footage.reloadChanged`, its pass on the spot) drop them once a LUT file changes, goes away or
/// comes back.
#[test]
fn a_changed_lut_file_drops_the_frames_kept_in_memory() {
    let dir = tempfile::tempdir().unwrap();
    let lut = dir.path().join("watched.cube");
    write_lut(&lut, BLACK, 1_000);
    let mut s = session(None);
    graded(&mut s, "Apply Color LUT", &[("lut", json!(lut.to_string_lossy()))]);
    let look = |s: &mut Session| {
        s.events.clear();
        let r = s.execute_checked("footage.reloadChanged", json!({})).unwrap();
        (r.get("luts").and_then(serde_json::Value::as_u64), s.events.iter().filter(|e| matches!(e, crate::Event::PurgeCaches)).count())
    };
    assert_eq!(look(&mut s), (None, 0), "a file seen for the first time is only remembered");
    assert_eq!(look(&mut s), (None, 0), "an unchanged file");
    write_lut(&lut, WHITE, 2_000);
    assert_eq!(look(&mut s), (Some(1), 1), "an edited file");
    std::fs::remove_file(&lut).unwrap();
    assert_eq!(look(&mut s), (Some(1), 1), "a deleted file");
    write_lut(&lut, WHITE, 3_000);
    assert_eq!(look(&mut s), (Some(1), 1), "a file created again");
    assert_eq!(look(&mut s), (None, 0));
}

/// `.cube` text in the parameter is its own content: it renders, and stays cached.
#[test]
fn an_inline_lut_stays_cached() {
    let mut s = session(None);
    let comps = graded(&mut s, "Apply Color LUT", &[("lut", json!(WHITE))]);
    assert!(near(reds(&s, comps), 1.0), "{:?}", reds(&s, comps));
    let misses = s.layer_cache.stats().misses;
    assert!(near(reds(&s, comps), 1.0));
    assert_eq!(s.layer_cache.stats().misses, misses);
}
