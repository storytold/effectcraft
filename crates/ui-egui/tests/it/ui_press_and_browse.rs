//! Drop-downs open on the press and choose on the release, as After Effects' do (#2686): the
//! timeline's blend-mode cell keeps its list up while the button is held, highlights the entry
//! under the pointer and applies the one the button comes up over; the header's » menu stays
//! up when the button comes back up over its trigger, and the next press puts it away. Both also
//! open from the keyboard (Space / Enter on the focused control), a touch tap opens the list once,
//! and Escape cancels a browsed list without choosing.

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

fn key(key: egui::Key) -> Event {
    Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }
}

/// Space and Enter on the focused » button open the workspace menu, and Escape puts it away
/// without changing the workspace: press-and-browse must not make the menu mouse-only (#2686).
#[test]
fn the_workspaces_menu_opens_from_the_keyboard_and_escape_cancels() {
    let (mut h, _id) = harness();
    let workspace = h.state().ui.workspace.clone();
    // Tab (or an assistive technology) puts the keyboard on the » button.
    h.ctx.memory_mut(|m| m.request_focus(egui::Id::unique("ws-more")));
    h.run_steps(1);
    h.event(key(egui::Key::Space));
    h.run_steps(3);
    assert!(h.query_by_label("Minimal").is_some(), "Space on the focused button opens the menu");
    h.event(key(egui::Key::Escape));
    h.run_steps(3);
    assert!(h.query_by_label("Minimal").is_none(), "Escape cancels it");
    assert_eq!(h.state().ui.workspace, workspace, "without choosing a workspace");
    h.ctx.memory_mut(|m| m.request_focus(egui::Id::unique("ws-more")));
    h.run_steps(1);
    h.event(key(egui::Key::Enter));
    h.run_steps(3);
    assert!(h.query_by_label("Minimal").is_some(), "Enter opens it too");
}

/// A tap has no hover before it and arrives as a touch event plus the pointer events egui-winit
/// translates it into: it must open the blend-mode list once, and the tap that opened it chooses
/// nothing (#2686).
#[test]
fn a_touch_tap_opens_the_blend_mode_list_once() {
    let (mut h, id) = harness();
    let trigger = rect(&h, &format!("timeline.layer.{id}.mode")).center();
    let before = blend(&h, id);
    let touch = |phase| Event::Touch { device_id: egui::TouchDeviceId(0), id: egui::TouchId(0), phase, pos: trigger, force: None };
    let press = |pressed| Event::PointerButton { pos: trigger, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    // egui-winit puts the touch and the pointer events it translates it into in one frame —
    // the pointer only appears at the tap, so there is no hover before it — and they go into
    // one pass here too (`Harness::step` would otherwise give each its own).
    h.input_mut().events.push(touch(egui::TouchPhase::Start));
    h.input_mut().events.push(Event::PointerMoved(trigger));
    h.input_mut().events.push(press(true));
    h.step();
    h.run_steps(2);
    assert!(h.query_by_label("Multiply").is_some(), "the tap opens the list");
    assert_eq!(blend(&h, id), before, "and chooses nothing yet");
    h.input_mut().events.push(touch(egui::TouchPhase::End));
    h.input_mut().events.push(press(false));
    h.input_mut().events.push(Event::PointerGone);
    h.step();
    h.run_steps(2);
    assert!(h.query_by_label("Multiply").is_some(), "the tap leaves the list up, as a click does");
    assert_eq!(blend(&h, id), before, "and chooses nothing");
}

/// Escape cancels a browsed list without applying the entry the pointer rests on, even while the
/// button is still down (#2686).
#[test]
fn escape_cancels_the_browsed_blend_mode_list() {
    let (mut h, id) = harness();
    let trigger = rect(&h, &format!("timeline.layer.{id}.mode")).center();
    let before = blend(&h, id);
    button(&mut h, trigger, true);
    // (The accessibility tree is one pass behind the drawing.)
    h.run_steps(2);
    assert!(h.query_by_label("Multiply").is_some(), "the press opens the list");
    let row = h.query_by_label("Multiply").expect("the list is open").rect().center();
    h.event(Event::PointerMoved(row));
    h.step();
    h.event(key(egui::Key::Escape));
    h.step();
    h.run_steps(2);
    assert!(h.query_by_label("Multiply").is_none(), "Escape closes the list");
    assert_eq!(blend(&h, id), before, "without applying the entry under the pointer");
    button(&mut h, trigger, false);
    h.run_steps(2);
    assert!(h.query_by_label("Multiply").is_none(), "and the release that follows keeps it closed");
}
