//! The persistent disk cache (PRV-2) through the session: settings, layer cache persistence
//! across restarts, invalidation on edits, purging.

use serde_json::json;

use crate::Session;
use crate::render::RenderOpts;

fn folder(name: &str) -> String {
    let d = std::env::temp_dir().join(format!("effectcraft-session-disk-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d.to_string_lossy().to_string()
}

fn session(dir: &str) -> Session {
    let mut s = Session::default();
    s.execute_checked("prefs.set", json!({"values": {"disk.diskCacheFolder": dir, "disk.diskCacheEnabled": true}})).unwrap();
    // Write every layer buffer, however quick to render.
    s.disk_cache.as_ref().unwrap().set_min_layer_ms(0);
    s
}

fn project(s: &mut Session) -> effectcraft_project::ItemId {
    s.execute("comp.new", json!({"name": "D", "width": 160, "height": 90, "frameRate": 30, "duration": 1})).unwrap();
    s.execute("layer.newSolid", json!({"color": "#3080ff", "width": 80, "height": 40})).unwrap();
    s.execute("effect.apply", json!({"layer": "#1", "effect": "Gaussian Blur"})).unwrap();
    s.execute("prop.set", json!({"layer": "#1", "path": "effects/#1/blurriness", "value": 12})).unwrap();
    s.active_comp_id().unwrap()
}

#[test]
fn disk_cache_settings_persistence_and_invalidation() {
    crate::tests::roto::hold_roto_cache_test_lock();
    let dir = folder("persist");
    let mut s = session(&dir);
    let st = s.execute_checked("cache.diskStats", json!({})).unwrap();
    assert_eq!(st["enabled"], json!(true));
    assert_eq!(st["maxBytes"], json!(20u64 << 30));
    let cid = project(&mut s);
    let t = s.time();
    let first = s.render(cid, t, RenderOpts::default());
    let dc = s.disk_cache.clone().unwrap();
    dc.flush();
    let written = dc.stats().entries;
    assert!(written > 0, "layer buffers written");
    // "Restart": a new session on the same folder and project renders from disk.
    let saved = s.project.clone();
    drop(s);
    let mut s2 = session(&dir);
    s2.project = saved;
    s2.bump();
    s2.open_comp(cid);
    let dc2 = s2.disk_cache.clone().unwrap();
    assert_eq!(dc2.stats().entries, written);
    let again = s2.render(cid, t, RenderOpts::default());
    assert!(dc2.stats().hits > 0, "{:?}", dc2.stats());
    assert_eq!(again.data, first.data, "disk-cached pixels are exact");
    // An edit changes the keys: a miss, and new entries.
    let hits = dc2.stats().hits;
    s2.execute("prop.set", json!({"layer": "#1", "path": "effects/#1/blurriness", "value": 20})).unwrap();
    let edited = s2.render(cid, t, RenderOpts::default());
    assert_ne!(edited.data, first.data);
    assert_eq!(dc2.stats().hits, hits);
    dc2.flush();
    assert!(dc2.stats().entries > written);
    // Purge ▸ All Disk Cache empties it; memory purge leaves the disk alone.
    let r = s2.execute_checked("edit.purge", json!({"what": "memory"})).unwrap();
    assert_eq!(r["diskEntries"], json!(0));
    assert!(dc2.stats().entries > 0);
    let r = s2.execute_checked("edit.purge", json!({"what": "disk"})).unwrap();
    assert!(r["diskEntries"].as_u64().unwrap() > 0);
    assert_eq!(dc2.stats().entries, 0);
    // Disabling detaches it.
    s2.execute_checked("prefs.set", json!({"key": "disk.diskCacheEnabled", "value": false})).unwrap();
    assert!(s2.disk_cache.is_none());
    assert!(s2.layer_cache.disk().is_none());
}

/// The comp key comes from the project in memory: a footage file that changes on disk doesn't
/// change it (the entries check their own files, #3184); edits that change pixels do.
#[test]
fn comp_content_keys_come_from_memory() {
    let dir = folder("keys");
    let mut s = session(&dir);
    let foot = std::env::temp_dir().join(format!("effectcraft-keys-{}.png", std::process::id()));
    std::fs::write(&foot, b"one").unwrap();
    s.edit("add", None, |p, _| {
        let f = effectcraft_project::Footage {
            path: foot.to_string_lossy().to_string(),
            kind: effectcraft_project::FootageKind::Still,
            width: 1,
            height: 1,
            pixel_aspect: 1.0,
            frame_rate: effectcraft_time::FrameRate::FPS_30,
            native_rate: None,
            duration: effectcraft_time::Tick::ZERO,
            has_video: true,
            has_audio: false,
            alpha: effectcraft_project::AlphaMode::Ignore,
            premul_color: [0.0; 3],
            loop_count: 1,
            codec: "PNG".into(),
            missing: false,
            sequence: vec![],
            color_profile: None,
            layer: None,
            ..Default::default()
        };
        p.add_item("f.png", effectcraft_color::Label::Lavender, None, effectcraft_project::ItemKind::Footage(f));
        Ok(())
    })
    .unwrap();
    let cid = project(&mut s);
    let k1 = effectcraft_render::disk_cache::comp_content_key(&s.project, cid);
    std::fs::write(&foot, b"two, longer").unwrap();
    assert_eq!(effectcraft_render::disk_cache::comp_content_key(&s.project, cid), k1, "no file is looked at");
    s.execute("prop.set", json!({"layer": "#1", "path": "transform/opacity", "value": 50})).unwrap();
    let k2 = effectcraft_render::disk_cache::comp_content_key(&s.project, cid);
    assert_ne!(k1, k2);
    // Switches that don't change pixels (Audio, Lock, Shy, Hide Shy Layers) don't change it (#103).
    for sw in ["audio", "lock", "shy"] {
        s.execute("layer.setSwitch", json!({"layers": ["#1"], "switch": sw, "value": sw != "audio"})).unwrap();
    }
    s.execute("comp.setSwitch", json!({"switch": "hideShy", "value": true})).unwrap();
    assert_eq!(effectcraft_render::disk_cache::comp_content_key(&s.project, cid), k2);
    // Unrelated comps do not change it.
    s.execute("comp.new", json!({"name": "Other", "width": 10, "height": 10, "frameRate": 30, "duration": 1})).unwrap();
    assert_eq!(effectcraft_render::disk_cache::comp_content_key(&s.project, cid), k2);
    // A proxy, and its Use Proxy switch, change the pixels.
    let set_proxy = |s: &mut Session, enabled: bool| {
        let it = std::sync::Arc::make_mut(&mut s.project).items.get_mut(&cid).unwrap();
        it.proxy = Some(Box::new(effectcraft_project::Proxy { footage: effectcraft_project::Footage::default(), enabled }));
        effectcraft_render::disk_cache::comp_content_key(&s.project, cid)
    };
    let k3 = set_proxy(&mut s, true);
    assert_ne!(k3, k2);
    assert_ne!(set_proxy(&mut s, false), k3);
}

/// No salt and no scan (#3184, replacing the background footage salt of #3106): the disk cache
/// is attached to the layer cache as soon as it is switched on, and stays attached through
/// edits that add footage (a large image sequence included), settings changes and opening a
/// project, none of which looks at a single file for it. The first render may use it at once.
#[test]
fn the_disk_cache_is_attached_at_once_and_edits_do_no_file_work() {
    let dir = folder("attach");
    let mut s = session(&dir);
    let dc = s.disk_cache.clone().unwrap();
    let attached = |s: &Session| s.layer_cache.disk().is_some_and(|d| std::sync::Arc::ptr_eq(&d, &dc));
    assert!(attached(&s), "attached at once");
    // A sequence of 5000 frames that don't even exist.
    let seq_dir = std::env::temp_dir().join(format!("effectcraft-no-scan-{}", std::process::id()));
    let f = effectcraft_project::Footage {
        path: seq_dir.join("f_00000.exr").to_string_lossy().to_string(),
        kind: effectcraft_project::FootageKind::Sequence,
        sequence: (0..5000).map(|i| seq_dir.join(format!("f_{i:05}.exr")).to_string_lossy().to_string()).collect(),
        width: 8,
        height: 8,
        has_video: true,
        ..Default::default()
    };
    s.edit("add", None, |p, _| {
        p.add_item("seq", effectcraft_color::Label::None, None, effectcraft_project::ItemKind::Footage(f));
        Ok(())
    })
    .unwrap();
    assert!(attached(&s), "an import keeps it attached");
    let cid = project(&mut s);
    s.execute("prop.set", json!({"layer": "#1", "path": "transform/opacity", "value": 50})).unwrap();
    s.execute_checked("prefs.set", json!({"key": "disk.diskCacheMaxGb", "value": 30})).unwrap();
    assert!(attached(&s), "edits and other settings keep it attached");
    let same = (*s.project).clone();
    s.replace_project(same, None);
    assert!(attached(&s), "opening a project keeps it attached");
    // The first render uses it: a solid reads no file, so nothing is checked either.
    s.render(cid, s.time(), RenderOpts::default());
    dc.flush();
    assert!(dc.stats().entries > 0, "written by the first render");
    assert_eq!(dc.file_checks(), 0, "no file looked at for the disk cache");
    // Switched off and on: attached again at once.
    s.execute_checked("prefs.set", json!({"key": "disk.diskCacheEnabled", "value": false})).unwrap();
    assert!(s.layer_cache.disk().is_none());
    s.execute_checked("prefs.set", json!({"key": "disk.diskCacheEnabled", "value": true})).unwrap();
    assert!(s.layer_cache.disk().is_some());
}

/// Marker edits keep the disk key (the sound of a playing preview must not wait for frames to
/// render again, #1083); an expression that reads markers makes them count.
#[test]
fn markers_keep_the_content_key_unless_an_expression_reads_them() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "M", "width": 8, "height": 8, "frameRate": 30, "duration": 2})).unwrap();
    let cid = s.active_comp_id().unwrap();
    let l = s.execute("layer.newSolid", json!({"color": "#ff0000"})).unwrap()["layer"].as_u64().unwrap();
    let key = |s: &Session| effectcraft_render::disk_cache::comp_content_key(&s.project, cid);
    let k0 = key(&s);
    s.execute("comp.addMarker", json!({"time": 0.5, "comment": "beat"})).unwrap();
    s.execute("markers.set", json!({"layer": l, "new": true, "time": 0.25})).unwrap();
    assert_eq!(key(&s), k0, "markers");
    s.execute("prop.setExpression", json!({"layer": l, "path": "transform/opacity", "expression": "thisComp.marker.numKeys"})).unwrap();
    let k1 = key(&s);
    assert_ne!(k1, k0);
    s.execute("comp.addMarker", json!({"time": 1.0})).unwrap();
    assert_ne!(key(&s), k1, "a marker read by an expression");
}

