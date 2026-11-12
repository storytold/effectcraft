//! A LUT file that changes on disk renders again (#3493): Apply Color LUT and Lumetri's Input
//! LUT name a `.cube` file; an edit to it, or a file created, deleted or repaired, shows on the
//! next render of the same frame, from warm layer caches in memory and on disk, through a precomp
//! and a layer reading that precomp, and a frame kept on disk made with the old file is refused.

use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::Session;
use crate::render::RenderOpts;
use effectcraft_project::ItemId;
use effectcraft_render::disk_cache::{DiskCache, comp_content_key, frame_key};
use effectcraft_render::reads::ReadLog;
use effectcraft_time::Tick;

/// Every input to black, or to white (1D, two entries: the same size, so only the modification
/// time tells the two apart).
const BLACK: &str = "LUT_1D_SIZE 2\n0 0 0\n0 0 0\n";
const WHITE: &str = "LUT_1D_SIZE 2\n1 1 1\n1 1 1\n";
const GREY: f32 = 128.0 / 255.0;

/// Write `text` to `path` with its modification time `secs` after the epoch.
fn write(path: &Path, text: &str, secs: u64) {
    std::fs::write(path, text).unwrap();
    let f = std::fs::File::options().write(true).open(path).unwrap();
    f.set_modified(UNIX_EPOCH + Duration::from_secs(secs)).unwrap();
}

/// A session, its layer buffers written to a disk cache in `disk` when given.
fn session(disk: Option<&Path>) -> Session {
    let mut s = Session::default();
    if let Some(dir) = disk {
        s.execute_checked("prefs.set", json!({"values": {"disk.diskCacheFolder": dir.to_string_lossy(), "disk.diskCacheEnabled": true}})).unwrap();
        s.disk_cache.as_ref().unwrap().set_min_layer_ms(0);
    }
    s
}

/// Comp "Grade": a grey solid graded by `effect` (`params` set on it). Comp "Shot" shows it as a
/// precomp layer, under a solid that copies the precomp's channels (Set Channels): that layer is
/// cached with what the precomp shows. Returns (Grade, Shot).
fn shot(s: &mut Session, effect: &str, params: &[(&str, Value)]) -> (ItemId, ItemId) {
    s.execute_checked("comp.new", json!({"name": "Shot", "width": 8, "height": 8, "frameRate": 24, "duration": 1})).unwrap();
    let shot = s.active_comp_id().unwrap();
    let plate = s.execute_checked("layer.newSolid", json!({"name": "Plate", "color": "#808080", "width": 8, "height": 8})).unwrap()["layer"].clone();
    s.execute_checked("effect.apply", json!({"layer": plate, "effect": effect})).unwrap();
    for (path, value) in params {
        s.execute_checked("prop.set", json!({"layer": plate, "path": format!("effects/#1/{path}"), "value": value})).unwrap();
    }
    let pre = s.execute_checked("layer.precompose", json!({"layers": [plate], "name": "Grade"})).unwrap();
    let grade = ItemId(pre["comp"].as_u64().unwrap());
    let copy = s.execute_checked("layer.newSolid", json!({"name": "Copy", "color": "#00ff00", "width": 8, "height": 8})).unwrap()["layer"].clone();
    s.execute_checked("effect.apply", json!({"layer": copy, "effect": "Set Channels"})).unwrap();
    for i in 1..=3 {
        s.execute_checked("prop.set", json!({"layer": copy, "path": format!("effects/#1/sourceLayer{i}"), "value": pre["layer"]})).unwrap();
    }
    (grade, shot)
}

/// The red and green of the centre of both comps' first frame.
fn colours(s: &Session, (grade, shot): (ItemId, ItemId)) -> [f32; 4] {
    let at = |comp| {
        let img = s.render(comp, Tick::ZERO, RenderOpts::default());
        img.get(i64::from(img.width / 2), i64::from(img.height / 2))
    };
    let (g, c) = (at(grade), at(shot));
    [g[0], g[1], c[0], c[1]]
}

#[track_caller]
fn assert_all(got: [f32; 4], want: f32) {
    assert!(got.iter().all(|v| (v - want).abs() < 0.01), "{got:?}, want {want}");
}

