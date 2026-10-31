//! Preview in the settings dialogs that edit something that exists (After Effects' Preview
//! checkbox): Solid Settings on a solid layer, Camera Settings, Light Settings and Composition
//! Settings. A change of the draft reaches the project at once, so the canvas shows it while it is
//! made; OK keeps it in one undo step, Cancel and Escape take it back, and with Preview off nothing
//! is applied until OK. The window behind a previewing dialog is not dimmed, and the dialog opens
//! at the lower right; the same dialogs creating something new keep the middle.

use effectcraft_engine::Session;
use effectcraft_engine::project::{ItemKind, LayerId, Resolution};
use effectcraft_ui_egui::{Dialog, EffectcraftApp, menus};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::{Value, json};

fn harness(setup: impl FnOnce(&mut Session)) -> Harness<'static, EffectcraftApp> {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "duration": 10})).unwrap();
    setup(&mut s);
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    h
}

fn open(h: &mut Harness<'_, EffectcraftApp>, id: &str, params: Value) {
    let ctx = h.ctx.clone();
    menus::invoke(h.state_mut(), &ctx, id, params).unwrap();
    h.run_steps(3);
}

fn center(h: &Harness<'_, EffectcraftApp>, id: &str) -> Pos2 {
    let r = h.state().auto.find(id).unwrap_or_else(|| panic!("no automation id {id}")).rect;
    pos2(r[0] + r[2] / 2.0, r[1] + r[3] / 2.0)
}

