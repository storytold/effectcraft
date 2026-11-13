//! Headless Effect Controls checks (egui_kittest): the AE-style widgets register their
//! automation ids, the on-viewer effect point control appears for the selected effect, and the
//! crosshair / eyedropper picks set parameters from a viewer click.

use effectcraft_engine::Session;
use effectcraft_engine::keyframe::Value as KV;
use effectcraft_engine::project::{Layer, LayerId};
use effectcraft_ui_egui::EffectcraftApp;
use effectcraft_ui_egui::dock::PanelKind;
use effectcraft_ui_egui::state::FxPick;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

struct Ids {
    small: u64,
    point: u64,
    color: u64,
    angle: u64,
    layer_ctl: u64,
    curves: u64,
    levels: u64,
    point_fx: u64,
}

fn layer(s: &Session, id: u64) -> Layer {
    s.active_comp().unwrap().layer(LayerId(id)).unwrap().clone()
}

/// A 640×360 comp: a full-frame green solid under a 200×100 solid scaled 200% at the centre,
/// carrying controls, Curves and Levels (Individual Controls).
fn app() -> (EffectcraftApp, Ids) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 640, "height": 360, "frameRate": 30, "duration": 10})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Back", "color": "#20c040"})).unwrap();
    let small = s.execute("layer.newSolid", json!({"name": "Small", "color": "#ff0000", "width": 200, "height": 100})).unwrap()["layer"].as_u64().unwrap();
    s.execute("prop.set", json!({"layer": small, "path": "transform/scale", "value": [200.0, 200.0]})).unwrap();
    let mut fx = |name: &str| s.execute("effect.apply", json!({"layers": [small], "effect": name})).unwrap()["effects"][0].as_u64().unwrap();
    let point_fx = fx("Point Control");
    fx("Color Control");
    fx("Angle Control");
    fx("Layer Control");
    let curves = fx("Curves");
    let levels = fx("Levels (Individual Controls)");
    let l = layer(&s, small);
    let fxg = l.effects().unwrap();
    let param = |effect: u64, id: &str| fxg.groups().find(|g| g.uid == effect).and_then(|g| g.get(id)).map(|p| p.uid).unwrap();
    let find = |m: &str, id: &str| fxg.groups().find(|g| g.match_id == m).and_then(|g| g.get(id)).map(|p| p.uid).unwrap();
    let ids = Ids {
        small,
        point: param(point_fx, "point"),
        color: find("ec.control.color", "color"),
        angle: find("ec.control.angle", "angle"),
        layer_ctl: find("ec.control.layer", "layer"),
        curves,
        levels,
        point_fx,
    };
    s.state.selected_layers = vec![LayerId(small)];
    s.state.selected_props.clear();
    let mut app = EffectcraftApp::new(s);
    app.show_panel(PanelKind::EffectControls);
    (app, ids)
}

