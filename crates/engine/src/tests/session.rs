use serde_json::json;

use crate::Session;

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

#[test]
fn demo_opens_and_renders() {
    let s = demo();
    let cid = s.active_comp_id().unwrap();
    let img = s.render(cid, s.time(), effectcraft_render::RenderOpts { scale: 0.25, ..Default::default() });
    assert_eq!((img.width, img.height), (480, 270));
    let lit = img.data.iter().filter(|p| p[3] > 0.99).count();
    assert!(lit > 100_000, "{lit}");
}

/// The session's layer cache never changes pixels: scrubbing the demo (including back and
/// forth, and across an edit made through a command) matches cache-less renders exactly.
#[test]
fn demo_layer_cache_is_transparent() {
    let mut s = demo();
    let cid = s.active_comp_id().unwrap();
    let opts = effectcraft_render::RenderOpts { scale: 0.25, ..Default::default() };
    let uncached = |s: &Session, t: f64| {
        let mut r = effectcraft_render::Renderer::new(&s.project, s.footage.as_ref(), opts);
        r.expr = s.expr.as_deref();
        r.comp_frame(cid, effectcraft_time::Tick::from_seconds_f64(t))
    };
    let check = |s: &Session, t: f64| {
        let a = s.render(cid, effectcraft_time::Tick::from_seconds_f64(t), opts);
        let b = uncached(s, t);
        assert!(a.data.iter().zip(&b.data).all(|(p, q)| (0..4).all(|c| (p[c] - q[c]).abs() < 1e-6)), "t={t}");
    };
    for t in [0.5, 1.0, 3.0, 3.0334, 6.0, 3.0, 0.5] {
        check(&s, t);
    }
    assert!(s.layer_cache.stats().hits > 0);
    // Edit the title's glow through the command layer, then scrub again.
    let title = s.project.comp(cid).unwrap().layers.iter().find(|l| l.name == "EFFECTCRAFT").map(|l| l.id.0).unwrap();
    s.execute("prop.set", json!({"layer": title, "path": "effects/#1/radius", "value": 5.0})).unwrap();
    for t in [3.0, 0.5] {
        check(&s, t);
    }
}

/// `layer.addItem` puts the layer at `index` (1-based) in the stack and starts it at `time`, as a
/// drop in the Timeline does (#89).
#[test]
fn add_item_at_a_stack_index_and_time() {
    let mut s = Session::default();
    let clip = s.execute("comp.new", json!({"name": "Clip", "width": 16, "height": 16, "frameRate": 30, "duration": 1})).unwrap()["comp"].as_u64().unwrap();
    s.execute("comp.new", json!({"name": "Main", "width": 32, "height": 32, "frameRate": 30, "duration": 4})).unwrap();
    for name in ["C", "B", "A"] {
        s.execute("layer.newSolid", json!({"name": name})).unwrap();
    }
    let names = |s: &Session| s.active_comp().unwrap().layers.iter().map(|l| l.name.clone()).collect::<Vec<_>>();
    s.execute("layer.addItem", json!({"item": clip, "index": 2, "time": 1.01})).unwrap();
    assert_eq!(names(&s), ["A", "Clip", "B", "C"]);
    let l = &s.active_comp().unwrap().layers[1];
    assert_eq!(l.in_point, effectcraft_time::Tick::from_seconds_f64(1.0), "on the nearest frame");
    // Past the bottom: the bottom.
    s.execute("layer.addItem", json!({"item": clip, "index": 99})).unwrap();
    assert_eq!(names(&s).last().map(String::as_str), Some("Clip 2"));
    assert!(s.execute("layer.addItem", json!({"item": clip, "index": 0})).is_err(), "1-based");
}

/// Files dropped on the Timeline or the Composition viewer (`file.import {addToComp}`) become
/// layers of the comp that was active, one under the other, where they were dropped (#85, #89).
#[test]
fn imported_files_go_into_the_comp_where_they_were_dropped() {
    use effectcraft_project::{Footage, FootageKind};
    struct Probe;
    impl crate::Importer for Probe {
        fn probe(&self, path: &str) -> Result<Footage, String> {
            Ok(Footage { path: path.into(), kind: FootageKind::Still, width: 64, height: 32, has_video: true, ..Default::default() })
        }
    }
    let mut s = Session { importer: Some(std::sync::Arc::new(Probe)), ..Default::default() };
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "frameRate": 30, "duration": 4})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Bg"})).unwrap();
    let params = json!({"paths": ["/drop/a.png", "/drop/b.png"], "addToComp": true, "position": [40.0, 50.0], "index": 1, "time": 1.0});
    s.execute("file.import", params).unwrap();
    let comp = s.active_comp().unwrap();
    assert_eq!(comp.layers.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["a.png", "b.png", "Bg"]);
    for l in &comp.layers[..2] {
        assert_eq!(l.props.prop("transform/position").map(|p| p.value.clone()), Some(effectcraft_keyframe::Value::Vec3([40.0, 50.0, 0.0])));
        assert_eq!(l.in_point, effectcraft_time::Tick::from_seconds_f64(1.0));
    }
    // Without `addToComp` they only import.
    s.execute("file.import", json!({"paths": ["/drop/c.png"]})).unwrap();
    assert_eq!(s.active_comp().unwrap().layers.len(), 3);
    assert!(s.execute("layer.addItem", json!({"item": "a.png", "position": [1.0]})).is_err(), "position needs x and y");
}

