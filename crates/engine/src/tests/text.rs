//! Text animation commands: animators, properties, selectors, per-character 3D, presets (with
//! undo/redo and serde round-trips).

use effectcraft_keyframe::Value as KV;
use effectcraft_project::{LayerId, Node, Project, PropGroup};
use serde_json::json;

use crate::Session;

fn session() -> (Session, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "C", "width": 640, "height": 360, "duration": 4})).unwrap();
    let t = s.execute("layer.newText", json!({"text": "Hello World", "size": 60})).unwrap()["layer"].as_u64().unwrap();
    (s, t)
}

fn text_group(s: &Session) -> PropGroup {
    s.active_comp().unwrap().layers[0].props.sub("text").unwrap().clone()
}

fn matches(g: &PropGroup) -> Vec<String> {
    g.children.iter().map(|c| c.match_id().to_string()).collect()
}

#[test]
fn animate_text_adds_animators_with_companion_properties() {
    let (mut s, t) = session();
    let r = s.execute("layer.addTextAnimator", json!({"layer": t, "properties": ["skew", "tracking", "characterOffset"]})).unwrap();
    assert!(r["animator"].as_u64().is_some());
    let tg = text_group(&s);
    let a = tg.group("animators/#1").unwrap();
    assert_eq!(
        matches(a.sub("properties").unwrap()),
        ["skew", "skewAxis", "trackingType", "tracking", "characterAlignment", "characterRange", "characterOffset"]
    );
    assert_eq!(matches(a.sub("selectors").unwrap()), ["rangeSelector"]);
    // Every Animate entry is accepted.
    for (k, _) in effectcraft_project::build::TEXT_ANIMATOR_KINDS.iter().filter(|(k, _)| *k != "-") {
        s.execute("layer.addTextAnimator", json!({"layer": t, "property": k})).unwrap_or_else(|e| panic!("{k}: {e}"));
    }
    assert!(s.execute("layer.addTextAnimator", json!({"layer": t, "property": "nope"})).is_err());
    // All Transform Properties.
    s.execute("layer.addTextAnimator", json!({"layer": t, "property": "transformAll"})).unwrap();
    let tg = text_group(&s);
    let last = tg.sub("animators").unwrap().groups().last().unwrap().clone();
    assert_eq!(matches(last.sub("properties").unwrap()), ["anchor", "position", "scale", "skew", "skewAxis", "rotation", "opacity"]);
    // Rendering with every property present works.
    let cid = s.active_comp_id().unwrap();
    let _ = s.render(cid, s.time(), Default::default());
}

#[test]
fn add_property_and_selectors_with_undo_redo() {
    let (mut s, t) = session();
    s.execute("layer.addTextAnimator", json!({"layer": t, "property": "position"})).unwrap();
    s.execute("layer.addTextAnimatorProperty", json!({"layer": t, "animator": 1, "property": "fillHue"})).unwrap();
    assert!(s.execute("layer.addTextAnimatorProperty", json!({"layer": t, "animator": 1, "property": "bogus"})).is_err());
    for kind in ["wiggly", "expression", "range"] {
        s.execute("layer.addTextSelector", json!({"layer": t, "kind": kind})).unwrap();
    }
    assert!(s.execute("layer.addTextSelector", json!({"layer": t, "kind": "other"})).is_err());
    let tg = text_group(&s);
    let a = tg.group("animators/#1").unwrap();
    assert_eq!(matches(a.sub("properties").unwrap()), ["position", "fillHue"]);
    let sels = a.sub("selectors").unwrap();
    assert_eq!(matches(sels), ["rangeSelector", "wigglySelector", "expressionSelector", "rangeSelector"]);
    let names: Vec<&str> = sels.children.iter().map(Node::name).collect();
    assert_eq!(names, ["Range Selector 1", "Wiggly Selector 1", "Expression Selector 1", "Range Selector 2"]);
    // Expression selector carries the default amount expression (and no Mode); wiggly defaults to
    // Intersect.
    let ex = sels.group("expressionSelector").unwrap();
    assert!(ex.get("amount").unwrap().has_expression());
    assert!(ex.get("mode").is_none());
    assert_eq!(sels.group("wigglySelector").unwrap().get("mode").unwrap().value, KV::Enum(2));
    let cid = s.active_comp_id().unwrap();
    let a1 = s.render(cid, s.time(), Default::default());
    let a2 = s.render(cid, s.time(), Default::default());
    assert_eq!(a1.data, a2.data, "wiggly text renders deterministically");
    // Undo the three selector additions and the property, redo them.
    for _ in 0..3 {
        s.execute("edit.undo", json!({})).unwrap();
    }
    assert_eq!(matches(text_group(&s).group("animators/#1/selectors").unwrap()), ["rangeSelector"]);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(matches(text_group(&s).group("animators/#1/properties").unwrap()), ["position"]);
    for _ in 0..4 {
        s.execute("edit.redo", json!({})).unwrap();
    }
    assert_eq!(text_group(&s).group("animators/#1/selectors").unwrap().children.len(), 4);
}

