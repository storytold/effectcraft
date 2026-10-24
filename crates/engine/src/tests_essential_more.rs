//! Essential Graphics Font and uniform Scale controls, Composition ▸ Open in Essential Graphics.

use effectcraft_keyframe::Value as KV;
use effectcraft_project::essential::{self, ControlType};
use serde_json::json;

use crate::{Event, Session};

#[test]
fn font_and_scale_controls_override_per_instance() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Title", "width": 64, "height": 64, "frameRate": 30, "duration": 1})).unwrap();
    let title = s.active_comp_id().unwrap();
    let text = s.execute("layer.newText", json!({"text": "Hello"})).unwrap()["layer"].as_u64().unwrap();
    // Source Text: as text and as font; Scale as one slider.
    let t = s.execute("essential.addProperty", json!({"layer": text, "path": "text/sourceText"})).unwrap()["controls"][0].as_u64().unwrap();
    let f = s.execute("essential.addProperty", json!({"layer": text, "path": "text/sourceText", "as": "font"})).unwrap()["controls"][0].as_u64().unwrap();
    let sc = s.execute("essential.addProperty", json!({"layer": text, "path": "transform/scale", "as": "scale"})).unwrap()["controls"][0].as_u64().unwrap();
    // Adding the same font control again adds nothing with `mirror: false` (else a mirror of
    // it); opacity can't be a font.
    assert!(
        s.execute("essential.addProperty", json!({"layer": text, "path": "text/sourceText", "as": "font", "mirror": false})).unwrap()["controls"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let m = s.execute("essential.addProperty", json!({"layer": text, "path": "text/sourceText", "as": "font"})).unwrap();
    assert_eq!(m["mirrors"].as_array().unwrap().len(), 1);
    let ml = s.execute("essential.list", json!({})).unwrap();
    let mirror = ml["controls"].as_array().unwrap().iter().find(|c| c["kind"] == "mirror").unwrap().clone();
    assert_eq!((mirror["of"].as_u64(), mirror["type"].as_str()), (Some(f), Some("font")));
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.execute("essential.addProperty", json!({"layer": text, "path": "transform/opacity", "as": "font"})).is_err());
    assert_eq!(s.execute("essential.canAdd", json!({"layer": text, "path": "transform/opacity", "as": "font"})).unwrap()["ok"], false);
    assert_eq!(s.execute("essential.canAdd", json!({"layer": text, "path": "transform/scale", "as": "scale"})).unwrap()["type"], "scale");
    let l = s.execute("essential.list", json!({})).unwrap();
    let types: Vec<&str> = l["controls"].as_array().unwrap().iter().map(|c| c["type"].as_str().unwrap()).collect();
    assert_eq!(types, ["text", "font", "scale"]);
    assert!(l["controls"][1]["value"]["size"].as_f64().unwrap() > 0.0);
    let eg = s.project.comp(title).unwrap().essential.clone().unwrap();
    assert_eq!(eg.find(f).unwrap().as_type, Some(ControlType::Font));
    // Survives save / load (serde).
    let j = serde_json::to_value(&eg).unwrap();
    assert_eq!(serde_json::from_value::<essential::EssentialGraphics>(j).unwrap(), eg);

    // An instance: change the text, then the font and the scale.
    s.execute("comp.new", json!({"name": "Main", "width": 64, "height": 64, "frameRate": 30, "duration": 1})).unwrap();
    let inst = s.execute("layer.addItem", json!({"item": title.0})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.select", json!({"layers": [inst]})).unwrap();
    s.execute("essential.set", json!({"layer": inst, "control": t, "value": "Bye"})).unwrap();
    s.execute("essential.set", json!({"layer": inst, "control": f, "value": {"font": "Serif Test", "size": 40}})).unwrap();
    s.execute("essential.set", json!({"layer": inst, "control": sc, "value": 50})).unwrap();
    let i = s.execute("essential.instance", json!({"layer": inst})).unwrap();
    assert_eq!(i["controls"][1]["value"]["font"], "Serif Test");
    assert_eq!(i["controls"][1]["value"]["size"], 40.0);
    assert_eq!(i["controls"][1]["overridden"], true);
    assert_eq!(i["controls"][2]["type"], "scale");
    assert_eq!(i["controls"][2]["value"][0], 50.0);
    assert_eq!(i["controls"][2]["value"][1], 50.0);
    // What the instance renders: the overridden text with the overridden font, scaled.
    let layer = s.active_comp().unwrap().layer(effectcraft_project::LayerId(inst)).unwrap().clone();
    let over: Vec<essential::Override> = {
        let g = essential::group(&layer).unwrap();
        let mut v = vec![];
        g.walk("", &mut |_, p| {
            if let Some(c) = essential::control_of(&p.match_id) {
                v.push(essential::Override { control: c, value: p.value.clone(), keys: vec![] });
            }
        });
        v
    };
    let p = essential::with_overrides(&s.project, title, &over).unwrap();
    let src = p.comp(title).unwrap().layer(effectcraft_project::LayerId(text)).unwrap();
    match src.props.prop("text/sourceText").map(|x| &x.value) {
        Some(KV::Text(d)) => assert_eq!((d.text.as_str(), d.font.as_str(), d.size), ("Bye", "Serif Test", 40.0)),
        o => panic!("{o:?}"),
    }
    // Push to Comp with only the font: the source keeps its own text.
    s.execute("essential.revert", json!({"layer": inst, "control": t})).unwrap();
    s.execute("essential.pushToComp", json!({"layer": inst, "control": f})).unwrap();
    let srcl = s.project.comp(title).unwrap().layer(effectcraft_project::LayerId(text)).unwrap();
    match srcl.props.prop("text/sourceText").map(|x| &x.value) {
        Some(KV::Text(d)) => assert_eq!((d.text.as_str(), d.font.as_str()), ("Hello", "Serif Test")),
        o => panic!("{o:?}"),
    }
}

#[test]
fn open_in_essential_graphics_sets_the_primary_comp() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "A", "width": 32, "height": 32, "duration": 1})).unwrap();
    let a = s.active_comp_id().unwrap();
    s.execute("comp.new", json!({"name": "B", "width": 32, "height": 32, "duration": 1})).unwrap();
    s.drain_events();
    let r = s.execute("comp.openInEssentialGraphics", json!({"comp": a.0})).unwrap();
    assert_eq!(r["name"], "A");
    assert_eq!(s.state.essential_primary, Some(a));
    assert!(
        s.drain_events()
            .iter()
            .any(|e| matches!(e, Event::Frontend { command, params } if command == "window.panel" && params["panel"] == "essentialGraphics"))
    );
    assert_eq!(s.execute("essential.list", json!({})).unwrap()["name"], "A");
}

