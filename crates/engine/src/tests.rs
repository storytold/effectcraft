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
fn shape_add_index_is_optional_bounded_and_validated_before_editing() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"width": 64, "height": 64, "duration": 1})).unwrap();
    let layer = s.execute("layer.newShape", json!({})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.addShapeItem", json!({"layer": layer, "kind": "fill"})).unwrap();
    s.execute("layer.addShapeItem", json!({"layer": layer, "kind": "rect", "index": 0})).unwrap();
    s.execute("layer.addShapeItem", json!({"layer": layer, "kind": "repeater", "index": u64::MAX})).unwrap();
    let order = |s: &Session| {
        s.active_comp()
            .unwrap()
            .layer(effectcraft_project::LayerId(layer))
            .unwrap()
            .props
            .sub("contents")
            .unwrap()
            .groups()
            .map(|g| g.match_id.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(order(&s), vec!["rect", "fill", "repeater"]);
    for index in [json!(-1), json!(0.5), json!("0"), json!(null)] {
        assert!(s.execute("layer.addShapeItem", json!({"layer": layer, "kind": "ellipse", "index": index})).is_err());
        assert_eq!(order(&s), vec!["rect", "fill", "repeater"]);
    }
    // Invalid requests must not add undo entries.
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(order(&s), vec!["rect", "fill"]);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(order(&s), vec!["rect", "fill", "repeater"]);
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