/// A comp marker alone (no layer marker beside it) keeps the key too.
#[test]
fn a_lone_comp_marker_keeps_the_content_key() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "M", "width": 8, "height": 8, "frameRate": 30, "duration": 2})).unwrap();
    let cid = s.active_comp_id().unwrap();
    s.execute("layer.newSolid", json!({"color": "#ff0000"})).unwrap();
    let key = |s: &Session| effectcraft_render::disk_cache::comp_content_key(&s.project, cid);
    let k0 = key(&s);
    s.execute("comp.addMarker", json!({"time": 0.5, "comment": "beat"})).unwrap();
    assert_eq!(key(&s), k0);
}

/// A protected marker with a duration retimes a stretched precomp layer (Responsive Design), so
/// changing it changes the key of the comp that nests it; a plain marker beside it does not.
#[test]
fn protected_markers_with_a_duration_change_the_content_key() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Inner", "width": 8, "height": 8, "frameRate": 30, "duration": 4})).unwrap();
    let inner = s.active_comp_id().unwrap();
    s.execute("layer.newSolid", json!({"color": "#ff0000"})).unwrap();
    s.execute("comp.new", json!({"name": "Outer", "width": 8, "height": 8, "frameRate": 30, "duration": 10})).unwrap();
    let outer = s.active_comp_id().unwrap();
    let l = s.execute("layer.addItem", json!({"item": inner.0})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.select", json!({"layers": [l]})).unwrap();
    s.execute("layer.timeStretch", json!({"percent": 200})).unwrap();
    let key = |s: &Session| effectcraft_render::disk_cache::comp_content_key(&s.project, outer);
    let k0 = key(&s);
    s.execute("comp.addMarker", json!({"comp": inner.0, "time": 2.0, "comment": "beat"})).unwrap();
    assert_eq!(key(&s), k0, "a plain marker");
    s.execute("comp.addMarker", json!({"comp": inner.0, "time": 1.0})).unwrap();
    let index = s.project.comp(inner).unwrap().markers.iter().position(|m| m.time.seconds() == 1.0).unwrap();
    s.execute("markers.set", json!({"comp": inner.0, "index": index, "duration": 1.0, "protected": true})).unwrap();
    let k1 = key(&s);
    assert_ne!(k1, k0, "a protected marker with a duration");
    s.execute("markers.set", json!({"comp": inner.0, "index": index, "duration": 2.0})).unwrap();
    assert_ne!(key(&s), k1, "its duration");
}

