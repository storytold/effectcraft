//! Picking Cryptomatte IDs with the Selection tool (#2839): as in After Effects, while the
//! Cryptomatte effect is selected in Effect Controls a click on its layer in the Composition
//! viewer picks the ID under the cursor (plain replaces, Shift adds, Alt or Ctrl removes), one
//! undo step each, without arming the eyedropper, and moves or selects nothing. With another
//! effect selected, off the layer, or with another tool, the viewer behaves as before.

use crate::ui_crypto_pick::{chip, crypto_exr, settle, wait_probe};
use effectcraft_engine::Session;
use effectcraft_engine::keyframe::Value as KV;
use effectcraft_engine::project::LayerId;
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::{self, PanelKind, SplitSize};
use effectcraft_ui_egui::panels::viewer::comp_to_screen;
use effectcraft_ui_egui::state::Tool;
use egui::{Event, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;

struct Ids {
    plate: u64,
    side: u64,
    blur: u64,
    crypto: u64,
    sel: u64,
    dir: std::path::PathBuf,
}

/// A 480 × 180 comp: the 320 × 180 Cryptomatte plate in the middle (comp x 80 to 400) with a
/// Gaussian Blur and a Cryptomatte effect, and a 64 × 64 solid beside it (comp x 408 to 472).
/// The plate is selected, and its Cryptomatte effect in Effect Controls.
fn session(tag: &str) -> (Session, Ids) {
    let dir = std::env::temp_dir().join(format!("ec-ui-crysel-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = crypto_exr(&dir, "plate.exr", 320, 180, 1);
    let mut s = effectcraft_host::session();
    let r = s.execute("file.import", json!({"paths": [path]})).unwrap();
    assert_eq!(r["errors"], json!([]), "{r}");
    let footage = r["items"][0].as_u64().unwrap();
    s.execute("comp.new", json!({"name": "Crypto", "width": 480, "height": 180, "frameRate": 24, "duration": 2})).unwrap();
    let plate = s.execute("layer.addItem", json!({"item": footage})).unwrap()["layer"].as_u64().unwrap();
    s.state.selected_layers = vec![LayerId(plate)];
    s.execute_checked("effect.apply", json!({"effect": "Gaussian Blur"})).unwrap();
    s.execute_checked("effect.apply", json!({"effect": "Cryptomatte"})).unwrap();
    let side = s.execute("layer.newSolid", json!({"name": "Side", "width": 64, "height": 64})).unwrap()["layer"].as_u64().unwrap();
    s.execute_checked("prop.set", json!({"layer": side, "path": "transform/position", "value": [440.0, 40.0]})).unwrap();
    let fx = s.active_comp().unwrap().layer(LayerId(plate)).unwrap().effects().unwrap();
    let (blur, crypto) = (fx.groups().next().unwrap(), fx.groups().nth(1).unwrap());
    assert_eq!(crypto.match_id, "ec.3d.cryptomatte");
    let (blur, crypto, sel) = (blur.uid, crypto.uid, crypto.get("selection").unwrap().uid);
    s.state.selected_layers = vec![LayerId(plate)];
    s.state.selected_props = vec![(LayerId(plate), crypto)];
    s.history = Default::default();
    (s, Ids { plate, side, blur, crypto, sel, dir })
}

/// The Composition viewer and Effect Controls side by side, the Timeline below.
fn harness(s: Session) -> Harness<'static, EffectcraftApp> {
    let mut app = EffectcraftApp::new(s);
    app.ui.viewer.show_layer_controls = false;
    app.ui.tool = Tool::Selection;
    app.ui.dock = dock::docked(dock::vsplit(
        SplitSize::FixedSecond(160.0),
        dock::hsplit(SplitSize::FixedSecond(420.0), dock::tabs(&[PanelKind::Composition], 0), dock::tabs(&[PanelKind::EffectControls], 0)),
        dock::tabs(&[PanelKind::Timeline], 0),
    ));
    let mut h = Harness::builder().with_size(vec2(1400.0, 800.0)).with_pixels_per_point(1.0).with_step_dt(0.25).build_eframe(|_| app);
    settle(&mut h);
    h
}

/// The screen point of the plate's pixel (x, y).
fn on_plate(h: &Harness<'_, EffectcraftApp>, x: f64, y: f64) -> Pos2 {
    comp_to_screen(&h.ctx, [(80.0 + x) as f32, y as f32]).unwrap()
}

/// Moves the pointer to `pos`, waits until the Cryptomatte IDs are read, and returns the chip's text.
fn hover(h: &mut Harness<'_, EffectcraftApp>, pos: Pos2) -> Option<String> {
    h.event(Event::PointerMoved(pos));
    // The overlay comes up over the layer in this pass and is hovered from the next one on.
    h.run_steps(2);
    wait_probe(h);
    h.step();
    chip(h)
}

/// Hover (a pass for the viewer to see the pointer), then press and release with `modifiers`.
fn click(h: &mut Harness<'_, EffectcraftApp>, pos: Pos2, modifiers: Modifiers) {
    h.event(Event::PointerMoved(pos));
    h.event(Event::ModifiersChanged(modifiers));
    h.step();
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers });
        h.step();
    }
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.step();
    wait_probe(h);
    h.run_steps(2);
}