#[test]
fn instance_keyframes_reach_the_comp_so_expressions_can_look_ahead() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Inner", "width": 32, "height": 32, "frameRate": 30, "duration": 2})).unwrap();
    let inner = s.active_comp_id().unwrap();
    let text = s.execute("layer.newText", json!({"text": "Hi"})).unwrap()["layer"].as_u64().unwrap();
    let ctl = s.execute("essential.addProperty", json!({"layer": text, "path": "transform/opacity"})).unwrap()["controls"][0].as_u64().unwrap();
    let keys = vec![
        effectcraft_keyframe::Keyframe::new(effectcraft_time::Tick::ZERO, KV::Scalar(0.0)),
        effectcraft_keyframe::Keyframe::new(effectcraft_time::Tick::from_seconds_f64(1.0), KV::Scalar(100.0)),
    ];
    let over = [essential::Override { control: ctl, value: KV::Scalar(25.0), keys: keys.clone() }];
    let p = essential::with_overrides(&s.project, inner, &over).unwrap();
    let pr = p.comp(inner).unwrap().layer(effectcraft_project::LayerId(text)).unwrap().props.prop("transform/opacity").unwrap();
    assert_eq!(pr.keys, keys);
    assert_eq!(pr.value, KV::Scalar(25.0));
}