fn settle(h: &mut Harness<'_, EffectcraftApp>) {
    for _ in 0..600 {
        h.step();
        if h.state().frames.inflight() == 0 && h.state().frames.last_ms.lock().map(|v| *v > 0.0).unwrap_or(false) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    for _ in 0..4 {
        h.step();
    }
}

fn ids(h: &Harness<'_, EffectcraftApp>) -> Vec<String> {
    h.state().auto.previous.iter().chain(h.state().auto.elements.iter()).map(|e| e.id.clone()).collect()
}

fn click(h: &mut Harness<'_, EffectcraftApp>, pos: egui::Pos2) {
    h.event(egui::Event::PointerMoved(pos));
    h.step();
    h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.step();
    h.step();
}

fn header_arrow_harness() -> (Harness<'static, EffectcraftApp>, LayerId, [(u64, u64); 3]) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Disclosure", "width": 32, "height": 18, "frameRate": 30, "duration": 2})).unwrap();
    let lid = s.execute("layer.newSolid", json!({"name": "Controls", "color": "#40b080"})).unwrap()["layer"].as_u64().unwrap();
    let mut effects = [(0, 0); 3];
    for pair in &mut effects {
        let fx = s.execute("effect.apply", json!({"layers": [lid], "effect": "Point Control"})).unwrap()["effects"][0].as_u64().unwrap();
        let prop = layer(&s, lid).props.find_group(fx).unwrap().get("point").unwrap().uid;
        *pair = (fx, prop);
    }
    s.state.selected_props.clear();
    let mut app = EffectcraftApp::new(s);
    app.show_panel(PanelKind::EffectControls);
    app.toggle_maximize(PanelKind::EffectControls);
    let mut h = Harness::builder().with_size(egui::vec2(1000.0, 700.0)).with_step_dt(1.0 / 60.0).build_eframe(|_| app);
    h.run_steps(4);
    (h, LayerId(lid), effects)
}

fn header_arrow_key(h: &mut Harness<'_, EffectcraftApp>, key: egui::Key, modifiers: egui::Modifiers) {
    h.key_press_modifiers(modifiers, key);
    h.run_steps(2);
}

#[test]
fn effect_header_arrows_collapse_and_expand_without_editing_the_document() {
    let (mut h, lid, [(fx, prop), (_, other), _]) = header_arrow_harness();
    let at = rect_of(&h, &format!("effectControls.effect.{fx}.name")).center();
    click(&mut h, at);
    assert_eq!(h.state().ui.focused, PanelKind::EffectControls);
    assert_eq!(h.state().session.state.selected_props, vec![(lid, fx)]);
    let project = serde_json::to_value(&h.state().session.project).unwrap();
    let time = h.state().session.time();
    let undo = h.state().session.history.undo.len();
    let redo = h.state().session.history.redo.len();
    for key in [egui::Key::ArrowLeft, egui::Key::ArrowLeft, egui::Key::ArrowRight, egui::Key::ArrowRight] {
        header_arrow_key(&mut h, key, egui::Modifiers::NONE);
        assert_eq!(h.state().auto.find(&format!("effectControls.row.{prop}")).is_some(), key == egui::Key::ArrowRight);
        assert!(h.state().auto.find(&format!("effectControls.row.{other}")).is_some(), "unselected effect stays open");
        assert_eq!(h.state().session.state.selected_props, vec![(lid, fx)]);
        assert_eq!(serde_json::to_value(&h.state().session.project).unwrap(), project);
        assert_eq!(h.state().session.time(), time);
        assert_eq!((h.state().session.history.undo.len(), h.state().session.history.redo.len()), (undo, redo));
    }
}

#[test]
fn effect_header_arrows_only_disclose_selected_headers_on_the_displayed_layer() {
    let (mut h, lid, [(first, _), (second, _), (third, prop)]) = header_arrow_harness();
    h.state_mut().session.state.selected_props = vec![(lid, first), (lid, second), (LayerId(0), third), (lid, u64::MAX)];
    header_arrow_key(&mut h, egui::Key::ArrowLeft, egui::Modifiers::NONE);
    assert_eq!(h.state().ui.fx_closed.iter().copied().collect::<Vec<_>>(), vec![first, second]);
    assert!(h.state().auto.find(&format!("effectControls.row.{prop}")).is_some());
    header_arrow_key(&mut h, egui::Key::ArrowRight, egui::Modifiers::NONE);
    assert!(h.state().ui.fx_closed.is_empty());

    // A parameter selection, stale header, or header on another layer must still fall through.
    h.state_mut().session.state.selected_props = vec![(lid, prop), (LayerId(0), first), (lid, u64::MAX)];
    let before = serde_json::to_value(&h.state().session.project).unwrap();
    header_arrow_key(&mut h, egui::Key::ArrowLeft, egui::Modifiers::NONE);
    assert!(h.state().ui.fx_closed.is_empty());
    assert_ne!(serde_json::to_value(&h.state().session.project).unwrap(), before, "the existing layer nudge still runs");
}

#[test]
fn effect_header_arrows_ignore_hidden_panels_with_stale_focus() {
    // A maximized frame hides every other frame and every floating window, so a floating Effect
    // Controls is hidden too.
    for hidden in ["closed", "maximized", "floating under maximized", "home"] {
        let (mut h, lid, [(fx, _), _, _]) = header_arrow_harness();
        h.state_mut().session.state.selected_props = vec![(lid, fx)];
        match hidden {
            "closed" => h.state_mut().close_panel(PanelKind::EffectControls),
            "maximized" => h.state_mut().toggle_maximize(PanelKind::Composition),
            "floating under maximized" => {
                h.state_mut().apply_dock(craft_ui::docking::Action::Float { panel: PanelKind::EffectControls, rect: [40.0, 40.0, 600.0, 400.0] }).unwrap();
                h.state_mut().toggle_maximize(PanelKind::Composition);
            }
            _ => h.state_mut().ui.start_screen = true,
        }
        h.run_steps(2);
        assert_eq!(h.state().ui.focused, PanelKind::EffectControls, "{hidden}: stale focus is preserved");
        assert!(h.state().auto.find("panel.EffectControls").is_none(), "{hidden}: Effect Controls is hidden");
        let x = |h: &Harness<'_, EffectcraftApp>| layer(&h.state().session, lid.0).props.prop("transform/position").unwrap().value.as_vec3()[0];
        let before = x(&h);
        header_arrow_key(&mut h, egui::Key::ArrowLeft, egui::Modifiers::NONE);
        assert!(h.state().ui.fx_closed.is_empty(), "{hidden}: hidden effects keep their disclosure state");
        assert!(x(&h) < before, "{hidden}: Left still nudges the layer");
    }
}

#[test]
fn effect_header_arrows_work_wherever_effect_controls_is_on_screen() {
    for place in ["floating", "tab of a maximized frame"] {
        let (mut h, lid, [(fx, prop), _, _]) = header_arrow_harness();
        h.state_mut().toggle_maximize(PanelKind::EffectControls);
        if place == "floating" {
            h.state_mut().apply_dock(craft_ui::docking::Action::Float { panel: PanelKind::EffectControls, rect: [40.0, 40.0, 600.0, 400.0] }).unwrap();
        } else {
            let placement = craft_ui::docking::Placement::Tab { before: None };
            h.state_mut().apply_dock(craft_ui::docking::Action::Move { panel: PanelKind::EffectControls, anchor: PanelKind::Composition, placement }).unwrap();
            h.state_mut().toggle_maximize(PanelKind::Composition);
            assert_eq!(h.state().ui.maximized, Some(PanelKind::Composition), "the frame was maximized from its Composition tab");
        }
        // The moved tab is the front one; focus it as a click on it would.
        h.state_mut().ui.focused = PanelKind::EffectControls;
        h.state_mut().session.state.selected_props = vec![(lid, fx)];
        h.run_steps(3);
        assert!(h.state().auto.find(&format!("effectControls.row.{prop}")).is_some(), "{place}: Effect Controls is on screen");
        header_arrow_key(&mut h, egui::Key::ArrowLeft, egui::Modifiers::NONE);
        assert!(h.state().auto.find(&format!("effectControls.row.{prop}")).is_none(), "{place}: Left collapses");
        header_arrow_key(&mut h, egui::Key::ArrowRight, egui::Modifiers::NONE);
        assert!(h.state().auto.find(&format!("effectControls.row.{prop}")).is_some(), "{place}: Right expands");
    }
}

#[test]
fn effect_header_arrows_preserve_modified_shortcuts_and_keyboard_ownership() {
    let (mut h, lid, [(fx, _), _, _]) = header_arrow_harness();
    h.state_mut().session.state.selected_props = vec![(lid, fx)];
    let time = h.state().session.time();
    header_arrow_key(&mut h, egui::Key::ArrowRight, egui::Modifiers::COMMAND);
    assert!(h.state().session.time() > time, "Cmd/Ctrl+Right still advances a frame");
    header_arrow_key(&mut h, egui::Key::ArrowLeft, egui::Modifiers::COMMAND);
    assert_eq!(h.state().session.time(), time);
    header_arrow_key(&mut h, egui::Key::ArrowLeft, egui::Modifiers::SHIFT);
    assert!(h.state().ui.fx_closed.is_empty(), "Shift+Left does not collapse effects");
    h.state_mut().ui.focused = PanelKind::Composition;
    header_arrow_key(&mut h, egui::Key::ArrowLeft, egui::Modifiers::NONE);
    assert!(h.state().ui.fx_closed.is_empty(), "another panel owns the arrows");
    h.state_mut().ui.focused = PanelKind::EffectControls;
    h.state_mut().dialog = Some(effectcraft_ui_egui::Dialog::About);
    header_arrow_key(&mut h, egui::Key::ArrowLeft, egui::Modifiers::NONE);
    assert!(h.state().ui.fx_closed.is_empty(), "a dialog owns the arrows");
    h.state_mut().dialog = None;
    h.run_steps(3);
    let at = rect_of(&h, &format!("effectControls.effect.{fx}.name")).center();
    h.event(egui::Event::PointerMoved(at));
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed, modifiers: egui::Modifiers::NONE });
        h.step();
    }
    h.run_steps(2);
    assert!(egui::Popup::is_any_open(&h.ctx), "the effect context menu is open");
    header_arrow_key(&mut h, egui::Key::ArrowLeft, egui::Modifiers::NONE);
    assert!(h.state().ui.fx_closed.is_empty(), "the context menu owns the arrows");
    header_arrow_key(&mut h, egui::Key::Escape, egui::Modifiers::NONE);
    h.run_steps(3);
    assert!(!egui::Popup::is_any_open(&h.ctx), "Escape closed the context menu");
    double_click(&mut h, at);
    h.run_steps(3);
    assert!(h.state().auto.find(&format!("effectControls.effect.{fx}.nameEdit")).is_some(), "the rename field is open");
    header_arrow_key(&mut h, egui::Key::ArrowLeft, egui::Modifiers::NONE);
    assert!(h.state().ui.fx_closed.is_empty(), "Left moves the text caret instead of collapsing");
    assert!(h.state().auto.find(&format!("effectControls.effect.{fx}.nameEdit")).is_some());
}

#[test]
fn effect_header_arrows_leave_viewer_text_editing_alone() {
    let (mut h, lid, [(fx, _), _, _]) = header_arrow_harness();
    let text = h.state_mut().session.execute("layer.newText", json!({"text": "abcdef"})).unwrap()["layer"].as_u64().unwrap();
    h.state_mut().session.execute("text.edit", json!({"layer": text, "caret": 3})).unwrap();
    h.state_mut().session.state.selected_layers = vec![lid];
    h.state_mut().session.state.selected_props = vec![(lid, fx)];
    h.state_mut().show_panel(PanelKind::EffectControls);
    h.run_steps(2);
    assert!(h.state().session.state.text_edit.is_some(), "the text is still being edited");
    header_arrow_key(&mut h, egui::Key::ArrowLeft, egui::Modifiers::NONE);
    assert!(h.state().ui.fx_closed.is_empty(), "the text edit owns the arrows");
    h.state_mut().session.state.text_edit = None;
    header_arrow_key(&mut h, egui::Key::ArrowLeft, egui::Modifiers::NONE);
    assert_eq!(h.state().ui.fx_closed.iter().copied().collect::<Vec<_>>(), vec![fx], "with the edit over, Left collapses the effect");
}