/// With expressions the comp key covers what they can read, not the whole project (#3184,
/// #1492): an expression reading only its own comp keeps the key through edits of another comp
/// and of unused footage; `comp("Other")` makes "Other" count (and which comps have that name),
/// not a third comp; a computed name counts every item.
#[test]
fn expressions_reach_only_what_they_name() {
    let mut s = Session::default();
    let comp = |s: &mut Session, name: &str| {
        let c = s.execute("comp.new", json!({"name": name, "width": 8, "height": 8, "frameRate": 30, "duration": 1})).unwrap()["comp"].as_u64().unwrap();
        let l = s.execute("layer.newSolid", json!({"comp": c, "color": "#ff0000"})).unwrap()["layer"].as_u64().unwrap();
        (effectcraft_project::ItemId(c), (c, l))
    };
    let (main, ml) = comp(&mut s, "Main");
    let (_, ol) = comp(&mut s, "Other");
    let (third, tl) = comp(&mut s, "Third");
    let unused = s.edit("add", None, |p, _| {
        let f = effectcraft_project::Footage { path: "/nowhere/a.png".into(), has_video: true, ..Default::default() };
        Ok(p.add_item("a.png", effectcraft_color::Label::None, None, effectcraft_project::ItemKind::Footage(f)))
    });
    let unused = unused.unwrap();
    let key = |s: &Session| effectcraft_render::disk_cache::comp_content_key(&s.project, main);
    let expr = |s: &mut Session, (comp, layer): (u64, u64), text: &str| {
        s.execute("prop.setExpression", json!({"comp": comp, "layer": layer, "path": "transform/opacity", "expression": text})).unwrap();
    };
    let opacity = |s: &mut Session, (comp, layer): (u64, u64), v: f64| {
        s.execute("prop.set", json!({"comp": comp, "layer": layer, "path": "transform/opacity", "value": v})).unwrap();
    };
    let repath = |s: &mut Session, path: &str| {
        let path = path.to_string();
        s.edit("repath", None, move |p, _| {
            if let Some(effectcraft_project::ItemKind::Footage(f)) = p.item_mut(unused).map(|i| &mut i.kind) {
                f.path = path;
            }
            Ok(())
        })
        .unwrap();
    };
    // Its own comp only.
    expr(&mut s, ml, "thisComp.layer(1).transform.rotation + wiggle(1, 1)[0]");
    expr(&mut s, ol, "wiggle(2, 10)");
    let k0 = key(&s);
    opacity(&mut s, ol, 40.0);
    expr(&mut s, tl, "time * 10");
    repath(&mut s, "/nowhere/b.png");
    s.execute("comp.addMarker", json!({"comp": third.0, "time": 0.5})).unwrap();
    assert_eq!(key(&s), k0, "edits elsewhere keep the key");
    // An edit in the comp itself changes it.
    opacity(&mut s, ml, 30.0);
    assert_ne!(key(&s), k0);
    // `comp("Other")`: Other counts, Third doesn't.
    expr(&mut s, ml, "comp(\"Other\").layer(1).transform.opacity");
    let k1 = key(&s);
    expr(&mut s, ol, "wiggle(3, 10)");
    let k2 = key(&s);
    assert_ne!(k2, k1, "the comp it names");
    opacity(&mut s, tl, 20.0);
    repath(&mut s, "/nowhere/c.png");
    assert_eq!(key(&s), k2, "a comp it doesn't name");
    // Another comp taking the name changes what it may read.
    s.edit("rename", None, |p, _| {
        p.item_mut(third).unwrap().name = "Other".into();
        Ok(())
    })
    .unwrap();
    let k3 = key(&s);
    assert_ne!(k3, k2, "a second comp named Other");
    // A computed name reaches every item.
    expr(&mut s, ml, "comp(\"Oth\" + \"er\").layer(1).transform.opacity");
    let k4 = key(&s);
    repath(&mut s, "/nowhere/d.png");
    assert_ne!(key(&s), k4, "unused footage counts with a computed name");
}

