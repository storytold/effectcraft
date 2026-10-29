//! Headless checks (egui_kittest) of the visual editors for hidden / text effect parameters
//! (M13.10): Lumetri's RGB and hue/saturation curves, Colorama's output cycle wheel, Glow's
//! colour map and Reshape's correspondence points (Effect Controls buttons and viewer handles).
//! Each edit goes through `prop.set` and undoes in one step.

use effectcraft_engine::Session;
use effectcraft_engine::keyframe::Value as KV;
use effectcraft_engine::project::{LayerId, Node, PropGroup};
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use effectcraft_ui_egui::dock::WorkspaceExt as _;
use egui::{Event, Pos2, pos2};
use egui_kittest::Harness;
use serde_json::json;

fn harness(s: Session) -> Harness<'static, EffectcraftApp> {
    let mut app = EffectcraftApp::new(s);
    app.show_panel(PanelKind::EffectControls);
    app.toggle_maximize(PanelKind::EffectControls);
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1400.0)).build_eframe(|_| app);
    h.run_steps(4);
    h
}

/// A comp with one 320×180 solid carrying `effect`; returns the session, layer and effect uid.
fn with_effect(effect: &str) -> (Session, u64, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Fx", "width": 320, "height": 180, "frameRate": 24, "duration": 4})).unwrap();
    let l = s.execute("layer.newSolid", json!({"name": "S", "color": "#6080a0"})).unwrap()["layer"].as_u64().unwrap();
    let fx = s.execute("effect.apply", json!({"layers": [l], "effect": effect})).unwrap()["effects"][0].as_u64().unwrap();
    s.state.selected_layers = vec![LayerId(l)];
    (s, l, fx)
}

fn group(h: &Harness<'_, EffectcraftApp>, layer: u64, fx: u64) -> PropGroup {
    h.state().session.active_comp().unwrap().layer(LayerId(layer)).unwrap().props.find_group(fx).unwrap().clone()
}

/// A parameter's value by its spec-id path inside the effect (`curves/rgbCurves/master`).
fn value(h: &Harness<'_, EffectcraftApp>, layer: u64, fx: u64, path: &str) -> KV {
    let g = group(h, layer, fx);
    let mut cur = &g;
    let parts: Vec<&str> = path.split('/').collect();
    for p in &parts[..parts.len() - 1] {
        cur = cur.sub(p).unwrap_or_else(|| panic!("no group {p}"));
    }
    cur.get(parts[parts.len() - 1]).unwrap().value.clone()
}

fn s(v: KV) -> String {
    match v {
        KV::Str(s) => s,
        other => panic!("{other:?}"),
    }
}

/// Close every parameter group of the effect except those named (and their parents).
fn only_groups(h: &mut Harness<'_, EffectcraftApp>, layer: u64, fx: u64, keep: &[&str]) {
    fn walk(g: &PropGroup, keep: &[&str], out: &mut Vec<u64>) {
        for c in &g.children {
            if let Node::Group(sg) = c {
                if !keep.contains(&sg.match_id.as_str()) {
                    out.push(sg.uid);
                }
                walk(sg, keep, out);
            }
        }
    }
    let g = group(h, layer, fx);
    let mut closed = vec![];
    walk(&g, keep, &mut closed);
    h.state_mut().ui.fx_closed.extend(closed);
    h.run_steps(3);
}

fn rect(h: &Harness<'_, EffectcraftApp>, id: &str) -> egui::Rect {
    let e = h.state().auto.previous.iter().chain(h.state().auto.elements.iter()).find(|e| e.id == id).cloned().unwrap_or_else(|| {
        let near: Vec<String> = h
            .state()
            .auto
            .elements
            .iter()
            .map(|e| e.id.clone())
            .filter(|i| i.starts_with("effectControls.effect") || i.starts_with("viewer.reshape"))
            .collect();
        panic!("no element {id}; have {near:?}")
    });
    egui::Rect::from_min_size(pos2(e.rect[0], e.rect[1]), egui::vec2(e.rect[2], e.rect[3]))
}

fn at(r: egui::Rect, fx: f32, fy: f32) -> Pos2 {
    pos2(r.min.x + r.width() * fx, r.min.y + r.height() * fy)
}