#[test]
fn effect_controls_widgets_register_automation_ids() {
    let (app, x) = app();
    let mut h = Harness::builder().with_size(egui::vec2(1700.0, 1100.0)).build_eframe(|_| app);
    settle(&mut h);
    let have = ids(&h);
    let want = [
        format!("effectControls.prop.{}.crosshair", x.point),
        format!("effectControls.prop.{}.eyedropper", x.color),
        format!("effectControls.prop.{}.twirl", x.angle),
        format!("effectControls.prop.{}.source", x.layer_ctl),
        format!("effectControls.effect.{}.curves.graph", x.curves),
        format!("effectControls.effect.{}.curves.channel", x.curves),
        format!("effectControls.effect.{}.curves.reset", x.curves),
        format!("effectControls.effect.{}.reset", x.point_fx),
    ];
    let missing: Vec<&String> = want.iter().filter(|w| !have.contains(w)).collect();
    assert!(missing.is_empty(), "missing {missing:?}");
    // Levels sits below: collapse Curves to bring it into view.
    h.state_mut().ui.fx_closed.insert(x.curves);
    h.step();
    h.step();
    let have = ids(&h);
    let want =
        ["histogram", "channel", "inBlack", "gamma", "inWhite", "outBlack", "outWhite"].map(|k| format!("effectControls.effect.{}.levels.{k}", x.levels));
    let missing: Vec<&String> = want.iter().filter(|w| !have.contains(w)).collect();
    assert!(missing.is_empty(), "missing {missing:?}");
    // The angle reads AE-style.
    let angle =
        h.state().auto.previous.iter().chain(h.state().auto.elements.iter()).find(|e| e.id == format!("effectControls.prop.{}.value", x.angle)).cloned();
    assert_eq!(angle.map(|e| e.label), Some("0x+0.0°".to_string()));
    // Twirling the angle open shows its dial.
    h.state_mut().ui.fx_slider_open.insert(x.angle);
    h.step();
    h.step();
    assert!(ids(&h).contains(&format!("effectControls.prop.{}.dial", x.angle)));
    // No effect selected: no on-viewer point control. Selecting the effect shows it.
    let pt_id = format!("viewer.effectPoint.{}", x.point);
    assert!(!ids(&h).contains(&pt_id));
    h.state_mut().session.state.selected_props = vec![(LayerId(x.small), x.point_fx)];
    h.step();
    h.step();
    assert!(ids(&h).contains(&pt_id), "effect point control shown for the selected effect");
}

#[test]
fn tint_swap_button_targets_its_instance_and_undoes_both_colors() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"width": 640, "height": 360, "frameRate": 30, "duration": 5})).unwrap();
    let lid = s.execute("layer.newSolid", json!({"color": "#000000"})).unwrap()["layer"].as_u64().unwrap();
    let first = s.execute("effect.apply", json!({"effect": "Tint"})).unwrap()["effects"][0].as_u64().unwrap();
    let target = s.execute("effect.apply", json!({"effect": "Tint"})).unwrap()["effects"][0].as_u64().unwrap();
    let blur = s.execute("effect.apply", json!({"effect": "Gaussian Blur"})).unwrap()["effects"][0].as_u64().unwrap();
    let mut app = EffectcraftApp::new(s);
    app.show_panel(PanelKind::EffectControls);
    let mut h = Harness::builder().with_size(egui::vec2(1700.0, 1100.0)).build_eframe(|_| app);
    h.run_steps(4);
    let id = format!("effectControls.effect.{target}.swapColors");
    let rect = h.state().auto.find(&id).unwrap().rect;
    assert!(ids(&h).contains(&format!("effectControls.effect.{first}.swapColors")));
    assert!(!ids(&h).contains(&format!("effectControls.effect.{blur}.swapColors")));
    let n = h.state().session.history.undo.len();
    click(&mut h, egui::pos2(rect[0] + rect[2] / 2.0, rect[1] + rect[3] / 2.0));
    let l = layer(&h.state().session, lid);
    let g = l.props.find_group(target).unwrap();
    assert_eq!(g.prop("black").unwrap().value, KV::Color([1.0; 4]));
    assert_eq!(g.prop("white").unwrap().value, KV::Color([0.0, 0.0, 0.0, 1.0]));
    assert_eq!(l.props.find_group(first).unwrap().prop("black").unwrap().value, KV::Color([0.0, 0.0, 0.0, 1.0]));
    assert_eq!(h.state().session.history.undo.len(), n + 1);
    assert_eq!(h.state().session.state.selected_props, vec![(LayerId(lid), target)]);
    assert!(h.state_mut().session.undo());
    let l = layer(&h.state().session, lid);
    assert_eq!(l.props.find_group(target).unwrap().prop("black").unwrap().value, KV::Color([0.0, 0.0, 0.0, 1.0]));
    assert_eq!(l.props.find_group(target).unwrap().prop("white").unwrap().value, KV::Color([1.0; 4]));
    h.state_mut().ui.fx_closed.insert(target);
    h.run_steps(2);
    assert!(!h.state().auto.elements.iter().any(|e| e.id == id));
}

#[test]
fn crosshair_and_eyedropper_pick_from_the_viewer() {
    let (app, x) = app();
    let mut h = Harness::builder().with_size(egui::vec2(1700.0, 1100.0)).build_eframe(|_| app);
    settle(&mut h);
    // Crosshair: a click at comp (330, 190) is layer (105, 55) on the 200%-scaled layer
    // (anchor (100, 50) at position (320, 180)).
    h.state_mut().ui.fx_pick = Some(FxPick { kind: "point".into(), layer: x.small, prop: x.point, name: "Point".into() });
    h.step();
    assert!(ids(&h).contains(&"viewer.fxPick".to_string()));
    let pos = effectcraft_ui_egui::panels::viewer::comp_to_screen(&h.ctx, [330.0, 190.0]).unwrap();
    click(&mut h, pos);
    assert!(h.state().ui.fx_pick.is_none(), "the pick is used up");
    let v = layer(&h.state().session, x.small).effects().unwrap().find(x.point).unwrap().value.clone();
    let KV::Vec2(p) = v else { panic!("{v:?}") };
    // One viewer point is up to 1/zoom comp pixels.
    let tol = 1.0 / effectcraft_ui_egui::panels::viewer::last_fit(&h.ctx) as f64;
    assert!((p[0] - 105.0).abs() <= tol && (p[1] - 55.0).abs() <= tol, "{p:?}");
    // Eyedropper over the green background.
    h.state_mut().ui.fx_pick = Some(FxPick { kind: "color".into(), layer: x.small, prop: x.color, name: "Color".into() });
    h.step();
    let pos = effectcraft_ui_egui::panels::viewer::comp_to_screen(&h.ctx, [24.0, 24.0]).unwrap();
    click(&mut h, pos);
    let v = layer(&h.state().session, x.small).effects().unwrap().find(x.color).unwrap().value.clone();
    let KV::Color(c) = v else { panic!("{v:?}") };
    let want = [0x20 as f64 / 255.0, 0xc0 as f64 / 255.0, 0x40 as f64 / 255.0];
    for k in 0..3 {
        assert!((c[k] - want[k]).abs() < 0.02, "{c:?} vs {want:?}");
    }
    assert_eq!(c[3], 1.0);
}

/// #284: with a Region of Interest the shown frame covers only that region; the eyedropper
/// samples the composite under the pointer there (it read the frame as if it covered the whole
/// comp, so it sampled somewhere else).
#[test]
fn the_eyedropper_samples_under_the_pointer_in_a_region_of_interest() {
    let (mut app, x) = app();
    app.session.execute("view.setRegionOfInterest", json!({"rect": [0.0, 0.0, 320.0, 180.0]})).unwrap();
    // (Not the red it will pick.)
    app.session.execute("prop.set", json!({"layer": x.small, "prop": x.color, "value": [0.0, 0.0, 1.0, 1.0]})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1700.0, 1100.0)).build_eframe(|_| app);
    settle(&mut h);
    h.state_mut().ui.fx_pick = Some(FxPick { kind: "color".into(), layer: x.small, prop: x.color, name: "Color".into() });
    h.step();
    // (200, 150) is on the red layer (it covers 120..520 × 80..280); read as a frame of the
    // whole comp, the green background at (100, 75).
    let pos = effectcraft_ui_egui::panels::viewer::comp_to_screen(&h.ctx, [200.0, 150.0]).unwrap();
    click(&mut h, pos);
    let v = layer(&h.state().session, x.small).effects().unwrap().find(x.color).unwrap().value.clone();
    let KV::Color(c) = v else { panic!("{v:?}") };
    assert!(c[0] > 0.98 && c[1] < 0.02 && c[2] < 0.02, "{c:?}");
}

