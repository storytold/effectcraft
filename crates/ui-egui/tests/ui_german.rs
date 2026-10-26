//! German preferences and dialogs through the real frame loop, with an isolated in-memory store.
use std::sync::Arc;

use effectcraft_engine::Session;
use effectcraft_engine::config::MemoryConfig;
use effectcraft_ui_egui::{EffectcraftApp, menus};
use egui::{Event, PointerButton, Shape, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

/// Send a primary click through the frame loop, then allow the dialog to settle.
fn click_at(h: &mut Harness<'_, EffectcraftApp>, at: egui::Pos2) {
    h.input_mut().events.push(Event::PointerMoved(at));
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Default::default() });
        h.step();
    }
    h.run_steps(3);
}

/// Click the centre of a registered automation element without relying on its translated label.
fn click(h: &mut Harness<'_, EffectcraftApp>, id: &str) {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("missing {id}")).clone();
    click_at(h, pos2(e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0));
}

/// Open and scroll the real language picker before selecting the visible language label.
fn language_choice(h: &mut Harness<'_, EffectcraftApp>, label: &str) {
    click(h, "settings.general.language");
    let picker = h.state().auto.find("settings.general.language").unwrap();
    let popup_top = pos2(picker.rect[0] + picker.rect[2] / 2.0, picker.rect[1] + picker.rect[3] + 20.0);
    // The language list now has twelve entries, so its later choices need scrolling.
    h.input_mut().events.push(Event::PointerMoved(popup_top));
    h.input_mut().events.push(Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, if label == "Deutsch" { -300.0 } else { 300.0 }),
        modifiers: Default::default(),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Wheel,
    });
    h.run_steps(20);
    let at = h.query_by_label(label).unwrap().rect().center();
    click_at(h, at);
}

/// Invoke an application command and advance enough frames to expose its dialog.
fn invoke(h: &mut Harness<'_, EffectcraftApp>, id: &str, params: serde_json::Value) {
    let ctx = h.ctx.clone();
    menus::invoke(h.state_mut(), &ctx, id, params).unwrap();
    h.run_steps(3);
}

/// Extract the text actually painted in the latest frame, including nested shape groups.
fn texts(h: &Harness<'_, EffectcraftApp>) -> Vec<String> {
    /// Visit a painted shape tree and append each text galley's source string.
    fn collect(shape: &Shape, out: &mut Vec<String>) {
        match shape {
            Shape::Text(t) => out.push(t.galley.job.text.clone()),
            Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, out);
                }
            }
            _ => {}
        }
    }
    let mut out = vec![];
    for shape in &h.output().shapes {
        collect(&shape.shape, &mut out);
    }
    out
}

/// Switching English/German updates the live interface and survives reopening the saved store.
#[test]
fn language_picker_switches_live_and_reloads_german_in_a_new_app() {
    let store = Arc::new(MemoryConfig::default());
    let mut session = Session { config: Some(store.clone()), ..Default::default() };
    session.execute("prefs.set", json!({"key":"general.language", "value":"en"})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_eframe(|_| EffectcraftApp::new(session));
    h.run_steps(3);
    invoke(&mut h, "app.settings", json!({"page":"general"}));
    assert!(texts(&h).iter().any(|s| s == "Language:"));
    language_choice(&mut h, "Deutsch");
    assert_eq!(h.state().session.prefs.general.language, "de");
    assert!(texts(&h).iter().any(|s| s == "Sprache:"));
    assert!(h.query_by_label("Abbrechen").is_some());
    assert!(h.state().auto.find("settings.general.language").is_some());

    language_choice(&mut h, "English");
    assert_eq!(h.state().session.prefs.general.language, "en");
    assert!(texts(&h).iter().any(|s| s == "Language:"));
    assert!(h.query_by_label("Cancel").is_some());

    language_choice(&mut h, "Deutsch");
    click(&mut h, "settings.ok");
    assert_eq!(h.state().dialog, None);
    drop(h);
    let mut restarted = Session { config: Some(store), ..Default::default() };
    restarted.load_settings();
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_eframe(|_| EffectcraftApp::new(restarted));
    h.run_steps(3);
    invoke(&mut h, "app.settings", json!({"page":"general"}));
    assert_eq!(h.state().session.prefs.general.language, "de");
    assert!(texts(&h).iter().any(|s| s == "Sprache:"));
}

/// German composition labels leave automation IDs and user-chosen project names unchanged.
#[test]
fn german_composition_form_keeps_ids_and_project_names() {
    let mut s = Session::default();
    s.execute("prefs.set", json!({"key":"general.language", "value":"de"})).unwrap();
    s.execute("comp.new", json!({"name":"File", "width":320,"height":180})).unwrap();
    s.execute("layer.newSolid", json!({"name":"Layer"})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.run_steps(3);
    invoke(&mut h, "comp.settings", json!({}));
    assert!(texts(&h).iter().any(|s| s == "Kompositionsname:"));
    assert!(h.state().auto.find("dialog.comp.name").is_some());
    assert!(texts(&h).iter().any(|s| s == "File"));
    h.state_mut().dialog = None;
    let comp_id = h.state().session.active_comp_id().unwrap();
    assert_eq!(h.state().session.project.item(comp_id).unwrap().name, "File");
}