fn click(h: &mut Harness<'_, EffectcraftApp>, id: &str) {
    let p = center(h, id);
    h.input_mut().events.push(Event::PointerMoved(p));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

fn escape(h: &mut Harness<'_, EffectcraftApp>) {
    h.input_mut().events.push(Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.input_mut().events.push(Event::Key { key: Key::Escape, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

fn undo_len(h: &Harness<'_, EffectcraftApp>) -> usize {
    h.state().session.history.undo.len()
}

fn solid_size(h: &Harness<'_, EffectcraftApp>, layer: u64) -> [u32; 2] {
    let s = &h.state().session;
    let l = s.active_comp().unwrap().layer(LayerId(layer)).unwrap();
    let effectcraft_engine::project::LayerSource::Solid { item } = l.source else { panic!("not a solid") };
    match &s.project.item(item).unwrap().kind {
        ItemKind::Solid(so) => [so.width, so.height],
        k => panic!("{k:?}"),
    }
}

fn scalar(h: &Harness<'_, EffectcraftApp>, layer: u64, path: &str) -> f64 {
    let s = &h.state().session;
    let l = s.active_comp().unwrap().layer(LayerId(layer)).unwrap();
    l.props.prop(path).unwrap().value_at(l.layer_time(s.time())).as_f64()
}

fn comp_width(h: &Harness<'_, EffectcraftApp>) -> u32 {
    h.state().session.active_comp().unwrap().width
}

fn solid_setup(s: &mut Session) {
    s.execute("layer.newSolid", json!({"name": "Box", "color": "#ff0000", "width": 100, "height": 100})).unwrap();
}

fn solid_layer(h: &Harness<'_, EffectcraftApp>) -> u64 {
    h.state().session.active_comp().unwrap().layers[0].id.0
}

#[test]
fn solid_settings_previews_live_and_cancel_and_escape_take_it_back() {
    let mut h = harness(solid_setup);
    let l = solid_layer(&h);
    let undo = undo_len(&h);
    open(&mut h, "layer.settings", json!({"layer": l}));
    assert_eq!(h.state().dialog, Some(Dialog::SolidSettings));
    assert!(h.state().auto.find("dialog.solid.preview").is_some(), "a Preview checkbox");
    h.state_mut().dialog_state_mut().solid_size = [200, 150];
    h.run_steps(3);
    assert_eq!(solid_size(&h, l), [200, 150], "the canvas shows the new size before OK");
    h.state_mut().dialog_state_mut().solid_size = [240, 120];
    h.run_steps(3);
    assert_eq!(solid_size(&h, l), [240, 120]);
    assert_eq!(undo_len(&h), undo + 1, "however much was tried, the project is the original plus one command");
    click(&mut h, "dialog.solid.cancel");
    assert_eq!(h.state().dialog, None);
    assert_eq!(solid_size(&h, l), [100, 100], "Cancel puts it back");
    assert_eq!(undo_len(&h), undo, "and leaves no undo step");
    // Escape does the same.
    open(&mut h, "layer.settings", json!({"layer": l}));
    h.state_mut().dialog_state_mut().solid_size = [200, 150];
    h.run_steps(3);
    assert_eq!(solid_size(&h, l), [200, 150]);
    escape(&mut h);
    assert_eq!(h.state().dialog, None);
    assert_eq!(solid_size(&h, l), [100, 100]);
    assert_eq!(undo_len(&h), undo);
}

#[test]
fn solid_settings_ok_keeps_the_preview_as_one_undo_step() {
    let mut h = harness(solid_setup);
    let l = solid_layer(&h);
    let undo = undo_len(&h);
    open(&mut h, "layer.settings", json!({"layer": l}));
    h.state_mut().dialog_state_mut().solid_size = [200, 150];
    h.run_steps(3);
    h.state_mut().dialog_state_mut().solid_size = [220, 130];
    h.run_steps(3);
    click(&mut h, "dialog.solid.ok");
    assert_eq!(h.state().dialog, None);
    assert_eq!(solid_size(&h, l), [220, 130]);
    assert_eq!(undo_len(&h), undo + 1, "one undo step");
    h.state_mut().session.execute("edit.undo", json!({})).unwrap();
    assert_eq!(solid_size(&h, l), [100, 100], "undone in one go");
}

#[test]
fn with_preview_off_nothing_is_applied_until_ok() {
    let mut h = harness(solid_setup);
    let l = solid_layer(&h);
    let undo = undo_len(&h);
    open(&mut h, "layer.settings", json!({"layer": l}));
    h.state_mut().dialog_state_mut().solid_size = [200, 150];
    h.run_steps(3);
    assert_eq!(solid_size(&h, l), [200, 150]);
    click(&mut h, "dialog.solid.preview");
    assert!(!h.state().dialog_state().live.on);
    assert_eq!(solid_size(&h, l), [100, 100], "switching Preview off takes the preview back");
    assert_eq!(undo_len(&h), undo);
    h.state_mut().dialog_state_mut().solid_size = [260, 140];
    h.run_steps(3);
    assert_eq!(solid_size(&h, l), [100, 100], "nothing is applied while editing");
    click(&mut h, "dialog.solid.ok");
    assert_eq!(solid_size(&h, l), [260, 140], "OK applies the draft");
    assert_eq!(undo_len(&h), undo + 1);
}

#[test]
fn camera_settings_preview() {
    let mut h = harness(|s| {
        s.execute("layer.newCamera", json!({})).unwrap();
    });
    let l = solid_layer(&h);
    let before = scalar(&h, l, "cameraOptions/zoom");
    let undo = undo_len(&h);
    open(&mut h, "layer.cameraSettings", json!({"layer": l}));
    assert_eq!(h.state().dialog, Some(Dialog::CameraSettings));
    assert!(h.state().auto.find("dialog.camera.preview").is_some());
    h.state_mut().dialog_state_mut().camera.zoom = before + 500.0;
    h.run_steps(3);
    assert!((scalar(&h, l, "cameraOptions/zoom") - (before + 500.0)).abs() < 1e-6, "the zoom changes live");
    assert_eq!(undo_len(&h), undo + 1);
    click(&mut h, "dialog.camera.cancel");
    assert!((scalar(&h, l, "cameraOptions/zoom") - before).abs() < 1e-6, "Cancel puts the zoom back");
    assert_eq!(undo_len(&h), undo);
    open(&mut h, "layer.cameraSettings", json!({"layer": l}));
    h.state_mut().dialog_state_mut().camera.zoom = before + 300.0;
    h.run_steps(3);
    click(&mut h, "dialog.camera.ok");
    assert!((scalar(&h, l, "cameraOptions/zoom") - (before + 300.0)).abs() < 1e-6, "OK keeps it");
    assert_eq!(undo_len(&h), undo + 1);
}

#[test]
fn light_settings_preview() {
    let mut h = harness(|s| {
        s.execute("layer.newLight", json!({"kind": "Point"})).unwrap();
    });
    let l = solid_layer(&h);
    let before = scalar(&h, l, "lightOptions/intensity");
    let undo = undo_len(&h);
    open(&mut h, "layer.lightSettings", json!({"layer": l}));
    assert_eq!(h.state().dialog, Some(Dialog::LightSettings));
    assert!(h.state().auto.find("dialog.light.preview").is_some());
    h.state_mut().dialog_state_mut().light.intensity = before + 37.0;
    h.run_steps(3);
    assert!((scalar(&h, l, "lightOptions/intensity") - (before + 37.0)).abs() < 1e-6, "the intensity changes live");
    escape(&mut h);
    assert_eq!(h.state().dialog, None);
    assert!((scalar(&h, l, "lightOptions/intensity") - before).abs() < 1e-6, "Escape puts it back");
    assert_eq!(undo_len(&h), undo);
}

#[test]
fn composition_settings_preview() {
    let mut h = harness(|_| {});
    let before = comp_width(&h);
    let undo = undo_len(&h);
    open(&mut h, "comp.settings", json!({}));
    assert_eq!(h.state().dialog, Some(Dialog::CompSettings));
    assert!(h.state().auto.find("dialog.comp.preview").is_some());
    h.state_mut().dialog_state_mut().comp.width = before + 80;
    h.run_steps(3);
    assert_eq!(comp_width(&h), before + 80, "the comp is resized live");
    click(&mut h, "dialog.comp.cancel");
    assert_eq!(comp_width(&h), before, "Cancel puts the size back");
    assert_eq!(undo_len(&h), undo);
    open(&mut h, "comp.settings", json!({}));
    h.state_mut().dialog_state_mut().comp.width = before + 40;
    h.run_steps(3);
    click(&mut h, "dialog.comp.ok");
    assert_eq!(comp_width(&h), before + 40, "OK keeps it");
    assert_eq!(undo_len(&h), undo + 1, "in one undo step");
}

/// The Resolution is no part of the Composition Settings command, so a preview never applies it:
/// OK has to, also when it keeps a preview of another field.
#[test]
fn composition_settings_ok_after_a_preview_still_applies_the_resolution() {
    let mut h = harness(|_| {});
    let before = comp_width(&h);
    open(&mut h, "comp.settings", json!({}));
    h.state_mut().dialog_state_mut().comp.width = before + 80;
    h.run_steps(3);
    assert_eq!(comp_width(&h), before + 80, "the preview applied the width");
    h.state_mut().dialog_state_mut().comp.resolution = Resolution::Half;
    h.run_steps(3);
    click(&mut h, "dialog.comp.ok");
    assert_eq!(comp_width(&h), before + 80);
    assert_eq!(h.state().session.active_comp().unwrap().resolution, Resolution::Half, "OK applied the Resolution");
}

/// An edit made in the same frame as OK (Enter) is not in the last preview yet: OK brings the
/// project up to the draft it finds instead of keeping the older preview.
#[test]
fn ok_in_the_frame_of_an_edit_keeps_the_edit_not_the_older_preview() {
    let mut h = harness(solid_setup);
    let l = solid_layer(&h);
    open(&mut h, "layer.settings", json!({"layer": l}));
    h.state_mut().dialog_state_mut().solid_size = [200, 150];
    h.run_steps(3);
    assert_eq!(solid_size(&h, l), [200, 150]);
    h.state_mut().dialog_state_mut().solid_size = [240, 120];
    h.input_mut().events.push(Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.step();
    h.run_steps(2);
    assert_eq!(h.state().dialog, None);
    assert_eq!(solid_size(&h, l), [240, 120], "OK kept the draft it found, not the earlier preview");
}

/// A previewing dialog opens at the lower right so the canvas stays in view; the same dialogs
/// creating something new keep the middle and have no Preview.
#[test]
fn previewing_dialogs_open_at_the_lower_right_and_new_ones_in_the_middle() {
    let mut h = harness(solid_setup);
    let l = solid_layer(&h);
    open(&mut h, "layer.settings", json!({"layer": l}));
    let ok = center(&h, "dialog.solid.ok");
    assert!(ok.x > 800.0 && ok.y > 500.0, "Solid Settings on a layer is in the lower right: {ok:?}");
    click(&mut h, "dialog.solid.cancel");
    open(&mut h, "layer.newSolid", json!({}));
    assert_eq!(h.state().dialog, Some(Dialog::SolidSettings));
    assert!(h.state().auto.find("dialog.solid.preview").is_none(), "a new solid has no Preview");
    let ok = center(&h, "dialog.solid.ok");
    assert!(ok.x < 1000.0, "New Solid stays in the middle: {ok:?}");
}

fn drag(h: &mut Harness<'_, EffectcraftApp>, from: Pos2, to: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(from));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    for k in 1..=6 {
        h.input_mut().events.push(Event::PointerMoved(from + (to - from) * (k as f32 / 6.0)));
        h.step();
    }
    h.input_mut().events.push(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

/// A previewing dialog opens at the lower right, but it can be dragged by its title to wherever
/// the canvas needs to be seen, and the preview keeps working from there.
#[test]
fn a_previewing_dialog_can_be_dragged_by_its_title() {
    let mut h = harness(solid_setup);
    let l = solid_layer(&h);
    open(&mut h, "layer.settings", json!({"layer": l}));
    let ok = center(&h, "dialog.solid.ok");
    let title = h.get_by_label("Solid Settings").rect().center();
    drag(&mut h, title, title + egui::vec2(-500.0, -300.0));
    let moved = center(&h, "dialog.solid.ok");
    assert!(moved.x < ok.x - 300.0 && moved.y < ok.y - 150.0, "the dialog followed the drag: {ok:?} -> {moved:?}");
    // Still previewing, and the position is kept from frame to frame.
    h.state_mut().dialog_state_mut().solid_size = [200, 150];
    h.run_steps(3);
    assert_eq!(solid_size(&h, l), [200, 150]);
    assert_eq!(center(&h, "dialog.solid.ok"), moved, "it stays where it was put");
}