/// Presses at `from`, drags by `by` over a few passes and releases.
fn drag(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, by: egui::Vec2) {
    h.event(Event::PointerMoved(from));
    h.step();
    h.event(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    for k in 1..=6 {
        h.event(Event::PointerMoved(from + by * k as f32 / 6.0));
        h.step();
    }
    h.event(Event::PointerButton { pos: from + by, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

/// The plate's Cryptomatte Selection text.
fn selection(h: &Harness<'_, EffectcraftApp>, ids: &Ids) -> String {
    let s = &h.state().session;
    match &s.active_comp().unwrap().layer(LayerId(ids.plate)).unwrap().props.find(ids.sel).unwrap().value {
        KV::Str(t) => t.clone(),
        v => panic!("{v:?}"),
    }
}

/// The layer's Position value, formatted for comparison.
fn position(h: &Harness<'_, EffectcraftApp>, layer: u64) -> String {
    let l = h.state().session.active_comp().unwrap().layer(LayerId(layer)).unwrap().clone();
    format!("{:?}", l.transform().and_then(|t| t.get("position")).map(|p| p.value.clone()))
}

/// How many steps the undo history holds.
fn undo_steps(h: &Harness<'_, EffectcraftApp>) -> usize {
    h.state().session.history.undo.len()
}

/// The ids of the selected layers.
fn selected_layers(h: &Harness<'_, EffectcraftApp>) -> Vec<u64> {
    h.state().session.state.selected_layers.iter().map(|l| l.0).collect()
}

/// Without the change, the viewer took these clicks itself: the Selection stayed empty.
#[test]
fn the_selection_tool_picks_while_the_cryptomatte_effect_is_selected() {
    let (s, ids) = session("picks");
    let mut h = harness(s);
    let (steps, before) = (undo_steps(&h), position(&h, ids.plate));
    let (a, b) = (on_plate(&h, 50.0, 40.0), on_plate(&h, 120.0, 100.0));
    // The viewer names the ID under the cursor, as the armed eyedropper does.
    assert_eq!(hover(&mut h, a).as_deref(), Some("c1_1"));
    assert!(h.state().auto.find("viewer.cryptoSelect").is_some(), "the Selection tool picks over the layer");
    assert_eq!(hover(&mut h, b).as_deref(), Some("c3_3"));
    click(&mut h, a, Modifiers::NONE);
    assert_eq!(selection(&h, &ids), "c1_1", "a click replaces");
    click(&mut h, b, Modifiers::SHIFT);
    assert_eq!(selection(&h, &ids), "c1_1, c3_3", "Shift adds");
    click(&mut h, a, Modifiers::ALT);
    assert_eq!(selection(&h, &ids), "c3_3", "Alt removes");
    click(&mut h, b, Modifiers::COMMAND);
    assert_eq!(selection(&h, &ids), "", "Ctrl removes");
    click(&mut h, b, Modifiers::NONE);
    assert_eq!(selection(&h, &ids), "c3_3");
    click(&mut h, b, Modifiers::CTRL);
    assert_eq!(selection(&h, &ids), "", "Ctrl (not Cmd) removes too");
    click(&mut h, b, Modifiers::NONE);
    assert_eq!(selection(&h, &ids), "c3_3");
    assert_eq!(undo_steps(&h), steps + 7, "one undo step per pick");
    assert!(h.state().ui.fx_pick.is_none(), "nothing armed");
    // Nothing else changed: the layer stays put and stays the only one selected.
    assert_eq!(position(&h, ids.plate), before);
    assert_eq!(selected_layers(&h), vec![ids.plate]);
    // A drag on the layer moves nothing and picks nothing.
    drag(&mut h, a, vec2(120.0, 40.0));
    assert_eq!((position(&h, ids.plate), undo_steps(&h), selection(&h, &ids)), (before, steps + 7, "c3_3".to_string()));
    // Esc has nothing to end: the next click still picks.
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    click(&mut h, a, Modifiers::SHIFT);
    assert_eq!(selection(&h, &ids), "c3_3, c1_1", "added at the end");
    // Undo takes the picks back one at a time.
    h.state_mut().session.execute_checked("edit.undo", json!({})).unwrap();
    assert_eq!(selection(&h, &ids), "c3_3");
    let _ = std::fs::remove_dir_all(&ids.dir);
}

/// With another effect selected, beside the layer, or with another tool, the viewer keeps its
/// clicks: the Selection tool moves and selects layers as before, and nothing is picked.
#[test]
fn the_viewer_keeps_its_clicks_with_another_effect_beside_the_layer_or_another_tool() {
    let (s, ids) = session("keeps");
    let mut h = harness(s);
    let a = on_plate(&h, 50.0, 40.0);
    // Gaussian Blur selected instead: a drag moves the layer.
    h.state_mut().session.state.selected_props = vec![(LayerId(ids.plate), ids.blur)];
    h.run_steps(2);
    let before = position(&h, ids.plate);
    assert_eq!(hover(&mut h, a), None, "no ID named");
    assert!(h.state().auto.find("viewer.cryptoSelect").is_none());
    drag(&mut h, a, vec2(60.0, 0.0));
    assert_ne!(position(&h, ids.plate), before, "the drag moved the layer");
    assert_eq!(selection(&h, &ids), "");
    // The Cryptomatte effect selected again: a click on the other layer beside it selects that one.
    h.state_mut().session.execute_checked("edit.undo", json!({})).unwrap();
    h.state_mut().session.state.selected_layers = vec![LayerId(ids.plate)];
    h.state_mut().session.state.selected_props = vec![(LayerId(ids.plate), ids.crypto)];
    h.run_steps(2);
    let side = comp_to_screen(&h.ctx, [440.0, 40.0]).unwrap();
    let a = on_plate(&h, 50.0, 40.0);
    assert_eq!(hover(&mut h, a).as_deref(), Some("c1_1"));
    click(&mut h, side, Modifiers::NONE);
    assert_eq!(selected_layers(&h), vec![ids.side], "the click beside the layer selected the other layer");
    assert_eq!(selection(&h, &ids), "");
    // The Hand tool with the effect selected: no pick.
    h.state_mut().session.state.selected_layers = vec![LayerId(ids.plate)];
    h.state_mut().session.state.selected_props = vec![(LayerId(ids.plate), ids.crypto)];
    h.state_mut().ui.tool = Tool::Hand;
    h.run_steps(2);
    let a = on_plate(&h, 50.0, 40.0);
    click(&mut h, a, Modifiers::NONE);
    assert_eq!(selection(&h, &ids), "", "the Hand tool picks nothing");
    // Back to the Selection tool: it picks.
    h.state_mut().ui.tool = Tool::Selection;
    h.run_steps(2);
    let a = on_plate(&h, 50.0, 40.0);
    assert_eq!(hover(&mut h, a).as_deref(), Some("c1_1"));
    click(&mut h, a, Modifiers::NONE);
    assert_eq!(selection(&h, &ids), "c1_1");
    let _ = std::fs::remove_dir_all(&ids.dir);
}

/// A press in the very pass the pointer leaves the layer for another one (the overlay sensed it
/// the pass before, so egui hands it the press): passed on to the viewer, which selects that layer.
#[test]
fn a_press_as_the_pointer_leaves_the_layer_still_selects_the_other_layer() {
    let (s, ids) = session("leave");
    let mut h = harness(s);
    let a = on_plate(&h, 50.0, 40.0);
    assert_eq!(hover(&mut h, a).as_deref(), Some("c1_1"));
    let side = comp_to_screen(&h.ctx, [440.0, 40.0]).unwrap();
    let ev = &mut h.input_mut().events;
    ev.push(Event::PointerMoved(side));
    ev.push(Event::PointerButton { pos: side, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.event(Event::PointerButton { pos: side, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
    assert_eq!(selected_layers(&h), vec![ids.side], "the press wasn't swallowed");
    assert_eq!(selection(&h, &ids), "");
    let _ = std::fs::remove_dir_all(&ids.dir);
}

/// The viewer's own handles on the layer keep working with the effect selected: a drag on a
/// corner of the bounding box scales the layer and picks nothing.
#[test]
fn a_corner_handle_still_scales_with_the_effect_selected() {
    let (s, ids) = session("handle");
    let mut h = harness(s);
    h.state_mut().ui.viewer.show_layer_controls = true;
    h.run_steps(2);
    let a = on_plate(&h, 50.0, 40.0);
    assert_eq!(hover(&mut h, a).as_deref(), Some("c1_1"), "inside the layer it picks");
    let scale = |h: &Harness<'_, EffectcraftApp>| {
        let l = h.state().session.active_comp().unwrap().layer(LayerId(ids.plate)).unwrap().clone();
        format!("{:?}", l.transform().and_then(|t| t.get("scale")).map(|p| p.value.clone()))
    };
    let before = scale(&h);
    // The bottom-right corner of the plate (comp 400, 180).
    let corner = comp_to_screen(&h.ctx, [400.0, 180.0]).unwrap();
    drag(&mut h, corner, vec2(-40.0, -20.0));
    assert_ne!(scale(&h), before, "the handle scaled the layer");
    assert_eq!(selection(&h, &ids), "");
    let _ = std::fs::remove_dir_all(&ids.dir);
}

/// From the effect selected to the eyedropper and back: its button selects the Selection
/// property (the effect itself no longer), so after Esc a click on the layer picks nothing and
/// no Cryptomatte IDs are kept read.
#[test]
fn after_the_eyedropper_and_esc_the_selection_tool_does_not_pick() {
    let (s, ids) = session("handoff");
    let mut h = harness(s);
    let a = on_plate(&h, 50.0, 40.0);
    assert_eq!(hover(&mut h, a).as_deref(), Some("c1_1"), "the effect selected: it picks");
    let eye = crate::ui_crypto_pick::at(&h, &format!("effectControls.prop.{}.eyedropper", ids.sel));
    click(&mut h, eye, Modifiers::NONE);
    assert!(h.state().ui.fx_pick.as_ref().is_some_and(|p| p.kind == "crypto" && p.prop == ids.sel), "armed");
    assert_eq!(h.state().session.state.selected_props, vec![(LayerId(ids.plate), ids.sel)], "the row is selected");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(h.state().ui.fx_pick.is_none(), "Esc ends the eyedropper");
    click(&mut h, a, Modifiers::NONE);
    h.run_steps(2);
    assert_eq!(selection(&h, &ids), "", "no pick");
    assert!(h.state().auto.find("viewer.cryptoSelect").is_none());
    assert!(!h.state().session.crypto_probe_stats().0, "no IDs kept read");
    let _ = std::fs::remove_dir_all(&ids.dir);
}

/// A touch (or a pen without hover) arrives on the layer and presses in one pass: the overlay
/// wasn't there the pass before, so egui hands the press to the viewer. It is still a pick (and
/// Shift still adds), and the viewer neither selects, toggles nor moves the layer with it.
#[test]
fn a_touch_that_arrives_and_presses_in_one_pass_picks() {
    let (s, ids) = session("touch");
    let mut h = harness(s);
    let (steps, before) = (undo_steps(&h), position(&h, ids.plate));
    let (a, b) = (on_plate(&h, 50.0, 40.0), on_plate(&h, 120.0, 100.0));
    let touch = |h: &mut Harness<'_, EffectcraftApp>, pos: Pos2, modifiers: Modifiers, by: egui::Vec2| {
        h.event(Event::PointerGone);
        h.run_steps(2);
        let ev = &mut h.input_mut().events;
        ev.push(Event::ModifiersChanged(modifiers));
        ev.push(Event::PointerMoved(pos));
        ev.push(Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers });
        h.step();
        for k in 1..=4 {
            h.event(Event::PointerMoved(pos + by * k as f32 / 4.0));
            h.step();
        }
        h.event(Event::PointerButton { pos: pos + by, button: PointerButton::Primary, pressed: false, modifiers });
        h.step();
        h.event(Event::ModifiersChanged(Modifiers::NONE));
        h.event(Event::PointerGone);
        h.step();
        wait_probe(h);
        h.run_steps(2);
    };
    touch(&mut h, a, Modifiers::NONE, vec2(0.0, 0.0));
    assert_eq!(selection(&h, &ids), "c1_1", "the touch picked");
    touch(&mut h, b, Modifiers::SHIFT, vec2(0.0, 0.0));
    assert_eq!(selection(&h, &ids), "c1_1, c3_3", "Shift added");
    assert_eq!(selected_layers(&h), vec![ids.plate], "the viewer didn't toggle the layer");
    // A touch that drags moves nothing (and picks nothing).
    touch(&mut h, a, Modifiers::NONE, vec2(120.0, 40.0));
    assert_eq!((position(&h, ids.plate), undo_steps(&h)), (before, steps + 2), "the layer stays put");
    assert_eq!(selection(&h, &ids), "c1_1, c3_3");
    let _ = std::fs::remove_dir_all(&ids.dir);
}