#[test]
fn an_edited_lut_file_renders_again_from_warm_caches() {
    let dir = tempfile::tempdir().unwrap();
    let lut = dir.path().join("grade.cube");
    write(&lut, BLACK, 1_000);
    let mut s = session(None);
    let comps = shot(&mut s, "Apply Color LUT", &[("lut", json!(lut.to_string_lossy()))]);
    assert_all(colours(&s, comps), 0.0);
    let misses = s.layer_cache.stats().misses;
    assert_all(colours(&s, comps), 0.0);
    assert_eq!(s.layer_cache.stats().misses, misses, "an unchanged file keeps the cached layers");
    // A frame on disk made with the file names it.
    let frames = DiskCache::open(dir.path().join("frames"), 1 << 30).unwrap();
    let key = frame_key(comp_content_key(&s.project, comps.1), 0, 1000, 0);
    let log = ReadLog::new();
    let mut r = s.renderer(RenderOpts::default());
    r.reads = Some(&log);
    let _ = r.comp_frame(comps.1, Tick::ZERO);
    let files = log.into_files();
    assert!(files.get(&lut.to_string_lossy()).is_some(), "the render notes the LUT file: {files:?}");
    frames.put_frame_now(key, 1, 1, &[0; 4], &files);
    assert!(frames.get_frame(key, None).is_some());
    let generation = effectcraft_effects::lut_generation();
    write(&lut, WHITE, 2_000);
    assert_all(colours(&s, comps), 1.0);
    assert!(effectcraft_effects::lut_generation() > generation, "the viewer's frames made before are dropped");
    assert!(frames.get_frame(key, None).is_none(), "the frame on disk made with the old file");
}

#[test]
fn an_edited_lut_file_is_not_served_from_the_disk_cache() {
    let dir = tempfile::tempdir().unwrap();
    let (lut, disk) = (dir.path().join("grade.cube"), dir.path().join("cache"));
    write(&lut, BLACK, 1_000);
    let mut s = session(Some(&disk));
    let comps = shot(&mut s, "Apply Color LUT", &[("lut", json!(lut.to_string_lossy()))]);
    assert_all(colours(&s, comps), 0.0);
    let dc = s.disk_cache.clone().unwrap();
    dc.flush();
    assert!(dc.stats().entries > 0, "layer buffers written");
    let project = s.project.clone();
    drop((s, dc));
    // A restart on the same cache, after the file changed.
    write(&lut, WHITE, 2_000);
    let mut s = session(Some(&disk));
    s.project = project;
    s.bump();
    assert_all(colours(&s, comps), 1.0);
}

#[test]
fn a_lut_file_created_deleted_or_repaired_renders_again() {
    let dir = tempfile::tempdir().unwrap();
    let lut = dir.path().join("later.cube");
    let mut s = session(None);
    let comps = shot(&mut s, "Apply Color LUT", &[("lut", json!(lut.to_string_lossy()))]);
    assert_all(colours(&s, comps), GREY);
    write(&lut, WHITE, 1_000);
    assert_all(colours(&s, comps), 1.0);
    std::fs::remove_file(&lut).unwrap();
    assert_all(colours(&s, comps), GREY);
    write(&lut, "not a LUT", 2_000);
    assert_all(colours(&s, comps), GREY);
    write(&lut, BLACK, 3_000);
    assert_all(colours(&s, comps), 0.0);
}

#[test]
fn lumetri_follows_its_input_lut_file() {
    let dir = tempfile::tempdir().unwrap();
    let lut = dir.path().join("input.cube");
    write(&lut, BLACK, 1_000);
    let mut s = session(None);
    let comps = shot(&mut s, "Lumetri Color", &[("basicCorrection/inputLut", json!(1)), ("basicCorrection/inputLutFile", json!(lut.to_string_lossy()))]);
    assert_all(colours(&s, comps), 0.0);
    write(&lut, WHITE, 2_000);
    assert_all(colours(&s, comps), 1.0);
}