/// A click before the viewer has a frame to sample (it is still rendering) leaves the eyedropper
/// armed: it used the pick up without setting a colour.
#[test]
fn the_eyedropper_stays_armed_until_the_viewer_has_a_frame() {
    // A large blurred frame, so the first render outlasts the click.
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Slow", "width": 7680, "height": 4320, "frameRate": 30, "duration": 1})).unwrap();
    let back = s.execute("layer.newSolid", json!({"name": "Back", "color": "#20c040"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("effect.apply", json!({"layers": [back], "effect": "Gaussian Blur"})).unwrap();
    let top = s.execute("layer.newSolid", json!({"name": "Top", "color": "#ff0000", "width": 200, "height": 100})).unwrap()["layer"].as_u64().unwrap();
    s.execute("effect.apply", json!({"layers": [top], "effect": "Color Control"})).unwrap();
    let prop = layer(&s, top).effects().unwrap().groups().find(|g| g.match_id == "ec.control.color").and_then(|g| g.get("color")).map(|p| p.uid).unwrap();
    s.execute("prop.set", json!({"layer": top, "prop": prop, "value": [0.0, 0.0, 1.0, 1.0]})).unwrap();
    let colour = |h: &Harness<'_, EffectcraftApp>| {
        let v = layer(&h.state().session, top).effects().unwrap().find(prop).unwrap().value.clone();
        let KV::Color(c) = v else { panic!("{v:?}") };
        c
    };
    let mut h = Harness::builder().with_size(egui::vec2(1700.0, 1100.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.state_mut().ui.fx_pick = Some(FxPick { kind: "color".into(), layer: top, prop, name: "Color".into() });
    h.step();
    let pos = effectcraft_ui_egui::panels::viewer::comp_to_screen(&h.ctx, [100.0, 100.0]).unwrap();
    click(&mut h, pos);
    // (A frame that made it in time was sampled: the colour below is checked either way.)
    if h.state().ui.fx_pick.is_some() {
        assert_eq!(colour(&h), [0.0, 0.0, 1.0, 1.0], "nothing sampled yet");
        settle(&mut h);
        click(&mut h, pos);
        assert!(h.state().ui.fx_pick.is_none(), "the pick is used up");
    }
    let c = colour(&h);
    let want = [0x20 as f64 / 255.0, 0xc0 as f64 / 255.0, 0x40 as f64 / 255.0];
    for k in 0..3 {
        assert!((c[k] - want[k]).abs() < 0.02, "{c:?} vs {want:?}");
    }
}

/// Screen Color Key's Screen Colour eyedropper picks from the effect's input: the shown frame is
/// already keyed (the default screen colour takes most of a green), so sampling it would miss.
#[test]
fn keyer_eyedropper_picks_the_screen_from_the_effect_input() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Key", "width": 320, "height": 180, "frameRate": 30, "duration": 1})).unwrap();
    let screen = s.execute("layer.newSolid", json!({"name": "Screen", "color": "#30b050"})).unwrap()["layer"].as_u64().unwrap();
    let fx = s.execute("effect.apply", json!({"layers": [screen], "effect": "Screen Color Key"})).unwrap()["effects"][0].as_u64().unwrap();
    let prop = layer(&s, screen).effects().unwrap().groups().find(|g| g.uid == fx).and_then(|g| g.get("screenColour")).map(|p| p.uid).unwrap();
    s.state.selected_layers = vec![LayerId(screen)];
    let mut app = EffectcraftApp::new(s);
    app.show_panel(PanelKind::EffectControls);
    let mut h = Harness::builder().with_size(egui::vec2(1700.0, 1100.0)).build_eframe(|_| app);
    settle(&mut h);
    h.state_mut().ui.fx_pick = Some(FxPick { kind: "color".into(), layer: screen, prop, name: "Screen Colour".into() });
    h.step();
    let pos = effectcraft_ui_egui::panels::viewer::comp_to_screen(&h.ctx, [160.0, 90.0]).unwrap();
    click(&mut h, pos);
    assert!(h.state().ui.fx_pick.is_none(), "the pick is used up");
    let v = layer(&h.state().session, screen).effects().unwrap().find(prop).unwrap().value.clone();
    let KV::Color(c) = v else { panic!("{v:?}") };
    let want = [0x30 as f64 / 255.0, 0xb0 as f64 / 255.0, 0x50 as f64 / 255.0];
    for k in 0..3 {
        assert!((c[k] - want[k]).abs() < 0.01, "{c:?} vs {want:?}");
    }
    // The picked screen keys the whole solid out.
    let s = &h.state().session;
    let img = s.render(s.active_comp_id().unwrap(), s.time(), Default::default());
    assert!(img.get(160, 90)[3] < 0.01, "{:?}", img.get(160, 90));
}

#[test]
fn command_clicking_an_effect_twirl_closes_and_opens_its_groups() {
    let (app, x) = app();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| app);
    let fx = h.state_mut().session.execute("effect.apply", json!({"layers": [x.small], "effect": "Color Grading"})).unwrap()["effects"][0].as_u64().unwrap();
    // Only this effect open, so its header is on screen.
    let l = layer(&h.state().session, x.small);
    let others: Vec<u64> = l.effects().unwrap().groups().map(|g| g.uid).filter(|u| *u != fx).collect();
    h.state_mut().ui.fx_closed.extend(others.iter().copied());
    h.run_steps(3);
    let inner: Vec<u64> = l.effects().unwrap().groups().find(|g| g.uid == fx).unwrap().groups().map(|g| g.uid).collect();
    assert!(!inner.is_empty(), "the effect has twirl-down groups");
    let cmd_click = |h: &mut Harness<'_, EffectcraftApp>| {
        let at = rect_of(h, &format!("effectControls.effect.{fx}.twirl")).center();
        h.event(egui::Event::ModifiersChanged(egui::Modifiers::COMMAND));
        click(h, at);
        h.event(egui::Event::ModifiersChanged(Default::default()));
        h.step();
    };
    cmd_click(&mut h);
    let closed = h.state().ui.fx_closed.clone();
    assert!(closed.contains(&fx) && inner.iter().all(|u| closed.contains(u)), "the effect and its groups twirl closed");
    cmd_click(&mut h);
    let closed = h.state().ui.fx_closed.clone();
    assert!(!closed.contains(&fx) && !inner.iter().any(|u| closed.contains(u)), "and open again together");
    assert!(others.iter().all(|u| closed.contains(u)), "other effects stay as they were");
}

/// Shift-clicking effect headers selects the range from the last effect picked, which stays the
/// end the next range starts from; Cmd/Ctrl adds or takes out one effect and moves that end.
#[test]
fn shift_click_on_effect_headers_selects_a_range_and_command_toggles() {
    let (app, x) = app();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| app);
    let fx: Vec<u64> = layer(&h.state().session, x.small).effects().unwrap().groups().map(|g| g.uid).collect();
    // All twirled closed, so every header is on screen.
    h.state_mut().ui.fx_closed.extend(fx.iter().copied());
    h.run_steps(3);
    let pick = |h: &mut Harness<'_, EffectcraftApp>, i: usize, modifiers: egui::Modifiers| {
        let at = rect_of(h, &format!("effectControls.effect.{}", fx[i])).center();
        h.event(egui::Event::ModifiersChanged(modifiers));
        click(h, at);
        h.event(egui::Event::ModifiersChanged(Default::default()));
        h.step();
        let picked = h.state().session.state.selected_props.iter().map(|(_, u)| *u).collect::<std::collections::BTreeSet<_>>();
        fx.iter().enumerate().filter(|(_, u)| picked.contains(u)).map(|(i, _)| i).collect::<Vec<_>>()
    };
    let (shift, command) = (egui::Modifiers::SHIFT, egui::Modifiers::COMMAND);
    assert_eq!(pick(&mut h, 3, shift), [3], "with nothing picked, the range is the effect itself");
    assert_eq!(pick(&mut h, 1, Default::default()), [1]);
    assert_eq!(pick(&mut h, 3, shift), [1, 2, 3], "Shift takes in the effects between");
    assert_eq!(pick(&mut h, 0, shift), [0, 1], "the next range starts from the same effect");
    assert_eq!(pick(&mut h, 4, shift), [1, 2, 3, 4]);
    assert_eq!(pick(&mut h, 2, command), [1, 3, 4], "Cmd/Ctrl takes one out");
    assert_eq!(pick(&mut h, 5, command), [1, 3, 4, 5], "and adds one");
    assert_eq!(pick(&mut h, 3, shift), [3, 4, 5], "which is where the next range starts");
    assert_eq!(pick(&mut h, 0, Default::default()), [0], "a plain click picks one");
}

