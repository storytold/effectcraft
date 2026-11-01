//! M13.5 long-tail parity: stroke taper/wave/dashes, and the other small gaps (commands, undo,
//! serde).

use serde_json::{Value, json};

use crate::Session;
use effectcraft_project::{LayerId, Project};

pub(crate) fn comp(w: u32, h: u32) -> Session {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Tail", "width": w, "height": h, "frameRate": 30, "duration": 2})).unwrap();
    s
}

pub(crate) fn roundtrip(s: &Session) -> Project {
    Project::from_json(&s.project.to_json()).unwrap()
}

/// A horizontal open path 160 px long across a 200×100 comp, 10 px red stroke and no fill.
fn line_layer(s: &mut Session) -> (u64, u64) {
    let r = s.execute("shape.newPath", json!({"vertices": [[20, 50], [180, 50]], "fill": false, "stroke": [1, 0, 0], "strokeWidth": 10})).unwrap();
    let lid = r["layer"].as_u64().unwrap();
    let l = s.active_comp().unwrap().layer(LayerId(lid)).unwrap();
    let stroke = l.props.find_group(r["group"].as_u64().unwrap()).unwrap().sub("contents").unwrap().sub("stroke").unwrap().uid;
    (lid, stroke)
}

fn column(s: &Session, x: u32) -> f32 {
    let cid = s.active_comp_id().unwrap();
    let img = s.render(cid, effectcraft_time::Tick::ZERO, Default::default());
    (0..img.height).map(|y| img.data[(y * img.width + x) as usize][3]).sum()
}

#[test]
fn stroke_taper_wave_commands_render_undo_and_serde() {
    let mut s = comp(200, 100);
    let (lid, stroke) = line_layer(&mut s);
    assert!((column(&s, 30) - 10.0).abs() < 0.6);
    let r = s.execute("shape.stroke.taper", json!({"layer": lid, "startLength": 40, "startWidth": 0})).unwrap();
    assert_eq!(r["stroke"], json!(stroke));
    assert!((column(&s, 30) - 2.5).abs() < 0.8, "tapered start: {}", column(&s, 30));
    // Percent units.
    s.execute("shape.stroke.taper", json!({"layer": lid, "units": "percent", "startLength": 25})).unwrap();
    assert!((column(&s, 30) - 2.5).abs() < 0.8, "25 % of 160 px = 40 px: {}", column(&s, 30));
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert!((column(&s, 30) - 10.0).abs() < 0.6, "undo restores the plain stroke");
    s.execute("edit.redo", json!({})).unwrap();
    assert!((column(&s, 30) - 2.5).abs() < 0.8);
    // Wave in cycles.
    s.execute("shape.stroke.wave", json!({"layer": lid, "amount": 100, "units": "cycles", "cycles": 4})).unwrap();
    assert!(column(&s, 120) < 0.6, "trough at 100 px (2.5 cycles): {}", column(&s, 120));
    // Values are ordinary animatable properties that survive save/load.
    let back = roundtrip(&s);
    let l = back.comp(s.active_comp_id().unwrap()).unwrap().layer(LayerId(lid)).unwrap();
    let g = l.props.find_group(stroke).unwrap();
    assert_eq!(g.prop("wave/cycles").unwrap().value.as_f64(), 4.0);
    assert_eq!(g.prop("taper/startLength").unwrap().value.as_f64(), 40.0);
    assert!(s.execute("shape.stroke.wave", json!({"layer": lid, "units": "furlongs"})).is_err());
}

