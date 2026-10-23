//! Composition Settings: saving, picking and deleting your own presets (egui_kittest).

use effectcraft_engine::Session;
use effectcraft_ui_egui::{Dialog, EffectcraftApp};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, pos2};
use egui_kittest::Harness;
use serde_json::json;

fn harness() -> Harness<'static, EffectcraftApp> {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Edit", "width": 2560, "height": 1440, "frameRate": 60, "duration": 4})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    h
}

fn center(h: &Harness<'_, EffectcraftApp>, id: &str) -> Pos2 {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}"));
    pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0)
}

fn click(h: &mut Harness<'_, EffectcraftApp>, id: &str) {
    let p = center(h, id);
    h.input_mut().events.push(Event::PointerMoved(p));
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(1);
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    // The automation registry still holds the previous frame's elements: one more frame.
    h.run_steps(3);
}

fn key(h: &mut Harness<'_, EffectcraftApp>, key: Key) {
    h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
}

fn open_settings(h: &mut Harness<'_, EffectcraftApp>) {
    let ctx = h.ctx.clone();
    effectcraft_ui_egui::menus::invoke(h.state_mut(), &ctx, "comp.settings", json!({})).unwrap();
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::CompSettings));
}

fn preset_names(h: &Harness<'_, EffectcraftApp>) -> Vec<String> {
    h.state().session.prefs.comp_presets.iter().map(|q| q.name.clone()).collect()
}

/// Save the comp's size and rate as a preset (Enter saves without pressing OK), then delete it.
#[test]
fn composition_settings_save_and_delete_your_own_preset() {
    let mut h = harness();
    open_settings(&mut h);
    assert!(h.state().auto.find("dialog.comp.presetDelete").is_none(), "nothing of yours to delete yet");
    click(&mut h, "dialog.comp.presetSave");
    // Choose Name opens with the keyboard in its field.
    h.input_mut().events.push(Event::Text("1440p 60".into()));
    h.run_steps(2);
    key(&mut h, Key::Enter);
    assert_eq!(preset_names(&h), ["1440p 60"]);
    let q = &h.state().session.prefs.comp_presets[0];
    assert_eq!((q.width, q.height, q.pixel_aspect, q.frame_rate), (2560, 1440, 1.0, 60.0));
    assert_eq!(h.state().dialog, Some(Dialog::CompSettings), "Enter in the name field doesn't press OK");
    assert!(h.state().auto.find("dialog.comp.presetName").is_none(), "the name row closes");
    // The comp's values are now that preset, so it can be deleted.
    click(&mut h, "dialog.comp.presetDelete");
    assert!(preset_names(&h).is_empty());
    assert!(h.state().auto.find("dialog.comp.presetDelete").is_none());
}

/// A built-in preset's name can't be taken; Cancel closes the name row without saving.
#[test]
fn a_preset_name_cannot_be_a_built_in_one() {
    let mut h = harness();
    open_settings(&mut h);
    click(&mut h, "dialog.comp.presetSave");
    click(&mut h, "dialog.comp.presetName");
    h.input_mut().events.push(Event::Text("HDTV 1080 24".into()));
    h.run_steps(2);
    click(&mut h, "dialog.comp.presetNameSave");
    assert!(preset_names(&h).is_empty(), "the built-in name is refused");
    click(&mut h, "dialog.comp.presetNameCancel");
    assert!(h.state().auto.find("dialog.comp.presetName").is_none());
    assert!(preset_names(&h).is_empty());
}

/// Cancel and OK stay where they are when switching between Basic, Advanced and 3D Renderer,
/// at any interface scale: the tabs' heights are measured, not assumed.
#[test]
fn composition_settings_buttons_stay_put_between_tabs() {
    for zoom in [1.0, 1.3] {
        let mut h = harness();
        h.ctx.set_zoom_factor(zoom);
        h.run_steps(3);
        open_settings(&mut h);
        let rects = |h: &Harness<'_, EffectcraftApp>| {
            ["dialog.comp.ok", "dialog.comp.cancel", "dialog.comp.name"].map(|id| h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}")).rect)
        };
        let basic = rects(&h);
        for tab in ["advanced", "renderer", "basic"] {
            click(&mut h, &format!("dialog.comp.tab.{tab}"));
            assert_eq!(rects(&h), basic, "{tab} at {zoom}x");
        }
    }
}