/// A precomp instance whose Essential Property names another LUT file follows that file, also
/// for a layer that reads the instance through a layer parameter.
#[test]
fn an_overridden_lut_file_renders_again() {
    let dir = tempfile::tempdir().unwrap();
    let (base, overridden) = (dir.path().join("base.cube"), dir.path().join("override.cube"));
    write(&base, WHITE, 1_000);
    write(&overridden, BLACK, 1_000);
    let mut s = session(None);
    s.execute_checked("comp.new", json!({"name": "Grade", "width": 8, "height": 8, "frameRate": 24, "duration": 1})).unwrap();
    let grade = s.active_comp_id().unwrap();
    let plate = s.execute_checked("layer.newSolid", json!({"name": "Plate", "color": "#808080", "width": 8, "height": 8})).unwrap()["layer"].clone();
    s.execute_checked("effect.apply", json!({"layer": plate, "effect": "Apply Color LUT"})).unwrap();
    s.execute_checked("prop.set", json!({"layer": plate, "path": "effects/#1/lut", "value": base.to_string_lossy()})).unwrap();
    let control = s.execute_checked("essential.addProperty", json!({"layer": plate, "path": "effects/#1/lut", "name": "LUT"})).unwrap()["controls"][0].clone();
    s.execute_checked("comp.new", json!({"name": "Shot", "width": 8, "height": 8, "frameRate": 24, "duration": 1})).unwrap();
    let shot = s.active_comp_id().unwrap();
    let instance = s.execute_checked("layer.addItem", json!({"item": grade.0})).unwrap()["layer"].clone();
    s.execute_checked("essential.set", json!({"layer": instance, "control": control, "value": overridden.to_string_lossy()})).unwrap();
    let copy = s.execute_checked("layer.newSolid", json!({"name": "Copy", "color": "#00ff00", "width": 8, "height": 8})).unwrap()["layer"].clone();
    s.execute_checked("effect.apply", json!({"layer": copy, "effect": "Set Channels"})).unwrap();
    for i in 1..=3 {
        s.execute_checked("prop.set", json!({"layer": copy, "path": format!("effects/#1/sourceLayer{i}"), "value": instance})).unwrap();
    }
    let centre = |s: &Session| {
        let img = s.render(shot, Tick::ZERO, RenderOpts::default());
        let p = img.get(i64::from(img.width / 2), i64::from(img.height / 2));
        [p[0], p[1], p[0], p[1]]
    };
    assert_all(centre(&s), 0.0);
    write(&overridden, WHITE, 2_000);
    assert_all(centre(&s), 1.0);
}

/// #3538: OCIO File Transform and a custom OCIO config (and the LUT it names) follow their files
/// through warm layer caches, and a render notes the config's LUT for the disk cache.
#[test]
fn ocio_files_render_again_from_warm_caches() {
    let dir = tempfile::tempdir().unwrap();
    let (lut, config, grade) = (dir.path().join("file.cube"), dir.path().join("config.ocio"), dir.path().join("luts/grade.cube"));
    std::fs::create_dir_all(dir.path().join("luts")).unwrap();
    write(&lut, BLACK, 1_000);
    write(&grade, BLACK, 1_000);
    let text = "ocio_profile_version: 2\nsearch_path: luts\nroles:\n  scene_linear: lin\ncolorspaces:\n  - !<ColorSpace>\n    name: lin\n  - !<ColorSpace>\n    name: graded\n    to_reference: !<FileTransform> {src: grade.cube, interpolation: linear}\n";
    write(&config, text, 1_000);
    let mut s = session(None);
    let file = shot(&mut s, "OCIO File Transform", &[("file", json!(lut.to_string_lossy()))]);
    let custom = shot(
        &mut s,
        "OCIO Color Space Transform",
        &[("config", json!(1)), ("configFile", json!(config.to_string_lossy())), ("sourceName", json!("graded")), ("destinationName", json!("lin"))],
    );
    // A render notes the LUT the config names (a frame on disk is made of it too).
    let log = ReadLog::new();
    let mut r = s.renderer(RenderOpts::default());
    r.reads = Some(&log);
    let _ = r.comp_frame(custom.1, Tick::ZERO);
    let files = log.into_files();
    assert!(files.get(&grade.to_string_lossy()).is_some(), "the render notes the config's LUT: {files:?}");
    assert_all(colours(&s, file), 0.0);
    assert_all(colours(&s, custom), 0.0);
    let misses = s.layer_cache.stats().misses;
    assert_all(colours(&s, file), 0.0);
    assert_all(colours(&s, custom), 0.0);
    assert_eq!(s.layer_cache.stats().misses, misses, "unchanged files keep the cached layers");
    let generation = effectcraft_effects::lut_generation();
    write(&lut, WHITE, 2_000);
    assert_all(colours(&s, file), 1.0);
    write(&grade, WHITE, 2_000);
    assert_all(colours(&s, custom), 1.0);
    assert!(effectcraft_effects::lut_generation() > generation, "the viewer's frames made before are dropped");
}
