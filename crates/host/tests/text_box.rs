//! Regression: expanded plugin pixels survive source bounds through the real compositor.
use effectcraft_engine::{Session, render::RenderOpts, time::Tick};
use serde_json::json;

fn prop(s: &mut Session, layer: u64, id: &str, value: serde_json::Value) {
    s.execute("prop.set", json!({"layer":layer,"path":format!("effects/#1/{id}"),"value":value})).unwrap();
}
#[test]
fn textbox_expands_solid_beyond_source_through_compositor() {
    let mut s = effectcraft_host::session();
    s.execute("comp.new", json!({"width":160,"height":100,"duration":2})).unwrap();
    let l = s.execute("layer.newSolid", json!({"width":20,"height":10,"color":[1.0,0.0,0.0]})).unwrap()["layer"].as_u64().unwrap();
    s.execute("effect.apply", json!({"layer":l,"effect":"TextBox"})).unwrap();
    prop(&mut s, l, "fillColor", json!([0.0, 0.0, 1.0, 1.0]));
    let cid = s.active_comp_id().unwrap();
    for scale in [1.0, 0.5] {
        let img = s.render(cid, Tick::ZERO, RenderOpts { scale, ..Default::default() });
        assert_eq!(img.get((80.0 * scale) as i64, (50.0 * scale) as i64), [1.0, 0.0, 0.0, 1.0]);
        // Solid starts at x=70; x=60 is outside the original framebuffer.
        let p = img.get((60.0 * scale) as i64, (50.0 * scale) as i64);
        assert!(p[2] > 0.99 && p[3] > 0.99, "scale={scale}, pixel={p:?}");
    }
    prop(&mut s, l, "offsetX", json!(-35));
    let img = s.render(cid, Tick::ZERO, Default::default());
    assert!(img.get(30, 50)[2] > 0.99);
    assert_eq!(img.get(80, 50), [1.0, 0.0, 0.0, 1.0]);
}
#[test]
fn textbox_follows_real_text_edits_and_empty_alpha() {
    let mut s = effectcraft_host::session();
    s.execute("comp.new", json!({"width":640,"height":360,"duration":2})).unwrap();
    let l = s.execute("layer.newText", json!({"text":"I","size":40})).unwrap()["layer"].as_u64().unwrap();
    s.execute("effect.apply", json!({"layer":l,"effect":"org.effectcraft.text-box"})).unwrap();
    prop(&mut s, l, "fillColor", json!([0.0, 0.0, 1.0, 1.0]));
    let cid = s.active_comp_id().unwrap();
    let before = s.render(cid, Tick::ZERO, Default::default());
    s.execute("layer.setText", json!({"layer":l,"text":"WIDE TEXT\nsecond line","size":60})).unwrap();
    let after = s.render(cid, Tick::ZERO, Default::default());
    let blue = |img: &effectcraft_raster::Image| img.data.iter().filter(|p| p[2] > 0.9 && p[0] < 0.1 && p[3] > 0.9).count();
    assert!(blue(&after) > blue(&before) * 2);
    s.execute("layer.setText", json!({"layer":l,"text":"   \n  "})).unwrap();
    let blank = s.render(cid, Tick::ZERO, Default::default());
    assert!(blank.data.iter().all(|p| p[3] == 0.0));
}

#[test]
fn textbox_parameters_animate_and_survive_project_serialization() {
    let mut s = effectcraft_host::session();
    s.execute("comp.new", json!({"width":160,"height":100,"duration":2})).unwrap();
    let l = s.execute("layer.newSolid", json!({"width":20,"height":10,"color":[1.0,0.0,0.0]})).unwrap()["layer"].as_u64().unwrap();
    s.execute("effect.apply", json!({"layer":l,"effect":"TextBox"})).unwrap();
    prop(&mut s, l, "fillColor", json!([0.0, 0.0, 1.0, 1.0]));
    for (time, value) in [(0.0, 0.0), (1.0, 30.0)] {
        s.execute("prop.addKey", json!({"layer":l,"path":"effects/#1/paddingX","time":time,"value":value})).unwrap();
    }
    let cid = s.active_comp_id().unwrap();
    let a = s.render(cid, Tick::ZERO, Default::default());
    let b = s.render(cid, Tick::from_seconds_f64(1.0), Default::default());
    assert_eq!(a.get(50, 50)[3], 0.0);
    assert!(b.get(50, 50)[2] > 0.99);
    let bytes = serde_json::to_vec(&s.project).unwrap();
    s.project = serde_json::from_slice(&bytes).unwrap();
    let c = s.render(cid, Tick::from_seconds_f64(1.0), Default::default());
    assert_eq!(b.data, c.data);
    let plugins = s.execute("effect.plugins.list", json!({})).unwrap();
    let p = plugins["plugins"].as_array().unwrap().iter().find(|p| p["id"] == "org.effectcraft.text-box").unwrap();
    assert_eq!(p["params"].as_array().unwrap().len(), 14);
}

#[test]
fn textbox_matte_negative_padding_animates_and_roundtrips_with_transparent_fill() {
    let mut s = effectcraft_host::session();
    s.execute("comp.new", json!({"width":160,"height":100,"duration":2})).unwrap();
    let l = s.execute("layer.newSolid", json!({"width":40,"height":20,"color":[1.0,0.0,0.0]})).unwrap()["layer"].as_u64().unwrap();
    s.execute("effect.apply", json!({"layer":l,"effect":"TextBox"})).unwrap();
    prop(&mut s, l, "matte", json!(true));
    prop(&mut s, l, "paddingY", json!(-4));
    // Both the fill opacity and the color alpha may hide the background, never the text.
    prop(&mut s, l, "fillColor", json!([0.0, 0.0, 1.0, 0.0]));
    prop(&mut s, l, "fillOpacity", json!(0));
    for (time, value) in [(0.0, -4.0), (1.0, -12.0)] {
        s.execute("prop.addKey", json!({"layer":l,"path":"effects/#1/paddingX","time":time,"value":value})).unwrap();
    }
    let bytes = serde_json::to_vec(&s.project).unwrap();
    let cid = s.active_comp_id().unwrap();
    for scale in [1.0, 0.5] {
        let opts = RenderOpts { scale, ..Default::default() };
        let sample = |img: &effectcraft_raster::Image, x: f64, y: f64| img.get((x * scale) as i64, (y * scale) as i64);
        let a = s.render(cid, Tick::ZERO, opts);
        assert_eq!(sample(&a, 60.0, 50.0), [0.0; 4]);
        assert_eq!(sample(&a, 68.0, 50.0), [1.0, 0.0, 0.0, 1.0]);
        let b = s.render(cid, Tick::from_seconds_f64(1.0), opts);
        assert_eq!(sample(&b, 68.0, 50.0), [0.0; 4]);
        assert_eq!(sample(&b, 80.0, 50.0), [1.0, 0.0, 0.0, 1.0]);
        s.project = serde_json::from_slice(&bytes).unwrap();
        let restored = s.render(cid, Tick::from_seconds_f64(1.0), opts);
        assert_eq!(restored.data, b.data);
    }
}