#[test]
fn dashes_plus_minus_buttons() {
    let mut s = comp(200, 100);
    let (lid, stroke) = line_layer(&mut s);
    let add = |s: &mut Session| s.execute("shape.dashes.add", json!({"layer": lid, "prop": stroke})).unwrap()["added"].as_str().unwrap().to_string();
    assert_eq!(add(&mut s), "dash");
    s.execute("prop.set", json!({"layer": lid, "prop": dash_uid(&s, lid, stroke, "dash"), "value": 20})).unwrap();
    assert_eq!(add(&mut s), "dash2");
    assert_eq!(add(&mut s), "dash3");
    assert!(s.execute("shape.dashes.add", json!({"layer": lid})).is_err(), "three pairs at most");
    let names = |s: &Session| {
        let l = s.active_comp().unwrap().layer(LayerId(lid)).unwrap();
        l.props.find_group(stroke).unwrap().sub("dashes").unwrap().children.iter().map(|n| n.match_id().to_string()).collect::<Vec<_>>()
    };
    assert_eq!(names(&s), ["dash", "gap", "dash2", "gap2", "dash3", "gap3", "offset"]);
    // Dash 20, Gap 20 (= dash), Dash 2 20, Gap 2 20…: set Dash 2 to 5.
    s.execute("prop.set", json!({"layer": lid, "prop": dash_uid(&s, lid, stroke, "gap"), "value": 10})).unwrap();
    s.execute("prop.set", json!({"layer": lid, "prop": dash_uid(&s, lid, stroke, "dash2"), "value": 5})).unwrap();
    s.execute("prop.set", json!({"layer": lid, "prop": dash_uid(&s, lid, stroke, "gap2"), "value": 15})).unwrap();
    s.execute("shape.dashes.remove", json!({"layer": lid})).unwrap();
    assert_eq!(names(&s), ["dash", "gap", "dash2", "gap2", "offset"]);
    // Pattern 20 on, 10 off, 5 on, 15 off from x = 20 (butt caps).
    assert!(column(&s, 30) > 9.0 && column(&s, 45) < 0.1 && column(&s, 52) > 9.0 && column(&s, 60) < 0.1 && column(&s, 80) > 9.0);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(names(&s).len(), 7);
    let back = roundtrip(&s);
    let l = back.comp(s.active_comp_id().unwrap()).unwrap().layer(LayerId(lid)).unwrap();
    assert_eq!(l.props.find_group(stroke).unwrap().prop("dashes/dash2").unwrap().value.as_f64(), 5.0);
}

fn dash_uid(s: &Session, lid: u64, stroke: u64, m: &str) -> Value {
    let l = s.active_comp().unwrap().layer(LayerId(lid)).unwrap();
    json!(l.props.find_group(stroke).unwrap().sub("dashes").unwrap().get(m).unwrap().uid)
}

/// A white 200×200 solid in a 200×200 comp with a 100×100 rectangular mask (50…150).
fn masked_solid(s: &mut Session) -> (u64, u64) {
    let l = s.execute("layer.newSolid", json!({"color": "#ffffff", "width": 200, "height": 200})).unwrap()["layer"].as_u64().unwrap();
    let r = s.execute("mask.new", json!({"layer": l, "vertices": [[50, 50], [150, 50], [150, 150], [50, 150]], "closed": true})).unwrap();
    (l, r["mask"].as_u64().unwrap())
}

fn render_at(s: &Session, t: f64) -> effectcraft_raster::Image {
    let cid = s.active_comp_id().unwrap();
    s.render(cid, effectcraft_time::Tick::from_seconds_f64(t), Default::default())
}

fn alpha_at(s: &Session, x: u32, y: u32, t: f64) -> f32 {
    let img = render_at(s, t);
    img.data[(y * img.width + x) as usize][3]
}

#[test]
fn variable_mask_feather_points() {
    let mut s = comp(200, 200);
    let (l, m) = masked_solid(&mut s);
    s.execute("layer.mask.featherFalloff", json!({"layer": l, "mode": "linear"})).unwrap();
    assert!(alpha_at(&s, 100, 45, 0.0) < 0.01);
    // Mask Feather tool: click on the path and drag out above the top edge → an outer point of 20 px.
    let r = s.execute("mask.featherPoint.add", json!({"layer": l, "mask": m, "point": [100, 30]})).unwrap();
    assert_eq!(r["index"], json!(0));
    let pts = s.execute("mask.featherPoint.list", json!({"layer": l, "mask": m})).unwrap();
    let p0 = &pts["points"][0];
    assert_eq!(p0["segment"], json!(0));
    assert!((p0["t"].as_f64().unwrap() - 0.5).abs() < 1e-3 && (p0["radius"].as_f64().unwrap() - 20.0).abs() < 1e-3, "{pts}");
    let soft = alpha_at(&s, 100, 39, 0.0);
    assert!((soft - 0.475).abs() < 0.06, "{soft}");
    // An inner point dragged inside the bottom edge.
    s.execute("mask.featherPoint.add", json!({"layer": l, "mask": m, "point": [100, 130], "tension": 100})).unwrap();
    let pts = s.execute("mask.featherPoint.list", json!({"layer": l, "mask": m})).unwrap();
    assert!((pts["points"][1]["radius"].as_f64().unwrap() + 20.0).abs() < 1e-3, "{pts}");
    assert!((alpha_at(&s, 100, 140, 0.0) - 0.475).abs() < 0.08, "{}", alpha_at(&s, 100, 140, 0.0));
    assert!(alpha_at(&s, 100, 152, 0.0) < 0.01);
    // Drag the radius; undo; redo.
    s.execute("mask.featherPoint.set", json!({"layer": l, "mask": m, "index": 0, "radius": 40})).unwrap();
    assert!(alpha_at(&s, 100, 39, 0.0) > 0.7);
    s.execute("edit.undo", json!({})).unwrap();
    assert!((alpha_at(&s, 100, 39, 0.0) - soft).abs() < 0.08);
    s.execute("edit.redo", json!({})).unwrap();
    // Inserting a vertex keeps the points where they were on the path.
    let before = alpha_at(&s, 100, 39, 0.0);
    s.execute("mask.insertVertex", json!({"layer": l, "mask": m, "segment": 0, "t": 0.25})).unwrap();
    let pts = s.execute("mask.featherPoint.list", json!({"layer": l, "mask": m})).unwrap();
    assert_eq!(pts["points"][0]["segment"], json!(1));
    assert!((pts["points"][0]["t"].as_f64().unwrap() - 1.0 / 3.0).abs() < 1e-6);
    assert!((alpha_at(&s, 100, 39, 0.0) - before).abs() < 0.02);
    // Serde: the points travel in the Mask Path value.
    let back = roundtrip(&s);
    let lay = back.comp(s.active_comp_id().unwrap()).unwrap().layer(LayerId(l)).unwrap();
    let effectcraft_keyframe::Value::Path(sp) = &lay.props.find_group(m).unwrap().get("path").unwrap().value else { panic!() };
    assert_eq!(sp.feather.len(), 2);
    s.execute("mask.featherPoint.remove", json!({"layer": l, "mask": m, "all": true})).unwrap();
    assert!(alpha_at(&s, 100, 39, 0.0) < 0.01);
    assert!(s.execute("mask.featherPoint.set", json!({"layer": l, "mask": m, "index": 0, "radius": 1})).is_err());
}