#[test]
fn per_character_3d_toggles_layer_and_dimensions() {
    let (mut s, t) = session();
    s.execute("layer.addTextAnimator", json!({"layer": t, "properties": ["position", "rotation"]})).unwrap();
    s.execute("layer.enablePerChar3D", json!({"layer": t})).unwrap();
    let l = s.active_comp().unwrap().layers[0].clone();
    assert!(l.switches.three_d);
    assert_eq!(l.props.prop("text/perChar3d").unwrap().value, KV::Bool(true));
    let props = l.props.group("text/animators/#1/properties").unwrap();
    assert_eq!(props.get("position").unwrap().shown_dims, 3);
    assert_eq!(matches(props), ["position", "rotationX", "rotationY", "rotation"]);
    assert_eq!(props.get("rotation").unwrap().name, "Z Rotation");
    // New animators get 3D properties too.
    s.execute("layer.addTextAnimator", json!({"layer": t, "property": "rotation"})).unwrap();
    assert_eq!(matches(text_group(&s).group("animators/#2/properties").unwrap()), ["rotationX", "rotationY", "rotation"]);
    // Renders through the 3D path.
    s.execute("prop.set", json!({"layer": t, "path": "text/animators/#1/properties/rotationY", "value": 45})).unwrap();
    let cid = s.active_comp_id().unwrap();
    let img = s.render(cid, s.time(), Default::default());
    assert!(img.data.iter().any(|p| p[3] > 0.5));
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    let l = s.active_comp().unwrap().layers[0].clone();
    assert!(!l.switches.three_d);
    assert_eq!(matches(l.props.group("text/animators/#1/properties").unwrap()), ["position", "rotation"]);
    // Disable removes X / Y rotation again.
    s.execute("edit.redo", json!({})).unwrap();
    s.execute("layer.enablePerChar3D", json!({"layer": t, "enabled": false})).unwrap();
    let l = s.active_comp().unwrap().layers[0].clone();
    assert_eq!(matches(l.props.group("text/animators/#1/properties").unwrap()), ["position", "rotation"]);
    assert_eq!(l.props.prop("text/perChar3d").unwrap().value, KV::Bool(false));
}

#[test]
fn presets_apply_render_and_round_trip() {
    let (mut s, t) = session();
    let list = s.execute("text.presets", json!({})).unwrap();
    let ids: Vec<String> = list.as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap().to_string()).collect();
    assert!(ids.len() >= 5 && ids.len() <= 8, "{ids:?}");
    let cid = s.active_comp_id().unwrap();
    for id in &ids {
        let before = s.active_comp().unwrap().layers[0].props.group("text/animators").unwrap().children.len();
        let r = s.execute("layer.applyTextPreset", json!({"layer": t, "preset": id})).unwrap();
        assert!(!r["animators"].as_array().unwrap().is_empty(), "{id}");
        let after = s.active_comp().unwrap().layers[0].props.group("text/animators").unwrap().children.len();
        assert!(after > before, "{id}");
        for secs in [0.0, 0.7, 3.0] {
            let _ = s.render(cid, effectcraft_time::Tick::from_seconds_f64(secs), effectcraft_render::RenderOpts { scale: 0.25, ..Default::default() });
        }
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.active_comp().unwrap().layers[0].props.group("text/animators").unwrap().children.len(), before, "{id}");
    }
    // Typewriter: the start keyframes reveal the text over time.
    s.execute("layer.applyTextPreset", json!({"layer": t, "preset": "Typewriter"})).unwrap();
    let opts = effectcraft_render::RenderOpts { scale: 0.5, ..Default::default() };
    let cov = |img: &effectcraft_render::Image| img.data.iter().filter(|p| p[3] > 0.5).count();
    let a = cov(&s.render(cid, effectcraft_time::Tick::from_seconds_f64(0.0), opts));
    let b = cov(&s.render(cid, effectcraft_time::Tick::from_seconds_f64(1.0), opts));
    let c = cov(&s.render(cid, effectcraft_time::Tick::from_seconds_f64(3.0), opts));
    assert!(a < b && b < c, "{a} {b} {c}");
    // Every selector kind and More / Path Options survive a save/load round trip.
    s.execute("layer.addTextSelector", json!({"layer": t, "kind": "wiggly"})).unwrap();
    s.execute("layer.addTextSelector", json!({"layer": t, "kind": "expression"})).unwrap();
    s.execute("layer.enablePerChar3D", json!({"layer": t})).unwrap();
    let json = s.project.to_json();
    let back = Project::from_json(&json).unwrap();
    assert_eq!(*s.project, back);
    let tg = text_group(&s);
    for m in ["pathOptions", "moreOptions"] {
        assert!(tg.sub(m).is_some(), "{m}");
    }
    assert!(matches(tg.sub("pathOptions").unwrap()).contains(&"forceAlignment".to_string()));
}