fn click_n(h: &mut Harness<'_, EffectcraftApp>, p: Pos2, n: u32) {
    h.event(Event::PointerMoved(p));
    h.step();
    // (all in one frame: the harness' frame time is longer than a double-click's)
    for _ in 0..n {
        h.event(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
        h.event(Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    }
    h.step();
    h.run_steps(2);
}

fn click_id(h: &mut Harness<'_, EffectcraftApp>, id: &str) {
    let p = rect(h, id).center();
    click_n(h, p, 1);
}

fn drag(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2) {
    h.event(Event::PointerMoved(from));
    h.step();
    h.event(Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    for k in 1..=6 {
        let t = k as f32 / 6.0;
        h.event(Event::PointerMoved(from + (to - from) * t));
        h.step();
    }
    h.event(Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(3);
}

fn undo(h: &mut Harness<'_, EffectcraftApp>) {
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    h.run_steps(2);
}

#[test]
fn lumetri_rgb_and_hue_saturation_curve_editors() {
    let (s0, l, fx) = with_effect("Lumetri Color");
    let mut h = harness(s0);
    only_groups(&mut h, l, fx, &["curves", "rgbCurves", "hueSaturationCurves"]);
    let pre = format!("effectControls.effect.{fx}.lumetri.rgbCurves");
    for k in 0..4 {
        rect(&h, &format!("{pre}.channel.{k}"));
    }
    rect(&h, &format!("{pre}.point.0"));
    // Red channel: a click in the graph adds a point to the red curve only.
    click_id(&mut h, &format!("{pre}.channel.1"));
    let gr = rect(&h, &format!("{pre}.graph"));
    click_n(&mut h, at(gr, 0.5, 0.25), 1);
    let red = s(value(&h, l, fx, "curves/rgbCurves/red"));
    assert_eq!(red.split_whitespace().count(), 3, "{red}");
    assert_eq!(s(value(&h, l, fx, "curves/rgbCurves/master")), "0,0 1,1");
    rect(&h, &format!("{pre}.point.2"));
    undo(&mut h);
    assert_eq!(s(value(&h, l, fx, "curves/rgbCurves/red")), "0,0 1,1");
    // Hue vs Hue: pick the tab, add a point, drag it: one undo step for the drag.
    let hs = format!("effectControls.effect.{fx}.lumetri.hueSatCurves");
    click_id(&mut h, &format!("{hs}.curve.1"));
    let gr = rect(&h, &format!("{hs}.graph"));
    click_n(&mut h, at(gr, 0.3, 0.5), 1);
    let v = s(value(&h, l, fx, "curves/hueSaturationCurves/hueVsHue"));
    assert_eq!(v.split_whitespace().count(), 1, "{v}");
    assert_eq!(s(value(&h, l, fx, "curves/hueSaturationCurves/hueVsSat")), "");
    let p0 = rect(&h, &format!("{hs}.point.0")).center();
    let undo_len = h.state().session.history.undo.len();
    drag(&mut h, p0, p0 + egui::vec2(0.0, -gr.height() * 0.25));
    let v = s(value(&h, l, fx, "curves/hueSaturationCurves/hueVsHue"));
    let y: f32 = v.split(',').nth(1).unwrap().parse().unwrap();
    assert!(y > 0.7, "{v}");
    assert_eq!(h.state().session.history.undo.len(), undo_len + 1, "one undo step per drag");
    // Reset empties the curve.
    click_id(&mut h, &format!("{hs}.reset"));
    assert_eq!(s(value(&h, l, fx, "curves/hueSaturationCurves/hueVsHue")), "");
}

#[test]
fn colorama_output_cycle_wheel() {
    let (s0, l, fx) = with_effect("Colorama");
    let mut h = harness(s0);
    only_groups(&mut h, l, fx, &["outputCycle"]);
    let pre = format!("effectControls.effect.{fx}.colorama");
    // The Hue Cycle preset has six stops.
    for i in 0..6 {
        rect(&h, &format!("{pre}.stop.{i}"));
    }
    // Clicking the ring adds a stop: the palette becomes Custom with seven stops.
    let w = rect(&h, &format!("{pre}.wheel"));
    let mid = w.width() / 2.0 - 20.0 - 9.0;
    let ring = pos2(w.center().x + mid * std::f32::consts::FRAC_1_SQRT_2, w.center().y + mid * std::f32::consts::FRAC_1_SQRT_2);
    click_n(&mut h, ring, 1);
    assert_eq!(value(&h, l, fx, "outputCycle/usePresetPalette"), KV::Enum(8));
    let pal = s(value(&h, l, fx, "outputCycle/palette"));
    assert_eq!(pal.split_whitespace().count(), 7, "{pal}");
    rect(&h, &format!("{pre}.stop.6"));
    // Dragging a stop moves its position.
    let st = rect(&h, &format!("{pre}.stop.0")).center();
    drag(&mut h, st, pos2(w.max.x - 6.0, w.center().y));
    let pal2 = s(value(&h, l, fx, "outputCycle/palette"));
    assert_ne!(pal, pal2);
    assert!(pal2.split_whitespace().any(|t| t.starts_with("0.25") || t.starts_with("0.24") || t.starts_with("0.26")), "{pal2}");
    rect(&h, &format!("{pre}.color"));
}

#[test]
fn glow_arbitrary_map_editor() {
    let (mut s0, l, fx) = with_effect("Glow");
    let colors = s0.active_comp().unwrap().layer(LayerId(l)).unwrap().props.find_group(fx).unwrap().get("colors").unwrap().uid;
    s0.execute("prop.set", json!({"layer": l, "prop": colors, "value": 1})).unwrap();
    let mut h = harness(s0);
    let pre = format!("effectControls.effect.{fx}.glow");
    rect(&h, &format!("{pre}.colorMap"));
    assert!(!h.state().auto.elements.iter().any(|e| e.id == format!("{pre}.graph")), "A & B: just the strip");
    h.state_mut().session.execute("prop.set", json!({"layer": l, "prop": colors, "value": 2})).unwrap();
    h.run_steps(3);
    click_id(&mut h, &format!("{pre}.channel.2"));
    let gr = rect(&h, &format!("{pre}.graph"));
    click_n(&mut h, at(gr, 0.5, 0.9), 1);
    let m = s(value(&h, l, fx, "arbitraryMap"));
    let parts: Vec<&str> = m.split('|').map(str::trim).collect();
    assert_eq!(parts.len(), 3, "{m}");
    assert_eq!(parts[0], "0,0 1,1");
    assert_eq!(parts[2].split_whitespace().count(), 3, "{m}");
}

#[test]
fn reshape_correspondence_points_on_the_viewer() {
    let (mut s0, l, fx) = with_effect("Reshape");
    let sq = |x0: f64, y0: f64, d: f64| json!([[x0, y0], [x0 + d, y0], [x0 + d, y0 + d], [x0, y0 + d]]);
    s0.execute("mask.new", json!({"layer": l, "vertices": sq(60.0, 40.0, 60.0), "closed": true})).unwrap();
    s0.execute("mask.new", json!({"layer": l, "vertices": sq(180.0, 60.0, 80.0), "closed": true})).unwrap();
    let g = s0.active_comp().unwrap().layer(LayerId(l)).unwrap().props.find_group(fx).unwrap().clone();
    s0.execute("prop.set", json!({"layer": l, "prop": g.get("sourceMask").unwrap().uid, "value": 1})).unwrap();
    s0.execute("prop.set", json!({"layer": l, "prop": g.get("destinationMask").unwrap().uid, "value": 2})).unwrap();
    let mut h = harness(s0);
    let pre = format!("effectControls.effect.{fx}.reshape");
    click_id(&mut h, &format!("{pre}.add"));
    assert_eq!(s(value(&h, l, fx, "correspondencePoints")), "0,0");
    click_id(&mut h, &format!("{pre}.add"));
    assert_eq!(s(value(&h, l, fx, "correspondencePoints")).split_whitespace().count(), 2);
    click_id(&mut h, &format!("{pre}.remove"));
    assert_eq!(s(value(&h, l, fx, "correspondencePoints")), "0,0");
    // With the effect selected, the viewer shows the handles: drag the destination one along
    // its outline.
    h.state_mut().toggle_maximize(PanelKind::EffectControls);
    h.state_mut().session.state.selected_props = vec![(LayerId(l), fx)];
    for _ in 0..6 {
        h.step();
    }
    let d0 = rect(&h, &format!("viewer.reshape.{fx}.dest.0"));
    rect(&h, &format!("viewer.reshape.{fx}.source.0"));
    let undo_len = h.state().session.history.undo.len();
    drag(&mut h, d0.center(), d0.center() + egui::vec2(60.0, 0.0));
    let v = s(value(&h, l, fx, "correspondencePoints"));
    let d: f64 = v.split(',').nth(1).unwrap().parse().unwrap();
    assert!(d > 0.02 && d < 0.3, "moved along the top edge: {v}");
    assert_eq!(h.state().session.history.undo.len(), undo_len + 1);
}

/// #303: with Mesh Warp selected, the viewer shows its grid; dragging a vertex warps the mesh
/// (one undo step), and the other vertices stay put.
#[test]
fn mesh_warp_vertices_drag_on_the_viewer() {
    let (mut s0, l, fx) = with_effect("Mesh Warp");
    let g = s0.active_comp().unwrap().layer(LayerId(l)).unwrap().props.find_group(fx).unwrap().clone();
    s0.execute("prop.set", json!({"layer": l, "prop": g.get("rows").unwrap().uid, "value": 2})).unwrap();
    s0.execute("prop.set", json!({"layer": l, "prop": g.get("columns").unwrap().uid, "value": 2})).unwrap();
    let mut h = harness(s0);
    h.state_mut().toggle_maximize(PanelKind::EffectControls);
    h.state_mut().session.state.selected_props = vec![(LayerId(l), fx)];
    for _ in 0..6 {
        h.step();
    }
    let v = |r: usize, c: usize| format!("viewer.meshWarp.{fx}.{r}.{c}");
    let (tl, mid, br) = (rect(&h, &v(0, 0)).center(), rect(&h, &v(1, 1)).center(), rect(&h, &v(2, 2)).center());
    assert!(((tl.x + br.x) / 2.0 - mid.x).abs() < 1.0 && ((tl.y + br.y) / 2.0 - mid.y).abs() < 1.0, "{tl:?} {mid:?} {br:?}");
    assert!(br.x - tl.x > 100.0, "the grid spans the layer: {tl:?} {br:?}");
    assert_eq!(s(value(&h, l, fx, "mesh")), "");
    let undo_len = h.state().session.history.undo.len();
    drag(&mut h, mid, mid + egui::vec2(40.0, 0.0));
    let mesh = effectcraft_engine::effects::parse_mesh(&s(value(&h, l, fx, "mesh")));
    assert_eq!(mesh.len(), 9, "{mesh:?}");
    assert!(mesh[4].0 > 10.0 && mesh[4].1.abs() < 1.0, "the centre vertex moved right: {mesh:?}");
    assert!(mesh.iter().enumerate().all(|(i, o)| i == 4 || *o == (0.0, 0.0)), "{mesh:?}");
    assert_eq!(h.state().session.history.undo.len(), undo_len + 1);
    let moved = rect(&h, &v(1, 1)).center();
    assert!((moved.x - mid.x - 40.0).abs() < 2.0, "the handle follows the vertex: {mid:?} → {moved:?}");
}

/// UI fixes of M13.10: the Composition / Timeline tabs' close button, swatch and lock; the
/// Preview panel scrolls in the default workspace (where it shows just the transport).
#[test]
fn composition_tabs_and_scrolling_preview() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(4);
    rect(&h, "panel.tab.Timeline.close");
    rect(&h, "preview.scroll");
    click_id(&mut h, "panel.tab.Composition.lock");
    assert!(h.state().ui.locked_tabs.contains("Composition"));
    click_id(&mut h, "panel.tab.Composition.lock");
    assert!(h.state().ui.locked_tabs.is_empty());
    // The Timeline's × closes its comp's tab (the panel stays, as in After Effects).
    let shown = h.state().session.state.active_comp.unwrap();
    click_id(&mut h, "panel.tab.Timeline.close");
    assert!(!h.state().session.state.open_comps.contains(&shown));
    assert!(h.state().ui.dock.is_docked(PanelKind::Timeline));
}