/// Markers count only for the comps whose expressions can read them (#1083): an expression in
/// another comp that reads markers leaves this comp's markers out of its key.
#[test]
fn markers_count_only_where_an_expression_can_read_them() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "M", "width": 8, "height": 8, "frameRate": 30, "duration": 2})).unwrap();
    let cid = s.active_comp_id().unwrap();
    s.execute("layer.newSolid", json!({"color": "#ff0000"})).unwrap();
    let other = s.execute("comp.new", json!({"name": "N", "width": 8, "height": 8, "frameRate": 30, "duration": 2})).unwrap()["comp"].as_u64().unwrap();
    let ol = s.execute("layer.newSolid", json!({"comp": other, "color": "#ff0000"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("prop.setExpression", json!({"comp": other, "layer": ol, "path": "transform/opacity", "expression": "thisComp.marker.numKeys"})).unwrap();
    let key = |s: &Session| effectcraft_render::disk_cache::comp_content_key(&s.project, cid);
    let k0 = key(&s);
    s.execute("comp.addMarker", json!({"comp": cid.0, "time": 0.5, "comment": "beat"})).unwrap();
    assert_eq!(key(&s), k0, "no expression of M reads its markers");
    s.execute("prop.setExpression", json!({"comp": other, "layer": ol, "path": "transform/opacity", "expression": "comp(\"M\").marker.numKeys"})).unwrap();
    let other_key = |s: &Session| effectcraft_render::disk_cache::comp_content_key(&s.project, effectcraft_project::ItemId(other));
    let n0 = other_key(&s);
    s.execute("comp.addMarker", json!({"comp": cid.0, "time": 1.0})).unwrap();
    assert_ne!(other_key(&s), n0, "N reads M's markers");
    assert_eq!(key(&s), k0, "M itself still doesn't");
}

/// File ▸ Reload Footage drops the disk entries made of the item's files (#3184): a file
/// replaced by one with the same size and modification time is read again (the check on hit
/// can't tell), and entries of other files stay.
#[test]
fn reload_footage_drops_the_entries_of_its_files() {
    use crate::render::reads::{FileStamp, ReadFile, ReadFiles};
    struct Probe;
    impl crate::Importer for Probe {
        fn probe(&self, path: &str) -> std::result::Result<effectcraft_project::Footage, String> {
            Ok(effectcraft_project::Footage {
                path: path.into(),
                kind: effectcraft_project::FootageKind::Still,
                width: 1,
                height: 1,
                has_video: true,
                ..Default::default()
            })
        }
    }
    let dir = folder("reload");
    let mut s = Session { importer: Some(std::sync::Arc::new(Probe)), ..Default::default() };
    s.execute_checked("prefs.set", json!({"values": {"disk.diskCacheFolder": &dir, "disk.diskCacheEnabled": true}})).unwrap();
    let src = std::path::Path::new(&dir).join("src");
    std::fs::create_dir_all(&src).unwrap();
    let mut add = |name: &str| {
        let p = src.join(name);
        std::fs::write(&p, b"aa").unwrap();
        let path = p.to_string_lossy().to_string();
        let f = effectcraft_project::Footage {
            path: path.clone(),
            kind: effectcraft_project::FootageKind::Still,
            width: 1,
            height: 1,
            has_video: true,
            ..Default::default()
        };
        let id = s.edit("add", None, |p, _| Ok(p.add_item(name, effectcraft_color::Label::None, None, effectcraft_project::ItemKind::Footage(f)))).unwrap();
        (id, path)
    };
    let (a, a_path) = add("a.png");
    let (_, b_path) = add("b.png");
    let dc = s.disk_cache.clone().unwrap();
    let read = |p: &str| -> ReadFiles { [ReadFile::new(p, FileStamp::native(p).unwrap())].into_iter().collect() };
    dc.put_frame_now(1, 1, 1, &[1; 4], &read(&a_path));
    dc.put_frame_now(2, 1, 1, &[2; 4], &read(&b_path));
    dc.put_layer_now(3, &effectcraft_effects::Buf { img: effectcraft_raster::Image::new(1, 1), offset: [0.0; 2], scale: 1.0 }, &read(&a_path));
    // Other bytes, same size, the old time back.
    let t = std::fs::metadata(&a_path).unwrap().modified().unwrap();
    std::fs::write(&a_path, b"AA").unwrap();
    std::fs::File::options().append(true).open(&a_path).unwrap().set_modified(t).unwrap();
    assert!(dc.contains(crate::render::disk_cache::Kind::Frame, 1));
    s.execute_checked("file.reloadFootage", json!({"item": a.0})).unwrap();
    dc.flush();
    assert!(dc.get_frame(1, None).is_none(), "the reloaded file's frame renders again");
    assert!(dc.get_layer(3, None).is_none(), "and its layer buffer");
    assert!(dc.get_frame(2, None).is_some(), "another file's frame stays");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Reload Footage while the disk cache is off (#3184): when it is switched on later in the
/// session, the entries made of the reloaded files are dropped before anything is read from it
/// (a file replaced by one with the same size and time would pass the check on hit); entries of
/// other files stay.
#[test]
fn reload_footage_while_the_disk_cache_is_off_drops_its_entries_when_it_is_on() {
    use crate::render::disk_cache::{DiskCache, Kind};
    use crate::render::reads::{FileStamp, ReadFile, ReadFiles};
    struct Probe;
    impl crate::Importer for Probe {
        fn probe(&self, path: &str) -> std::result::Result<effectcraft_project::Footage, String> {
            Ok(effectcraft_project::Footage {
                path: path.into(),
                kind: effectcraft_project::FootageKind::Still,
                width: 1,
                height: 1,
                has_video: true,
                ..Default::default()
            })
        }
    }
    let dir = folder("reload-off");
    let src = std::path::Path::new(&dir).join("src");
    std::fs::create_dir_all(&src).unwrap();
    let mut s = Session { importer: Some(std::sync::Arc::new(Probe)), ..Default::default() };
    let mut add = |name: &str| {
        let p = src.join(name);
        std::fs::write(&p, b"aa").unwrap();
        let path = p.to_string_lossy().to_string();
        let f = effectcraft_project::Footage {
            path: path.clone(),
            kind: effectcraft_project::FootageKind::Still,
            width: 1,
            height: 1,
            has_video: true,
            ..Default::default()
        };
        let id = s.edit("add", None, |p, _| Ok(p.add_item(name, effectcraft_color::Label::None, None, effectcraft_project::ItemKind::Footage(f)))).unwrap();
        (id, path)
    };
    let (a, a_path) = add("a.png");
    let (_, b_path) = add("b.png");
    // Entries from an earlier session in the cache folder.
    let read = |p: &str| -> ReadFiles { [ReadFile::new(p, FileStamp::native(p).unwrap())].into_iter().collect() };
    {
        let dc = DiskCache::open(&dir, 1 << 30).unwrap();
        dc.put_frame_now(1, 1, 1, &[1; 4], &read(&a_path));
        dc.put_frame_now(2, 1, 1, &[2; 4], &read(&b_path));
        dc.put_layer_now(3, &effectcraft_effects::Buf { img: effectcraft_raster::Image::new(1, 1), offset: [0.0; 2], scale: 1.0 }, &read(&a_path));
    }
    // Other bytes, same size, the old time back; reloaded while the cache is off.
    let t = std::fs::metadata(&a_path).unwrap().modified().unwrap();
    std::fs::write(&a_path, b"AA").unwrap();
    std::fs::File::options().append(true).open(&a_path).unwrap().set_modified(t).unwrap();
    assert!(s.disk_cache.is_none());
    s.execute_checked("file.reloadFootage", json!({"item": a.0})).unwrap();
    // Switched on: the reloaded file's entries are misses from the start, and then gone.
    s.execute_checked("prefs.set", json!({"values": {"disk.diskCacheFolder": &dir, "disk.diskCacheEnabled": true}})).unwrap();
    let dc = s.disk_cache.clone().unwrap();
    assert!(dc.get_frame(1, None).is_none(), "the reloaded file's frame renders again");
    assert!(dc.get_layer(3, None).is_none(), "and its layer buffer");
    dc.flush();
    assert!(!dc.contains(Kind::Frame, 1) && !dc.contains(Kind::Layer, 3));
    assert!(dc.get_frame(2, None).is_some(), "another file's frame stays");
    assert!(s.pending_disk_drops.is_empty(), "applied once");
    drop(dc);
    drop(s);
    let _ = std::fs::remove_dir_all(&dir);
}