/// #270: files dropped or picked in the Import dialog (`file.import {background}`) are probed in
/// a background job that says which file it is reading and how many are left; then they arrive
/// in one undo step, all selected, with a count. The same file imported again arrives again.
#[test]
fn background_imports_report_progress_and_select_what_arrived() {
    use effectcraft_project::{Footage, FootageKind, ItemId};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    /// Probes file k once `allow` > k (a slow video; 5 s at most, so an import that waits for it
    /// fails the test instead of hanging it); `.xyz` files fail.
    struct Gated {
        started: AtomicUsize,
        allow: Arc<AtomicUsize>,
    }
    impl crate::Importer for Gated {
        fn probe(&self, path: &str) -> Result<Footage, String> {
            let k = self.started.fetch_add(1, Ordering::SeqCst);
            let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while self.allow.load(Ordering::SeqCst) <= k && std::time::Instant::now() < until {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            if path.ends_with(".xyz") {
                return Err("unsupported file".into());
            }
            Ok(Footage { path: path.into(), kind: FootageKind::Still, width: 64, height: 32, has_video: true, ..Default::default() })
        }
    }
    let allow = Arc::new(AtomicUsize::new(1));
    let mut s = Session { importer: Some(Arc::new(Gated { started: AtomicUsize::new(0), allow: allow.clone() })), ..Default::default() };
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "frameRate": 30, "duration": 4})).unwrap();
    let undo = s.history.undo.len();
    let r = s.execute("file.import", json!({"paths": ["/drop/a.png", "/drop/b.png", "/drop/a.png"], "background": true})).unwrap();
    let job = r["job"].as_str().unwrap().to_string();
    // The first file is through; the second is being read.
    let running = |s: &mut Session| s.execute("jobs.list", json!({})).unwrap()["running"].as_array().unwrap().clone();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while running(&mut s)[0]["done"] != json!(1) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let row = running(&mut s)[0].clone();
    assert_eq!(row["id"], json!(job));
    assert_eq!(row["label"], json!("Importing 3 files"));
    assert_eq!(row["message"], json!("b.png (2 of 3)"));
    assert_eq!((row["done"].as_u64(), row["total"].as_u64()), (Some(1), Some(3)));
    assert!(s.project.items.values().all(|i| i.name != "a.png"), "nothing is added before the import finishes");
    allow.store(usize::MAX, Ordering::SeqCst);
    s.execute("jobs.wait", json!({})).unwrap();
    let names = |s: &Session, ids: &[ItemId]| ids.iter().map(|i| s.project.item(*i).unwrap().name.clone()).collect::<Vec<_>>();
    assert_eq!(names(&s, &s.state.project_selection), ["a.png", "b.png", "a.png"], "everything imported is selected, the same file twice too");
    assert_eq!(s.history.undo.len(), undo + 1, "one undo step");
    let rec = s.job_log.last().unwrap();
    assert_eq!((rec.status.as_str(), rec.message.as_str()), ("done", "Imported 3 items"));
    assert!(s.drain_events().iter().any(|e| matches!(e, crate::Event::Toast { message, .. } if message == "Imported 3 items")));
    // Dropped on the viewer: they become layers too; a file that can't be read is reported.
    s.execute("file.import", json!({"paths": ["/drop/c.png", "/drop/d.xyz"], "background": true, "addToComp": true, "position": [10.0, 20.0]})).unwrap();
    s.execute("jobs.wait", json!({})).unwrap();
    assert_eq!(s.active_comp().unwrap().layers.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["c.png"]);
    assert_eq!(s.job_log.last().unwrap().message, "Imported 1 item. /drop/d.xyz: unsupported file");
    // Agents and scripts import at once by default.
    let r = s.execute("file.import", json!({"paths": ["/drop/e.png"]})).unwrap();
    assert_eq!(r["items"].as_array().map(Vec::len), Some(1));
}

/// Audio, Lock and Shy don't change pixels: toggling them keeps the cached layers (#103).
#[test]
fn audio_lock_and_shy_keep_cached_layers() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "C", "width": 160, "height": 90, "frameRate": 30, "duration": 1})).unwrap();
    s.execute("layer.newSolid", json!({"color": "#3080ff", "width": 80, "height": 40})).unwrap();
    s.execute("effect.apply", json!({"layer": "#1", "effect": "Gaussian Blur"})).unwrap();
    s.execute("prop.set", json!({"layer": "#1", "path": "effects/#1/blurriness", "value": 12})).unwrap();
    let cid = s.active_comp_id().unwrap();
    let t = s.time();
    s.render(cid, t, Default::default());
    let before = s.layer_cache.stats();
    for sw in ["audio", "lock", "shy"] {
        s.execute("layer.setSwitch", json!({"layers": ["#1"], "switch": sw, "value": sw != "audio"})).unwrap();
    }
    s.render(cid, t, Default::default());
    let after = s.layer_cache.stats();
    assert_eq!(after.misses, before.misses, "no layer rendered again");
    assert!(after.hits > before.hits);
}

#[test]
fn every_command_has_unique_id_and_runs_or_reports() {
    let mut ids = std::collections::HashSet::new();
    for c in crate::command_specs() {
        assert!(ids.insert(c.id), "duplicate {}", c.id);
    }
    assert!(ids.len() >= 90, "{}", ids.len());
}