fn rect_of(h: &Harness<'_, EffectcraftApp>, id: &str) -> egui::Rect {
    let e = h.state().auto.previous.iter().chain(h.state().auto.elements.iter()).find(|e| e.id == id).unwrap_or_else(|| panic!("no {id}")).clone();
    egui::Rect::from_min_size(egui::pos2(e.rect[0], e.rect[1]), egui::vec2(e.rect[2], e.rect[3]))
}

/// Drag one pixel per frame, as a slow hand does.
fn slow_drag(h: &mut Harness<'_, EffectcraftApp>, from: egui::Pos2, dx: f32) {
    h.event(egui::Event::PointerMoved(from));
    h.step();
    h.event(egui::Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    for k in 1..=dx as i32 {
        h.event(egui::Event::PointerMoved(from + egui::vec2(k as f32, 0.0)));
        h.step();
    }
    let to = from + egui::vec2(dx, 0.0);
    h.event(egui::Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.run_steps(3);
}

/// Click a hot number and type a value over it.
fn type_into(h: &mut Harness<'_, EffectcraftApp>, id: &str, text: &str) {
    let p = rect_of(h, id).center();
    click(h, p);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.event(egui::Event::Text(text.into()));
    h.step();
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
}

/// An angle's revolutions ("0x") scrub and take typing on their own, in Effect Controls and
/// the Timeline, keeping the degrees (#93).
#[test]
fn angle_revolutions_scrub_and_take_typing() {
    let (app, x) = app();
    let mut h = Harness::builder().with_size(egui::vec2(1700.0, 1100.0)).build_eframe(|_| app);
    settle(&mut h);
    let angle = |h: &Harness<'_, EffectcraftApp>| layer(&h.state().session, x.small).effects().unwrap().find(x.angle).unwrap().value.as_f64();
    h.state_mut().session.execute("prop.set", json!({"layer": x.small, "prop": x.angle, "value": 45.0})).unwrap();
    h.run_steps(3);
    let revs = format!("effectControls.prop.{}.revolutions", x.angle);
    // Ten pixels a turn, however slowly the pointer moves; the degrees stay.
    let from = rect_of(&h, &revs).center();
    slow_drag(&mut h, from, 40.0);
    let v = angle(&h);
    assert!((v - 45.0).rem_euclid(360.0).abs() < 1e-9 && (2.0..=4.0).contains(&((v - 45.0) / 360.0)), "{v}");
    type_into(&mut h, &revs, "3");
    assert_eq!(angle(&h), 3.0 * 360.0 + 45.0);
    let deg = format!("effectControls.prop.{}.value", x.angle);
    assert_eq!(h.state().auto.find(&deg).map(|e| e.label.clone()), Some("3x+45.0°".into()));
    // Negative angles keep their sign: -1x-30° → 2x-30°.
    h.state_mut().session.execute("prop.set", json!({"layer": x.small, "prop": x.angle, "value": -390.0})).unwrap();
    h.run_steps(3);
    type_into(&mut h, &revs, "2");
    assert_eq!(angle(&h), 2.0 * 360.0 - 30.0);

    // The Timeline's Rotation.
    let rot = layer(&h.state().session, x.small).props.prop("transform/rotation").unwrap().uid;
    h.state_mut().show_panel(PanelKind::Timeline);
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "timeline.reveal.rotation", json!({})).unwrap();
    h.run_steps(4);
    type_into(&mut h, &format!("timeline.prop.{rot}.revolutions"), "-2");
    let rotation = |h: &Harness<'_, EffectcraftApp>| layer(&h.state().session, x.small).props.prop("transform/rotation").unwrap().value.as_f64();
    assert_eq!(rotation(&h), -720.0);
    // The Properties panel's Rotation.
    h.state_mut().show_panel(PanelKind::Properties);
    h.run_steps(4);
    type_into(&mut h, &format!("properties.prop.{rot}.revolutions"), "1");
    assert_eq!(rotation(&h), 360.0);
}

fn right_click(h: &mut Harness<'_, EffectcraftApp>, pos: egui::Pos2) {
    h.event(egui::Event::PointerMoved(pos));
    h.step();
    h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Secondary, pressed: true, modifiers: Default::default() });
    h.step();
    h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Secondary, pressed: false, modifiers: Default::default() });
    h.run_steps(2);
}

fn hover(h: &mut Harness<'_, EffectcraftApp>, pos: egui::Pos2) {
    h.event(egui::Event::PointerMoved(pos));
    h.run_steps(3);
}

/// #227: right-clicking Effect Controls where no control has a menu of its own shows the
/// Effect menu (After Effects), which applies an effect to the selected layers; an effect's
/// header keeps its own menu.
#[test]
fn right_click_in_effect_controls_shows_the_effect_menu() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "frameRate": 30, "duration": 4})).unwrap();
    let a = s.execute("layer.newSolid", json!({"name": "A", "color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
    let b = s.execute("layer.newSolid", json!({"name": "B", "color": "#804060"})).unwrap()["layer"].as_u64().unwrap();
    let fill = s.execute("effect.apply", json!({"layers": [b], "effect": "Fill"})).unwrap()["effects"][0].as_u64().unwrap();
    s.state.selected_layers = vec![LayerId(b), LayerId(a)];
    let mut app = EffectcraftApp::new(s);
    app.show_panel(PanelKind::EffectControls);
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| app);
    h.run_steps(3);
    let category = " Blur & Sharpen";
    // An effect's header: its own menu.
    let header = rect_of(&h, &format!("effectControls.effect.{fill}")).center();
    right_click(&mut h, header);
    assert!(h.query_by_label("Duplicate").is_some(), "the effect's own menu");
    assert!(h.query_by_label(category).is_none());
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    // A control without a menu (the header's Reset): the Effect menu.
    let reset = rect_of(&h, &format!("effectControls.effect.{fill}.reset")).center();
    right_click(&mut h, reset);
    assert!(h.query_by_label(category).is_some());
    assert_eq!(layer(&h.state().session, b).effects().map(|f| f.groups().count()), Some(1), "Reset didn't run");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(h.query_by_label(category).is_none());
    // The empty area below it: the Effect menu.
    let panel = rect_of(&h, "panel.EffectControls");
    right_click(&mut h, egui::pos2(panel.center().x, panel.max.y - 30.0));
    let cat = h.query_by_label(category).expect("the Effect menu's categories").rect();
    assert!(h.query_by_label_contains("Remove All").is_some(), "the whole Effect menu");
    hover(&mut h, cat.center());
    let at = h.query_by_label(" Gaussian Blur").expect("Blur & Sharpen ▸ Gaussian Blur").rect().center();
    hover(&mut h, egui::pos2(at.x, cat.center().y));
    for k in 1..=10 {
        hover(&mut h, egui::pos2(at.x, cat.center().y + (at.y - cat.center().y) * k as f32 / 10.0));
    }
    click(&mut h, at);
    let names = |h: &Harness<'_, EffectcraftApp>, l: u64| -> Vec<String> {
        layer(&h.state().session, l).effects().map(|f| f.groups().map(|g| g.name.clone()).collect()).unwrap_or_default()
    };
    assert_eq!(names(&h, b), ["Fill", "Gaussian Blur"]);
    assert_eq!(names(&h, a), ["Gaussian Blur"], "every selected layer gets it");
    assert!(h.query_by_label(category).is_none(), "the menu closed");
}

