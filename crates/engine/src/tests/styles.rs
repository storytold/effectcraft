//! Layer Styles commands: add/remove/show, eye switches, Global Light linkage, undo, serde.

use serde_json::json;

use crate::Session;

fn session() -> (Session, u64, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Styles", "width": 200, "height": 100, "frameRate": 30, "duration": 2})).unwrap();
    let a = s.execute("layer.newSolid", json!({"color": "#ffffff", "width": 40, "height": 40})).unwrap()["layer"].as_u64().unwrap();
    let b = s.execute("layer.newSolid", json!({"color": "#808080", "width": 40, "height": 40})).unwrap()["layer"].as_u64().unwrap();
    (s, a, b)
}

fn styles_of(s: &Session, layer: u64) -> Vec<(String, bool)> {
    let v = s.execute_query("layer.style.list", json!({"layer": layer}));
    v["styles"].as_array().unwrap().iter().map(|x| (x["style"].as_str().unwrap().to_string(), x["enabled"].as_bool().unwrap())).collect()
}

impl Session {
    fn execute_query(&self, id: &str, p: serde_json::Value) -> serde_json::Value {
        let mut c = Session { project: self.project.clone(), state: self.state.clone(), ..Default::default() };
        c.execute(id, p).unwrap()
    }
}

#[test]
fn add_remove_show_all_with_undo() {
    let (mut s, a, _) = session();
    s.execute("layer.select", json!({"layers": [a]})).unwrap();
    s.execute("layer.style.stroke", json!({})).unwrap();
    s.execute("layer.style.dropShadow", json!({})).unwrap();
    s.execute("layer.style.add", json!({"style": "Bevel and Emboss"})).unwrap();
    assert_eq!(styles_of(&s, a), vec![("dropShadow".into(), true), ("bevelEmboss".into(), true), ("stroke".into(), true)]);
    // Values are ordinary properties.
    s.execute("prop.set", json!({"layer": a, "path": "layerStyles/dropShadow/distance", "value": 12})).unwrap();
    let v = s.execute("prop.get", json!({"layer": a, "path": "layerStyles/dropShadow/distance"})).unwrap();
    assert!(v.to_string().contains("12"));
    // Eye switch.
    s.execute("layer.style.toggle", json!({"layer": a, "style": "stroke"})).unwrap();
    assert!(styles_of(&s, a).contains(&("stroke".into(), false)));
    s.execute("edit.undo", json!({})).unwrap();
    assert!(styles_of(&s, a).contains(&("stroke".into(), true)));
    // Remove one, then everything.
    s.execute("layer.style.remove", json!({"layer": a, "style": "bevelEmboss"})).unwrap();
    assert_eq!(styles_of(&s, a).len(), 2);
    s.execute("layer.style.removeAll", json!({})).unwrap();
    assert!(styles_of(&s, a).is_empty());
    let layer = s.active_comp().unwrap().layer(effectcraft_project::LayerId(a)).unwrap().clone();
    assert!(layer.layer_styles().is_none());
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(styles_of(&s, a).len(), 2);
    s.execute("edit.redo", json!({})).unwrap();
    assert!(styles_of(&s, a).is_empty());
    // Show All adds every style switched off; choosing one switches it on.
    s.execute("layer.style.showAll", json!({})).unwrap();
    let all = styles_of(&s, a);
    assert_eq!(all.len(), 9);
    assert!(all.iter().all(|(_, on)| !on));
    s.execute("layer.style.satin", json!({})).unwrap();
    assert!(styles_of(&s, a).contains(&("satin".into(), true)));
    // Removing the last style removes the group.
    s.execute("layer.style.removeAll", json!({})).unwrap();
    s.execute("layer.style.colorOverlay", json!({})).unwrap();
    s.execute("layer.style.remove", json!({"layer": a, "style": "Color Overlay"})).unwrap();
    assert!(s.active_comp().unwrap().layer(effectcraft_project::LayerId(a)).unwrap().layer_styles().is_none());
    // Convert to Editable Styles is unavailable without PSD styles.
    assert!(!s.is_enabled("layer.style.convertToEditable"));
}

#[test]
fn global_light_is_shared_by_the_comp() {
    let (mut s, a, b) = session();
    s.execute("layer.style.dropShadow", json!({"layers": [a]})).unwrap();
    s.execute("layer.style.bevelEmboss", json!({"layers": [b]})).unwrap();
    // Editing one layer's Global Light Angle updates the other layer and the comp.
    s.execute("prop.set", json!({"layer": a, "path": "layerStyles/blendingOptions/globalLightAngle", "value": 45})).unwrap();
    let bv = s.execute("prop.get", json!({"layer": b, "path": "layerStyles/blendingOptions/globalLightAngle"})).unwrap();
    assert!(bv.to_string().contains("45"), "{bv}");
    let gl = s.execute("layer.style.globalLight", json!({})).unwrap();
    assert_eq!(gl["angle"].as_f64(), Some(45.0));
    // The command sets both and new style groups pick it up.
    s.execute("layer.style.globalLight", json!({"angle": 10, "altitude": 60})).unwrap();
    let c = s.execute("layer.newSolid", json!({"color": "#ff0000", "width": 10, "height": 10})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.style.innerShadow", json!({"layers": [c]})).unwrap();
    for l in [a, b, c] {
        let v = s.execute("prop.get", json!({"layer": l, "path": "layerStyles/blendingOptions/globalLightAltitude"})).unwrap();
        assert!(v.to_string().contains("60"), "{l}: {v}");
    }
    // Undo restores the previous shared value on every layer.
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    let v = s.execute("prop.get", json!({"layer": b, "path": "layerStyles/blendingOptions/globalLightAngle"})).unwrap();
    assert!(v.to_string().contains("45"), "{v}");
}

#[test]
fn styles_render_and_round_trip() {
    let (mut s, a, _) = session();
    s.execute("layer.style.stroke", json!({"layers": [a]})).unwrap();
    s.execute("prop.set", json!({"layer": a, "path": "layerStyles/stroke/size", "value": 6})).unwrap();
    let cid = s.active_comp_id().unwrap();
    // Cached session render == uncached render, and the stroke shows outside the 40×40 square.
    let img = s.render(cid, s.time(), Default::default());
    let r = effectcraft_render::render_frame(&s.project, cid, s.time(), 1.0);
    assert_eq!(img.data, r.data);
    assert!(img.get(123, 50)[0] > 0.98 && img.get(123, 50)[1] < 0.02, "{:?}", img.get(123, 50));
    // Change the colour through a command: the cache must not serve stale pixels.
    s.execute("prop.set", json!({"layer": a, "path": "layerStyles/stroke/color", "value": [0, 0, 1, 1]})).unwrap();
    let img2 = s.render(cid, s.time(), Default::default());
    assert!(img2.get(123, 50)[2] > 0.98 && img2.get(123, 50)[0] < 0.02);
    // Serde round trip keeps styles and the comp's Global Light.
    s.execute("layer.style.globalLight", json!({"angle": 33})).unwrap();
    let json = serde_json::to_string(&*s.project).unwrap();
    let back: effectcraft_project::Project = serde_json::from_str(&json).unwrap();
    assert_eq!(back, *s.project);
    assert_eq!(back.comp(cid).unwrap().global_light.angle.value.as_f64(), 33.0);
}