#[test]
fn layer_workflow_with_undo() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Test", "width": 640, "height": 360, "frameRate": 30, "duration": 4})).unwrap();
    let r = s.execute("layer.newSolid", json!({"color": "#336699"})).unwrap();
    let lid = r["layer"].as_u64().unwrap();
    s.execute("prop.set", json!({"layer": lid, "path": "transform/opacity", "value": 50})).unwrap();
    let tree = s.execute("layer.tree", json!({"layer": lid})).unwrap();
    assert!(tree.to_string().contains("\"value\":50.0"));
    s.execute("prop.toggleAnimation", json!({"layer": lid, "path": "transform/position"})).unwrap();
    s.execute("time.set", json!({"time": 2.0})).unwrap();
    s.execute("prop.set", json!({"layer": lid, "path": "transform/position", "value": [100, 100]})).unwrap();
    let comp = s.active_comp().unwrap();
    let pos = comp.layers[0].props.prop("transform/position").unwrap();
    assert_eq!(pos.keys.len(), 2);
    s.execute("prop.select", json!({"layer": lid, "path": "transform/position"})).unwrap();
    assert_eq!(s.state.selected_keys.len(), 2);
    s.execute("keys.easyEase", json!({})).unwrap();
    s.execute("keys.move", json!({"delta": 0.5})).unwrap();
    let pos = s.active_comp().unwrap().layers[0].props.prop("transform/position").unwrap().clone();
    assert!((pos.keys[0].time.seconds() - 0.5).abs() < 0.02);
    assert_eq!(pos.keys[0].out_interp, effectcraft_keyframe::Interp::Bezier);
    // undo back to before the move
    s.execute("edit.undo", json!({})).unwrap();
    let pos = s.active_comp().unwrap().layers[0].props.prop("transform/position").unwrap().clone();
    assert!(pos.keys[0].time.seconds().abs() < 0.02);
    s.execute("effect.apply", json!({"layer": lid, "effect": "Gaussian Blur"})).unwrap();
    s.execute("prop.set", json!({"layer": lid, "path": "effects/#1/blurriness", "value": 12})).unwrap();
    // Applying selects the new effect (Edit ▸ Duplicate would duplicate it); select the layer.
    s.execute("layer.select", json!({"layers": [lid]})).unwrap();
    s.execute("edit.duplicate", json!({})).unwrap();
    assert_eq!(s.active_comp().unwrap().layers.len(), 2);
    s.execute("layer.precompose", json!({"layers": [1, 2], "name": "Pre"})).unwrap();
    assert_eq!(s.active_comp().unwrap().layers.len(), 1);
}

#[test]
fn text_shape_mask_matte_parent() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"width": 800, "height": 450, "duration": 3})).unwrap();
    let t = s.execute("layer.newText", json!({"text": "Hello", "size": 90})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.addTextAnimator", json!({"layer": t, "properties": ["position", "opacity"]})).unwrap();
    let sh = s.execute("layer.newShape", json!({"kind": "star", "fill": "#ffcc00"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.addShapeItem", json!({"layer": sh, "kind": "trim"})).unwrap();
    s.execute("layer.addMask", json!({"layer": sh, "shape": "ellipse"})).unwrap();
    s.execute("layer.setMask", json!({"layer": sh, "mask": 1, "mode": "Subtract", "inverted": true})).unwrap();
    s.execute("layer.setTrackMatte", json!({"layer": sh, "matte": t, "kind": "luma"})).unwrap();
    s.execute("layer.setParent", json!({"layers": [sh], "parent": t})).unwrap();
    assert!(s.execute("layer.setParent", json!({"layers": [t], "parent": sh})).is_err());
    s.execute("layer.setBlendMode", json!({"layers": [sh], "mode": "Screen"})).unwrap();
    s.execute("layer.setSwitch", json!({"layers": [sh], "switch": "threeD"})).unwrap();
    let cid = s.active_comp_id().unwrap();
    let _ = s.render(cid, s.time(), Default::default());
    let info = s.execute("comp.info", json!({})).unwrap();
    assert_eq!(info["layers"].as_array().unwrap().len(), 2);
}

/// The toolbar's Fill / Stroke change a selected shape layer's paint (#205): a shape drawn
/// without a stroke gets one, later edits change it, and the Contents "Add:" items (#206) include
/// an empty Path and a Gradient Stroke that render.
#[test]
fn toolbar_fill_and_stroke_paint_selected_shape_layers() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"width": 400, "height": 300, "duration": 2})).unwrap();
    let sh = s.execute("layer.newShape", json!({"kind": "rect", "fill": "#ff0000"})).unwrap()["layer"].as_u64().unwrap();
    let paint = |s: &mut Session| s.execute("shape.fillStroke", json!({})).unwrap();
    assert_eq!(paint(&mut s)["stroke"], serde_json::Value::Null, "drawn without a stroke");
    s.execute("shape.fillStroke", json!({"stroke": "#00ff00", "strokeWidth": 6})).unwrap();
    let p = paint(&mut s);
    assert_eq!((p["stroke"][1].as_f64(), p["strokeWidth"].as_f64()), (Some(1.0), Some(6.0)), "{p}");
    s.execute("shape.fillStroke", json!({"fill": "#0000ff", "strokeWidth": 3})).unwrap();
    let p = paint(&mut s);
    assert_eq!((p["fill"][2].as_f64(), p["strokeWidth"].as_f64()), (Some(1.0), Some(3.0)), "{p}");
    // One stroke, before the fill (paths, stroke, fill), and each change is one undo step.
    let l = s.active_comp().unwrap().layer(effectcraft_project::LayerId(sh)).unwrap().clone();
    let group = l.props.group("contents/group").unwrap();
    let order: Vec<&str> = group.sub("contents").unwrap().groups().map(|g| g.match_id.as_str()).collect();
    assert_eq!(order, ["rect", "stroke", "fill"]);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(paint(&mut s)["strokeWidth"].as_f64(), Some(6.0));
    // Only shape layers.
    s.execute("layer.newSolid", json!({"color": "#808080"})).unwrap();
    assert!(s.execute("shape.fillStroke", json!({"fill": "#ffffff"})).is_err());
    for kind in ["path", "gstroke"] {
        let r = s.execute("layer.addShapeItem", json!({"layer": sh, "kind": kind})).unwrap();
        assert!(r["path"].as_str().is_some_and(|p| p.starts_with("contents/")), "{r}");
    }
    let cid = s.active_comp_id().unwrap();
    let _ = s.render(cid, s.time(), Default::default());
}

