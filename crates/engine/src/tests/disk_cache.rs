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

#[test]
fn footage_changes_change_the_salt() {
    let dir = folder("salt");
    let mut s = session(&dir);
    let foot = std::env::temp_dir().join(format!("effectcraft-salt-{}.png", std::process::id()));
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
    let a = effectcraft_render::disk_cache::footage_salt(&s.project);
    std::fs::write(&foot, b"two, longer").unwrap();
    let b = effectcraft_render::disk_cache::footage_salt(&s.project);
    assert_ne!(a, b);
    let cid = project(&mut s);
    let k1 = effectcraft_render::disk_cache::comp_content_key(&s.project, cid);
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

/// The desktop app stamps footage files for the disk cache's salt on a worker thread: importing an
/// After Effects project with thousands of sequence frames on a network share stamped them all
/// inside the import's edit, freezing the UI for half a minute. Until the salt lands the disk
/// cache sits out (keys under the old salt may name other files); a newer footage change
/// supersedes a salt still being taken; a settings change keeps the salt.
#[test]
fn footage_salt_is_taken_off_the_ui_thread_and_the_disk_cache_waits_for_it() {
    let dir = folder("bg-salt");
    let mut s = session(&dir);
    s.disk_salt_background = true;
    let at = |n: &str| std::env::temp_dir().join(format!("effectcraft-bg-salt-{}-{n}.png", std::process::id()));
    let add = |s: &mut Session, path: &std::path::Path| {
        let f = effectcraft_project::Footage {
            path: path.to_string_lossy().to_string(),
            kind: effectcraft_project::FootageKind::Still,
            width: 1,
            height: 1,
            has_video: true,
            ..Default::default()
        };
        s.edit("add", None, |p, _| {
            p.add_item("f.png", effectcraft_color::Label::None, None, effectcraft_project::ItemKind::Footage(f));
            Ok(())
        })
        .unwrap();
    };
    let wait = |s: &mut Session| {
        let t0 = std::time::Instant::now();
        while s.poll_disk_salt() {
            assert!(t0.elapsed().as_secs() < 20, "the salt never landed");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };
    assert!(s.layer_cache.disk().is_some(), "attached with its salt");
    std::fs::write(at("a"), b"one").unwrap();
    add(&mut s, &at("a"));
    // The edit returned before the files were stamped, and the disk cache waits for the salt.
    assert!(s.disk_salt.is_none() && s.layer_cache.disk().is_none());
    // Edits that leave the footage alone don't start over (and may pick up the salt meanwhile).
    s.execute("comp.new", json!({"name": "C", "width": 8, "height": 8, "frameRate": 30, "duration": 1})).unwrap();
    let r = s.refresh_disk_salt(false);
    assert!(r.is_none() || r == Some(effectcraft_render::disk_cache::footage_salt(&s.project)), "{r:?}");
    // New footage while it is taken: the salt for the newest footage lands.
    std::fs::write(at("b"), b"two").unwrap();
    add(&mut s, &at("b"));
    wait(&mut s);
    let salt = effectcraft_render::disk_cache::footage_salt(&s.project);
    assert_eq!(s.disk_salt.map(|(_, salt)| salt), Some(salt));
    assert_eq!(s.refresh_disk_salt(false), Some(salt));
    assert!(s.layer_cache.disk().is_some(), "attached again");
    // Another setting keeps the salt and the disk cache attached.
    s.execute_checked("prefs.set", json!({"key": "disk.diskCacheMaxGb", "value": 30})).unwrap();
    assert!(s.layer_cache.disk().is_some() && s.disk_salt.map(|(_, salt)| salt) == Some(salt));
    // Turned off and on again: stamped anew, in the background.
    s.execute_checked("prefs.set", json!({"key": "disk.diskCacheEnabled", "value": false})).unwrap();
    s.execute_checked("prefs.set", json!({"key": "disk.diskCacheEnabled", "value": true})).unwrap();
    wait(&mut s);
    assert_eq!(s.refresh_disk_salt(false), Some(salt));
    let _ = std::fs::remove_file(at("a"));
    let _ = std::fs::remove_file(at("b"));
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

/// A project opened while a footage salt is still being taken must not keep that salt when it
/// names the same footage: its files may have changed since the worker read them.
#[test]
fn opening_a_project_drops_the_footage_salt_being_taken() {
    let dir = folder("bg-salt-open");
    let mut s = session(&dir);
    s.disk_salt_background = true;
    let path = std::env::temp_dir().join(format!("effectcraft-bg-salt-open-{}.png", std::process::id()));
    std::fs::write(&path, b"one").unwrap();
    let f = effectcraft_project::Footage {
        path: path.to_string_lossy().to_string(),
        kind: effectcraft_project::FootageKind::Still,
        width: 1,
        height: 1,
        has_video: true,
        ..Default::default()
    };
    s.edit("add", None, |p, _| {
        p.add_item("f.png", effectcraft_color::Label::None, None, effectcraft_project::ItemKind::Footage(f));
        Ok(())
    })
    .unwrap();
    // The file changes after the edit started the salt, before the same footage is opened again.
    std::fs::write(&path, b"a longer content").unwrap();
    let same = (*s.project).clone();
    s.replace_project(same, None);
    let t0 = std::time::Instant::now();
    while s.poll_disk_salt() {
        assert!(t0.elapsed().as_secs() < 20, "the salt never landed");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let now = effectcraft_render::disk_cache::footage_salt(&s.project);
    assert_eq!(s.disk_salt.map(|(_, salt)| salt), Some(now), "the salt reflects the file as it is now");
    let _ = std::fs::remove_file(&path);
}