/// #295: EXtractoR on a multi-layer OpenEXR layer offers the file's layers (a Layer popup that
/// sets red, green, blue and alpha in one undo step) and its channels (a popup per colour);
/// there was no way to pick any.
#[test]
fn extractor_offers_the_exr_layers_and_channels() {
    use exr::prelude::*;
    let (w, hh) = (8usize, 4usize);
    let ch = |n: &str, v: f32| AnyChannel::new(n, FlatSamples::F32(vec![v; w * hh]));
    let channels = AnyChannels::sort(
        vec![ch("diffuse.R", 1.0), ch("diffuse.G", 0.5), ch("diffuse.B", 0.0), ch("spec.R", 0.0), ch("spec.G", 0.0), ch("spec.B", 1.0), ch("depth.Z", 0.25)]
            .into(),
    );
    let path = std::env::temp_dir().join(format!("ec-ui-exr-layers-{}.exr", std::process::id()));
    Image::from_layer(exr::prelude::Layer::new((w, hh), LayerAttributes::default(), Encoding::FAST_LOSSLESS, channels)).write().to_file(&path).unwrap();

    let mut s = effectcraft_host::session();
    let item = s.execute("file.import", json!({"paths": [path.to_string_lossy()]})).unwrap()["items"][0].clone();
    s.execute("comp.new", json!({"name": "EXR", "width": 8, "height": 4, "frameRate": 24, "duration": 1})).unwrap();
    let l = s.execute("layer.addItem", json!({"item": item})).unwrap()["layer"].as_u64().unwrap();
    let fx = s.execute("effect.apply", json!({"layers": [l], "effect": "EXtractoR"})).unwrap()["effects"][0].as_u64().unwrap();
    let uid = |s: &Session, id: &str| layer(s, l).effects().and_then(|f| f.groups().find(|g| g.uid == fx)).and_then(|g| g.get(id)).map(|p| p.uid).unwrap();
    let red = uid(&s, "red");
    s.state.selected_layers = vec![LayerId(l)];
    let mut app = EffectcraftApp::new(s);
    app.show_panel(PanelKind::EffectControls);
    let mut h = Harness::builder().with_size(egui::vec2(1700.0, 1100.0)).build_eframe(|_| app);
    settle(&mut h);
    let values = |h: &Harness<'_, EffectcraftApp>| -> Vec<String> {
        let l = layer(&h.state().session, l);
        let g = l.effects().and_then(|f| f.groups().find(|g| g.uid == fx)).unwrap();
        ["red", "green", "blue", "alpha"]
            .iter()
            .map(|id| {
                g.get(id)
                    .map(|p| match &p.value {
                        KV::Str(s) => s.clone(),
                        _ => String::new(),
                    })
                    .unwrap_or_default()
            })
            .collect()
    };
    assert_eq!(values(&h), ["R", "G", "B", "A"]);

    // Layer ▸ spec: its channels in red, green and blue (no alpha channel: none).
    let at = rect_of(&h, &format!("effectControls.effect.{fx}.extractor.layer")).center();
    click(&mut h, at);
    for name in ["depth", "diffuse", "spec"] {
        assert!(h.query_by_label(name).is_some(), "the file's layer {name}");
    }
    h.get_by_label("spec").click();
    h.run_steps(3);
    assert_eq!(values(&h), ["spec.R", "spec.G", "spec.B", ""]);
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(values(&h), ["R", "G", "B", "A"], "one undo step");
    h.run_steps(2);

    // Red ▸ depth.Z: every channel of the file is listed.
    let at = rect_of(&h, &format!("effectControls.prop.{red}.value")).center();
    click(&mut h, at);
    assert!(h.query_by_label("diffuse.G").is_some() && h.query_by_label("(none)").is_some());
    h.get_by_label("depth.Z").click();
    h.run_steps(3);
    assert_eq!(values(&h)[0], "depth.Z");
    let _ = std::fs::remove_file(path);
}