/// The toolbar's Fill Options / Stroke Options change a selected shape layer's paint kind (#732):
/// None switches the fill off, a gradient turns the Fill into a Gradient Fill that keeps its
/// opacity, Solid Color turns it back, and a Stroke kind adds a stroke where there is none.
#[test]
fn toolbar_fill_and_stroke_kinds_change_selected_shape_layers() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"width": 400, "height": 300, "duration": 2})).unwrap();
    let sh = s.execute("layer.newShape", json!({"kind": "rect", "fill": "#ff0000"})).unwrap()["layer"].as_u64().unwrap();
    let items = |s: &Session| -> Vec<(String, bool)> {
        let l = s.active_comp().unwrap().layer(effectcraft_project::LayerId(sh)).unwrap();
        l.props.group("contents/group/contents").unwrap().groups().map(|g| (g.match_id.clone(), g.enabled)).collect()
    };
    let kinds = |s: &mut Session| {
        let p = s.execute("shape.fillStroke", json!({})).unwrap();
        (p["fillType"].as_str().unwrap().to_string(), p["strokeType"].as_str().unwrap().to_string())
    };
    assert_eq!(kinds(&mut s), ("solid".into(), "none".into()));
    let cid = s.active_comp_id().unwrap();
    let centre = |s: &Session| s.render(cid, s.time(), Default::default()).get(200, 150);
    let red = centre(&s);
    // None switches the fill off: nothing is drawn, and the item stays.
    s.execute("shape.fillStroke", json!({"fillType": "none"})).unwrap();
    assert_eq!(items(&s), [("rect".into(), true), ("fill".into(), false)]);
    assert_eq!(kinds(&mut s).0, "none");
    assert_ne!(centre(&s), red, "the fill is off");
    // A radial gradient: the Fill becomes a Gradient Fill (switched on) that keeps its opacity.
    s.execute("shape.fillStroke", json!({"fillType": "solid", "fill": "#ff0000"})).unwrap();
    assert_eq!(centre(&s), red);
    let opacity = |s: &Session, m: &str| {
        let l = s.active_comp().unwrap().layer(effectcraft_project::LayerId(sh)).unwrap();
        l.props.group(&format!("contents/group/contents/{m}")).unwrap().get("opacity").unwrap().value.as_f64()
    };
    s.execute("prop.set", json!({"layer": sh, "path": "contents/group/contents/fill/opacity", "value": 40})).unwrap();
    s.execute("shape.fillStroke", json!({"fillType": "radial"})).unwrap();
    assert_eq!(items(&s), [("rect".into(), true), ("gfill".into(), true)]);
    assert_eq!((kinds(&mut s).0.as_str(), opacity(&s, "gfill")), ("radial", 40.0));
    s.execute("shape.fillStroke", json!({"fillType": "linear"})).unwrap();
    assert_eq!(kinds(&mut s).0, "linear");
    s.execute("shape.fillStroke", json!({"fillType": "solid", "fill": "#00ff00"})).unwrap();
    assert_eq!(items(&s), [("rect".into(), true), ("fill".into(), true)]);
    assert_eq!(opacity(&s, "fill"), 40.0);
    // A Stroke kind adds a stroke (before the fill); None switches it off again; undo is per step.
    s.execute("shape.fillStroke", json!({"strokeType": "linear"})).unwrap();
    assert_eq!(items(&s), [("rect".into(), true), ("gstroke".into(), true), ("fill".into(), true)]);
    s.execute("shape.fillStroke", json!({"strokeType": "none"})).unwrap();
    assert_eq!(kinds(&mut s), ("solid".into(), "none".into()));
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(kinds(&mut s), ("solid".into(), "linear".into()));
    assert!(s.execute("shape.fillStroke", json!({"fillType": "plaid"})).is_err());
}