#[test]
fn mask_motion_blur_ramps_across_the_motion() {
    let mut s = comp(200, 200);
    let (l, m) = masked_solid(&mut s);
    s.execute("comp.settings", json!({"shutterAngle": 360, "shutterPhase": 0})).unwrap();
    s.execute("layer.mask.motionBlur", json!({"layer": l, "mode": "on"})).unwrap();
    s.execute("prop.addKey", json!({"layer": l, "path": "masks/#1/path", "time": 0.0})).unwrap();
    s.execute("time.set", json!({"time": 1.0})).unwrap();
    // 30 px in 30 frames: 1 px per frame; with a 360° shutter at frame 15 the right edge sweeps
    // 165…166, so pixel 165 is half covered.
    let verts: Vec<Value> = (0..4).map(|i| json!({"layer": l, "mask": m, "index": i})).collect();
    s.execute("mask.moveVertices", json!({"vertices": verts, "delta": [30, 0]})).unwrap();
    let a = alpha_at(&s, 165, 100, 0.5);
    assert!(a > 0.3 && a < 0.7, "{a}");
    s.execute("layer.mask.motionBlur", json!({"layer": l, "mode": "off"})).unwrap();
    let hard = alpha_at(&s, 165, 100, 0.5);
    assert!(!(0.3..0.7).contains(&hard), "{hard}");
}

#[test]
fn adaptive_sample_limit_drives_layer_samples() {
    assert_eq!(effectcraft_render::adaptive_samples(0.5, 16, 128), 16);
    assert_eq!(effectcraft_render::adaptive_samples(100.0, 16, 128), 100);
    assert_eq!(effectcraft_render::adaptive_samples(400.0, 16, 128), 128);
    assert_eq!(effectcraft_render::adaptive_samples(400.0, 16, 9999), 256);
    let mut s = comp(400, 100);
    s.execute("comp.settings", json!({"shutterAngle": 360, "shutterPhase": 0})).unwrap();
    let l = s.execute("layer.newSolid", json!({"color": "#ffffff", "width": 20, "height": 20})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setSwitch", json!({"layers": [l], "switch": "motionBlur", "value": true})).unwrap();
    // 100 px per frame (keys 3 frames apart, 300 px).
    s.execute("prop.addKey", json!({"layer": l, "path": "transform/position", "time": 0.0, "value": [50, 50]})).unwrap();
    s.execute("prop.addKey", json!({"layer": l, "path": "transform/position", "time": 0.1, "value": [350, 50]})).unwrap();
    let max_step = |s: &Session| {
        let img = render_at(s, 1.0 / 30.0);
        let row: Vec<f32> = (0..img.width).map(|x| img.data[(50 * img.width + x) as usize][3]).collect();
        row.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max)
    };
    s.execute("comp.settings", json!({"adaptiveSampleLimit": 16})).unwrap();
    let coarse = max_step(&s);
    s.execute("comp.settings", json!({"adaptiveSampleLimit": 256})).unwrap();
    let fine = max_step(&s);
    assert!(coarse > 0.05, "16 samples over 100 px leave visible steps: {coarse}");
    assert!(fine < 0.03, "≈100 samples give a smooth ramp: {fine}");
    assert_eq!(s.active_comp().unwrap().motion_blur_adaptive_limit, 256);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.active_comp().unwrap().motion_blur_adaptive_limit, 16);
    assert_eq!(roundtrip(&s).comp(s.active_comp_id().unwrap()).unwrap().motion_blur_adaptive_limit, 16);
    // Older projects without the field read the After Effects default.
    fn strip(v: &mut Value) {
        match v {
            Value::Object(o) => {
                o.remove("motion_blur_adaptive_limit");
                o.values_mut().for_each(strip);
            }
            Value::Array(a) => a.iter_mut().for_each(strip),
            _ => {}
        }
    }
    let mut v: Value = serde_json::from_str(&s.project.to_json()).unwrap();
    strip(&mut v);
    let json = v.to_string();
    assert!(!json.contains("motion_blur_adaptive_limit"));
    assert_eq!(Project::from_json(&json).unwrap().comp(s.active_comp_id().unwrap()).unwrap().motion_blur_adaptive_limit, 128);
}

