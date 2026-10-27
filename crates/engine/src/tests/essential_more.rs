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
                v.push(essential::Override { control: c, value: p.value.clone() });
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