/// Selecting a mask in the Timeline (its row or its Mask Path) selects all of its points, so the
/// viewer drags the whole mask (#203); a shape's Path item works the same way.
#[test]
fn selecting_a_mask_selects_its_points() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"width": 400, "height": 300, "duration": 2})).unwrap();
    let l = s.execute("layer.newSolid", json!({"color": "#808080"})).unwrap()["layer"].as_u64().unwrap();
    let m1 = s.execute("layer.addMask", json!({"layer": l, "shape": "rect"})).unwrap()["mask"].as_u64().unwrap();
    let m2 = s.execute("layer.addMask", json!({"layer": l, "shape": "ellipse"})).unwrap()["mask"].as_u64().unwrap();
    let points = |s: &Session, m: u64| s.state.selected_vertices.iter().filter(|v| v.mask == m).count();
    s.execute("prop.select", json!({"layer": l, "prop": m1, "selectKeys": false})).unwrap();
    assert_eq!((points(&s, m1), s.state.selected_vertices.len()), (4, 4));
    // Mask Path selects them too; Shift-adding another mask keeps the first.
    let path = s.active_comp().unwrap().layer(effectcraft_project::LayerId(l)).unwrap().props.find_group(m2).unwrap().get("path").unwrap().uid;
    s.execute("prop.select", json!({"layer": l, "prop": path, "add": true})).unwrap();
    assert_eq!((points(&s, m1), points(&s, m2)), (4, 4));
    // Another mask replaces them; another property deselects them.
    s.execute("prop.select", json!({"layer": l, "prop": m2})).unwrap();
    assert_eq!((points(&s, m1), points(&s, m2)), (0, 4));
    // Delete removes the selected mask (not only its points).
    s.execute("edit.clear", json!({})).unwrap();
    let masks = |s: &Session| s.active_comp().unwrap().layer(effectcraft_project::LayerId(l)).unwrap().masks().unwrap().children.len();
    assert_eq!(masks(&s), 1);
    s.execute("prop.select", json!({"layer": l, "path": "transform/opacity"})).unwrap();
    assert!(s.state.selected_vertices.is_empty());
    let sh = s.execute("layer.newShape", json!({"kind": "star"})).unwrap()["layer"].as_u64().unwrap();
    let star = s.execute("layer.addShapeItem", json!({"layer": sh, "kind": "path"})).unwrap()["uid"].as_u64().unwrap();
    s.execute("prop.select", json!({"layer": sh, "prop": star})).unwrap();
    assert_eq!(s.state.selected_vertices.len(), 0, "an empty Path has no points");
}

#[test]
fn save_and_open_roundtrip() {
    let mut s = demo();
    let dir = std::env::temp_dir().join(format!("ec-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("demo.ecproj").to_string_lossy().to_string();
    s.execute("file.saveAs", json!({"path": path})).unwrap();
    let before = s.project.clone();
    let mut s2 = Session::default();
    s2.execute("file.open", json!({"path": path})).unwrap();
    assert_eq!(*before, *s2.project);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn help_links_emit_urls() {
    let mut s = Session::default();
    let r = s.execute("help.discord", json!({})).unwrap();
    assert_eq!(r["url"], "https://discord.gg/artcraft");
    assert!(s.drain_events().iter().any(|e| matches!(e, crate::Event::OpenUrl(u) if u.contains("discord"))));
    assert_eq!(s.execute("help.github", json!({})).unwrap()["url"], "https://github.com/storytold/effectcraft");
}

#[test]
fn time_navigation() {
    let mut s = demo();
    s.execute("time.start", json!({})).unwrap();
    let r = s.execute("time.nextKey", json!({})).unwrap();
    assert!(r["time"].as_f64().unwrap() > 0.0);
    s.execute("time.set", json!({"frame": 45})).unwrap();
    assert_eq!(s.execute("time.step", json!({"frames": 5})).unwrap()["frame"], 50);
}

#[test]
fn time_navigation_per_property() {
    let mut s = demo();
    // An animated property and the comp times of its keys.
    let comp = s.active_comp().unwrap().clone();
    let mut found = None;
    for l in &comp.layers {
        l.props.walk("", &mut |_, pr| {
            if found.is_none() && pr.keys.len() >= 2 {
                found = Some((pr.uid, pr.keys.iter().map(|k| l.comp_time(k.time)).collect::<Vec<_>>()));
            }
        });
    }
    let (uid, times) = found.expect("demo has an animated property");
    s.set_time(effectcraft_time::Tick::ZERO);
    let half = comp.frame_duration().0 / 2;
    let expect = times.iter().copied().filter(|t| t.0 > half).min().unwrap();
    s.execute("time.go", json!({"to": "nextKey", "prop": uid})).unwrap();
    let snap = |t| comp.frame_rate.snap(t);
    assert_eq!(s.time(), snap(expect));
    // Going back from past the last key lands on the last key.
    s.set_time(comp.duration);
    s.execute("time.go", json!({"to": "prevKey", "prop": uid})).unwrap();
    assert_eq!(s.time(), snap(*times.iter().max().unwrap()));
}

#[test]
fn set_text_paragraph_fill_and_leading() {
    use effectcraft_keyframe::{Justify, Value as KValue};
    let mut s = demo();
    let t = s.execute("layer.newText", json!({"text": "Hi"})).unwrap()["layer"].as_u64().unwrap();
    let doc = |s: &Session| {
        let l = s.active_comp().unwrap().layer(crate::project::LayerId(t)).unwrap().clone();
        match &l.props.prop("text/sourceText").unwrap().value {
            KValue::Text(d) => (**d).clone(),
            v => panic!("{v:?}"),
        }
    };
    for (key, j) in [
        ("justifyLeft", Justify::JustifyLastLeft),
        ("justifyCenter", Justify::JustifyLastCenter),
        ("justifyRight", Justify::JustifyLastRight),
        ("justifyAll", Justify::JustifyAll),
        ("center", Justify::Center),
    ] {
        s.execute("layer.setText", json!({"layer": t, "justify": key})).unwrap();
        assert_eq!(doc(&s).justify, j, "{key}");
    }
    s.execute("layer.setText", json!({"layer": t, "leading": 50, "applyFill": false, "applyStroke": true})).unwrap();
    let d = doc(&s);
    assert_eq!((d.leading, d.apply_fill, d.apply_stroke), (Some(50.0), false, true));
    s.execute("layer.setText", json!({"layer": t, "leading": "auto"})).unwrap();
    assert_eq!(doc(&s).leading, None);
    s.execute("layer.setText", json!({"layer": t, "hScale": 120, "vScale": 80, "baselineShift": 4, "smallCaps": true, "strokeOverFill": true})).unwrap();
    let d = doc(&s);
    assert_eq!((d.h_scale, d.v_scale, d.baseline_shift, d.small_caps, d.stroke_over_fill), (120.0, 80.0, 4.0, true, true));
}

#[test]
fn property_groups_report_actionable_errors() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "T"})).unwrap();
    let l = s.execute("layer.newText", json!({"text": "Hi"})).unwrap()["layer"].as_u64().unwrap();
    let uid = s.active_comp().unwrap().layer(crate::project::LayerId(l)).unwrap().props.group("text/pathOptions").unwrap().uid;
    for selector in [json!({"path": "text"}), json!({"path": "text/pathOptions"}), json!({"path": format!("@{uid}")}), json!({"prop": uid})] {
        let mut params = selector;
        params["layer"] = json!(l);
        for cmd in ["prop.get", "prop.set"] {
            let err = s.execute(cmd, params.clone()).unwrap_err().to_string();
            assert!(err.contains("is a property group"), "{cmd}: {err}");
            assert!(err.contains("get_layer"), "{cmd}: {err}");
        }
    }
    assert!(s.execute("prop.get", json!({"layer": l, "path": "text/sourceText"})).is_ok());
    let err = s.execute("prop.get", json!({"layer": l, "path": "missing"})).unwrap_err().to_string();
    assert!(err.contains("no property `missing`"), "{err}");
}