#[test]
fn keyframe_label_groups() {
    let mut s = comp(200, 100);
    let a = s.execute("layer.newSolid", json!({"color": "#ff0000", "width": 20, "height": 20})).unwrap()["layer"].as_u64().unwrap();
    let b = s.execute("layer.newSolid", json!({"color": "#00ff00", "width": 20, "height": 20})).unwrap()["layer"].as_u64().unwrap();
    for l in [a, b] {
        for t in [0.0, 0.5, 1.0] {
            s.execute("prop.addKey", json!({"layer": l, "path": "transform/opacity", "time": t})).unwrap();
        }
    }
    let uid = |s: &Session, l: u64| s.active_comp().unwrap().layer(LayerId(l)).unwrap().props.prop("transform/opacity").unwrap().uid;
    let (ua, ub) = (uid(&s, a), uid(&s, b));
    // Label a's first key and b's last key Blue.
    s.execute("keys.select", json!({"keys": [{"layer": a, "prop": ua, "time": 0.0}, {"layer": b, "prop": ub, "time": 1.0}]})).unwrap();
    s.execute("keys.setLabel", json!({"label": "Blue"})).unwrap();
    s.execute("keys.select", json!({"keys": [{"layer": a, "prop": ua, "time": 0.5}]})).unwrap();
    s.execute("keys.setLabel", json!({"label": "Green"})).unwrap();
    // On selected layers: from a's first key, only a's Blue key.
    s.execute("layer.select", json!({"layers": [a]})).unwrap();
    s.execute("keys.select", json!({"keys": [{"layer": a, "prop": ua, "time": 0.0}]})).unwrap();
    assert_eq!(s.execute("keys.selectLabelGroup", json!({"scope": "selected"})).unwrap()["keys"], json!(1));
    // On all layers: both Blue keys.
    assert_eq!(s.execute("keys.selectLabelGroup", json!({"scope": "all"})).unwrap()["keys"], json!(2));
    assert!(s.state.selected_keys.iter().any(|k| k.layer == LayerId(b) && (k.time.seconds() - 1.0).abs() < 1e-6));
    // Visible keyframes: only the revealed properties count.
    s.execute("keys.select", json!({"keys": [{"layer": a, "prop": ua, "time": 0.0}]})).unwrap();
    assert_eq!(s.execute("keys.selectLabelGroup", json!({"scope": "visibleAll", "visible": [ua]})).unwrap()["keys"], json!(1));
    // Unlabelled keys group together too (label None).
    s.execute("keys.select", json!({"keys": [{"layer": a, "prop": ua, "time": 1.0}]})).unwrap();
    assert_eq!(s.execute("keys.selectLabelGroup", json!({"scope": "all"})).unwrap()["keys"], json!(3));
    // Undo, serde.
    let lab = |s: &Session| s.active_comp().unwrap().layer(LayerId(a)).unwrap().props.prop("transform/opacity").unwrap().keys[1].label;
    assert_eq!(lab(&s), 9);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(lab(&s), 0);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(roundtrip(&s).comp(s.active_comp_id().unwrap()).unwrap().layer(LayerId(a)).unwrap().props.prop("transform/opacity").unwrap().keys[1].label, 9);
    assert!(s.execute("keys.setLabel", json!({"label": "Mauve"})).is_err());
}