#[test]
fn cached_text_presets_keep_the_catalog_and_application_independent() {
    let expected = [
        ("typewriter", "Typewriter"),
        ("fadeUpCharacters", "Fade Up Characters"),
        ("bounceInWords", "Bounce In Words"),
        ("trackingIn", "Tracking In"),
        ("scramble", "Scramble"),
        ("blurIn", "Blur In"),
        ("jitter", "Jitter"),
        ("dropInLines", "Drop In Lines"),
    ];
    let (mut s, layer) = session();
    let catalog = s.execute("text.presets", json!({})).unwrap();
    let mut returned = catalog.clone();
    returned[0]["name"] = json!("Edited copy");
    let project = s.project.clone();
    for _ in 0..2 {
        let names = crate::text_presets();
        assert_eq!(names.iter().map(|(id, name)| (id.as_str(), name.as_str())).collect::<Vec<_>>(), expected);
        for (id, _) in expected {
            let result = s.execute("layer.applyTextPreset", json!({"layer": layer, "preset": id})).unwrap();
            assert!(!result["animators"].as_array().unwrap().is_empty(), "{id}");
            s.execute("edit.undo", json!({})).unwrap();
            assert_eq!(s.project, project, "{id}: undo restores the document");
        }
        assert_eq!(s.execute("text.presets", json!({})).unwrap(), catalog);
    }
}

#[test]
#[ignore = "warm text-preset listing benchmark"]
fn text_preset_listing_benchmark() {
    let old = || {
        let v: serde_json::Value = serde_json::from_str(include_str!("../../presets/text_animators.json")).unwrap();
        let presets = v["presets"].as_array().unwrap().clone();
        presets.iter().map(|p| (p["id"].as_str().unwrap().to_string(), p["name"].as_str().unwrap().to_string())).collect::<Vec<_>>()
    };
    assert_eq!(old(), crate::text_presets());
    let mut before = Vec::new();
    let mut after = Vec::new();
    for _ in 0..5 {
        let start = std::time::Instant::now();
        for _ in 0..1000 {
            std::hint::black_box(old());
        }
        before.push(start.elapsed());
        let start = std::time::Instant::now();
        for _ in 0..1000 {
            std::hint::black_box(crate::text_presets());
        }
        after.push(start.elapsed());
    }
    before.sort();
    after.sort();
    println!("Text preset names, median per warm lookup: {:?} -> {:?}", before[2] / 1000, after[2] / 1000);
}

#[test]
fn menus_have_text_animation_entries() {
    let (s, _) = session();
    let spec = crate::commands::find("layer.enablePerChar3D").unwrap();
    assert!(spec.menu.is_empty(), "placed by the engine menu tree");
    let entries = crate::menus::entries();
    let has = |path: &[&str], cmd: &str| {
        let (label, parent) = path.split_last().unwrap();
        entries.iter().any(|(p, e)| p.iter().map(String::as_str).eq(parent.iter().copied()) && e.label == *label && e.command == cmd)
    };
    assert!(has(&["Animation", "Animate Text", "Enable Per-character 3D"], "layer.enablePerChar3D"));
    assert!(has(&["Animation", "Animate Text", "Fill Color", "Hue"], "layer.addTextAnimator"));
    assert!(has(&["Animation", "Add Text Selector", "Wiggly"], "text.addSelector"));
    assert!(has(&["Animation", "Text Animation Presets", "Typewriter"], "layer.applyTextPreset"));
    // Every preset in the menu exists.
    let ids: Vec<String> = crate::text_presets().into_iter().map(|p| p.0).collect();
    for (_, e) in entries.iter().filter(|(_, e)| e.command == "layer.applyTextPreset") {
        assert!(ids.iter().any(|i| e.params["preset"] == i.as_str()), "{:?}", e.params);
    }
    assert!(s.is_enabled("layer.enablePerChar3D"));
    assert!(!s.is_enabled("layer.addTextSelector"), "needs an animator");
}

/// Duplicate with a Range Selector selected under a text animator copies the selector, not the
/// whole text layer (#1665); a selected animator is copied the same way.
#[test]
fn duplicate_copies_selected_text_selector_not_the_layer() {
    let (mut s, t) = session();
    s.execute("layer.addTextAnimator", json!({"layer": t, "properties": ["tracking"]})).unwrap();
    let anims = |s: &Session| s.active_comp().unwrap().layer(LayerId(t)).unwrap().props.group("text/animators").unwrap().clone();
    let selectors = |s: &Session| -> Vec<u64> { anims(s).groups().next().unwrap().sub("selectors").unwrap().groups().map(|g| g.uid).collect() };
    let sel = selectors(&s)[0];

    s.execute("prop.select", json!({"layer": t, "prop": sel, "selectKeys": false})).unwrap();
    s.execute("edit.duplicate", json!({})).unwrap();
    assert_eq!(s.active_comp().unwrap().layers.len(), 1, "no second text layer");
    assert_eq!(selectors(&s).len(), 2, "the selector is copied");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(selectors(&s), vec![sel]);

    let anim = anims(&s).groups().next().unwrap().uid;
    s.execute("prop.select", json!({"layer": t, "prop": anim, "selectKeys": false})).unwrap();
    s.execute("edit.duplicate", json!({})).unwrap();
    assert_eq!(s.active_comp().unwrap().layers.len(), 1);
    assert_eq!(anims(&s).groups().count(), 2, "the animator is copied");
}