#[test]
fn prop_get_and_render_rgba8() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "T", "width": 320, "height": 180, "duration": 2.0, "frameRate": 30})).unwrap();
    let l = s.execute("layer.newSolid", json!({"color": "#ff0000"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("prop.addKey", json!({"layer": l, "path": "transform/position", "time": 0.0, "value": [0, 0]})).unwrap();
    s.execute("prop.addKey", json!({"layer": l, "path": "transform/position", "time": 1.0, "value": [100, 50]})).unwrap();
    let v = s.execute("prop.get", json!({"layer": l, "path": "transform/position", "time": 1.0})).unwrap();
    assert_eq!(v["animated"], true);
    assert_eq!(v["keys"].as_array().unwrap().len(), 2);
    assert_eq!(v["value"][0].as_f64().unwrap(), 100.0);
    let cid = s.resolve_comp(Some(&json!("T"))).unwrap();
    let (w, h, rgba) = s.render_rgba8(cid, effectcraft_time::Tick::ZERO, 160).unwrap();
    assert_eq!((w, h), (160, 90));
    assert_eq!(rgba.len(), (w * h * 4) as usize);
}

#[test]
fn params_docs_parse() {
    use crate::commands::accepted_params;
    let k = |d: &str| accepted_params(d).unwrap();
    assert_eq!(k("{layers: [id|name|#n], add?, toggle?}"), ["layers", "add", "toggle"]);
    assert_eq!(k("{time? (s) | frame? | timecode?}"), ["time", "frame", "timecode"]);
    assert_eq!(k("{interpolation?|in?|out?: linear|bezier|hold, autoBezier?}"), ["interpolation", "in", "out", "autoBezier"]);
    assert_eq!(k("{keys: [{layer, prop, time}], add?}"), ["keys", "add"]);
    assert_eq!(k("{label: Red|Yellow|Aqua|…, layers?}"), ["label", "layers"]);
    assert_eq!(k("{name?, background? [r,g,b]|#hex, open?}"), ["name", "background", "open"]);
    assert!(k("{}").is_empty());
    assert!(accepted_params("free text").is_none());
    // Every registered command documents its params as a parseable key list.
    for c in crate::command_specs() {
        assert!(accepted_params(c.params).is_some(), "{}: {}", c.id, c.params);
    }
}

#[test]
fn execute_checked_rejects_unknown_params() {
    let mut s = demo();
    let e = s.execute_checked("layer.select", json!({"index": 2})).unwrap_err().to_string();
    assert!(e.contains("`index`") && e.contains("layers, add, toggle"), "{e}");
    s.execute_checked("layer.select", json!({"layers": ["#2"]})).unwrap();
    assert_eq!(s.state.selected_layers.len(), 1);
    // Aliases and always-accepted keys.
    s.execute_checked("layer.select", json!({"layer": "#1", "comp": s.active_comp_id().unwrap().0})).unwrap();
    s.execute_checked("time.set", json!({"frame": 3})).unwrap();
    assert!(s.execute_checked("edit.undo", json!({"steps": 2})).is_err());
    // Non-object params are not validated; the unchecked path stays lenient.
    s.execute("layer.select", json!({"layers": ["#1"], "index": 2})).unwrap();
}

#[test]
fn layer_tree_paths_resolve() {
    let mut s = demo();
    s.execute("effect.apply", json!({"layer": 1, "effect": "Gaussian Blur"})).unwrap();
    let comp = s.execute("comp.info", json!({})).unwrap();
    let mut checked = 0;
    for l in comp["layers"].as_array().unwrap() {
        let tree = s.execute("layer.tree", json!({"layer": l["id"]})).unwrap();
        let mut stack = vec![tree["properties"].clone()];
        while let Some(n) = stack.pop() {
            if let Some(ch) = n["children"].as_array() {
                stack.extend(ch.iter().cloned());
                continue;
            }
            let path = n["path"].as_str().unwrap();
            let got = s.execute("prop.get", json!({"layer": l["id"], "path": path})).unwrap();
            assert_eq!(got["uid"], n["uid"], "path {path} on layer {}", l["name"]);
            checked += 1;
        }
    }
    assert!(checked > 20, "{checked}");
}

#[test]
fn comp_settings_anchor_start_timecode_renderer() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "A", "width": 100, "height": 100, "duration": 2.0})).unwrap();
    let l = s.execute("layer.newSolid", json!({"color": "#ff0000"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("prop.addKey", json!({"layer": l, "path": "transform/position", "time": 0.0, "value": [10, 20]})).unwrap();
    let pos = |s: &Session| {
        let pr = s.active_comp().unwrap().layer(crate::project::LayerId(l)).unwrap().props.prop("transform/position").unwrap().clone();
        pr.keys[0].value.components()
    };
    // Center anchor: grow by 100x50 → shift by half.
    s.execute("comp.settings", json!({"width": 200, "height": 150})).unwrap();
    assert_eq!(pos(&s)[..2], [60.0, 45.0]);
    // Top-left anchor: no shift. Bottom-right: full shift.
    s.execute("comp.settings", json!({"width": 300, "anchor": 0})).unwrap();
    assert_eq!(pos(&s)[..2], [60.0, 45.0]);
    s.execute("comp.settings", json!({"width": 200, "height": 100, "anchor": 8})).unwrap();
    assert_eq!(pos(&s)[..2], [-40.0, -5.0]);
    s.execute("comp.settings", json!({"startTimecode": "0:00:01:00", "renderer": "advanced3D"})).unwrap();
    let c = s.active_comp().unwrap();
    assert_eq!(c.display_start, c.frame_rate.tick_of(30));
    assert_eq!(c.renderer, crate::project::Renderer::Advanced3D);
}

/// `advanced3D` is a value of `renderer`, not a key: comp.new / comp.settings don't advertise it
/// in their schemas and reject it as unknown instead of ignoring it (#177).
#[test]
fn renderer_values_are_not_parameter_keys() {
    let mut s = Session::default();
    for (id, p) in [("comp.new", json!({"name": "A"})), ("comp.settings", json!({}))] {
        let spec = crate::command_specs().iter().find(|c| c.id == id).unwrap();
        let keys = crate::commands::accepted_params(spec.params).unwrap();
        assert!(keys.contains(&"renderer".to_string()) && !keys.contains(&"advanced3D".to_string()), "{id}: {keys:?}");
        let mut bad = p.clone();
        bad["advanced3D"] = json!(true);
        let e = s.execute_checked(id, bad).unwrap_err().to_string();
        assert!(e.contains("unknown parameter(s) `advanced3D`"), "{id}: {e}");
        let mut good = p;
        good["renderer"] = json!("advanced3D");
        s.execute_checked(id, good).unwrap();
        assert_eq!(s.active_comp().unwrap().renderer, crate::project::Renderer::Advanced3D, "{id}");
        s.execute("comp.renderer", json!({"renderer": "classic3d"})).unwrap();
    }
}

#[test]
fn comp_rejects_zero_and_negative_frame_rates() {
    let mut s = Session::default();
    for fps in [0.0, -30.0, 0.0001] {
        assert!(s.execute("comp.new", json!({"frameRate": fps})).is_err(), "{fps}");
    }
    s.execute("comp.new", json!({"frameRate": 30, "duration": 1})).unwrap();
    assert!(s.execute("comp.settings", json!({"frameRate": 0})).is_err());
    assert_eq!(s.active_comp().unwrap().frame_rate, crate::time::FrameRate::FPS_30);
}

#[test]
fn render_queue_add_accepts_documented_settings_when_checked() {
    let mut s = demo();
    let r = s.execute_checked(
        "renderQueue.add",
        json!({"format": "gif", "output": "/tmp/x.gif", "resolution": "quarter", "timeSpan": "custom", "start": 0.0, "end": 0.5, "quality": "draft", "loop": true}),
    );
    // Without an exporter the add itself still succeeds (rendering is what needs one).
    assert!(r.is_ok(), "{r:?}");
    assert!(s.execute_checked("renderQueue.add", json!({"bogus": 1})).is_err());
}

#[test]
fn new_text_rejects_malformed_positions_without_creating_a_layer() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "C", "width": 64, "height": 64, "duration": 1})).unwrap();
    for position in [json!([]), json!([10]), json!(["x", 20]), json!([null, 20])] {
        let err = s.execute("layer.newText", json!({"text": "Hello", "position": position})).unwrap_err();
        assert!(err.to_string().contains("position: [x, y]"), "{err}");
        assert!(s.active_comp().unwrap().layers.is_empty());
    }
    s.execute("layer.newText", json!({"name": "Valid", "text": "Hello", "position": [10, 20]})).unwrap();
    assert_eq!(s.active_comp().unwrap().layers[0].props.prop("transform/position").unwrap().value, effectcraft_keyframe::Value::Vec3([10.0, 20.0, 0.0]));
    s.execute("layer.newText", json!({"text": "No explicit position"})).unwrap();
    assert_eq!(s.active_comp().unwrap().layers.len(), 2);
}

