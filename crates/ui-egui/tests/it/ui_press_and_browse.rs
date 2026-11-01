//! Drop-downs open on the press and choose on the release, as After Effects' do (#2686): the
//! timeline's blend-mode cell keeps its list up while the button is held, highlights the entry
//! under the pointer and applies the one the button comes up over; the header's » menu stays
//! up when the button comes back up over its trigger, and the next press puts it away.

use effectcraft_engine::Session;
use effectcraft_engine::color::BlendMode;
use effectcraft_engine::project::LayerId;
use effectcraft_ui_egui::EffectcraftApp;
use egui::{Event, Modifiers, Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

/// Comp "Main" with one solid A, and the timeline's Modes column shown.
fn harness() -> (Harness<'static, EffectcraftApp>, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 320, "height": 180, "frameRate": 30, "duration": 4})).unwrap();
    let id = s.execute("layer.newSolid", json!({"name": "A", "color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("edit.deselectAll", json!({})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_eframe(|_| EffectcraftApp::new(s));
    h.state_mut().ui.timeline.show_modes = true;
    h.run_steps(3);
    (h, id)
}

fn rect(h: &Harness<'_, EffectcraftApp>, id: &str) -> Rect {
    let e = h.state().auto.find(id).unwrap_or_else(|| panic!("no {id}")).clone();
    Rect::from_min_size(pos2(e.rect[0], e.rect[1]), vec2(e.rect[2], e.rect[3]))
}

fn button(h: &mut Harness<'_, EffectcraftApp>, at: Pos2, pressed: bool) {
    if pressed {
        h.event(Event::PointerMoved(at));
        h.step();
    }
    h.event(Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    h.step();
}

fn blend(h: &Harness<'_, EffectcraftApp>, id: u64) -> BlendMode {
    h.state().session.active_comp().unwrap().layer(LayerId(id)).unwrap().blend_mode
}

/// The press opens the list, moving over an entry keeps it up and the release over one applies it.
#[test]
fn the_blend_mode_list_browses_on_the_press_and_applies_on_the_release() {
    let (mut h, id) = harness();
    let trigger = rect(&h, &format!("timeline.layer.{id}.mode")).center();
    button(&mut h, trigger, true);
    // (The accessibility tree is one pass behind the drawing.)
    h.run_steps(2);
    assert!(h.query_by_label("Multiply").is_some(), "the press opens the list, while the button is still down");
    let row = h.query_by_label("Multiply").expect("the list is open").rect().center();
    h.event(Event::PointerMoved(row));
    h.step();
    assert!(h.query_by_label("Multiply").is_some(), "moving onto an entry keeps the list open");
    button(&mut h, row, false);
    h.run_steps(2);
    assert_eq!(blend(&h, id), BlendMode::Multiply, "the release over the entry applies it");
    assert!(h.query_by_label("Dissolve").is_none(), "and closes the list");
}

/// A release back over the trigger chooses nothing and leaves the list up; the next press closes
/// it, so the control is its own toggle (as in After Effects). The workspaces menu is short and
/// opens below the header, so it never covers the » button the way a long list can.
#[test]
fn a_release_back_over_the_trigger_leaves_the_list_up() {
    let (mut h, _id) = harness();
    let trigger = rect(&h, "header.workspaces").center();
    let workspace = h.state().ui.workspace.clone();
    button(&mut h, trigger, true);
    h.run_steps(2);
    assert!(h.query_by_label("Minimal").is_some(), "the press opens the list, while the button is still down");
    button(&mut h, trigger, false);
    h.run_steps(2);
    assert_eq!(h.state().ui.workspace, workspace, "the release back over the trigger chooses nothing");
    assert!(h.query_by_label("Minimal").is_some(), "and leaves the list up");
    button(&mut h, trigger, true);
    h.run_steps(2);
    assert!(h.query_by_label("Minimal").is_none(), "the next press closes it");
}