#[test]
fn keys_set_on_an_instance_are_passed_to_the_nested_comp_render() {
    use effectcraft_project::LayerId;
    use effectcraft_time::Tick;
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Inner", "width": 32, "height": 32, "frameRate": 30, "duration": 2})).unwrap();
    let inner = s.active_comp_id().unwrap();
    let solid = s.execute("layer.newSolid", json!({"name": "BG", "color": "#ffffff"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("essential.addProperty", json!({"layer": solid, "path": "transform/opacity"})).unwrap();
    s.execute("comp.new", json!({"name": "Scene", "width": 32, "height": 32, "frameRate": 30, "duration": 2})).unwrap();
    let scene = s.active_comp_id().unwrap();
    let inst = s.execute("layer.addItem", json!({"item": inner.0, "time": 0.0})).unwrap()["layer"].as_u64().unwrap();
    let path = s.execute("essential.instance", json!({"layer": inst})).unwrap()["controls"][0]["path"].as_str().unwrap().to_string();
    s.execute("prop.addKey", json!({"layer": inst, "path": path, "time": 0.0, "value": 0.0})).unwrap();
    s.execute("prop.addKey", json!({"layer": inst, "path": path, "time": 1.0, "value": 100.0})).unwrap();
    // At 0.25 s the nested comp's opacity carries the instance's keys, so it reads the instance's
    // value half a second ahead too (valueAtTime), not only the current one.
    let nested_opacity = |s: &Session| {
        let comp = s.project.comp(scene).unwrap();
        let layer = comp.layer(LayerId(inst)).unwrap();
        let ctx = effectcraft_render::EvalCtx::new(&s.project, scene, comp, Tick::from_seconds_f64(0.25));
        let over = effectcraft_render::essential_overrides(&ctx, layer);
        let p = essential::with_overrides(&s.project, inner, &over).unwrap();
        p.comp(inner).unwrap().layer(LayerId(solid)).unwrap().props.prop("transform/opacity").unwrap().clone()
    };
    let pr = nested_opacity(&s);
    let ahead = s.project.comp(scene).unwrap().layer(LayerId(inst)).unwrap().props.prop(&path).unwrap().value_at(Tick::from_seconds_f64(0.75));
    assert_eq!(pr.keys.len(), 2);
    assert_eq!(pr.value_at(Tick::from_seconds_f64(0.75)), ahead);
    assert_ne!(pr.value_at(Tick::from_seconds_f64(0.75)), pr.value_at(Tick::from_seconds_f64(0.25)));
    // With Time Remap the instance's layer time is not the nested comp's time: only the value.
    s.execute("layer.enableTimeRemap", json!({"layers": [inst]})).unwrap();
    assert!(nested_opacity(&s).keys.is_empty());
}

/// Inner comp with two opacity-exposed solids (the second linked to the first's control), an
/// instance of it in Scene keyed 0 -> 100 over 0..1 s. `timing` sets each solid's start (s) and
/// stretch (%). Returns the pieces the checks below need.
fn keyed_instance(timing: [(f64, f64); 2]) -> (Session, effectcraft_project::ItemId, effectcraft_project::ItemId, [u64; 2], u64) {
    use effectcraft_project::LayerId;
    use effectcraft_time::Tick;
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Inner", "width": 32, "height": 32, "frameRate": 30, "duration": 4})).unwrap();
    let inner = s.active_comp_id().unwrap();
    let a = s.execute("layer.newSolid", json!({"name": "A", "color": "#ffffff"})).unwrap()["layer"].as_u64().unwrap();
    let ctl = s.execute("essential.addProperty", json!({"layer": a, "path": "transform/opacity"})).unwrap()["controls"][0].as_u64().unwrap();
    let b = s.execute("layer.newSolid", json!({"name": "B", "color": "#ffffff"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("essential.linkProperty", json!({"control": ctl, "layer": b, "path": "transform/opacity"})).unwrap();
    for (id, (start, stretch)) in [a, b].into_iter().zip(timing) {
        let l = std::sync::Arc::make_mut(&mut s.project).comp_mut(inner).unwrap().layer_mut(LayerId(id)).unwrap();
        l.start_time = Tick::from_seconds_f64(start);
        l.stretch = stretch;
    }
    s.execute("comp.new", json!({"name": "Scene", "width": 32, "height": 32, "frameRate": 30, "duration": 4})).unwrap();
    let scene = s.active_comp_id().unwrap();
    let inst = s.execute("layer.addItem", json!({"item": inner.0, "time": 0.0})).unwrap()["layer"].as_u64().unwrap();
    let path = s.execute("essential.instance", json!({"layer": inst})).unwrap()["controls"][0]["path"].as_str().unwrap().to_string();
    s.execute("prop.addKey", json!({"layer": inst, "path": path, "time": 0.0, "value": 0.0})).unwrap();
    s.execute("prop.addKey", json!({"layer": inst, "path": path, "time": 1.0, "value": 100.0})).unwrap();
    (s, scene, inner, [a, b], inst)
}

/// Opacity of `solid` inside Inner as the instance renders it at nested time `t`, and the
/// overridden property itself (its keys are what `numKeys` / `valueAtTime` read).
fn rendered(
    s: &Session,
    scene: effectcraft_project::ItemId,
    inner: effectcraft_project::ItemId,
    inst: u64,
    solid: u64,
    t: f64,
) -> (f64, effectcraft_project::Property) {
    use effectcraft_project::LayerId;
    use effectcraft_time::Tick;
    let comp = s.project.comp(scene).unwrap();
    let layer = comp.layer(LayerId(inst)).unwrap();
    let ctx = effectcraft_render::EvalCtx::new(&s.project, scene, comp, Tick::from_seconds_f64(t));
    let over = effectcraft_render::essential_overrides(&ctx, layer);
    let p = essential::with_overrides(&s.project, inner, &over).unwrap();
    let ic = p.comp(inner).unwrap();
    let il = ic.layer(LayerId(solid)).unwrap();
    let pr = il.props.prop("transform/opacity").unwrap().clone();
    let ictx = effectcraft_render::EvalCtx::new(&p, inner, ic, Tick::from_seconds_f64(t));
    (ictx.value(il, &pr).as_f64(), pr)
}

#[test]
fn instance_keys_follow_a_nested_layer_that_starts_late() {
    use effectcraft_time::Tick;
    let (s, scene, inner, [a, _], inst) = keyed_instance([(0.5, 100.0), (0.0, 100.0)]);
    // The instance reads 75 at 0.75 s; the layer starting at 0.5 s must too, not 25.
    let (v, pr) = rendered(&s, scene, inner, inst, a, 0.75);
    assert!((v - 75.0).abs() < 1e-6, "rendered {v}");
    assert_eq!(pr.keys.len(), 2);
    // Layer time: the 0 s and 1 s keys sit at -0.5 s and 0.5 s.
    assert_eq!(pr.keys[0].time, Tick::from_seconds_f64(-0.5));
    assert_eq!(pr.keys[1].time, Tick::from_seconds_f64(0.5));
    // What valueAtTime reads: layer time 0.5 s is the end key, -0.5 s the start key.
    assert_eq!(pr.value_at(Tick::from_seconds_f64(0.5)), KV::Scalar(100.0));
    assert_eq!(pr.value_at(Tick::from_seconds_f64(-0.5)), KV::Scalar(0.0));
}

#[test]
fn instance_keys_follow_a_stretched_nested_layer() {
    use effectcraft_time::Tick;
    let (s, scene, inner, [a, _], inst) = keyed_instance([(0.0, 200.0), (0.0, 100.0)]);
    // Stretched to 200 %: nested time 0.5 s is layer time 0.25 s.
    let (v, pr) = rendered(&s, scene, inner, inst, a, 0.5);
    assert!((v - 50.0).abs() < 1e-6, "rendered {v}");
    assert_eq!(pr.keys.len(), 2);
    assert_eq!(pr.keys[1].time, Tick::from_seconds_f64(0.5));
    assert_eq!(pr.value_at(Tick::from_seconds_f64(0.5)), KV::Scalar(100.0));
    // Offset and stretch together: starts at 0.25 s, 50 %: nested 0.5 s is layer time 0.5 s.
    let (s, scene, inner, [a, _], inst) = keyed_instance([(0.25, 50.0), (0.0, 100.0)]);
    let (v, pr) = rendered(&s, scene, inner, inst, a, 0.5);
    assert!((v - 50.0).abs() < 1e-6, "rendered {v}");
    assert_eq!(pr.keys[0].time, Tick::from_seconds_f64(-0.5));
    assert_eq!(pr.keys[1].time, Tick::from_seconds_f64(1.5));
}

#[test]
fn one_control_with_targets_of_different_timing_stays_in_step() {
    let (s, scene, inner, [a, b], inst) = keyed_instance([(0.5, 100.0), (0.25, 200.0)]);
    for t in [0.2, 0.6, 0.75, 0.9] {
        let (va, pa) = rendered(&s, scene, inner, inst, a, t);
        let (vb, pb) = rendered(&s, scene, inner, inst, b, t);
        let want = t * 100.0;
        assert!((va - want).abs() < 1e-6, "A at {t}: {va}");
        assert!((vb - want).abs() < 1e-6, "B at {t}: {vb}");
        // Each target carries the keys in its own time.
        assert_ne!(pa.keys[0].time, pb.keys[0].time);
        assert_ne!(pa.keys[1].time, pb.keys[1].time);
    }
}
