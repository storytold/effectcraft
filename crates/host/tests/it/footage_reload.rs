//! Footage changed on disk by another app, through the fully wired session (media decoding
//! included): File ▸ Reload Footage shows the new pixels and keeps a Photoshop layer item on its
//! layer, and `footage.reloadChanged` (Settings ▸ Import ▸ Automatically Reload Footage) finds
//! changed files by itself.

use effectcraft_engine::render::RenderOpts;
use effectcraft_engine::{Session, footage_reload};
use effectcraft_project::{Footage, ItemId, ItemKind};
use effectcraft_psd::Rect;
use effectcraft_psd::write::*;
use effectcraft_time::Tick;
use serde_json::json;

fn tmp(name: &str) -> String {
    let d = std::env::temp_dir().join(format!("effectcraft-host-reload-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name).to_string_lossy().to_string()
}

const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

/// A one-colour document; `extra` adds a hidden layer, so the file's size changes too.
fn flat(rgba: [f32; 4], extra: bool) -> Vec<u8> {
    let mut d = WDoc::new(64, 48);
    d.layers = vec![WLayer::solid("Background", Rect::new(0, 0, 64, 48), rgba)];
    if extra {
        let mut hidden = WLayer::solid("Hidden", Rect::new(0, 0, 8, 8), RED);
        hidden.hidden = true;
        d.layers.push(hidden);
    }
    d.composite = Some(vec![rgba; 64 * 48]);
    write(&d)
}

/// Write `bytes` to `path` so its modification time moves even on coarse file systems.
fn save(path: &str, bytes: &[u8]) {
    std::thread::sleep(std::time::Duration::from_millis(30));
    std::fs::write(path, bytes).unwrap();
}

fn footage(s: &Session, id: ItemId) -> Footage {
    match &s.project.item(id).unwrap().kind {
        ItemKind::Footage(f) => f.clone(),
        k => panic!("not footage: {k:?}"),
    }
}

/// The colour (straight RGB) at `(x, y)` of comp `cid` at time 0.
fn pixel(s: &Session, cid: ItemId, x: usize, y: usize) -> [f32; 3] {
    let img = s.render(cid, Tick::ZERO, RenderOpts::default());
    let p = img.data[y * img.width as usize + x];
    [p[0], p[1], p[2]].map(|c| if p[3] > 0.0 { c / p[3] } else { 0.0 })
}

fn near(a: [f32; 3], b: [f32; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.02)
}

/// A flat file as footage in a comp of its size: (session, footage item, comp).
fn flat_in_comp(path: &str) -> (Session, ItemId, ItemId) {
    let mut s = effectcraft_host::session();
    let r = s.execute_checked("file.import", json!({"paths": [path]})).unwrap();
    let item = ItemId(r["items"][0].as_u64().unwrap());
    s.execute_checked("comp.new", json!({"name": "C", "width": 64, "height": 48, "frameRate": 30, "duration": 1})).unwrap();
    s.execute_checked("layer.addItem", json!({"item": item.0})).unwrap();
    let cid = s.active_comp_id().unwrap();
    (s, item, cid)
}

#[test]
fn reload_footage_shows_the_new_pixels() {
    let path = tmp("flat.psd");
    std::fs::write(&path, flat(RED, false)).unwrap();
    let (mut s, item, cid) = flat_in_comp(&path);
    assert!(near(pixel(&s, cid, 32, 24), RED));
    // Same size, new colour: the frames decoded before must not be shown again.
    save(&path, &flat(BLUE, false));
    let r = s.execute_checked("file.reloadFootage", json!({"items": [item.0]})).unwrap();
    assert_eq!(r["reloaded"], 1);
    assert!(near(pixel(&s, cid, 32, 24), BLUE), "{:?}", pixel(&s, cid, 32, 24));
}

#[test]
fn reload_keeps_a_photoshop_layer_item_on_its_layer() {
    let path = tmp("layers.psd");
    let mut d = WDoc::new(64, 48);
    d.layers = vec![WLayer::solid("Background", Rect::new(0, 0, 64, 48), RED), WLayer::solid("Patch", Rect::new(0, 0, 32, 48), BLUE)];
    std::fs::write(&path, write(&d)).unwrap();
    let mut s = effectcraft_host::session();
    let r = s.execute_checked("file.import", json!({"paths": [path], "importAs": "compositionLayerSizes"})).unwrap();
    let cid = ItemId(r["comps"][0].as_u64().unwrap());
    let items: Vec<ItemId> = r["items"].as_array().unwrap().iter().filter_map(|v| v.as_u64()).map(ItemId).collect();
    let patch = *items.iter().find(|i| s.project.item(**i).is_some_and(|it| it.name.starts_with("Patch/"))).unwrap();
    assert_eq!((footage(&s, patch).width, footage(&s, patch).layer.unwrap().index), (32, 1));
    assert!(near(pixel(&s, cid, 8, 24), BLUE));
    // In the other app: a layer added under the patch, and the patch narrower and green.
    d.layers = vec![
        WLayer::solid("Background", Rect::new(0, 0, 64, 48), RED),
        WLayer::solid("Added", Rect::new(40, 0, 8, 8), RED),
        WLayer::solid("Patch", Rect::new(0, 0, 24, 48), GREEN),
    ];
    save(&path, &write(&d));
    let footage_items: Vec<u64> = items.iter().filter(|i| matches!(s.project.item(**i).map(|x| &x.kind), Some(ItemKind::Footage(_)))).map(|i| i.0).collect();
    s.execute_checked("file.reloadFootage", json!({"items": footage_items})).unwrap();
    let f = footage(&s, patch);
    let l = f.layer.clone().expect("still one layer of the file");
    assert_eq!((l.name.as_str(), l.index, f.width, f.height), ("Patch", 2, 24, 48));
    assert!(!f.missing);
    assert!(near(pixel(&s, cid, 8, 24), GREEN), "{:?}", pixel(&s, cid, 8, 24));
}

#[test]
fn changed_footage_reloads_by_itself_without_an_undo_step() {
    let path = tmp("auto.psd");
    std::fs::write(&path, flat(RED, false)).unwrap();
    let (mut s, _, cid) = flat_in_comp(&path);
    s.mark_saved();
    let undo = s.history.undo.len();
    // First look: remembered, nothing to reload.
    assert_eq!(s.execute_checked("footage.reloadChanged", json!({})).unwrap(), json!({"files": 0, "reloaded": 0}));
    save(&path, &flat(BLUE, true));
    assert_eq!(s.execute_checked("footage.reloadChanged", json!({})).unwrap(), json!({"files": 1, "reloaded": 1}));
    assert!(near(pixel(&s, cid, 32, 24), BLUE));
    assert_eq!(s.history.undo.len(), undo, "the disk changed, not the project");
    assert!(!s.is_dirty());
    assert_eq!(s.execute_checked("footage.reloadChanged", json!({})).unwrap(), json!({"files": 0, "reloaded": 0}));
}

#[test]
fn a_file_still_being_written_is_tried_again() {
    let path = tmp("partial.psd");
    std::fs::write(&path, flat(RED, false)).unwrap();
    let (mut s, item, cid) = flat_in_comp(&path);
    s.execute_checked("footage.reloadChanged", json!({})).unwrap();
    // Another app has only written the start of the file so far.
    let full = flat(BLUE, true);
    save(&path, &full[..12]);
    assert_eq!(s.execute_checked("footage.reloadChanged", json!({})).unwrap(), json!({"files": 0, "reloaded": 0}));
    assert!(!footage(&s, item).missing, "a file being written isn't missing");
    // Meanwhile it renders as whatever part of it can be decoded (here nothing); the next scan,
    // after the other app has finished, reads it whole.
    save(&path, &full);
    assert_eq!(s.execute_checked("footage.reloadChanged", json!({})).unwrap(), json!({"files": 1, "reloaded": 1}));
    assert!(near(pixel(&s, cid, 32, 24), BLUE));
}

/// The desktop app's watcher ([`footage_reload::Watcher`]) notices a footage file another app
/// saved, hands its stamps over once, and reloads it through [`footage_reload::apply_stamps`];
/// while nothing changes it hands over nothing (so the interface is never woken for it).
#[test]
fn the_watcher_notices_a_footage_file_changed_on_disk() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};
    let path = tmp("watched.psd");
    std::fs::write(&path, flat(RED, false)).unwrap();
    let (mut s, _, cid) = flat_in_comp(&path);
    let wakes = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&wakes);
    let fast = Duration::from_millis(20);
    let mut w = footage_reload::Watcher::start_with(
        s.services.clone(),
        move || {
            counter.fetch_add(1, Ordering::SeqCst);
        },
        fast,
        Duration::from_secs(60),
    )
    .unwrap();
    w.set_focused(true);
    w.set_watched(footage_reload::watched(&s));
    // Report `n`: the worker stores the stamps and then wakes the interface, so wait for the
    // wake too, not only for the stamps.
    let next = |w: &footage_reload::Watcher, n: usize| {
        let t0 = Instant::now();
        loop {
            if wakes.load(Ordering::SeqCst) >= n
                && let Some(stamps) = w.take()
            {
                return stamps;
            }
            assert!(t0.elapsed() < Duration::from_secs(10), "the watcher never reported");
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    // First look: remembered, nothing to reload.
    assert_eq!(footage_reload::apply_stamps(&mut s, next(&w, 1)).unwrap(), json!({"files": 0, "reloaded": 0}));
    assert_eq!(wakes.load(Ordering::SeqCst), 1);
    std::thread::sleep(fast * 15);
    assert!(w.take().is_none(), "an unchanged file is never reported");
    assert_eq!(wakes.load(Ordering::SeqCst), 1, "and never wakes the interface");
    // Saved in another app: reported once, reloaded, shown.
    save(&path, &flat(GREEN, true));
    assert_eq!(footage_reload::apply_stamps(&mut s, next(&w, 2)).unwrap(), json!({"files": 1, "reloaded": 1}));
    assert!(near(pixel(&s, cid, 32, 24), GREEN));
    assert_eq!(wakes.load(Ordering::SeqCst), 2);
    std::thread::sleep(fast * 10);
    assert!(w.take().is_none());
}

#[test]
fn image_sequences_reload_by_themselves_only_with_all_footage() {
    let png = |path: &str, v: u8| {
        std::thread::sleep(std::time::Duration::from_millis(30));
        image::RgbaImage::from_pixel(8, 8, image::Rgba([v, 0, 0, 255])).save(path).unwrap();
    };
    let (a, b) = (tmp("seq_0001.png"), tmp("seq_0002.png"));
    png(&a, 255);
    png(&b, 255);
    let mut s = effectcraft_host::session();
    let r = s.execute_checked("file.import", json!({"paths": [&a], "sequence": true})).unwrap();
    let item = ItemId(r["items"][0].as_u64().unwrap());
    assert_eq!(footage(&s, item).sequence.len(), 2, "imported as a sequence");
    // Non-Sequence Footage (the default, as in After Effects): sequences are never stamped.
    assert_eq!(s.prefs.import.auto_reload_footage, "nonSequence");
    assert_eq!(s.execute_checked("footage.reloadChanged", json!({})).unwrap(), json!({"files": 0, "reloaded": 0}));
    png(&a, 0);
    assert_eq!(s.execute_checked("footage.reloadChanged", json!({})).unwrap(), json!({"files": 0, "reloaded": 0}));
    // All Footage: every frame is watched (first seen now, so only remembered).
    s.prefs.import.auto_reload_footage = "all".into();
    assert_eq!(s.execute_checked("footage.reloadChanged", json!({})).unwrap(), json!({"files": 0, "reloaded": 0}));
    std::fs::write(&b, b"not yet a png").unwrap();
    png(&b, 128);
    assert_eq!(s.execute_checked("footage.reloadChanged", json!({})).unwrap()["files"], json!(1));
}

/// #1070: a file decoded in one project and overwritten with one of the same size is read again
/// when a new project imports it (neither File ▸ Reload Footage nor the change watcher knows it).
#[test]
fn a_new_project_importing_an_overwritten_file_shows_the_new_pixels() {
    let path = tmp("overwritten.psd");
    std::fs::write(&path, flat(RED, false)).unwrap();
    let (mut s, _item, cid) = flat_in_comp(&path);
    assert!(near(pixel(&s, cid, 32, 24), RED));
    // Another app overwrites the file with the same number of bytes.
    save(&path, &flat(BLUE, false));
    s.execute_checked("file.newProject", json!({})).unwrap();
    let r = s.execute_checked("file.import", json!({"paths": [path.clone()]})).unwrap();
    let item = r["items"][0].as_u64().unwrap();
    s.execute_checked("comp.new", json!({"name": "C", "width": 64, "height": 48, "frameRate": 30, "duration": 1})).unwrap();
    s.execute_checked("layer.addItem", json!({"item": item})).unwrap();
    let cid = s.active_comp_id().unwrap();
    assert!(near(pixel(&s, cid, 32, 24), BLUE), "stale pixels from the earlier project: {:?}", pixel(&s, cid, 32, 24));
}

fn layered_comp(s: &mut Session, path: &str, import_as: &str) -> ItemId {
    let r = s.execute_checked("file.import", json!({"paths": [path], "importAs": import_as})).unwrap();
    ItemId(r["comps"][0].as_u64().unwrap())
}

/// Composition imports create their sources before probing ordinary footage (#1070).
#[test]
fn fresh_layered_composition_import_refreshes_its_sources() {
    for import_as in ["composition", "compositionLayerSizes"] {
        let path = tmp(&format!("fresh-{import_as}.psd"));
        let (red, blue) = (flat(RED, false), flat(BLUE, false));
        assert_eq!(red.len(), blue.len(), "the overwritten file has the same byte size");
        std::fs::write(&path, red).unwrap();
        let mut s = effectcraft_host::session();
        let old_comp = layered_comp(&mut s, &path, import_as);
        assert!(near(pixel(&s, old_comp, 32, 24), RED));
        s.execute_checked("file.newProject", json!({})).unwrap();
        save(&path, &blue);
        s.drain_events();
        let comp = layered_comp(&mut s, &path, import_as);
        assert_eq!(comp, old_comp, "new projects reuse their item ids");
        assert!(!s.drain_events().contains(&effectcraft_engine::Event::PurgeCaches), "import must retain the RAM preview");
        assert!(near(pixel(&s, comp, 32, 24), BLUE), "layered import showed {:?}", pixel(&s, comp, 32, 24));
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn layered_reimport_refreshes_existing_layers_and_keeps_unrelated_decoded_files() {
    let path = tmp("layered-reimport.psd");
    let other = tmp("layered-unchanged.psd");
    std::fs::write(&path, flat(RED, false)).unwrap();
    std::fs::write(&other, flat(GREEN, false)).unwrap();
    let mut s = effectcraft_host::session();
    let comp = layered_comp(&mut s, &path, "compositionLayerSizes");
    let r = s.execute_checked("file.import", json!({"paths": [&other]})).unwrap();
    let other_item = ItemId(r["items"][0].as_u64().unwrap());
    let other_footage = footage(&s, other_item);
    let cached = s.footage.frame(other_item, &other_footage, Tick::ZERO).unwrap();
    assert!(near(pixel(&s, comp, 32, 24), RED));
    let before = s.project.clone();
    let undo = s.history.undo.len();
    save(&path, &flat(BLUE, false));
    s.drain_events();
    layered_comp(&mut s, &path, "compositionLayerSizes");
    assert!(!s.drain_events().contains(&effectcraft_engine::Event::PurgeCaches), "import must retain the RAM preview");
    assert!(near(pixel(&s, comp, 32, 24), BLUE), "existing layers see the refreshed source");
    assert!(std::sync::Arc::ptr_eq(&cached, &s.footage.frame(other_item, &other_footage, Tick::ZERO).unwrap()));
    assert_eq!(s.history.undo.len(), undo + 1, "composition import stays one undo step");
    s.undo();
    assert!(std::sync::Arc::ptr_eq(&s.project, &before));
    assert!(near(pixel(&s, comp, 32, 24), BLUE), "undo does not undo an external file write");
    std::fs::remove_file(path).unwrap();
    std::fs::remove_file(other).unwrap();
}

#[test]
fn background_import_refreshes_layered_sources_before_the_probe_job_finishes() {
    let path = tmp("layered-background.psd");
    let other = tmp("layered-background.png");
    std::fs::write(&path, flat(RED, false)).unwrap();
    image::RgbaImage::from_pixel(64, 48, image::Rgba([0, 255, 0, 255])).save(&other).unwrap();
    let mut s = effectcraft_host::session();
    let comp = layered_comp(&mut s, &path, "compositionLayerSizes");
    assert!(near(pixel(&s, comp, 32, 24), RED));
    save(&path, &flat(BLUE, false));
    s.drain_events();
    let r = s.execute_checked("file.import", json!({"paths": [&path, &other], "importAs": "compositionLayerSizes", "background": true})).unwrap();
    assert_eq!(r["started"], true, "ordinary footage is probed in a background job");
    assert!(near(pixel(&s, comp, 32, 24), BLUE), "composition sources are refreshed before the job is applied");
    s.wait_jobs();
    assert!(!s.drain_events().contains(&effectcraft_engine::Event::PurgeCaches), "import must retain the RAM preview");
    assert!(near(pixel(&s, s.active_comp_id().unwrap(), 32, 24), BLUE));
    std::fs::remove_file(path).unwrap();
    std::fs::remove_file(other).unwrap();
}