#[test]
fn tate_chu_yoko_via_set_text_with_undo_and_serde() {
    let mut s = comp(400, 400);
    let t = s.execute("layer.newText", json!({"text": "A12B", "size": 40})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setText", json!({"layer": t, "vertical": true})).unwrap();
    s.execute("layer.setText", json!({"layer": t, "range": [1, 3], "tateChuYoko": true})).unwrap();
    let doc = crate::commands::text_edit::layer_doc(&s, LayerId(t)).unwrap();
    assert!(doc.style_at(1).tate_chu_yoko && doc.style_at(2).tate_chu_yoko && !doc.style_at(0).tate_chu_yoko);
    let lay = effectcraft_text::layout_doc(&doc);
    assert!((lay.glyphs[1].origin.y - lay.glyphs[2].origin.y).abs() < 1e-6, "the digits share a row");
    let back = roundtrip(&s);
    let effectcraft_keyframe::Value::Text(d) =
        &back.comp(s.active_comp_id().unwrap()).unwrap().layer(LayerId(t)).unwrap().props.prop("text/sourceText").unwrap().value
    else {
        panic!()
    };
    assert!(d.style_at(1).tate_chu_yoko);
    s.execute("edit.undo", json!({})).unwrap();
    assert!(!crate::commands::text_edit::layer_doc(&s, LayerId(t)).unwrap().style_at(1).tate_chu_yoko);
    s.execute("layer.setText", json!({"layer": t, "verticalRomanUpright": true})).unwrap();
    assert!(crate::commands::text_edit::layer_doc(&s, LayerId(t)).unwrap().vertical_roman_upright);
}

#[test]
fn variable_font_axes_animator() {
    let mut s = comp(400, 200);
    let t = s.execute("layer.newText", json!({"text": "HH", "size": 60})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.select", json!({"layers": [t]})).unwrap();
    assert!(s.is_enabled("text.animatorFontAxes"));
    // Inter (bundled) is static.
    assert_eq!(s.execute("text.animatorFontAxes", json!({})).unwrap()["axes"], json!([]));
    assert!(s.execute("text.animatorFontAxes", json!({"axis": "wght"})).is_err());
    let Some((face, axes)) = effectcraft_text::variable::find_variable_face() else {
        eprintln!("no variable font installed: animation check skipped");
        return;
    };
    let info = effectcraft_text::fonts::face(face).info.clone();
    s.execute("layer.setText", json!({"layer": t, "font": info.family, "style": info.style})).unwrap();
    let axis = axes.iter().find(|a| a.max > a.min).unwrap().clone();
    let listed = s.execute("text.animatorFontAxes", json!({})).unwrap();
    if listed["axes"].as_array().is_none_or(|a| a.is_empty()) {
        eprintln!("the variable face isn't reachable by family/style: skipped");
        return;
    }
    let tag = axis.tag.clone();
    let r = s.execute("text.animatorFontAxes", json!({"axis": tag})).unwrap();
    let anim = r["animator"].as_u64().unwrap();
    let area = |s: &Session| {
        let cid = s.active_comp_id().unwrap();
        let comp = s.project.comp(cid).unwrap();
        let ctx = effectcraft_render::EvalCtx::new(&s.project, cid, comp, s.time());
        let l = comp.layer(LayerId(t)).unwrap();
        effectcraft_render::text::glyph_paths(&ctx, l).iter().map(|(p, _)| effectcraft_text::kurbo::Shape::area(p).abs()).sum::<f64>()
    };
    // The distance between the two H's left edges: the H advance.
    let gap = |s: &Session| {
        let cid = s.active_comp_id().unwrap();
        let comp = s.project.comp(cid).unwrap();
        let ctx = effectcraft_render::EvalCtx::new(&s.project, cid, comp, s.time());
        let l = comp.layer(LayerId(t)).unwrap();
        let x0: Vec<f64> = effectcraft_render::text::glyph_paths(&ctx, l).iter().map(|(p, _)| effectcraft_text::kurbo::Shape::bounding_box(p).x0).collect();
        x0[1] - x0[0]
    };
    let before = area(&s);
    let gap_before = gap(&s);
    let l = s.active_comp().unwrap().layer(LayerId(t)).unwrap().clone();
    let pg = l.props.find_group(anim).unwrap().sub("properties").unwrap().props().next().unwrap().uid;
    let target = if axis.max - axis.default >= axis.default - axis.min { axis.max } else { axis.min };
    let range = (target - axis.default) as f64;
    s.execute("prop.set", json!({"layer": t, "prop": pg, "value": range})).unwrap();
    let after = area(&s);
    assert!((after - before).abs() > 1.0, "{} axis moved the outline: {before} → {after}", axis.tag);
    // Advances follow the axis too (the text is re-spaced, not only redrawn).
    let a = axes.iter().find(|a| a.tag == tag).unwrap();
    let f = effectcraft_text::fonts::face(face);
    let gid = f.glyph('H').unwrap();
    let upem = f.units_per_em() as f64;
    let units = |v: f32| effectcraft_text::variable::advance_units_at(face, gid, &[(a.tag.clone(), v)]).unwrap() as f64;
    let want = (units((a.default + range as f32).clamp(a.min, a.max)) - units(a.default)) * 60.0 / upem;
    let moved = gap(&s) - gap_before;
    assert!((moved - want).abs() < 0.5, "{} axis: advance change {moved}, expected {want}", a.tag);
    s.execute("edit.undo", json!({})).unwrap();
    assert!((area(&s) - before).abs() < 1e-6);
    let _ = info;
}

#[test]
fn variable_font_axes_in_the_character_style() {
    let mut s = comp(600, 200);
    let t = s.execute("layer.newText", json!({"text": "Hamburg", "size": 60})).unwrap()["layer"].as_u64().unwrap();
    // The attribute round-trips and is undoable even for static fonts (it is kept, unused).
    s.execute("layer.setText", json!({"layer": t, "variations": {"wght": 650}})).unwrap();
    let d = crate::commands::text_edit::layer_doc(&s, LayerId(t)).unwrap();
    assert_eq!(d.style_at(0).variations, vec![("wght".to_string(), 650.0)]);
    assert_eq!(effectcraft_keyframe::text_doc::char_style_json(&d.style_at(0))["variations"], json!({"wght": 650.0}));
    s.execute("layer.setText", json!({"layer": t, "range": [0, 3], "variations": {"wght": null}})).unwrap();
    let d = crate::commands::text_edit::layer_doc(&s, LayerId(t)).unwrap();
    assert!(d.style_at(0).variations.is_empty() && !d.style_at(5).variations.is_empty());
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert!(crate::commands::text_edit::layer_doc(&s, LayerId(t)).unwrap().style_at(0).variations.is_empty());
    assert!(s.execute("layer.setText", json!({"layer": t, "variations": 3})).is_err());
    let Some((face, axes)) = effectcraft_text::variable::find_variable_face() else {
        eprintln!("no variable font installed: rendering check skipped");
        return;
    };
    let info = effectcraft_text::fonts::face(face).info.clone();
    if effectcraft_text::resolve(&info.family, &info.style).face != face {
        eprintln!("variable face not reachable by family/style: skipped");
        return;
    }
    s.execute("layer.setText", json!({"layer": t, "font": info.family, "style": info.style})).unwrap();
    let a = axes.iter().find(|a| a.tag == "wght" && a.max > a.min).or_else(|| axes.iter().find(|a| a.max > a.min)).unwrap().clone();
    let geom = |s: &Session| {
        let cid = s.active_comp_id().unwrap();
        let comp = s.project.comp(cid).unwrap();
        let ctx = effectcraft_render::EvalCtx::new(&s.project, cid, comp, s.time());
        let l = comp.layer(LayerId(t)).unwrap();
        let paths = effectcraft_render::text::glyph_paths(&ctx, l);
        let area: f64 = paths.iter().map(|(p, _)| effectcraft_text::kurbo::Shape::area(p).abs()).sum();
        let right = paths.iter().map(|(p, _)| effectcraft_text::kurbo::Shape::bounding_box(p).x1).fold(f64::MIN, f64::max);
        (area, right)
    };
    s.execute("layer.setText", json!({"layer": t, "variations": {a.tag.trim_end(): a.min}})).unwrap();
    let lo = geom(&s);
    s.execute("layer.setText", json!({"layer": t, "variations": {a.tag.trim_end(): a.max}})).unwrap();
    let hi = geom(&s);
    assert!((hi.0 - lo.0).abs() > 1.0, "{} outlines {lo:?} → {hi:?}", a.tag);
    assert!((hi.1 - lo.1).abs() > 0.5, "{} advances {lo:?} → {hi:?}", a.tag);
}

#[test]
fn camera_focus_commands_undo_and_serde() {
    let mut s = comp(400, 300);
    let t = s.execute("layer.newSolid", json!({"color": "#ffffff", "width": 50, "height": 50})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setSwitch", json!({"layers": [t], "switch": "threeD", "value": true})).unwrap();
    s.execute("layer.setTransform", json!({"layer": t, "prop": "position", "value": [200, 150, 500]})).unwrap();
    let cam = s.execute("layer.newCamera", json!({})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.select", json!({"layers": [cam, t]})).unwrap();
    assert!(s.is_enabled("camera.setFocusToLayer"));
    let before = s.execute("prop.get", json!({"layer": cam, "path": "cameraOptions/focusDistance"})).unwrap()["value"].as_f64().unwrap();
    let d = s.execute("camera.setFocusToLayer", json!({})).unwrap()["focusDistance"].as_f64().unwrap();
    assert!((d - before - 500.0).abs() < 1e-6, "the default camera focuses on z = 0: {before} → {d}");
    s.execute("camera.linkFocusToLayer", json!({})).unwrap();
    let e = s.execute("prop.get", json!({"layer": cam, "path": "cameraOptions/focusDistance"})).unwrap();
    assert!(e["expression"].as_str().unwrap().contains("thisComp.layer("), "{e}");
    let back = roundtrip(&s);
    assert!(back.comp(s.active_comp_id().unwrap()).unwrap().layer(LayerId(cam)).unwrap().props.prop("cameraOptions/focusDistance").unwrap().expr.is_some());
    s.execute("edit.undo", json!({})).unwrap();
    let e = s.execute("prop.get", json!({"layer": cam, "path": "cameraOptions/focusDistance"})).unwrap();
    assert!(e["expression"].is_null());
    s.execute("edit.undo", json!({})).unwrap();
    let v = s.execute("prop.get", json!({"layer": cam, "path": "cameraOptions/focusDistance"})).unwrap()["value"].as_f64().unwrap();
    assert!((v - before).abs() < 1e-9);
    s.execute("edit.redo", json!({})).unwrap();
    // A camera alone can't link to a layer, but can link to its point of interest.
    s.execute("layer.select", json!({"layers": [cam]})).unwrap();
    assert!(s.execute("camera.linkFocusToLayer", json!({})).is_err());
    assert!(s.execute("camera.linkFocusToPoi", json!({})).is_ok());
}

/// Set Focus Distance to Layer: degenerate targets (behind the camera, exactly at it) are clean
/// errors, and a depth-of-field frame renders right after the command.
#[test]
fn camera_set_focus_to_layer_degenerate_targets_and_dof_frame() {
    let mut s = comp(400, 300);
    let t = s.execute("layer.newSolid", json!({"color": "#ffffff", "width": 50, "height": 50})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setSwitch", json!({"layers": [t], "switch": "threeD", "value": true})).unwrap();
    let cam = s.execute("layer.newCamera", json!({})).unwrap()["layer"].as_u64().unwrap();
    s.execute("prop.set", json!({"layer": cam, "path": "transform/position", "value": [200, 150, -500]})).unwrap();
    s.execute("prop.set", json!({"layer": cam, "path": "cameraOptions/dof", "value": true})).unwrap();
    s.execute("layer.select", json!({"layers": [cam, t]})).unwrap();
    // Behind the camera (which now sits at z = −500 looking towards +z).
    s.execute("layer.setTransform", json!({"layer": t, "prop": "position", "value": [200, 150, -1000]})).unwrap();
    let e = s.execute("camera.setFocusToLayer", json!({})).unwrap_err().to_string();
    assert!(e.contains("behind the camera"), "{e}");
    // Exactly at the camera: depth 0, still a clean error.
    s.execute("layer.setTransform", json!({"layer": t, "prop": "position", "value": [200, 150, -500]})).unwrap();
    assert!(s.execute("camera.setFocusToLayer", json!({})).is_err());
    // In front: the command works, and the depth-of-field frame renders with finite pixels.
    s.execute("layer.setTransform", json!({"layer": t, "prop": "position", "value": [200, 150, 400]})).unwrap();
    let d = s.execute("camera.setFocusToLayer", json!({})).unwrap()["focusDistance"].as_f64().unwrap();
    assert!((d - 900.0).abs() < 1e-6, "{d}");
    // An out-of-focus 3D layer, which must visibly blur, and a 0%-scaled one, whose pixels cover
    // ~0 screen pixels and so turn an ordinary defocus into a radius of ~1e9 buffer pixels.
    s.execute("prop.set", json!({"layer": cam, "path": "cameraOptions/aperture", "value": 120})).unwrap();
    let blur = s.execute("layer.newSolid", json!({"color": "#00ff00", "width": 60, "height": 60})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setSwitch", json!({"layers": [blur], "switch": "threeD", "value": true})).unwrap();
    s.execute("layer.setTransform", json!({"layer": blur, "prop": "position", "value": [200, 150, 1200]})).unwrap();
    let invis = s.execute("layer.newSolid", json!({"color": "#ff0000", "width": 40, "height": 40})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setSwitch", json!({"layers": [invis], "switch": "threeD", "value": true})).unwrap();
    s.execute("layer.setTransform", json!({"layer": invis, "prop": "position", "value": [200, 150, 1600]})).unwrap();
    s.execute("layer.setTransform", json!({"layer": invis, "prop": "scale", "value": [0.0, 0.0, 0.0]})).unwrap();
    let cid = s.active_comp_id().unwrap();
    let img = s.render(cid, effectcraft_time::Tick::ZERO, Default::default());
    assert!(img.data.iter().all(|p| p.iter().all(|c| c.is_finite())), "non-finite pixels after setFocusToLayer");
    // The DoF frame actually blurred (not a no-op blur): the same frame with DoF off differs.
    s.execute("prop.set", json!({"layer": cam, "path": "cameraOptions/dof", "value": false})).unwrap();
    let plain = s.render(cid, effectcraft_time::Tick::ZERO, Default::default());
    assert_ne!(img.data, plain.data, "the out-of-focus layer should have blurred");
}

/// Set Focus Distance to Layer fails cleanly on a depth that isn't finite. Every stored number
/// here is finite, but their difference overflows f64 to infinity; JSON has no infinity, so
/// storing it would leave a project that can't be saved.
#[test]
fn camera_set_focus_to_layer_rejects_non_finite_depth() {
    let mut s = comp(400, 300);
    let t = s.execute("layer.newSolid", json!({"color": "#ffffff", "width": 50, "height": 50})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setSwitch", json!({"layers": [t], "switch": "threeD", "value": true})).unwrap();
    s.execute("layer.setTransform", json!({"layer": t, "prop": "position", "value": [200.0, 150.0, 1e308]})).unwrap();
    let cam = s.execute("layer.newCamera", json!({})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setTransform", json!({"layer": cam, "prop": "position", "value": [200.0, 150.0, -1e308]})).unwrap();
    s.execute("layer.select", json!({"layers": [cam, t]})).unwrap();
    let before = s.execute("prop.get", json!({"layer": cam, "path": "cameraOptions/focusDistance"})).unwrap()["value"].as_f64().unwrap();
    let e = s.execute("camera.setFocusToLayer", json!({})).unwrap_err().to_string();
    assert!(e.contains("out of range"), "{e}");
    let after = s.execute("prop.get", json!({"layer": cam, "path": "cameraOptions/focusDistance"})).unwrap()["value"].as_f64().unwrap();
    assert!((after - before).abs() < 1e-9, "a non-finite focus distance was written: {after}");
    // The project still round-trips through serde (a stored infinity would come back as null).
    let back = roundtrip(&s);
    let f = back.comp(s.active_comp_id().unwrap()).unwrap().layer(LayerId(cam)).unwrap().props.prop("cameraOptions/focusDistance").unwrap().value.as_f64();
    assert!((f - before).abs() < 1e-9, "serde focus distance: {f}");
}

/// An id sent as a string (`"98"`, as MCP clients often do) resolves as an id when no name matches.
#[test]
fn numeric_string_resolves_as_id() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "T", "width": 320, "height": 180, "duration": 2.0, "frameRate": 30})).unwrap();
    let l = s.execute("layer.newSolid", json!({"color": "#ff0000"})).unwrap()["layer"].as_u64().unwrap();
    let cid = s.active_comp_id().unwrap();
    assert_eq!(s.resolve_comp(Some(&json!(cid.0.to_string()))).unwrap(), cid);
    assert!(s.resolve_comp(Some(&json!("999999"))).is_err());
    let comp = s.project.comp(cid).unwrap();
    assert_eq!(crate::commands::resolve_layer(comp, &json!(l.to_string())), Some(LayerId(l)));
    assert_eq!(crate::commands::resolve_layer(comp, &json!("999999")), None);
    let r = s.execute("layer.tree", json!({"layer": l.to_string(), "comp": cid.0.to_string()})).unwrap();
    assert_eq!(r["id"].as_u64(), Some(l), "{r}");
}

/// Go to Work Area Start / End have After Effects' default keys (Shift+Home / Shift+End).
#[test]
fn work_area_navigation_has_after_effects_default_keys() {
    let s = Session::default();
    for (id, key) in [("time.workAreaStart", "Shift+Home"), ("time.workAreaEnd", "Shift+End")] {
        let b = s.shortcuts().find(id).unwrap_or_else(|| panic!("{id} is bindable"));
        assert_eq!(b.defaults, [key.to_string()], "{id}");
    }
}