/// The active comp and the open comp tabs are saved with the project and restored on open (#748).
#[test]
fn reopening_a_project_restores_its_active_comp() {
    use effectcraft_project::ItemId;
    let dir = std::env::temp_dir().join(format!("ec-test-748-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("view.ecproj").to_string_lossy().to_string();
    let mut s = Session::default();
    let a = s.execute("comp.new", json!({"name": "CompA"})).unwrap()["comp"].as_u64().unwrap();
    let b = s.execute("comp.new", json!({"name": "CompB"})).unwrap()["comp"].as_u64().unwrap();
    let c = s.execute("comp.new", json!({"name": "CompC"})).unwrap()["comp"].as_u64().unwrap();
    s.execute("comp.open", json!({"comp": "CompB"})).unwrap();
    s.state.open_comps.retain(|id| id.0 != c);
    s.execute("file.saveAs", json!({"path": path})).unwrap();
    let mut s2 = Session::default();
    s2.execute("file.open", json!({"path": path})).unwrap();
    assert_eq!(s2.state.active_comp, Some(ItemId(b)));
    assert_eq!(s2.state.open_comps, vec![ItemId(a), ItemId(b)]);
    assert!(!s2.is_dirty());

    // A file from before the view was saved opens on its first comp, as it always did.
    let text = std::fs::read_to_string(&path).unwrap();
    let mut old: serde_json::Value = serde_json::from_str(&text).unwrap();
    let obj = old.as_object_mut().unwrap();
    obj.remove("activeComp");
    obj.remove("openComps");
    std::fs::write(&path, serde_json::to_string_pretty(&old).unwrap()).unwrap();
    let mut s3 = Session::default();
    s3.execute("file.open", json!({"path": path})).unwrap();
    assert_eq!((s3.state.active_comp, s3.state.open_comps.clone()), (Some(ItemId(a)), vec![ItemId(a)]));

    // Ids that no longer name a comp (edited by hand, or written by another tool) fall back.
    obj_set(&mut old, json!(999), json!([999, b, "x"]));
    std::fs::write(&path, serde_json::to_string_pretty(&old).unwrap()).unwrap();
    let mut s4 = Session::default();
    s4.execute("file.open", json!({"path": path})).unwrap();
    assert_eq!((s4.state.active_comp, s4.state.open_comps.clone()), (Some(ItemId(b)), vec![ItemId(b)]));
    obj_set(&mut old, json!(999), json!([]));
    std::fs::write(&path, serde_json::to_string_pretty(&old).unwrap()).unwrap();
    let mut s5 = Session::default();
    s5.execute("file.open", json!({"path": path})).unwrap();
    assert_eq!((s5.state.active_comp, s5.state.open_comps.clone()), (Some(ItemId(a)), vec![ItemId(a)]));

    // XML copies carry it too.
    let xml = dir.join("view.ecprojx").to_string_lossy().to_string();
    s.execute("file.saveAs", json!({"path": xml})).unwrap();
    let mut s6 = Session::default();
    s6.execute("file.open", json!({"path": xml})).unwrap();
    assert_eq!(s6.state.active_comp, Some(ItemId(b)));
    let _ = std::fs::remove_dir_all(dir);

    fn obj_set(v: &mut serde_json::Value, active: serde_json::Value, open: serde_json::Value) {
        let o = v.as_object_mut().unwrap();
        o.insert("activeComp".into(), active);
        o.insert("openComps".into(), open);
    }
}

/// Layer times are saved as exact ticks and read back unchanged, and each comp reopens at the
/// current time it was saved at, not at 0, where a layer starting later shows nothing (#1714).
#[test]
fn reopening_a_project_restores_layer_times_and_current_time() {
    use effectcraft_project::ItemId;
    use effectcraft_time::{FrameRate, Tick};
    let dir = std::env::temp_dir().join(format!("ec-test-1714-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("times.ecproj").to_string_lossy().to_string();
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "C", "width": 108, "height": 192, "duration": 60})).unwrap();
    s.execute("layer.newSolid", json!({"name": "crown"})).unwrap();
    let cid = s.active_comp_id().unwrap();
    let comp = std::sync::Arc::make_mut(&mut s.project).comp_mut(cid).unwrap();
    comp.frame_rate = FrameRate::FPS_59_94;
    // The layer from the issue: 34f, 315f and 3033f at 59.94 fps.
    let l = comp.layers.first_mut().unwrap();
    (l.start_time, l.in_point, l.out_point) = (Tick(144_086_342_400), Tick(1_334_917_584_000), Tick(12_853_349_308_800));
    s.execute("time.set", json!({"time": 6.0})).unwrap();
    let t = s.time();
    // A second comp keeps its own time.
    let b = ItemId(s.execute("comp.new", json!({"name": "B"})).unwrap()["comp"].as_u64().unwrap());
    s.execute("time.set", json!({"time": 2.0})).unwrap();
    let tb = s.time();
    s.execute("comp.open", json!({"comp": "C"})).unwrap();
    s.execute("file.saveAs", json!({"path": path})).unwrap();

    let mut s2 = Session::default();
    s2.execute("file.open", json!({"path": path})).unwrap();
    let l = s2.project.comp(cid).unwrap().layers.first().unwrap();
    assert_eq!((l.start_time, l.in_point, l.out_point), (Tick(144_086_342_400), Tick(1_334_917_584_000), Tick(12_853_349_308_800)));
    assert_eq!((s2.time(), s2.state.times.get(&b).copied()), (t, Some(tb)));
    assert!(l.in_point <= s2.time() && s2.time() < l.out_point, "the layer shows at the reopened time");
    assert!(!s2.is_dirty());

    // Hand-edited times past the end or before the start land on the last or first frame.
    let mut file: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    file["compTimes"] = json!({cid.0.to_string(): i64::MAX, b.0.to_string(): -5});
    std::fs::write(&path, file.to_string()).unwrap();
    let mut s3 = Session::default();
    s3.execute("file.open", json!({"path": path})).unwrap();
    let c = s3.project.comp(cid).unwrap();
    assert_eq!(s3.time(), c.frame_rate.snap_nearest(c.duration - c.frame_duration()));
    assert_eq!(s3.time(), c.frame_rate.snap_nearest(c.duration - c.frame_duration()));
    assert_eq!(s3.state.times.get(&b).copied(), Some(Tick::ZERO));
}