/// #473: Cryptomatte's Layer popup lists the Cryptomatte layers of the footage's OpenEXR (a file
/// with only CryptoMaterial shows CryptoMaterial, not CryptoObject), and the three standard ones
/// when the layer has no such footage. The stored value stays the standard index.
#[test]
fn cryptomatte_layer_popup_follows_the_footage() {
    use exr::prelude::*;
    let (w, hh) = (8usize, 4usize);
    let ch = |n: &str, v: f32| AnyChannel::new(n, FlatSamples::F32(vec![v; w * hh]));
    let channels = AnyChannels::sort(vec![ch("R", 0.5), ch("G", 0.5), ch("B", 0.5), ch("CryptoMaterial00.R", 1.0), ch("CryptoMaterial00.G", 1.0)].into());
    let mut exr_layer = exr::prelude::Layer::new((w, hh), LayerAttributes::default(), Encoding::FAST_LOSSLESS, channels);
    exr_layer.attributes.other.insert(Text::from("cryptomatte/abc1234/name"), AttributeValue::Text(Text::from("CryptoMaterial")));
    exr_layer.attributes.other.insert(Text::from("cryptomatte/abc1234/manifest"), AttributeValue::Text(Text::from(r#"{"wood":"13851a76"}"#)));
    let path = std::env::temp_dir().join(format!("ec-ui-exr-crypto-{}.exr", std::process::id()));
    Image::from_layer(exr_layer).write().to_file(&path).unwrap();

    let mut s = effectcraft_host::session();
    let item = s.execute("file.import", json!({"paths": [path.to_string_lossy()]})).unwrap()["items"][0].clone();
    s.execute("comp.new", json!({"name": "EXR", "width": 8, "height": 4, "frameRate": 24, "duration": 1})).unwrap();
    let l = s.execute("layer.addItem", json!({"item": item})).unwrap()["layer"].as_u64().unwrap();
    let solid = s.execute("layer.newSolid", json!({"name": "S", "color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
    let apply = |s: &mut Session, l: u64| s.execute("effect.apply", json!({"layers": [l], "effect": "Cryptomatte"})).unwrap()["effects"][0].as_u64().unwrap();
    let (fx, solid_fx) = (apply(&mut s, l), apply(&mut s, solid));
    let layer_prop = |s: &Session, l: u64, fx: u64| {
        layer(s, l).effects().and_then(|f| f.groups().find(|g| g.uid == fx)).and_then(|g| g.get("layer")).map(|p| (p.uid, p.value.as_enum())).unwrap()
    };
    let (uid, stored) = layer_prop(&s, l, fx);
    assert_eq!(stored, 0, "a new effect stores CryptoObject");
    let (solid_uid, _) = layer_prop(&s, solid, solid_fx);
    s.state.selected_layers = vec![LayerId(l)];
    let mut app = EffectcraftApp::new(s);
    app.show_panel(PanelKind::EffectControls);
    let mut h = Harness::builder().with_size(egui::vec2(1700.0, 1100.0)).build_eframe(|_| app);
    settle(&mut h);
    // The file has only CryptoMaterial, so that is what the closed popup shows, and all it lists.
    assert!(h.query_by_label("CryptoMaterial").is_some() && h.query_by_label("CryptoObject").is_none());
    let at = rect_of(&h, &format!("effectControls.prop.{uid}.value")).center();
    click(&mut h, at);
    assert!(h.query_by_label("CryptoObject").is_none() && h.query_by_label("CryptoAsset").is_none(), "only the file's layers");
    assert_eq!(h.query_all_by_label("CryptoMaterial").count(), 2, "the popup and its entry");
    h.query_all_by_label("CryptoMaterial").last().unwrap().click();
    h.run_steps(3);
    assert_eq!(layer_prop(&h.state().session, l, fx).1, 1, "stored as the standard CryptoMaterial value");

    // A layer without Cryptomatte footage lists the standard three.
    h.state_mut().session.state.selected_layers = vec![LayerId(solid)];
    settle(&mut h);
    let at = rect_of(&h, &format!("effectControls.prop.{solid_uid}.value")).center();
    click(&mut h, at);
    for name in ["CryptoMaterial", "CryptoAsset"] {
        assert!(h.query_by_label(name).is_some(), "the standard layer {name}");
    }
    h.get_by_label("CryptoAsset").click();
    h.run_steps(3);
    assert_eq!(layer_prop(&h.state().session, solid, solid_fx).1, 2);
    let _ = std::fs::remove_file(path);
}

#[test]
fn point_picker_handle_follows_hover_then_commits_once_across_effects() {
    for (effect, parameter) in [("ec.control.point", "point"), ("ec.blur.ccradialfast", "center"), ("ec.generate.gradientramp", "start")] {
        let mut s = Session::default();
        s.execute("comp.new", json!({"width":640,"height":360,"duration":1})).unwrap();
        let lid = s.execute("layer.newSolid", json!({"width":200,"height":100,"color":"#406080"})).unwrap()["layer"].as_u64().unwrap();
        s.execute("prop.set", json!({"layer":lid,"path":"transform/scale","value":[200,200]})).unwrap();
        let fx = s.execute("effect.apply", json!({"layer":lid,"effect":effect})).unwrap()["effects"][0].as_u64().unwrap();
        let point = layer(&s, lid).effects().unwrap().find_group(fx).unwrap().get(parameter).unwrap().uid;
        s.execute("prop.set", json!({"layer":lid,"prop":point,"value":[100,50]})).unwrap();
        let mut app = EffectcraftApp::new(s);
        app.show_panel(PanelKind::EffectControls);
        app.ui.viewer.zoom = Some(0.85);
        app.ui.viewer.pan = [25.0, -10.0];
        let mut h = Harness::builder().with_size(egui::vec2(1700.0, 1100.0)).build_eframe(|_| app);
        settle(&mut h);
        let id = format!("viewer.effectPoint.{point}");
        let original = rect_of(&h, &id).center();
        let pick_button = rect_of(&h, &format!("effectControls.prop.{point}.crosshair")).center();
        click(&mut h, pick_button);
        assert!(h.state().ui.fx_pick.is_some(), "{effect}: crosshair arms the picker");
        assert_eq!(rect_of(&h, &id).center(), original, "the handle remains visible before entering the viewer");
        let undo = h.state().session.history.undo.len();
        for comp in [[350.0, 200.0], [370.0, 220.0]] {
            let pos = effectcraft_ui_egui::panels::viewer::comp_to_screen(&h.ctx, comp).unwrap();
            hover(&mut h, pos);
            assert!(rect_of(&h, &id).center().distance(pos) < 0.1, "{effect}: handle must follow the pointer before clicking");
            assert_eq!(layer(&h.state().session, lid).props.find(point).unwrap().value.as_vec2(), [100.0, 50.0], "hover is only a preview");
            assert_eq!(h.state().session.history.undo.len(), undo, "hover must not add undo entries");
        }
        let pos = effectcraft_ui_egui::panels::viewer::comp_to_screen(&h.ctx, [370.0, 220.0]).unwrap();
        click(&mut h, pos);
        assert!(h.state().ui.fx_pick.is_none());
        let after = layer(&h.state().session, lid).props.find(point).unwrap().value.as_vec2();
        assert!((after[0] - 125.0).abs() < 1.0 && (after[1] - 70.0).abs() < 1.0, "{effect}: click uses layer space: {after:?}");
        assert_eq!(h.state().session.history.undo.len(), undo + 1, "one click is one edit");
        h.state_mut().session.execute("edit.undo", json!({})).unwrap();
        assert_eq!(layer(&h.state().session, lid).props.find(point).unwrap().value.as_vec2(), [100.0, 50.0]);
        assert_eq!(layer(&h.state().session, lid).props.prop("transform/position").unwrap().value.as_vec2(), [320.0, 180.0]);
    }
}

#[test]
fn point_picker_preview_cancels_and_works_with_layer_controls_hidden() {
    let (app, x) = app();
    let mut h = Harness::builder().with_size(egui::vec2(1700.0, 1100.0)).build_eframe(|_| app);
    settle(&mut h);
    let value = layer(&h.state().session, x.small).props.find(x.point).unwrap().value.clone();
    let undo = h.state().session.history.undo.len();
    h.state_mut().ui.viewer.show_layer_controls = false;
    let pick_button = rect_of(&h, &format!("effectControls.prop.{}.crosshair", x.point)).center();
    click(&mut h, pick_button);
    let id = format!("viewer.effectPoint.{}", x.point);
    let original = rect_of(&h, &id).center();
    let pos = effectcraft_ui_egui::panels::viewer::comp_to_screen(&h.ctx, [370.0, 220.0]).unwrap();
    hover(&mut h, pos);
    assert!(rect_of(&h, &id).center().distance(pos) < 0.1, "explicit picks show a handle even with layer controls hidden");
    hover(&mut h, pick_button);
    assert_eq!(rect_of(&h, &id).center(), original, "leaving the viewer restores the current position");
    hover(&mut h, pos);
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(h.state().ui.fx_pick.is_none());
    assert!(!ids(&h).contains(&id), "cancel removes the temporary handle");
    assert_eq!(layer(&h.state().session, x.small).props.find(x.point).unwrap().value, value);
    assert_eq!(h.state().session.history.undo.len(), undo, "cancel leaves the project unchanged");
}

#[test]
fn radial_center_stays_draggable_when_its_property_is_selected() {
    let (mut app, x) = app();
    app.session.execute("effect.apply", json!({"layer":x.small,"effect":"ec.blur.ccradialfast"})).unwrap();
    let l = layer(&app.session, x.small);
    let effect = l.effects().unwrap().groups().find(|g| g.match_id == "ec.blur.ccradialfast").unwrap();
    let point = effect.get("center").unwrap().uid;
    app.session.state.selected_props = vec![(LayerId(x.small), point)];
    let mut h = Harness::builder().with_size(egui::vec2(1700.0, 1100.0)).build_eframe(|_| app);
    settle(&mut h);
    let id = format!("viewer.effectPoint.{point}");
    assert!(ids(&h).contains(&id), "selected point must keep its handle");
    let from = rect_of(&h, &id).center();
    let to = from + egui::vec2(40.0, 20.0);
    h.event(egui::Event::PointerMoved(from));
    h.step();
    h.event(egui::Event::PointerButton { pos: from, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
    h.step();
    for i in 1..=5 {
        h.event(egui::Event::PointerMoved(from + (to - from) * (i as f32 / 5.0)));
        h.step();
    }
    h.event(egui::Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
    h.step();
    let after = layer(&h.state().session, x.small).props.find(point).unwrap().value.as_vec2();
    assert!(after[0] > 100.0 && after[1] > 50.0, "drag updates Center: {after:?}");
    let position = layer(&h.state().session, x.small).props.prop("transform/position").unwrap().value.as_vec3();
    assert_eq!(position[..2], [320.0, 180.0], "dragging the effect must not move the layer");
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(layer(&h.state().session, x.small).props.find(point).unwrap().value.as_vec2(), [100.0, 50.0], "one Undo reverses the gesture");
}

#[test]
fn clicking_an_effect_parameter_activates_its_header_and_handles() {
    let (app, x) = app();
    let mut h = Harness::builder().with_size(egui::vec2(1700.0, 1100.0)).with_step_dt(1.0 / 60.0).build_eframe(|_| app);
    settle(&mut h);
    let name = rect_of(&h, &format!("effectControls.prop.{}.name", x.point)).center();
    click(&mut h, name);
    assert_eq!(h.state().session.state.selected_props, vec![(LayerId(x.small), x.point)]);
    assert!(ids(&h).contains(&format!("viewer.effectPoint.{}", x.point)), "clicking the parameter activates the owner effect");
}

#[test]
fn non_point_parameters_activate_handles_across_effect_families() {
    for (effect, parameter, points) in [
        ("ec.distort.cclens", "size", vec!["center"]),
        ("ec.generate.gradientramp", "startColor", vec!["start", "end"]),
        ("ec.generate.beam", "length", vec!["startPoint", "endPoint"]),
    ] {
        let mut s = Session::default();
        s.execute("comp.new", json!({"width":320,"height":180,"duration":1})).unwrap();
        let lid = s.execute("layer.newSolid", json!({"color":"#406080"})).unwrap()["layer"].as_u64().unwrap();
        let uid = s.execute("effect.apply", json!({"layer":lid,"effect":effect})).unwrap()["effects"][0].as_u64().unwrap();
        let l = layer(&s, lid);
        let g = l.effects().unwrap().find_group(uid).unwrap();
        let prop = g.prop(parameter).unwrap().uid;
        let point_ids: Vec<_> = points.iter().map(|name| g.prop(name).unwrap().uid).collect();
        s.state.selected_props.clear();
        let mut app = EffectcraftApp::new(s);
        app.show_panel(PanelKind::EffectControls);
        let mut h = Harness::builder().with_size(egui::vec2(1700.0, 1100.0)).build_eframe(|_| app);
        settle(&mut h);
        let name = rect_of(&h, &format!("effectControls.prop.{prop}.name")).center();
        click(&mut h, name);
        assert_eq!(h.state().session.state.selected_props, vec![(LayerId(lid), prop)], "{effect}");
        for point in point_ids {
            assert!(ids(&h).contains(&format!("viewer.effectPoint.{point}")), "{effect}: all its point handles activate");
        }
    }
}

#[test]
fn custom_effect_editors_activate_their_owner() {
    let (app, x) = app();
    let mut h = Harness::builder().with_size(egui::vec2(1700.0, 1100.0)).build_eframe(|_| app);
    settle(&mut h);
    let graph = rect_of(&h, &format!("effectControls.effect.{}.curves.graph", x.curves)).center();
    click(&mut h, graph);
    assert!(h.state().session.state.selected_props.contains(&(LayerId(x.small), x.curves)));
}

/// #514: Apply Color LUT asks for its LUT as it is applied (as in After Effects), Effect
/// Controls shows the file with a Choose… button that picks another, and the LUT changes the
/// pixels. #473: a text parameter (Cryptomatte's Selection) has a field that sets it.
#[test]
fn file_and_text_parameters_have_widgets() {
    let dir = std::env::temp_dir().join(format!("ec-ui-lut-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // A 2×2×2 LUT that inverts, and one that does nothing.
    let cube = |inv: bool| {
        let mut t = String::from("LUT_3D_SIZE 2\n");
        for b in 0..2 {
            for g in 0..2 {
                for r in 0..2 {
                    let f = |c: i32| if inv { 1 - c } else { c };
                    t += &format!("{} {} {}\n", f(r), f(g), f(b));
                }
            }
        }
        t
    };
    let (invert, identity) = (dir.join("invert.cube"), dir.join("identity.cube"));
    std::fs::write(&invert, cube(true)).unwrap();
    std::fs::write(&identity, cube(false)).unwrap();
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "LUT", "width": 64, "height": 32, "frameRate": 30, "duration": 1})).unwrap();
    let l = s.execute("layer.newSolid", json!({"name": "Plate", "color": "#ff0000", "width": 64, "height": 32})).unwrap()["layer"].as_u64().unwrap();
    s.state.selected_layers = vec![LayerId(l)];
    let mut app = EffectcraftApp::new(s);
    app.show_panel(PanelKind::EffectControls);
    let picked = std::sync::Arc::new(std::sync::Mutex::new(invert.to_string_lossy().to_string()));
    let pick = picked.clone();
    app.hooks.pick_files = Some(Box::new(move |exts: &[&str]| {
        assert_eq!(exts, ["cube"]);
        vec![pick.lock().unwrap().clone()]
    }));
    let mut h = Harness::builder().with_size(egui::vec2(1700.0, 1100.0)).build_eframe(|_| app);
    settle(&mut h);
    let ctx = h.ctx.clone();
    let r = effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "effect.apply", json!({"effect": "Apply Color LUT"})).unwrap();
    let fx = r["effects"][0].as_u64().unwrap();
    let lut = |h: &Harness<'_, EffectcraftApp>| {
        let g = layer(&h.state().session, l).effects().and_then(|f| f.groups().find(|g| g.uid == fx).cloned()).unwrap();
        let p = g.get("lut").unwrap().clone();
        (
            p.uid,
            match p.value {
                KV::Str(s) => s,
                _ => String::new(),
            },
        )
    };
    assert_eq!(lut(&h).1, invert.to_string_lossy(), "the LUT was asked for on apply");
    let px = |h: &Harness<'_, EffectcraftApp>| {
        let s = &h.state().session;
        s.render(s.active_comp_id().unwrap(), effectcraft_engine::time::Tick::ZERO, Default::default()).get(10, 10)
    };
    assert!(px(&h)[0] < 0.05 && px(&h)[1] > 0.95, "red inverted to cyan: {:?}", px(&h));
    settle(&mut h);
    let uid = lut(&h).0;
    assert!(h.state().auto.find(&format!("effectControls.prop.{uid}.value")).is_some_and(|e| e.label == "invert.cube"));
    *picked.lock().unwrap() = identity.to_string_lossy().to_string();
    let at = rect_of(&h, &format!("effectControls.prop.{uid}.choose")).center();
    click(&mut h, at);
    assert_eq!(lut(&h).1, identity.to_string_lossy());
    assert!(px(&h)[0] > 0.95, "{:?}", px(&h));

    // A text parameter: type into its field; it is set when the field loses the focus.
    let crypto = h.state_mut().session.execute("effect.apply", json!({"layers": [l], "effect": "ec.3d.cryptomatte"})).unwrap()["effects"][0].as_u64().unwrap();
    settle(&mut h);
    let sel =
        layer(&h.state().session, l).effects().and_then(|f| f.groups().find(|g| g.uid == crypto).and_then(|g| g.get("selection")).map(|p| p.uid)).unwrap();
    let at = rect_of(&h, &format!("effectControls.prop.{sel}.value")).center();
    click(&mut h, at);
    h.event(egui::Event::Text("left*".into()));
    h.step();
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    let v = layer(&h.state().session, l).props.find(sel).unwrap().value.clone();
    assert_eq!(v, KV::Str("left*".into()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn effect_control_name_can_be_renamed_from_its_header() {
    let (app, x) = app();
    let mut h = Harness::builder().with_size(egui::vec2(1700.0, 1100.0)).with_step_dt(1.0 / 60.0).build_eframe(|_| app);
    settle(&mut h);
    let name = format!("effectControls.effect.{}.name", x.point_fx);
    let point = rect_of(&h, &name).center();
    double_click(&mut h, point);
    h.run_steps(3);
    assert!(ids(&h).contains(&format!("effectControls.effect.{}.nameEdit", x.point_fx)));
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.event(egui::Event::Text("Speed".into()));
    h.step();
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    let current_name =
        |h: &Harness<'_, EffectcraftApp>| layer(&h.state().session, x.small).effects().unwrap().groups().find(|g| g.uid == x.point_fx).unwrap().name.clone();
    assert_eq!(current_name(&h), "Speed");
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(current_name(&h), "Point Control");
}

// Regression test for #1497: double-clicking an effect parameter's name in Effect Controls did
// nothing; it should reveal that property in the Timeline, as After Effects does.
#[test]
fn double_clicking_an_effect_property_name_reveals_it_in_the_timeline() {
    let (app, x) = app();
    let mut h = Harness::builder().with_size(egui::vec2(1700.0, 1100.0)).with_step_dt(1.0 / 60.0).build_eframe(|_| app);
    settle(&mut h);
    let row = format!("timeline.prop.{}.name", x.point);
    assert!(!ids(&h).contains(&row), "the effect's property is not in the Timeline before the double-click");
    assert!(!h.state().ui.timeline.reveal_props.contains(&x.point));
    let point = rect_of(&h, &format!("effectControls.prop.{}.name", x.point)).center();
    double_click(&mut h, point);
    h.run_steps(4);
    assert!(h.state().ui.timeline.reveal_props.contains(&x.point), "the property is marked revealed in the Timeline");
    assert!(ids(&h).contains(&row), "the Timeline shows the property's row");
}

#[test]
fn effect_rename_escape_keeps_the_name() {
    let (app, x) = app();
    let mut h = Harness::builder().with_size(egui::vec2(1700.0, 1100.0)).with_step_dt(1.0 / 60.0).build_eframe(|_| app);
    settle(&mut h);
    let point = rect_of(&h, &format!("effectControls.effect.{}.name", x.point_fx)).center();
    double_click(&mut h, point);
    h.run_steps(3);
    assert!(ids(&h).contains(&format!("effectControls.effect.{}.nameEdit", x.point_fx)));
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.event(egui::Event::Text("Speed".into()));
    h.step();
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    let name = layer(&h.state().session, x.small).effects().unwrap().groups().find(|g| g.uid == x.point_fx).unwrap().name.clone();
    assert_eq!(name, "Point Control");
    assert!(!ids(&h).contains(&format!("effectControls.effect.{}.nameEdit", x.point_fx)));
}

/// Two presses within egui's double-click time (one frame apart).
fn double_click(h: &mut Harness<'_, EffectcraftApp>, pos: egui::Pos2) {
    h.event(egui::Event::PointerMoved(pos));
    h.step();
    for pressed in [true, false, true, false] {
        h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
        h.step();
    }
    h.run_steps(2);
}
